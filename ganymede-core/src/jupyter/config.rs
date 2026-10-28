use std::{fmt, time::Duration};

use reqwest::{
    Url,
    header::{AUTHORIZATION, HeaderValue},
};

use super::{Error, Result};

/// A server root, including any deployment prefix (for example `/user/alice/`).
#[derive(Clone)]
pub struct ServerConfig {
    base_url: Url,
    authorization: Option<HeaderValue>,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub event_capacity: usize,
    pub max_message_bytes: usize,
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig")
            .field("base_url", &self.base_url)
            .field("authenticated", &self.authorization.is_some())
            .finish_non_exhaustive()
    }
}

impl ServerConfig {
    /// Tokens are supplied separately, never embedded in the URL.
    pub fn new(base_url: &str) -> Result<Self> {
        let mut url = Url::parse(base_url).map_err(|_| Error::Config("invalid URL".into()))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Config(
                "expected an HTTP(S) server root without credentials, query, or fragment".into(),
            ));
        }
        if !url.path().ends_with('/') {
            url.path_segments_mut()
                .map_err(|_| Error::Config("invalid base path".into()))?
                .push("");
        }
        Ok(Self {
            base_url: url,
            authorization: None,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            event_capacity: 256,
            max_message_bytes: 64 * 1024 * 1024,
        })
    }

    pub fn with_token(mut self, token: &str) -> Result<Self> {
        if token.is_empty() {
            return Err(Error::Config("token must not be empty".into()));
        }
        let mut value = HeaderValue::from_str(&format!("token {token}"))
            .map_err(|_| Error::Config("invalid token header".into()))?;
        value.set_sensitive(true);
        self.authorization = Some(value);
        Ok(self)
    }

    pub(crate) fn headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(value) = &self.authorization {
            headers.insert(AUTHORIZATION, value.clone());
        }
        headers
    }

    pub(crate) fn endpoint(&self, segments: &[&str]) -> Result<Url> {
        let mut url = self.base_url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| Error::Config("invalid base URL".into()))?;
            path.pop_if_empty();
            for segment in segments {
                if matches!(*segment, "." | "..") {
                    return Err(Error::Config("dot path segments are not allowed".into()));
                }
                path.push(segment);
            }
        }
        Ok(url)
    }

    pub(crate) fn websocket_url(&self, kernel_id: &str, session_id: &str) -> Result<Url> {
        let mut url = self.endpoint(&["api", "kernels", kernel_id, "channels"])?;
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|_| Error::Config("invalid WebSocket URL".into()))?;
        url.query_pairs_mut().append_pair("session_id", session_id);
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_and_credentials() {
        let config = ServerConfig::new("https://example.org/user/alice")
            .unwrap()
            .with_token("secret")
            .unwrap();
        assert_eq!(
            config
                .endpoint(&["api", "contents", "a b#?.ipynb"])
                .unwrap()
                .as_str(),
            "https://example.org/user/alice/api/contents/a%20b%23%3F.ipynb"
        );
        assert_eq!(
            config.websocket_url("kernel", "session").unwrap().as_str(),
            "wss://example.org/user/alice/api/kernels/kernel/channels?session_id=session"
        );
        assert!(!format!("{config:?}").contains("secret"));
        assert!(config.endpoint(&[".."]).is_err());
        assert!(ServerConfig::new("https://example.org/?token=secret").is_err());
        assert!(ServerConfig::new("https://user:secret@example.org").is_err());
    }
}
