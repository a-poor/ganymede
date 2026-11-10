use ganymede_core::jupyter::{ExportFormat, JupyterRestClient, ServerConfig};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::test]
async fn export_posts_snapshot_without_saving_or_executing_and_preserves_response_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let notebook = json!({"nbformat":4,"nbformat_minor":5,"metadata":{},"cells":[
        {"id":"a","cell_type":"code","metadata":{},"source":"unsaved text","execution_count":3,
        "outputs":[{"output_type":"stream","name":"stdout","text":"saved output"}]}
    ]});
    let expected = notebook.clone();
    let server = tokio::spawn(async move {
        for (format, mime, body) in [
            (
                "html",
                "text/html; charset=utf-8",
                "<html>unsaved text</html>",
            ),
            ("markdown", "text/markdown", "# Notebook"),
            ("markdown", "application/zip", "PK\x03\x04assets"),
            ("webpdf", "text/html; charset=utf-8", "%PDF-1.7\n"),
            ("webpdf", "text/html", "<html>login</html>"),
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let end = loop {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
            assert!(headers.starts_with(&format!("post /prefix/nbconvert/{format} ")));
            assert!(headers.contains("authorization: token test-token\r\n"));
            let len: usize = headers
                .lines()
                .find_map(|l| l.strip_prefix("content-length: "))
                .unwrap()
                .parse()
                .unwrap();
            while bytes.len() < end + len {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let payload: Value = serde_json::from_slice(&bytes[end..end + len]).unwrap();
            assert_eq!(payload, json!({"name":"demo.ipynb", "content":expected}));
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
    });
    let rest = JupyterRestClient::new(
        ServerConfig::new(&format!("http://{address}/prefix/"))
            .unwrap()
            .with_token("test-token")
            .unwrap(),
    )
    .unwrap();
    for (format, extension, body) in [
        (ExportFormat::Html, "html", "<html>unsaved text</html>"),
        (ExportFormat::Markdown, "md", "# Notebook"),
        (ExportFormat::Markdown, "zip", "PK\x03\x04assets"),
        (ExportFormat::Pdf, "pdf", "%PDF-1.7\n"),
    ] {
        let export = rest
            .export_notebook("demo.ipynb", notebook.clone(), format)
            .await
            .unwrap();
        assert_eq!(export.extension, extension);
        assert_eq!(export.bytes, body.as_bytes());
    }
    assert!(
        rest.export_notebook("demo.ipynb", notebook, ExportFormat::Pdf)
            .await
            .is_err()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn export_surfaces_missing_dependencies_and_timeouts_without_retrying() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for status in [Some("500 Internal Server Error"), None] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 4096];
            assert!(socket.read(&mut buf).await.unwrap() > 0);
            if let Some(status) = status {
                let body = r#"{"message":"Could not import nbconvert"}"#;
                socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    });
    let mut config = ServerConfig::new(&format!("http://{address}")).unwrap();
    config.request_timeout = std::time::Duration::from_millis(100);
    let rest = JupyterRestClient::new(config).unwrap();
    assert!(
        matches!(rest.export_notebook("a.ipynb", json!({}), ExportFormat::Html).await,
        Err(ganymede_core::jupyter::Error::Api { status, .. }) if status.as_u16()==500)
    );
    assert!(
        matches!(rest.export_notebook("a.ipynb", json!({}), ExportFormat::Html).await,
        Err(ganymede_core::jupyter::Error::Http(e)) if e.is_timeout())
    );
    server.await.unwrap();
}
