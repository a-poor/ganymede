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
        read_get(&mut socket).await;
        respond(&mut socket, "404 Not Found").await;
        let (mut socket, _) = listener.accept().await.unwrap();
        let body = read_request(&mut socket).await;
        assert_eq!(body["content"]["cells"][0]["source"], "before");
        arrived.send(()).unwrap();
        released.await.unwrap();
        respond(&mut socket, "200 OK").await;
        for status in ["500 Internal Server Error", "200 OK"] {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_get(&mut socket).await;
            respond(&mut socket, "200 OK").await;
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

async fn read_get(socket: &mut TcpStream) {
    let (header, _) = read_http(socket).await;
    assert!(header.starts_with("GET "));
}

async fn read_http(socket: &mut TcpStream) -> (String, Value) {
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
async fn reply_json(socket: &mut TcpStream, status: &str, body: Value) {
    let body = body.to_string();
    socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
}
fn contents(version: &str, source: &str) -> Value {
    json!({"name":"test.ipynb", "path":"test.ipynb", "type":"notebook", "writable":true,
    "created":"", "last_modified":version, "content":{
        "nbformat":4,"nbformat_minor":5,"metadata":{},
        "cells":[{"id":"cell","cell_type":"code","metadata":{},"source":source,"execution_count":null,"outputs":[]}]
    }})
}

#[tokio::test]
async fn conflicts_preserve_local_work_and_support_copy_reload_and_explicit_overwrite() {
    use ganymede_core::notebook::Error;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = ServerConfig::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        for (method, version, source, expected_source) in [
            ("GET", "1", "original", None),      // open
            ("GET", "2", "external edit", None), // conflict: no PUT allowed
            ("POST", "3", "", None),             // copy allocates a separate server path
            ("PUT", "4", "", Some("local edit")),
            ("GET", "2", "external edit", None), // reload
            ("GET", "2", "external edit", None), // save after reload
            ("PUT", "5", "", Some("after reload")),
            ("GET", "6", "another external edit", None),
            ("PUT", "7", "", Some("explicit overwrite")),
            ("GET", "7", "explicit overwrite", None), // own save updated baseline
            ("PUT", "8", "", Some("next edit")),
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (header, body) = read_http(&mut socket).await;
            assert!(header.starts_with(method), "{header}");
            if let Some(source) = expected_source {
                assert_eq!(body["content"]["cells"][0]["source"], source);
            }
            let mut model = contents(version, source);
            if version == "3" || version == "4" {
                model["path"] = json!("Untitled.ipynb");
            }
            if version == "4" {
                assert!(header.starts_with("PUT /api/contents/Untitled.ipynb "));
            }
            reply_json(&mut socket, "200 OK", model).await;
        }
    });
    let controller = NotebookController::open(config, "test.ipynb")
        .await
        .unwrap();
    let cell = controller.snapshot().document.cells()[0].id().clone();
    controller.update_source(&cell, "local edit").unwrap();
    assert!(matches!(controller.save().await, Err(Error::SaveConflict)));
    assert!(controller.snapshot().document.is_dirty());
    assert_eq!(controller.save_copy().await.unwrap(), "Untitled.ipynb");
    assert!(controller.snapshot().document.is_dirty());
    assert_eq!(controller.snapshot().path, "test.ipynb");
    controller.reload().await.unwrap();
    assert_eq!(
        controller.snapshot().document.cell(&cell).unwrap().source(),
        "external edit"
    );
    assert!(!controller.snapshot().document.is_dirty());
    controller.update_source(&cell, "after reload").unwrap();
    controller.save().await.unwrap();
    controller
        .update_source(&cell, "explicit overwrite")
        .unwrap();
    assert!(matches!(controller.save().await, Err(Error::SaveConflict)));
    controller.overwrite().await.unwrap();
    assert!(!controller.snapshot().document.is_dirty());
    controller.update_source(&cell, "next edit").unwrap();
    controller.save().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn deleted_files_and_failed_version_checks_never_write() {
    use ganymede_core::notebook::Error;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = ServerConfig::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        for status in ["200 OK", "404 Not Found", "500 Internal Server Error"] {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_get(&mut socket).await;
            reply_json(&mut socket, status, contents("1", "original")).await;
        }
    });
    let controller = NotebookController::open(config, "test.ipynb")
        .await
        .unwrap();
    let cell = controller.snapshot().document.cells()[0].id().clone();
    controller.update_source(&cell, "local").unwrap();
    assert!(matches!(controller.save().await, Err(Error::SaveConflict)));
    assert!(matches!(controller.save().await, Err(Error::Jupyter(_))));
    assert!(controller.snapshot().document.is_dirty());
    server.await.unwrap();
}

#[tokio::test]
async fn reload_refuses_to_discard_edits_made_during_fetch() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = ServerConfig::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let (arrived, arrival) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_get(&mut socket).await;
        reply_json(&mut socket, "200 OK", contents("1", "original")).await;
        let (mut socket, _) = listener.accept().await.unwrap();
        read_get(&mut socket).await;
        arrived.send(()).unwrap();
        released.await.unwrap();
        reply_json(&mut socket, "200 OK", contents("2", "external")).await;
    });
    let controller = NotebookController::open(config, "test.ipynb")
        .await
        .unwrap();
    let cell = controller.snapshot().document.cells()[0].id().clone();
    let reload = controller.clone();
    let task = tokio::spawn(async move { reload.reload().await });
    arrival.await.unwrap();
    controller.update_source(&cell, "new local edit").unwrap();
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert_eq!(
        controller.snapshot().document.cell(&cell).unwrap().source(),
        "new local edit"
    );
    assert!(controller.snapshot().document.is_dirty());
    server.await.unwrap();
}

#[tokio::test]
async fn rejected_kernel_switch_keeps_connection_and_unsaved_document() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (finish, finished) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (header, _) = read_http(&mut socket).await;
        assert!(header.starts_with("GET /api/sessions/session "));
        reply_json(&mut socket, "200 OK", json!({"id":"session","path":"test.ipynb","name":"test.ipynb","type":"notebook","kernel":{"id":"original","name":"python3"}})).await;
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
        let Message::Text(text) = ws.next().await.unwrap().unwrap() else {
            panic!()
        };
        let info: KernelMessage = serde_json::from_str(&text).unwrap();
        let mut reply = KernelMessage::request("server", "shell", "kernel_info_reply", json!({}));
        reply.parent_header = serde_json::to_value(info.header).unwrap();
        ws.send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
            .await
            .unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let (header, _) = read_http(&mut socket).await;
        assert!(header.starts_with("GET /api/kernelspecs "));
        reply_json(&mut socket, "200 OK", json!({"default":"python3","kernelspecs":{"broken":{"name":"broken","spec":{"display_name":"Broken Python","language":"python"}}}})).await;
        let (mut socket, _) = listener.accept().await.unwrap();
        let (header, body) = read_http(&mut socket).await;
        assert!(header.starts_with("PATCH /api/sessions/session "));
        assert_eq!(body, json!({"kernel":{"name":"broken"}}));
        reply_json(
            &mut socket,
            "501 Not Implemented",
            json!({"message":"kernel executable not found"}),
        )
        .await;
        finished.await.unwrap();
        drop(ws);
    });
    let mut document = NotebookDocument::new();
    document
        .insert_cell(0, CellKind::Code, "unsaved = 42")
        .unwrap();
    let controller = NotebookController::new(
        ServerConfig::new(&format!("http://{address}")).unwrap(),
        "test.ipynb",
        document,
    )
    .unwrap();
    controller.attach_session("session").await.unwrap();
    let before = controller.snapshot().document.to_json();
    assert!(controller.change_kernel("broken".into()).await.is_err());
    assert_eq!(controller.snapshot().connection, ConnectionState::Connected);
    assert_eq!(
        controller.snapshot().session.as_ref().unwrap().kernel.id,
        "original"
    );
    assert_eq!(controller.snapshot().document.to_json(), before);
    finish.send(()).unwrap();
    server.await.unwrap();
}
