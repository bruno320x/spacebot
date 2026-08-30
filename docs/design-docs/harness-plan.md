# Harness — Phased Implementation Plan

> Operational contract. This document is the spec we execute from — after it
> is approved we do not re-discuss it. Each phase ends with tests; a phase is
> done only when its verification is green. The human's only touchpoints are
> approving this plan and merging phase PRs.
>
> **Operating model (from memory-map.md):** a human is present at exactly two
> points — *initial planning* (this plan) and *PR review/merge*. Everything
> between is fully autonomous (YOLO under the `AutonomyLevel::Act` ceiling).
> Up-front delegation (plan/goal approved once) *is* the approval; there is no
> per-task human gate during execution.

---

## Execution Contract (how every phase runs)

1. Each phase lives on its own branch off `main` (`phase-<n>-<name>`).
2. Implement per the phase steps below. Prefer local verification where the
   sandbox allows (pure modules compile+test standalone; `cargo fmt --check`
   runs here; `bun tsc` for interface).
3. Push the branch → open a PR against `main` → GitHub Actions CI runs real
   `check` (clippy `-D warnings`), `fmt`, `test`, and `interface` checks.
4. **Green CI is the phase's test.** No phase merges red. Merge is human-only.
5. After merge, the next phase starts. No mid-phase discussion; if a phase
   fails CI, fix the failure and re-run — that is working the plan, not
   changing it.

**Test doctrine:** every new module carries unit tests; every new protocol
door gets a wire-level round-trip test; every phase leaves the full
`cargo test --lib` suite green (currently 1365+ tests).

---

## Phase Inventory

| Faz | Name | State |
|---|---|---|
| 0 | Solidify the base | ⏳ in progress (0.1 merged) |
| 1 | Finish the doors (MCP server / ACP server / TUI) | ❌ not started |
| 2 | Mode engine wired + verified goal loop | ❌ not started |
| 3 | Reasoning breadth (multimodal, standing e2e, code memory) | ❌ not started |

---

## Faz 0 — Solidify the base

**Goal:** a proven foundation: ACP adapter, process supervision, mode engine,
and an architecture knowledge base, all green on `main`.

### 0.1 ACP adapter (✅ DONE — merged as PR #1)
External coding CLIs (Claude Code, Codex, Cursor CLI, OpenCode ACP mode, …) as
interactive workers over Agent Client Protocol v1. Verified: 1365+ tests green
including ACP wire tests (11) and the deserialization-order fix.

### 0.2 Mode engine (⏳ branch `mode-in-tasks`, pending)
`src/mode.rs`: `TaskMode { Plan, Goal, Vibe, Yolo }` + pure `decide_mode()`
(novelty/risk/trust) + Display/FromStr + serde (lowercase form). 14 unit tests,
verified standalone. **Remaining:** commit the pending serde commit + memory-map
doc and open the PR.

- Files: `src/mode.rs` (landed on main via PR #1), serde commit on
  `mode-in-tasks`, `docs/design-docs/memory-map.md` (untracked).
- Verification: `cargo test --lib` green in CI (mode tests run with lib tests);
  standalone compile check already green.
- Done when: `mode-in-tasks` PR merged, `main` carries mode.rs + memory-map.md.

### 0.3 Supervisor adoption (not started)
`src/supervisor.rs` (no-hang `run_to_completion`, no-orphan `ChildRegistry`,
env sanitize) landed with `AgentDeps.child_registry` seam, but no backend uses
it yet. Adopt it backend-by-backend, one small PR each:

1. **ACP worker** — register its subprocess in `ChildRegistry` under the
   worker id; on drop/cancel the registry kills it (backstop to existing
   `kill_on_drop`). Keep stdio ownership with the driver.
2. **OpenCode server** — route spawn/health/restart through `run_to_completion`
   bounds; register pool children.
3. **Shell tool** — reuse `ProcessSpec::command()` env sanitization + the
   registry for command farms.

- Files: `src/acp/worker.rs`, `src/opencode/server.rs`, `src/tools/shell.rs`,
  `src/supervisor.rs`.
- Verification: CI clippy/test green; supervisor's 9 tests + backend unit tests.
- Done when: all three backends register children; `kill_group(worker_id)`
  terminates a spawned command (covered by tests).

---

## Faz 1 — Finish the doors

**Goal:** Spacebot is reachable *from* editors and terminals, not just *into*
them. Every protocol gets both directions (client ✅ shipped, server ❌ here).

### 1.1 MCP server (editors → Spacebot as tool host)
Editors (VS Code, Cursor, …) connect to Spacebot over MCP and expose its tools.

- Use the existing axum server + `rmcp` (already a dependency, server feature
  needed in `Cargo.toml`).
- Expose a curated tool surface: `task_create/list/update`, `memory_recall`,
  `spacebot_docs`, channel recall, wiki. No shell/file from editor sessions by
  default (untrusted source → sandbox encloses, per harness.md Pillar 2).
- Transport: stdio + streamable HTTP on the API server.
- Files: `src/mcp_server.rs` (new), `src/lib.rs` (mod), `src/main.rs` (serve),
  `src/tools.rs` (server-side tool factory), `Cargo.toml` (rmcp server feature).
- Verification: wire-level round-trip test (connect a mock MCP client, call a
  tool, assert response); CI green.
- Done when: `spacebot mcp serve` accepts an editor connection and answers a
  `tools/call` for `task_list` in the wire test.

### 1.2 ACP server (editors → Spacebot as coding agent)
Editors drive Spacebot as an ACP-compliant coding agent (the mirror of the
shipped ACP client).

- Reuse `src/acp/types.rs` wire types (initialize/session/new/prompt/update/
  permission). Server wraps a branch-backed agent session: `session/prompt`
  runs a branch with memory tools; `session/update` streams progress;
  `permission/request` surfaces risky actions.
- Files: `src/acp/server.rs` (new), `src/acp.rs` (re-export),
  `src/main.rs` (serve), tool factory for the session.
- Verification: wire-level round-trip test (initialize → session/new →
  prompt → update → exit) against a mock ACP client; CI green.
- Done when: the ACP server session test passes end-to-end in CI.

### 1.3 TUI (terminal interface)
Full terminal UI (today only a REPL exists).

- ratatui app: channel list, active tasks/workers, live status block, input.
- New binary target `spacebot tui` (or `src/bin/tui.rs`).
- Files: `src/bin/tui.rs` (new), `Cargo.toml` (ratatui dep), `src/api/*`
  (reuse SSE/state).
- Verification: CI check/clippy/test green (TUI unit tests for state mapping,
  no interactive test in CI); manual smoke deferred to runtime.
- Done when: binary builds in CI; state-mapping unit tests green.

---

## Faz 2 — Mode engine wired + verified goal loop

**Goal:** the autonomy loop that runs its own tests before saying "done".

### 2.1 Wire TaskMode into the task lifecycle
- Add `mode: Option<TaskMode>` (serde default) to `Task` + `ExecutionDefaults`
  in `src/tasks/store.rs`. Migration-safe: new column or metadata-backed with
  `#[serde(default)]` (decide at implementation time from how execution-plan
  fields are persisted).
- On task creation (task_create tool + cortex todo→task promotion) compute
  `decide_mode(TaskSignals)` and store it; clamp against the agent's
  `AutonomyLevel` ceiling (a task may not run above its ceiling).
- Show the mode in `task_list`/`task_update` output and the status block.
- Files: `src/tasks/store.rs`, `src/tools/task_create.rs`,
  `src/tools/task_update.rs`, `src/tools/task_list.rs`, `src/agent/cortex.rs`
  (promotion path), `src/mode.rs` (clamp helper).
- Verification: unit tests for the clamp + decide mapping; CI test suite green.
- Done when: created tasks carry a mode ≤ ceiling; tests pin the mapping.

### 2.2 Evidence-gated completion ("no test evidence, no done")
- Extend the outcome gate (tool-nudging): the worker's terminal
  `set_status(kind: "outcome")` must carry evidence — a command + exit code,
  or a passing test run — before the hook accepts it.
- Parse structured evidence in `hooks/spacebot.rs`; reject text-only "done".
- Files: `src/hooks/spacebot.rs`, `src/tools/set_status.rs`,
  `prompts/*/worker*.md.j2` (nudge text).
- Verification: unit tests for evidence parsing (pass/fail/missing); CI green.
- Done when: a worker claiming success without test evidence is nudged
  (existing nudge machinery) and cannot complete.

### 2.3 Standing e2e via wakes
- A built-in wake (`event = "worker.completed"` / schedule / webhook `ci`)
  that runs the project's verification suite on each push and spawns a fix
  worker when it fails (per wakes.md + cron semantics).
- Files: `src/wakes/*` (built-in wake), `src/agent/cortex.rs` (scheduling),
  `prompts/*` (instructions template).
- Verification: wake definition + firing unit tests; CI green.
- Done when: a failing-command wake event produces a fix-worker task
  (tested with a fixture, not a live repo).

---

## Faz 3 — Reasoning breadth

**Goal:** the agent sees and remembers like a person.

### 3.1 Multimodal vision
- Vision-capable model in the routing chain (`src/llm/routing.rs`) and plumbing
  to place image/video content into agent context (`src/llm/model.rs`,
  conversation/context).
- Files: `src/llm/*`, `src/conversation/context.rs`, `src/messaging/*`.
- Verification: unit tests for content-block routing; CI green (no live API).
- Done when: a prompt containing an image block routes to a vision model.

### 3.2 Standing autonomous e2e (from 2.3, made general)
- The wake-driven verification becomes a standing contract: every push runs
  suite → fix → re-run → PR-ready; failures escalate only after N attempts.
- Done when: fixture test proves push → suite → fix → re-verify loop.

### 3.3 Code / docs memory index
- Project files + docs indexed into the memory graph (embeddings) so the agent
  "remembers" the codebase across sessions; provenance = the door it came from.
- Files: `src/memory/embedding.rs`, `src/memory/lance.rs`, new ingestion path.
- Verification: indexing round-trip unit tests; CI green.

---

## Non-Goals (unchanged from harness.md)

- YOLO as default or soft choice — always explicit and per task.
- Vision as a prerequisite — layered last.
- Replacing the user as authority on completion — goals never auto-complete.
- Global one-mode-for-everything — modes are per task under the ceiling.