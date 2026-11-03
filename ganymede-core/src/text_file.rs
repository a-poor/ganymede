//! Server-backed UTF-8 text documents, independent of any GUI or file extension.
use crate::jupyter::{self, JupyterRestClient, ServerConfig, models::Contents};
use base64::Engine;
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Jupyter(#[from] jupyter::Error),
    #[error("This file is not supported UTF-8 text")]
    NotText,
    #[error("File changed on the server since it was opened or saved")]
    Conflict,
    #[error("File changed locally while reloading; your edits have been kept")]
    ChangedDuringReload,
    #[error("Text file operation was interrupted")]
    Interrupted,
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct Snapshot {
    pub path: String,
    pub text: String,
    pub revision: u64,
    saved_revision: u64,
}
impl Snapshot {
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct TextRecovery {
    pub path: String,
    pub text: String,
    pub saved_version: String,
    pub crlf: bool,
    pub bom: bool,
}

struct Data {
    snapshot: Snapshot,
    version: String,
    crlf: bool,
    bom: bool,
}
impl Data {
    fn from_contents(contents: Contents) -> Result<Self> {
        let raw = decode_text(&contents)?;
        let bom = raw.starts_with('\u{feff}');
        let text = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
        let crlf = text.contains("\r\n") && !text.replace("\r\n", "").contains('\n');
        let text = if crlf {
            text.replace("\r\n", "\n")
        } else {
            text.to_owned()
        };
        Ok(Self {
            snapshot: Snapshot {
                path: contents.path,
                text,
                revision: 0,
                saved_revision: 0,
            },
            version: contents.last_modified,
            crlf,
            bom,
        })
    }
    fn encoded(&self) -> String {
        let text = if self.crlf {
            self.snapshot.text.replace('\n', "\r\n")
        } else {
            self.snapshot.text.clone()
        };
        if self.bom {
            format!("\u{feff}{text}")
        } else {
            text
        }
    }
}

/// Decode by content, not extension. Reject binary control bytes and invalid UTF-8.
pub fn decode_text(contents: &Contents) -> Result<String> {
    if contents.content_type != "file" {
        return Err(Error::NotText);
    }
    let value = contents.content.as_str().ok_or(Error::NotText)?;
    let text = match contents.format.as_deref() {
        Some("text") => value.to_owned(),
        Some("base64") => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(value.replace(['\r', '\n'], ""))
                .map_err(|_| Error::NotText)?;
            String::from_utf8(bytes).map_err(|_| Error::NotText)?
        }
        _ => return Err(Error::NotText),
    };
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}'))
    {
        return Err(Error::NotText);
    }
    Ok(text)
}

#[derive(Clone)]
pub struct TextFileController {
    rest: JupyterRestClient,
    data: Arc<Mutex<Data>>,
    saves: Arc<tokio::sync::Mutex<()>>,
}
impl TextFileController {
    pub async fn open(config: ServerConfig, path: &str) -> Result<Self> {
        let rest = JupyterRestClient::new(config)?;
        let data = Data::from_contents(rest.get_contents(path, true).await?)?;
        Ok(Self {
            rest,
            data: Arc::new(Mutex::new(data)),
            saves: Default::default(),
        })
    }
    pub fn recovery(&self) -> Option<TextRecovery> {
        let data = self.data.lock().unwrap();
        data.snapshot.is_dirty().then(|| TextRecovery {
            path: data.snapshot.path.clone(),
            text: data.snapshot.text.clone(),
            saved_version: data.version.clone(),
            crlf: data.crlf,
            bom: data.bom,
        })
    }
    pub fn from_recovery(config: ServerConfig, recovery: TextRecovery) -> Result<Self> {
        Ok(Self {
            rest: JupyterRestClient::new(config)?,
            data: Arc::new(Mutex::new(Data {
                snapshot: Snapshot {
                    path: recovery.path,
                    text: recovery.text,
                    revision: 1,
                    saved_revision: 0,
                },
                version: recovery.saved_version,
                crlf: recovery.crlf,
                bom: recovery.bom,
            })),
            saves: Default::default(),
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        self.data.lock().unwrap().snapshot.clone()
    }
    /// Lightweight status for autosave and tab rendering without copying the buffer.
    pub fn status(&self) -> (u64, bool) {
        let data = self.data.lock().unwrap();
        (data.snapshot.revision, data.snapshot.is_dirty())
    }
    pub fn path(&self) -> String {
        self.data.lock().unwrap().snapshot.path.clone()
    }
    pub fn update_text(&self, text: String) {
        let mut data = self.data.lock().unwrap();
        if data.snapshot.text != text {
            data.snapshot.text = text;
            data.snapshot.revision += 1;
        }
    }
    pub async fn save(&self, overwrite: bool) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let _save = this.saves.lock().await;
            let (snapshot, version, text) = {
                let data = this.data.lock().unwrap();
                (data.snapshot.clone(), data.version.clone(), data.encoded())
            };
            if !overwrite {
                match this.rest.get_contents(&snapshot.path, false).await {
                    Ok(current)
                        if current.last_modified == version && current.content_type == "file" => {}
                    Ok(_) => return Err(Error::Conflict),
                    Err(jupyter::Error::Api { status, .. }) if status.as_u16() == 404 => {
                        return Err(Error::Conflict);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let saved = this.rest.save_text_file(&snapshot.path, &text).await?;
            let mut data = this.data.lock().unwrap();
            data.version = saved.last_modified;
            data.snapshot.saved_revision = snapshot.revision;
            Ok(())
        })
        .await
        .map_err(|_| Error::Interrupted)?
    }
    pub async fn save_as(&self, path: String) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let _save = this.saves.lock().await;
            this.rest.ensure_missing(&path).await?;
            let (revision, text) = {
                let data = this.data.lock().unwrap();
                (data.snapshot.revision, data.encoded())
            };
            let saved = this.rest.save_text_file(&path, &text).await?;
            let mut data = this.data.lock().unwrap();
            data.snapshot.path = path;
            data.snapshot.saved_revision = revision;
            data.version = saved.last_modified;
            Ok(())
        })
        .await
        .map_err(|_| Error::Interrupted)?
    }
    /// Call after a successful server rename, with application saves suspended.
    pub fn retarget_path(&self, path: String) {
        self.data.lock().unwrap().snapshot.path = path;
    }
    pub async fn reload(&self) -> Result<()> {
        let this = self.clone();
        tokio::spawn(async move {
            let _save = this.saves.lock().await;
            let before = this.snapshot();
            let mut loaded =
                Data::from_contents(this.rest.get_contents(&before.path, true).await?)?;
            let mut data = this.data.lock().unwrap();
            if data.snapshot.revision != before.revision {
                return Err(Error::ChangedDuringReload);
            }
            // Keep revisions monotonic, including a reload of a previously clean file.
            loaded.snapshot.revision = before.revision + 1;
            loaded.snapshot.saved_revision = loaded.snapshot.revision;
            *data = loaded;
            Ok(())
        })
        .await
        .map_err(|_| Error::Interrupted)?
    }
    pub async fn save_copy(&self) -> Result<String> {
        let this = self.clone();
        tokio::spawn(async move {
            let _save = this.saves.lock().await;
            let (path, text) = {
                let data = this.data.lock().unwrap();
                (data.snapshot.path.clone(), data.encoded())
            };
            let (parent, name) = path.rsplit_once('/').unwrap_or(("", &path));
            let extension = name
                .rsplit_once('.')
                .filter(|(stem, _)| !stem.is_empty())
                .map(|(_, ext)| format!(".{ext}"))
                .unwrap_or_default();
            let copy = this.rest.create_text_file(parent, &extension).await?;
            this.rest.save_text_file(&copy.path, &text).await?;
            Ok(copy.path)
        })
        .await
        .map_err(|_| Error::Interrupted)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contents(format: &str, content: &str) -> Contents {
        serde_json::from_value(
            serde_json::json!({"name":"uv.lock","path":"uv.lock","type":"file",
            "writable":true,"created":"","last_modified":"1","format":format,"content":content}),
        )
        .unwrap()
    }
    #[test]
    fn text_detection_is_independent_of_extension_and_rejects_binary() {
        for text in [
            "",
            "# Python 🪐\nprint(42)\n",
            "version = 1\n",
            "[project]\r\n",
        ] {
            assert_eq!(decode_text(&contents("text", text)).unwrap(), text);
            let encoded = base64::engine::general_purpose::STANDARD.encode(text);
            assert_eq!(decode_text(&contents("base64", &encoded)).unwrap(), text);
        }
        assert!(decode_text(&contents("text", "a\0b")).is_err());
        assert!(decode_text(&contents("base64", "/w==")).is_err());
        assert!(decode_text(&contents("base64", "not base64!")).is_err());
    }
    #[test]
    fn preserves_bom_crlf_and_empty_file_contents() {
        let mut data = Data::from_contents(contents("text", "\u{feff}hello\r\nworld\r\n")).unwrap();
        assert_eq!(data.snapshot.text, "hello\nworld\n");
        data.snapshot.text.push_str("next\n");
        assert_eq!(data.encoded(), "\u{feff}hello\r\nworld\r\nnext\r\n");
        assert_eq!(
            Data::from_contents(contents("text", "")).unwrap().encoded(),
            ""
        );
    }
}
