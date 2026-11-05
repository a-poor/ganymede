use ganymede_core::jupyter::{JupyterRestClient, KernelClient, ServerConfig};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn accept_proxy(listener: TcpListener, expected_host: &str, expected_port: u16) -> TcpStream {
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut greeting = [0; 3];
    stream.read_exact(&mut greeting).await.unwrap();
    assert_eq!(greeting, [5, 1, 0]);
    stream.write_all(&[5, 0]).await.unwrap();
    let mut header = [0; 5];
    stream.read_exact(&mut header).await.unwrap();
    assert_eq!(&header[..4], &[5, 1, 0, 3]);
    let mut host = vec![0; header[4] as usize];
    stream.read_exact(&mut host).await.unwrap();
    assert_eq!(host, expected_host.as_bytes());
    assert_eq!(stream.read_u16().await.unwrap(), expected_port);
    stream
        .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 1])
        .await
        .unwrap();
    stream
}

#[tokio::test]
async fn rest_uses_remote_dns_and_keeps_authentication_and_deployment_path() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut stream = accept_proxy(listener, "not-resolvable.invalid", 8888).await;
        let mut data = Vec::new();
        while !data.ends_with(b"\r\n\r\n") {
            data.push(stream.read_u8().await.unwrap());
        }
        let request = String::from_utf8(data).unwrap().to_lowercase();
        assert!(request.starts_with("get /user/alice/api/status "));
        assert!(request.contains("host: not-resolvable.invalid:8888"));
        assert!(request.contains("authorization: token fixture-secret"));
        let body = r#"{"started":"launch"}"#;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let config = ServerConfig::new("http://not-resolvable.invalid:8888/user/alice/")
        .unwrap()
        .with_token("fixture-secret")
        .unwrap()
        .with_socks_proxy(proxy, "ssh-fixture".into())
        .unwrap();
    assert_eq!(config.connection_key(), "ssh-fixture");
    assert_eq!(
        JupyterRestClient::new(config)
            .unwrap()
            .server_status()
            .await
            .unwrap()
            .started,
        "launch"
    );
    server.await.unwrap();
}

#[tokio::test]
#[allow(clippy::result_large_err)] // tungstenite callback requires this error type
async fn kernel_websocket_uses_the_same_proxy_and_original_host() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let stream = accept_proxy(listener, "not-resolvable.invalid", 8888).await;
        let socket = tokio_tungstenite::accept_hdr_async(
            stream,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                assert!(
                    request
                        .uri()
                        .path()
                        .starts_with("/prefix/api/kernels/kernel/channels")
                );
                assert_eq!(request.headers()["host"], "not-resolvable.invalid:8888");
                assert_eq!(request.headers()["authorization"], "token fixture-secret");
                Ok(response)
            },
        )
        .await
        .unwrap();
        drop(socket);
    });
    let config = ServerConfig::new("http://not-resolvable.invalid:8888/prefix/")
        .unwrap()
        .with_token("fixture-secret")
        .unwrap()
        .with_socks_proxy(proxy, "ssh-fixture".into())
        .unwrap();
    let (client, _) = KernelClient::connect(config, "kernel").await.unwrap();
    drop(client);
    server.await.unwrap();
}

#[tokio::test]
async fn unavailable_proxy_never_falls_back_to_direct_rest_or_websocket() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = closed.local_addr().unwrap();
    drop(closed);
    let config = ServerConfig::new(&format!("http://{}/", target.local_addr().unwrap()))
        .unwrap()
        .with_socks_proxy(proxy, "ssh-fixture".into())
        .unwrap();
    assert!(
        JupyterRestClient::new(config.clone())
            .unwrap()
            .server_status()
            .await
            .is_err()
    );
    assert!(KernelClient::connect(config, "kernel").await.is_err());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), target.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn https_and_secure_websockets_still_require_tls_through_the_proxy() {
    for websocket in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut stream = accept_proxy(listener, "not-resolvable.invalid", 443).await;
            // A TLS ClientHello, never an HTTP request carrying credentials.
            assert_eq!(stream.read_u8().await.unwrap(), 0x16);
        });
        let config = ServerConfig::new("https://not-resolvable.invalid/")
            .unwrap()
            .with_token("fixture-secret")
            .unwrap()
            .with_socks_proxy(proxy, "ssh-fixture".into())
            .unwrap();
        if websocket {
            assert!(KernelClient::connect(config, "kernel").await.is_err());
        } else {
            assert!(
                JupyterRestClient::new(config)
                    .unwrap()
                    .server_status()
                    .await
                    .is_err()
            );
        }
        server.await.unwrap();
    }
}
