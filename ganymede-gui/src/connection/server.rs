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
    child: Option<Child>,
    pub info: PathBuf,
    directory: PathBuf,
    pub address: String,
    pub config: ServerConfig,
}
impl PendingServer {
    pub fn accept(mut self) {
        if let Some(mut child) = self.child.take() {
            // Reap the process if it eventually exits while the app is still open.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
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
        }
    }
}

pub(super) fn read_local(info: &std::path::Path) -> Option<(String, ServerConfig)> {
    let data: serde_json::Value = serde_json::from_slice(&std::fs::read(info).ok()?).ok()?;
    let address = data.get("url")?.as_str()?;
    let token = data.get("token")?.as_str()?;
    parse_connection(address, token).ok()
}

#[derive(Clone, Copy, PartialEq, Eq)]
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

fn launch_command(
    mode: Environment,
    value: &str,
    root: &std::path::Path,
) -> Result<(std::ffi::OsString, Vec<&'static str>), String> {
    let python_args = vec!["-m", "jupyter_server"];
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
    let token = uuid::Uuid::new_v4().to_string();
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
            "--ServerApp.port=0",
            "--ServerApp.port_retries=0",
        ])
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
        child: Some(child),
        info,
        directory,
        address: String::new(),
        config: ServerConfig::new("http://127.0.0.1/").unwrap(),
    };
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) {
        if pending
            .child
            .as_mut()
            .unwrap()
            .try_wait()
            .map_err(|_| "Could not check server startup.".to_owned())?
            .is_some()
        {
            return Err("Jupyter could not start. Make sure jupyter-server and ipykernel are installed in the selected Python environment.".into());
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
        assert_eq!(args, ["-m", "jupyter_server"]);
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
