//! Wake definitions: the persisted registry of triggers and their run
//! instructions.
//!
//! A wake definition names a condition under which the agent stirs (a
//! schedule, a webhook, or an internal event) and the instructions rendered
//! into the run that consumes its events. Built-in wakes are seeded in code,
//! `[[agents.X.wakes]]` entries are reconciled from config, and user-created
//! wakes arrive through the API. The database is the source of truth.

use crate::config::AutonomyLevel;
use crate::error::Result;
use crate::wakes::SystemEvent;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row as _, SqlitePool};

/// Id of the built-in wake that pulls the next autonomy run forward when a
/// task is approved.
pub const TASK_APPROVED_WAKE_ID: &str = "task-approved";

/// Id of the built-in wake that verifies the project after a worker
/// completes and proposes a fix-worker task when the suite fails.
pub const VERIFY_AFTER_WORK_WAKE_ID: &str = "verify-after-work";

/// What causes a wake to fire. Persisted as a `trigger_kind` discriminant
/// plus a `trigger_spec` JSON object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WakeTrigger {
    Schedule {
        /// Standard 5-field cron expression. Takes precedence over
        /// `interval_secs` when both are set.
        cron_expr: Option<String>,
        interval_secs: Option<u64>,
    },
    Webhook,
    Event {
        event: SystemEvent,
    },
}

impl WakeTrigger {
    pub fn kind(&self) -> &'static str {
        match self {
            WakeTrigger::Schedule { .. } => "schedule",
            WakeTrigger::Webhook => "webhook",
            WakeTrigger::Event { .. } => "event",
        }
    }

    /// The `trigger_spec` JSON persisted alongside the kind discriminant.
    pub fn spec(&self) -> Value {
        match self {
            WakeTrigger::Schedule {
                cron_expr,
                interval_secs,
            } => serde_json::json!({
                "cron_expr": cron_expr,
                "interval_secs": interval_secs,
            }),
            WakeTrigger::Webhook => serde_json::json!({}),
            WakeTrigger::Event { event } => serde_json::json!({ "event": event.as_str() }),
        }
    }

    pub fn from_parts(kind: &str, spec: &Value) -> Result<Self> {
        match kind {
            "schedule" => Ok(WakeTrigger::Schedule {
                cron_expr: spec
                    .get("cron_expr")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                interval_secs: spec.get("interval_secs").and_then(Value::as_u64),
            }),
            "webhook" => Ok(WakeTrigger::Webhook),
            "event" => {
                let name = spec
                    .get("event")
                    .and_then(Value::as_str)
                    .context("event trigger spec is missing the event name")?;
                let event = SystemEvent::parse(name)
                    .with_context(|| format!("unknown system event '{name}' in trigger spec"))?;
                Ok(WakeTrigger::Event { event })
            }
            other => Err(anyhow::anyhow!("unknown wake trigger kind '{other}'").into()),
        }
    }
}

/// A wake definition row. See `docs/design-docs/wakes.md` for the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WakeDef {
    pub id: String,
    pub name: String,
    pub trigger: WakeTrigger,
    /// Instructions rendered into the run context when this wake contributes
    /// to a run.
    pub instructions: String,
    /// Minimum autonomy level at which this wake's instructions apply. Events
    /// from a wake above the current level still persist and are surfaced as
    /// observations.
    pub min_level: AutonomyLevel,
    pub enabled: bool,
    /// Seeded in code; can be tuned or disabled but not deleted.
    pub builtin: bool,
    /// Owned by a `[[agents.X.wakes]]` config entry and reconciled on load.
    pub config_owned: bool,
    /// Delivery target in "adapter:target" format for notify-style wakes.
    pub delivery_target: Option<String>,
    /// Bearer token authorizing webhook fires for this wake.
    pub webhook_token: Option<String>,
    /// (start_hour, end_hour) window outside of which the wake does not fire.
    pub active_hours: Option<(u8, u8)>,
    /// Schedule cursor: next scheduled fire, claimed via CAS.
    pub next_run_at: Option<String>,
    pub last_fired_at: Option<String>,
    /// Persisted so restarts do not reset progress toward the circuit breaker.
    pub consecutive_failures: i64,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Store over the per-agent `wake_defs` table.
#[derive(Debug, Clone)]
pub struct WakeDefStore {
    pool: SqlitePool,
}

impl WakeDefStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert or update a definition. Cursor columns (`next_run_at`,
    /// `last_fired_at`, `consecutive_failures`) and `created_at` survive
    /// updates so redefining a wake does not reset its schedule state. A
    /// stored webhook token likewise survives an upsert carrying `None`, so
    /// config reconciliation cannot invalidate published webhook URLs.
    pub async fn upsert(&self, def: &WakeDef) -> Result<()> {
        sqlx::query(
            "INSERT INTO wake_defs (id, name, trigger_kind, trigger_spec, instructions, \
                 min_level, enabled, builtin, config_owned, delivery_target, webhook_token, \
                 active_hours_start, active_hours_end, created_by) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
                 name = excluded.name, \
                 trigger_kind = excluded.trigger_kind, \
                 trigger_spec = excluded.trigger_spec, \
                 instructions = excluded.instructions, \
                 min_level = excluded.min_level, \
                 enabled = excluded.enabled, \
                 builtin = excluded.builtin, \
                 config_owned = excluded.config_owned, \
                 delivery_target = excluded.delivery_target, \
                 webhook_token = COALESCE(excluded.webhook_token, wake_defs.webhook_token), \
                 active_hours_start = excluded.active_hours_start, \
                 active_hours_end = excluded.active_hours_end, \
                 created_by = excluded.created_by, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        )
        .bind(&def.id)
        .bind(&def.name)
        .bind(def.trigger.kind())
        .bind(def.trigger.spec().to_string())
        .bind(&def.instructions)
        .bind(def.min_level.as_str())
        .bind(def.enabled)
        .bind(def.builtin)
        .bind(def.config_owned)
        .bind(def.delivery_target.as_deref())
        .bind(def.webhook_token.as_deref())
        .bind(def.active_hours.map(|(start, _)| start as i64))
        .bind(def.active_hours.map(|(_, end)| end as i64))
        .bind(&def.created_by)
        .execute(&self.pool)
        .await
        .context("failed to upsert wake def")?;

        Ok(())
    }

    /// Insert only when no row with this id exists; never overwrites.
    /// Returns whether a row was inserted.
    pub async fn insert_if_absent(&self, def: &WakeDef) -> Result<bool> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO wake_defs (id, name, trigger_kind, trigger_spec, \
                 instructions, min_level, enabled, builtin, config_owned, delivery_target, \
                 webhook_token, active_hours_start, active_hours_end, created_by) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&def.id)
        .bind(&def.name)
        .bind(def.trigger.kind())
        .bind(def.trigger.spec().to_string())
        .bind(&def.instructions)
        .bind(def.min_level.as_str())
        .bind(def.enabled)
        .bind(def.builtin)
        .bind(def.config_owned)
        .bind(def.delivery_target.as_deref())
        .bind(def.webhook_token.as_deref())
        .bind(def.active_hours.map(|(start, _)| start as i64))
        .bind(def.active_hours.map(|(_, end)| end as i64))
        .bind(&def.created_by)
        .execute(&self.pool)
        .await
        .context("failed to insert wake def")?;

        Ok(result.rows_affected() == 1)
    }

    pub async fn get(&self, id: &str) -> Result<Option<WakeDef>> {
        let row = sqlx::query(&format!(
            "SELECT {WAKE_DEF_COLUMNS} FROM wake_defs WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .context("failed to load wake def")?;

        row.map(def_from_row).transpose()
    }

    pub async fn list(&self) -> Result<Vec<WakeDef>> {
        let rows = sqlx::query(&format!(
            "SELECT {WAKE_DEF_COLUMNS} FROM wake_defs ORDER BY id ASC"
        ))
        .fetch_all(&self.pool)
        .await
        .context("failed to list wake defs")?;

        rows.into_iter().map(def_from_row).collect()
    }

    /// Returns whether a row was deleted.
    pub async fn delete(&self, id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM wake_defs WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .context("failed to delete wake def")?;

        Ok(result.rows_affected() == 1)
    }

    /// Returns whether a row was updated.
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE wake_defs SET enabled = ?, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?",
        )
        .bind(enabled)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("failed to set wake def enabled")?;

        Ok(result.rows_affected() == 1)
    }

    /// Enabled event-trigger definitions subscribed to `event`.
    pub async fn event_subscribers(&self, event: SystemEvent) -> Result<Vec<WakeDef>> {
        let rows = sqlx::query(&format!(
            "SELECT {WAKE_DEF_COLUMNS} FROM wake_defs \
             WHERE enabled = 1 AND trigger_kind = 'event' \
               AND json_extract(trigger_spec, '$.event') = ? \
             ORDER BY id ASC"
        ))
        .bind(event.as_str())
        .fetch_all(&self.pool)
        .await
        .context("failed to load wake event subscribers")?;

        rows.into_iter().map(def_from_row).collect()
    }

    /// Enabled schedule-trigger definitions due at `now`. A definition with
    /// no cursor yet is due — the producer initializes the cursor on first
    /// claim.
    pub async fn due_schedule_wakes(&self, now: &str) -> Result<Vec<WakeDef>> {
        let rows = sqlx::query(&format!(
            "SELECT {WAKE_DEF_COLUMNS} FROM wake_defs \
             WHERE enabled = 1 AND trigger_kind = 'schedule' \
               AND (next_run_at IS NULL OR next_run_at <= ?) \
             ORDER BY id ASC"
        ))
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .context("failed to load due schedule wakes")?;

        rows.into_iter().map(def_from_row).collect()
    }

    /// Atomically claim a scheduled fire and advance the cursor, the same
    /// CAS idiom as cron's `claim_and_advance`. Returns whether this caller
    /// won the claim.
    pub async fn claim_schedule_fire(
        &self,
        id: &str,
        expected_next_run_at: Option<&str>,
        new_next_run_at: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE wake_defs SET next_run_at = ?, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ? AND enabled = 1 AND next_run_at IS ?",
        )
        .bind(new_next_run_at)
        .bind(id)
        .bind(expected_next_run_at)
        .execute(&self.pool)
        .await
        .context("failed to claim schedule wake fire")?;

        Ok(result.rows_affected() == 1)
    }

    pub async fn touch_last_fired(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE wake_defs SET \
                 last_fired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .context("failed to touch wake last_fired_at")?;

        Ok(())
    }

    /// Update user-tunable fields, leaving `None` fields unchanged. Returns
    /// whether a row was updated.
    pub async fn update_tuning(
        &self,
        id: &str,
        name: Option<&str>,
        instructions: Option<&str>,
        min_level: Option<AutonomyLevel>,
        enabled: Option<bool>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE wake_defs SET \
                 name = COALESCE(?, name), \
                 instructions = COALESCE(?, instructions), \
                 min_level = COALESCE(?, min_level), \
                 enabled = COALESCE(?, enabled), \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ?",
        )
        .bind(name)
        .bind(instructions)
        .bind(min_level.map(AutonomyLevel::as_str))
        .bind(enabled)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("failed to update wake def tuning")?;

        Ok(result.rows_affected() == 1)
    }

    /// Store a webhook bearer token only when the row has none, so a
    /// concurrent minter cannot clobber a token already handed out. Returns
    /// whether this caller's token was stored.
    pub async fn set_webhook_token_if_absent(&self, id: &str, token: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE wake_defs SET webhook_token = ?, \
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             WHERE id = ? AND trigger_kind = 'webhook' AND webhook_token IS NULL",
        )
        .bind(token)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("failed to set wake def webhook token")?;

        Ok(result.rows_affected() == 1)
    }

    /// Look up the wake definition owning a webhook bearer token.
    pub async fn find_by_webhook_token(&self, token: &str) -> Result<Option<WakeDef>> {
        let row = sqlx::query(&format!(
            "SELECT {WAKE_DEF_COLUMNS} FROM wake_defs \
             WHERE trigger_kind = 'webhook' AND webhook_token = ?"
        ))
        .bind(token)
        .fetch_optional(&self.pool)
        .await
        .context("failed to look up wake by webhook token")?;

        row.map(def_from_row).transpose()
    }
}

const WAKE_DEF_COLUMNS: &str = "id, name, trigger_kind, trigger_spec, instructions, min_level, \
     enabled, builtin, config_owned, delivery_target, webhook_token, active_hours_start, \
     active_hours_end, next_run_at, last_fired_at, consecutive_failures, created_by, \
     created_at, updated_at";

fn def_from_row(row: sqlx::sqlite::SqliteRow) -> Result<WakeDef> {
    let trigger_kind: String = row
        .try_get("trigger_kind")
        .context("failed to read wake def trigger_kind")?;
    let trigger_spec_text: String = row
        .try_get("trigger_spec")
        .unwrap_or_else(|_| "{}".to_string());
    let trigger_spec: Value = serde_json::from_str(&trigger_spec_text)
        .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
    let trigger = WakeTrigger::from_parts(&trigger_kind, &trigger_spec)?;

    let min_level_text: String = row
        .try_get("min_level")
        .context("failed to read wake def min_level")?;
    let min_level = AutonomyLevel::parse(&min_level_text)
        .with_context(|| format!("unknown wake def min_level '{min_level_text}'"))?;

    let active_hours_start: Option<i64> = row.try_get("active_hours_start").ok().flatten();
    let active_hours_end: Option<i64> = row.try_get("active_hours_end").ok().flatten();
    let active_hours = match (active_hours_start, active_hours_end) {
        (Some(start), Some(end)) => Some((start as u8, end as u8)),
        _ => None,
    };

    Ok(WakeDef {
        id: row.try_get("id").context("failed to read wake def id")?,
        name: row
            .try_get("name")
            .context("failed to read wake def name")?,
        trigger,
        instructions: row
            .try_get("instructions")
            .context("failed to read wake def instructions")?,
        min_level,
        enabled: row
            .try_get("enabled")
            .context("failed to read wake def enabled")?,
        builtin: row
            .try_get("builtin")
            .context("failed to read wake def builtin")?,
        config_owned: row
            .try_get("config_owned")
            .context("failed to read wake def config_owned")?,
        delivery_target: row.try_get("delivery_target").ok().flatten(),
        webhook_token: row.try_get("webhook_token").ok().flatten(),
        active_hours,
        next_run_at: row.try_get("next_run_at").ok().flatten(),
        last_fired_at: row.try_get("last_fired_at").ok().flatten(),
        consecutive_failures: row
            .try_get("consecutive_failures")
            .context("failed to read wake def consecutive_failures")?,
        created_by: row
            .try_get("created_by")
            .context("failed to read wake def created_by")?,
        created_at: row
            .try_get("created_at")
            .context("failed to read wake def created_at")?,
        updated_at: row
            .try_get("updated_at")
            .context("failed to read wake def updated_at")?,
    })
}

/// Seed built-in wake definitions. Inserts only when absent so operator
/// tuning (enabled, min_level) survives restarts.
pub async fn seed_builtin_wakes(store: &WakeDefStore) -> Result<()> {
    let task_approved = WakeDef {
        id: TASK_APPROVED_WAKE_ID.to_string(),
        name: "Task approved".to_string(),
        trigger: WakeTrigger::Event {
            event: SystemEvent::TaskApproved,
        },
        instructions: "A task was approved for execution. Pick it up now instead of waiting \
             for the next interval."
            .to_string(),
        min_level: AutonomyLevel::Act,
        enabled: true,
        builtin: true,
        config_owned: false,
        delivery_target: None,
        webhook_token: None,
        active_hours: None,
        next_run_at: None,
        last_fired_at: None,
        consecutive_failures: 0,
        created_by: "system".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
    };
    store.insert_if_absent(&task_approved).await?;

    let verify_after_work = WakeDef {
        id: VERIFY_AFTER_WORK_WAKE_ID.to_string(),
        name: "Verify after work".to_string(),
        trigger: WakeTrigger::Event {
            event: SystemEvent::WorkerCompleted,
        },
        instructions: "A worker just completed. Run the project's verification suite — \
             the repo's check/build/test commands (e.g. `just gate-pr` or the \
             equivalent CI script) — to confirm the change is green. If the \
             suite fails, create a fix-worker task titled with the failing \
             area, describe the failure and the relevant command output in the \
             description, and attach the failing evidence so it can be fixed \
             autonomously. If the suite passes, do not create a task."
            .to_string(),
        min_level: AutonomyLevel::Suggest,
        enabled: true,
        builtin: true,
        config_owned: false,
        delivery_target: None,
        webhook_token: None,
        active_hours: None,
        next_run_at: None,
        last_fired_at: None,
        consecutive_failures: 0,
        created_by: "system".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
    };
    store.insert_if_absent(&verify_after_work).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wakes::{WakeEventStore, emit_to_stores};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn store() -> WakeDefStore {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        WakeDefStore::new(pool)
    }

    fn def(id: &str, trigger: WakeTrigger) -> WakeDef {
        WakeDef {
            id: id.to_string(),
            name: format!("{id} name"),
            trigger,
            instructions: format!("{id} instructions"),
            min_level: AutonomyLevel::Suggest,
            enabled: true,
            builtin: false,
            config_owned: false,
            delivery_target: None,
            webhook_token: None,
            active_hours: Some((8, 22)),
            next_run_at: None,
            last_fired_at: None,
            consecutive_failures: 0,
            created_by: "user".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[tokio::test]
    async fn upsert_and_get_round_trip() {
        let store = store().await;
        let schedule = def(
            "morning-brief",
            WakeTrigger::Schedule {
                cron_expr: Some("0 8 * * *".to_string()),
                interval_secs: None,
            },
        );
        store.upsert(&schedule).await.expect("upsert");

        let loaded = store
            .get("morning-brief")
            .await
            .expect("get")
            .expect("row exists");
        assert_eq!(loaded.name, "morning-brief name");
        assert_eq!(loaded.trigger, schedule.trigger);
        assert_eq!(loaded.min_level, AutonomyLevel::Suggest);
        assert_eq!(loaded.active_hours, Some((8, 22)));
        assert!(loaded.enabled);
        assert!(!loaded.builtin);
        assert!(!loaded.created_at.is_empty());
    }

    #[tokio::test]
    async fn upsert_preserves_schedule_cursor() {
        let store = store().await;
        let mut wake = def(
            "interval",
            WakeTrigger::Schedule {
                cron_expr: None,
                interval_secs: Some(600),
            },
        );
        store.upsert(&wake).await.expect("insert");
        assert!(
            store
                .claim_schedule_fire("interval", None, "2026-08-09T10:00:00.000Z")
                .await
                .expect("claim")
        );

        wake.instructions = "updated instructions".to_string();
        store.upsert(&wake).await.expect("update");

        let loaded = store.get("interval").await.expect("get").expect("row");
        assert_eq!(loaded.instructions, "updated instructions");
        assert_eq!(
            loaded.next_run_at.as_deref(),
            Some("2026-08-09T10:00:00.000Z")
        );
    }

    #[tokio::test]
    async fn upsert_preserves_webhook_token() {
        let store = store().await;
        let mut hook = def("hook", WakeTrigger::Webhook);
        store.upsert(&hook).await.expect("insert");
        assert!(
            store
                .set_webhook_token_if_absent("hook", "minted-token")
                .await
                .expect("mint")
        );

        // Reconciliation rebuilds defs without a token; the stored one stays.
        hook.instructions = "updated instructions".to_string();
        hook.webhook_token = None;
        store.upsert(&hook).await.expect("update");

        let loaded = store.get("hook").await.expect("get").expect("row");
        assert_eq!(loaded.instructions, "updated instructions");
        assert_eq!(loaded.webhook_token.as_deref(), Some("minted-token"));
    }

    #[tokio::test]
    async fn insert_if_absent_never_overwrites() {
        let store = store().await;
        let original = def("wake", WakeTrigger::Webhook);
        assert!(store.insert_if_absent(&original).await.expect("insert"));

        let mut changed = original.clone();
        changed.instructions = "changed".to_string();
        assert!(!store.insert_if_absent(&changed).await.expect("second"));

        let loaded = store.get("wake").await.expect("get").expect("row");
        assert_eq!(loaded.instructions, "wake instructions");
    }

    #[tokio::test]
    async fn event_subscribers_filter_by_event_and_enabled() {
        let store = store().await;
        store
            .upsert(&def(
                "on-approve",
                WakeTrigger::Event {
                    event: SystemEvent::TaskApproved,
                },
            ))
            .await
            .expect("upsert");
        store
            .upsert(&def(
                "on-goal",
                WakeTrigger::Event {
                    event: SystemEvent::GoalCreated,
                },
            ))
            .await
            .expect("upsert");
        let mut disabled = def(
            "disabled",
            WakeTrigger::Event {
                event: SystemEvent::TaskApproved,
            },
        );
        disabled.enabled = false;
        store.upsert(&disabled).await.expect("upsert");

        let subscribers = store
            .event_subscribers(SystemEvent::TaskApproved)
            .await
            .expect("subscribers");
        assert_eq!(subscribers.len(), 1);
        assert_eq!(subscribers[0].id, "on-approve");
    }

    #[tokio::test]
    async fn due_schedule_wakes_orders_by_cursor() {
        let store = store().await;
        store
            .upsert(&def(
                "uninitialized",
                WakeTrigger::Schedule {
                    cron_expr: None,
                    interval_secs: Some(600),
                },
            ))
            .await
            .expect("upsert");
        store
            .upsert(&def(
                "future",
                WakeTrigger::Schedule {
                    cron_expr: None,
                    interval_secs: Some(600),
                },
            ))
            .await
            .expect("upsert");
        store
            .upsert(&def("not-schedule", WakeTrigger::Webhook))
            .await
            .expect("upsert");
        assert!(
            store
                .claim_schedule_fire("future", None, "2099-01-01T00:00:00.000Z")
                .await
                .expect("claim")
        );

        let due = store
            .due_schedule_wakes("2026-08-09T10:00:00.000Z")
            .await
            .expect("due");
        let ids: Vec<&str> = due.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["uninitialized"]);
    }

    #[tokio::test]
    async fn claim_schedule_fire_is_cas_guarded() {
        let store = store().await;
        store
            .upsert(&def(
                "sched",
                WakeTrigger::Schedule {
                    cron_expr: None,
                    interval_secs: Some(600),
                },
            ))
            .await
            .expect("upsert");

        // First claim initializes from the NULL cursor.
        assert!(
            store
                .claim_schedule_fire("sched", None, "2026-08-09T10:00:00.000Z")
                .await
                .expect("first")
        );
        // A racer holding the stale expectation loses.
        assert!(
            !store
                .claim_schedule_fire("sched", None, "2026-08-09T10:10:00.000Z")
                .await
                .expect("stale")
        );
        // The current cursor claims and advances.
        assert!(
            store
                .claim_schedule_fire(
                    "sched",
                    Some("2026-08-09T10:00:00.000Z"),
                    "2026-08-09T10:10:00.000Z"
                )
                .await
                .expect("advance")
        );
    }

    #[tokio::test]
    async fn find_by_webhook_token_matches_token() {
        let store = store().await;
        let mut hook = def("ci-failed", WakeTrigger::Webhook);
        hook.webhook_token = Some("secret-token".to_string());
        store.upsert(&hook).await.expect("upsert");

        let found = store
            .find_by_webhook_token("secret-token")
            .await
            .expect("lookup");
        assert_eq!(found.map(|d| d.id), Some("ci-failed".to_string()));
        assert!(
            store
                .find_by_webhook_token("wrong")
                .await
                .expect("lookup")
                .is_none()
        );
    }

    #[tokio::test]
    async fn set_enabled_and_delete() {
        let store = store().await;
        store
            .upsert(&def("wake", WakeTrigger::Webhook))
            .await
            .expect("upsert");

        assert!(store.set_enabled("wake", false).await.expect("disable"));
        let loaded = store.get("wake").await.expect("get").expect("row");
        assert!(!loaded.enabled);

        assert!(store.delete("wake").await.expect("delete"));
        assert!(store.get("wake").await.expect("get").is_none());
        assert!(!store.delete("wake").await.expect("second delete"));
    }

    #[tokio::test]
    async fn update_tuning_leaves_unset_fields_unchanged() {
        let store = store().await;
        store
            .upsert(&def("wake", WakeTrigger::Webhook))
            .await
            .expect("upsert");

        assert!(
            store
                .update_tuning(
                    "wake",
                    None,
                    Some("new instructions"),
                    Some(AutonomyLevel::Act),
                    Some(false),
                )
                .await
                .expect("update")
        );

        let loaded = store.get("wake").await.expect("get").expect("row");
        assert_eq!(loaded.name, "wake name");
        assert_eq!(loaded.instructions, "new instructions");
        assert_eq!(loaded.min_level, AutonomyLevel::Act);
        assert!(!loaded.enabled);
        assert!(
            !store
                .update_tuning("missing", None, None, None, Some(true))
                .await
                .expect("missing")
        );
    }

    #[tokio::test]
    async fn set_webhook_token_if_absent_never_overwrites() {
        let store = store().await;
        store
            .upsert(&def("hook", WakeTrigger::Webhook))
            .await
            .expect("upsert");
        store
            .upsert(&def(
                "sched",
                WakeTrigger::Schedule {
                    cron_expr: None,
                    interval_secs: Some(600),
                },
            ))
            .await
            .expect("upsert");

        assert!(
            store
                .set_webhook_token_if_absent("hook", "first")
                .await
                .expect("mint")
        );
        assert!(
            !store
                .set_webhook_token_if_absent("hook", "second")
                .await
                .expect("re-mint")
        );
        // Non-webhook wakes never receive tokens.
        assert!(
            !store
                .set_webhook_token_if_absent("sched", "token")
                .await
                .expect("schedule")
        );

        let loaded = store.get("hook").await.expect("get").expect("row");
        assert_eq!(loaded.webhook_token.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn touch_last_fired_sets_timestamp() {
        let store = store().await;
        store
            .upsert(&def("wake", WakeTrigger::Webhook))
            .await
            .expect("upsert");
        store.touch_last_fired("wake").await.expect("touch");

        let loaded = store.get("wake").await.expect("get").expect("row");
        assert!(loaded.last_fired_at.is_some());
    }

    #[tokio::test]
    async fn seed_builtin_is_idempotent_and_preserves_tuning() {
        let store = store().await;
        seed_builtin_wakes(&store).await.expect("seed");

        let seeded = store
            .get(TASK_APPROVED_WAKE_ID)
            .await
            .expect("get")
            .expect("row");
        assert!(seeded.builtin);
        assert_eq!(seeded.min_level, AutonomyLevel::Act);
        assert_eq!(
            seeded.trigger,
            WakeTrigger::Event {
                event: SystemEvent::TaskApproved
            }
        );

        // Operator disables the builtin; a restart's re-seed keeps that.
        store
            .set_enabled(TASK_APPROVED_WAKE_ID, false)
            .await
            .expect("disable");
        seed_builtin_wakes(&store).await.expect("re-seed");
        let after = store
            .get(TASK_APPROVED_WAKE_ID)
            .await
            .expect("get")
            .expect("row");
        assert!(!after.enabled);
    }

    #[tokio::test]
    async fn seed_also_installs_verify_after_work() {
        let store = store().await;
        seed_builtin_wakes(&store).await.expect("seed");

        let seeded = store
            .get(VERIFY_AFTER_WORK_WAKE_ID)
            .await
            .expect("get")
            .expect("row");
        assert!(seeded.builtin);
        assert_eq!(seeded.min_level, AutonomyLevel::Suggest);
        assert_eq!(
            seeded.trigger,
            WakeTrigger::Event {
                event: SystemEvent::WorkerCompleted
            }
        );
        // Its instructions direct the agent to run verification and propose
        // a fix-worker task only on failure.
        assert!(seeded.instructions.contains("verification"));
        assert!(seeded.instructions.contains("fix-worker"));

        // The event routes to it: a WorkerCompleted emission enqueues exactly
        // one pending event carrying the worker payload.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations");
        let store = WakeDefStore::new(pool.clone());
        let events = WakeEventStore::new(pool);
        seed_builtin_wakes(&store).await.expect("seed");
        let count = emit_to_stores(
            &store,
            &events,
            SystemEvent::WorkerCompleted,
            "worker:abc",
            &serde_json::json!({ "worker_id": "abc", "success": false, "summary": "failed" }),
        )
        .await
        .expect("emit");
        assert_eq!(count, 1);
        let pending = events.pending(10).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].wake_id, VERIFY_AFTER_WORK_WAKE_ID);
        assert_eq!(pending[0].payload["success"], false);
    }
}
