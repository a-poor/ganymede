//! Server-assisted conversion of an in-memory notebook. No save or execution requests.
use super::{Error, JupyterRestClient, Result};
use reqwest::Method;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Html,
    Markdown,
    Pdf,
}
impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Markdown => "md",
            Self::Pdf => "pdf",
        }
    }
    fn exporter(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Markdown => "markdown",
            Self::Pdf => "webpdf",
        }
    }
}

pub struct NotebookExport {
    pub bytes: Vec<u8>,
    /// Markdown with extracted images is returned as a ZIP by Jupyter.
    pub extension: &'static str,
}

impl JupyterRestClient {
    pub async fn export_notebook(
        &self,
        name: &str,
        notebook: Value,
        format: ExportFormat,
    ) -> Result<NotebookExport> {
        let mut response = self
            .request(
                Method::POST,
                self.config.endpoint(&["nbconvert", format.exporter()])?,
                Some(json!({"name": name, "content": notebook})),
            )
            .await?;
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let zipped = mime == "application/zip";
        let valid = match format {
            ExportFormat::Html => mime == "text/html",
            ExportFormat::Markdown => {
                matches!(mime, "text/markdown" | "text/plain" | "application/zip")
            }
            // WebPDFExporter inherits HTMLExporter's MIME type in nbconvert.
            // Require the PDF magic below, so an HTML login page is still rejected.
            ExportFormat::Pdf => matches!(mime, "application/pdf" | "text/html"),
        };
        if !valid {
            return Err(Error::Protocol(
                "Unexpected export response from the server".into(),
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len().saturating_add(chunk.len()) > 128 * 1024 * 1024 {
                return Err(Error::Protocol(
                    "Export exceeds the 128 MB download limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty()
            || (format == ExportFormat::Pdf && !bytes.starts_with(b"%PDF-"))
            || (zipped && !bytes.starts_with(b"PK\x03\x04"))
        {
            return Err(Error::Protocol(
                "The server returned an invalid export".into(),
            ));
        }
        Ok(NotebookExport {
            bytes,
            extension: if zipped { "zip" } else { format.extension() },
        })
    }
}
