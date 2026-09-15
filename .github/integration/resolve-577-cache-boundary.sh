#!/usr/bin/env bash
set -euo pipefail

# Preserve the fork's newer prompt engine/template structure in the three
# conflicted files, while keeping every cleanly-applied upstream change.
for path in prompts/en/channel.md.j2 src/prompts.rs src/prompts/engine.rs; do
  git checkout --ours -- "$path"
  git add "$path"
done

python3 - <<'PY'
from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one match, found {count}")
    p.write_text(text.replace(old, new, 1))

# Put the boundary at the fork's explicit instruction/context seam. Task board
# and linked-agent instructions stay in the stable prefix; world-state context
# remains volatile.
replace_once(
    "prompts/en/channel.md.j2",
    '''Everything below this line is context — a description of the world as it is now, not instruction.\n''',
    '''<!-- spacebot-system-prompt-cache-boundary -->\n\nEverything below this line is context — a description of the world as it is now, not instruction.\n''',
)

replace_once(
    "src/prompts.rs",
    '''pub use engine::{ChannelPromptInputs, PromptEngine, PromptInputs, SkillInfo};\n''',
    '''pub use engine::{\n    ChannelPromptInputs, PromptEngine, PromptInputs, SkillInfo,\n    split_system_prompt_cache_boundary, strip_system_prompt_cache_boundary,\n};\n''',
)

replace_once(
    "src/prompts/engine.rs",
    '''use std::sync::Arc;\n''',
    '''use std::sync::Arc;\n\npub const SYSTEM_PROMPT_CACHE_BOUNDARY: &str =\n    "<!-- spacebot-system-prompt-cache-boundary -->";\n\npub fn split_system_prompt_cache_boundary(prompt: &str) -> Option<(&str, &str)> {\n    prompt.split_once(SYSTEM_PROMPT_CACHE_BOUNDARY)\n}\n\npub fn strip_system_prompt_cache_boundary(prompt: &str) -> String {\n    prompt.replace(SYSTEM_PROMPT_CACHE_BOUNDARY, "")\n}\n''',
)

replace_once(
    "src/prompts/engine.rs",
    '''mod tests {\n    use super::{ChannelPromptInputs, PromptEngine};\n''',
    '''mod tests {\n    use super::{\n        ChannelPromptInputs, PromptEngine, split_system_prompt_cache_boundary,\n        strip_system_prompt_cache_boundary,\n    };\n''',
)

needle = '''    /// The block map must describe the prompt that would have been sent\n'''
test = '''    #[test]\n    fn channel_prompt_cache_boundary_separates_stable_and_volatile_context() {\n        let engine = PromptEngine::new("en").expect("prompt engine should build");\n        let mut inputs = base_inputs(&engine);\n        inputs.worker_capabilities = "## Worker Types\\n\\nStable capabilities.".to_string();\n        inputs.working_memory = Some("## Working Memory\\n\\nVolatile memory.".to_string());\n        inputs.status_text = Some("Volatile status.".to_string());\n\n        let prompt = engine\n            .render_channel_prompt(inputs)\n            .expect("channel prompt should render")\n            .text;\n        let (stable, volatile) = split_system_prompt_cache_boundary(&prompt)\n            .expect("channel prompt should contain cache boundary");\n\n        assert!(stable.contains("Stable capabilities."));\n        assert!(!stable.contains("Volatile memory."));\n        assert!(!stable.contains("Volatile status."));\n        assert!(volatile.contains("Volatile memory."));\n        assert!(volatile.contains("Volatile status."));\n    }\n\n    #[test]\n    fn cache_boundary_helpers_split_and_strip_marker() {\n        let prompt = format!(\n            "stable\\n{}\\nvolatile",\n            super::SYSTEM_PROMPT_CACHE_BOUNDARY\n        );\n        let (stable, volatile) = split_system_prompt_cache_boundary(&prompt).unwrap();\n        assert_eq!(stable, "stable\\n");\n        assert_eq!(volatile, "\\nvolatile");\n        assert_eq!(strip_system_prompt_cache_boundary(&prompt), "stable\\n\\nvolatile");\n    }\n\n'''
replace_once("src/prompts/engine.rs", needle, test + needle)

# The upstream docs example used a template variable because its older engine
# injected the marker through render context. This fork embeds the marker at the
# template's explicit instruction/context seam, so document that literal seam.
docs = Path("docs/content/docs/(core)/prompts.mdx")
if docs.exists():
    text = docs.read_text()
    text = text.replace(
        "{{ system_prompt_cache_boundary }}",
        "<!-- spacebot-system-prompt-cache-boundary -->",
    )
    docs.write_text(text)
PY

cargo fmt --all
git add prompts/en/channel.md.j2 src/prompts.rs src/prompts/engine.rs \
  'docs/content/docs/(core)/prompts.mdx'
git diff --cached --check
