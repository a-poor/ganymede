use futures_util::{SinkExt, StreamExt};
use ganymede_core::{
    jupyter::ServerConfig,
    notebook::{CellKind, NotebookController, NotebookDocument, NotebookEvent, SaveState},
};
use ganymede_core::{
    jupyter::messaging::KernelMessage,
    notebook::{ConnectionState, ExecutionPhase},
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn abrupt_disconnect_preserves_edits_and_marks_execution_unknown() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (disconnect, disconnected) = tokio::sync::oneshot::channel();
    let (executed, execution) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = vec![];
        while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            let mut chunk = [0; 2048];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
        }
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .starts_with("GET /api/sessions/session ")
        );
        let body = json!({"id":"session","path":"test.ipynb","name":"test.ipynb","type":"notebook","kernel":{"id":"kernel","name":"python3"}}).to_string();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        drop(socket);
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        let Message::Text(text) = ws.next().await.unwrap().unwrap() else {
            panic!()
        };
        let info: KernelMessage = serde_json::from_str(&text).unwrap();
        assert_eq!(info.header.msg_type, "kernel_info_request");
        let mut reply = KernelMessage::request("server", "shell", "kernel_info_reply", json!({}));
        reply.parent_header = serde_json::to_value(info.header).unwrap();
        ws.send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
            .await
            .unwrap();
        let Message::Text(text) = ws.next().await.unwrap().unwrap() else {
            panic!()
        };
        let request: KernelMessage = serde_json::from_str(&text).unwrap();
        assert_eq!(request.header.msg_type, "execute_request");
        executed.send(()).unwrap();
        disconnected.await.unwrap();
        // Drop TCP without a WebSocket close frame.
        drop(ws);
    });
    let mut doc = NotebookDocument::new();
    let cell = doc.insert_cell(0, CellKind::Code, "original").unwrap();
    let controller = NotebookController::new(
        ServerConfig::new(&format!("http://{address}/")).unwrap(),
        "test.ipynb",
        doc,
    )
    .unwrap();
    controller.attach_session("session").await.unwrap();
    controller.run_cell(&cell, false).await.unwrap();
    execution.await.unwrap();
    controller.update_source(&cell, "unsaved edit").unwrap();
    let mut snapshots = controller.watch();
    disconnect.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                snapshots.borrow_and_update().connection,
                ConnectionState::Disconnected { .. }
            ) {
                break;
            }
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let snapshot = controller.snapshot();
    assert_eq!(snapshot.executions[&cell].phase, ExecutionPhase::Unknown);
    assert_eq!(
        snapshot.document.cell(&cell).unwrap().source(),
        "unsaved edit"
    );
    assert!(snapshot.document.is_dirty());
    assert!(controller.run_cell(&cell, false).await.is_err());
    server.await.unwrap();
}

async fn read_request(socket: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let count = socket.read(&mut chunk).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(start) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = std::str::from_utf8(&bytes[..start]).unwrap();
            assert!(header.starts_with("PUT /prefix/api/contents/test.ipynb "));
            let length: usize = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_owned)
                })
                .unwrap()
                .parse()
                .unwrap();
            if bytes.len() >= start + 4 + length {
                return serde_json::from_slice(&bytes[start + 4..start + 4 + length]).unwrap();
            }
        }
    }
}
async fn respond(socket: &mut TcpStream, status: &str) {
    let body = json!({"name":"test.ipynb","path":"test.ipynb","type":"notebook","writable":true,"created":"","last_modified":"","mimetype":null,"format":null,"size":null,"content":null});
    let body = body.to_string();
    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
}

#[tokio::test]
async fn save_handles_concurrent_edits_cancellation_and_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (arrived, arrival) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let body = read_request(&mut socket).await;
        assert_eq!(body["content"]["cells"][0]["source"], "before");
        arrived.send(()).unwrap();
        released.await.unwrap();
        respond(&mut socket, "200 OK").await;
        for status in ["500 Internal Server Error", "200 OK"] {
            let (mut socket, _) = listener.accept().await.unwrap();
            assert_eq!(
                read_request(&mut socket).await["content"]["cells"][0]["source"],
                "after"
            );
            respond(&mut socket, status).await;
        }
    });
    let mut document = NotebookDocument::new();
    let cell = document.insert_cell(0, CellKind::Code, "before").unwrap();
    let controller = NotebookController::new(
        ServerConfig::new(&format!("http://{address}/prefix/")).unwrap(),
        "test.ipynb",
        document,
    )
    .unwrap();
    let mut snapshots = controller.watch();
    let saving = controller.clone();
    let caller = tokio::spawn(async move { saving.save().await });
    tokio::time::timeout(Duration::from_secs(5), arrival)
        .await
        .unwrap()
        .unwrap();
    controller.update_source(&cell, "after").unwrap();
    caller.abort(); // The actual save still completes and acknowledges its captured revision.
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(snapshots.borrow_and_update().save, SaveState::Idle) {
                break;
            }
            snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(controller.snapshot().document.is_dirty());
    assert!(controller.save().await.is_err());
    assert!(matches!(
        controller.snapshot().save,
        SaveState::Failed { .. }
    ));
    assert!(controller.snapshot().document.is_dirty());
    controller.save().await.unwrap();
    assert!(!controller.snapshot().document.is_dirty());
    server.await.unwrap();
}

#[tokio::test]
async fn offline_edits_snapshots_and_event_lag_are_explicit() {
    let controller = NotebookController::new(
        ServerConfig::new("http://127.0.0.1:1/").unwrap(),
        "offline.ipynb",
        NotebookDocument::new(),
    )
    .unwrap();
    let mut events = controller.subscribe();
    let cell = controller.insert_cell(0, CellKind::Code, "1").unwrap();
    assert!(matches!(
        events.recv().await.unwrap(),
        NotebookEvent::DocumentChanged { .. }
    ));
    assert!(controller.run_cell(&cell, false).await.is_err());
    for i in 0..300 {
        controller.update_source(&cell, i.to_string()).unwrap();
    }
    assert!(matches!(
        events.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
    ));
    assert_eq!(
        controller.snapshot().document.cell(&cell).unwrap().source(),
        "299"
    );
    assert!(controller.snapshot().document.is_dirty());
    controller.remove_cell(&cell).unwrap();
    assert!(controller.snapshot().document.cells().is_empty());
}
