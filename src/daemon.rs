//! Process daemonization and IPC for background operation.

use crate::config::{Config, TelemetryConfig};

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithHttpConfig;
use opentelemetry_sdk::trace::SdkTracerProvider;
use serde::{Deserialize, Serialize};
<<<<<<< ours
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
=======
>>>>>>> theirs
use tracing_subscriber::fmt::format;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use std::path::PathBuf;

/// Commands sent from CLI client to the running daemon.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum IpcCommand {
    Shutdown,
    Restart,
    Status,
}

/// Responses from the daemon back to the CLI client.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum IpcResponse {
    Ok,
    Status {
        pid: u32,
        uptime_seconds: u64,
        /// Per-process nonce. A restart is confirmed by observing this value
        /// change, since a foreground re-exec keeps the same PID. Optional so
        /// clients tolerate daemons that predate the field.
        #[serde(default)]
        run_id: Option<String>,
    },
    Error {
        message: String,
    },
}

/// Paths for daemon runtime files, all derived from the instance directory.
pub struct DaemonPaths {
    pub pid_file: PathBuf,
    pub socket: PathBuf,
    pub log_dir: PathBuf,
}

impl DaemonPaths {
    pub fn new(instance_dir: &std::path::Path) -> Self {
        Self {
            pid_file: instance_dir.join("spacebot.pid"),
            socket: instance_dir.join("spacebot.sock"),
            log_dir: instance_dir.join("logs"),
        }
    }

    pub fn from_default() -> Self {
        Self::new(&Config::default_instance_dir())
    }
}

fn truncate_for_log(message: &str, max_chars: usize) -> (&str, bool) {
    match message.char_indices().nth(max_chars) {
        Some((byte_index, _character)) => (&message[..byte_index], true),
        None => (message, false),
    }
}

<<<<<<< ours
/// Check whether a daemon is already running by testing PID file liveness
/// and socket connectivity.
pub fn is_running(paths: &DaemonPaths) -> Option<u32> {
    // Synchronous std IO traits, scoped here: this runs on the CLI side
    // before any runtime exists, while the module-level imports are the
    // daemon's tokio equivalents.
    use std::io::{BufRead as _, Write as _};

    // Prefer querying the IPC socket first. This handles cases where the
    // daemon was started in foreground (or inside a container as PID 1) and
    // therefore did not create a PID file via the daemonize helper.
    if paths.socket.exists() {
        if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&paths.socket) {
            // Try to query the daemon for its status synchronously. If this
            // succeeds we can return the authoritative PID returned by the
            // daemon. If the query fails but the socket connected, treat the
            // daemon as running (unknown PID).
            if let Ok(command_bytes) = serde_json::to_vec(&IpcCommand::Status) {
                let _ = stream.write_all(&command_bytes);
                let _ = stream.write_all(b"\n");
                let _ = stream.flush();

                let mut reader = std::io::BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).is_ok() {
                    if let Ok(IpcResponse::Status { pid, .. }) = serde_json::from_str(line.trim()) {
                        return Some(pid);
                    }
                    // Received some response but not a Status payload;
                    // treat as running with unknown PID.
                    return Some(0);
                }
            }

            // Connected to socket but couldn't complete a query — assume
            // the daemon is running (PID unknown).
            return Some(0);
        }

        // Socket exists but can't connect — stale files
        cleanup_stale_files(paths);
        return None;
    }

    // Fallback: read PID file and verify liveness.
    let pid = read_pid_file(&paths.pid_file)?;

    // Verify the process is actually alive
    if !is_process_alive(pid) {
        cleanup_stale_files(paths);
        return None;
=======
// ---------------------------------------------------------------------------
// Unix: full daemonization, IPC via Unix domain sockets, PID liveness
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use anyhow::{Context as _, anyhow};
    use std::time::Instant;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::watch;

    /// Check whether a daemon is already running by testing PID file liveness
    /// and socket connectivity.
    pub fn is_running(paths: &DaemonPaths) -> Option<u32> {
        let pid = read_pid_file(&paths.pid_file)?;

        if !is_process_alive(pid) {
            cleanup_stale_files(paths);
            return None;
        }

        if paths.socket.exists() {
            if let Ok(stream) = std::os::unix::net::UnixStream::connect(&paths.socket) {
                drop(stream);
                return Some(pid);
            }
            cleanup_stale_files(paths);
            return None;
        }

        Some(pid)
    }

    /// Daemonize the current process. Returns in the child; the parent prints
    /// a message and exits.
    pub fn daemonize(paths: &DaemonPaths) -> anyhow::Result<()> {
        std::fs::create_dir_all(&paths.log_dir).with_context(|| {
            format!(
                "failed to create log directory: {}",
                paths.log_dir.display()
            )
        })?;

        let stdout = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.log_dir.join("spacebot.out"))
            .context("failed to open stdout log")?;

        let stderr = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.log_dir.join("spacebot.err"))
            .context("failed to open stderr log")?;

        let daemonize = daemonize::Daemonize::new()
            .pid_file(&paths.pid_file)
            .chown_pid_file(true)
            .stdout(stdout)
            .stderr(stderr);

        daemonize
            .start()
            .map_err(|error| anyhow!("failed to daemonize: {error}"))?;

        Ok(())
    }

    /// Start the IPC server. Returns a shutdown receiver that the main event
    /// loop should select on.
    pub async fn start_ipc_server(
        paths: &DaemonPaths,
    ) -> anyhow::Result<(watch::Receiver<bool>, tokio::task::JoinHandle<()>)> {
        if let Some(parent) = paths.socket.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create instance directory: {}", parent.display())
            })?;
        }

        if paths.socket.exists() {
            std::fs::remove_file(&paths.socket).with_context(|| {
                format!("failed to remove stale socket: {}", paths.socket.display())
            })?;
        }

        let listener = UnixListener::bind(&paths.socket)
            .with_context(|| format!("failed to bind IPC socket: {}", paths.socket.display()))?;

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let start_time = Instant::now();
        let socket_path = paths.socket.clone();

        let handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _address)) => {
                        let shutdown_tx = shutdown_tx.clone();
                        let uptime = start_time.elapsed();
                        tokio::spawn(async move {
                            if let Err(error) =
                                handle_ipc_connection(stream, &shutdown_tx, uptime).await
                            {
                                tracing::warn!(%error, "IPC connection handler failed");
                            }
                        });
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to accept IPC connection");
                    }
                }
            }
        });

        let cleanup_socket = socket_path.clone();
        let mut cleanup_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let _ = cleanup_rx.wait_for(|shutdown| *shutdown).await;
            let _ = std::fs::remove_file(&cleanup_socket);
        });

        Ok((shutdown_rx, handle))
    }

    async fn handle_ipc_connection(
        stream: UnixStream,
        shutdown_tx: &watch::Sender<bool>,
        uptime: std::time::Duration,
    ) -> anyhow::Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut reader = tokio::io::BufReader::new(reader);
        let mut line = String::new();
        reader.read_line(&mut line).await?;

        let command: IpcCommand = serde_json::from_str(line.trim())
            .with_context(|| format!("invalid IPC command: {line}"))?;

        let response = match command {
            IpcCommand::Shutdown => {
                tracing::info!("shutdown requested via IPC");
                shutdown_tx.send(true).ok();
                IpcResponse::Ok
            }
            IpcCommand::Status => IpcResponse::Status {
                pid: std::process::id(),
                uptime_seconds: uptime.as_secs(),
            },
        };

        let mut response_bytes = serde_json::to_vec(&response)?;
        response_bytes.push(b'\n');
        writer.write_all(&response_bytes).await?;
        writer.flush().await?;

        Ok(())
>>>>>>> theirs
    }

    /// Send a command to the running daemon and return the response.
    pub async fn send_command(
        paths: &DaemonPaths,
        command: IpcCommand,
    ) -> anyhow::Result<IpcResponse> {
        let stream = UnixStream::connect(&paths.socket)
            .await
            .with_context(|| "failed to connect to spacebot daemon. is it running?")?;

        let (reader, mut writer) = stream.into_split();

        let mut command_bytes = serde_json::to_vec(&command)?;
        command_bytes.push(b'\n');
        writer.write_all(&command_bytes).await?;
        writer.flush().await?;

        let mut reader = tokio::io::BufReader::new(reader);
        let mut line = String::new();
        reader.read_line(&mut line).await?;

        let response: IpcResponse = serde_json::from_str(line.trim())
            .with_context(|| format!("invalid IPC response: {line}"))?;

        Ok(response)
    }

    /// Clean up PID and socket files on shutdown.
    pub fn cleanup(paths: &DaemonPaths) {
        if let Err(error) = std::fs::remove_file(&paths.pid_file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "failed to remove PID file");
        }
        if let Err(error) = std::fs::remove_file(&paths.socket)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "failed to remove socket file");
        }
    }

    fn read_pid_file(path: &std::path::Path) -> Option<u32> {
        let content = std::fs::read_to_string(path).ok()?;
        content.trim().parse::<u32>().ok()
    }

    fn is_process_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    fn cleanup_stale_files(paths: &DaemonPaths) {
        let _ = std::fs::remove_file(&paths.pid_file);
        let _ = std::fs::remove_file(&paths.socket);
    }

    /// Wait for the daemon process to exit after sending a shutdown command.
    /// Polls the PID with a short interval, times out after 10 seconds.
    pub fn wait_for_exit(pid: u32) -> bool {
        for _ in 0..100 {
            if !is_process_alive(pid) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Windows: foreground-only with Ctrl+C shutdown.
// TODO(windows): implement named-pipe IPC for stop/restart/status commands
// TODO(windows): implement is_running via PID file + OpenProcess liveness check
// TODO(windows): consider Windows Service integration for background operation
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use tokio::sync::watch;

    /// TODO(windows): implement PID file + OpenProcess liveness check
    pub fn is_running(_paths: &DaemonPaths) -> Option<u32> {
        None
    }

    pub fn daemonize(_paths: &DaemonPaths) -> anyhow::Result<()> {
        anyhow::bail!("background daemon mode is not supported on Windows, use --foreground")
    }

    /// Ctrl+C signal handler providing the same shutdown interface as Unix IPC.
    /// TODO(windows): replace with named-pipe IPC server for CLI management
    pub async fn start_ipc_server(
        _paths: &DaemonPaths,
    ) -> anyhow::Result<(watch::Receiver<bool>, tokio::task::JoinHandle<()>)> {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(async move {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::warn!(%error, "failed to listen for Ctrl+C");
                return;
            }
            tracing::info!("shutdown requested via Ctrl+C");
            shutdown_tx.send(true).ok();
        });

        Ok((shutdown_rx, handle))
    }

    /// TODO(windows): implement via named-pipe client
    pub async fn send_command(
        _paths: &DaemonPaths,
        _command: IpcCommand,
    ) -> anyhow::Result<IpcResponse> {
        anyhow::bail!("IPC commands are not yet supported on Windows")
    }

    pub fn cleanup(_paths: &DaemonPaths) {}

    pub fn wait_for_exit(_pid: u32) -> bool {
        true
    }
}

// Re-export platform implementation at module level
#[cfg(unix)]
pub use unix_impl::*;
#[cfg(windows)]
pub use windows_impl::*;

// ---------------------------------------------------------------------------
// Cross-platform: tracing initialization and OTLP
// ---------------------------------------------------------------------------

/// Initialize tracing for background (daemon) mode.
///
/// Returns an `SdkTracerProvider` if OTLP export is configured. The caller must
/// hold onto it for the process lifetime and call `.shutdown()` before exit so
/// the batch exporter flushes buffered spans.
pub fn init_background_tracing(
    paths: &DaemonPaths,
    debug: bool,
    telemetry: &TelemetryConfig,
) -> Option<SdkTracerProvider> {
    let file_appender = tracing_appender::rolling::daily(&paths.log_dir, "spacebot.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    let field_formatter = format::debug_fn(|writer, field, value| {
        let field_name = field.name();

        if field_name == "gen_ai.system_instructions"
            || field_name == "gen_ai.tool.call.arguments"
            || field_name == "gen_ai.tool.call.result"
        {
            Ok(())
        } else if field_name == "message" {
            let formatted = format!("{value:?}");
            const MAX_MESSAGE_CHARS: usize = 280;
            let (truncated, was_truncated) = truncate_for_log(&formatted, MAX_MESSAGE_CHARS);
            if was_truncated {
                write!(writer, "{}={}...", field_name, truncated)
            } else {
                write!(writer, "{}={formatted}", field_name)
            }
        } else {
            write!(writer, "{}={value:?}", field_name)
        }
    });

    // Leak the guard so the non-blocking writer lives for the entire process.
    // The process owns this -- it's cleaned up on exit.
    std::mem::forget(_guard);

    let filter = build_env_filter(debug);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .fmt_fields(field_formatter)
        .compact();

    match build_otlp_provider(telemetry) {
        Some(provider) => {
            let tracer = provider.tracer("spacebot");
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .init();
            Some(provider)
        }
        None => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .init();
            None
        }
    }
}

/// Initialize tracing for foreground (terminal) mode.
///
/// Returns an `SdkTracerProvider` if OTLP export is configured.
pub fn init_foreground_tracing(
    debug: bool,
    telemetry: &TelemetryConfig,
) -> Option<SdkTracerProvider> {
    let field_formatter = format::debug_fn(|writer, field, value| {
        let field_name = field.name();

        if field_name == "gen_ai.system_instructions"
            || field_name == "gen_ai.tool.call.arguments"
            || field_name == "gen_ai.tool.call.result"
        {
            Ok(())
        } else if field_name == "message" {
            let formatted = format!("{value:?}");
            const MAX_MESSAGE_CHARS: usize = 280;
            let (truncated, was_truncated) = truncate_for_log(&formatted, MAX_MESSAGE_CHARS);
            if was_truncated {
                write!(writer, "{}={}", field_name, truncated)?;
                write!(writer, "...")?;
            } else {
                write!(writer, "{}={formatted}", field_name)?;
            }
            Ok(())
        } else {
            write!(writer, "{}={value:?}", field_name)
        }
    });
    let filter = build_env_filter(debug);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .fmt_fields(field_formatter)
        .compact();

    match build_otlp_provider(telemetry) {
        Some(provider) => {
            let tracer = provider.tracer("spacebot");
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .with(tracing_opentelemetry::layer().with_tracer(tracer))
                .init();
            Some(provider)
        }
        None => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt_layer)
                .init();
            None
        }
    }
}

fn build_env_filter(debug: bool) -> tracing_subscriber::EnvFilter {
    if debug {
        tracing_subscriber::EnvFilter::new("debug")
    } else {
        tracing_subscriber::EnvFilter::new("info")
    }
}

/// Build an OTLP `SdkTracerProvider` when an endpoint is configured.
///
/// Returns `None` if neither the config field nor the `OTEL_EXPORTER_OTLP_ENDPOINT`
/// environment variable is set, allowing the OTel layer to be omitted entirely.
fn build_otlp_provider(telemetry: &TelemetryConfig) -> Option<SdkTracerProvider> {
    use opentelemetry_otlp::WithExportConfig as _;

    let endpoint = telemetry.otlp_endpoint.as_deref()?;

    let endpoint = if endpoint.ends_with("/v1/traces") {
        endpoint.to_owned()
    } else {
        format!("{}/v1/traces", endpoint.trim_end_matches('/'))
    };

    let mut exporter_builder = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint);
    if !telemetry.otlp_headers.is_empty() {
        exporter_builder = exporter_builder.with_headers(telemetry.otlp_headers.clone());
    }
    let exporter = exporter_builder
        .build()
        .map_err(|error| eprintln!("failed to build OTLP exporter: {error}"))
        .ok()?;

    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name(telemetry.service_name.clone())
        .build();

    let sampler: opentelemetry_sdk::trace::Sampler =
        if (telemetry.sample_rate - 1.0).abs() < f64::EPSILON {
            opentelemetry_sdk::trace::Sampler::AlwaysOn
        } else {
            opentelemetry_sdk::trace::Sampler::ParentBased(Box::new(
                opentelemetry_sdk::trace::Sampler::TraceIdRatioBased(telemetry.sample_rate),
            ))
        };

    let batch_processor =
        opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor::builder(
            exporter,
            opentelemetry_sdk::runtime::Tokio,
        )
        .build();

    let provider = SdkTracerProvider::builder()
        .with_span_processor(batch_processor)
        .with_resource(resource)
        .with_sampler(sampler)
        .build();

    Some(provider)
}

<<<<<<< ours
/// Start the IPC server. Lifecycle commands are forwarded to the provided
/// handle; the main event loop selects on its watch channel.
pub async fn start_ipc_server(
    paths: &DaemonPaths,
    lifecycle: crate::lifecycle::LifecycleHandle,
    run_id: String,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    // Ensure the instance directory exists (e.g. on first run)
    if let Some(parent) = paths.socket.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create instance directory: {}", parent.display())
        })?;
    }

    // Clean up any stale socket file
    if paths.socket.exists() {
        std::fs::remove_file(&paths.socket).with_context(|| {
            format!("failed to remove stale socket: {}", paths.socket.display())
        })?;
    }

    let listener = UnixListener::bind(&paths.socket)
        .with_context(|| format!("failed to bind IPC socket: {}", paths.socket.display()))?;

    let start_time = Instant::now();
    let socket_path = paths.socket.clone();

    let accept_lifecycle = lifecycle.clone();
    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _address)) => {
                    let lifecycle = accept_lifecycle.clone();
                    let uptime = start_time.elapsed();
                    let run_id = run_id.clone();
                    tokio::spawn(async move {
                        if let Err(error) =
                            handle_ipc_connection(stream, &lifecycle, uptime, &run_id).await
                        {
                            tracing::warn!(%error, "IPC connection handler failed");
                        }
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to accept IPC connection");
                }
            }
        }
    });

    // Spawn a cleanup task that removes the socket file when the server shuts down
    let cleanup_socket = socket_path.clone();
    let mut cleanup_rx = lifecycle.subscribe();
    tokio::spawn(async move {
        if cleanup_rx
            .wait_for(|state| *state != crate::lifecycle::LifecycleState::Running)
            .await
            .is_err()
        {
            tracing::debug!("lifecycle sender dropped before socket cleanup");
        }
        if let Err(error) = std::fs::remove_file(&cleanup_socket)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(
                %error,
                path = %cleanup_socket.display(),
                "failed to remove IPC socket"
            );
        }
    });

    Ok(handle)
}

/// Handle a single IPC client connection.
async fn handle_ipc_connection(
    stream: UnixStream,
    lifecycle: &crate::lifecycle::LifecycleHandle,
    uptime: std::time::Duration,
    run_id: &str,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let command: IpcCommand = serde_json::from_str(line.trim())
        .with_context(|| format!("invalid IPC command: {line}"))?;

    // Lifecycle requests only arm a delayed sender that fires after
    // SHUTDOWN_GRACE, so requesting the transition first still leaves the
    // whole grace window to flush the acknowledgement before teardown begins.
    let response = match command {
        IpcCommand::Shutdown => {
            if lifecycle.request_shutdown("ipc") {
                IpcResponse::Ok
            } else {
                IpcResponse::Error {
                    message: "a shutdown is already pending".to_string(),
                }
            }
        }
        IpcCommand::Restart => {
            if lifecycle.request_restart("ipc") {
                IpcResponse::Ok
            } else {
                IpcResponse::Error {
                    message: "a shutdown or restart is already pending".to_string(),
                }
            }
        }
        IpcCommand::Status => IpcResponse::Status {
            pid: std::process::id(),
            uptime_seconds: uptime.as_secs(),
            run_id: Some(run_id.to_string()),
        },
    };

    let mut response_bytes = serde_json::to_vec(&response)?;
    response_bytes.push(b'\n');
    writer.write_all(&response_bytes).await?;
    writer.flush().await?;

    Ok(())
}

/// Send a command to the running daemon and return the response.
pub async fn send_command(paths: &DaemonPaths, command: IpcCommand) -> anyhow::Result<IpcResponse> {
    let stream = UnixStream::connect(&paths.socket)
        .await
        .with_context(|| "failed to connect to spacebot daemon. is it running?")?;

    let (reader, mut writer) = stream.into_split();

    let mut command_bytes = serde_json::to_vec(&command)?;
    command_bytes.push(b'\n');
    writer.write_all(&command_bytes).await?;
    writer.flush().await?;

    let mut reader = tokio::io::BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response: IpcResponse = serde_json::from_str(line.trim())
        .with_context(|| format!("invalid IPC response: {line}"))?;

    Ok(response)
}

/// Clean up PID and socket files on shutdown.
pub fn cleanup(paths: &DaemonPaths) {
    if let Err(error) = std::fs::remove_file(&paths.pid_file)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, "failed to remove PID file");
    }
    if let Err(error) = std::fs::remove_file(&paths.socket)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, "failed to remove socket file");
    }
}

fn read_pid_file(path: &std::path::Path) -> Option<u32> {
    let content = std::fs::read_to_string(path).ok()?;
    content.trim().parse::<u32>().ok()
}

fn is_process_alive(pid: u32) -> bool {
    // kill(pid, 0) checks if the process exists without sending a signal
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn cleanup_stale_files(paths: &DaemonPaths) {
    let _ = std::fs::remove_file(&paths.pid_file);
    let _ = std::fs::remove_file(&paths.socket);
}

/// Wait for the daemon process to exit after sending a shutdown command.
/// Polls the PID with a short interval, times out after 10 seconds.
pub fn wait_for_exit(pid: u32) -> bool {
    for _ in 0..100 {
        if !is_process_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

=======
>>>>>>> theirs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_for_log_handles_multibyte_characters() {
        let message = "abc\u{2192}def";
        let (truncated, was_truncated) = truncate_for_log(message, 4);

        assert!(was_truncated);
        assert_eq!(truncated, "abc\u{2192}");
    }

    #[test]
    fn truncate_for_log_returns_original_when_within_limit() {
        let message = "hello";
        let (truncated, was_truncated) = truncate_for_log(message, 10);

        assert!(!was_truncated);
        assert_eq!(truncated, "hello");
    }

    #[test]
    fn restart_command_roundtrips() {
        let json = serde_json::to_string(&IpcCommand::Restart).unwrap();
        assert_eq!(json, r#"{"command":"restart"}"#);
        assert!(matches!(
            serde_json::from_str(&json).unwrap(),
            IpcCommand::Restart
        ));
    }

    #[test]
    fn status_response_tolerates_missing_run_id() {
        // A daemon predating the run_id field replies without it.
        let legacy = r#"{"result":"status","pid":42,"uptime_seconds":7}"#;
        let response: IpcResponse = serde_json::from_str(legacy).unwrap();
        assert!(matches!(
            response,
            IpcResponse::Status {
                pid: 42,
                run_id: None,
                ..
            }
        ));
    }

    #[test]
    fn status_response_carries_run_id() {
        let response = IpcResponse::Status {
            pid: 1,
            uptime_seconds: 0,
            run_id: Some("abc".into()),
        };
        let json = serde_json::to_string(&response).unwrap();
        let parsed: IpcResponse = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            IpcResponse::Status { run_id: Some(id), .. } if id == "abc"
        ));
    }

    async fn send_ipc(
        lifecycle: &crate::lifecycle::LifecycleHandle,
        command: IpcCommand,
    ) -> IpcResponse {
        let (client, server) = UnixStream::pair().unwrap();
        let lifecycle = lifecycle.clone();
        let server_task = tokio::spawn(async move {
            handle_ipc_connection(server, &lifecycle, std::time::Duration::ZERO, "test-run").await
        });

        let (reader, mut writer) = client.into_split();
        let mut command_bytes = serde_json::to_vec(&command).unwrap();
        command_bytes.push(b'\n');
        writer.write_all(&command_bytes).await.unwrap();
        writer.flush().await.unwrap();

        let mut reader = tokio::io::BufReader::new(reader);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        server_task.await.unwrap().unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn ipc_shutdown_acknowledges_and_fires_lifecycle() {
        let (lifecycle, mut lifecycle_rx) = crate::lifecycle::LifecycleHandle::new();

        let response = send_ipc(&lifecycle, IpcCommand::Shutdown).await;
        assert!(matches!(response, IpcResponse::Ok));

        lifecycle_rx
            .wait_for(|state| *state != crate::lifecycle::LifecycleState::Running)
            .await
            .unwrap();
        assert_eq!(
            *lifecycle_rx.borrow(),
            crate::lifecycle::LifecycleState::Exit
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ipc_rejected_restart_is_reported_to_client() {
        let (lifecycle, _lifecycle_rx) = crate::lifecycle::LifecycleHandle::new();
        assert!(lifecycle.request_shutdown("test"));

        // A restart cannot supersede a pending shutdown; the client must see
        // the rejection instead of a blind Ok.
        let response = send_ipc(&lifecycle, IpcCommand::Restart).await;
        assert!(matches!(response, IpcResponse::Error { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn ipc_duplicate_shutdown_is_rejected() {
        let (lifecycle, _lifecycle_rx) = crate::lifecycle::LifecycleHandle::new();

        assert!(matches!(
            send_ipc(&lifecycle, IpcCommand::Shutdown).await,
            IpcResponse::Ok
        ));
        assert!(matches!(
            send_ipc(&lifecycle, IpcCommand::Shutdown).await,
            IpcResponse::Error { .. }
        ));
    }
}
