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
