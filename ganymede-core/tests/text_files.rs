use ganymede_core::{
    jupyter::ServerConfig,
    text_file::{Error, TextFileController},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn request(socket: &mut TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(start) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = std::str::from_utf8(&bytes[..start]).unwrap().to_owned();
            let length = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|s| s.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() >= start + 4 + length {
                let body = if length == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes[start + 4..start + 4 + length]).unwrap()
                };
                return (header, body);
            }
        }
    }
}
async fn respond(socket: &mut TcpStream, status: &str, model: Value) {
    let body = model.to_string();
    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
}
fn model(version: &str, content: &str) -> Value {
    json!({"name":"uv.lock","path":"uv.lock","type":"file","writable":true,
        "created":"","last_modified":version,"format":"text","content":content})
}
async fn setup() -> (TcpListener, ServerConfig) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = ServerConfig::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    (listener, config)
}

#[tokio::test]
async fn text_conflicts_copy_reload_overwrite_and_empty_saves() {
    let (listener, config) = setup().await;
    let server = tokio::spawn(async move {
        for (method, version, content, expected) in [
            ("GET", "1", "version = 1\n", None),
            ("GET", "2", "external\n", None), // conflict; no write
            ("POST", "3", "", None),
            ("PUT", "4", "local\n", Some("local\n")),
            ("GET", "2", "external\n", None), // reload
            ("GET", "2", "external\n", None),
            ("PUT", "5", "", Some("")), // deletion of all content must save
            ("GET", "6", "external again", None),
            ("PUT", "7", "overwrite", Some("overwrite")),
            ("GET", "7", "overwrite", None),
            ("PUT", "8", "next", Some("next")),
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (header, body) = request(&mut socket).await;
            assert!(header.starts_with(method), "{header}");
            if let Some(text) = expected {
                assert_eq!(
                    body,
                    json!({"type":"file", "format":"text", "content":text})
                );
            }
            let mut result = model(version, content);
            if version == "3" {
                assert_eq!(body, json!({"type":"file", "ext":".lock"}));
                result["path"] = json!("untitled.lock");
            }
            if version == "4" {
                assert!(header.starts_with("PUT /api/contents/untitled.lock "));
            }
            respond(&mut socket, "200 OK", result).await;
        }
    });
    let file = TextFileController::open(config, "uv.lock").await.unwrap();
    assert!(!file.snapshot().is_dirty());
    file.update_text("local\n".into());
    assert!(matches!(file.save(false).await, Err(Error::Conflict)));
    assert!(file.snapshot().is_dirty());
    assert_eq!(file.save_copy().await.unwrap(), "untitled.lock");
    assert_eq!(file.snapshot().path, "uv.lock");
    assert!(file.snapshot().is_dirty());
    file.reload().await.unwrap();
    assert_eq!(file.snapshot().text, "external\n");
    assert!(!file.snapshot().is_dirty());
    file.update_text(String::new());
    assert!(file.snapshot().is_dirty());
    file.save(false).await.unwrap();
    assert!(!file.snapshot().is_dirty());
    file.update_text("overwrite".into());
    assert!(matches!(file.save(false).await, Err(Error::Conflict)));
    file.save(true).await.unwrap();
    file.update_text("next".into());
    file.save(false).await.unwrap();
    assert!(!file.snapshot().is_dirty());
    server.await.unwrap();
}

#[tokio::test]
async fn edits_during_cancelled_save_remain_dirty_and_failed_save_preserves_them() {
    let (listener, config) = setup().await;
    let (arrived, arrival) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            assert!(request(&mut socket).await.0.starts_with("GET"));
            respond(&mut socket, "200 OK", model("1", "original")).await;
        }
        let (mut socket, _) = listener.accept().await.unwrap();
        assert_eq!(request(&mut socket).await.1["content"], "first");
        arrived.send(()).unwrap();
        released.await.unwrap();
        respond(&mut socket, "200 OK", model("2", "first")).await;
        for status in ["500 Internal Server Error", "200 OK"] {
            let (mut socket, _) = listener.accept().await.unwrap();
            assert!(request(&mut socket).await.0.starts_with("GET"));
            respond(&mut socket, "200 OK", model("2", "first")).await;
            let (mut socket, _) = listener.accept().await.unwrap();
            assert_eq!(request(&mut socket).await.1["content"], "later");
            respond(&mut socket, status, model("3", "later")).await;
        }
    });
    let file = TextFileController::open(config, "uv.lock").await.unwrap();
    file.update_text("first".into());
    let saving = file.clone();
    let caller = tokio::spawn(async move { saving.save(false).await });
    arrival.await.unwrap();
    file.update_text("later".into());
    caller.abort();
    release.send(()).unwrap();
    assert!(file.save(false).await.is_err());
    assert!(file.snapshot().is_dirty());
    assert_eq!(file.snapshot().text, "later");
    file.save(false).await.unwrap();
    assert!(!file.snapshot().is_dirty());
    server.await.unwrap();
}

#[tokio::test]
async fn reload_keeps_edits_arriving_during_fetch_and_deleted_file_conflicts() {
    let (listener, config) = setup().await;
    let (arrived, arrival) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        respond(&mut socket, "200 OK", model("1", "original")).await;
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        arrived.send(()).unwrap();
        released.await.unwrap();
        respond(&mut socket, "200 OK", model("2", "external")).await;
        let (mut socket, _) = listener.accept().await.unwrap();
        assert!(request(&mut socket).await.0.starts_with("GET"));
        respond(&mut socket, "404 Not Found", json!({"message":"deleted"})).await;
    });
    let file = TextFileController::open(config, "uv.lock").await.unwrap();
    let reloading = file.clone();
    let task = tokio::spawn(async move { reloading.reload().await });
    arrival.await.unwrap();
    file.update_text("keep me".into());
    release.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(Error::ChangedDuringReload)
    ));
    assert_eq!(file.snapshot().text, "keep me");
    assert!(file.snapshot().is_dirty());
    assert!(matches!(file.save(false).await, Err(Error::Conflict)));
    server.await.unwrap();
}
