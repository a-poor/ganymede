use reqwest::StatusCode;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid server configuration: {0}")]
    Config(String),
    #[error("HTTP transport failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Jupyter returned HTTP {status}: {message}")]
    Api { status: StatusCode, message: String },
    #[error("WebSocket transport failed: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid kernel protocol: {0}")]
    Protocol(String),
    #[error("connection closed")]
    Disconnected,
    #[error("kernel event queue overflowed; connection closed to avoid silent output loss")]
    EventOverflow,
    #[error("operation timed out")]
    Timeout,
}
