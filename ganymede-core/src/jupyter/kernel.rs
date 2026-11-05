use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

use super::{
    Error, Result, ServerConfig, codec,
    messaging::{ExecuteRequest, KernelMessage},
};

#[derive(Debug)]
pub enum KernelEvent {
    Message(Box<KernelMessage>),
    /// Normal peer close, explicit close, or loss of every command handle.
    Closed,
    /// Terminal error, delivered even when the ordinary event queue overflowed.
    Failed(Error),
}

pub struct KernelEvents {
    messages: mpsc::Receiver<KernelMessage>,
    terminal: Option<oneshot::Receiver<Result<()>>>,
}

impl KernelEvents {
    /// Drains queued messages, then yields exactly one terminal event.
    pub async fn recv(&mut self) -> Option<KernelEvent> {
        if let Some(message) = self.messages.recv().await {
            return Some(KernelEvent::Message(Box::new(message)));
        }
        let terminal = self.terminal.take()?;
        Some(match terminal.await {
            Ok(Ok(())) => KernelEvent::Closed,
            Ok(Err(error)) => KernelEvent::Failed(error),
            Err(_) => KernelEvent::Failed(Error::Disconnected),
        })
    }
}

enum Command {
    Send(Box<KernelMessage>, oneshot::Sender<Result<()>>),
    Close(oneshot::Sender<Result<()>>),
}

/// Cloneable command handle. The connection lives until closed or all handles
/// (or its event receiver) are dropped. It never shuts down the server's kernel.
#[derive(Clone)]
pub struct KernelClient {
    commands: mpsc::Sender<Command>,
    session_id: String,
}

impl KernelClient {
    pub async fn connect(config: ServerConfig, kernel_id: &str) -> Result<(Self, KernelEvents)> {
        Self::connect_with_session(config, kernel_id, uuid::Uuid::new_v4().to_string()).await
    }

    /// Reuse the frontend message-session ID when explicitly reconnecting.
    /// This is not the REST session ID. Requests are never replayed.
    pub async fn connect_with_session(
        config: ServerConfig,
        kernel_id: &str,
        session_id: String,
    ) -> Result<(Self, KernelEvents)> {
        if config.event_capacity == 0 || config.max_message_bytes == 0 || session_id.is_empty() {
            return Err(Error::Config(
                "queue capacity, message limit, and session ID must be nonempty".into(),
            ));
        }
        let mut request = config
            .websocket_url(kernel_id, &session_id)?
            .as_str()
            .into_client_request()?;
        request.headers_mut().extend(config.headers());
        // No subprotocol header selects Jupyter's required default protocol.
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(config.max_message_bytes))
            .max_frame_size(Some(config.max_message_bytes));
        let (socket, _) = tokio::time::timeout(config.connect_timeout, async {
            if let Some(proxy) = config.socks_proxy() {
                let stream =
                    super::socks::connect(proxy, &config.websocket_url(kernel_id, &session_id)?)
                        .await
                        .map_err(tokio_tungstenite::tungstenite::Error::Io)?;
                Ok::<_, Error>(
                    tokio_tungstenite::client_async_tls_with_config(
                        request,
                        stream,
                        Some(ws_config),
                        None,
                    )
                    .await?,
                )
            } else {
                Ok(connect_async_with_config(request, Some(ws_config), false).await?)
            }
        })
        .await
        .map_err(|_| Error::Timeout)??;
        let (commands, mut command_rx) = mpsc::channel::<Command>(32);
        let (messages, message_rx) = mpsc::channel(config.event_capacity);
        let (terminal_tx, terminal) = oneshot::channel();
        let (mut writer, mut reader) = socket.split();
        tokio::spawn(async move {
            let outcome: Result<()> = async {
                loop {
                    tokio::select! {
                        _ = messages.closed() => { break; }
                        command = command_rx.recv() => {
                            match command {
                                Some(Command::Send(message, ack)) => {
                                    let frame = match codec::encode(&message) {
                                        Ok(frame) if frame.len() <= config.max_message_bytes => frame,
                                        Ok(_) => { let _ = ack.send(Err(Error::Protocol("outgoing message exceeds configured limit".into()))); continue; }
                                        Err(error) => { let _ = ack.send(Err(error)); continue; }
                                    };
                                    let sent = tokio::time::timeout(config.request_timeout, writer.send(frame)).await;
                                    match sent {
                                        Ok(Ok(())) => { let _ = ack.send(Ok(())); }
                                        Ok(Err(error)) => { let _ = ack.send(Err(Error::Disconnected)); return Err(error.into()); }
                                        Err(_) => { let _ = ack.send(Err(Error::Timeout)); return Err(Error::Timeout); }
                                    }
                                }
                                Some(Command::Close(ack)) => {
                                    let result = tokio::time::timeout(config.request_timeout, writer.close()).await;
                                    match result {
                                        Ok(Ok(())) => { let _ = ack.send(Ok(())); break; }
                                        Ok(Err(error)) => { let _ = ack.send(Err(Error::Disconnected)); return Err(error.into()); }
                                        Err(_) => { let _ = ack.send(Err(Error::Timeout)); return Err(Error::Timeout); }
                                    }
                                }
                                None => { break; }
                            }
                        }
                        frame = reader.next() => {
                            let message = match frame {
                                Some(Ok(Message::Text(text))) => serde_json::from_str(&text)?,
                                Some(Ok(Message::Binary(bytes))) => codec::decode_binary(&bytes)?,
                                Some(Ok(Message::Ping(_))) => {
                                    // Tungstenite queued the matching Pong while reading.
                                    tokio::time::timeout(config.request_timeout, writer.flush()).await.map_err(|_| Error::Timeout)??;
                                    continue;
                                }
                                Some(Ok(Message::Pong(_))) => continue,
                                Some(Ok(Message::Close(_))) => {
                                    let _ = tokio::time::timeout(config.request_timeout, writer.flush()).await;
                                    break;
                                }
                                Some(Ok(Message::Frame(_))) => continue,
                                Some(Err(error)) => return Err(error.into()),
                                None => break,
                            };
                            match messages.try_send(message) {
                                Ok(()) => (),
                                Err(mpsc::error::TrySendError::Full(_)) => return Err(Error::EventOverflow),
                                Err(mpsc::error::TrySendError::Closed(_)) => break,
                            }
                        }
                    }
                }
                Ok(())
            }.await;
            // A best-effort close cannot keep a dropped GUI alive indefinitely.
            let _ = tokio::time::timeout(Duration::from_secs(1), writer.close()).await;
            let _ = terminal_tx.send(outcome);
        });
        Ok((
            Self {
                commands,
                session_id,
            },
            KernelEvents {
                messages: message_rx,
                terminal: Some(terminal),
            },
        ))
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Resolves when the frame is sent, not when the kernel finishes execution.
    /// Dropping this future after enqueueing does not cancel execution.
    pub async fn send(&self, message: KernelMessage) -> Result<String> {
        let id = message.header.msg_id.clone();
        let (ack, done) = oneshot::channel();
        self.commands
            .send(Command::Send(Box::new(message), ack))
            .await
            .map_err(|_| Error::Disconnected)?;
        done.await.map_err(|_| Error::Disconnected)??;
        Ok(id)
    }
    pub async fn request(&self, channel: &str, msg_type: &str, content: Value) -> Result<String> {
        self.send(KernelMessage::request(
            &self.session_id,
            channel,
            msg_type,
            content,
        ))
        .await
    }
    pub async fn execute(&self, request: ExecuteRequest) -> Result<String> {
        self.request("shell", "execute_request", serde_json::to_value(request)?)
            .await
    }
    pub async fn kernel_info(&self) -> Result<String> {
        self.request("shell", "kernel_info_request", json!({}))
            .await
    }
    /// Cursor positions count Unicode code points, not UTF-8 bytes.
    pub async fn complete(&self, code: &str, cursor_pos: usize) -> Result<String> {
        self.request(
            "shell",
            "complete_request",
            json!({"code":code,"cursor_pos":cursor_pos}),
        )
        .await
    }
    pub async fn inspect(&self, code: &str, cursor_pos: usize, detail_level: u8) -> Result<String> {
        self.request(
            "shell",
            "inspect_request",
            json!({"code":code,"cursor_pos":cursor_pos,"detail_level":detail_level}),
        )
        .await
    }
    pub async fn input_reply(&self, input_request: &KernelMessage, value: &str) -> Result<String> {
        if input_request.channel != "stdin" || input_request.header.msg_type != "input_request" {
            return Err(Error::Protocol(
                "input_reply requires an input_request".into(),
            ));
        }
        let mut message = KernelMessage::request(
            &self.session_id,
            "stdin",
            "input_reply",
            json!({"value":value}),
        );
        message.parent_header = serde_json::to_value(&input_request.header)?;
        self.send(message).await
    }
    pub async fn close(&self) -> Result<()> {
        let (ack, done) = oneshot::channel();
        self.commands
            .send(Command::Close(ack))
            .await
            .map_err(|_| Error::Disconnected)?;
        done.await.map_err(|_| Error::Disconnected)?
    }
}
