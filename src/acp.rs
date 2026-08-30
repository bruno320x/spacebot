//! ACP (Agent Client Protocol) subprocess integration for coding workers.
//!
//! ACP is a JSON-RPC-over-stdio protocol for driving external coding agents
//! (Claude Code, Codex, Cursor CLI, etc.) as dedicated subprocesses. This
//! module provides an alternative worker backend that delegates to any
//! ACP-compatible CLI instead of running a Rig agent loop with basic tools.

pub mod types;
pub mod worker;

pub use worker::{AcpWorker, AcpWorkerResult};
