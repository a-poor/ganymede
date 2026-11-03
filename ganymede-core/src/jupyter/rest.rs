use reqwest::{Method, Url};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{Error, Result, ServerConfig, models::*};

#[derive(Debug, Clone)]
pub struct JupyterRestClient {
    config: ServerConfig,
    http: reqwest::Client,
}

impl JupyterRestClient {
    pub fn new(config: ServerConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .default_headers(config.headers())
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            // A login redirect should surface as an error, not an HTML success.
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { config, http })
    }

    async fn request(
        &self,
        method: Method,
        url: Url,
        body: Option<Value>,
    ) -> Result<reqwest::Response> {
        let mut request = self.http.request(method, url);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            let message = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("message")
                        .or_else(|| value.get("reason"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| body.chars().take(4096).collect());
            return Err(Error::Api { status, message });
        }
        Ok(response)
    }

    async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<Value>,
    ) -> Result<T> {
        Ok(self
            .request(method, self.config.endpoint(segments)?, body)
            .await?
            .json()
            .await?)
    }

    /// Explicitly stop the server and its kernels. Closing a client must not call this.
    pub async fn shutdown_server(&self) -> Result<()> {
        self.request(
            Method::POST,
            self.config.endpoint(&["api", "shutdown"])?,
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn server_version(&self) -> Result<ServerVersion> {
        self.json(Method::GET, &["api"], None).await
    }
    pub async fn list_kernel_specs(&self) -> Result<KernelSpecs> {
        self.json(Method::GET, &["api", "kernelspecs"], None).await
    }
    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        self.json(Method::GET, &["api", "sessions"], None).await
    }
    pub async fn get_session(&self, id: &str) -> Result<Session> {
        self.json(Method::GET, &["api", "sessions", id], None).await
    }
    /// Replace a session's kernel. The server shuts down the previous kernel.
    pub async fn change_session_kernel(&self, id: &str, name: &str) -> Result<Session> {
        self.json(
            Method::PATCH,
            &["api", "sessions", id],
            Some(json!({"kernel": {"name": name}})),
        )
        .await
    }
    pub async fn create_session(&self, request: &CreateSession) -> Result<Session> {
        self.json(
            Method::POST,
            &["api", "sessions"],
            Some(serde_json::to_value(request)?),
        )
        .await
    }
    /// Deletes the session and its kernel. Closing a WebSocket does neither.
    pub async fn delete_session(&self, id: &str) -> Result<()> {
        self.request(
            Method::DELETE,
            self.config.endpoint(&["api", "sessions", id])?,
            None,
        )
        .await?;
        Ok(())
    }
    pub async fn list_kernels(&self) -> Result<Vec<Kernel>> {
        self.json(Method::GET, &["api", "kernels"], None).await
    }
    pub async fn get_kernel(&self, id: &str) -> Result<Kernel> {
        self.json(Method::GET, &["api", "kernels", id], None).await
    }
    pub async fn start_kernel(&self, name: &str, path: Option<&str>) -> Result<Kernel> {
        let mut body = json!({"name": name});
        if let Some(path) = path {
            body["path"] = path.into();
        }
        self.json(Method::POST, &["api", "kernels"], Some(body))
            .await
    }
    pub async fn interrupt_kernel(&self, id: &str) -> Result<()> {
        self.request(
            Method::POST,
            self.config.endpoint(&["api", "kernels", id, "interrupt"])?,
            None,
        )
        .await?;
        Ok(())
    }
    pub async fn restart_kernel(&self, id: &str) -> Result<Kernel> {
        self.json(Method::POST, &["api", "kernels", id, "restart"], None)
            .await
    }
    pub async fn shutdown_kernel(&self, id: &str) -> Result<()> {
        self.request(
            Method::DELETE,
            self.config.endpoint(&["api", "kernels", id])?,
            None,
        )
        .await?;
        Ok(())
    }
    fn contents_url(&self, path: &str) -> Result<Url> {
        let mut segments = vec!["api", "contents"];
        segments.extend(
            path.trim_matches('/')
                .split('/')
                .filter(|part| !part.is_empty()),
        );
        self.config.endpoint(&segments)
    }
    pub async fn get_contents(&self, path: &str, include_content: bool) -> Result<Contents> {
        let mut url = self.contents_url(path)?;
        url.query_pairs_mut()
            .append_pair("content", if include_content { "1" } else { "0" });
        Ok(self.request(Method::GET, url, None).await?.json().await?)
    }
    /// Read a file through Contents, with a bounded base64 response.
    pub async fn read_file_bytes(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let mut url = self.contents_url(path)?;
        url.query_pairs_mut()
            .append_pair("content", "1")
            .append_pair("format", "base64");
        let mut response = self.request(Method::GET, url, None).await?;
        let limit = max_bytes.saturating_mul(4) / 3 + 64 * 1024;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len().saturating_add(chunk.len()) > limit {
                return Err(Error::Protocol("file exceeds download limit".into()));
            }
            body.extend_from_slice(&chunk);
        }
        let model: Contents = serde_json::from_slice(&body)?;
        if model.content_type != "file" || model.format.as_deref() != Some("base64") {
            return Err(Error::Protocol("expected a base64 file".into()));
        }
        let encoded = model
            .content
            .as_str()
            .ok_or_else(|| Error::Protocol("missing file data".into()))?;
        let encoded: String = encoded
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| Error::Protocol("invalid base64 file".into()))?;
        if bytes.len() > max_bytes {
            return Err(Error::Protocol("file exceeds download limit".into()));
        }
        Ok(bytes)
    }

    /// Let the server allocate an unused notebook name in this directory.
    pub async fn create_notebook(&self, directory: &str) -> Result<Contents> {
        Ok(self
            .request(
                Method::POST,
                self.contents_url(directory)?,
                Some(json!({"type": "notebook"})),
            )
            .await?
            .json()
            .await?)
    }
    /// Create a directory with a server-assigned unused name.
    pub async fn create_directory(&self, parent: &str) -> Result<Contents> {
        Ok(self
            .request(
                Method::POST,
                self.contents_url(parent)?,
                Some(json!({"type": "directory"})),
            )
            .await?
            .json()
            .await?)
    }

    /// Upload a file, refusing an existing destination. The Contents API does not
    /// offer an atomic create-only PUT; this check prevents ordinary accidental replacement.
    pub async fn upload_file(&self, path: &str, bytes: &[u8]) -> Result<Contents> {
        match self.get_contents(path, false).await {
            Ok(_) => return Err(Error::Api {
                status: reqwest::StatusCode::CONFLICT,
                message:
                    "A file with this name already exists. Rename the local file before uploading."
                        .into(),
            }),
            Err(Error::Api { status, .. }) if status == reqwest::StatusCode::NOT_FOUND => (),
            Err(error) => return Err(error),
        }
        if path.ends_with(".ipynb") {
            return self
                .save_notebook(path, &serde_json::from_slice(bytes)?)
                .await;
        }
        use base64::Engine;
        let content = base64::engine::general_purpose::STANDARD.encode(bytes);
        Ok(self
            .request(
                Method::PUT,
                self.contents_url(path)?,
                Some(json!({"type":"file", "format":"base64", "content":content})),
            )
            .await?
            .json()
            .await?)
    }

    /// Allocate an unused name using the server's normal new-file behavior.
    pub async fn create_text_file(&self, directory: &str, extension: &str) -> Result<Contents> {
        Ok(self
            .request(
                Method::POST,
                self.contents_url(directory)?,
                Some(json!({"type":"file", "ext":extension})),
            )
            .await?
            .json()
            .await?)
    }

    pub async fn save_text_file(&self, path: &str, text: &str) -> Result<Contents> {
        Ok(self
            .request(
                Method::PUT,
                self.contents_url(path)?,
                Some(json!({"type":"file", "format":"text", "content":text})),
            )
            .await?
            .json()
            .await?)
    }

    /// The standard Contents API has no atomic conditional PUT. Check before
    /// creating a new destination; never deliberately replace an existing file.
    pub async fn ensure_missing(&self, path: &str) -> Result<()> {
        crate::files::validate_path(path)?;
        match self.get_contents(path, false).await {
            Ok(_) => Err(Error::Api {
                status: reqwest::StatusCode::CONFLICT,
                message: "A file or folder with this name already exists".into(),
            }),
            Err(Error::Api { status, .. }) if status == reqwest::StatusCode::NOT_FOUND => Ok(()),
            Err(error) => Err(error),
        }
    }
    pub async fn rename_contents(&self, from: &str, to: &str) -> Result<Contents> {
        crate::files::validate_path(from)?;
        crate::files::validate_path(to)?;
        if crate::files::is_within(to, from) {
            return Err(Error::InvalidPath(
                "Choose a different location outside the source folder".into(),
            ));
        }
        self.ensure_missing(to).await?;
        Ok(self
            .request(
                Method::PATCH,
                self.contents_url(from)?,
                Some(json!({"path":to})),
            )
            .await?
            .json()
            .await?)
    }
    pub async fn create_named_directory(&self, path: &str) -> Result<Contents> {
        self.ensure_missing(path).await?;
        Ok(self
            .request(
                Method::PUT,
                self.contents_url(path)?,
                Some(json!({"type":"directory"})),
            )
            .await?
            .json()
            .await?)
    }
    pub async fn copy_contents(&self, from: &str, to: &str) -> Result<Contents> {
        self.ensure_missing(to).await?;
        let contents = self.get_contents(from, true).await?;
        if !matches!(contents.content_type.as_str(), "file" | "notebook") {
            return Err(Error::InvalidPath(
                "Save As is only available for files".into(),
            ));
        }
        Ok(self.request(Method::PUT, self.contents_url(to)?,
            Some(json!({"type":contents.content_type,"format":contents.format,"content":contents.content}))).await?.json().await?)
    }
    pub async fn update_session_path(&self, id: &str, path: &str) -> Result<Session> {
        Ok(self
            .request(
                Method::PATCH,
                self.config.endpoint(&["api", "sessions", id])?,
                Some(json!({"path":path,"name":crate::files::name(path)})),
            )
            .await?
            .json()
            .await?)
    }

    /// Explicitly remove a file through the Contents API.
    pub async fn delete_contents(&self, path: &str) -> Result<()> {
        self.request(Method::DELETE, self.contents_url(path)?, None)
            .await?;
        Ok(())
    }
    /// Preserve the entire notebook document, including unknown metadata.
    pub async fn save_notebook(&self, path: &str, notebook: &Value) -> Result<Contents> {
        Ok(self
            .request(
                Method::PUT,
                self.contents_url(path)?,
                Some(json!({"type": "notebook", "format": "json", "content": notebook})),
            )
            .await?
            .json()
            .await?)
    }
}
