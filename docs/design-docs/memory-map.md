# Spacebot Memory Map (Architecture Knowledge Base)

> This is the harness's own memory map: a navigable map of who the processes
> are, what each is allowed to do, how they talk, and what state lives where.
> It is the source of truth the agent reasons against before touching code.
>
> **Operating model (the human-in-the-loop contract):** a human is only ever
> present at two points — *initial design / planning* and *pull-request
> review*. Everything in between — task creation, research, execution,
> testing, iteration, commits — is fully autonomous (YOLO by default). There
> is no human approval gate in the middle of work. Approval is delegated
> up-front (a plan/goal), and only merge needs a human.

---

## 1. The process model

Every LLM process is a Rig `Agent`. They differ in **system prompt, tools,
history, and lifecycle**. Five/h six core process kinds plus supporting loops:

| Process | What it is | Memory tools? | Shell/typed exec? | Speaks |
|---|---|---|---|---|
| **Channel** | User-facing conversation (one per thread/chat). Always responsive. | no (delegates) | no | `reply`, `route`, `spawn_worker`, `branch`, `cancel`, `skip`, `react` |
| **Branch** | Fork of channel context that thinks in isolation, then returns a conclusion. | `memory_recall/save/delete`, `channel_recall`, task tools | via spawned workers | returns result injected into channel history |
| **Worker** | Detached job runner. Fire-and-forget or interactive. Builtin (Rig) or external (ACP / OpenCode). | no (unless briefed) | `shell`, `file`, `browser`, `set_status` | reports `ProcessEvent`s; interactive ones take `route` follow-ups |
| **Compactor** | Programmatic context monitor (NOT an LLM). Tiered thresholds (80/85/95%). Spawns compaction workers. | triggers compaction worker | no | emits `CompactionTriggered` |
| **Cortex** | System observer + maintainer. Memonary bulletin, health, circuit breakers, memory maintenance, consolidation. Reader of the event bus, not a commander. | `memory_save` (bulletin/consolidation) | maintenance/background tasks only | subscribes to `event_tx` + `memory_event_tx` |
| **Autonomy channel** | One persistent channel for self-directed work. Enriches `pending_approval` tasks, executes `ready` tasks, creates tasks. Driven by wakes. | `memory_save/recall` directly (no branch) | execution tools directly | controlled by `AutonomyLevel` dial + `wake` triggers |
| **Cron / Wakes** | Time/event/HTTP/condition triggers that stir the autonomy channel or isolated cron channels. | n/a | n/a | `WakeSender` doorbell + persisted `wake_events` |

**Invariant:** the channel never blocks/waits on branches or workers. Work is
delegated; the channel stays responsive. Compaction never blocks the channel.

## 2. The communication backbone

- **`ProcessEvent` broadcast bus** (per agent, 256-slot bounded): the single
  event spine. Worker→channel stubs: `WorkerStart`, `WorkerStatus`,
  `WorkerInitialResult`, `WorkerComplete`. Branch results are injected into
  channel history, then deleted.
- **Channel→interactive worker:** the `route` tool resolves a worker's
  `input_tx` (mpsc) and sends the next turn message. Interactive workers keep
  one session; follow-ups are new turns on the same session (ACP) or same pool
  (OpenCode).
- **Cortex** subscribes to the same bus (separate `event_tx` + `memory_event_tx`
  subscribe) and `observe()`s into a rolling signal buffer — it does **not**
  command workers or write tasks. It runs health ticks, maintenance, bulletin.
- **Wake / injection:** cross-agent delegation and wake triggers ring a
  payload-free `WakeSender` doorbell; the autonomy channel is the single
  consumer of wake events (schedule / webhook / internal event / condition).
- **Cross-agent:** `agent_links` + `send_agent_message` → built as an
  `InboundMessage(source:"internal")` → `MessagingManager::inject_message()` →
  the main loop routes by `agent_id`/`conversation_id`. Link channels
  (`link:{id}`) carry relationship context (peer/superior/subordinate).
- **Human surface:** `reply` (channel), `set_outcome` (cron delivery), and
  `send_message_to_another_channel` (platform channels).

## 3. Memory system

Structured objects, never files. **SQLite** is the source of truth (rows +
associations + decay fields); **LanceDB** holds embeddings/vector + FTS; hybrid
search fuses via RRF + graph traversal.

- **Types:** Fact, Preference, Decision, Identity, Event, Observation, Todo.
  Identity is decay-exempt.
- **Two tiers (hot/warm):**
  - *Working state* (3-day hot, LRU cap ~64, 1.5x boost, no decay). New saves
    land here. `memory_promote` re-hots an old memory.
  - *Graph* (warm). Decay: `importance * age_decay * access_boost`
    (≈0.05/day, linear). Prune: importance < 0.1 after 30d. Merge: similarity
    > 0.95. `demoted_at` clocks retention for memories that passed through
    working state.
- **Recall flow:** branch → `memory_recall` → hybrid search (working first,
  then graph, boost applied) → **curation** → clean results returned. Channels
  never see raw rows.
- **Working memory:** append-only event log (temporal "what happened today"),
  assembled into the channel system prompt via a 5-layer context stack.
- **Bulletin:** cortex synthesizes a ~500-word `memory_bulletin` on an interval;
  all channels read it every turn via `ArcSwap`.
- **Maintenance** (cortex tick/maint interval): decay, prune, merge,
  working-state demotion, vector/word reindex.

## 4. Tasks, goals, wakes — how work is assigned and tracked

- **Two-tier origin:** a conversation-captured `Todo` memory → **Cortex
  promotes** actionable todos into structured `Task`s on a kanban board
  (`tasks` table, per-agent `#N`).
- **Lifecycle:** `pending_approval → ready → in_progress → done`, plus
  `backlog` and `failed`. (Shipped enum confirms 6 states.)
- **Assignment:** `assigned_agent_id` atomic claim; `last_enriched_at` for
  prioritization; `depends_on` → blocked_by; `goal_id` links work to a goal;
  `task_comments` are the append-only shared workspace between agent and user.
- **Execution paths:**
  1. *Channel path:* human says "start #42" → branch validates `ready`, sets
     `in_progress`, spawns worker (`worker_id` set), worker gets task
     description + subtasks as execution plan (+ briefing from comments).
  2. *Autonomy path:* wakes → survey → enrich `pending_approval` (parallel
     investigation workers + `add_task_comment`) → execute `ready` → report via
     `autonomy_complete`.
- **Failure / incomplete work:** every attempt persists in `worker_runs`;
  next-boot reconciliation marks unfinished; a crashed task returns to `ready`;
  three consecutive failures move it to `failed` and emit a working-memory
  Error event; circuit breakers (3 strikes) disable cron/wakes/maintenance.
- **Goals:** high-level "why", human-closed only; tasks are the "what";
  autonomy channel is the "how". No auto-completion of goals.

## 5. The autonomy dial — where YOLO sits

`AutonomyLevel` is the global permission ceiling, per agent:
`off → observe → suggest → act`.

- **Wakes** declare `min_level`; below it they don't fire instructions (but
  events still persist for later runs).
- The autonomy channel, task execution, enrichment, and self-healing are gated
  on this dial.
- **Harness stance (this repo's operating model):** the dial lives at
  **`act`** (max) so that everything between planning and PR review runs YOLO.
  Up-front delegation — a plan/goal approved once — *is* the approval; there is
  no per-task human gate during execution. Review and merge are the only human
  chokepoints.

### Per-task mode (`src/mode.rs`, landed) — the refinement under the ceiling

`TaskMode { Plan, Goal, Vibe, Yolo }` with a pure `decide_mode(TaskSignals)`
decision (novelty, risk, trust). Under the harness's YOLO stance, tasks
default to the ceiling (`act`), and `TaskMode` is an *execution detail* — a
way to spend more care (Plan research first, or Goal verify with tests) when a
specific task warrants it, **never** above the ceiling. The integration point
is task creation / dispatch (task_create + cortex promotion), not the worker
spawn path, so both the channel and autonomy paths see the same mode.

## 6. Storage

| Store | Engine | Holds |
|---|---|---|
| Per-agent SQLite | sqlx + `migrations/` | conversations, memories (rows/graph), tasks, worker_runs, cron, goals, wakes |
| LanceDB | embedded vector + FTS | memory embeddings, hybrid search indexes |
| Instance SQLite | `migrations_instance/` | cross-agent data (agent_links) |
| redb | embedded KV | secrets (AES-256-GCM), settings |

**Migration rule:** never edit an applied migration; add a new one.

## 7. Where tools live

Tools are organized by **function**, not consumer; which process gets which
is configured in `tools.rs` factories:
- Channel: reply, branch, spawn_worker, route, cancel, skip, react, cron.
- Branch/cortex-chat: memory, channel_recall, spacebot_docs, task/goal tools.
- Worker: shell, file, browser, set_status (builtin) | ACP/OpenCode subprocess.
- Cortex: memory_save, memory_consolidate, system_monitor (future), task/goal.

## 8. Guardrails (harness invariants)

- **No hang:** worker wall-clock + supervisor `run_to_completion` timeout/kill;
  interactive ACP per-turn timeout; idle/stall detection.
- **No orphan:** `ChildRegistry` + kill_on_drop; workers transfer to cortex on
  channel death.
- **No silent failure:** outcome must be persisted before notify (~15 paths ->
  outbox design); `set_status` outcome-gate on worker exit.
- **No leak:** secret scrub + leak detection on tool output and outbound HTTP.
- **No workspace corruption:** file-tool guards on identity/memory paths.
- **No red unknown:** circuit breakers, `-D warnings` clippy gate, tests gate.