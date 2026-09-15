# Autonomy

Spacebot has all the primitives for autonomous operation but no loop that ties them together. Tasks can be claimed. Workers can be spawned. Memories can be saved. Goals can be stored. The cortex observes everything. But none of this fires without a user message.

This doc defines the autonomy system: how Spacebot wakes up, what it sees, what it can do, and how state is tracked — without a heartbeat.json or heartbeat.md.

---

## Philosophy

Most agent harnesses implement "heartbeat" as: run a prompt on a timer, let the LLM figure out what to do, persist state to a markdown or JSON file for next time.

This has a well-known failure mode: the agent writes malformed JSON, overwrites fields it shouldn't, or drifts from the schema over time. The Spacebot approach inverts this: **state lives in structured storage, not in files the LLM writes.** The database is the source of truth. State is tracked through tasks and the working memory event log. The LLM reads from structured queries and writes through typed tools. There is no heartbeat.md and no heartbeat.json.

---

## The Autonomy Channel

<<<<<<< ours
> **Superseded in part by [autonomy-lifecycle.md](autonomy-lifecycle.md).**
> That doc replaces the fixed interval with run-scheduled wakes, splits the
> run into a deliberation phase and an action phase, moves ready-task pickup
> off the cortex and into the run, and adds explicit task enrichment state.
> The philosophy, level semantics, wake model, and briefing-as-system-prompt
> decisions below still hold.
=======
The autonomy channel starts with the agent and remains alive until shutdown. It is not per-task and not per-heartbeat. It waits while idle and processes one event at a time. Interactive worker controls live in the agent registry.
>>>>>>> theirs

The autonomy channel is the agent's process for self-directed work. It is not per-task. It is one channel that wakes on a configured interval, surveys the task state, does as much enrichment and preparation as useful, executes ready tasks if any exist, and exits. On the next interval it wakes again.

<<<<<<< ours
The interval is the default trigger, not the only one. Wake events — schedules, webhooks, internal events like a task approval or a user comment, and idle/staleness conditions — pull the next run forward and appear in its context with their payloads and instructions. The channel is the single consumer of all wake sources; see [`wakes.md`](wakes.md) for the trigger model, queue semantics, and authority rules.

It is structurally similar to a cron channel — periodic, no user present, full agent context. The difference is that it is persistent across runs and has awareness of its own history.
=======
The resident channel and an autonomy run have different lifetimes. The agent registry owns live worker controls. A run is a durable decision epoch with a run id, claimed wake events, operation-scoped child attribution, and a terminal summary. `autonomy_complete` closes the epoch and returns the channel to idle.
>>>>>>> theirs

The autonomy channel is the only process that:
- Enriches and researches `pending_approval` tasks without a user present
- Executes `ready` tasks (user-approved) without a user present
- Creates new tasks as part of its run
- Maintains a run history via `autonomy_complete`

It does **not** branch. Branches exist to keep memory tool calls out of user-facing channel context. The autonomy channel has no user context to protect — it uses tools directly.

---

## Context on Wake

The cortex assembles the autonomy channel's context before each wake. It gets:

- **Identity** — SOUL.md, IDENTITY.md, ROLE.md. The agent knows who it is.
- **Memory bulletin** — the cortex's current knowledge synthesis.
- **Working memory** — recent system events. What's been happening across all channels.
- **Wake events** — what pulled this run forward, if anything: the wake's name, instructions, and payload for each pending event since the last run. Surfaced first, because they are usually why the run exists.
- **Task state** — all active tasks: ready, in-progress, backlog, pending_approval. Full detail on each, including all comments.
- **Goals** — all active goals with descriptions and notes. Background context and direction, not a work queue. See [`goals.md`](goals.md).
<<<<<<< ours
- **Active workers** — what's currently running so it doesn't duplicate work.
- **Its own prior transcript** — the channel's persisted history, where each finished run has compacted to its `autonomy_complete` summary. This is the continuity mechanism; see [Continuity Between Runs](#continuity-between-runs).

Continuity arrives as the channel's own history rather than as injected run summaries — the run store stays the queryable index and provenance record, not a second delivery path for the same content. The autonomy channel wakes with spatial awareness of where things stand and what it did recently.

### The briefing is the system prompt, not a message

All of the above renders into the channel's system prompt, re-rendered on each wake. It is not delivered as an inbound message.

This matters because the autonomy channel is one conversation that spans every run — a single conversation id, one persistent transcript. Anything delivered as a message is written into that transcript and stays there. A briefing sent as a message means run fifty wakes to forty-nine copies of its own instructions, with its actual work crowded out between them. Rendering the same content as the system prompt costs nothing extra, since the cortex already assembles it fresh each wake, and it leaves no residue. It is also the honest representation: a briefing stored with a user role and a system sender describes a participant who does not exist.
=======
- **Workers** — registry-backed liveness and routability, plus durable nonterminal rows that require reconciliation.
- **Recent epoch summaries** - compact continuity from `autonomy_complete`.

The run store is the continuity index and provenance record. Live history belongs to the current epoch and is cleared before the next one. Retained worker controls are agent runtime state, not transcript state.
>>>>>>> theirs

Two things legitimately arrive mid-run and so must be messages: the soft wrap-up warning at `warn_secs` and the hard timeout notice at `timeout_secs`. Both are ephemeral — present in the live run's context, never persisted to the transcript. They are scaffolding for one run, not history.

The rule the channel is built around: **the transcript holds the agent's own output.** Everything the system tells it is either the system prompt, re-rendered per wake, or an ephemeral mid-run injection. Nothing the system says accumulates.

### Journaled events

That rule governs scaffolding. It does not exclude the agent's own actions — and some of those leave the transcript entirely. When a run sends a message to the home channel, the effect lands on a human, not in the conversation the run is having with itself. The next wake has no way to know it happened.

So actions with effects outside the transcript are journaled into it as they occur:

```text
Sent to home channel (telegram:8659410676):
"Found three open issues on the repo you cloned — the oldest looks like a quick win."
```

**These are recorded as the agent's own turn.** Not as a system row: `log_system_message` persists `role = "system"`, and rehydration keeps only `user` and `assistant` rows (`render_conversation_history_backfill`). A system row is visible in the dashboard and invisible to the agent, which is precisely backwards for a journal entry — the dashboard is not who needs to read it.

What qualifies is decided by one test: **journal only what the next wake cannot re-derive.** Task state, goals, worker status, and run counts are all re-rendered into the system prompt on every wake, so journaling them duplicates content that is already arriving fresh. An outbound message fails that test on both counts — nothing regenerates it, and it cannot be undone. The agent has to know it already spoke.

Held to that test, the set stays small: things a human perceived, and writes to the world outside the agent. Internal state transitions stay out. The failure mode to avoid is a transcript that degrades from a train of thought into a syslog, which is the same pollution the briefing rule exists to prevent, arriving through a different door.

**Journaling is independent of waking.** Two axes that happen to share a vocabulary:

| | Wakes the agent | Appears in the transcript |
|---|---|---|
| Registered as a wake trigger | yes | yes, via the run it causes |
| Journal-only | no | yes, on the next wake |
| Neither | no | no |

An event is journaled because the agent needs to remember it, and it triggers a wake because it needs acting on *now*. Most things are one or the other. This is what several declared-but-unproduced `SystemEvent` variants are reaching for: `cortex.observation` wants to be journal-only.

**Journal entries must survive compaction.** A run's detail collapses into its summary on exit, and "have I already told them this?" is a question spanning days — exactly the range compaction removes. An outbound-message record that lives only in run detail works for one wake and then silently stops. Outward-facing actions are promoted into the run summary rather than discarded with the rest of the detail.

---

## What It Does

All tasks require human approval before execution. `pending_approval` tasks are waiting for the user to review them. `ready` tasks have been approved and are waiting to be executed. The autonomy channel's primary activity — especially overnight or during long idle windows — is **enrichment**: researching, investigating, and preparing `pending_approval` tasks so they are fully reasoned when the user comes to review them.

The autonomy channel reasons about which tasks to prioritise given goal context and current system state — it is not a FIFO queue.

During a run it can:
- **Enrich pending tasks** — spawn investigation workers, reason about their findings, and add comments to tasks with synthesised results. A task that arrived as a title becomes a fully researched brief before the user ever approves it.
- **Execute ready tasks** — tasks the user has approved. Uses execution tools directly (shell, file, browser) with no forced delegation. Workers available for genuine parallelism.
- **Create new tasks** — identifies follow-on work and adds it to `pending_approval`. The agent proposes; the user decides.
- **Update task metadata** — priority, blockers, progress notes.
- **Record what it notices** — the channel holds `memory_save` and `memory_recall` directly (it does not branch, so there is no persistence branch behind it). Findings are written as they are found, not batched at the end, because a run can time out and lose them.

Recording is licensed, not quota'd. A run that genuinely learned nothing records nothing, and the briefing says so in as many words. An agent told to always produce an observation will produce one — restating what it was already given, or narrating its own activity as a discovery — and manufactured memories are worse than none, because they degrade every future recall that has to sift past them.

What it **cannot** do:
- Hold a conversation. There is no `reply` tool: the channel has no inbound turn to answer, and a user who replies to something it sent is answered by the normal user channel for that conversation. Delivery to a configured target is not conversation — see **Reaching Out** below.
- Execute tasks that are still in `pending_approval`
- Create cron jobs
- Spawn other autonomy channels

---

## The Empty Instance

A fresh instance has no tasks, no goals, and no history. The default outcome is a run that surveys nothing, concludes "nothing new here", and exits — and because nothing changed, the next wake reaches the same conclusion. An agent that idles until someone gives it work is not autonomous; it is a queue consumer with a timer.

The survey already knows when it came back empty, so the briefing branches on it rather than leaving the agent to notice. The template is already conditional on wake events, run history, goals, workers, and level; empty state is one more branch, and it fires deterministically. That matters more than it sounds: routing this through a skill the agent chooses to invoke reintroduces the exact failure being fixed, because the run that fails to reach for the skill is indistinguishable from the run that had nothing to do.

The empty branch is built on one claim: **on an empty instance, learning the user and the system is the highest-value work available, not filler while waiting for real work.**

- **Read what is actually here.** The workspace, registered projects, whatever the user has already done. A cloned repository is a statement of intent.
- **Record what it learns**, under the rules above.
- **Find capability gaps** via `spacebot_docs` — features that fit what the user appears to be doing and that they have not set up.
- **Ask one good question.** If there is a single thing the user could say that would unlock the most, ask that.

The last one is the point. A question that gets answered converts an empty instance into a non-empty one and compounds into every later run. Ten manufactured observations compound into nothing. When the empty branch is deciding what is worth doing, one good question outranks a full survey of an empty system.

`spacebot_docs` is currently registered only on the branch and cortex tool servers (`create_branch_tool_server`, `create_cortex_tool_server`), not in `add_direct_mode_tools` — which is what the autonomy channel receives, and it does not branch. It has to be added there before any of this is reachable.

---

## Reaching Out

At `suggest` and above, a run may send to the home channel ([`home-channel.md`](home-channel.md)). This is the one place autonomous work becomes visible to a human without them going looking, so the bar is deliberately high — an agent that reports in every interval gets muted, and a muted agent is worth less than a silent one.

Send when:

- It needs something only the user can provide — a decision, access, a credential, missing context that blocks otherwise-ready work.
- It found something time-sensitive, where waiting until the user next opens a channel has a cost.

Do not send to report activity. "Here is what I did this run" is what run history is for, and it is visible on demand rather than pushed.

Every send is journaled into the transcript as the agent's own turn, so the next run can see that it already raised something and decide against repeating it. That judgment is the primary control; the content-key backstop exists for loops, not for taste. An unanswered question asked twice in a week is a worse outcome than one asked once and left standing.

With the dial at `observe`, or with no home channel configured, this section does not apply — findings are recorded and nothing is sent.

---

## Task Comments

Comments are the primary output of the enrichment loop. When the autonomy channel or a worker completes investigation on a task, findings are written as a comment — not appended to the task description, not stuffed into metadata. Comments are append-only and chronological. The task description remains the stable statement of what needs to be done; comments are everything that has been learned or decided since.

Both the agent and the user can comment on a task. This makes tasks a shared workspace: the agent enriches overnight, the user wakes up to investigated briefs and can weigh in before approving.

### Schema

```sql
CREATE TABLE task_comments (
    id          TEXT PRIMARY KEY DEFAULT (lower(hex(randomblob(16)))),
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    author_type TEXT NOT NULL,   -- 'agent' | 'user' | 'worker'
    author_id   TEXT,            -- user_id for users, worker_id for workers, null for agent
    body        TEXT NOT NULL,   -- synthesised comment text (2-5 lines)
    worker_id   TEXT,            -- if this comment summarises a worker run, links to that worker
    metadata    TEXT DEFAULT '{}',
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX task_comments_task ON task_comments(task_id, created_at);
```

`worker_id` links a comment to a specific worker run. The UI renders it as a pill on the comment — click to expand the full worker output. The comment body is always the agent's synthesised 2-5 line summary; the worker output is available on demand, never inlined.

### `add_task_comment` tool

Available to the autonomy channel and to workers (via the task tools toolset).

```rust
struct AddTaskCommentInput {
    task_id: String,
    body: String,               // synthesised finding — 2-5 lines
    worker_id: Option<String>,  // tag the worker whose output this summarises
}
```

### Enrichment Pattern

> Enrichment gains an explicit task state in
> [autonomy-lifecycle.md](autonomy-lifecycle.md) §4 — `not_needed` /
> `needed` / `in_progress` / `done`, with a plan document written as a task
> attachment and a terminating rule (a run may not re-enrich a `done`
> task). The pattern below is the shipped, state-free version.

```
wake → survey pending_approval tasks
  → pick highest-priority unenriched task
  → spawn 1-3 investigation workers in parallel
  → reason about worker findings
  → add_task_comment: synthesised finding + worker_id(s)
  → repeat for next task within turn budget
  → autonomy_complete → exit
```

The autonomy channel system prompt instructs: investigate and comment freely; never execute a task still in `pending_approval`.

### Worker Briefing

When a `ready` task is eventually executed, `WorkerContextMode::Briefed` pulls the task's comments as part of the briefing synthesis alongside memory recall and working memory events. The executing worker walks in knowing what was investigated, what solution was proposed, and what the user said — without re-doing any of the research.

---

## Task Selection and Deduplication

### `last_enriched_at`

Tasks gain a `last_enriched_at` timestamp, set by the autonomy channel each time it works on a task. This prevents the same tasks from dominating every run.

```sql
ALTER TABLE tasks ADD COLUMN last_enriched_at TEXT;
```

### Task Ownership

Tasks have an `assigned_agent_id` field. Once an agent claims a task, no other agent can work on it. Ownership is enforced at the query level and set atomically when a task is first enriched or executed.

The global tasks table currently declares `assigned_agent_id TEXT NOT NULL` with assignment at creation time, so the unowned-task model requires making the column nullable. That migration touches the global database and every task consumer; it ships as its own change ahead of this system, not inside it.

```sql
-- Atomic claim: only succeeds if still unassigned or already assigned to this agent
UPDATE tasks SET assigned_agent_id = ?1
WHERE id = ?2 AND (assigned_agent_id IS NULL OR assigned_agent_id = ?1)
```

If the UPDATE affects 0 rows, another agent claimed it first — skip and move on.

`claim_unowned` is an agent-level config flag (default: `true`). Set to `false` for agents that should only work on tasks explicitly assigned to them — useful in multi-agent setups where task routing is intentional. Once claimed, ownership is permanent. Reassignment is user-initiated only.

### Selection Priority

On wake, tasks are ordered by:

1. **User-engaged since last enrichment** — tasks where a user comment exists with `created_at > last_enriched_at`. A user weighing in is the strongest signal to come back.
2. **Never enriched** — `last_enriched_at IS NULL`. Fresh tasks the agent hasn't seen yet.
3. **Stale** — `last_enriched_at < now - (interval_secs * 3)`. Tasks not touched in a while.
4. **Excluded** — `last_enriched_at > last_run_started_at`. Worked on in the most recent run — skip unless the user has commented since.

Rule 4 prevents just-worked-on tasks from floating to the top next wake. Rule 1 clears that exclusion when the user responds — natural gate, no separate flag needed.

The channel selects up to `max_tasks_per_run` from this ordered list, reasoning about which are most valuable given goal context. Not a mechanical top-N pick. All queries filter to `assigned_agent_id = this_agent OR assigned_agent_id IS NULL` (when `claim_unowned = true`).

### Run History as Context

The last few `autonomy_complete` summaries provide a second deduplication layer — even if a task slips through the timestamp filter, the channel can see "I already researched this last run" and move on.

---

## Soft Timeout

Hard timeouts waste runs — cutting off mid-task leaves work in undefined state. Spacebot uses a soft warning instead.

The cortex monitors elapsed time. At `warn_secs`, it injects an addendum into the channel's live context:

```
You have approximately 2 minutes remaining in this run.
Finish your current task, add any final comments, and call autonomy_complete.
Do not start a new task.
```

The channel wraps up gracefully and exits cleanly. The hard `timeout_secs` remains as a safety net but should almost never fire.

Delivery mechanism: a synthetic system message between turns, the same pattern the cron scheduler uses for its timeout wrap-up injection. The worker-style mid-turn injection hook (`inject_rx` on `SpacebotHook`) is wired only for workers today; giving `Channel` the same pair would let the warning land inside a live turn, but that is an enhancement, not a prerequisite.

---

## Continuity Between Runs

**Task comments** — the primary record of what has been investigated and found. Persist indefinitely. The next run sees all prior comments when it reads task state on wake, so it does not duplicate completed investigation.

**Run summaries** — on exit, `autonomy_complete` records what was enriched, what was executed, what was created, and which wake events the run consumed. The summary is persisted as the run's assistant turn in the channel transcript, and the UI renders the consumed wakes as "woken by" provenance per run.

**The summary is the compaction unit.** During a run the channel carries full detail: tool calls, worker results, intermediate reasoning. When the run ends, that detail collapses to the summary. What persists is a stream of summaries — roughly five lines per run — plus the live detail of whichever run is currently executing. The transcript is therefore a continuous record of the agent's own thinking that stays bounded no matter how many times it wakes.

This makes the transcript itself the continuity mechanism, so `run_history_count` is a compaction window rather than a second delivery path. Persisting the transcript *and* injecting the last N summaries from the runs table would feed the same content twice by two mechanisms; the run store remains the queryable index and the provenance record, not a parallel context source.

One consequence worth stating: wake provenance lives in the run store and the UI, not in the transcript. Read on its own, the transcript is uninterrupted thought with no visible cause — "why did run 47 happen" is answered by run history, not by scrolling back.

Working memory provides broader system context. The transcript provides the autonomy-specific thread.

---

## Lifecycle

> The flow below describes the shipped implementation. See
> [autonomy-lifecycle.md](autonomy-lifecycle.md) for the successor: the
> trigger becomes the previous run's requested `next_wake` (interval as
> floor and crash fallback), and the run opens with a deliberation phase
> that queries rather than reading a rendered dump.

```
Cortex tick
  → elapsed since last autonomy run >= interval_secs
    OR unconsumed wake events are pending
  → no autonomy channel currently running
  → autonomy.enabled = true
  ↓
Cortex assembles context (identity + bulletin + working memory + tasks + goals)
  → rendered into the channel's system prompt, not sent as a message
  ↓
Autonomy channel wakes with full context + its own prior transcript
  ↓
  ├─ pending_approval tasks exist?
  │    → enrich: spawn investigation workers, reason about findings, add_task_comment
  │    → repeat within turn/task budget
  │
  ├─ ready tasks exist?
  │    → execute: pick highest-priority, run with full context + briefing from comments
  │
  └─ no tasks worth acting on?
       → create pending_approval tasks from goals, or exit with "nothing to do"
  ↓
Calls autonomy_complete → summary recorded in the run store
  → and persisted as the run's assistant turn; the run's detail compacts to it
  ↓
<<<<<<< ours
Channel exits → cortex records last_run_at, cleans up
=======
Epoch closes → channel returns to idle; retained worker controls remain in the agent registry
>>>>>>> theirs
```

If the channel crashes mid-execution, the task returns to `ready`. If a task fails 3 consecutive times, it moves to `failed` and emits a working memory `Error` event. Enrichment runs (comments only) do not count as failures. `failed` is a new `TaskStatus` variant — the current set is pending_approval, backlog, ready, in_progress, done — so adding it includes the transition table, API, and UI sweep.

---

## Configuration

```toml
[autonomy]
enabled = false                    # Off by default
interval_secs = 1800               # How often to wake (seconds)
active_hours = [8, 22]             # UTC hour range — suppress wakes outside this window (optional)
max_turns = 20                     # Turn budget per run — on exhaustion, behaves like soft timeout
max_tasks_per_run = 2              # Max tasks to work on per wake
timeout_secs = 600                 # Hard timeout — last resort, should rarely fire
warn_secs = 480                    # Cortex injects soft warning at this point
run_history_count = 5              # How many past run summaries to surface on wake
claim_unowned = true               # Pick up tasks with no assigned agent
```

### Validation

Enforced at startup and on config reload — autonomy does not start if any rule is violated:

- `timeout_secs` ≤ `interval_secs` — a run cannot outlast the interval. If equal, runs are back-to-back (continuous operation) — valid but intentional.
- `warn_secs` < `timeout_secs` — warning must fire before hard timeout. Clamped to `timeout_secs - 60` if violated.
- `max_tasks_per_run` ≥ 1
- `interval_secs` ≥ 60 — minimum 1 minute, prevents accidental tight loops.

---

## Implementation

**Phase 1 — Autonomy Channel**
- `AutonomyChannelContext` builder: identity + bulletin + working memory + tasks + goals + run summaries
- `autonomy_channel.md.j2` system prompt (enrichment-first, `autonomy_complete` on exit, never execute `pending_approval`)
- Goals table migration + `goal_create`, `goal_update`, `goal_list` tools (see `goals.md`)
- Cortex: interval trigger, `last_run_at` tracking, context assembly, channel lifecycle, soft timeout addendum at `warn_secs`
- `autonomy_complete` tool + run summary storage + retrieval for run history. Intentionally distinct from `set_outcome`, which is an unpersisted last-write-wins delivery buffer with no completion contract. Model it on `memory_persistence_complete` instead, which already enforces call-the-terminal-tool-before-exit through the hook retry machinery.
- `last_enriched_at` column on tasks; atomic claim via `assigned_agent_id`; selection query with priority ordering
- Task retry/failure handling (3 strikes → `failed`, working memory error event)
- All config fields + validation

**Phase 2 — Task Comments**
- `task_comments` table migration
- `add_task_comment` tool (autonomy channel + workers)
- Task comments included in task state context on wake
- Task comments pulled into `WorkerContextMode::Briefed` pipeline
- API endpoints: list and create comments per task
- UI: chronological comments on task detail, worker pill with expandable full output

**Phase 3 — Polish**
- Autonomy UI surface: run history, last wake time, active enrichment progress
- "Quiet while active" flag: suppress autonomy wakes when user channels have been recently active
- Autonomy outcomes surfaced to relevant user channels via working memory synthesis

---

## Non-Goals

- **No talking to users.** No `reply` tool. Output goes to task comments, working memory, and `autonomy_complete`.
- **No recursive autonomy.** The autonomy channel cannot spawn other autonomy channels.
- **No identity or config modification.** Factory tools remain cortex-chat / admin only.
- **Goals are not auto-completed.** The agent proposes completion; the user confirms.
- **Per-goal active hours** are out of scope. `active_hours` applies to the whole system.
