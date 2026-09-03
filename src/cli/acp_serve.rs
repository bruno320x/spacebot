//! `spacebot acp serve` — run a stdio ACP v1 agent server.
//!
//! Editors and hosts that speak Agent Client Protocol (the protocol used by
//! Claude Code, Cursor, etc.) spawn this command and drive Spacebot as a
//! coding agent — the mirror of the shipped ACP *client*. The protocol engine
//! lives in `spacebot::acp::server` and is generic over a `Session` backend.
//!
//! The session backend shipped with this step is a standby that speaks the
//! wire protocol end-to-end but does not run a branch yet; its reply names the
//! outstanding work so a caller is never misled into thinking a prompt was
//! executed. The branch-backed session is the immediate follow-up.

use clap::Args;
use spacebot::acp::server::{AgentInfo, RunResult, Server, Session};
use std::io::{BufReader, BufWriter, stdin, stdout};

#[derive(Debug, Args)]
pub struct AcpServeArgs {
    /// Agent whose identity/session the server presents
    #[arg(short, long)]
    pub agent: Option<String>,
}

pub async fn run(_ctx: &super::Context, args: AcpServeArgs) -> anyhow::Result<()> {
    let mut server = Server::new(AgentInfo::spacebot(env!("SPACEBOT_VERSION")), || {
        Box::new(StandbySession)
    });
    let mut reader = BufReader::new(stdin().lock());
    let mut writer = BufWriter::new(stdout().lock());
    tracing::info!(agent = ?args.agent, "serving ACP v1 over stdio");
    server.serve_reader(&mut reader, &mut writer)?;
    Ok(())
}

/// Session backend shipped with the door. Speaks the wire protocol but does
/// not run a branch; prompt replies name the outstanding step so callers are
/// never mistaken about what ran.
struct StandbySession;

impl Session for StandbySession {
    fn start(&mut self, _session_id: &str) {}

    fn run(&mut self, prompt: &str, resume: Option<&str>) -> RunResult {
        let echo = match resume {
            Some(outcome) => format!("prompt {prompt:?} resolved {outcome}"),
            None => format!(
                "console session ready — prompt received ({prompt:?}); branch backend lands next"
            ),
        };
        RunResult::Finished {
            updates: vec![],
            result: serde_json::json!({ "echo": echo }),
        }
    }
}
