use futures_util::{SinkExt, StreamExt};
use ganymede_core::jupyter::{
    Error, JupyterRestClient, KernelClient, KernelEvent, ServerConfig, messaging::KernelMessage,
};
use serde_json::json;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
    },
};

#[tokio::test]
async fn rest_auth_base_path_errors_and_no_content() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for (expected, status, body) in [
            (
                "GET /prefix/api/contents/folder/a%20b.ipynb?content=1",
                "404 Not Found",
                r#"{"message":"missing notebook"}"#,
            ),
            (
                "POST /prefix/api/kernels/kernel/interrupt",
                "204 No Content",
                "",
            ),
            ("POST /prefix/api/shutdown", "200 OK", ""),
            (
                "GET /prefix/api/status",
                "200 OK",
                r#"{"started":"2026-11-03T12:00:00Z"}"#,
            ),
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                let mut buf = [0; 2048];
                let count = socket.read(&mut buf).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buf[..count]);
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.starts_with(expected), "{request}");
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: token test-token\r\n")
            );
            socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
    });
    let rest = JupyterRestClient::new(
        ServerConfig::new(&format!("http://{address}/prefix"))
            .unwrap()
            .with_token("test-token")
            .unwrap(),
    )
    .unwrap();
    match rest
        .get_contents("folder/a b.ipynb", true)
        .await
        .unwrap_err()
    {
        Error::Api { status, message } => {
            assert_eq!(status.as_u16(), 404);
            assert_eq!(message, "missing notebook");
        }
        error => panic!("{error}"),
    }
    rest.interrupt_kernel("kernel").await.unwrap();
    rest.shutdown_server().await.unwrap();
    assert_eq!(
        rest.server_status().await.unwrap().started,
        "2026-11-03T12:00:00Z"
    );
    server.await.unwrap();
}

#[tokio::test]
#[allow(clippy::result_large_err)] // Tungstenite fixes the handshake callback's error type.
async fn websocket_handles_ping_binary_and_close() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = accept_hdr_async(socket, |request: &Request, response: Response| {
            assert_eq!(request.uri().path(), "/prefix/api/kernels/kernel/channels");
            assert_eq!(request.headers()["authorization"], "token test-token");
            assert!(!request.headers().contains_key("sec-websocket-protocol"));
            assert!(request.uri().query().unwrap().contains("session_id="));
            Ok(response)
        })
        .await
        .unwrap();
        ws.send(Message::Ping(vec![1, 2, 3].into())).await.unwrap();
        let mut pong = false;
        let mut sent = false;
        while !pong || !sent {
            match ws.next().await.unwrap().unwrap() {
                Message::Pong(bytes) => {
                    assert_eq!(bytes.as_ref(), &[1, 2, 3]);
                    pong = true;
                }
                Message::Text(text) => {
                    let request: KernelMessage = serde_json::from_str(&text).unwrap();
                    assert_eq!(request.header.msg_type, "kernel_info_request");
                    let mut reply = KernelMessage::request(
                        "server",
                        "shell",
                        "extension_reply",
                        json!({"custom":true}),
                    );
                    reply.parent_header = serde_json::to_value(request.header).unwrap();
                    let data = serde_json::to_vec(&reply).unwrap();
                    let mut binary = Vec::new();
                    for n in [2_u32, 12, 12 + data.len() as u32] {
                        binary.extend_from_slice(&n.to_be_bytes());
                    }
                    binary.extend_from_slice(&data);
                    binary.extend_from_slice(&[0, 255, 42]);
                    ws.send(Message::Binary(binary.into())).await.unwrap();
                    sent = true;
                }
                message => panic!("unexpected {message:?}"),
            }
        }
        ws.close(None).await.unwrap();
    });
    let config = ServerConfig::new(&format!("http://{address}/prefix/"))
        .unwrap()
        .with_token("test-token")
        .unwrap();
    let (kernel, mut events) = KernelClient::connect(config, "kernel").await.unwrap();
    let id = kernel.kernel_info().await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    let KernelEvent::Message(message) = event else {
        panic!("{event:?}")
    };
    assert_eq!(message.parent_id(), Some(id.as_str()));
    assert_eq!(message.buffers, vec![vec![0, 255, 42]]);
    assert!(matches!(events.recv().await, Some(KernelEvent::Closed)));
    assert!(events.recv().await.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn overflow_is_a_terminal_error_not_silent_loss() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sent, ready) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        let message = KernelMessage::request(
            "server",
            "iopub",
            "status",
            json!({"execution_state":"busy"}),
        );
        for _ in 0..3 {
            ws.send(Message::Text(
                serde_json::to_string(&message).unwrap().into(),
            ))
            .await
            .unwrap();
        }
        // Wait for the client to close after overflowing its queue.
        let _ = ws.next().await;
        let _ = sent.send(());
    });
    let mut config = ServerConfig::new(&format!("http://{address}")).unwrap();
    config.event_capacity = 1;
    let (_kernel, mut events) = KernelClient::connect(config, "kernel").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(events.recv().await, Some(KernelEvent::Message(_))));
    assert!(matches!(
        events.recv().await,
        Some(KernelEvent::Failed(Error::EventOverflow))
    ));
    assert!(events.recv().await.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn contents_binary_download_authenticates_and_enforces_size_limit() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                let mut buf = [0; 2048];
                let count = socket.read(&mut buf).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buf[..count]);
            }
            let request = String::from_utf8(bytes).unwrap().to_ascii_lowercase();
            assert!(
                request.starts_with(
                    "get /prefix/api/contents/images/a%20b.png?content=1&format=base64 "
                )
            );
            assert!(request.contains("authorization: token image-test\r\n"));
            let body = json!({"name":"a b.png", "path":"images/a b.png", "type":"file", "writable":true, "created":"", "last_modified":"", "format":"base64", "content":"YWJjZA==\n"}).to_string();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let rest = JupyterRestClient::new(
        ServerConfig::new(&format!("http://{address}/prefix"))
            .unwrap()
            .with_token("image-test")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        rest.read_file_bytes("images/a b.png", 4).await.unwrap(),
        b"abcd"
    );
    assert!(rest.read_file_bytes("images/a b.png", 3).await.is_err());
    server.await.unwrap();
}
