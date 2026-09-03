//! CLI command tree and dispatch.
//!
//! Resource commands are HTTP clients of the running daemon's control API,
//! authenticated with the bearer token from config. Lifecycle commands
//! (start/stop/restart/status) talk to the daemon socket and stay in
//! `main.rs` alongside the daemon entry point.

mod activity;
mod agent;
mod auth;
mod binding;
mod channel;
mod chat;
mod client;
mod completions;
mod config;
mod cron;
mod dashboard;
mod desktop;
mod ingest;
mod mcp;
mod mcp_serve;
mod memory;
mod messaging;
mod model;
mod notification;
mod output;
mod project;
mod prompt;
mod provider;
mod secrets;
mod skill;
mod task;
mod update;
mod usage;
mod wiki;

use anyhow::Context as _;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "spacebot", version = env!("SPACEBOT_VERSION"))]
#[command(about = "A Rust agentic system with dedicated processes for every task")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Path to config file (optional)
    #[arg(short, long, global = true)]
    pub config: Option<std::path::PathBuf>,

    /// Enable debug logging
    #[arg(short, long, global = true)]
    pub debug: bool,

    /// Print raw API responses as JSON
    #[arg(long, global = true)]
    pub json: bool,

    /// Base URL of a spacebot instance (defaults to the local daemon)
    #[arg(long, global = true, env = "SPACEBOT_URL")]
    pub url: Option<String>,

    /// API auth token (defaults to api.auth_token from config)
    #[arg(long, global = true, env = "SPACEBOT_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the daemon (default when no subcommand is given)
    Start {
        /// Run in the foreground instead of daemonizing
        #[arg(short, long)]
        foreground: bool,
    },
    /// Stop the running daemon
    Stop,
    /// Restart the daemon (stop + start)
    Restart {
        /// Run in the foreground instead of daemonizing
        #[arg(short, long)]
        foreground: bool,
    },
    /// Show status of the running daemon
    Status,
    /// Manage agents
    #[command(subcommand)]
    Agent(agent::AgentCommand),
    /// Inspect and manage conversation channels
    #[command(subcommand)]
    Channel(channel::ChannelCommand),
    /// Manage tasks
    #[command(subcommand)]
    Task(task::TaskCommand),
    /// Manage cron jobs
    #[command(subcommand)]
    Cron(cron::CronCommand),
    /// Browse and search agent memories
    #[command(subcommand)]
    Memory(memory::MemoryCommand),
    /// Manage wiki pages
    #[command(subcommand)]
    Wiki(wiki::WikiCommand),
    /// Manage projects, repos, and worktrees
    #[command(subcommand)]
    Project(project::ProjectCommand),
    /// Manage ingest files
    #[command(subcommand)]
    Ingest(ingest::IngestCommand),
    /// Manage messaging platforms and adapter instances
    #[command(subcommand)]
    Messaging(messaging::MessagingCommand),
    /// Manage agent-to-channel bindings
    #[command(subcommand)]
    Binding(Box<binding::BindingCommand>),
    /// Manage LLM providers
    #[command(subcommand)]
    Provider(provider::ProviderCommand),
    /// Browse the model catalog
    #[command(subcommand)]
    Model(model::ModelCommand),
    /// Manage MCP servers
    #[command(subcommand)]
    Mcp(mcp::McpCommand),
    /// Manage global settings and raw config
    #[command(subcommand)]
    Config(config::ConfigCommand),
    /// Manage notifications
    #[command(subcommand)]
    Notification(notification::NotificationCommand),
    /// Show token usage
    Usage(usage::UsageArgs),
    /// Show daily activity
    Activity(activity::ActivityArgs),
    /// Inspect captured LLM requests
    #[command(subcommand)]
    Prompt(prompt::PromptCommand),
    /// Check for and apply updates
    #[command(subcommand)]
    Update(update::UpdateCommand),
    /// Manage skills
    #[command(subcommand)]
    Skill(skill::SkillCommand),
    /// Manage authentication
    #[command(subcommand)]
    Auth(auth::AuthCommand),
    /// Manage secrets stored in the running instance
    #[command(subcommand)]
    Secrets(secrets::SecretsCommand),
    /// Chat with an agent from the terminal
    Chat(chat::ChatArgs),
    /// Open the web dashboard, starting the daemon if needed
    Dashboard(dashboard::DashboardArgs),
    /// Launch the installed desktop app
    Desktop,
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
}

/// Global options shared by every resource command.
pub struct Context {
    pub config_path: Option<std::path::PathBuf>,
    pub json: bool,
    pub url: Option<String>,
    pub token: Option<String>,
}

/// Run a resource command. Lifecycle commands are dispatched in `main.rs`
/// before this is reached.
pub fn dispatch(command: Command, ctx: Context) -> anyhow::Result<()> {
    if let Command::Completions { shell } = command {
        return completions::run(shell);
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;

    runtime.block_on(async {
        match command {
            Command::Agent(cmd) => agent::run(&ctx, cmd).await,
            Command::Channel(cmd) => channel::run(&ctx, cmd).await,
            Command::Task(cmd) => task::run(&ctx, cmd).await,
            Command::Cron(cmd) => cron::run(&ctx, cmd).await,
            Command::Memory(cmd) => memory::run(&ctx, cmd).await,
            Command::Wiki(cmd) => wiki::run(&ctx, cmd).await,
            Command::Project(cmd) => project::run(&ctx, cmd).await,
            Command::Ingest(cmd) => ingest::run(&ctx, cmd).await,
            Command::Messaging(cmd) => messaging::run(&ctx, cmd).await,
            Command::Binding(cmd) => binding::run(&ctx, *cmd).await,
            Command::Provider(cmd) => provider::run(&ctx, cmd).await,
            Command::Model(cmd) => model::run(&ctx, cmd).await,
            Command::Mcp(cmd) => mcp::run(&ctx, cmd).await,
            Command::Config(cmd) => config::run(&ctx, cmd).await,
            Command::Notification(cmd) => notification::run(&ctx, cmd).await,
            Command::Usage(args) => usage::run(&ctx, args).await,
            Command::Activity(args) => activity::run(&ctx, args).await,
            Command::Prompt(cmd) => prompt::run(&ctx, cmd).await,
            Command::Update(cmd) => update::run(&ctx, cmd).await,
            Command::Skill(cmd) => skill::run(&ctx, cmd).await,
            Command::Auth(cmd) => auth::run(&ctx, cmd).await,
            Command::Secrets(cmd) => secrets::run(&ctx, cmd).await,
            Command::Chat(args) => chat::run(&ctx, args).await,
            Command::Dashboard(args) => dashboard::run(&ctx, args).await,
            Command::Desktop => desktop::run(&ctx).await,
            Command::Completions { .. }
            | Command::Start { .. }
            | Command::Stop
            | Command::Restart { .. }
            | Command::Status => {
                unreachable!("lifecycle commands are dispatched in main")
            }
        }
    })
}

pub fn load_config(
    config_path: &Option<std::path::PathBuf>,
) -> anyhow::Result<spacebot::config::Config> {
    if let Some(path) = config_path {
        spacebot::config::Config::load_from_path(path)
            .with_context(|| format!("failed to load config from {}", path.display()))
    } else {
        spacebot::config::Config::load().with_context(|| "failed to load configuration")
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    /// clap only validates the command tree when it's built, so conflicts
    /// like duplicate short flags panic at runtime. Catch them here instead.
    #[test]
    fn cli_tree_is_valid() {
        super::Cli::command().debug_assert();
    }
}
