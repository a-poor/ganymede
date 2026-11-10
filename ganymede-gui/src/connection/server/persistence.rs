use super::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Record {
    version: u32,
    id: String,
    launch: Launch,
    address: String,
    info: PathBuf,
    pid: u32,
    started: String,
}

pub(super) fn directory() -> PathBuf {
    crate::storage::Paths::current().servers
}

fn private(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return false;
        }
    }
    true
}

fn secure_directory(path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("Local server storage cannot be a symbolic link.".into());
    }
    fs::create_dir_all(path).map_err(|_| "Could not create local server storage.".to_owned())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Could not secure local server storage.".to_owned())?;
    }
    if !private(path) {
        return Err("Local server storage is not private.".into());
    }
    Ok(())
}

impl Record {
    pub(super) fn adopt(&self, server: &mut Managed) -> Result<(), String> {
        server.config = self.config()?;
        server.launch = self.launch.clone();
        server.info = self.info.clone();
        Ok(())
    }

    pub(super) async fn replacement(
        pending: &PendingServer,
        previous: &Self,
    ) -> Result<Self, String> {
        Self::from_pending(pending, previous.id.clone()).await
    }
    pub(super) async fn from_pending(pending: &PendingServer, id: String) -> Result<Self, String> {
        let started = JupyterRestClient::new(pending.config.clone())
            .map_err(|_| "Could not verify local server.".to_owned())?
            .server_status()
            .await
            .map_err(|_| "Could not verify local server identity.".to_owned())?
            .started;
        Ok(Self {
            version: 1,
            id,
            launch: pending.launch.clone(),
            address: pending.address.clone(),
            info: pending.info.clone(),
            pid: pending.child.as_ref().unwrap().id(),
            started,
        })
    }
    pub(super) fn save(&self, path: &Path) -> Result<(), String> {
        let parent = path.parent().ok_or("Invalid local server storage path.")?;
        secure_directory(parent)?;
        let temp = parent.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> std::io::Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
            fs::rename(&temp, path)?;
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result.map_err(|_| "Could not save local server settings.".into())
    }
    pub(super) fn load(path: &Path) -> Option<Self> {
        if !private(path) || !private(path.parent()?) {
            return None;
        }
        let record: Self = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        if record.version != 1
            || uuid::Uuid::parse_str(&record.id).is_err()
            || path.file_stem()?.to_str()? != record.id
            || record.launch.token.len() < 32
            || record.started.is_empty()
        {
            return None;
        }
        let config = record.config().ok()?;
        let url = config.base_url();
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || url.path() != "/"
            || url.port() != Some(record.launch.port)
            || record.launch.port == 0
        {
            return None;
        }
        Some(record)
    }
    fn config(&self) -> Result<ServerConfig, String> {
        parse_connection(&self.address, &self.launch.token).map(|(_, c)| c)
    }
    pub(super) fn lock(path: &Path) -> Result<File, String> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path.with_extension("lock"))
            .map_err(|_| "Could not lock local server settings.".to_owned())?;
        file.try_lock().map_err(|_| {
            "Another Ganymede window or process is managing this server. Try again shortly."
                .to_owned()
        })?;
        Ok(file)
    }
    pub(super) async fn verify(&self) -> LocalState {
        let port = self.launch.port;
        let socket = tokio::task::spawn_blocking(move || {
            std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
                Duration::from_secs(2),
            )
        })
        .await;
        match socket {
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                return LocalState::Stopped;
            }
            Ok(Ok(_)) => (),
            _ => return LocalState::Unverified,
        }
        // A reused port or PID does not establish identity. Require the private runtime
        // metadata AND the original launch timestamp returned by the authenticated API.
        if !private(&self.info) || !self.info.parent().is_some_and(private) {
            return LocalState::Unverified;
        }
        let data = fs::read(&self.info)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        let Some(data) = data else {
            return LocalState::Unverified;
        };
        if data["pid"].as_u64() != Some(self.pid as u64)
            || data["token"].as_str() != Some(&self.launch.token)
            || data["url"].as_str() != Some(&self.address)
            || data["root_dir"].as_str() != Some(&self.launch.folder)
        {
            return LocalState::Unverified;
        }
        let Ok(mut config) = self.config() else {
            return LocalState::Unverified;
        };
        config.request_timeout = Duration::from_secs(3);
        let Ok(client) = JupyterRestClient::new(config) else {
            return LocalState::Unverified;
        };
        match client.server_status().await {
            Ok(status) if status.started == self.started => LocalState::Running,
            _ => LocalState::Unverified,
        }
    }
}

impl PendingServer {
    pub(in crate::connection) fn id(&self) -> Option<String> {
        self.record.as_ref().map(|(_, r)| r.id.clone())
    }

    pub(in crate::connection) async fn persist(&mut self, directory: &Path) -> Result<(), String> {
        let record = Record::from_pending(self, uuid::Uuid::new_v4().to_string()).await?;
        let path = directory.join(format!("{}.json", record.id));
        record.save(&path)?;
        self.record = Some((path, record));
        Ok(())
    }
}

pub(crate) fn load_servers(ids: &[String]) -> std::collections::HashMap<String, ManagedServer> {
    load_from(&directory(), ids)
}

fn load_from(directory: &Path, ids: &[String]) -> std::collections::HashMap<String, ManagedServer> {
    let mut servers = std::collections::HashMap::new();
    for id in ids {
        if uuid::Uuid::parse_str(id).is_err() {
            continue;
        }
        let path = directory.join(format!("{id}.json"));
        let Some(record) = Record::load(&path) else {
            continue;
        };
        let Ok(config) = record.config() else {
            continue;
        };
        let server = ManagedServer(std::sync::Arc::new(std::sync::Mutex::new(Managed {
            child: None,
            launch: record.launch.clone(),
            info: record.info.clone(),
            config,
            busy: false,
            state: LocalState::Unverified,
            epoch: 0,
            record: Some((path, record.clone())),
        })));
        servers.entry(record.address).or_insert(server);
    }
    servers
}

impl ManagedServer {
    pub(crate) fn id(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap()
            .record
            .as_ref()
            .map(|(_, r)| r.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) -> (PathBuf, Record) {
        let id = uuid::Uuid::new_v4().to_string();
        let path = root.join(format!("{id}.json"));
        let record = Record {
            version: 1,
            id,
            launch: Launch {
                mode: Environment::Custom,
                python: "python3".into(),
                folder: "/project".into(),
                token: uuid::Uuid::new_v4().to_string(),
                port: 18888,
            },
            address: "http://127.0.0.1:18888/".into(),
            info: root.join("runtime.json"),
            pid: 123,
            started: "2026-11-03T00:00:00Z".into(),
        };
        (path, record)
    }

    #[test]
    fn records_are_private_versioned_and_locked() {
        let root = std::env::temp_dir().join(format!("ganymede-record-{}", uuid::Uuid::new_v4()));
        let (path, record) = fixture(&root);
        record.save(&path).unwrap();
        assert!(private(&root));
        assert!(private(&path));
        assert!(Record::load(&path).is_some());
        let lock = Record::lock(&path).unwrap();
        assert!(Record::lock(&path).is_err());
        drop(lock);
        assert!(Record::lock(&path).is_ok());
        let mut invalid = record.clone();
        invalid.version = 999;
        invalid.save(&path).unwrap();
        assert!(Record::load(&path).is_none());
        invalid = record.clone();
        invalid.address = "https://example.com/".into();
        invalid.save(&path).unwrap();
        assert!(Record::load(&path).is_none());
        record.save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(Record::load(&path).is_none());
        }
        fs::write(&path, b"broken json").unwrap();
        assert!(Record::load(&path).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires GANYMEDE_TEST_PYTHON with jupyter-server and ipykernel"]
    async fn persisted_servers_reconcile_without_child_handles_and_reject_reused_ports() {
        let python = std::env::var("GANYMEDE_TEST_PYTHON").unwrap();
        let root =
            std::env::temp_dir().join(format!("ganymede-persisted-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let folder = root.to_string_lossy().into_owned();
        let mut pending =
            tokio::task::spawn_blocking(move || start(Environment::Custom, &python, &folder))
                .await
                .unwrap()
                .unwrap();
        validate(pending.config.clone()).await.unwrap();
        let storage = root.join("servers");
        pending.persist(&storage).await.unwrap();
        let id = pending.id().unwrap();
        let address = pending.address.clone();
        let original = pending.accept();
        let record_path = storage.join(format!("{id}.json"));
        let first = Record::load(&record_path).unwrap();
        let restored = load_from(&storage, std::slice::from_ref(&id))
            .remove(&address)
            .unwrap();
        assert!(restored.0.lock().unwrap().child.is_none());
        assert!(restored.reconcile().await == LocalState::Running);
        // Same runtime metadata and token are insufficient if launch identity differs.
        let mut wrong = first.clone();
        wrong.started.push_str("wrong");
        wrong.save(&record_path).unwrap();
        assert!(restored.reconcile().await == LocalState::Unverified);
        assert!(restored.change(false).await.is_err());
        assert!(validate(original.connection().0).await.is_ok());
        first.save(&record_path).unwrap();
        assert!(restored.reconcile().await == LocalState::Running);
        restored.change(false).await.unwrap();
        assert!(restored.reconcile().await == LocalState::Stopped);
        let next = load_from(&storage, std::slice::from_ref(&id))
            .remove(&address)
            .unwrap();
        assert!(next.reconcile().await == LocalState::Stopped);
        // An unrelated listener on the old port must never be shut down or replaced.
        let occupied = std::net::TcpListener::bind(("127.0.0.1", first.launch.port)).unwrap();
        assert!(next.reconcile().await == LocalState::Unverified);
        assert!(next.change(false).await.is_err());
        assert!(next.change(true).await.is_err());
        drop(occupied);
        let lock = Record::lock(&record_path).unwrap();
        assert!(next.change(true).await.is_err());
        drop(lock);
        next.change(true).await.unwrap();
        let current = Record::load(&record_path).unwrap();
        assert_eq!(current.id, first.id);
        assert_eq!(current.address, first.address);
        assert_ne!(current.started, first.started);
        let client = JupyterRestClient::new(next.connection().0).unwrap();
        assert!(client.list_kernels().await.unwrap().is_empty());
        // Another old app instance re-reads the new generation before Stop.
        restored.change(false).await.unwrap();
        assert!(next.reconcile().await == LocalState::Stopped);
        for record in [first, current] {
            let _ = fs::remove_dir_all(record.info.parent().unwrap());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
