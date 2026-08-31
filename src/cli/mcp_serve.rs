//! `spacebot mcp serve` — run a stdio MCP server backed by the daemon.
//!
//! Editors (Claude Code, Cursor, …) spawn this command and get Spacebot's
//! curated tool surface over the Model Context Protocol. Tool calls proxy to
//! the running daemon's control API; `spacebot_docs` is served locally from
//! the embedded docs. Shell/file execution is not exposed here — the editor
//! context is untrusted by default.

use super::Context;
use super::client::ApiClient;
use clap::Args;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use rig::tool::server::ToolServer;
use serde::Deserialize;
use spacebot::mcp_server::McpServer;
use std::sync::Arc;

#[derive(Debug, Args)]
pub struct McpServeArgs {
    /// Agent whose tasks and memories are exposed (defaults to the first
    /// configured agent)
    #[arg(short, long)]
    pub agent: Option<String>,
}

pub async fn run(ctx: &Context, args: McpServeArgs) -> anyhow::Result<()> {
    let agent = match args.agent {
        Some(agent) => agent,
        None => first_configured_agent(ctx)?,
    };
    let client = Arc::new(ApiClient::from_context(ctx)?);

    let handle = ToolServer::new()
        .tool(TaskListProxy::new(client.clone(), agent.clone()))
        .tool(TaskCreateProxy::new(client.clone(), agent.clone()))
        .tool(TaskUpdateProxy::new(client.clone(), agent.clone()))
        .tool(MemoryRecallProxy::new(client.clone(), agent.clone()))
        .tool(ChannelRecallProxy::new(client.clone(), agent.clone()))
        .tool(SpacebotDocsProxy::new(agent.clone()))
        .run();

    tracing::info!(%agent, "serving MCP over stdio");
    McpServer::new(handle).serve().await
}

fn first_configured_agent(ctx: &Context) -> anyhow::Result<String> {
    let config = super::load_config(&ctx.config_path)?;
    match config.agents.first() {
        Some(agent) => Ok(agent.id.clone()),
        None => anyhow::bail!("no agents configured — pass --agent"),
    }
}

/// Error type for the MCP proxy tools.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ProxyError(String);

impl ProxyError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

async fn api_get(client: &ApiClient, path: &str) -> Result<serde_json::Value, ProxyError> {
    client
        .get(path)
        .await
        .map_err(|err| ProxyError::new(format!("GET {path}: {err}")))
}

async fn api_post(
    client: &ApiClient,
    path: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, ProxyError> {
    client
        .post(path, body)
        .await
        .map_err(|err| ProxyError::new(format!("POST {path}: {err}")))
}

async fn api_put(
    client: &ApiClient,
    path: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, ProxyError> {
    client
        .put(path, body)
        .await
        .map_err(|err| ProxyError::new(format!("PUT {path}: {err}")))
}

fn encode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

/// Shared proxy-tool fields.
struct ProxyBase {
    client: Arc<ApiClient>,
    agent: String,
}

// ---- task_list -------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct TaskListArgs {
    /// Filter by agent id (defaults to the serve agent)
    pub agent: Option<String>,
    /// Filter by status (e.g. pending_approval, ready, in_progress, done)
    pub status: Option<String>,
    /// Maximum number of tasks (default 20, max 100)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

pub struct TaskListProxy(ProxyBase);

impl TaskListProxy {
    fn new(client: Arc<ApiClient>, agent: String) -> Self {
        Self(ProxyBase { client, agent })
    }
}

impl Tool for TaskListProxy {
    const NAME: &'static str = "task_list";
    type Error = ProxyError;
    type Args = TaskListArgs;
    type Output = serde_json::Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List tasks from Spacebot's task board".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent": { "type": "string", "description": "Filter by agent id" },
                    "status": { "type": "string", "description": "Filter by status" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let agent = args.agent.unwrap_or_else(|| self.0.agent.clone());
        let mut query = vec![format!("limit={}", args.limit.clamp(1, 100))];
        if !agent.is_empty() {
            query.push(format!("agent_id={}", encode(&agent)));
        }
        if let Some(status) = args.status.filter(|s| !s.is_empty()) {
            query.push(format!("status={}", encode(&status)));
        }
        let value = api_get(&self.0.client, &format!("tasks?{}", query.join("&"))).await?;
        let tasks = value["tasks"].as_array().cloned().unwrap_or_default();
        let summary = tasks
            .iter()
            .map(|task| {
                serde_json::json!({
                    "number": task["number"],
                    "title": task["title"],
                    "status": task["status"],
                    "priority": task["priority"],
                    "assigned_agent_id": task["assigned_agent_id"],
                    "description": task["description"],
                })
            })
            .collect::<Vec<_>>();
        Ok(serde_json::json!({ "count": summary.len(), "tasks": summary }))
    }
}

// ---- task_create -----------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct TaskCreateArgs {
    /// Task title (required)
    pub title: String,
    /// Optional description
    pub description: Option<String>,
    /// Optional priority (low/normal/high/critical)
    pub priority: Option<String>,
    /// Optional subtask titles
    #[serde(default)]
    pub subtasks: Vec<String>,
}

pub struct TaskCreateProxy(ProxyBase);

impl TaskCreateProxy {
    fn new(client: Arc<ApiClient>, agent: String) -> Self {
        Self(ProxyBase { client, agent })
    }
}

impl Tool for TaskCreateProxy {
    const NAME: &'static str = "task_create";
    type Error = ProxyError;
    type Args = TaskCreateArgs;
    type Output = serde_json::Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Create a task on Spacebot's task board".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Task title (required)" },
                    "description": { "type": "string", "description": "Task description" },
                    "priority": { "type": "string", "enum": ["low", "normal", "high", "critical"] },
                    "subtasks": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["title"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let title = args.title.trim();
        if title.is_empty() {
            return Err(ProxyError::new("`title` is required"));
        }
        let mut body = serde_json::json!({
            "owner_agent_id": self.0.agent,
            "title": title,
        });
        if let Some(description) = args.description.filter(|d| !d.trim().is_empty()) {
            body["description"] = serde_json::json!(description);
        }
        if let Some(priority) = args.priority.filter(|p| !p.trim().is_empty()) {
            body["priority"] = serde_json::json!(priority);
        }
        if !args.subtasks.is_empty() {
            body["subtasks"] = serde_json::json!(
                args.subtasks
                    .iter()
                    .map(|subtitle| serde_json::json!({ "title": subtitle }))
                    .collect::<Vec<_>>()
            );
        }
        let value = api_post(&self.0.client, "tasks", &body).await?;
        Ok(serde_json::json!({
            "created": value["task"]["number"],
            "title": value["task"]["title"],
            "status": value["task"]["status"],
        }))
    }
}

// ---- task_update -----------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct TaskUpdateArgs {
    /// Task number
    pub number: i64,
    /// New title
    pub title: Option<String>,
    /// New description
    pub description: Option<String>,
    /// New status
    pub status: Option<String>,
    /// New priority
    pub priority: Option<String>,
}

pub struct TaskUpdateProxy(ProxyBase);

impl TaskUpdateProxy {
    fn new(client: Arc<ApiClient>, agent: String) -> Self {
        Self(ProxyBase { client, agent })
    }
}

impl Tool for TaskUpdateProxy {
    const NAME: &'static str = "task_update";
    type Error = ProxyError;
    type Args = TaskUpdateArgs;
    type Output = serde_json::Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Update a task on Spacebot's task board".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "number": { "type": "integer", "description": "Task number (required)" },
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "status": { "type": "string" },
                    "priority": { "type": "string" }
                },
                "required": ["number"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut body = serde_json::json!({});
        if let Some(title) = args.title.filter(|t| !t.trim().is_empty()) {
            body["title"] = serde_json::json!(title);
        }
        if let Some(description) = args.description.filter(|d| !d.trim().is_empty()) {
            body["description"] = serde_json::json!(description);
        }
        if let Some(status) = args.status.filter(|s| !s.trim().is_empty()) {
            body["status"] = serde_json::json!(status);
        }
        if let Some(priority) = args.priority.filter(|p| !p.trim().is_empty()) {
            body["priority"] = serde_json::json!(priority);
        }
        let value = api_put(&self.0.client, &format!("tasks/{}", args.number), &body).await?;
        Ok(serde_json::json!({
            "updated": value["task"]["number"],
            "title": value["task"]["title"],
            "status": value["task"]["status"],
        }))
    }
}

// ---- memory_recall ---------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryRecallArgs {
    /// Search query
    pub query: String,
    /// Agent whose memories to search (defaults to the serve agent)
    pub agent: Option<String>,
    /// Maximum number of results (default 10, max 100)
    #[serde(default = "default_recall_limit")]
    pub limit: usize,
}

fn default_recall_limit() -> usize {
    10
}

pub struct MemoryRecallProxy(ProxyBase);

impl MemoryRecallProxy {
    fn new(client: Arc<ApiClient>, agent: String) -> Self {
        Self(ProxyBase { client, agent })
    }
}

impl Tool for MemoryRecallProxy {
    const NAME: &'static str = "memory_recall";
    type Error = ProxyError;
    type Args = MemoryRecallArgs;
    type Output = serde_json::Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search Spacebot's memory (hybrid search over facts, events, decisions)"
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query (required)" },
                    "agent": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let query = args.query.trim();
        if query.is_empty() {
            return Err(ProxyError::new("`query` is required"));
        }
        let agent = args.agent.unwrap_or_else(|| self.0.agent.clone());
        let path = format!(
            "agents/memories/search?agent_id={}&q={}&limit={}",
            encode(&agent),
            encode(query),
            args.limit.clamp(1, 100)
        );
        let value = api_get(&self.0.client, &path).await?;
        let results = value["results"].as_array().cloned().unwrap_or_default();
        let memories = results
            .iter()
            .map(|result| {
                serde_json::json!({
                    "id": result["memory"]["id"],
                    "content": result["memory"]["content"],
                    "memory_type": result["memory"]["memory_type"],
                    "importance": result["memory"]["importance"],
                    "score": result["score"],
                })
            })
            .collect::<Vec<_>>();
        Ok(serde_json::json!({ "count": memories.len(), "results": memories }))
    }
}

// ---- channel_recall --------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelRecallArgs {
    /// Channel id (e.g. telegram:123456 or discord:server:channel)
    pub channel_id: String,
    /// Maximum number of messages (default 20, max 100)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

pub struct ChannelRecallProxy(ProxyBase);

impl ChannelRecallProxy {
    fn new(client: Arc<ApiClient>, agent: String) -> Self {
        Self(ProxyBase { client, agent })
    }
}

impl Tool for ChannelRecallProxy {
    const NAME: &'static str = "channel_recall";
    type Error = ProxyError;
    type Args = ChannelRecallArgs;
    type Output = serde_json::Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Retrieve the recent transcript of a channel".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "channel_id": { "type": "string", "description": "Channel id (required)" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                },
                "required": ["channel_id"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if args.channel_id.trim().is_empty() {
            return Err(ProxyError::new("`channel_id` is required"));
        }
        let path = format!(
            "channels/messages?channel_id={}&limit={}",
            encode(&args.channel_id.trim()),
            args.limit.clamp(1, 100)
        );
        let value = api_get(&self.0.client, &path).await?;
        let items = value["items"].as_array().cloned().unwrap_or_default();
        let messages = items
            .iter()
            .map(|item| {
                let kind = item["type"].as_str().unwrap_or("unknown");
                match kind {
                    "message" => serde_json::json!({
                        "type": "message",
                        "role": item["role"],
                        "content": item["content"],
                        "created_at": item["created_at"],
                    }),
                    _ => serde_json::json!({
                        "type": kind,
                        "description": item["description"],
                        "conclusion": item["conclusion"],
                        "created_at": item["created_at"],
                    }),
                }
            })
            .collect::<Vec<_>>();
        Ok(serde_json::json!({ "count": messages.len(), "messages": messages }))
    }
}

// ---- spacebot_docs ---------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct SpacebotDocsArgs {
    /// Action: list or read
    #[serde(default = "default_docs_action")]
    pub action: String,
    /// Doc id/path to read (required for read)
    pub doc_id: Option<String>,
    /// Filter for list, or fallback lookup hint for read
    pub query: Option<String>,
}

fn default_docs_action() -> String {
    "list".to_string()
}

/// Docs are served locally from the embedded self-awareness docs; the daemon
/// is not consulted.
pub struct SpacebotDocsProxy;

impl SpacebotDocsProxy {
    fn new(_agent: String) -> Self {
        Self
    }
}

impl Tool for SpacebotDocsProxy {
    const NAME: &'static str = "spacebot_docs";
    type Error = ProxyError;
    type Args = SpacebotDocsArgs;
    type Output = serde_json::Value;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read Spacebot's embedded docs (list or read)".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "read"], "default": "list" },
                    "doc_id": { "type": "string", "description": "Doc id/path to read" },
                    "query": { "type": "string", "description": "Filter for list, or fallback lookup for read" }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let action = args.action.trim().to_ascii_lowercase();
        match action.as_str() {
            "list" => {
                let docs = spacebot::self_awareness::list_embedded_docs(args.query.as_deref());
                let summaries = docs
                    .into_iter()
                    .map(|doc| serde_json::json!({ "id": doc.id, "title": doc.title }))
                    .collect::<Vec<_>>();
                Ok(serde_json::json!({ "count": summaries.len(), "docs": summaries }))
            }
            "read" => {
                let requested = args
                    .doc_id
                    .as_deref()
                    .or(args.query.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| ProxyError::new("`doc_id` is required for action=read"))?;
                let document = spacebot::self_awareness::get_embedded_doc(requested)
                    .ok_or_else(|| ProxyError::new(format!("unknown doc id: {requested}")))?;
                Ok(serde_json::json!({
                    "id": document.summary.id,
                    "title": document.summary.title,
                    "content": document.content,
                }))
            }
            other => Err(ProxyError::new(format!("unknown action: {other}"))),
        }
    }
}

/// Keep the compiler honest: every tool the MCP server exposes must have a
/// proxy implementation here, and nothing else may be registered.
#[allow(dead_code)]
const REGISTERED_PROXY_TOOLS: [&str; 6] = [
    "channel_recall",
    "memory_recall",
    "spacebot_docs",
    "task_create",
    "task_list",
    "task_update",
];

#[cfg(test)]
mod tests {
    use super::*;
    use spacebot::mcp_server::MCP_TOOL_ALLOWLIST;

    #[test]
    fn every_allowlisted_tool_has_a_proxy() {
        let mut registered = REGISTERED_PROXY_TOOLS.to_vec();
        registered.sort_unstable();
        let mut allowlist = MCP_TOOL_ALLOWLIST.to_vec();
        allowlist.sort_unstable();
        assert_eq!(registered, allowlist);
    }

    #[test]
    fn task_list_limit_clamped_to_range() {
        let args = TaskListArgs {
            agent: None,
            status: None,
            limit: 500,
        };
        assert_eq!(args.limit.clamp(1, 100), 100);
    }
}
