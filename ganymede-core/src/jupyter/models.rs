use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
pub struct ServerStatus {
    pub started: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerVersion {
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Kernel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub last_activity: Option<String>,
    #[serde(default)]
    pub execution_state: Option<String>,
    #[serde(default)]
    pub connections: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Session {
    pub id: String,
    pub path: String,
    pub name: String,
    #[serde(rename = "type")]
    pub session_type: String,
    pub kernel: Kernel,
}

/// Select an existing kernel or start a new one with a kernelspec name.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum KernelSelection {
    Existing { id: String },
    New { name: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateSession {
    pub path: String,
    pub name: String,
    #[serde(rename = "type")]
    pub session_type: String,
    pub kernel: KernelSelection,
}

impl CreateSession {
    pub fn notebook(path: impl Into<String>, kernel_name: impl Into<String>) -> Self {
        let path = path.into();
        let name = path.rsplit('/').next().unwrap_or("").to_owned();
        Self {
            path,
            name,
            session_type: "notebook".into(),
            kernel: KernelSelection::New {
                name: kernel_name.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct KernelSpecs {
    pub default: String,
    pub kernelspecs: BTreeMap<String, KernelSpecEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KernelSpecEntry {
    pub name: String,
    pub spec: KernelSpec,
    #[serde(default)]
    pub resources: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KernelSpec {
    pub display_name: String,
    pub language: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Contents {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub content_type: String,
    pub writable: bool,
    pub created: String,
    pub last_modified: String,
    pub mimetype: Option<String>,
    pub format: Option<String>,
    pub size: Option<u64>,
    /// Notebook JSON, a directory listing, text, base64, or null.
    pub content: Value,
}
