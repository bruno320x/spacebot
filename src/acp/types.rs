//! ACP (Agent Client Protocol) v1 wire types.
//!
//! ACP is a JSON-RPC 2.0 protocol transported as newline-delimited JSON over
//! the agent subprocess's stdin/stdout. This module defines the small subset
//! of the protocol Spacebot needs as a *client*: `initialize`, `session/new`,
//! `session/prompt`, `session/update` notifications, `permission/request`, and
//! `session/exit`.
//!
//! Reference: <https://agentclientprotocol.com/protocol/v1/overview>

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request ID. ACP agents may echo numeric or string ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(u64),
    String(String),
}

/// A JSON-RPC error object (response side).
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// Any message that can arrive from the agent process.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum IncomingMessage {
    /// A reply to one of our requests.
    Response {
        jsonrpc: String,
        id: RequestId,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<JsonRpcError>,
    },
    /// A one-way notification (e.g. `session/update`).
    Notification {
        jsonrpc: String,
        method: String,
        #[serde(default)]
        params: Option<Value>,
    },
    /// An incoming request from the agent that we must answer
    /// (e.g. `permission/request`).
    Request {
        jsonrpc: String,
        id: RequestId,
        method: String,
        #[serde(default)]
        params: Option<Value>,
    },
}

/// Status values carried by `session/update` notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    WaitingForInput,
    Completed,
    Cancelled,
    Error,
}

impl SessionStatus {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "running" => Some(Self::Running),
            "waiting_for_input" => Some(Self::WaitingForInput),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// A streamed content block inside a `session/update` notification.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    Text { text: String },
    Other(Value),
}

impl ContentBlock {
    /// Parse a content block from its JSON representation. Text blocks carry
    /// the assistant's incremental output; everything else is opaque.
    pub fn from_value(value: &Value) -> Option<Self> {
        let kind = value.get("type")?.as_str()?;
        match kind {
            "text" => Some(Self::Text {
                text: value.get("text")?.as_str()?.to_string(),
            }),
            _ => Some(Self::Other(value.clone())),
        }
    }

    /// Extract displayable text, if any.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Other(_) => None,
        }
    }
}

/// Parsed `session/update` notification parameters.
#[derive(Debug, Clone)]
pub struct SessionUpdate {
    pub session_id: String,
    pub status: Option<SessionStatus>,
    pub content: Option<ContentBlock>,
}

impl SessionUpdate {
    pub fn parse(params: &Value) -> Option<Self> {
        let session_id = params.get("sessionId")?.as_str()?.to_string();
        let status = params.get("status").and_then(|v| v.as_str()).and_then(SessionStatus::parse);
        let content = params
            .get("content")
            .and_then(ContentBlock::from_value);
        Some(Self {
            session_id,
            status,
            content,
        })
    }
}

/// Parsed `permission/request` parameters.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub session_id: String,
    pub permission_id: String,
    pub permission: Value,
}

impl PermissionRequest {
    pub fn parse(params: &Value) -> Option<Self> {
        Some(Self {
            session_id: params.get("sessionId")?.as_str()?.to_string(),
            permission_id: params.get("permissionId")?.as_str()?.to_string(),
            permission: params.get("permission")?.clone(),
        })
    }
}

/// Outcome we send in response to a `permission/request`.
#[derive(Debug, Clone, Serialize)]
pub struct PermissionResponse {
    pub outcome: String,
}

/// Build an `initialize` request. We advertise a minimal client: no fs,
/// terminal, or auth delegation — the agent runs with its own tools.
pub fn build_initialize_request(id: u64, client_name: &str, client_version: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": { "readTextFile": false, "writeTextFile": false },
                "session": {},
                "terminal": { "supportsOutput": false },
                "auth": {}
            },
            "clientInfo": {
                "name": client_name,
                "version": client_version
            }
        }
    })
}

/// Build a `session/new` request.
pub fn build_session_new_request(id: u64, cwd: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/new",
        "params": {
            "cwd": cwd,
            "mcpServers": []
        }
    })
}

/// Build a `session/prompt` request. ACP v1 accepts a plain string prompt;
/// the system prompt is prepended by the worker before this call.
pub fn build_session_prompt_request(id: u64, session_id: &str, prompt: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": prompt,
        }
    })
}

/// Build a JSON-RPC response to a `permission/request`.
///
/// ACP permission requests are JSON-RPC requests from the agent; the client
/// answers with a result keyed by the request id.
pub fn build_permission_response(id: &RequestId, outcome: &str) -> Value {
    let id_value = match id {
        RequestId::Number(number) => Value::Number((*number).into()),
        RequestId::String(string) => Value::String(string.clone()),
    };
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id_value,
        "result": {
            "outcome": outcome,
        }
    })
}

/// Build a `session/exit` request.
pub fn build_session_exit_request(id: u64, session_id: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/exit",
        "params": {
            "sessionId": session_id,
        }
    })
}

/// Build a JSON-RPC error response for an unsupported incoming request.
pub fn build_error_response(id: &RequestId, code: i64, message: &str) -> Value {
    let id_value = match id {
        RequestId::Number(number) => Value::Number((*number).into()),
        RequestId::String(string) => Value::String(string.clone()),
    };
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id_value,
        "error": {
            "code": code,
            "message": message,
        }
    })
}

/// Serialize a request value as a single newline-delimited JSON line.
pub fn serialize_line(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).expect("ACP request serialization is infallible");
    bytes.push(b'\n');
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_status_parses_known_and_rejects_unknown() {
        assert_eq!(SessionStatus::parse("running"), Some(SessionStatus::Running));
        assert_eq!(SessionStatus::parse("waiting_for_input"), Some(SessionStatus::WaitingForInput));
        assert_eq!(SessionStatus::parse("completed"), Some(SessionStatus::Completed));
        assert_eq!(SessionStatus::parse("cancelled"), Some(SessionStatus::Cancelled));
        assert_eq!(SessionStatus::parse("error"), Some(SessionStatus::Error));
        assert_eq!(SessionStatus::parse("bogus"), None);
        assert_eq!(SessionStatus::parse(""), None);
    }

    #[test]
    fn content_block_parses_text_and_keeps_other_opaque() {
        let text = serde_json::json!({ "type": "text", "text": "hello" });
        let block = ContentBlock::from_value(&text).expect("text block");
        assert_eq!(block.as_text(), Some("hello"));

        let tool = serde_json::json!({ "type": "tool_call", "callId": "x" });
        let other = ContentBlock::from_value(&tool).expect("other block");
        assert_eq!(other.as_text(), None);
        assert!(matches!(other, ContentBlock::Other(_)));

        let malformed = serde_json::json!({ "nope": true });
        assert!(ContentBlock::from_value(&malformed).is_none());
    }

    #[test]
    fn session_update_parses_status_and_content() {
        let params = serde_json::json!({
            "sessionId": "s1",
            "status": "running",
            "content": { "type": "text", "text": "working" }
        });
        let update = SessionUpdate::parse(&params).expect("parse");
        assert_eq!(update.session_id, "s1");
        assert_eq!(update.status, Some(SessionStatus::Running));
        assert_eq!(update.content.as_ref().and_then(ContentBlock::as_text), Some("working"));
    }

    #[test]
    fn session_update_allows_missing_status_and_content() {
        let update = SessionUpdate::parse(&serde_json::json!({ "sessionId": "s1" })).expect("parse");
        assert_eq!(update.session_id, "s1");
        assert_eq!(update.status, None);
        assert!(update.content.is_none());
    }

    #[test]
    fn session_update_requires_session_id() {
        assert!(SessionUpdate::parse(&serde_json::json!({})).is_none());
    }

    #[test]
    fn permission_request_parses_fields() {
        let params = serde_json::json!({
            "sessionId": "s1",
            "permissionId": "p1",
            "permission": { "type": "write_file", "path": "/tmp/x" }
        });
        let request = PermissionRequest::parse(&params).expect("parse");
        assert_eq!(request.session_id, "s1");
        assert_eq!(request.permission_id, "p1");
        assert_eq!(request.permission["type"], "write_file");
    }

    #[test]
    fn initialize_request_has_minimal_capabilities() {
        let value = build_initialize_request(7, "test-client", "1.0.0");
        assert_eq!(value["method"], "initialize");
        assert_eq!(value["id"], 7);
        assert_eq!(value["params"]["protocolVersion"], 1);
        assert_eq!(value["params"]["clientInfo"]["name"], "test-client");
        // fs + terminal delegation are advertised as disabled.
        assert_eq!(value["params"]["clientCapabilities"]["fs"]["readTextFile"], false);
        assert_eq!(
            value["params"]["clientCapabilities"]["terminal"]["supportsOutput"],
            false
        );
    }

    #[test]
    fn session_prompt_request_shapes() {
        let request = build_session_prompt_request(3, "session-1", "do the thing");
        assert_eq!(request["method"], "session/prompt");
        assert_eq!(request["params"]["sessionId"], "session-1");
        assert_eq!(request["params"]["prompt"], "do the thing");

        let newsession = build_session_new_request(2, "/tmp/work");
        assert_eq!(newsession["method"], "session/new");
        assert_eq!(newsession["params"]["cwd"], "/tmp/work");

        let exit = build_session_exit_request(4, "session-1");
        assert_eq!(exit["method"], "session/exit");
        assert_eq!(exit["params"]["sessionId"], "session-1");
    }

    #[test]
    fn permission_and_error_responses_preserve_numeric_and_string_ids() {
        let response = build_permission_response(&RequestId::Number(42), "allowed");
        assert_eq!(response["id"], 42);
        assert_eq!(response["result"]["outcome"], "allowed");

        let response = build_permission_response(&RequestId::String(String::from("abc")), "rejected");
        assert_eq!(response["id"], "abc");

        let error = build_error_response(&RequestId::Number(9), -32601, "nope");
        assert_eq!(error["id"], 9);
        assert_eq!(error["error"]["code"], -32601);
        assert_eq!(error["error"]["message"], "nope");
    }

    #[test]
    fn incoming_message_deserializes_all_three_shapes() {
        let response: IncomingMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).expect("response");
        assert!(matches!(response, IncomingMessage::Response { id: RequestId::Number(1), .. }));

        let notification: IncomingMessage =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"session/update"}"#)
                .expect("notification");
        assert!(matches!(notification, IncomingMessage::Notification { .. }));

        let request: IncomingMessage = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":"req-1","method":"permission/request"}"#,
        )
        .expect("request");
        assert!(matches!(
            request,
            IncomingMessage::Request { id: RequestId::String(_), .. }
        ));
    }

    #[test]
    fn serialize_line_appends_newline_and_round_trips() {
        let value = build_initialize_request(0, "spacebot", "0.0.0");
        let bytes = serialize_line(&value);
        assert_eq!(bytes.last(), Some(&b'\n'));
        let parsed: Value = serde_json::from_slice(&bytes[..bytes.len() - 1]).expect("json");
        assert_eq!(parsed, value);
    }
}
