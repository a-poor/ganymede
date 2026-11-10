//! Platform storage locations and non-destructive migration of legacy Linux state.
use std::{env, ffi::OsString, path::PathBuf};

pub(crate) struct Paths {
    pub connections: PathBuf,
    pub preferences: PathBuf,
    pub workspaces: PathBuf,
    pub servers: PathBuf,
    #[cfg(target_os = "linux")]
    migrations: Vec<(PathBuf, PathBuf)>,
}

impl Paths {
    pub fn current() -> Self {
        Self::resolve(|key| env::var_os(key))
    }

    fn resolve(get: impl Fn(&str) -> Option<OsString>) -> Self {
        let home = get("HOME").map(PathBuf::from).unwrap_or_else(env::temp_dir);
        let legacy = home.join(if cfg!(target_os = "macos") {
            "Library/Application Support/Ganymede/connections.json"
        } else {
            ".config/ganymede/connections.json"
        });
        let connection_override = get("GANYMEDE_CONNECTIONS_FILE").map(PathBuf::from);
        let preference_override = get("GANYMEDE_PREFERENCES_FILE").map(PathBuf::from);
        #[cfg(target_os = "linux")]
        let (connections, preferences) = {
            let base = |key, fallback| {
                get(key)
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .unwrap_or_else(|| home.join(fallback))
                    .join("ganymede")
            };
            (
                base("XDG_STATE_HOME", ".local/state").join("connections.json"),
                base("XDG_CONFIG_HOME", ".config").join("preferences.json"),
            )
        };
        #[cfg(not(target_os = "linux"))]
        let (connections, preferences) =
            (legacy.clone(), legacy.with_extension("preferences.json"));
        // Preserve the existing development override's isolation of all state,
        // including preferences unless they have their own explicit override.
        let preferences = preference_override.clone().unwrap_or_else(|| {
            connection_override
                .as_ref()
                .map(|p| p.with_extension("preferences.json"))
                .unwrap_or(preferences)
        });
        let connections = connection_override.clone().unwrap_or(connections);
        let workspaces = connections.with_extension("workspaces");
        let servers = connections.with_extension("servers");
        #[cfg(target_os = "linux")]
        let mut migrations = Vec::new();
        #[cfg(target_os = "linux")]
        if connection_override.is_none() {
            migrations.push((legacy.clone(), connections.clone()));
            migrations.push((legacy.with_extension("workspaces"), workspaces.clone()));
            migrations.push((legacy.with_extension("servers"), servers.clone()));
            if preference_override.is_none() {
                migrations.push((
                    legacy.with_extension("preferences.json"),
                    preferences.clone(),
                ));
            }
        }
        Self {
            connections,
            preferences,
            workspaces,
            servers,
            #[cfg(target_os = "linux")]
            migrations,
        }
    }

    pub fn migrate(&self) -> std::io::Result<()> {
        #[cfg(target_os = "linux")]
        for (source, destination) in &self.migrations {
            linux::migrate(source, destination)?;
        }
        Ok(())
    }
}

pub(crate) fn runtime_directory() -> std::io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    return linux::runtime_directory(env::var_os("XDG_RUNTIME_DIR"));
    #[cfg(not(target_os = "linux"))]
    {
        let path = env::temp_dir().join(format!("ganymede-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(path)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        ffi::OsString,
        fs, io,
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        path::{Path, PathBuf},
    };

    fn mkdir(path: &Path) -> io::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }

    // Install complete files atomically without replacing a concurrently created
    // destination. Keep originals for rollback and conflicting recovery records.
    fn copy_missing(source: &Path, destination: &Path) -> io::Result<()> {
        let meta = match fs::symlink_metadata(source) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if meta.file_type().is_symlink() {
            return Err(io::Error::other("Legacy storage contains a symbolic link"));
        }
        if let Ok(dest) = fs::symlink_metadata(destination) {
            if dest.file_type().is_symlink() || dest.is_dir() != meta.is_dir() {
                return Err(io::Error::other(
                    "Storage migration destination has an incompatible type",
                ));
            }
            if dest.is_file() {
                return Ok(());
            }
        }
        if meta.is_dir() {
            mkdir(destination)?;
            for entry in fs::read_dir(source)? {
                let entry = entry?;
                copy_missing(&entry.path(), &destination.join(entry.file_name()))?;
            }
        } else if meta.is_file() {
            mkdir(destination.parent().unwrap())?;
            let temporary = destination.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
            let result = (|| {
                let mut output = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&temporary)?;
                io::copy(&mut fs::File::open(source)?, &mut output)?;
                output.sync_all()?;
                match fs::hard_link(&temporary, destination) {
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
                    result => result,
                }
            })();
            let _ = fs::remove_file(temporary);
            result?;
        } else {
            return Err(io::Error::other("Legacy storage contains a special file"));
        }
        Ok(())
    }

    pub(super) fn migrate(source: &Path, destination: &Path) -> io::Result<()> {
        if source == destination {
            return Ok(());
        }
        let mut marker_name = destination.as_os_str().to_os_string();
        marker_name.push(".migration-v1-complete");
        let marker = PathBuf::from(marker_name);
        if marker.try_exists()? {
            return Ok(());
        }
        copy_missing(source, destination)?;
        mkdir(marker.parent().unwrap())?;
        // Prevent old state from being resurrected after the user deletes new state.
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(marker)
        {
            Ok(file) => file.sync_all(),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn private_directory(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|m| {
            m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o777 == 0o700
        })
    }

    pub(super) fn runtime_directory(value: Option<OsString>) -> io::Result<PathBuf> {
        let base = value
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && private_directory(p));
        if let Some(base) = base {
            let app = base.join("ganymede");
            mkdir(&app)?;
            if !private_directory(&app) {
                return Err(io::Error::other(
                    "Ganymede runtime directory must be private and owned by the current user",
                ));
            }
            let path = app.join(format!("server-{}", uuid::Uuid::new_v4()));
            fs::DirBuilder::new().mode(0o700).create(&path)?;
            return Ok(path);
        }
        eprintln!(
            "XDG_RUNTIME_DIR is unavailable or invalid; using a private temporary server directory."
        );
        let path = std::env::temp_dir().join(format!("ganymede-server-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(path)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
        path::Path,
    };

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path =
                env::temp_dir().join(format!("ganymede-storage-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn paths(home: &Path, values: &[(&str, &str)]) -> Paths {
        Paths::resolve(|key| {
            if key == "HOME" {
                return Some(home.as_os_str().to_owned());
            }
            values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[test]
    fn xdg_defaults_and_absolute_overrides_separate_config_from_state() {
        let home = Path::new("/home/test");
        let default = paths(home, &[]);
        assert_eq!(
            default.preferences,
            home.join(".config/ganymede/preferences.json")
        );
        assert_eq!(
            default.connections,
            home.join(".local/state/ganymede/connections.json")
        );
        let custom = paths(
            home,
            &[("XDG_CONFIG_HOME", "/config"), ("XDG_STATE_HOME", "/state")],
        );
        assert_eq!(
            custom.preferences,
            Path::new("/config/ganymede/preferences.json")
        );
        assert_eq!(
            custom.workspaces,
            Path::new("/state/ganymede/connections.workspaces")
        );
        assert_eq!(
            custom.servers,
            Path::new("/state/ganymede/connections.servers")
        );
        for invalid in ["", "relative/path"] {
            let actual = paths(
                home,
                &[("XDG_CONFIG_HOME", invalid), ("XDG_STATE_HOME", invalid)],
            );
            assert_eq!(actual.preferences, default.preferences);
            assert_eq!(actual.connections, default.connections);
        }
    }

    #[test]
    fn development_overrides_preserve_isolation_and_precedence() {
        let p = paths(
            Path::new("/home/test"),
            &[("GANYMEDE_CONNECTIONS_FILE", "/dev/history.json")],
        );
        assert_eq!(p.preferences, Path::new("/dev/history.preferences.json"));
        assert_eq!(p.servers, Path::new("/dev/history.servers"));
        assert!(p.migrations.is_empty());
        let p = paths(
            Path::new("/home/test"),
            &[
                ("GANYMEDE_CONNECTIONS_FILE", "/dev/history.json"),
                ("GANYMEDE_PREFERENCES_FILE", "/dev/settings.json"),
            ],
        );
        assert_eq!(p.preferences, Path::new("/dev/settings.json"));
        let p = paths(
            Path::new("/home/test"),
            &[("GANYMEDE_PREFERENCES_FILE", "/dev/settings.json")],
        );
        assert_eq!(p.migrations.len(), 3);
    }

    #[test]
    fn migration_preserves_conflicts_and_recovery_and_does_not_resurrect_deleted_state() {
        let root = Directory::new();
        let p = paths(&root.0, &[]);
        let old = root.0.join(".config/ganymede");
        fs::create_dir_all(old.join("connections.workspaces/server/recovery")).unwrap();
        fs::create_dir_all(old.join("connections.servers")).unwrap();
        fs::write(old.join("connections.json"), "old history").unwrap();
        fs::write(old.join("connections.preferences.json"), "preferences").unwrap();
        fs::write(old.join("connections.servers/id.json"), "server").unwrap();
        fs::write(
            old.join("connections.workspaces/server/recovery/backup.json"),
            "unsaved edits",
        )
        .unwrap();
        fs::create_dir_all(p.connections.parent().unwrap()).unwrap();
        fs::write(&p.connections, "new history").unwrap();
        p.migrate().unwrap();
        assert_eq!(fs::read_to_string(&p.connections).unwrap(), "new history");
        assert_eq!(fs::read_to_string(&p.preferences).unwrap(), "preferences");
        assert_eq!(
            fs::read_to_string(p.servers.join("id.json")).unwrap(),
            "server"
        );
        let backup = p.workspaces.join("server/recovery/backup.json");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "unsaved edits");
        assert_eq!(fs::metadata(&backup).unwrap().mode() & 0o777, 0o600);
        assert!(
            old.join("connections.workspaces/server/recovery/backup.json")
                .exists()
        );
        fs::remove_file(&backup).unwrap();
        fs::remove_file(&p.connections).unwrap();
        p.migrate().unwrap();
        assert!(!backup.exists());
        assert!(!p.connections.exists());
    }

    #[test]
    fn migration_can_retry_after_failure_without_overwriting_completed_files() {
        let root = Directory::new();
        let source = root.0.join("old");
        let dest = root.0.join("new");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("a.json"), "a").unwrap();
        symlink(source.join("a.json"), source.join("b.json")).unwrap();
        assert!(linux::migrate(&source, &dest).is_err());
        fs::remove_file(source.join("b.json")).unwrap();
        fs::write(source.join("b.json"), "b").unwrap();
        linux::migrate(&source, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("a.json")).unwrap(), "a");
        assert_eq!(fs::read_to_string(dest.join("b.json")).unwrap(), "b");
    }

    #[test]
    fn runtime_uses_private_xdg_directory_and_falls_back_for_invalid_values() {
        let root = Directory::new();
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o700)).unwrap();
        let path = linux::runtime_directory(Some(root.0.as_os_str().to_owned())).unwrap();
        assert!(path.starts_with(root.0.join("ganymede")));
        assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o755)).unwrap();
        for value in [
            None,
            Some(OsString::from("")),
            Some(OsString::from("relative")),
            Some(root.0.as_os_str().to_owned()),
        ] {
            let path = linux::runtime_directory(value).unwrap();
            assert!(path.starts_with(env::temp_dir()));
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o700);
            fs::remove_dir(path).unwrap();
        }
    }
}
