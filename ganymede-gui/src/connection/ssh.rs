//! App-owned OpenSSH dynamic forwarding. Never starts or stops a remote server.
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(super) fn host(value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.starts_with('-')
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-@[]:".contains(&c))
    {
        return Err(
            "Enter an SSH config alias or user@host. Set ports and jump hosts in ~/.ssh/config."
                .into(),
        );
    }
    Ok(Some(value.into()))
}
pub(super) fn identity(address: &str, host: Option<&str>) -> String {
    match host {
        Some(host) => format!("ssh:{}", serde_json::json!([host, address])),
        None => address.to_owned(),
    }
}

#[derive(Clone)]
pub(crate) struct Tunnel {
    process: Arc<Mutex<Process>>,
    host: String,
    endpoint: SocketAddr,
}
struct Process {
    host: String,
    endpoint: SocketAddr,
    child: Option<Child>,
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}
impl Process {
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGTERM);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn start(&mut self) -> Result<(), String> {
        self.stop();
        let mut cmd = Command::new("ssh");
        cmd.args(arguments(&self.host, self.endpoint));
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|_| "Could not start OpenSSH. Check that ssh is installed.".to_owned())?;
        let mut stderr = child.stderr.take().unwrap();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let captured = errors.clone();
        std::thread::spawn(move || {
            let mut chunk = [0; 1024];
            while let Ok(n) = stderr.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut text = captured.lock().unwrap();
                let remaining = 8192usize.saturating_sub(text.len());
                text.extend_from_slice(&chunk[..n.min(remaining)]);
            }
        });
        self.child = Some(child);
        let until = Instant::now() + Duration::from_secs(15);
        while Instant::now() < until {
            if self
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .ok()
                .flatten()
                .is_some()
            {
                let message = failure(&String::from_utf8_lossy(&errors.lock().unwrap()));
                self.stop();
                return Err(message.into());
            }
            if let Ok(mut stream) =
                TcpStream::connect_timeout(&self.endpoint, Duration::from_millis(100))
            {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                let mut response = [0; 2];
                if stream.write_all(&[5, 1, 0]).is_ok()
                    && stream.read_exact(&mut response).is_ok()
                    && response == [5, 0]
                {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.stop();
        Err("SSH timed out. Check the host, network, and SSH configuration.".into())
    }
}
fn arguments(host: &str, endpoint: SocketAddr) -> Vec<String> {
    [
        "-N",
        "-T",
        "-a",
        "-x",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "ControlMaster=no",
        "-o",
        "ControlPath=none",
        "-o",
        "ForkAfterAuthentication=no",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "RemoteCommand=none",
        "-D",
        &endpoint.to_string(),
        "--",
        host,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
fn failure(stderr: &str) -> &'static str {
    if stderr.contains("Host key verification failed")
        || stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
    {
        "SSH host key is not trusted or has changed. Verify this host in Terminal before connecting."
    } else if stderr.contains("Permission denied") {
        "SSH authentication failed. Check your SSH key or unlock it with ssh-add in Terminal."
    } else if stderr.contains("Address already in use") {
        "The SSH forwarding port is in use. Open a new connection and retry."
    } else {
        "SSH connection failed. Check the host and ~/.ssh/config, and confirm you can connect in Terminal."
    }
}
impl Tunnel {
    pub(super) async fn open(host: String) -> Result<Self, String> {
        tokio::task::spawn_blocking(move || {
            let listener = TcpListener::bind("127.0.0.1:0")
                .map_err(|_| "Could not reserve an SSH forwarding port.".to_owned())?;
            let endpoint = listener
                .local_addr()
                .map_err(|_| "Could not read SSH forwarding port.".to_owned())?;
            drop(listener);
            let mut process = Process {
                host: host.clone(),
                endpoint,
                child: None,
            };
            process.start()?;
            Ok(Self {
                process: Arc::new(Mutex::new(process)),
                host,
                endpoint,
            })
        })
        .await
        .map_err(|_| "SSH startup interrupted.".to_owned())?
    }
    pub(crate) fn register_quit(&self, cx: &gpui::App) {
        let weak = Arc::downgrade(&self.process);
        cx.on_app_quit(move |_| {
            if let Some(process) = weak.upgrade() {
                process.lock().unwrap().stop();
            }
            std::future::ready(())
        })
        .detach();
    }
    pub(crate) fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }
    pub(crate) fn host(&self) -> String {
        self.host.clone()
    }
    pub(crate) async fn check(&self, restart: bool) -> Result<(), String> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut process = this.process.lock().unwrap();
            if process
                .child
                .as_mut()
                .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
            {
                return Ok(());
            }
            if restart {
                process.start()
            } else {
                Err("SSH tunnel disconnected. Use Reconnect to reopen it.".into())
            }
        })
        .await
        .map_err(|_| "SSH check interrupted.".to_owned())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssh_hosts_cannot_inject_arguments_or_commands() {
        for value in [
            "-oProxyCommand=evil",
            "host;whoami",
            "user@host command",
            "$(bad)",
            "host\ncommand",
            "host/other",
        ] {
            assert!(host(value).is_err());
        }
        assert_eq!(
            host(" user@my-server ").unwrap(),
            Some("user@my-server".into())
        );
        assert_eq!(host("").unwrap(), None);
        let args = arguments("my-server", "127.0.0.1:12345".parse().unwrap());
        assert!(args.windows(2).any(|a| a == ["-D", "127.0.0.1:12345"]));
        assert!(args.contains(&"StrictHostKeyChecking=yes".into()));
        assert!(args.ends_with(&["--".into(), "my-server".into()]));
    }
    #[test]
    fn remote_identity_does_not_collide_with_localhost_or_other_hosts() {
        let address = "http://localhost:8888/";
        assert_ne!(identity(address, None), identity(address, Some("a")));
        assert_ne!(identity(address, Some("a")), identity(address, Some("b")));
        assert_ne!(
            identity(address, Some("a")),
            identity("http://localhost:8888/user/a/", Some("a"))
        );
    }
    #[tokio::test]
    #[ignore = "requires GANYMEDE_TEST_SSH_HOST with key authentication"]
    async fn tunnel_reconnects_on_request_and_drop_closes_only_its_forward() {
        let host = std::env::var("GANYMEDE_TEST_SSH_HOST").unwrap();
        let tunnel = Tunnel::open(host).await.unwrap();
        let endpoint = tunnel.endpoint();
        assert!(tunnel.check(false).await.is_ok());
        tunnel.process.lock().unwrap().stop();
        assert!(tunnel.check(false).await.is_err());
        tunnel.check(true).await.unwrap();
        assert_eq!(tunnel.endpoint(), endpoint);
        drop(tunnel);
        assert!(TcpStream::connect_timeout(&endpoint, Duration::from_millis(100)).is_err());
    }
}
