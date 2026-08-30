# Harness

Spacebot is not a chatbot and not just a coding agent. It is a **harness**: the scaffolding that turns a naked language model into an agent that is reachable from everywhere, speaks every protocol, never hangs, and remembers. Every protocol is a door into the same brain; every mode is a dial on the same autonomy engine.

This doc is the north star for that ambition. It defines the harness concept, the mode engine, Cortex as the "what to do and when" decider, loop engineering, and the reasoning/multimodal layers that make the agent think — and verify — like a person would. It is intentionally a vision + design document, not a shipped spec; each section names the primitives that already exist and what must be built.

---

## The North Star

> Spacebot = a multi-protocol, everywhere-connected, never-stuck, deeply-remembering harness.

Three pillars:

1. **Connectivity** — talk to *anything*, be reachable from *everywhere*. Every protocol is a door into the same core.
2. **Power** — never get stuck, never silently fail to run a command. Structural guarantees, not hopes.
3. **Memory** — the best memory system. The differentiator that compounds every use.

A true harness does both directions of every protocol: it *calls out* (client) and *lets itself be called into* (server). It decides for itself when to act and how hard to push, within a trust model that keeps the user in control.

---

## Pillar 1 — Connectivity: every protocol, both directions

| Door | Direction | Status (verified in repo) |
|---|---|---|
| CLI + REPL | in/out | ✅ shipped (`src/cli/`, `cli chat` multi-turn) |
| Messaging (Discord/Telegram/Slack/Twitch/Email/Mattermost/Signal/Webhook) | remote | ✅ shipped |
| HTTP API + webhook | programmatic | ✅ shipped (axum) |
| Desktop (Tauri) | desktop | ✅ shipped (`desktop/`) |
| MCP client | external tools | ✅ shipped (`src/mcp.rs`) |
| ACP client | external coding agents | ✅ shipped (`src/acp/`) — the ACP adapter |
| **MCP server** | editors → Spacebot | ❌ to build |
| **ACP server** | editors → Spacebot | ❌ to build |
| **TUI** | terminal UI | ❌ to build (only a REPL exists) |

**Rule:** every protocol gets two directions. The client side of MCP/ACP exists; the server side (editors reaching *into* Spacebot, treating it as a tool host or as a coding agent) does not. Completing both directions is Faz 1.

---

## Pillar 2 — Power: never stuck

The repo already does serious work here — but it is scattered. This pillar makes it structural.

| Guarantee | Shipped | Build |
|---|---|---|
| Per-command wall-clock budget, kill-on-timeout, streaming, prompt suppression | `shell` tool timeout + kill, `CI=true`, `DEBIAN_FRONTEND` | Standardise a single `ProcessSupervisor`: spawn → timeout → kill → restart → stream → redacted env, shared by builtin + ACP + OpenCode subprocesses |
| Sandbox trust model | `src/sandbox.rs` backends bubblewrap/sandbox-exec/passthrough | Make "command always runs" the default. Sandbox **encloses untrusted** (remote ACP/editor) processes; trusted local agents run with power. If no sandbox backend exists, fall back to passthrough — never block. |
| Subprocess lifecycle | ACI worker `kill_on_drop` + `prompt_timeout` | Promote to a common rule for every backend via the shared supervisor |
| Loop protection | `hooks/loop_guard.rs`, `max_turns`, worker state machine | Budgets + evidence-gated completion (below) |

**Design stance:** the philosophy is *"trusted sources get powerful execution; only untrusted (remote) sources are sandboxed."* The sandbox is a boundary, not a gate. When no backend exists it degrades to passthrough, so a command *always* runs — no hard "stuck in sandbox" failure.

---

## Pillar 3 — Memory: best in class

Shipped and strong: SQLite graph + LanceDB vector/FTS + RRF hybrid search, typed memories, importance scoring, decay/prune/merge, working memory, chronicles, cortex memory bulletin.

From "good" to "best":

- **Universal access** — recall reachable from every door (editor/ACP/MCP/chat), always through a curation branch, never raw dumps into channel context.
- **Automatic enrichment** — memories extracted from conversations, worker transcripts, editor sessions (compactor + worker completion + a shared pipeline).
- **Provenance** — a memory knows which door it came from (editor / chat / code / document).
- **Code memory** — project files + docs indexed into the memory graph so the agent "remembers" across work sessions.

---

## The Mode Engine

Modes are the autonomy dial. Every task carries a `Mode` chosen by Cortex, which sets the level of permission, planning, and verification.

| Mode | What it does | Permission | Verification |
|---|---|---|---|
| **Plan** | Investigate only; no side effects. Branch + status block. | read-only | output is a plan/recommendation |
| **Plan/Review** | Produce a plan, get it reviewed/approved, only then execute. | read-only until approval | review gate between stages |
| **Goal** | Objective + verified loop: plan → worker → run tests → if failing, fix and iterate → only then "done". | normal, delegated | **evidence-gated** — tests must pass before completion |
| **By-permission** | Every risky action asks the user first. | per-action approval | approval gate on each action |
| **Vibe** | Fluid, low-risk, minimal approval. | broad, low-risk | lightweight |
| **YOLO** | Guardrails off. **Never chosen unless the user explicitly opts in** per task. | unrestricted | none — the user asked for it |

Two orthogonal dimensions fold into the mode: **permission** (who approves, and how often) and **verification** (what proof is required before "done"). Plan lives at one extreme of both; YOLO at the other.

### Logging modes v.s. loop modes

A mode is not just a permission level — it selects a *loop shape*:

- **Plan** → one-shot research loop, no writes.
- **Plan/Review** → research → present → await decision → execute loop.
- **Goal** → the classic verified ReAct/PDCA loop: *set context → reason → act with tools → observe → repeat until tests pass, bounded by budget.*
- **Vibe/YOLO** → the same loop with lower permission friction and (YOLO) no verification gate.

The loop always runs inside guardrails (budget, loop-limit, outcome gate). The mode decides how tall those guardrails are.

---

## Cortex as the Decider ("what to do, when")

Cortex currently generates the memory bulletin and observes system signals. It becomes the **task-mode decision engine**:

When a task arrives, Cortex evaluates:
- **Novelty** — is this the first time, or a known/repeated shape?
- **Risk** — does it touch prod, secrets, destructive ops, external systems?
- **Stability** — is the environment known/contained or volatile?

And decides:
- **New task** → start in **Plan** → user approves the plan → escalate to **Goal** loop.
- **Repeated / stable** → **Goal** directly.
- **Low risk, in sandbox** → **Vibe** (or **YOLO** only if the user explicitly opted in).

The decision is made **per task** and written to a `TaskMode` field on the task (analogous to `TaskWorkerType`). It is not a global setting. A task can progress along the mode ladder as it matures.

The interesting consequence: Spacebot's autonomy channel already wakes, surveys the task board, and enriches/executes. Adding a per-task mode means the same wake loop now **also** proves whether the mode still fits — and can propose an escalation/deescalation to the user.

---

## The Verified Goal Loop (self-driven e2e)

The signature behaviour: **the agent runs its own e2e tests before it says "done".**

```
goal/ready task → worker starts (builtin | opencode | acp)
  → worker edits code with shell/file tools
  → worker runs real checks: cargo test, e2e suite, typecheck
  → output is parsed — NOT taken on trust
    → pass → worker reports outcome → task closeable
    → fail → worker diagnoses, fixes, re-runs
       → within turn/budget budget; fail after N → escalate to user
```

This leans on the shipped **outcome gate** (tool-nudging): a worker cannot exit with a text-only response until it signals a terminal outcome via `set_status(kind: "outcome")`. It is extended:
- The hook **parses the outcome's evidence** (a test run that passed, a specific command + exit 0), not just the fact an outcome was declared.
- **Evidence-gated completion:** no test evidence, no victory. "Test passed" requires the actual passing test output in the outcome payload.

Cortex can also schedule this via `wakes`/`cron`: "on every push, run the e2e suite; if it breaks, spawn a worker to fix it" — a standing verification contract managed without the user needing to say anything each time.

---

## Reasoning & Multimodal ("think like a person")

**Deliberation gate (Plan/Review):** before every significant action sequence, the flow passes through a reflection branch (fork → think → return a conclusion → present for approval). Already structurally available via `branch.rs`.

**Multimodal vision (Faz 3):**
- `SpacebotModel` is text-only today. To *see* screenshots, diagrams, and (eventually) video, Spacebot needs a **vision-capable model** in the routing chain and the plumbing to put image/video content into agent context.
- Browser screenshots + tool output already produce images in the pipeline; surfacing them to a vision model turns them into ground truth the agent can reason over, instead of a passing log line.

This is the largest lift and is deliberately last. The agent reasons with tools + memory + verified loops first; vision is breadth, not the foundation.

---

## Honest Guardrail

Research across harnesses (Databricks, LangChain, Fowler, OpenAI) is unanimous on one point: **tests verify the mechanism, not the intent. Self-correction without governance becomes confidently wrong fast.** So autonomy here is always *verification-gated and guardrailed*, never blind. Budgets, outcome gates, evidence requirements, and a human approval point for anything risky are not optional conveniences — they are what makes the loop trustworthy instead of dangerous.

---

## Build Order

**Faz 0 — Solidify (current):**
1. Land the ACP adapter WITHOUT touching `main` directly: create a feature branch (`acp-adapter`), commit the work there, push the branch, and open a PR against `main`. GitHub Actions CI runs real `check`/`clippy`/`test` (no API key needed) on the PR. **Only merge to `main` after CI is green.** Nothing untested ever merges to `main`. (This sandbox cannot compile the tree — 2 GB memory limit, background builds killed — so GitHub Actions is the verification source of truth.)
2. `ProcessSupervisor` + no-hang guarantee + sandbox trust model (the user's stated priority: "no stuck in sandbox").

**Faz 1 — Finish the doors:**
1. **MCP server** (on the existing axum server + `rmcp`) — editors reach Spacebot as a tool host.
2. **ACP server** (reusing the shipped `acp/types.rs` wire types) — editors reach Spacebot as a coding agent.
3. **TUI** (ratatui) — full terminal interface.

**Faz 2 — Mode engine + verified loop:**
1. `TaskMode` per task (Plan → Plan/Review → Goal → By-permission → Vibe → YOLO).
2. Cortex as task-mode decider (novelty/risk/stability).
3. Verified Goal loop: evidence-parsing hook + evidence-gated completion ("no test evidence, no done").
4. Fetch / stand up e2e scheduling via wakes.

**Faz 3 — Reasoning breadth:**
1. Multimodal vision into the model routing + agent context.
2. Full autonomous e2e standing verification.
3. Code/docs memory indexing.

---

## Non-Goals (for now)

- YOLO as a default or a soft choice — it is always explicit and per-task.
- Vision and video as a prerequisite for anything — they are breadth, layered last.
- Replacing the user as the authority on completion — goals are never auto-completed; the agent proposes, the user confirms (see [`goals.md`](goals.md)).
- Global "one mode for everything" — modes are per-task and progress along a ladder.