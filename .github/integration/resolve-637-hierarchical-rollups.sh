#!/usr/bin/env bash
set -euo pipefail

# #637 is already substantially implemented by the fork. Discard the carrier
# patch and add only the missing hierarchical behavior on the current staging
# architecture, preserving rolling_up serialization, prompt-debug capture and
# SQLite busy retries.
git reset --hard "origin/${PORT_TARGET}"

python3 - <<'PY'
from pathlib import Path
import re


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    n = text.count(old)
    if n != 1:
        raise SystemExit(f"{path}: expected one match, found {n}")
    p.write_text(text.replace(old, new, 1))

# Chronicle store: top-level render selection now works for arbitrary nesting;
# add generic listing and parent -> children navigation.
replace_once(
    "src/conversation/chronicle.rs",
    '''    /// The compact chronicle representation for prompt rendering: level-1\n    /// rollups plus level-0 checkpoints not yet absorbed by a rollup.\n    pub async fn list_renderable(\n        &self,\n        channel_id: &str,\n        limit: i64,\n    ) -> Result<Vec<ChronicleCheckpoint>> {\n        let rows = sqlx::query(\n            "SELECT * FROM ( \\\n                 SELECT * FROM channel_chronicle_checkpoints \\\n                 WHERE channel_id = ? \\\n                   AND (level = 1 OR (level = 0 AND rolled_up_into IS NULL)) \\\n                 ORDER BY covers_to_seq DESC LIMIT ? \\\n             ) ORDER BY covers_from_seq ASC",\n        )\n        .bind(channel_id)\n        .bind(limit)\n        .fetch_all(&self.pool)\n        .await\n        .map_err(|error| anyhow::anyhow!(error))?;\n\n        Ok(checkpoints_from_rows(rows))\n    }\n''',
    '''    /// The compact chronicle representation for prompt rendering: every\n    /// checkpoint not represented by a parent rollup, at any level.\n    pub async fn list_renderable(\n        &self,\n        channel_id: &str,\n        limit: i64,\n    ) -> Result<Vec<ChronicleCheckpoint>> {\n        let rows = sqlx::query(\n            "SELECT * FROM ( \\\n                 SELECT * FROM channel_chronicle_checkpoints \\\n                 WHERE channel_id = ? AND rolled_up_into IS NULL \\\n                 ORDER BY covers_to_seq DESC LIMIT ? \\\n             ) ORDER BY covers_from_seq ASC, seq ASC",\n        )\n        .bind(channel_id)\n        .bind(limit)\n        .fetch_all(&self.pool)\n        .await\n        .map_err(|error| anyhow::anyhow!(error))?;\n\n        Ok(checkpoints_from_rows(rows))\n    }\n\n    /// Checkpoints at every level, newest first.\n    pub async fn list_all_levels(\n        &self,\n        channel_id: &str,\n        limit: i64,\n    ) -> Result<Vec<ChronicleCheckpoint>> {\n        let rows = sqlx::query(\n            "SELECT * FROM channel_chronicle_checkpoints \\\n             WHERE channel_id = ? ORDER BY seq DESC LIMIT ?",\n        )\n        .bind(channel_id)\n        .bind(limit)\n        .fetch_all(&self.pool)\n        .await\n        .map_err(|error| anyhow::anyhow!(error))?;\n        Ok(checkpoints_from_rows(rows))\n    }\n\n    /// Direct children represented by one rollup, oldest first.\n    pub async fn children_of(&self, rollup_id: &str) -> Result<Vec<ChronicleCheckpoint>> {\n        let rows = sqlx::query(\n            "SELECT * FROM channel_chronicle_checkpoints \\\n             WHERE rolled_up_into = ? ORDER BY seq ASC",\n        )\n        .bind(rollup_id)\n        .fetch_all(&self.pool)\n        .await\n        .map_err(|error| anyhow::anyhow!(error))?;\n        Ok(checkpoints_from_rows(rows))\n    }\n''',
)

# Rollup execution: keep the fork's serialized background task, debug records,
# secret scan and busy-retry store. Generalize its existing level-0 -> level-1
# body to a bounded hierarchy.
p = Path("src/agent/chronicle.rs")
text = p.read_text()
start = text.index("impl RollupContext {\n")
end = text.index("\n/// Drop live entries a checkpoint covers", start)
new_impl = r'''impl RollupContext {
    async fn run(&self) -> Result<()> {
        const MAX_ROLLUP_LEVELS: i64 = 8;
        let mut progressed = true;
        while progressed {
            progressed = false;
            for level in 0..MAX_ROLLUP_LEVELS {
                if self.roll_up_level(level).await? {
                    progressed = true;
                }
            }
        }
        Ok(())
    }

    async fn roll_up_level(&self, level: i64) -> Result<bool> {
        let unrolled = self
            .store
            .list_unrolled(
                &self.channel_id,
                level,
                self.config.rollup_threshold as i64,
            )
            .await?;
        if unrolled.len() < self.config.rollup_threshold {
            return Ok(false);
        }

        let source_count = self.config.rollup_batch.min(unrolled.len());
        if source_count < 2 {
            return Ok(false);
        }
        let sources = &unrolled[..source_count];
        for pair in sources.windows(2) {
            if pair[0].covers_to_seq != pair[1].covers_from_seq {
                tracing::warn!(
                    channel_id = %self.channel_id,
                    level,
                    "chronicle rollup skipped because source coverage is not contiguous"
                );
                return Ok(false);
            }
        }

        let prompt_engine = self.deps.runtime_config.prompts.load();
        let preamble = prompt_engine.render_static_segmented("chronicle_rollup")?;
        let routing = self.deps.runtime_config.routing.load();
        let model_name = self
            .model_override
            .clone()
            .unwrap_or_else(|| routing.resolve(ProcessType::Compactor, None).to_string());
        let model = SpacebotModel::make(&self.deps.llm_manager, &model_name)
            .with_context(&*self.deps.agent_id, "chronicle_rollup")
            .with_routing((**routing).clone())
            .with_debug(
                self.deps.prompt_records(),
                crate::llm::record::DebugContext {
                    process: Some(crate::llm::record::ProcessRef {
                        kind: "chronicle_rollup".to_string(),
                        id: Some(self.channel_id.to_string()),
                        process_type: None,
                        channel_id: Some(self.channel_id.to_string()),
                    }),
                    trigger: Some(crate::llm::record::Trigger {
                        kind: "chronicle_rollup".to_string(),
                        message_id: None,
                        input: None,
                        parent: Some(format!("channel:{}", self.channel_id)),
                    }),
                    blocks: preamble.blocks.clone(),
                },
            );
        let agent = AgentBuilder::new(model)
            .preamble(&preamble.text)
            .default_max_turns(1)
            .build();
        let hook = SpacebotHook::new(
            self.deps.agent_id.clone(),
            ProcessId::Worker(Uuid::new_v4()),
            ProcessType::Compactor,
            Some(self.channel_id.clone()),
            self.deps.event_tx.clone(),
        )
        .with_secret_scan_mode(self.deps.runtime_config.sandbox.load().secret_scanner);
        let prompt = build_rollup_prompt(sources);
        let mut history = Vec::new();
        let response = hook.prompt_once(&agent, &mut history, &prompt).await;
        let fallback_title = format!(
            "{} to {}",
            sources.first().expect("sources is non-empty").title,
            sources.last().expect("sources is non-empty").title
        );
        let (title, summary, model) = match response {
            Ok(text) => {
                let (title, summary) = parse_checkpoint_response(&text);
                (title.unwrap_or(fallback_title), summary, Some(model_name))
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    channel_id = %self.channel_id,
                    level,
                    "chronicle rollup summarization failed"
                );
                return Ok(false);
            }
        };

        let first = sources.first().expect("sources is non-empty");
        let last = sources.last().expect("sources is non-empty");
        let source_ids: Vec<String> = sources.iter().map(|c| c.id.clone()).collect();
        let outcome = self
            .store
            .commit_rollup(
                NewCheckpoint {
                    channel_id: self.channel_id.to_string(),
                    level: level + 1,
                    kind: CheckpointKind::Rollup,
                    title,
                    summary: summary.clone(),
                    covers_from: first.start_boundary(),
                    covers_to: last.end_boundary(),
                    covers_from_at: first.covers_from_at,
                    covers_to_at: last.covers_to_at,
                    covers_from_message_id: first.covers_from_message_id.clone(),
                    covers_to_message_id: last.covers_to_message_id.clone(),
                    message_count: sources.iter().map(|c| c.message_count).sum(),
                    token_estimate: estimate_text_tokens(&summary) as i64,
                    rolls_up_from_seq: Some(first.seq),
                    rolls_up_to_seq: Some(last.seq),
                    model,
                },
                &source_ids,
            )
            .await?;

        match outcome {
            CommitOutcome::Committed(checkpoint) => {
                emit_checkpoint_event(&self.deps, &self.channel_id, &checkpoint);
                tracing::info!(
                    channel_id = %self.channel_id,
                    seq = checkpoint.seq,
                    level = checkpoint.level,
                    source_count = sources.len(),
                    "chronicle hierarchical rollup committed"
                );
                Ok(true)
            }
            CommitOutcome::Superseded { .. } | CommitOutcome::Busy => Ok(false),
        }
    }
}
'''
p.write_text(text[:start] + new_impl + text[end:])

# Config invariant: a one-child rollup is not a rollup.
replace_once(
    "src/config/load.rs",
    '''    config.rollup_threshold = config.rollup_threshold.max(1);\n    config.rollup_batch = config.rollup_batch.clamp(1, config.rollup_threshold);\n''',
    '''    config.rollup_batch = config.rollup_batch.clamp(2, MAX_LIST_ENTRIES);\n    config.rollup_threshold = config\n        .rollup_threshold\n        .clamp(config.rollup_batch, MAX_LIST_ENTRIES.saturating_mul(2));\n''',
)

# Chronicle tool: show only top-level entries and make rollups transparently
# navigable to their direct children.
p = Path("src/tools/chronicle.rs")
text = p.read_text()
old = '''        let checkpoints = self\n            .store\n            .list(&self.channel_id, 0, limit)\n            .await\n            .map_err(|error| ChronicleError(format!("Failed to list checkpoints: {error}")))?;\n'''
new = '''        let checkpoints = self\n            .store\n            .list_all_levels(&self.channel_id, MAX_LIST_LIMIT)\n            .await\n            .map_err(|error| ChronicleError(format!("Failed to list checkpoints: {error}")))?;\n        let checkpoints: Vec<_> = checkpoints\n            .into_iter()\n            .filter(|checkpoint| checkpoint.rolled_up_into.is_none())\n            .take(limit as usize)\n            .collect();\n'''
if text.count(old) != 1:
    raise SystemExit("tools/chronicle.rs list query marker mismatch")
text = text.replace(old, new, 1)
old = '''    async fn open(&self, seq: Option<i64>) -> Result<ChronicleOutput, ChronicleError> {\n        let checkpoint = self.require_checkpoint(seq, "open").await?;\n        Ok(self.output("open", render_checkpoint(&checkpoint)))\n    }\n'''
new = '''    async fn open(&self, seq: Option<i64>) -> Result<ChronicleOutput, ChronicleError> {\n        let checkpoint = self.require_checkpoint(seq, "open").await?;\n        let mut summary = render_checkpoint(&checkpoint);\n        if checkpoint.level > 0 {\n            let children = self.store.children_of(&checkpoint.id).await.map_err(|error| {\n                ChronicleError(format!("Failed to read rollup children: {error}"))\n            })?;\n            if !children.is_empty() {\n                summary.push_str(&format!("\\n### Covers {} checkpoint(s)\\n\\n", children.len()));\n                for child in &children {\n                    summary.push_str(&format!(\n                        "- **#{}** {}{} — {} → {} · {} messages\\n",\n                        child.seq,\n                        if child.level > 0 { "[rollup] " } else { "" },\n                        child.title,\n                        child.covers_from_at.format("%Y-%m-%d %H:%M"),\n                        child.covers_to_at.format("%Y-%m-%d %H:%M"),\n                        child.message_count,\n                    ));\n                }\n                summary.push_str("\\nOpen a child for its summary, or expand a leaf for raw messages.\\n");\n            }\n        }\n        Ok(self.output("open", summary))\n    }\n'''
if text.count(old) != 1:
    raise SystemExit("tools/chronicle.rs open marker mismatch")
p.write_text(text.replace(old, new, 1))

# Focused store tests: top-level selection and parent/children are the durable
# invariants that make nested rollups reversible.
p = Path("src/conversation/chronicle.rs")
text = p.read_text()
marker = '''    #[tokio::test]\n    async fn stats_report_unsummarized_tail() {\n'''
test = r'''    #[tokio::test]
    async fn nested_rollup_children_remain_navigable() {
        let store = setup().await;
        let mut children = Vec::new();
        let mut from = 0;
        for _ in 0..4 {
            let to = from + 10;
            let CommitOutcome::Committed(checkpoint) = store
                .commit(new_checkpoint("ch", from, to, 10))
                .await
                .expect("commit")
            else { panic!("checkpoint commit") };
            children.push(*checkpoint);
            from = to;
        }
        let first = &children[0];
        let last = &children[3];
        let ids: Vec<String> = children.iter().map(|c| c.id.clone()).collect();
        let CommitOutcome::Committed(rollup) = store
            .commit_rollup(
                NewCheckpoint {
                    channel_id: "ch".into(), level: 1, kind: CheckpointKind::Rollup,
                    title: "rollup".into(), summary: "summary".into(),
                    covers_from: first.start_boundary(), covers_to: last.end_boundary(),
                    covers_from_at: first.covers_from_at, covers_to_at: last.covers_to_at,
                    covers_from_message_id: None, covers_to_message_id: None,
                    message_count: 40, token_estimate: 10,
                    rolls_up_from_seq: Some(first.seq), rolls_up_to_seq: Some(last.seq), model: None,
                },
                &ids,
            )
            .await
            .expect("rollup")
        else { panic!("rollup commit") };
        let listed = store.children_of(&rollup.id).await.expect("children");
        assert_eq!(listed.len(), 4);
        let top = store.list_renderable("ch", 20).await.expect("renderable");
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].id, rollup.id);
    }

'''
if text.count(marker) != 1:
    raise SystemExit("chronicle store test marker mismatch")
p.write_text(text.replace(marker, test + marker, 1))
PY

cargo fmt --all
git add src/agent/chronicle.rs src/config/load.rs src/conversation/chronicle.rs src/tools/chronicle.rs
git diff --cached --check
