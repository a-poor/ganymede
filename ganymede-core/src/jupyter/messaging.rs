use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub msg_id: String,
    pub session: String,
    pub username: String,
    pub date: String,
    pub msg_type: String,
    pub version: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelMessage {
    pub channel: String,
    pub header: Header,
    /// Empty object for unsolicited messages; otherwise the originating header.
    pub parent_header: Value,
    pub metadata: Value,
    pub content: Value,
    #[serde(skip)]
    pub buffers: Vec<Vec<u8>>,
}

impl KernelMessage {
    pub fn request(session_id: &str, channel: &str, msg_type: &str, content: Value) -> Self {
        Self {
            channel: channel.into(),
            header: Header {
                msg_id: uuid::Uuid::new_v4().to_string(),
                session: session_id.into(),
                username: "ganymede".into(),
                date: chrono::Utc::now().to_rfc3339(),
                msg_type: msg_type.into(),
                version: "5.3".into(),
                extra: BTreeMap::new(),
            },
            parent_header: json!({}),
            metadata: json!({}),
            content,
            buffers: Vec::new(),
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        self.parent_header.get("msg_id").and_then(Value::as_str)
    }

    pub fn body(&self) -> Result<MessageBody> {
        let value = self.content.clone();
        Ok(match self.header.msg_type.as_str() {
            "status" => MessageBody::Status(serde_json::from_value(value)?),
            "stream" => MessageBody::Stream(serde_json::from_value(value)?),
            "execute_reply" => MessageBody::ExecuteReply(serde_json::from_value(value)?),
            "execute_input" => MessageBody::ExecuteInput(serde_json::from_value(value)?),
            "execute_result" => MessageBody::ExecuteResult(serde_json::from_value(value)?),
            "display_data" => MessageBody::DisplayData(serde_json::from_value(value)?),
            "update_display_data" => MessageBody::UpdateDisplayData(serde_json::from_value(value)?),
            "clear_output" => MessageBody::ClearOutput(serde_json::from_value(value)?),
            "error" => MessageBody::Error(serde_json::from_value(value)?),
            "input_request" => MessageBody::InputRequest(serde_json::from_value(value)?),
            _ => MessageBody::Unknown {
                msg_type: self.header.msg_type.clone(),
                content: value,
            },
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecuteRequest {
    pub code: String,
    pub silent: bool,
    pub store_history: bool,
    pub user_expressions: BTreeMap<String, String>,
    pub allow_stdin: bool,
    pub stop_on_error: bool,
}

impl ExecuteRequest {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            silent: false,
            store_history: true,
            user_expressions: BTreeMap::new(),
            allow_stdin: false,
            stop_on_error: true,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MessageBody {
    Status(Status),
    Stream(StreamOutput),
    ExecuteReply(ExecuteReply),
    ExecuteInput(ExecuteInput),
    ExecuteResult(DisplayData),
    DisplayData(DisplayData),
    UpdateDisplayData(DisplayData),
    ClearOutput(ClearOutput),
    Error(ExecutionError),
    InputRequest(InputRequest),
    Unknown { msg_type: String, content: Value },
}
#[derive(Debug, Clone, Deserialize)]
pub struct Status {
    pub execution_state: String,
}
#[derive(Debug, Clone, Deserialize)]
pub struct StreamOutput {
    pub name: String,
    pub text: String,
}
#[derive(Debug, Clone, Deserialize)]
pub struct ExecuteReply {
    pub status: String,
    pub execution_count: Option<u64>,
    #[serde(flatten)]
    pub details: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct ExecuteInput {
    pub code: String,
    pub execution_count: u64,
}
#[derive(Debug, Clone, Deserialize)]
pub struct DisplayData {
    pub data: BTreeMap<String, Value>,
    pub metadata: Value,
    #[serde(default)]
    pub transient: Value,
    pub execution_count: Option<u64>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct ClearOutput {
    pub wait: bool,
}
#[derive(Debug, Clone, Deserialize)]
pub struct ExecutionError {
    pub ename: String,
    pub evalue: String,
    pub traceback: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct InputRequest {
    pub prompt: String,
    pub password: bool,
}

/// Tracks one execution without discarding output that arrives after completion.
#[derive(Debug, Clone)]
pub struct ExecutionTracker {
    request_id: String,
    reply: bool,
    idle: bool,
}
impl ExecutionTracker {
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            reply: false,
            idle: false,
        }
    }
    pub fn observe(&mut self, message: &KernelMessage) {
        if message.parent_id() != Some(self.request_id.as_str()) {
            return;
        }
        if message.channel == "shell" && message.header.msg_type == "execute_reply" {
            self.reply = true;
        }
        if message.channel == "iopub"
            && message.header.msg_type == "status"
            && message
                .content
                .get("execution_state")
                .and_then(Value::as_str)
                == Some("idle")
        {
            self.idle = true;
        }
    }
    pub fn is_complete(&self) -> bool {
        self.reply && self.idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_requires_matching_reply_and_idle_in_either_order() {
        for idle_first in [false, true] {
            let mut tracker = ExecutionTracker::new("request");
            let mut idle = KernelMessage::request(
                "kernel",
                "iopub",
                "status",
                json!({"execution_state":"idle"}),
            );
            idle.parent_header = json!({"msg_id":"other"});
            tracker.observe(&idle);
            assert!(!tracker.is_complete());
            idle.parent_header = json!({"msg_id":"request"});
            let mut reply =
                KernelMessage::request("kernel", "shell", "execute_reply", json!({"status":"ok"}));
            reply.parent_header = idle.parent_header.clone();
            let messages = if idle_first {
                [idle, reply]
            } else {
                [reply, idle]
            };
            tracker.observe(&messages[0]);
            assert!(!tracker.is_complete());
            tracker.observe(&messages[1]);
            assert!(tracker.is_complete());
        }
    }
}
