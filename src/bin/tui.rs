//! `spacebot tui` — a terminal dashboard over the daemon control API.
//!
//! This binary is deliberately thin: it renders the normalized model produced
//! by `spacebot::tui` (a pure, unit-tested state-mapping module) using only
//! `crossterm`. All shape parsing and state mapping lives in the lib; this
//! file only does API polling and simple line rendering.
//!
//! Keys: `q`/Esc quits, `r` forces a refresh.

use anyhow::Context as _;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};
use spacebot::config::Config;
use spacebot::tui::{ChannelRow, Snapshot, WorkerRow, build_snapshot, is_terminal};
use std::io::{self, Write, stdout};
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let config = Config::load().context("failed to load configuration")?;
    let base = format!("http://{}:{}", config.api.bind, config.api.port);
    let token = config.api.auth_token.clone();

    let client = reqwest::Client::new();

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, Hide)?;

    let run_result = {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to build tokio runtime")?;
        runtime.block_on(run_loop(&mut out, &client, &base, token.as_deref()))
    };

    execute!(out, Show, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    run_result?;
    Ok(())
}

async fn run_loop<W: Write>(
    out: &mut W,
    client: &reqwest::Client,
    base: &str,
    token: Option<&str>,
) -> anyhow::Result<()> {
    let mut snapshot = Snapshot::default();
    let mut height = 24usize;

    loop {
        if event::poll(Duration::from_millis(400))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('r') => snapshot = fetch_snapshot(client, base, token).await,
                    _ => {}
                }
            }
            if event::poll(Duration::ZERO)? {
                if let Event::Resize(rows, _cols) = event::read()? {
                    height = rows as usize;
                }
            }
        }

        draw(out, &snapshot, height)?;
    }
}

async fn fetch_snapshot(client: &reqwest::Client, base: &str, token: Option<&str>) -> Snapshot {
    let channels = fetch_json(client, base, token, "channels")
        .await
        .unwrap_or_default();
    let agent = channels
        .get("channels")
        .and_then(|c| c.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.get("agent_id"))
        .and_then(|id| id.as_str())
        .unwrap_or("main");
    let workers = fetch_json(
        client,
        base,
        token,
        &format!("agents/workers?agent_id={}", encode(agent)),
    )
    .await
    .unwrap_or_default();
    let processes = fetch_json(
        client,
        base,
        token,
        &format!("agents/processes?agent_id={}", encode(agent)),
    )
    .await
    .unwrap_or_default();
    build_snapshot(&channels, &workers, &processes)
}

async fn fetch_json(
    client: &reqwest::Client,
    base: &str,
    token: Option<&str>,
    path: &str,
) -> anyhow::Result<serde_json::Value> {
    let mut request = client.get(format!("{}/{}", base.trim_end_matches('/'), path));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.context("control API request failed")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read response body")?;
    if !status.is_success() {
        anyhow::bail!("GET /{path} -> {status}: {body}");
    }
    Ok(serde_json::from_str(&body).context("failed to parse response")?)
}

fn encode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

fn draw<W: Write>(out: &mut W, snapshot: &Snapshot, height: usize) -> io::Result<()> {
    let header = format!(
        "spacebot  |  {} channels ({} active)  |  {} workers ({} running)  |  {} processes",
        snapshot.channels.len(),
        snapshot.active_channel_count(),
        snapshot.workers.len(),
        snapshot.running_worker_count(),
        snapshot.process_total,
    );

    let mut lines: Vec<String> = Vec::new();
    lines.push(header);
    lines.push("── channels ─────────────────────────────────────────────".to_string());
    if snapshot.channels.is_empty() {
        lines.push("  (no channels)".to_string());
    }
    for row in &snapshot.channels {
        lines.push(format_channel(row));
    }
    lines.push("── workers ──────────────────────────────────────────────".to_string());
    if snapshot.workers.is_empty() {
        lines.push("  (no workers)".to_string());
    }
    for row in &snapshot.workers {
        lines.push(format_worker(row));
    }
    lines.push("q/Esc quit · r refresh".to_string());

    queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
    for (index, line) in lines.into_iter().take(height).enumerate() {
        queue!(out, MoveTo(0, index as u16))?;
        write!(out, "{line}")?;
    }
    out.flush()
}

fn format_channel(row: &ChannelRow) -> String {
    let dot = if row.is_active { '●' } else { '○' };
    let mode = row.response_mode.as_deref().unwrap_or("active");
    let model = row.model.as_deref().unwrap_or("-");
    format!(
        " {dot} {:<24} [{:10}] {mode} · {model}",
        shorten(&row.display_name, 24),
        row.platform
    )
}

fn format_worker(row: &WorkerRow) -> String {
    let marker = if is_terminal(&row.status) {
        "✓"
    } else {
        "▶"
    };
    let live = row
        .live_status
        .as_deref()
        .map(|s| format!(" — {s}"))
        .unwrap_or_default();
    format!(
        " {marker} {:<24} [{:14}] {} calls{live}",
        shorten(&row.task, 24),
        row.status,
        row.tool_calls
    )
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}
