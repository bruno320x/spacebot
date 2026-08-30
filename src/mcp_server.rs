//! MCP server — expose Spacebot's curated tools to external coding agents
//! (Claude Code, Cursor, etc.) over the Model Context Protocol.
//!
//! Editors spawn this server over stdio and get a small, curated tool surface:
//! task-board (create/list/update), memory recall, docs introspection, and
//! cross-channel recall. Shell/file execution stays out of the editor context —
//! it is reserved for trusted workers running under the supervisor.

use std::borrow::Cow;

use rig::tool::server::ToolServerHandle;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParams, CallToolResult, Content, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};

/// Tools exposed over MCP. Curated allowlist — shell/file and other
/// code-execution tools stay out of the editor context (untrusted by default).
pub const MCP_TOOL_ALLOWLIST: &[&str] = &[
    "channel_recall",
    "memory_recall",
    "spacebot_docs",
    "task_create",
    "task_list",
    "task_update",
];

/// Spacebot's MCP server. Wraps a Rig [`ToolServerHandle`] and exposes only the
/// curated allowlist over the wire. Everything else on the handle is invisible
/// to MCP clients.
#[derive(Clone)]
pub struct McpServer {
    tools: ToolServerHandle,
}

impl McpServer {
    pub fn new(tools: ToolServerHandle) -> Self {
        Self { tools }
    }

    /// Serve over stdio — what editors spawn (`mcpServers.spacebot.command`).
    pub async fn serve(self) -> anyhow::Result<()> {
        let service = rmcp::service::serve_server(self, rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    }

    fn is_allowed(name: &str) -> bool {
        MCP_TOOL_ALLOWLIST.contains(&name)
    }

    fn tool_from_definition(def: &rig::completion::ToolDefinition) -> Option<Tool> {
        let input_schema = def.parameters.as_object().cloned().unwrap_or_default();
        Some(Tool::new_with_raw(
            def.name.clone(),
            (!def.description.is_empty()).then(|| Cow::Owned(def.description.clone())),
            input_schema,
        ))
    }
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::default()).with_server_info(
            Implementation::new("spacebot", env!("CARGO_PKG_VERSION"))
                .with_description("Spacebot MCP server — task board, memory, docs"),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let defs = self
            .tools
            .get_tool_defs(None)
            .await
            .map_err(|err| rmcp::ErrorData::internal_error(err.to_string(), None))?;
        let tools = defs
            .iter()
            .filter(|def| Self::is_allowed(&def.name))
            .filter_map(Self::tool_from_definition)
            .collect();
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        if !Self::is_allowed(name) {
            return None;
        }
        Some(Tool::new_with_raw(
            name.to_string(),
            None,
            serde_json::Map::new(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let name = request.name.as_ref();
        if !Self::is_allowed(name) {
            return Err(rmcp::ErrorData::method_not_found::<CallToolRequestMethod>());
        }
        let args = request.arguments.unwrap_or_default();
        let args_json = serde_json::to_string(&args).map_err(|err| {
            rmcp::ErrorData::invalid_params(format!("failed to serialize arguments: {err}"), None)
        })?;
        match self.tools.call_tool(name, &args_json).await {
            Ok(output) => Ok(CallToolResult::success(vec![Content::text(output)])),
            Err(err) => Err(rmcp::ErrorData::internal_error(err.to_string(), None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::service::{RoleClient, ServiceExt, serve_server};
    use serde::Deserialize;

    // ---- Fake rig tools -----------------------------------------------------

    #[derive(Debug)]
    struct FakeToolError(String);

    impl std::fmt::Display for FakeToolError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for FakeToolError {}

    #[derive(Debug, Clone, Deserialize)]
    struct TaskListArgs {}

    #[derive(Debug, Clone, serde::Serialize)]
    struct TaskListOutput {
        tasks: Vec<String>,
    }

    struct FakeTaskListTool;

    impl rig::tool::Tool for FakeTaskListTool {
        const NAME: &'static str = "task_list";
        type Error = FakeToolError;
        type Args = TaskListArgs;
        type Output = TaskListOutput;

        async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
            rig::completion::ToolDefinition {
                name: "task_list".into(),
                description: "List tasks for the agent".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            }
        }

        async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
            Ok(TaskListOutput {
                tasks: vec!["task-1".into()],
            })
        }
    }

    #[derive(Debug, Clone, Deserialize)]
    struct ShellArgs {}

    #[derive(Debug, Clone, serde::Serialize)]
    struct ShellOutput {
        ran: bool,
    }

    /// Deliberately NOT in the allowlist — must be invisible to MCP clients.
    struct FakeShellTool;

    impl rig::tool::Tool for FakeShellTool {
        const NAME: &'static str = "shell";
        type Error = FakeToolError;
        type Args = ShellArgs;
        type Output = ShellOutput;

        async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
            rig::completion::ToolDefinition {
                name: "shell".into(),
                description: "Run a shell command".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            }
        }

        async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
            Ok(ShellOutput { ran: true })
        }
    }

    fn build_handle() -> ToolServerHandle {
        rig::tool::server::ToolServer::new()
            .tool(FakeTaskListTool)
            .tool(FakeShellTool)
            .run()
    }

    /// Wire the server and a mock client together over in-memory duplex
    /// streams, run the MCP handshake, and return both running services. The
    /// server must stay alive for the test: dropping it closes the transport.
    async fn connect() -> (
        rmcp::service::RunningService<RoleServer, McpServer>,
        rmcp::service::RunningService<RoleClient, ()>,
    ) {
        let (server_to_client, client_from_server) = tokio::io::duplex(16 * 1024);
        let (client_to_server, server_from_client) = tokio::io::duplex(16 * 1024);

        let server = serve_server(
            McpServer::new(build_handle()),
            (server_from_client, server_to_client),
        );
        let client = ().serve((client_from_server, client_to_server));

        let (server, client) = tokio::join!(server, client);
        (
            server.expect("server handshake failed"),
            client.expect("client handshake failed"),
        )
    }

    #[tokio::test]
    async fn list_tools_exposes_only_allowlist() {
        let (_server, client) = connect().await;
        let tools = client.peer().list_all_tools().await.expect("list_tools");

        let names: Vec<String> = tools.iter().map(|tool| tool.name.to_string()).collect();
        assert_eq!(names, vec!["task_list"], "only allowlisted tools appear");
        for tool in &tools {
            assert!(MCP_TOOL_ALLOWLIST.contains(&tool.name.as_ref()));
        }
    }

    #[tokio::test]
    async fn call_tool_roundtrips_text_content() {
        let (_server, client) = connect().await;
        let result = client
            .peer()
            .call_tool(
                CallToolRequestParams::new("task_list")
                    .with_arguments(serde_json::json!({}).as_object().unwrap().clone()),
            )
            .await
            .expect("call_tool");

        let text = result
            .content
            .iter()
            .filter_map(|content| content.as_text())
            .map(|text| text.text.clone())
            .collect::<String>();
        assert!(
            text.contains("task-1"),
            "tool output reaches the client: {text}"
        );
    }

    #[tokio::test]
    async fn call_tool_rejects_non_allowlisted_tool() {
        let (_server, client) = connect().await;
        let err = client
            .peer()
            .call_tool(
                CallToolRequestParams::new("shell")
                    .with_arguments(serde_json::json!({}).as_object().unwrap().clone()),
            )
            .await
            .expect_err("shell is not exposed over MCP");
        assert!(
            err.to_string().contains("-32601"),
            "expected METHOD_NOT_FOUND (-32601), got: {err}"
        );
    }

    #[test]
    fn allowlist_is_sorted_and_unique() {
        let mut sorted = MCP_TOOL_ALLOWLIST.to_vec();
        sorted.sort_unstable();
        assert_eq!(MCP_TOOL_ALLOWLIST.to_vec(), sorted, "allowlist is sorted");
        sorted.dedup();
        assert_eq!(MCP_TOOL_ALLOWLIST.to_vec(), sorted, "allowlist is unique");
    }
}
