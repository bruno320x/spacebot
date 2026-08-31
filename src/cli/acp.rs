//! `spacebot acp` — ACP agent-server commands.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum AcpCommand {
    /// Run a stdio ACP v1 agent server (for editors and hosts)
    Serve {
        #[command(flatten)]
        args: super::acp_serve::AcpServeArgs,
    },
}

pub async fn run(ctx: &super::Context, cmd: AcpCommand) -> anyhow::Result<()> {
    match cmd {
        AcpCommand::Serve { args } => super::acp_serve::run(ctx, args).await,
    }
}
