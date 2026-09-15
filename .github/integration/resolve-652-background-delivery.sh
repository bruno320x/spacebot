#!/usr/bin/env bash
set -euo pipefail

# The exact upstream delta applies everywhere except three overlapping relay
# blocks in channel.rs. Preserve the fork's duplicate suppression and combine it
# with #652's confirmed delivery semantics. Never record/log a relay as delivered
# until the adapter acknowledges it.

python3 - <<'PY'
from pathlib import Path

path = Path('src/agent/channel.rs')
text = path.read_text()

# Conflict 1: retain the fork's duplicate guard, then use the upstream confirmed
# send path and bool result.
start = text.index('<<<<<<< ours\n    /// Best-effort duplicate guard for retrigger fallback relays.')
end = text.index('>>>>>>> theirs\n', start) + len('>>>>>>> theirs\n')
block = text[start:end]
ours, theirs = block.split('=======\n', 1)
ours = ours.removeprefix('<<<<<<< ours\n')
theirs = theirs.rsplit('>>>>>>> theirs\n', 1)[0]
# Keep everything from ours before the old send_outbound_text definition.
needle = '    async fn send_outbound_text(&self, text: String, error_context: &str) {'
prefix, _old_send = ours.split(needle, 1)
merged = prefix + '''    async fn send_outbound_text(&self, text: String, error_context: &str) -> bool {\n        match self\n            .send_routed_confirmed(OutboundResponse::Text(text))\n            .await\n        {\n'''
# The common match arms follow immediately after the conflict block, so only
# reconstruct the function header here.
text = text[:start] + merged + text[end:]

# Conflict 2: skipped retrigger fallback. Dedupe first, send confirmed, and only
# then log + mark delivered. Keep explicit duplicate suppression logging.
old = '''<<<<<<< ours\n                                self.state\n                                    .conversation_logger\n                                    .log_bot_message(&self.state.channel_id, &final_text);\n                                self.send_outbound_text(\n                                    final_text,\n                                    "failed to send retrigger fallback reply",\n                                )\n                                .await;\n                            } else if !final_text.is_empty() {\n                                tracing::info!(\n                                    channel_id = %self.id,\n                                    "suppressing duplicate retrigger fallback output; result already relayed"\n                                );\n=======\n                                if self\n                                    .send_outbound_text(\n                                        final_text.clone(),\n                                        "failed to send retrigger fallback reply",\n                                    )\n                                    .await\n                                {\n                                    self.state\n                                        .conversation_logger\n                                        .log_bot_message(&self.state.channel_id, &final_text);\n                                    delivered_text = Some(final_text);\n                                }\n>>>>>>> theirs\n'''
new = '''                                if self\n                                    .send_outbound_text(\n                                        final_text.clone(),\n                                        "failed to send retrigger fallback reply",\n                                    )\n                                    .await\n                                {\n                                    self.state\n                                        .conversation_logger\n                                        .log_bot_message(&self.state.channel_id, &final_text);\n                                    delivered_text = Some(final_text);\n                                }\n                            } else if !final_text.is_empty() {\n                                tracing::info!(\n                                    channel_id = %self.id,\n                                    "suppressing duplicate retrigger fallback output; result already relayed"\n                                );\n'''
if text.count(old) != 1:
    raise SystemExit(f'skipped-retrigger conflict mismatch: {text.count(old)}')
text = text.replace(old, new, 1)

# Conflict 3: ordinary retrigger fallback. Again, dedupe before confirmed send.
old = '''<<<<<<< ours\n                                && !self.is_duplicate_of_recent_assistant(&final_text).await\n                            {\n                                if extracted.is_some() {\n                                    tracing::warn!(channel_id = %self.id, "extracted reply from malformed tool syntax in retrigger fallback");\n                                }\n                                self.state\n                                    .conversation_logger\n                                    .log_bot_message(&self.state.channel_id, &final_text);\n                                self.send_outbound_text(\n                                    final_text,\n                                    "failed to send retrigger fallback reply",\n                                )\n                                .await;\n                            } else if !final_text.is_empty() {\n                                tracing::info!(\n                                    channel_id = %self.id,\n                                    "suppressing duplicate retrigger fallback output; result already relayed"\n                                );\n=======\n                                && self\n                                    .send_outbound_text(\n                                        final_text.clone(),\n                                        "failed to send retrigger fallback reply",\n                                    )\n                                    .await\n                            {\n                                self.state\n                                    .conversation_logger\n                                    .log_bot_message(&self.state.channel_id, &final_text);\n                                delivered_text = Some(final_text);\n>>>>>>> theirs\n'''
new = '''                                && !self.is_duplicate_of_recent_assistant(&final_text).await\n                            {\n                                if extracted.is_some() {\n                                    tracing::warn!(channel_id = %self.id, "extracted reply from malformed tool syntax in retrigger fallback");\n                                }\n                                if self\n                                    .send_outbound_text(\n                                        final_text.clone(),\n                                        "failed to send retrigger fallback reply",\n                                    )\n                                    .await\n                                {\n                                    self.state\n                                        .conversation_logger\n                                        .log_bot_message(&self.state.channel_id, &final_text);\n                                    delivered_text = Some(final_text);\n                                }\n                            } else if !final_text.is_empty() {\n                                tracing::info!(\n                                    channel_id = %self.id,\n                                    "suppressing duplicate retrigger fallback output; result already relayed"\n                                );\n'''
if text.count(old) != 1:
    raise SystemExit(f'plain-retrigger conflict mismatch: {text.count(old)}')
text = text.replace(old, new, 1)

if any(marker in text for marker in ('<<<<<<<', '=======', '>>>>>>>')):
    raise SystemExit('conflict marker remains in src/agent/channel.rs')

path.write_text(text)
PY

git add src/agent/channel.rs
git diff --cached --check

git grep -n -E '^(<<<<<<<|=======|>>>>>>>)' -- src/agent/channel.rs && {
  echo 'conflict markers remain in channel.rs' >&2
  exit 1
} || true
