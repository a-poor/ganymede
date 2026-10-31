use ganymede_core::{
    jupyter::{JupyterRestClient, ServerConfig},
    notebook::{CellId, NotebookController},
    text_file::TextFileController,
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
                return (
                    header,
                    if length == 0 {
                        Value::Null
                    } else {
                        serde_json::from_slice(&bytes[start + 4..start + 4 + length]).unwrap()
                    },
                );
            }
        }
    }
}
fn model(path: &str, version: &str, kind: &str, content: Value) -> Value {
    json!({"name":ganymede_core::files::name(path),"path":path,"type":kind,"writable":true,"created":"",
        "last_modified":version,"format":if kind == "notebook" {"json"} else {"text"},"content":content})
}
fn notebook(source: &str) -> Value {
    json!({"nbformat":4,"nbformat_minor":5,"metadata":{},"cells":[{"id":"cell","cell_type":"code","source":source,"metadata":{},"execution_count":null,"outputs":[]}]})
}
struct Step {
    request: &'static str,
    status: &'static str,
    body: Value,
    sent: Value,
}
async fn server(steps: Vec<Step>) -> (ServerConfig, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = ServerConfig::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        for step in steps {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (header, body) = request(&mut socket).await;
            assert!(header.starts_with(step.request), "{header}");
            assert_eq!(body, step.sent);
            let body = step.body.to_string();
            socket.write_all(format!("HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",step.status,body.len()).as_bytes()).await.unwrap();
        }
    });
    (config, server)
}
fn step(request: &'static str, status: &'static str, body: Value, sent: Value) -> Step {
    Step {
        request,
        status,
        body,
        sent,
    }
}

#[tokio::test]
async fn notebook_save_as_saves_local_edits_and_retargets_future_saves_without_overwriting() {
    let (config, server) = server(vec![
        step(
            "GET /api/contents/a.ipynb?",
            "200 OK",
            model("a.ipynb", "1", "notebook", notebook("original")),
            Value::Null,
        ),
        step(
            "GET /api/contents/b.ipynb?",
            "404 Not Found",
            json!({}),
            Value::Null,
        ),
        step(
            "PUT /api/contents/b.ipynb ",
            "200 OK",
            model("b.ipynb", "2", "notebook", Value::Null),
            json!({"type":"notebook","format":"json","content":notebook("local edit")}),
        ),
        step(
            "GET /api/contents/b.ipynb?",
            "200 OK",
            model("b.ipynb", "2", "notebook", Value::Null),
            Value::Null,
        ),
        step(
            "PUT /api/contents/b.ipynb ",
            "200 OK",
            model("b.ipynb", "3", "notebook", Value::Null),
            json!({"type":"notebook","format":"json","content":notebook("later")}),
        ),
        step(
            "GET /api/contents/existing.ipynb?",
            "200 OK",
            model("existing.ipynb", "1", "notebook", Value::Null),
            Value::Null,
        ),
    ])
    .await;
    let controller = NotebookController::open(config, "a.ipynb").await.unwrap();
    let id: CellId = controller.snapshot().document.cells()[0].id().clone();
    controller.update_source(&id, "local edit").unwrap();
    controller.save_as("b.ipynb".into()).await.unwrap();
    assert_eq!(controller.snapshot().path, "b.ipynb");
    assert!(!controller.snapshot().document.is_dirty());
    controller.update_source(&id, "later").unwrap();
    controller.save().await.unwrap();
    controller.update_source(&id, "keep unsaved").unwrap();
    assert!(controller.save_as("existing.ipynb".into()).await.is_err());
    assert_eq!(controller.snapshot().path, "b.ipynb");
    assert!(controller.snapshot().document.is_dirty());
    server.await.unwrap();
}

#[tokio::test]
async fn text_save_as_keeps_original_encoding_and_retargets_only_on_success() {
    let (config, server) = server(vec![
        step(
            "GET /api/contents/a.py?",
            "200 OK",
            model("a.py", "1", "file", json!("\u{feff}old\r\n")),
            Value::Null,
        ),
        step(
            "GET /api/contents/b.py?",
            "404 Not Found",
            json!({}),
            Value::Null,
        ),
        step(
            "PUT /api/contents/b.py ",
            "200 OK",
            model("b.py", "2", "file", Value::Null),
            json!({"type":"file","format":"text","content":"\u{feff}new\r\n"}),
        ),
        step(
            "GET /api/contents/c.py?",
            "404 Not Found",
            json!({}),
            Value::Null,
        ),
        step(
            "PUT /api/contents/c.py ",
            "500 Internal Server Error",
            json!({"message":"failed"}),
            json!({"type":"file","format":"text","content":"\u{feff}latest\r\n"}),
        ),
    ])
    .await;
    let file = TextFileController::open(config, "a.py").await.unwrap();
    file.update_text("new\n".into());
    file.save_as("b.py".into()).await.unwrap();
    assert_eq!(file.path(), "b.py");
    assert!(!file.status().1);
    file.update_text("latest\n".into());
    assert!(file.save_as("c.py".into()).await.is_err());
    assert_eq!(file.path(), "b.py");
    assert!(file.status().1);
    server.await.unwrap();
}

#[tokio::test]
async fn rename_folder_create_and_copy_use_contents_api_and_refuse_collisions() {
    let (config, server) = server(vec![
        step(
            "GET /api/contents/new%20folder?",
            "404 Not Found",
            json!({}),
            Value::Null,
        ),
        step(
            "PATCH /api/contents/old%20folder ",
            "200 OK",
            model("new folder", "1", "directory", Value::Null),
            json!({"path":"new folder"}),
        ),
        step(
            "GET /api/contents/existing?",
            "200 OK",
            model("existing", "1", "directory", Value::Null),
            Value::Null,
        ),
        step(
            "GET /api/contents/new%20folder/child?",
            "404 Not Found",
            json!({}),
            Value::Null,
        ),
        step(
            "PUT /api/contents/new%20folder/child ",
            "200 OK",
            model("new folder/child", "1", "directory", Value::Null),
            json!({"type":"directory"}),
        ),
        step(
            "GET /api/contents/copy.txt?",
            "404 Not Found",
            json!({}),
            Value::Null,
        ),
        step(
            "GET /api/contents/source.txt?",
            "200 OK",
            model("source.txt", "1", "file", json!("hello")),
            Value::Null,
        ),
        step(
            "PUT /api/contents/copy.txt ",
            "200 OK",
            model("copy.txt", "1", "file", Value::Null),
            json!({"type":"file","format":"text","content":"hello"}),
        ),
    ])
    .await;
    let rest = JupyterRestClient::new(config).unwrap();
    assert!(
        rest.rename_contents("old folder", "old folder/child")
            .await
            .is_err()
    );
    assert_eq!(
        rest.rename_contents("old folder", "new folder")
            .await
            .unwrap()
            .path,
        "new folder"
    );
    assert!(
        rest.rename_contents("new folder", "existing")
            .await
            .is_err()
    );
    rest.create_named_directory("new folder/child")
        .await
        .unwrap();
    rest.copy_contents("source.txt", "copy.txt").await.unwrap();
    server.await.unwrap();
}
