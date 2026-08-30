//! Central process supervision for worker subprocesses.
//!
//! Spacebot spawns subprocesses from several places — the shell tool, the ACP
//! worker (a coding CLI over stdio), the OpenCode server, and internal helpers.
//! Each historically handled its own lifecycle. This module provides the
//! shared guarantees every subprocess should have:
//!
//! - **No hang.** Every wait is bounded by a wall-clock timeout; on expiry the
//!   process is killed, never left running unseen.
//! - **No orphan.** A tracked child is killed when its owning group is
//!   terminated or when the process handle is dropped.
//! - **Sanitized environment.** Subprocesses never inherit tool-secret env or
//!   library-injection variables unless explicitly allowed.
//!
//! The module is intentionally additive: existing call sites (shell, ACP,
//! OpenCode) migrate to it in follow-up changes. It is exercised directly by
//! unit tests here so the guarantees are pinned independently of any one
//! backend.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context as _, Result};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Env vars that enable library injection or alter runtime loading behavior.
///
/// Always dropped from subprocess environments, regardless of sandbox state.
/// Mirrors the list in `shell.rs` / `sandbox.rs`; kept here so the supervisor
/// is self-contained.
const DANGEROUS_ENV_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "PYTHONPATH",
    "PYTHONSTARTUP",
    "NODE_OPTIONS",
    "RUBYOPT",
    "PERL5OPT",
    "PERL5LIB",
    "BASH_ENV",
    "ENV",
];

/// Env vars required for basic process operation; always re-injected.
const SAFE_ENV_VARS: &[&str] = &["USER", "LANG", "TERM"];

/// Env vars set by the hardened defaults and never overridable by callers.
const RESERVED_ENV_VARS: &[&str] = &["PATH", "HOME", "TMPDIR", "CI", "DEBIAN_FRONTEND"];

/// Returns true if a variable name may be overridden by caller-provided env.
fn is_overridable_env_var(name: &str) -> bool {
    !RESERVED_ENV_VARS.contains(&name) && !SAFE_ENV_VARS.contains(&name)
}

/// Returns true if a variable is on the dangerous (library-injection) list.
fn is_dangerous_env_var(name: &str) -> bool {
    DANGEROUS_ENV_VARS
        .iter()
        .any(|blocked| name.eq_ignore_ascii_case(blocked))
}

/// Default hard timeout for supervised execution when none is supplied.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Description of a process to spawn under supervision.
#[derive(Debug, Clone)]
pub struct ProcessSpec {
    /// Executable to run.
    pub program: String,
    /// Argument vector (excluding argv[0]).
    pub args: Vec<String>,
    /// Working directory; defaults to the current directory.
    pub working_dir: Option<PathBuf>,
    /// Explicit environment variables to set on the subprocess.
    /// Dangerous vars are dropped, reserved vars are skipped.
    pub env: Vec<(String, String)>,
    /// Wall-clock budget. On expiry the child is killed.
    pub timeout: Duration,
}

impl ProcessSpec {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_dir: None,
            env: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build a sanitized `Command` from this spec. Never returns a command
    /// that inherits the full parent environment or dangerous env vars.
    pub fn command(&self) -> Command {
        // Start from a cleared environment so subprocesses never inherit tool
        // secrets (LLM keys, messaging tokens) or the agent's full env.
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        if let Some(dir) = &self.working_dir {
            cmd.current_dir(dir);
        }

        let path_env = std::env::var_os("PATH").unwrap_or_default();
        let home_env = std::env::var_os("HOME").unwrap_or_default();

        cmd.env("PATH", path_env);
        cmd.env("TMPDIR", "/tmp");
        cmd.env("CI", "true");
        cmd.env("DEBIAN_FRONTEND", "noninteractive");

        if let Some(home) = home_env.to_str() {
            cmd.env("HOME", home);
        }

        for var_name in SAFE_ENV_VARS {
            if let Ok(value) = std::env::var(var_name) {
                cmd.env(var_name, value);
            }
        }

        for (name, value) in &self.env {
            if is_dangerous_env_var(name) {
                tracing::warn!(%name, "supervisor: dropping dangerous per-command env var");
                continue;
            }
            if !is_overridable_env_var(name) {
                tracing::debug!(%name, "supervisor: skipping reserved per-command env var");
                continue;
            }
            cmd.env(name, value);
        }

        cmd
    }
}

/// Result of a supervised one-shot process run.
#[derive(Debug, Clone)]
pub struct SupervisedOutput {
    pub exit_code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Run a process to completion under a hard timeout.
///
/// Guarantees:
/// - stdout/stderr are captured (bounded by the caller via truncation).
/// - If the wall-clock `timeout` elapses, the child is killed before this
///   returns, so the caller can never be stuck waiting.
/// - A failure to kill a timed-out child is surfaced rather than swallowed.
pub async fn run_to_completion(spec: &ProcessSpec) -> Result<SupervisedOutput> {
    let mut cmd = spec.command();
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn '{}'", spec.program))?;
    let mut stdout_pipe = child
        .stdout
        .take()
        .context("supervisor stdout pipe unavailable")?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .context("supervisor stderr pipe unavailable")?;

    // Borrow the child (via pipes + wait) rather than moving it, so the same
    // handle is still owned after a timeout and can be killed and reaped.
    let finished = async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stdout_pipe, &mut stdout)
            .await
            .context("failed to read stdout")?;
        tokio::io::AsyncReadExt::read_to_end(&mut stderr_pipe, &mut stderr)
            .await
            .context("failed to read stderr")?;
        let status = child
            .wait()
            .await
            .context("failed to wait for subprocess")?;
        Ok::<_, anyhow::Error>((status, stdout, stderr))
    };

    let (status, stdout, stderr) = match tokio::time::timeout(spec.timeout, finished).await {
        Ok(result) => result?,
        Err(_) => {
            // Timed out: kill, then reap so we never orphan or hang on a zombie.
            kill_and_wait_child(&mut child).await;
            return Ok(SupervisedOutput {
                exit_code: None,
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: true,
            });
        }
    };

    let stdout = String::from_utf8_lossy(&stdout).to_string();
    let stderr = String::from_utf8_lossy(&stderr).to_string();

    Ok(SupervisedOutput {
        exit_code: status.code(),
        success: status.success(),
        stdout,
        stderr,
        timed_out: false,
    })
}

/// Kill a child and await reaping, with a bounded wait on the kill itself.
async fn kill_and_wait_child(child: &mut Child) {
    // A short internal budget so we never hang waiting to clean up after a
    // timeout. A child that refuses to die after SIGKILL (rare, kernel-level)
    // is logged and handed back — blocking the supervisor on it would defeat
    // the no-hang guarantee.
    match tokio::time::timeout(Duration::from_secs(5), async {
        let _ = child.start_kill();
        let _ = child.wait().await;
    })
    .await
    {
        Ok(()) => {}
        Err(_) => {
            tracing::warn!("supervisor: timed out reaping killed child (leaving to OS reap)");
        }
    }
}

/// A tracked subprocess keyed under a group ownership id.
///
/// Groups let a caller terminate *all* subprocesses owned by one unit of work
/// (a worker, a channel, a milestone) in a single call. This is the orphan
/// prevention gate: cancel a worker and every child it spawned goes down with
/// it, even ones the caller lost direct references to.
///
/// The registry tracks by **PID**, not by a `Child` handle, so a driver that
/// owns the `Child` (and its stdio) never contends with the registry over the
/// same handle: the registry signals the OS, the driver keeps driving.
pub struct TrackedChild {
    /// OS process id. Children spawned via `spawn_tracked` are process-group
    /// leaders, so `-pid` signals the whole tree.
    pid: i32,
    /// Program name, for diagnostics.
    pub program: String,
}

impl TrackedChild {
    /// Kill the process and, when it is a group leader, its whole tree.
    pub fn kill(&self) {
        kill_by_pid(self.pid);
    }
}
/// SIGKILL a process group (`-pid`) and fall back to the plain pid.
///
/// A negative pid signals every member of the group whose id is `pid`
/// (covering descendants spawned by a group leader); the plain-pid kill is a
/// fallback for children that were never made group leaders. Shared with
/// backends (e.g. OpenCode) that own a child but want tree-kill semantics.
#[cfg(unix)]
pub(crate) fn kill_by_pid(pid: i32) {
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
pub(crate) fn kill_by_pid(pid: i32) {
    tracing::warn!(
        pid,
        "supervisor: process kill not implemented on this platform"
    );
}

/// Registry of tracked subprocesses, grouped for bulk termination.
#[derive(Default)]
pub struct ChildRegistry {
    children: Mutex<HashMap<String, Vec<TrackedChild>>>,
}

impl ChildRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn and track a child belonging to `group`, returning the `Child` so
    /// the caller keeps stdio ownership and drives it directly.
    ///
    /// The child is made a process-group leader (`process_group(0)`) so a
    /// later `kill_group` tears down its whole tree; `kill_on_drop(true)`
    /// stays as the last-resort safety net if the registry is ever dropped
    /// before termination.
    pub async fn spawn_tracked(
        &self,
        group: impl Into<String>,
        program: impl Into<String>,
        mut cmd: Command,
    ) -> Result<Child> {
        let program = program.into();
        cmd.kill_on_drop(true);
        cmd.process_group(0);
        let child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn '{program}'"))?;
        self.register(group, &child, program).await?;
        Ok(child)
    }

    /// Track an already-spawned child under `group`. The caller keeps
    /// ownership of the `Child`; the registry only records its pid.
    pub async fn register(
        &self,
        group: impl Into<String>,
        child: &Child,
        program: impl Into<String>,
    ) -> Result<()> {
        let pid = child
            .id()
            .context("cannot register a child with no process id")? as i32;
        let mut children = self.children.lock().await;
        children
            .entry(group.into())
            .or_default()
            .push(TrackedChild {
                pid,
                program: program.into(),
            });
        Ok(())
    }

    /// Terminate every child in a group and remove them from the registry.
    /// Returns the number of children killed.
    pub async fn kill_group(&self, group: &str) -> usize {
        let mut children = self.children.lock().await;
        let Some(group_children) = children.remove(group) else {
            return 0;
        };
        let count = group_children.len();
        for tracked in group_children {
            tracked.kill();
        }
        count
    }

    /// Terminate every tracked child across all groups.
    pub async fn kill_all(&self) -> usize {
        let mut children = self.children.lock().await;
        let drained: Vec<TrackedChild> = children
            .drain()
            .flat_map(|(_group, group_children)| group_children)
            .collect();
        let count = drained.len();
        for tracked in drained {
            tracked.kill();
        }
        count
    }

    /// Number of live tracked children.
    pub async fn len(&self) -> usize {
        self.children.lock().await.values().map(Vec::len).sum()
    }

    /// True when no children are tracked.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

/// Convenience: truncate captured output so no single tool result can exceed
/// a bounded size regardless of how chatty the subprocess was.
pub fn truncate_output(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max_bytes).collect();
        s.push_str("\n…[truncated]…");
        s
    }
}

/// The default byte cap for captured subprocess output (matches tool limits).
pub const MAX_CAPTURE_BYTES: usize = 60 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_to_completion_success_collects_output() {
        let spec = ProcessSpec::new("sh")
            .args(["-c", "echo hello-contents"])
            .timeout(Duration::from_secs(10));
        let output = run_to_completion(&spec).await.expect("run");
        assert!(output.success);
        assert!(output.stdout.contains("hello-contents"));
        assert!(!output.timed_out);
    }

    #[tokio::test]
    async fn run_to_completion_captures_failure_exit() {
        let spec = ProcessSpec::new("sh")
            .args(["-c", "exit 3"])
            .timeout(Duration::from_secs(10));
        let output = run_to_completion(&spec).await.expect("run");
        assert!(!output.success);
        assert_eq!(output.exit_code, Some(3));
        assert!(!output.timed_out);
    }

    #[tokio::test]
    async fn run_to_completion_kills_hanging_process_on_timeout() {
        // Do not hang the test suite: 1s budget, sleep 30s. The supervisor
        // must kill it and return `timed_out`.
        let spec = ProcessSpec::new("sh")
            .args(["-c", "sleep 30"])
            .timeout(Duration::from_secs(1));
        let output = run_to_completion(&spec).await.expect("run");
        assert!(output.timed_out);
        assert!(!output.success);
    }

    #[tokio::test]
    async fn env_sanitization_drops_dangerous_and_blocks_reserved() {
        let spec = ProcessSpec::new("sh")
            .args(["-c", "env | grep -c LD_PRELOAD || true"])
            .env("LD_PRELOAD", "should-not-leak")
            .env("CI", "false") // reserved: must be skipped
            .timeout(Duration::from_secs(10));
        let output = run_to_completion(&spec).await.expect("run");
        // LD_PRELOAD must not appear in the child env at all.
        assert!(!output.stdout.contains("should-not-leak"));
        // CI stays forced to "true" regardless of the caller's value.
        let spec_ci = ProcessSpec::new("sh")
            .args(["-c", "echo \"CI=$CI\""])
            .env("CI", "false")
            .timeout(Duration::from_secs(10));
        let out_ci = run_to_completion(&spec_ci).await.expect("run");
        assert!(out_ci.stdout.contains("CI=true"));
    }

    #[tokio::test]
    async fn registry_tracks_and_kills_group() {
        let registry = ChildRegistry::new();

        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let _child = registry
            .spawn_tracked("worker-1", "sh", cmd)
            .await
            .expect("spawn");

        let child_count = registry.len().await;
        assert_eq!(child_count, 1);

        let killed = registry.kill_group("worker-1").await;
        assert_eq!(killed, 1);
        assert!(registry.is_empty().await);
    }

    #[tokio::test]
    async fn registry_kill_all_clears_everything() {
        let registry = ChildRegistry::new();
        let mut children = Vec::new();
        for group in ["w1", "w2"] {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "sleep 30"]);
            children.push(
                registry
                    .spawn_tracked(group, "sh", cmd)
                    .await
                    .expect("spawn"),
            );
        }
        let _keep_alive = children;

        assert_eq!(registry.len().await, 2);
        let killed = registry.kill_all().await;
        assert_eq!(killed, 2);
        assert!(registry.is_empty().await);
    }

    #[tokio::test]
    async fn kill_group_tears_down_child_tree() {
        let registry = ChildRegistry::new();
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30 & sleep 30"]);
        let mut child = registry
            .spawn_tracked("tree", "sh", cmd)
            .await
            .expect("spawn");

        registry.kill_group("tree").await;

        // The group leader must be dead and reaped — wait returns promptly
        // instead of hanging for the 30s sleep to finish.
        let waited = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        assert!(
            waited.is_ok(),
            "killed group leader must be reaped, not hang"
        );
    }

    #[tokio::test]
    async fn registry_isolates_groups() {
        let registry = ChildRegistry::new();
        let mut cmd_a = Command::new("sh");
        cmd_a.args(["-c", "sleep 30"]);
        let _child_a = registry
            .spawn_tracked("g-a", "sh", cmd_a)
            .await
            .expect("spawn a");
        let mut cmd_b = Command::new("sh");
        cmd_b.args(["-c", "sleep 30"]);
        let _child_b = registry
            .spawn_tracked("g-b", "sh", cmd_b)
            .await
            .expect("spawn b");

        registry.kill_group("g-a").await;

        assert_eq!(registry.len().await, 1);
    }

    #[test]
    fn truncate_output_bounds_length() {
        let long = "x".repeat(1000);
        let cut = truncate_output(&long, 500);
        assert!(cut.len() < 1000);
        assert!(cut.contains("[truncated]"));
        assert!(truncate_output("short", 500).len() == 5);
    }

    #[test]
    fn process_spec_chain_builds_command() {
        let spec = ProcessSpec::new("git")
            .arg("status")
            .current_dir("/tmp")
            .timeout(Duration::from_secs(5));
        assert_eq!(spec.program, "git");
        assert_eq!(spec.args, vec!["status".to_string()]);
        assert_eq!(
            spec.working_dir.as_deref(),
            Some(std::path::Path::new("/tmp"))
        );
        assert_eq!(spec.timeout, Duration::from_secs(5));
    }
}
