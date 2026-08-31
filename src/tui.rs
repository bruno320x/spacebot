//! Pure, dependency-free state mapping for the terminal UI (`spacebot tui`).
//!
//! The TUI renders a live snapshot of the daemon: channels, workers and
//! processes. All transformation from raw control-API JSON into display rows
//! lives here so it can be unit-tested in isolation and reused by the ratatui
//! shell in `src/bin/tui.rs` (which only does I/O and rendering).
//!
//! Parsers are deliberately tolerant: unknown fields are ignored and missing
//! fields fall back to defaults, so a shape drift in the API degrades to an
//! empty-ish row instead of a hard parse failure.

use serde_json::Value;

/// Terminal statuses a worker row can be in. Anything not in this set is
/// treated as "still running" for the live count.
const TERMINAL_STATUSES: &[&str] = &["done", "completed", "failed", "error", "cancelled"];

/// A single channel, normalized for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRow {
    pub agent_id: String,
    pub id: String,
    pub platform: String,
    pub display_name: String,
    pub is_active: bool,
    pub last_activity_at: String,
    pub response_mode: Option<String>,
    pub model: Option<String>,
}

/// A single worker, normalized for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRow {
    pub id: String,
    pub task: String,
    pub status: String,
    pub kind: String,
    pub live_status: Option<String>,
    pub started_at: String,
    pub tool_calls: i64,
    pub interactive: bool,
}

/// A whole live snapshot for one render frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub channels: Vec<ChannelRow>,
    pub workers: Vec<WorkerRow>,
    pub process_total: i64,
}

impl Snapshot {
    pub fn active_channel_count(&self) -> usize {
        self.channels.iter().filter(|row| row.is_active).count()
    }

    pub fn running_worker_count(&self) -> usize {
        self.workers
            .iter()
            .filter(|row| !is_terminal(&row.status))
            .count()
    }
}

/// True when a worker status means the worker no longer does work.
pub fn is_terminal(status: &str) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

fn as_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn opt_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn as_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn as_i64(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// Map a `GET /channels` response body into channel rows.
pub fn parse_channels(body: &Value) -> Vec<ChannelRow> {
    body.get("channels")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| ChannelRow {
                    agent_id: as_str(item, "agent_id"),
                    id: as_str(item, "id"),
                    platform: as_str(item, "platform"),
                    display_name: as_str(item, "display_name"),
                    is_active: as_bool(item, "is_active"),
                    last_activity_at: as_str(item, "last_activity_at"),
                    response_mode: opt_str(item, "response_mode"),
                    model: opt_str(item, "model"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Map a `GET /agents/workers` response body into worker rows.
pub fn parse_workers(body: &Value) -> Vec<WorkerRow> {
    body.get("workers")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| WorkerRow {
                    id: as_str(item, "id"),
                    task: as_str(item, "task"),
                    status: as_str(item, "status"),
                    kind: opt_str(item, "worker_type")
                        .or_else(|| opt_str(item, "kind"))
                        .unwrap_or_default(),
                    live_status: opt_str(item, "live_status"),
                    started_at: as_str(item, "started_at"),
                    tool_calls: as_i64(item, "tool_calls"),
                    interactive: as_bool(item, "interactive"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Read the `total` field from a `GET /agents/processes` response body.
pub fn process_total(body: &Value) -> i64 {
    as_i64(body, "total")
}

/// Map a whole poll round (channels + workers + processes) into a snapshot.
pub fn build_snapshot(channels: &Value, workers: &Value, processes: &Value) -> Snapshot {
    Snapshot {
        channels: parse_channels(channels),
        workers: parse_workers(workers),
        process_total: process_total(processes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn channel(active: bool) -> Value {
        json!({
            "agent_id": "main",
            "id": "telegram:1",
            "platform": "telegram",
            "display_name": "DMs",
            "is_active": active,
            "last_activity_at": "2026-08-31T10:00:00Z",
            "response_mode": "observe",
            "model": "gpt-4o",
        })
    }

    fn worker(status: &str, calls: i64) -> Value {
        json!({
            "id": "w1",
            "task": "fix the bug",
            "status": status,
            "worker_type": "rig",
            "live_status": "compiling",
            "started_at": "2026-08-31T09:00:00Z",
            "tool_calls": calls,
            "interactive": true,
        })
    }

    #[test]
    fn parse_channels_maps_fields() {
        let body = json!({ "channels": [channel(true), channel(false)] });
        let rows = parse_channels(&body);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "telegram:1");
        assert_eq!(rows[0].agent_id, "main");
        assert_eq!(rows[0].platform, "telegram");
        assert_eq!(rows[0].display_name, "DMs");
        assert!(rows[0].is_active);
        assert_eq!(rows[0].response_mode.as_deref(), Some("observe"));
        assert_eq!(rows[0].model.as_deref(), Some("gpt-4o"));
        assert_eq!(rows[0].last_activity_at, "2026-08-31T10:00:00Z");
    }

    #[test]
    fn parse_channels_tolerates_missing_and_unknown_fields() {
        let body = json!({ "channels": [ { "id": "x" } ], "extra": "ignored" });
        let rows = parse_channels(&body);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "x");
        assert_eq!(rows[0].agent_id, "");
        assert_eq!(rows[0].platform, "");
        assert!(!rows[0].is_active);
        assert_eq!(rows[0].response_mode, None);
        assert_eq!(rows[0].model, None);
    }

    #[test]
    fn parse_channels_handles_missing_array() {
        assert!(parse_channels(&json!({})).is_empty());
        assert!(parse_channels(&json!({ "channels": "not-an-array" })).is_empty());
    }

    #[test]
    fn parse_workers_maps_fields() {
        let done = json!({ "workers": [worker("done", 12)] });
        let rows = parse_workers(&done);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "w1");
        assert_eq!(rows[0].task, "fix the bug");
        assert_eq!(rows[0].status, "done");
        assert_eq!(rows[0].kind, "rig");
        assert_eq!(rows[0].live_status.as_deref(), Some("compiling"));
        assert_eq!(rows[0].tool_calls, 12);
        assert!(rows[0].interactive);
    }

    #[test]
    fn worker_kind_falls_back_to_kind_key() {
        let body = json!({ "workers": [ { "id": "p", "kind": "branch", "status": "running" } ] });
        let rows = parse_workers(&body);
        assert_eq!(rows[0].kind, "branch");
    }

    #[test]
    fn terminal_statuses_are_classified() {
        assert!(is_terminal("done"));
        assert!(is_terminal("failed"));
        assert!(is_terminal("cancelled"));
        assert!(!is_terminal("running"));
        assert!(!is_terminal("in_progress"));
        assert!(!is_terminal(""));
    }

    #[test]
    fn snapshot_counts_active_channels_and_running_workers() {
        let body = json!({ "channels": [channel(true), channel(false), channel(true)] });
        let workers =
            json!({ "workers": [worker("running", 1), worker("done", 2), worker("failed", 3)] });
        let snapshot = build_snapshot(&body, &workers, &json!({ "total": 7 }));
        assert_eq!(snapshot.active_channel_count(), 2);
        assert_eq!(snapshot.running_worker_count(), 1);
        assert_eq!(snapshot.process_total, 7);
    }
}
