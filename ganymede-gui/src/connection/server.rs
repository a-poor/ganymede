mod persistence;
pub(crate) use persistence::load_servers;
pub(super) fn storage_directory() -> PathBuf {
    persistence::directory()
}
use ganymede_core::jupyter::{JupyterRestClient, ServerConfig};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// A successful connection stores only the sanitized endpoint in recent history.
pub(super) fn parse_connection(
    address: &str,
    token: &str,
) -> Result<(String, ServerConfig), String> {
    let mut url = reqwest::Url::parse(address.trim())
        .map_err(|_| "Enter a valid HTTP or HTTPS server URL.".to_owned())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use an HTTP(S) URL and put credentials in the token field.".into());
    }
    let mut pasted_token = None;
    for (key, value) in url.query_pairs() {
        if key != "token" {
            return Err("Use the Jupyter server URL, without other query parameters.".into());
        }
        pasted_token = Some(value.into_owned());
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/');
    let root = path
        .strip_suffix("/lab")
        .or_else(|| path.strip_suffix("/tree"))
        .unwrap_or(path)
        .to_owned();
    url.set_path(&format!("{root}/"));
    let address = url.to_string();
    let mut config =
        ServerConfig::new(&address).map_err(|_| "Enter a valid Jupyter server URL.".to_owned())?;
    let token = if token.trim().is_empty() {
        pasted_token.as_deref().unwrap_or("")
    } else {
        token.trim()
    };
    if !token.is_empty() {
        config = config
            .with_token(token)
            .map_err(|_| "The token contains invalid characters.".to_owned())?;
    }
    Ok((address, config))
}

pub(super) async fn validate(mut config: ServerConfig) -> Result<String, String> {
    config.connect_timeout = Duration::from_secs(5);
    config.request_timeout = Duration::from_secs(10);
    // An authenticated endpoint verifies credentials and discovers the default kernel.
    let client = JupyterRestClient::new(config)
        .map_err(|_| "Could not create the connection.".to_owned())?;
    let specs = client.list_kernel_specs().await.map_err(|_| {
        "Could not connect. Check the server URL, token, and whether the server is running."
            .to_owned()
    })?;
    Ok(specs.default)
}

/// Until accepted by the UI, cancellation/failure cleans up a server that has
/// never been exposed to notebook execution. Accepted servers are left running.
pub(super) struct PendingServer {
    record: Option<(PathBuf, persistence::Record)>,
    launch: Launch,
    child: Option<Child>,
    pub info: PathBuf,
    directory: PathBuf,
    pub address: String,
    pub config: ServerConfig,
}
impl PendingServer {
    pub fn accept(mut self) -> ManagedServer {
        ManagedServer(std::sync::Arc::new(std::sync::Mutex::new(Managed {
            record: self.record.take(),
            state: LocalState::Running,
            epoch: 0,
            child: self.child.take(),
            launch: self.launch.clone(),
            config: self.config.clone(),
            info: self.info.clone(),
            busy: false,
        })))
    }
}
impl Drop for PendingServer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Terminate any child processes of the unaccepted launch.
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&self.directory);
            if let Some((path, _)) = &self.record {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

pub(super) fn read_local(info: &std::path::Path) -> Option<(String, ServerConfig)> {
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(info).ok()?).ok()?;
    let address = data.get("url")?.as_str()?;
    let token = data.get("token")?.as_str()?;
    parse_connection(address, token).ok()
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum Environment {
    Global,
    Venv,
    Custom,
}
impl Environment {
    pub const ALL: [Self; 3] = [Self::Global, Self::Venv, Self::Custom];
    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "Global Python",
            Self::Venv => "Virtual environment",
            Self::Custom => "Custom Python",
        }
    }
}

// Fixed script: user paths are separate process arguments, never interpolated code.
const PYTHON_LAUNCH: &str = "import importlib.util,sys,runpy; sys.exit(81) if importlib.util.find_spec('jupyter_server') is None else None; sys.exit(82) if importlib.util.find_spec('ipykernel') is None else None; sys.argv[0]='jupyter-server'; runpy.run_module('jupyter_server',run_name='__main__')";

fn launch_command(
    mode: Environment,
    value: &str,
    root: &std::path::Path,
) -> Result<(std::ffi::OsString, Vec<&'static str>), String> {
    let python_args = vec!["-c", PYTHON_LAUNCH];
    match mode {
        Environment::Global => Ok(("python3".into(), python_args)),
        Environment::Venv | Environment::Custom => {
            if value.trim().is_empty() {
                return Err("Enter an environment or Python executable path.".into());
            }
            let mut path = PathBuf::from(value.trim());
            if mode == Environment::Venv {
                path = path.join(if cfg!(windows) {
                    "Scripts/python.exe"
                } else {
                    "bin/python"
                });
            }
            // Preserve venv symlinks; relative paths are resolved from the selected project.
            if path.is_relative()
                && (mode == Environment::Venv || value.contains(std::path::MAIN_SEPARATOR))
            {
                path = root.join(path);
            }
            Ok((path.into_os_string(), python_args))
        }
    }
}

pub(super) fn start(
    mode: Environment,
    python: &str,
    folder: &str,
) -> Result<PendingServer, String> {
    start_launch(mode, python, folder, None)
}

fn start_launch(
    mode: Environment,
    python: &str,
    folder: &str,
    reuse: Option<&Launch>,
) -> Result<PendingServer, String> {
    let root = std::fs::canonicalize(folder.trim())
        .map_err(|_| "Choose an existing project folder.".to_owned())?;
    if !root.is_dir() {
        return Err("The project root must be a folder.".into());
    }
    let (executable, arguments) = launch_command(mode, python, &root)?;
    let directory = std::env::temp_dir().join(format!("ganymede-server-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory)
        .map_err(|_| "Could not create the server runtime directory.".to_owned())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "Could not secure the runtime directory.".to_owned())?;
    }
    let token = reuse
        .map(|l| l.token.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let port = reuse.map(|l| l.port).unwrap_or(0);
    let mut command = Command::new(executable);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command
        .args(arguments)
        .args([
            "--no-browser",
            "--ServerApp.ip=127.0.0.1",
            "--ServerApp.base_url=/",
            "--ServerApp.port_retries=0",
        ])
        .arg(format!("--ServerApp.port={port}"))
        .arg(format!("--ServerApp.root_dir={}", root.display()))
        .env("JUPYTER_RUNTIME_DIR", &directory)
        .env("JUPYTER_TOKEN", &token)
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(_) => {
            let _ = std::fs::remove_dir(&directory);
            return Err("Could not launch the environment. Check that Python is installed and the path is correct.".into());
        }
    };
    let info = directory.join(format!("jpserver-{}.json", child.id()));
    let mut pending = PendingServer {
        record: None,
        launch: Launch {
            mode,
            python: python.into(),
            folder: root.to_string_lossy().into_owned(),
            token,
            port,
        },
        child: Some(child),
        info,
        directory,
        address: String::new(),
        config: ServerConfig::new("http://127.0.0.1/").unwrap(),
    };
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) {
        if let Some(status) = pending
            .child
            .as_mut()
            .unwrap()
            .try_wait()
            .map_err(|_| "Could not check server startup.".to_owned())?
        {
            return Err(match status.code() {
                Some(81) => "The selected Python environment is missing jupyter-server. Install it there, then try again.",
                Some(82) => "The selected Python environment is missing ipykernel. Install it there, then try again.",
                _ => "Jupyter exited during startup. Check the selected environment and its Jupyter configuration.",
            }.into());
        }
        let info = std::fs::read_dir(&pending.directory)
            .ok()
            .and_then(|entries| {
                entries.filter_map(Result::ok).find_map(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    (name.starts_with("jpserver-") && name.ends_with(".json")).then(|| entry.path())
                })
            });
        if let Some(info) = info
            && let Some((address, config)) = read_local(&info)
        {
            pending.info = info;
            pending.launch.port = config.base_url().port_or_known_default().unwrap();
            pending.address = address;
            pending.config = config;
            return Ok(pending);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Jupyter startup timed out. Check the environment and its installed packages.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn environment_commands_preserve_venv_paths_without_a_shell() {
        let root = std::env::temp_dir().join("project with spaces");
        let (exe, args) = launch_command(Environment::Venv, ".venv", &root).unwrap();
        assert_eq!(
            PathBuf::from(exe),
            root.join(".venv").join(if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            })
        );
        assert_eq!(args, ["-c", PYTHON_LAUNCH]);
        assert_eq!(
            launch_command(Environment::Global, "", &root).unwrap().0,
            "python3"
        );
        assert!(launch_command(Environment::Custom, "", &root).is_err());
    }
    #[test]
    fn pasted_urls_strip_tokens_and_preserve_deployment_prefixes() {
        let (url, config) =
            parse_connection("https://host/user/alice/lab?token=secret#fragment", "").unwrap();
        assert_eq!(url, "https://host/user/alice/");
        assert!(!format!("{config:?}").contains("secret"));
        assert!(parse_connection("https://host/?signed=secret", "").is_err());
        assert!(parse_connection("https://user:secret@host/", "").is_err());
        assert!(parse_connection("file:///tmp/file", "").is_err());
        assert_eq!(
            parse_connection("http://localhost:8888/", "abc").unwrap().0,
            "http://localhost:8888/"
        );
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    #[tokio::test]
    async fn connections_keep_authentication_and_default_kernels_separate() {
        async fn mock(
            token: &'static str,
            kernel: &'static str,
        ) -> (String, tokio::task::JoinHandle<()>) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = format!("http://{}/user/test/", listener.local_addr().unwrap());
            let task = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                    let mut chunk = [0; 1024];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let request = String::from_utf8(bytes).unwrap();
                assert!(request.starts_with("GET /user/test/api/kernelspecs "));
                assert!(
                    request
                        .to_lowercase()
                        .contains(&format!("authorization: token {token}\r\n"))
                );
                let body = serde_json::json!({"default":kernel,"kernelspecs":{}}).to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            });
            (address, task)
        }
        let (a, one) = mock("first-secret", "python3").await;
        let (b, two) = mock("second-secret", "julia").await;
        let (_, a) = parse_connection(&a, "first-secret").unwrap();
        let (_, b) = parse_connection(&b, "second-secret").unwrap();
        let (a, b) = tokio::join!(validate(a), validate(b));
        assert_eq!(a.unwrap(), "python3");
        assert_eq!(b.unwrap(), "julia");
        one.await.unwrap();
        two.await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires GANYMEDE_TEST_PYTHON with jupyter-server and ipykernel"]
    async fn local_server_uses_selected_folder_and_cleans_up_before_acceptance() {
        let python = std::env::var("GANYMEDE_TEST_PYTHON").unwrap();
        check_local_server(Environment::Custom, python).await;
    }

    #[tokio::test]
    #[ignore = "requires GANYMEDE_TEST_PYTHON with jupyter-server and ipykernel"]
    async fn managed_server_stops_and_restarts_without_running_kernels() {
        let python = std::env::var("GANYMEDE_TEST_PYTHON").unwrap();
        let root =
            std::env::temp_dir().join(format!("ganymede-lifecycle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("example.txt"), "preserved").unwrap();
        let folder = root.to_string_lossy().into_owned();
        let pending =
            tokio::task::spawn_blocking(move || start(Environment::Custom, &python, &folder))
                .await
                .unwrap()
                .unwrap();
        validate(pending.config.clone()).await.unwrap();
        let client = JupyterRestClient::new(pending.config.clone()).unwrap();
        let first_info = pending.info.clone();
        let server = pending.accept();
        assert!(server.state() == LocalState::Running);
        assert!(server.change(true).await.is_err());
        client.start_kernel("python3", None).await.unwrap();
        server.change(false).await.unwrap();
        assert!(server.state() == LocalState::Stopped);
        assert!(client.list_kernels().await.is_err());
        assert!(server.change(false).await.is_err());
        let port = server.connection().0.base_url().port().unwrap();
        let occupied = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        assert!(server.change(true).await.is_err());
        assert!(server.state() == LocalState::Stopped);
        assert_eq!(occupied.local_addr().unwrap().port(), port);
        drop(occupied);
        server.change(true).await.unwrap();
        // The original client and authentication still work; no execution was replayed.
        assert!(server.state() == LocalState::Running);
        assert!(client.list_kernels().await.unwrap().is_empty());
        assert_eq!(
            client
                .get_contents("example.txt", true)
                .await
                .unwrap()
                .content,
            "preserved"
        );
        let second_info = server.connection().1;
        server.change(false).await.unwrap();
        for info in [first_info, second_info] {
            let _ = std::fs::remove_dir_all(info.parent().unwrap());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires GANYMEDE_TEST_PYTHON with venv support"]
    async fn startup_identifies_missing_python_and_packages() {
        let python = std::env::var("GANYMEDE_TEST_PYTHON").unwrap();
        let root =
            std::env::temp_dir().join(format!("ganymede-start-errors-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let folder = root.to_string_lossy().into_owned();
        let missing = root.join("missing-python").to_string_lossy().into_owned();
        let error = start(Environment::Custom, &missing, &folder).err().unwrap();
        assert!(error.contains("Could not launch the environment"));
        let env = root.join("venv");
        assert!(
            Command::new(python)
                .args(["-m", "venv", "--without-pip"])
                .arg(&env)
                .status()
                .unwrap()
                .success()
        );
        let env = env.to_string_lossy().into_owned();
        let error = start(Environment::Venv, &env, &folder).err().unwrap();
        assert!(error.contains("missing jupyter-server"), "{error}");
        // Make just jupyter_server discoverable in this disposable project.
        std::fs::write(root.join("jupyter_server.py"), "").unwrap();
        let error = start(Environment::Venv, &env, &folder).err().unwrap();
        assert!(error.contains("missing ipykernel"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
    }

    async fn check_local_server(mode: Environment, python: String) {
        let root = std::env::temp_dir().join(format!("ganymede-root-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("example.txt"), "fixture").unwrap();
        let folder = root.to_string_lossy().into_owned();
        let pending = tokio::task::spawn_blocking(move || start(mode, &python, &folder))
            .await
            .unwrap()
            .unwrap();
        let kernel = validate(pending.config.clone()).await.unwrap();
        assert!(!kernel.is_empty());
        let client = JupyterRestClient::new(pending.config.clone()).unwrap();
        let file = client.get_contents("example.txt", true).await.unwrap();
        assert_eq!(file.content, "fixture");
        let info = pending.info.clone();
        drop(pending);
        assert!(!info.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}

// Ownership is retained across workspace windows, but never inferred from a loopback URL.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Launch {
    mode: Environment,
    python: String,
    folder: String,
    token: String,
    port: u16,
}
struct Managed {
    record: Option<(PathBuf, persistence::Record)>,
    state: LocalState,
    epoch: u64,
    child: Option<Child>,
    launch: Launch,
    config: ServerConfig,
    info: PathBuf,
    busy: bool,
}
impl Managed {
    fn release_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Reap accepted processes without tying their lifetime to an app/window.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}
impl Drop for Managed {
    fn drop(&mut self) {
        self.release_child();
    }
}
#[derive(Clone)]
pub(crate) struct ManagedServer(std::sync::Arc<std::sync::Mutex<Managed>>);
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalState {
    Unverified,
    Running,
    Stopped,
    Busy,
}
impl ManagedServer {
    pub(super) fn launch_options(&self) -> (Environment, String, String) {
        let server = self.0.lock().unwrap();
        (
            server.launch.mode,
            server.launch.python.clone(),
            server.launch.folder.clone(),
        )
    }

    pub fn connection(&self) -> (ServerConfig, PathBuf) {
        let server = self.0.lock().unwrap();
        (server.config.clone(), server.info.clone())
    }
    pub fn state(&self) -> LocalState {
        let mut server = self.0.lock().unwrap();
        if server.busy {
            return LocalState::Busy;
        }
        if server
            .child
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_some())
        {
            server.child = None;
        }
        if server.record.is_some() {
            return server.state;
        }
        if server
            .child
            .as_mut()
            .is_some_and(|child| child.try_wait().ok().flatten().is_none())
        {
            LocalState::Running
        } else {
            LocalState::Stopped
        }
    }
    pub async fn reconcile(&self) -> LocalState {
        let (stored, epoch) = {
            let server = self.0.lock().unwrap();
            if server.busy {
                return LocalState::Busy;
            }
            (server.record.clone(), server.epoch)
        };
        let Some((path, _)) = stored else {
            return self.state();
        };
        let record = persistence::Record::load(&path);
        let state = if let Some(record) = &record {
            record.verify().await
        } else {
            LocalState::Unverified
        };
        let mut server = self.0.lock().unwrap();
        if !server.busy && server.epoch == epoch {
            server.state = state;
            if let Some(record) = record {
                let _ = record.adopt(&mut server);
                server.record = Some((path, record));
            }
        }
        if server.busy {
            LocalState::Busy
        } else {
            server.state
        }
    }
    pub async fn change(&self, restart: bool) -> Result<(), String> {
        {
            let mut server = self.0.lock().unwrap();
            if server.busy {
                return Err("A server operation is already in progress.".into());
            }
            server.busy = true;
            server.epoch = server.epoch.wrapping_add(1);
        }
        // Also release the operation lock if the async task is cancelled.
        struct ResetBusy(ManagedServer);
        impl Drop for ResetBusy {
            fn drop(&mut self) {
                self.0.0.lock().unwrap().busy = false;
            }
        }
        let _reset = ResetBusy(self.clone());
        let stored = self.0.lock().unwrap().record.clone();
        let _lock = if let Some((path, _)) = &stored {
            Some(persistence::Record::lock(path)?)
        } else {
            None
        };
        let state = if let Some((path, _)) = &stored {
            let record = persistence::Record::load(path)
                .ok_or("Local server settings are missing or invalid.")?;
            let state = record.verify().await;
            let mut server = self.0.lock().unwrap();
            record.adopt(&mut server)?;
            server.record = Some((path.clone(), record));
            state
        } else {
            let mut server = self.0.lock().unwrap();
            if server
                .child
                .as_mut()
                .is_some_and(|c| c.try_wait().ok().flatten().is_none())
            {
                LocalState::Running
            } else {
                LocalState::Stopped
            }
        };
        if state
            != if restart {
                LocalState::Stopped
            } else {
                LocalState::Running
            }
        {
            self.0.lock().unwrap().state = state;
            return Err("Could not verify the server state. Reconnect and try again.".into());
        }
        let (launch, config) = {
            let server = self.0.lock().unwrap();
            (server.launch.clone(), server.config.clone())
        };
        let result = self.change_inner(restart, launch, config).await;
        if result.is_ok() {
            self.0.lock().unwrap().state = if restart {
                LocalState::Running
            } else {
                LocalState::Stopped
            };
        }
        result
    }
    async fn change_inner(
        &self,
        restart: bool,
        launch: Launch,
        mut config: ServerConfig,
    ) -> Result<(), String> {
        if restart {
            let mut pending = tokio::task::spawn_blocking(move || {
                start_launch(launch.mode, &launch.python, &launch.folder, Some(&launch))
            })
            .await
            .map_err(|_| "Could not restart Jupyter.".to_owned())??;
            validate(pending.config.clone()).await?;
            let stored = self.0.lock().unwrap().record.clone();
            if let Some((path, record)) = stored {
                let record = persistence::Record::replacement(&pending, &record).await?;
                record.save(&path)?;
                self.0.lock().unwrap().record = Some((path, record));
            }
            let mut server = self.0.lock().unwrap();
            server.release_child();
            server.child = pending.child.take();
            server.info = pending.info.clone();
            return Ok(());
        }
        config.request_timeout = Duration::from_secs(10);
        JupyterRestClient::new(config)
            .map_err(|_| "Could not contact Jupyter.".to_owned())?
            .shutdown_server()
            .await
            .map_err(|_| {
                "Could not stop the server. Check its connection and try again.".to_owned()
            })?;
        for _ in 0..100 {
            let record = self.0.lock().unwrap().record.clone();
            let stopped = if let Some((_, record)) = record {
                record.verify().await == LocalState::Stopped
            } else {
                self.0
                    .lock()
                    .unwrap()
                    .child
                    .as_mut()
                    .is_none_or(|c| c.try_wait().ok().flatten().is_some())
            };
            if stopped {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("The server is still shutting down. Check its status before trying again.".into())
    }
}
