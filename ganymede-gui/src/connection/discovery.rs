//! Bounded discovery and explicitly requested environment setup. Never resolves
//! Python symlinks: a venv must retain its original path for environment identity.
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Clone, Debug)]
pub(super) struct Environment {
    pub path: PathBuf,
    pub label: String,
    pub status: String,
    pub ready: bool,
    pub missing: Vec<&'static str>,
    pub project: bool,
}

const PROBE: &str = "import sys,json,importlib.util; print(json.dumps({'version':'.'.join(map(str,sys.version_info[:3])), 'server':importlib.util.find_spec('jupyter_server') is not None, 'kernel':importlib.util.find_spec('ipykernel') is not None}))";

async fn output(executable: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut child = Command::new(executable)
        .args(args)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "Python unavailable".to_string())?;
    let mut stdout = child.stdout.take().unwrap().take(65537);
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Unable to read environment")?;
        if bytes.len() > 65536 {
            return Err("Unexpected environment response");
        }
        if !child
            .wait()
            .await
            .map_err(|_| "Unable to check environment")?
            .success()
        {
            return Err("Unable to check environment");
        }
        Ok(bytes)
    })
    .await;
    match result {
        Ok(result) => result.map_err(str::to_string),
        Err(_) => Err("Check timed out".into()),
    }
}

fn status(bytes: &[u8]) -> Result<(String, String, bool, Vec<&'static str>), String> {
    #[derive(serde::Deserialize)]
    struct Probe {
        version: String,
        server: bool,
        kernel: bool,
    }
    let probe: Probe =
        serde_json::from_slice(bytes).map_err(|_| "Unable to read Python details".to_string())?;
    let message = match (probe.server, probe.kernel) {
        (true, true) => "Ready",
        (false, true) => "Missing jupyter-server",
        (true, false) => "Missing ipykernel",
        (false, false) => "Missing jupyter-server and ipykernel",
    };
    let mut missing = Vec::new();
    if !probe.server {
        missing.push("jupyter-server");
    }
    if !probe.kernel {
        missing.push("ipykernel");
    }
    Ok((probe.version, message.into(), missing.is_empty(), missing))
}

pub(super) fn venv_python(folder: &Path) -> PathBuf {
    folder.join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    })
}

fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|p| p.is_absolute())
        .collect();
    // Finder-launched apps commonly inherit a minimal PATH.
    if cfg!(target_os = "macos") {
        dirs.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".local/bin"));
        }
    }
    dirs
}

pub(super) fn uv_executable() -> Option<PathBuf> {
    search_dirs()
        .iter()
        .map(|p| p.join("uv"))
        .find(|p| p.is_file())
}

fn install_command(python: &Path, uv: Option<&Path>, packages: &[&str]) -> Command {
    let mut command = if let Some(uv) = uv {
        let mut command = Command::new(uv);
        command.env_remove("UV_BREAK_SYSTEM_PACKAGES");
        command
            .args([
                "pip",
                "install",
                "--no-config",
                "--no-python-downloads",
                "--python",
            ])
            .arg(python);
        command
    } else {
        let mut command = Command::new(python);
        command.args([
            "-m",
            "pip",
            "--isolated",
            "install",
            "--no-input",
            "--no-user",
            "--disable-pip-version-check",
        ]);
        command
    };
    command.args(packages);
    command
}

fn venv_command(folder: &Path) -> Command {
    let mut command = Command::new("python3");
    command.args(["-I", "-m", "venv", "--"]).arg(folder);
    command
}

async fn create_venv(folder: &Path) -> Result<(), String> {
    // Reserve a new directory exclusively. Never merge into an existing directory
    // (including an empty one) or follow an existing symlink.
    std::fs::create_dir(folder).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            "That folder already exists. Select an existing environment or choose a new folder."
        } else {
            "Could not create the environment folder. Check its parent folder and permissions."
        }
        .to_string()
    })?;
    run_installer(venv_command(folder), Duration::from_secs(120)).await
        .map_err(|_| "Could not create the environment with python3. Check that python3 and its venv module are available. The partially created folder has been kept.".to_string())
}

/// Creates only a new folder, then installs the packages named in the confirmation.
pub(super) async fn create_environment(folder: PathBuf) -> Result<(), String> {
    create_venv(&folder).await?;
    install(venv_python(&folder), vec!["jupyter-server", "ipykernel"]).await
}

/// Only called after the user confirms the target interpreter and packages.
pub(super) async fn install(python: PathBuf, confirmed: Vec<&'static str>) -> Result<(), String> {
    let python = if python.components().count() == 1 {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.join(&python))
            .find(|p| p.is_file())
            .ok_or_else(|| {
                "Python is no longer available. Refresh and choose an environment.".to_string()
            })?
    } else {
        python
    };
    // Make uv's target unambiguous, without resolving a venv's Python symlink.
    let python = if python.is_absolute() {
        python
    } else {
        std::env::current_dir()
            .map_err(|_| "Could not resolve the Python path.")?
            .join(python)
    };
    let (_, _, _, mut missing) = status(&output(&python, &["-c", PROBE]).await?)?;
    missing.retain(|package| confirmed.contains(package));
    if missing.is_empty() {
        return Ok(());
    }
    let uv = uv_executable();
    let command = install_command(&python, uv.as_deref(), &missing);
    run_installer(command, Duration::from_secs(600)).await?;
    let (_, _, ready, _) = status(&output(&python, &["-c", PROBE]).await?)?;
    if ready {
        Ok(())
    } else {
        Err("The installer finished, but Jupyter is still unavailable in this environment.".into())
    }
}

async fn run_installer(mut command: Command, timeout: Duration) -> Result<(), String> {
    #[cfg(unix)]
    command.process_group(0);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start the package installer.".to_string())?;
    // uv/pip can spawn build subprocesses. Cancel the whole installer group.
    struct ProcessGroup(Option<u32>);
    impl Drop for ProcessGroup {
        fn drop(&mut self) {
            #[cfg(unix)]
            if let Some(pid) = self.0 {
                // This group belongs to the child spawned immediately above.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGTERM);
                }
            }
        }
    }
    let mut group = ProcessGroup(child.id());
    let mut stderr = child.stderr.take().unwrap();
    let result = tokio::time::timeout(timeout, async {
        let mut detail = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let n = stderr.read(&mut buffer).await.map_err(|_| "Could not read installer status.")?;
            if n == 0 { break; }
            let remaining = 65536_usize.saturating_sub(detail.len());
            detail.extend_from_slice(&buffer[..n.min(remaining)]);
        }
        let result = child.wait().await.map_err(|_| "Could not check installer status.")?;
        group.0 = None;
        if result.success() { return Ok(()); }
        // Installer logs can contain private index URLs. Show fixed guidance only.
        let detail = String::from_utf8_lossy(&detail);
        Err(if detail.contains("externally-managed") || detail.contains("externally managed") {
            "This Python is managed by your OS or package manager. Choose a virtual environment instead."
        } else if detail.contains("No module named pip") {
            "This Python has no pip. Install uv or set up pip in this environment, then retry."
        } else {
            "Installation failed. Check network access and environment permissions, or install the packages in Terminal."
        })
    }).await;
    match result {
        Ok(result) => result.map_err(str::to_string)?,
        Err(_) => return Err("Installation timed out. Some packages may already be installed; refresh before retrying.".into()),
    }
    Ok(())
}

pub(super) async fn discover(folder: PathBuf, selected: PathBuf) -> Vec<Environment> {
    let mut candidates = Vec::new();
    for name in [".venv", "venv"] {
        let root = folder.join(name);
        if root.is_dir() {
            candidates.push((venv_python(&root), format!("Project {name}"), true));
        }
    }
    candidates.push((selected, "Python".into(), false));
    let dirs = search_dirs();
    for dir in &dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            let mut paths: Vec<_> = entries
                .flatten()
                .filter(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    name == "python"
                        || name == "python3"
                        || name.strip_prefix("python3.").is_some_and(|suffix| {
                            !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit())
                        })
                })
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect();
            paths.sort();
            candidates.extend(paths.into_iter().map(|path| (path, "Python".into(), false)));
        }
    }
    if let Some(uv) = dirs.iter().map(|p| p.join("uv")).find(|p| p.is_file())
        && let Ok(bytes) = output(
            &uv,
            &[
                "python",
                "list",
                "--only-installed",
                "--output-format",
                "json",
                "--offline",
                "--no-config",
            ],
        )
        .await
        && let Ok(entries) = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes)
    {
        for entry in entries {
            if let Some(path) = entry.get("path").and_then(|p| p.as_str()) {
                candidates.push((PathBuf::from(path), "uv Python".into(), false));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|(p, _, _)| {
        // Resolve command names only for deduplication. Never canonicalize the
        // Python symlink itself: distinct venvs can point to the same binary.
        let key = if p.components().count() == 1 {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join(p))
                .find(|path| path.is_file())
                .unwrap_or_else(|| p.clone())
        } else {
            p.clone()
        };
        seen.insert(key)
    });
    candidates.truncate(32);
    let mut results = Vec::new();
    // Bounded concurrency; dropping JoinSet cancels probes and kills children.
    for chunk in candidates.chunks(4) {
        let mut tasks = tokio::task::JoinSet::new();
        for (path, label, project) in chunk.iter().cloned() {
            tasks.spawn(async move {
                let result = match output(&path, &["-c", PROBE]).await {
                    Ok(bytes) => status(&bytes),
                    Err(e) => Err(e),
                };
                let (label, status, ready, missing) = match result {
                    Ok((version, status, ready, missing)) => {
                        (format!("{label} · {version}"), status, ready, missing)
                    }
                    Err(status) => (label, status, false, Vec::new()),
                };
                Environment {
                    path,
                    label,
                    status,
                    ready,
                    missing,
                    project,
                }
            });
        }
        while let Some(Ok(result)) = tasks.join_next().await {
            results.push(result);
        }
    }
    results.sort_by(|a, b| (!a.project, !a.ready, &a.path).cmp(&(!b.project, !b.ready, &b.path)));
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn venv_creation_uses_default_python_and_a_separate_path_argument() {
        let command = venv_command(Path::new("/project with spaces/.venv"));
        assert_eq!(command.as_std().get_program(), "python3");
        let args: Vec<_> = command.as_std().get_args().collect();
        assert_eq!(
            args,
            ["-I", "-m", "venv", "--", "/project with spaces/.venv"]
        );
    }

    #[tokio::test]
    async fn environment_creation_preserves_existing_folders() {
        let folder =
            std::env::temp_dir().join(format!("ganymede-existing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("keep.txt");
        std::fs::write(&file, "keep me").unwrap();
        let result = create_environment(folder.clone()).await.unwrap_err();
        assert!(result.contains("already exists"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep me");
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires python3 with venv; creates a disposable environment without downloading packages"]
    async fn creates_a_working_venv_with_default_python() {
        let folder = std::env::temp_dir().join(format!("ganymede venv {}", uuid::Uuid::new_v4()));
        let result = create_venv(&folder).await;
        let probe = output(
            &venv_python(&folder),
            &[
                "-I",
                "-c",
                "import sys; print(sys.prefix != sys.base_prefix)",
            ],
        )
        .await;
        let _ = std::fs::remove_dir_all(&folder);
        result.unwrap();
        assert_eq!(String::from_utf8(probe.unwrap()).unwrap().trim(), "True");
    }
    #[test]
    fn installers_target_the_exact_interpreter_without_overriding_protections() {
        let python = Path::new("/project with spaces/.venv/bin/python");
        let command = install_command(python, Some(Path::new("/tools/uv")), &["ipykernel"]);
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "pip",
                "install",
                "--no-config",
                "--no-python-downloads",
                "--python",
                "/project with spaces/.venv/bin/python",
                "ipykernel"
            ]
        );
        let command = install_command(python, None, &["ipykernel"]);
        assert_eq!(command.as_std().get_program(), python.as_os_str());
        assert!(command.as_std().get_args().any(|a| a == "--no-user"));
        assert!(
            !command
                .as_std()
                .get_args()
                .any(|a| a == "--break-system-packages")
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn installer_reports_failures_without_leaking_logs_and_times_out() {
        assert!(
            run_installer(Command::new("/usr/bin/true"), Duration::from_secs(1))
                .await
                .is_ok()
        );
        let mut failure = Command::new("/bin/sh");
        failure.args([
            "-c",
            "echo 'externally-managed-environment https://secret:token@example.test' >&2; exit 1",
        ]);
        let error = run_installer(failure, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(error.contains("managed"));
        assert!(!error.contains("secret"));
        let mut slow = Command::new("/bin/sh");
        slow.args(["-c", "sleep 10 & wait"]);
        assert!(
            run_installer(slow, Duration::from_millis(30))
                .await
                .unwrap_err()
                .contains("timed out")
        );
    }
    #[test]
    fn readiness_distinguishes_missing_dependencies() {
        assert!(
            status(br#"{"version":"3.12.1","server":true,"kernel":true}"#)
                .unwrap()
                .2
        );
        let missing = status(br#"{"version":"3.12.1","server":true,"kernel":false}"#).unwrap();
        assert!(!missing.2);
        assert_eq!(missing.1, "Missing ipykernel");
        assert!(status(b"not json").is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn failed_and_stuck_commands_are_bounded() {
        assert!(
            output(Path::new("/nonexistent/ganymede-python"), &[])
                .await
                .is_err()
        );
        assert_eq!(
            output(Path::new("/bin/sleep"), &["10"]).await.unwrap_err(),
            "Check timed out"
        );
    }
}
