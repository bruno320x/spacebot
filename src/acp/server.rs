//! ACP (Agent Client Protocol) v1 *server* — Spacebot as a coding agent.
//!
//! This is the mirror of the shipped ACP *client* (`worker.rs`): instead of
//! driving an external coding CLI, Spacebot itself plays the ACP agent role so
//! editors and hosts can drive its sessions over JSON-RPC over stdio.
//!
//! The protocol layer here is intentionally generic over a [`Session`] driver:
//! the wire semantics (`initialize` → `session/new` → `session/prompt` →
//! `session/update` → `permission/request` ↔ response → `session/exit`) are
//! tested deterministically against a mock backend and stay reusable when a
//! real branch-backed session is wired in later.

use super::types::serialize_line;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, Write};

/// What a session driver wants to tell the client from one `run` step.
#[derive(Debug)]
pub enum RunResult {
    /// The prompt finished: emit `updates` (`session/update` notifications),
    /// then reply with `result`.
    Finished { updates: Vec<Value>, result: Value },
    /// The driver needs permission before continuing. Emit `updates`, then the
    /// `permission/request` JSON-RPC request and wait for the client's answer.
    AwaitingPermission {
        updates: Vec<Value>,
        permission_request: Value,
    },
}

/// A single agent session backend: owns whatever state one conversation needs
/// and produces ACP outcomes.
pub trait Session {
    /// Called when the client opens a session.
    fn start(&mut self, session_id: &str);
    /// Run (or resume) a prompt. `resume` is `Some(outcome)` when the previous
    /// step asked for permission and the client just answered.
    fn run(&mut self, prompt: &str, resume: Option<&str>) -> RunResult;
}

/// A pending `permission/request` we are waiting on an answer for.
#[derive(Debug)]
struct Pending {
    session_id: String,
    prompt: String,
    /// JSON-RPC id of the awaiting `session/prompt` request, echoed on reply.
    reply_id: Value,
    /// ACP request id carried by `session/update` notifications.
    request_id: Value,
}

/// Agent identity advertised in `initialize`.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub homepage: String,
    pub license: String,
}

impl AgentInfo {
    /// The default identity Spacebot advertises as an ACP agent.
    pub fn spacebot(version: &str) -> Self {
        Self {
            name: "spacebot".to_string(),
            version: version.to_string(),
            description:
                "An autonomous coding agent that plans, executes and verifies its own work"
                    .to_string(),
            homepage: "https://github.com/coruhoorhan/spacebot".to_string(),
            license: "MIT".to_string(),
        }
    }
}

/// The ACP server core. A transport loop feeds one client line at a time into
/// [`Self::handle_line`]; outbound JSON-RPC lines are returned for writing.
pub struct Server {
    info: AgentInfo,
    new_session: Box<dyn FnMut() -> Box<dyn Session>>,
    sessions: HashMap<String, Box<dyn Session>>,
    pending: HashMap<Value, Pending>,
    next_id: u64,
}

impl Server {
    /// Build a server. `new_session` fabricates the session backend used by
    /// `session/new`; inject the mock in tests and a real backend in the CLI.
    pub fn new(info: AgentInfo, new_session: impl FnMut() -> Box<dyn Session> + 'static) -> Self {
        Self {
            info,
            new_session: Box::new(new_session),
            sessions: HashMap::new(),
            pending: HashMap::new(),
            next_id: 0,
        }
    }

    /// Handle one newline-delimited client line and return the outbound lines
    /// (notifications, `permission/request`s and responses) to write back.
    pub fn handle_line(&mut self, line: &str) -> Result<Vec<Value>, ServerError> {
        let message: Value = serde_json::from_str(line)?;
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str);

        // A response to one of our `permission/request`s carries no `method`.
        if method.is_none() {
            if let Some(key) = id
                && let Some(pending) = self.pending.remove(&key)
            {
                let outcome = message
                    .get("result")
                    .and_then(|r| r.get("outcome"))
                    .and_then(Value::as_str)
                    .unwrap_or("rejected");
                return Ok(self.resume_prompt(pending, outcome));
            }
            // An unmatched response is ignored.
            return Ok(Vec::new());
        }

        let response = match method {
            Some("initialize") => self.initialize(id),
            Some("session/new") => self.session_new(id, &message["params"]),
            Some("session/prompt") => self.session_prompt(id, &message["params"]),
            Some("session/exit") => self.session_exit(id, &message["params"]),
            _ => vec![self.error_response(id, -32601, "method not found")],
        };
        Ok(response)
    }

    /// Drain a reader line-by-line into a writer until EOF. Blocking; used both
    /// by the stdio CLI and by tests.
    pub fn serve_reader<R: BufRead, W: Write>(
        &mut self,
        reader: &mut R,
        writer: &mut W,
    ) -> std::io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            if line.trim().is_empty() {
                continue;
            }
            match self.handle_line(&line) {
                Ok(lines) => {
                    for value in lines {
                        writer.write_all(&serialize_line(&value))?;
                    }
                }
                Err(err) => tracing::warn!(%err, "ACP server dropped a client line"),
            }
            writer.flush()?;
        }
        Ok(())
    }

    fn initialize(&mut self, id: Option<Value>) -> Vec<Value> {
        vec![self.result_response(
            id,
            json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "session": {},
                    "terminal": { "supportsOutput": false },
                    "auth": false,
                },
                "agentInfo": {
                    "name": self.info.name,
                    "version": self.info.version,
                    "description": self.info.description,
                    "homepage": self.info.homepage,
                    "license": self.info.license,
                    "enableThreads": false,
                    "commands": [],
                },
            }),
        )]
    }

    fn session_new(&mut self, id: Option<Value>, params: &Value) -> Vec<Value> {
        let cwd = params.get("cwd").and_then(Value::as_str).unwrap_or("");
        let session_id = format!("session-{}", self.next_id);
        self.next_id += 1;
        let mut session = (self.new_session)();
        session.start(&session_id);
        self.sessions.insert(session_id.clone(), session);
        vec![self.result_response(
            id,
            json!({
                "sessionId": session_id,
                "cwd": cwd,
                "enabledFeatures": [],
            }),
        )]
    }

    fn session_prompt(&mut self, id: Option<Value>, params: &Value) -> Vec<Value> {
        let Some(session_id) = params.get("sessionId").and_then(Value::as_str) else {
            return vec![self.error_response(id, -32602, "missing sessionId")];
        };
        let prompt = params.get("prompt").and_then(Value::as_str).unwrap_or("");
        let Some(session) = self.sessions.get_mut(session_id) else {
            return vec![self.error_response(id, -32602, "unknown sessionId")];
        };
        let request_id = params.get("requestId").cloned().unwrap_or_else(|| {
            let value = Value::String(format!("request-{}", self.next_id));
            self.next_id += 1;
            value
        });
        match session.run(prompt, None) {
            RunResult::Finished { updates, result } => {
                let mut out = updates;
                out.push(self.result_response(id, result));
                out
            }
            RunResult::AwaitingPermission {
                updates,
                permission_request,
            } => {
                let key = permission_request.get("id").cloned().unwrap_or(Value::Null);
                self.pending.insert(
                    key,
                    Pending {
                        session_id: session_id.to_string(),
                        prompt: prompt.to_string(),
                        reply_id: id.unwrap_or(Value::Null),
                        request_id,
                    },
                );
                let mut out = updates;
                out.push(permission_request);
                out
            }
        }
    }

    fn resume_prompt(&mut self, pending: Pending, outcome: &str) -> Vec<Value> {
        let Some(session) = self.sessions.get_mut(&pending.session_id) else {
            return Vec::new();
        };
        match session.run(&pending.prompt, Some(outcome)) {
            RunResult::Finished { updates, result } => {
                let mut out = updates;
                out.push(self.result_response(Some(pending.reply_id), result));
                out
            }
            RunResult::AwaitingPermission {
                updates,
                permission_request,
            } => {
                let key = permission_request.get("id").cloned().unwrap_or(Value::Null);
                self.pending.insert(
                    key,
                    Pending {
                        session_id: pending.session_id,
                        prompt: pending.prompt,
                        reply_id: pending.reply_id,
                        request_id: pending.request_id,
                    },
                );
                let mut out = updates;
                out.push(permission_request);
                out
            }
        }
    }

    fn session_exit(&mut self, id: Option<Value>, params: &Value) -> Vec<Value> {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("");
        self.sessions.remove(session_id);
        vec![self.result_response(id, json!({ "sessionId": session_id }))]
    }

    fn result_response(&self, id: Option<Value>, result: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result })
    }

    fn error_response(&self, id: Option<Value>, code: i64, message: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": { "code": code, "message": message },
        })
    }
}

/// Errors returned from a single `handle_line` step.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// A deterministic in-memory session used to exercise the wire protocol in
/// tests. Echoes prompts back, streams two `session/update` notifications, and
/// asks for permission when the prompt mentions "risky". Test-only: it is not
/// referenced from production code, so it is gated to keep `cargo check` free
/// of `-D warnings` dead-code errors.
#[cfg(test)]
#[derive(Debug, Default)]
struct MockSession {
    session_id: String,
}

#[cfg(test)]
impl Session for MockSession {
    fn start(&mut self, session_id: &str) {
        self.session_id = session_id.to_string();
    }

    fn run(&mut self, prompt: &str, resume: Option<&str>) -> RunResult {
        let session_id = &self.session_id;
        if let Some(outcome) = resume {
            if outcome == "allowed" {
                return RunResult::Finished {
                    updates: vec![update(
                        session_id,
                        "completed",
                        format!("cleared for: {prompt}"),
                    )],
                    result: json!({ "echo": "allowed", "prompt": prompt }),
                };
            }
            return RunResult::Finished {
                updates: vec![update(
                    session_id,
                    "error",
                    format!("permission rejected: {prompt}"),
                )],
                result: json!({ "echo": "rejected", "prompt": prompt }),
            };
        }
        if prompt.contains("risky") {
            return RunResult::AwaitingPermission {
                updates: vec![update(
                    session_id,
                    "running",
                    format!("requesting permission for: {prompt}"),
                )],
                permission_request: json!({
                    "jsonrpc": "2.0",
                    "id": "permission-mock",
                    "method": "permission/request",
                    "params": {
                        "sessionId": session_id,
                        "permissionId": "permission-mock",
                        "permission": { "type": "mock_check", "detail": prompt },
                    },
                }),
            };
        }
        RunResult::Finished {
            updates: vec![
                update(session_id, "running", format!("processing: {prompt}")),
                update(session_id, "completed", format!("done: {prompt}")),
            ],
            result: json!({ "echo": "ok", "prompt": prompt }),
        }
    }
}

#[cfg(test)]
fn update(session_id: &str, status: &str, text: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "status": status,
            "content": { "type": "text", "text": text },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn test_server() -> Server {
        Server::new(AgentInfo::spacebot("0.0.0"), || {
            Box::new(MockSession::default())
        })
    }

    fn initialize_line() -> &'static str {
        "{ \"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"initialize\" }"
    }

    #[test]
    fn initialize_advertises_agent_info() {
        let mut server = test_server();
        let lines = server.handle_line(initialize_line()).expect("handled");
        assert_eq!(lines.len(), 1);
        let result = &lines[0]["result"];
        assert_eq!(result["protocolVersion"], 1);
        assert_eq!(result["agentInfo"]["name"], "spacebot");
        assert_eq!(result["agentInfo"]["version"], "0.0.0");
        assert_eq!(result["agentCapabilities"]["fs"]["readTextFile"], false);
    }

    #[test]
    fn session_new_returns_persisted_id() {
        let mut server = test_server();
        server.handle_line(initialize_line()).expect("handled");
        let lines = server
            .handle_line(
                r#"{ "jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp"} }"#,
            )
            .expect("handled");
        let session_id = lines[0]["result"]["sessionId"].as_str().unwrap_or("");
        assert!(!session_id.is_empty());
        assert!(session_id.starts_with("session-"));
    }

    #[test]
    fn prompt_streams_updates_then_result() {
        let mut server = test_server();
        server.handle_line(initialize_line()).expect("handled");
        let new = server
            .handle_line(
                r#"{ "jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp"} }"#,
            )
            .expect("handled");
        let session_id = new[0]["result"]["sessionId"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let lines = server
            .handle_line(&format!(
                r#"{{ "jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{session_id}","prompt":"hello"}} }}"#
            ))
            .expect("handled");
        // running + completed updates, then the prompt result.
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["method"], "session/update");
        assert_eq!(lines[0]["params"]["status"], "running");
        assert_eq!(lines[1]["method"], "session/update");
        assert_eq!(lines[1]["params"]["status"], "completed");
        assert_eq!(lines[2]["id"], 3);
        assert_eq!(lines[2]["result"]["echo"], "ok");
        assert_eq!(lines[2]["result"]["prompt"], "hello");
    }

    #[test]
    fn risky_prompt_defers_until_permission_grants() {
        let mut server = test_server();
        server.handle_line(initialize_line()).expect("handled");
        let new = server
            .handle_line(
                r#"{ "jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp"} }"#,
            )
            .expect("handled");
        let session_id = new[0]["result"]["sessionId"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let first = server
            .handle_line(&format!(
                r#"{{ "jsonrpc":"2.0","id":3,"method":"session/prompt","params":{{"sessionId":"{session_id}","prompt":"risky action"}} }}"#
            ))
            .expect("handled");
        assert_eq!(first.len(), 2);
        assert_eq!(first[1]["method"], "permission/request");
        let permission_id = first[1]["id"].clone();

        // No result yet — the prompt reply must wait.
        let granted = server
            .handle_line(&format!(
                r#"{{ "jsonrpc":"2.0","id":{permission_id},"result":{{"outcome":"allowed"}} }}"#
            ))
            .expect("handled");
        let reply = granted.last().expect("has result");
        // The prompt reply reuses the original prompt request id.
        assert_eq!(reply["id"], 3);
        assert_eq!(reply["result"]["echo"], "allowed");
    }

    #[test]
    fn unknown_method_returns_error() {
        let mut server = test_server();
        let lines = server
            .handle_line(r#"{ "jsonrpc":"2.0","id":9,"method":"bogus/method" }"#)
            .expect("handled");
        assert_eq!(lines[0]["error"]["code"], -32601);
        assert_eq!(lines[0]["id"], 9);
    }

    #[test]
    fn exit_closes_and_acknowledges() {
        let mut server = test_server();
        server.handle_line(initialize_line()).expect("handled");
        let new = server
            .handle_line(
                r#"{ "jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp"} }"#,
            )
            .expect("handled");
        let session_id = new[0]["result"]["sessionId"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let lines = server
            .handle_line(&format!(
                r#"{{ "jsonrpc":"2.0","id":4,"method":"session/exit","params":{{"sessionId":"{session_id}"}} }}"#
            ))
            .expect("handled");
        assert_eq!(lines[0]["result"]["sessionId"], session_id);
        assert!(server.sessions.is_empty());
    }

    #[test]
    fn serve_reader_round_trips_the_full_flow() {
        let input = format!(
            "{}\n{}\n{}\n",
            initialize_line(),
            r#"{ "jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp"} }"#,
            r#"{ "jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"session-0","prompt":"via reader"} }"#,
        );
        let mut server = test_server();
        let mut reader = Cursor::new(input.into_bytes());
        let mut output = Vec::new();
        server
            .serve_reader(&mut reader, &mut output)
            .expect("served");
        let text = String::from_utf8(output).expect("utf8");
        let lines = text.lines().collect::<Vec<_>>();
        assert!(lines.iter().any(|l| l.contains("\"protocolVersion\":1")));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("\"sessionId\":\"session-0\""))
        );
        assert!(lines.iter().any(|l| l.contains("\"echo\":\"ok\"")));
    }
}
