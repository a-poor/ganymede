//! Explicit update checks. No credentials, background polling, or automatic installation.
use crate::toast::{Toast, ToastHost, ToastKind};
use gpui::{App, Entity};
use semver::Version;
use serde::Deserialize;
use std::{cmp::Ordering, time::Duration};

const ENDPOINT: &str = "https://api.github.com/repos/a-poor/ganymede/releases/latest";
const RELEASES: &str = "https://github.com/a-poor/ganymede/releases/tag/";
const MAX_RESPONSE: usize = 256 * 1024;

#[derive(Debug, PartialEq)]
enum Outcome {
    Available { version: String, url: String },
    Current,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}
fn interpret(bytes: &[u8], current: &str) -> Result<Outcome, String> {
    let release: Release = serde_json::from_slice(bytes)
        .map_err(|_| "GitHub returned an unreadable release.".to_owned())?;
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|_| "GitHub returned an invalid release version.".to_owned())?;
    if release.draft || release.prerelease || !version.pre.is_empty() {
        return Err("No stable release is available.".into());
    }
    let current =
        Version::parse(current).map_err(|_| "Could not read the app version.".to_owned())?;
    if version.cmp_precedence(&current) == Ordering::Greater {
        // Construct the link ourselves; never open a URL supplied by the response.
        Ok(Outcome::Available {
            version: version.to_string(),
            url: format!("{RELEASES}{}", release.tag_name),
        })
    } else {
        Ok(Outcome::Current)
    }
}
fn status_error(status: reqwest::StatusCode) -> Option<String> {
    match status.as_u16() {
        200 => None,
        403 | 429 => Some("GitHub is limiting requests. Please try again later.".into()),
        404 => Some("No published release was found.".into()),
        _ => Some("GitHub is unavailable. Please try again.".into()),
    }
}
async fn fetch(endpoint: &str, timeout: Duration) -> Result<Outcome, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("Ganymede/", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not start the update check.".to_owned())?;
    let mut response = client
        .get(endpoint)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(network_error)?;
    if let Some(error) = status_error(response.status()) {
        return Err(error);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if body.len() + chunk.len() > MAX_RESPONSE {
            return Err("GitHub returned an oversized response.".into());
        }
        body.extend_from_slice(&chunk);
    }
    interpret(&body, env!("CARGO_PKG_VERSION"))
}
fn network_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "The update check timed out. Please try again.".into()
    } else {
        "Could not reach GitHub. Check your connection and try again.".into()
    }
}
#[derive(Default)]
pub(crate) struct CheckState {
    pub(crate) running: bool,
}
impl gpui::Global for CheckState {}

pub fn check(host: Entity<ToastHost>, cx: &mut App) {
    if cx.try_global::<CheckState>().is_some_and(|s| s.running) {
        return;
    }
    cx.set_global(CheckState { running: true });
    let id = host.update(cx, |host, cx| {
        host.push(
            Toast::new("Checking for updates…").kind(ToastKind::Loading),
            cx,
        )
    });
    let host = host.downgrade();
    let task = cx
        .global::<crate::execution::Runtime>()
        .handle
        .spawn(async { fetch(ENDPOINT, Duration::from_secs(15)).await });
    cx.spawn(async move |cx| {
        let result = task
            .await
            .unwrap_or_else(|_| Err("Could not finish the update check.".into()));
        cx.update(|cx| {
            cx.set_global(CheckState::default());
            let retry_host = host.clone();
            let toast = match result {
                Ok(Outcome::Available { version, url }) => {
                    Toast::new(format!("Ganymede {version} is available"))
                        .description(format!("You’re using {}.", env!("CARGO_PKG_VERSION")))
                        .kind(ToastKind::Info)
                        .duration(Duration::from_secs(20))
                        .action("Open release", move |_, cx| cx.open_url(&url))
                }
                Ok(Outcome::Current) => Toast::new("You’re up to date")
                    .description(format!("Ganymede {}", env!("CARGO_PKG_VERSION")))
                    .kind(ToastKind::Success),
                Err(error) => Toast::new("Unable to check for updates")
                    .description(error)
                    .kind(ToastKind::Error)
                    .duration(Duration::from_secs(15))
                    .action("Retry", move |_, cx| {
                        let host = retry_host.clone();
                        // Toast callbacks run while the host is borrowed.
                        cx.defer(move |cx| {
                            if let Some(host) = host.upgrade() {
                                check(host, cx);
                            }
                        });
                    }),
            };
            host.update(cx, |host, cx| host.resolve(id, toast, cx)).ok();
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(tag: &str) -> Vec<u8> {
        serde_json::to_vec(
            &serde_json::json!({"tag_name": tag, "draft": false, "prerelease": false}),
        )
        .unwrap()
    }
    #[test]
    fn compares_semantic_versions_without_offering_downgrades() {
        assert!(matches!(
            interpret(&response("v0.1.10"), "0.1.9"),
            Ok(Outcome::Available { .. })
        ));
        assert_eq!(
            interpret(&response("v0.1.1"), "0.1.1"),
            Ok(Outcome::Current)
        );
        assert_eq!(
            interpret(&response("v0.1.1"), "0.2.0"),
            Ok(Outcome::Current)
        );
        assert_eq!(
            interpret(&response("v0.1.1+build2"), "0.1.1+build1"),
            Ok(Outcome::Current)
        );
        assert!(matches!(
            interpret(&response("v0.2.0"), "0.2.0-beta.1"),
            Ok(Outcome::Available { .. })
        ));
    }
    #[test]
    fn rejects_unstable_or_malformed_releases() {
        for bytes in [
            response("v1.0.0-rc.1"),
            response("../../bad"),
            b"not json".to_vec(),
            br#"{"tag_name":"v1.0.0","draft":true,"prerelease":false}"#.to_vec(),
            br#"{"tag_name":"v1.0.0","draft":false,"prerelease":true}"#.to_vec(),
        ] {
            assert!(interpret(&bytes, "0.1.1").is_err());
        }
    }
    #[tokio::test]
    async fn handles_http_failure_and_timeout() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (reply, expected) in [
            (
                "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n",
                "limiting requests",
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nnope",
                "unreadable",
            ),
            ("", "timed out"),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let count = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..count]).to_lowercase();
                assert!(request.contains("user-agent: ganymede/"));
                assert!(!request.contains("authorization:"));
                if reply.is_empty() {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                } else {
                    socket.write_all(reply.as_bytes()).await.unwrap();
                }
            });
            let error = fetch(&format!("http://{address}"), Duration::from_millis(100))
                .await
                .unwrap_err();
            assert!(error.contains(expected), "{error}");
            server.await.unwrap();
        }
    }
}
