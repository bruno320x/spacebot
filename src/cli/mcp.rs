//! `spacebot mcp` — MCP server management over the control API.

use super::client::{self, ApiClient};
use super::output;
use anyhow::Context as _;
use clap::Subcommand;
use spacebot::api::mcp::{McpAgentStatus, McpServerInfo, MutationResponse};

#[derive(Subcommand)]
pub enum McpCommand {
    /// List configured MCP servers
    List,
    /// Add an MCP server to config.toml
    Add {
        /// Server name
        name: String,
        /// Transport (stdio or http)
        #[arg(short, long, default_value = "stdio")]
        transport: String,
        /// Command to launch (stdio transport)
        #[arg(long)]
        command: Option<String>,
        /// Command argument, repeatable
        #[arg(long = "arg", value_name = "ARG")]
        args: Vec<String>,
        /// Environment variable as KEY=VALUE; pass KEY alone to be prompted for the value
        #[arg(short, long = "env", value_name = "KEY[=VALUE]")]
        env: Vec<String>,
        /// Server URL (http transport)
        #[arg(short, long)]
        url: Option<String>,
        /// HTTP header as NAME=VALUE; pass NAME alone to be prompted for the value
        #[arg(short = 'H', long = "header", value_name = "NAME[=VALUE]")]
        headers: Vec<String>,
        /// Add the server disabled
        #[arg(long)]
        disabled: bool,
    },
    /// Replace an existing MCP server definition. The definition is written
    /// exactly as the flags specify — env vars and headers omitted here are
    /// removed from the stored entry.
    Update {
        /// Server name
        name: String,
        /// Transport (stdio or http)
        #[arg(short, long)]
        transport: String,
        /// Command to launch (stdio transport)
        #[arg(long)]
        command: Option<String>,
        /// Command argument, repeatable
        #[arg(long = "arg", value_name = "ARG")]
        args: Vec<String>,
        /// Environment variable as KEY=VALUE; pass KEY alone to be prompted
        #[arg(short, long = "env", value_name = "KEY[=VALUE]")]
        env: Vec<String>,
        /// Server URL (http transport)
        #[arg(short, long)]
        url: Option<String>,
        /// HTTP header as NAME=VALUE; pass NAME alone to be prompted for the value
        #[arg(short = 'H', long = "header", value_name = "NAME[=VALUE]")]
        headers: Vec<String>,
        /// Leave the server disabled
        #[arg(long)]
        disabled: bool,
    },
    /// Remove an MCP server from config.toml
    Remove {
        /// Server name
        name: String,
    },
    /// Force-reconnect a server across all agents
    Reconnect {
        /// Server name
        name: String,
    },
    /// Per-agent MCP connection status
    Status,
    /// Run a stdio MCP server backed by the daemon (for editors)
    Serve {
        #[command(flatten)]
        args: super::mcp_serve::McpServeArgs,
    },
}

pub async fn run(ctx: &super::Context, cmd: McpCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match cmd {
        McpCommand::List => {
            let value = client.get("mcp/servers").await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let servers: Vec<McpServerInfo> = client::parse(value)?;
            if servers.is_empty() {
                eprintln!("No MCP servers configured.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = servers
                .iter()
                .map(|server| {
                    vec![
                        server.name.clone(),
                        server.transport.clone(),
                        server.enabled.to_string(),
                        server.state.clone(),
                    ]
                })
                .collect();
            output::table(&["NAME", "TRANSPORT", "ENABLED", "STATE"], &rows);
            Ok(())
        }
        McpCommand::Add {
            name,
            transport,
            command,
            args,
            env,
            url,
            headers,
            disabled,
        } => {
            let mut body = serde_json::json!({
                "name": name,
                "transport": transport,
                "enabled": !disabled,
            });
            if let Some(command) = &command {
                body["command"] = serde_json::json!(command);
            }
            if !args.is_empty() {
                body["args"] = serde_json::json!(args);
            }
            if let Some(url) = &url {
                body["url"] = serde_json::json!(url);
            }
            let env = parse_kv_pairs(&env, "env var")?;
            if !env.is_empty() {
                body["env"] = serde_json::Value::Object(env);
            }
            let headers = parse_kv_pairs(&headers, "header")?;
            if !headers.is_empty() {
                body["headers"] = serde_json::Value::Object(headers);
            }

            let value = client.post("mcp/servers", &body).await?;
            finish_mutation(ctx, value)
        }
        McpCommand::Update {
            name,
            transport,
            command,
            args,
            env,
            url,
            headers,
            disabled,
        } => {
            let mut body = serde_json::json!({
                "name": name,
                "transport": transport,
                "enabled": !disabled,
            });
            if let Some(command) = &command {
                body["command"] = serde_json::json!(command);
            }
            if !args.is_empty() {
                body["args"] = serde_json::json!(args);
            }
            if let Some(url) = &url {
                body["url"] = serde_json::json!(url);
            }
            let env = parse_kv_pairs(&env, "env var")?;
            if !env.is_empty() {
                body["env"] = serde_json::Value::Object(env);
            }
            let headers = parse_kv_pairs(&headers, "header")?;
            if !headers.is_empty() {
                body["headers"] = serde_json::Value::Object(headers);
            }

            let value = client.put("mcp/servers", &body).await?;
            finish_mutation(ctx, value)
        }
        McpCommand::Remove { name } => {
            let value = client
                .delete(&format!("mcp/servers/{}", client::encode_path(&name)))
                .await?;
            finish_mutation(ctx, value)
        }
        McpCommand::Reconnect { name } => {
            let value = client
                .post(
                    &format!("mcp/servers/{}/reconnect", client::encode_path(&name)),
                    &serde_json::json!({}),
                )
                .await?;
            finish_mutation(ctx, value)
        }
        McpCommand::Serve { args } => super::mcp_serve::run(ctx, args).await,
        McpCommand::Status => {
            let value = client.get("mcp/status").await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let agents: Vec<McpAgentStatus> = client::parse(value)?;
            let rows: Vec<Vec<String>> = agents
                .iter()
                .flat_map(|agent| {
                    agent.servers.iter().map(|server| {
                        vec![
                            agent.agent_id.clone(),
                            server.name.clone(),
                            server.transport.clone(),
                            server.enabled.to_string(),
                            server.state.clone(),
                        ]
                    })
                })
                .collect();
            if rows.is_empty() {
                eprintln!("No MCP servers connected.");
                return Ok(());
            }
            output::table(&["AGENT", "SERVER", "TRANSPORT", "ENABLED", "STATE"], &rows);
            Ok(())
        }
    }
}

/// Parse KEY=VALUE pairs. A bare KEY prompts for its value so credentials
/// stay out of shell history.
fn parse_kv_pairs(
    pairs: &[String],
    what: &str,
) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    let mut map = serde_json::Map::new();
    for pair in pairs {
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key.to_string(), value.to_string()),
            None => {
                let value = dialoguer::Password::new()
                    .with_prompt(format!("Value for {what} {pair}"))
                    .interact()
                    .with_context(|| format!("failed to read {what} value"))?;
                (pair.clone(), value)
            }
        };
        if key.is_empty() {
            anyhow::bail!("{what} name cannot be empty");
        }
        map.insert(key, serde_json::Value::String(value));
    }
    Ok(map)
}

/// Render a mutation result, failing the command when the API reports
/// an unsuccessful mutation.
fn finish_mutation(ctx: &super::Context, value: serde_json::Value) -> anyhow::Result<()> {
    if ctx.json {
        output::json(&value);
        return Ok(());
    }
    let result: MutationResponse = client::parse(value)?;
    if !result.success {
        anyhow::bail!("{}", result.message);
    }
    eprintln!("{}", result.message);
    Ok(())
}
