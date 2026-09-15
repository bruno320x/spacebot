<p align="center">
  <img src=".github/Ball.png" alt="Spacebot" width="120" height="120" />
</p>

<h1 align="center">Spacebot</h1>

<p align="center">
  <strong>The agent harness that runs teams, communities, and companies.</strong>
</p>

<p align="center">
  <a href="https://fsl.software/">
    <img src="https://img.shields.io/static/v1?label=License&message=FSL-1.1-ALv2&color=000" />
  </a>
  <a href="https://github.com/spacedriveapp/spacebot">
    <img src="https://img.shields.io/static/v1?label=Core&message=Rust&color=DEA584" />
  </a>
  <a href="https://discord.gg/gTaF2Z44f5">
    <img src="https://img.shields.io/discord/949090953497567312?label=Discord&color=5865F2" />
  </a>

  <a href="https://deepwiki.com/spacedriveapp/spacebot">
    <img src="https://img.shields.io/static/v1?label=Ask&message=DeepWiki&color=5B6EF7" />
  </a>
</p>

<p align="center">
  <a href="https://spacebot.sh"><strong>spacebot.sh</strong></a> •
  <a href="#how-it-works">How It Works</a> •
  <a href="#chronicles">Chronicles</a> •
  <a href="#autonomy">Autonomy</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#spacebot--spacedrive">Spacedrive</a> •
  <a href="https://docs.spacebot.sh">Docs</a>
</p>

> **One-click deploy with [spacebot.sh](https://spacebot.sh)** — connect your Discord, Slack, Telegram, or Twitch, configure your agent, and go. No self-hosting required.

<p align="center">
  <img src=".github/spacebot-ui.webp" alt="Spacebot UI" />
</p>

---

Spacebot is opinionated agent infrastructure, built for teams and usable by anyone. **State belongs in structured storage, not markdown files the LLM manages.** Memory lives in a typed graph in SQLite. Conversation history compacts into durable, navigable chronicles instead of one rolling summary. Autonomy runs on a task state machine linked to goals, not a heartbeat.json. The LLM reasons. The system holds state.

**It gets smarter the more you use it.** After complex tasks, the agent captures what it learned as reusable skills. After conversations go idle, a background process silently saves skills and memories worth keeping. Every session builds on the last — without any user action.

It works out of the box and scales from one person to a whole community.

---

## The Problem

Most AI agent frameworks run everything in a single session. One LLM thread handles conversation, thinking, tool execution, memory retrieval, and context compaction, all in one loop. When it's doing work, it can't talk to you. When it's compacting, it goes dark. When it retrieves memories, raw results pollute the context with noise.

Spacebot splits the monolith into specialized processes that only do one thing, and delegate everything else.

---

## Built for Teams

No other agent harness handles concurrent multi-user conversations, shared memory across channels, and true process-level concurrency. A Discord community with hundreds of active members, a Slack workspace running parallel workstreams, a Telegram group coordinating across time zones. Spacebot handles all of it without any user waiting on another.

Solo users get the same infrastructure. Better memory, better concurrency, better structure. Everything teams rely on, for one person.

**For communities:** drop Spacebot into a Discord server. It handles concurrent conversations across channels and threads, remembers context about every member, and does real work without going dark. Fifty people can interact simultaneously. A message coalescing system detects rapid-fire bursts, batches them into a single turn, and lets the agent read the room.

**For teams:** connect it to Slack. Each channel gets a dedicated conversation with shared memory. One engineer gets a deep coding session while another gets a quick answer. Workers handle heavy lifting in the background while the channel stays responsive.

**For multi-agent setups:** run multiple agents on one instance. A community bot on Discord, a dev assistant on Slack, a research agent handling background tasks. Each with its own identity, memory, and security permissions. One binary, one deploy.

---

## How It Works

Five process types. Each does one job.

**Channels** are the user-facing LLM process. One per conversation, with soul, identity, and personality. A channel never executes tasks or searches memories directly. It branches to think, spawns workers to act, and stays responsive.

**Branches** fork from the channel's context to think. They have the full conversation history and run concurrently. The channel sees the conclusion, not the working.

**Workers** are independent processes that execute real work. A worker begins as a full fork of the channel that delegated to it — it carries the conversation context behind the task, not just a one-line description, so it acts on intent instead of guessing at it. Chronicles keep that inherited history bounded as sessions age. Fire-and-forget for one-shot tasks, or interactive for longer sessions where follow-up routes to the active worker.

**The Compactor** is a programmatic monitor (not an LLM) that watches context size per channel and triggers compaction before the channel fills up. Compaction workers run alongside without blocking. Two modes, selected by `compaction.mode`: `rolling` keeps one summary at the head of history and rewrites it under pressure, while `chronicle` cuts durable checkpoints designed for sessions that run for weeks. See [Chronicles](#chronicles).

**The Cortex** is the system's supervisor. It sees across all channels, workers, and branches, detects hung workers and stale branches, enforces timeout policy, and runs periodic memory-graph maintenance — decay, pruning, and merging near-duplicates. It's the only process with a whole-system view, and it stays out of the token budget: when there's no work to do, nothing runs.

```
User sends message
    → Channel receives it
        → Branches to think (has channel's context)
            → Branch recalls memories, decides what to do
            → Branch might spawn a worker for heavy tasks
            → Branch returns conclusion
        → Branch deleted
    → Channel responds to user

Channel context hits 80%
    → Compactor notices
        → Spins off a compaction worker
            → Worker summarizes the oldest span
            → Summary swaps in (rolling) or a checkpoint is cut (chronicle)
    → Channel never interrupted
```

For process capabilities, tool access by type, memory internals, and multi-agent isolation, see the [architecture docs](<docs/content/docs/(core)/architecture.mdx>).

---

## Chronicles

Long-running sessions used to mean one rolling summary, rewritten under pressure until the details blurred away. Chronicles replace that with a durable, navigable record.

In chronicle mode, compaction cuts **append-only checkpoints** over contiguous ranges of the transcript. Each checkpoint summarizes only the span since the last one. The live prompt carries a bounded window of recent checkpoints; older ones roll up into coarser summaries that retain their provenance — every rollup knows exactly which checkpoints and transcript ranges it covers.

The history stays navigable, not just compressed:

- **List** — the agent can enumerate its own checkpoints like a table of contents
- **Open** — pull any checkpoint's full summary back into context
- **Expand** — have a branch re-read a range of raw transcript when the summary isn't enough

Checkpoints survive restarts, cover the transcript without gaps, and render inline in the Portal timeline, so you can scrub through weeks of session history the same way the agent does.

This is also what makes worker forks practical: a channel that has been alive for a month hands its workers a bounded chronicle view, not an unmanageable transcript.

---

## Autonomy

Spacebot is built around a task system. Goals set direction. Tasks carry work. The agent executes, remembers, and improves whether or not you're present.

Every agent gets a durable operating model for autonomous work:

- **Home channel** — where the agent delivers findings and reaches you. It only speaks up when it needs a decision or finds something time-sensitive.
- **Goals** — persistent direction, not a work queue. Background context for every run.
- **Autonomy level** — `off`, `observe`, `suggest`, or `act`. You choose how far it goes without you.
- **Run history** — every run records what woke it, what it did, and a summary the next run reads first.

**Typed wakes replace the bare interval.** A wake is a named condition paired with instructions: a schedule, a webhook, a task approval, a comment, an idle condition, or an internal system event. Wake events queue while the agent sleeps; the autonomy channel wakes once, sees everything that accumulated, and acts with full context — a webhook flood becomes one run with many payloads, not many runs. Every run records which wakes caused it, so history answers "why did the agent act," not just "what did it do."

On wake, the channel has its identity, task state, goals, recent activity, and its own prior work. It enriches proposed tasks, executes approved work, and records findings as it goes. State lives in tasks — after a crash, the next wake reads task state and picks up where things left off.

**The agent proposes. You decide.** Tasks the autonomy channel creates land in `pending_approval`. Nothing runs autonomously until you approve it. When the agent needs input mid-work, the `ask` tool files a durable question that waits for your answer — hours later, on a different platform, it still correlates back.

There is no idle loop burning tokens in the background. No work means no LLM calls.

---

## What It Does

### Memory

Spacebot's memory is a typed, graph-connected knowledge system in SQLite and LanceDB. Every memory has a type, an importance score, and graph edges to related memories. The agent distinguishes facts from decisions, preferences from goals.

- **Eight memory types** — Fact, Preference, Decision, Identity, Event, Observation, Goal, Todo
- **Graph edges** — RelatedTo, Updates, Contradicts, CausedBy, PartOf
- **Hybrid recall** — vector similarity + full-text search merged via Reciprocal Rank Fusion
- **Memory-first knowledge context** — every conversation opens with a deterministic render of the memory store: top memories per type with shown-of-total counts, straight from SQLite. No LLM synthesis pass, no staleness, no idle token spend
- **Human identity anchors** — each known human maps to an anchor memory, so the agent keeps people straight across platforms and channels
- **Write-time consolidation** — duplicates get merged when memories are saved, not cleaned up later by a background loop
- **Memory import** — drop files into `ingest/` and Spacebot extracts structured memories automatically. Supports text, markdown, and PDF.

### Voice Transcription

Audio attachments (voice messages, audio files) are transcribed before being passed to the channel. Set `routing.voice` to choose the backend:

**Provider-based** — route through any configured LLM provider that supports audio input:

```toml
[defaults.routing]
voice = "openai/whisper-1"
```

**Local Whisper** (`stt-whisper` feature, requires `--features stt-whisper` at build time) — run inference locally via [whisper-rs](https://codeberg.org/tazz4843/whisper-rs), no API call needed:

```toml
[defaults.routing]
voice = "whisper-local://small"
```

The model is downloaded automatically from [`ggerganov/whisper.cpp`](https://huggingface.co/ggerganov/whisper.cpp) on first use and cached in `~/.cache/huggingface/hub`. Supported size names: `tiny`, `tiny.en`, `base`, `base.en`, `small`, `small.en`, `medium`, `medium.en`, `large`, `large-v1`, `large-v2`, `large-v3`. An absolute path to a GGML model file also works.

GPU acceleration via Vulkan is enabled automatically when a compatible device is detected. The loaded model is cached for the process lifetime — restart to switch models.

Ogg/Opus audio (Telegram voice messages) is decoded natively. All other formats are handled via symphonia.

### Skills

Skills are reusable procedures injected into worker system prompts. The agent writes them from experience — and they accumulate automatically over time.

- **Reflection** — after multi-step work succeeds, a background pass distills it into a new skill or improves an existing one. The next worker starts with the procedure instead of rediscovering it
- **Built-in skills** — workers ship with established procedures compiled into the binary, so the first run isn't a cold start
- **Categories and ranking** — skills are organized into categories and ranked by access frequency, giving the model a compact index that routes it toward the most relevant procedures
- **Typed frontmatter with origin-scoped writes** — the agent can improve skills it authored; installed and built-in skills are protected
- **AI-assisted authoring** — describe a skill in plain language, the agent generates it and shows a preview before saving
- **skills.sh registry** — install any skill from the public ecosystem with one command

```bash
spacebot skill add vercel-labs/agent-skills
spacebot skill add anthropics/skills/pdf
spacebot skill list
```

### Tasks

Tasks carry the context needed to run real work, not just a title and a status.

- **Execution plans** — each task defines its worker type, project, worktree mode, and required skills before anything runs
- **Dependency graphs** — edges model gates and stacked work, with readiness gating so blocked tasks never run early
- **Spec-driven** — descriptions are living markdown specs with requirements, constraints, and acceptance criteria, refined through conversation
- **Operator reconciliation** — task status can be corrected directly when work completes outside Spacebot, without fabricating approval history

Workers come loaded with tools for the work itself:

- **Shell** — run arbitrary commands with configurable timeouts
- **File** — read, write, and list files with auto-created directories
- **Browser** — headless Chrome automation with accessibility-tree refs. Navigate, click, type, screenshot, manage tabs
- **[OpenCode](https://opencode.ai)** — spawn a full coding agent as a persistent worker with codebase exploration, LSP awareness, and deep context management
- **[Brave](https://brave.com/search/api/) web search** — search the web with freshness filters, localization, and configurable result count

### Scheduling

Cron jobs created and managed from conversation:

- **Natural scheduling** — "check my inbox every 30 minutes" becomes a cron job with a delivery target
- **Strict wall-clock schedules** — cron expressions for exact local-time execution
- **Single delivery** — all reply calls are buffered during the run and flushed as one message when the job completes. No mid-run fragments.
- **Circuit breaker** — auto-disables after 3 consecutive failures
- **Full agent capabilities** — each job gets a fresh channel with branching and workers

### Messaging

Native adapters for Discord, Slack, Telegram, Twitch, Signal, Mattermost, Email, and Webchat, plus a generic Webhook receiver:

- **Message coalescing** — rapid-fire messages are batched into a single LLM turn with timing context
- **Slash commands** — typed commands like `/whoami` and `/pause` work identically on every platform, with explicit authority checks and atomic settings updates
- **File attachments** — send and receive files, images, and documents. Attachments are saved to the workspace and recalled by ID
- **Rich messages** — embeds/cards, interactive buttons, select menus, and polls (Discord). Block Kit and slash commands (Slack)
- **Email** — IMAP polling + SMTP delivery with TLS, UID-based dedup, allowed sender filtering, and attachment limits. Works with local bridges like Proton Bridge
- **Webchat** — embeddable portal chat with SSE streaming, per-agent session isolation
- **Per-channel permissions** — guild, channel, and DM-level access control, hot-reloadable

### Model Routing

Four-level routing picks the right model for every call. Channels get the best conversational model. Workers get something fast and cheap. Coding workers upgrade automatically. Simple user messages are downgraded to cheaper models by a sub-millisecond prompt scorer with no external calls. Voice messages route to a dedicated voice model.

Any OpenAI-compatible or Anthropic-compatible endpoint works, including Ollama for local models, Z.ai GLM models, Azure OpenAI, and custom providers. Built-in support for Kilo Gateway, NVIDIA, MiniMax, Moonshot AI, Gemini, GitHub Copilot, OpenCode Go, and more.

### MCP Integration

Connect workers to external [MCP](https://modelcontextprotocol.io/) servers for arbitrary tool access — databases, APIs, SaaS products, custom integrations. Both stdio and streamable HTTP transports. Automatic tool discovery, hot-reloadable, exponential-backoff retry so a broken server never blocks startup.

### Security

Spacebot runs autonomous LLM processes that execute arbitrary shell commands. Security is layered so no single failure exposes credentials or breaks containment.

**Credential isolation:** secrets split into system credentials (LLM API keys, messaging tokens, never exposed to subprocesses) and tool credentials (CLI tokens injected as env vars into workers). Every subprocess starts with a sanitized environment. System secrets never enter any subprocess.

- **Secret store** — credentials live in a dedicated encrypted database, referenced by alias. Plain config files never contain secrets
- **Encryption at rest** — optional AES-256-GCM with a master key derived via Argon2id, stored in the OS credential store (macOS Keychain, Linux kernel keyring), never on disk or in an env var
- **Output scrubbing** — all tool secret values are redacted from worker output before it reaches channels or LLM context. A rolling buffer handles secrets split across stream chunks

**Process containment:** shell and exec tools run inside OS-level filesystem containment. On Linux, [bubblewrap](https://github.com/containers/bubblewrap) creates a mount namespace where the filesystem is read-only except the agent's workspace. On macOS, `sandbox-exec` enforces equivalent restrictions via SBPL profiles. Enforced at the kernel level.

- **Dynamic sandbox** — toggle sandbox mode via dashboard or API without restarting
- **Workspace isolation** — file tools reject paths outside the agent's workspace. Symlinks that escape are blocked.
- **Leak detection** — secret-pattern checks at channel egress across plaintext, URL-encoded, base64, and hex encodings
- **SSRF protection** — browser tool blocks requests to cloud metadata endpoints, private IPs, loopback, and link-local addresses

---

## Gets Better with Use

Spacebot builds on itself over time through four specific mechanisms.

**Reflection turns work into skills.** After a conversation goes idle or a multi-step task succeeds, a background pass reviews what happened and distills it — new skills, improved skills, memories worth keeping. It runs with a capped turn budget, produces no user-visible output, and fires only when there's enough to learn from.

**Memory deepens with every interaction.** Each conversation adds facts, preferences, decisions, and observations to a typed graph with importance scoring and graph edges. Consolidation happens at write time, and every future conversation opens with the current state of that graph rendered into context.

**Chronicles preserve the long arc.** Sessions that run for weeks keep a navigable record of everything that happened, not a lossy rolling summary. The agent — and its workers — can always go back to the source.

**Goals drive autonomous work between conversations.** Wakes pull the autonomy channel forward, it works through approved tasks, and each run's summary is the first thing the next run reads. Nothing resets when you walk away.

Everything goes through typed tools into structured storage. Nothing drifts.

---

## Spacebot + Spacedrive

Spacebot pairs with [Spacedrive](https://github.com/spacedriveapp/spacedrive), an open-source cross-platform file manager built on a virtual distributed filesystem. Neither requires the other. When paired, Spacebot is the only agent harness with direct integration into a cross-device filesystem.

### What Pairing Enables Today

**Multi-device access:** one Spacebot instance, all your devices. Talk to your agent from your phone while a worker executes on your server. Spacedrive's P2P layer (Iroh/QUIC) routes from every device through the paired node to Spacebot. No separate SDK, no separate auth.

**Remote execution:** workers can target any device in your library. A task that needs your home server's GPU, your work laptop's local repos, or your phone's camera routes through Spacedrive's permission system to the target device. From the agent's perspective, the tool call is identical.

**File system intelligence:** every directory can carry context nodes describing what it contains and what policies apply. When the agent navigates your filesystem it gets that context, not a blind listing.

**Safe data access:** Spacedrive indexes external sources (Gmail, Slack, Obsidian, GitHub, Apple Notes, contacts, calendar, browser history) as searchable data the agent can query. Every record passes through a local prompt injection classifier (Prompt Guard 2) before reaching the agent. The agent can search your emails without a malicious email hijacking it.

### Where This Is Going

A company deploys Spacebot + Spacedrive on their infrastructure. Employees install Spacedrive on their devices and join the company library. The company agent has access to employee devices through Spacedrive's permission system, with individual-level controls. The org graph in Spacebot defines hierarchy and delegation: which agents report to which, who can approve what, how tasks flow.

An employee talks to the company agent from their MacBook. The agent knows their projects, their device, their role, and can spawn workers on any authorized machine. They switch to their personal Spacedrive library and connect to their home Spacebot, with personal data and personal context. The app is the same. The agent is different.

No other agent harness is building this. It's a category.

---

## Quick Start

### Prerequisites

- **Rust** 1.85+ ([rustup](https://rustup.rs/))
- An LLM API key from any supported provider (Anthropic, OpenAI, OpenRouter, Kilo Gateway, Z.ai, Groq, Together, Fireworks, DeepSeek, xAI, Mistral, NVIDIA, MiniMax, Moonshot AI, Gemini, GitHub Copilot, OpenCode Zen, OpenCode Go), or use `spacebot auth login` for Anthropic OAuth

### Build and Run

```bash
git clone https://github.com/spacedriveapp/spacebot
cd spacebot

# Optional: build the OpenCode embedded UI (requires Node 22+ and bun)
# Without this, OpenCode workers still work — the Workers tab shows a transcript view instead.
# ./scripts/build-opencode-embed.sh

cargo build --release
```

#### Cross-compile for ARM (aarch64)

To build for Raspberry Pi or other aarch64 Linux machines from an x86_64 host:

```bash
# Install cross (once)
cargo install cross --locked

# Build (frontend is skipped automatically)
just build-aarch64
# Output: target/aarch64-unknown-linux-gnu/release/spacebot
```

The included `Dockerfile.cross-aarch64` provides the aarch64 cross-toolchain
and sysroot libraries. CI uses native ARM runners — this is for local builds.

### Run

```bash
spacebot                      # start as background daemon
spacebot start --foreground   # or run in the foreground
spacebot stop                 # graceful shutdown
spacebot restart              # stop + start
spacebot status               # show pid and uptime
spacebot auth login           # authenticate via Anthropic OAuth
```

The binary creates all databases and directories automatically on first run. Every instance resource — agents, channels, tasks, goals, skills, memories, cron jobs, secrets — is also manageable from the CLI. See the [quickstart guide](<docs/content/docs/(getting-started)/quickstart.mdx>) for more detail.

### Authentication

Spacebot supports Anthropic OAuth as an alternative to static API keys:

```bash
spacebot auth login             # OAuth via Claude Pro/Max (opens browser)
spacebot auth login --console   # OAuth via API Console
spacebot auth status            # show credential status and expiry
spacebot auth refresh           # manually refresh the access token
spacebot auth logout            # remove stored credentials
```

OAuth tokens are stored in `anthropic_oauth.json` and auto-refresh before each API call. When OAuth credentials are present, they take priority over a static `ANTHROPIC_API_KEY`.

---

## Deploy Your Way

| Method                                 | What You Get                                                                                |
| -------------------------------------- | ------------------------------------------------------------------------------------------- |
| **[spacebot.sh](https://spacebot.sh)** | One-click hosted deploy. Connect your platforms, configure your agent, done.                |
| **Self-hosted**                        | Single Rust binary. No Docker, no server dependencies, no microservices. Clone, build, run. |
| **Docker**                             | Container image with everything included. Mount a volume for persistent data.               |

---

## Tech Stack

| Layer           | Technology                                                                                                      |
| --------------- | --------------------------------------------------------------------------------------------------------------- |
| Language        | **Rust** (edition 2024) — single binary, no runtime dependencies, no GC pauses                                  |
| Async runtime   | **Tokio**                                                                                                       |
| LLM framework   | **[Rig](https://github.com/0xPlaygrounds/rig)** v0.33 — agentic loop, tool execution, hooks                     |
| Relational data | **SQLite** (sqlx) — conversations, memory graph, tasks, goals, cron jobs                                        |
| Vector + FTS    | **[LanceDB](https://lancedb.github.io/lancedb/)** — embeddings (HNSW), full-text (Tantivy), hybrid search (RRF) |
| Key-value       | **[redb](https://github.com/cberner/redb)** — settings, encrypted secrets                                       |
| Embeddings      | **FastEmbed** — local embedding generation                                                                      |
| Crypto          | **AES-256-GCM** — secret encryption at rest                                                                     |
| Discord         | **Serenity** — gateway, cache, events, rich messages, interactions                                              |
| Slack           | **slack-morphism** — Socket Mode, events, Block Kit, slash commands                                             |
| Telegram        | **teloxide** — long-poll, media attachments, group/DM support                                                   |
| Twitch          | **twitch-irc** — chat integration with trigger prefix                                                           |
| Browser         | **Chromiumoxide** — headless Chrome via CDP                                                                     |
| CLI             | **Clap** — command line interface                                                                               |

Single binary, no server dependencies. All data lives in embedded databases in a local directory.

---

## Documentation

| Doc                                                                  | Description                                               |
| -------------------------------------------------------------------- | --------------------------------------------------------- |
| [Quick Start](<docs/content/docs/(getting-started)/quickstart.mdx>)  | Setup, config, first run                                  |
| [Config Reference](<docs/content/docs/(configuration)/config.mdx>)   | Full `config.toml` reference                              |
| [Architecture](<docs/content/docs/(core)/architecture.mdx>)          | Process types, tool access, data layer, multi-agent       |
| [Memory](<docs/content/docs/(core)/memory.mdx>)                      | Memory system design                                      |
| [Chronicles](<docs/content/docs/(core)/chronicles.mdx>)              | Durable, navigable session history                        |
| [Autonomy](<docs/content/docs/(features)/autonomy.mdx>)              | Goals, wakes, and background task execution               |
| [Goals](<docs/content/docs/(features)/goals.mdx>)                    | Persistent direction for task and autonomy work            |
| [Wakes](<docs/content/docs/(features)/wakes.mdx>)                    | Schedules, webhooks, and event-driven autonomous work     |
| [Commands](<docs/content/docs/(features)/commands.mdx>)              | Cross-platform commands and authority rules                |
| [Tasks](<docs/content/docs/(features)/tasks.mdx>)                    | Task system, specs, and execution                         |
| [Tools](<docs/content/docs/(features)/tools.mdx>)                    | All available LLM tools                                   |
| [Routing](<docs/content/docs/(core)/routing.mdx>)                    | Model routing and fallback chains                         |
| [Secrets](<docs/content/docs/(configuration)/secrets.mdx>)           | Credential storage, encryption, output scrubbing          |
| [Sandbox](<docs/content/docs/(configuration)/sandbox.mdx>)           | Process containment and environment sanitization          |
| [Cron Jobs](<docs/content/docs/(features)/cron.mdx>)                 | Scheduled recurring tasks                                 |
| [MCP](<docs/content/docs/(features)/mcp.mdx>)                        | External tool servers via Model Context Protocol          |
| [OpenCode](<docs/content/docs/(features)/opencode.mdx>)              | OpenCode as a worker backend                              |
| [Messaging](<docs/content/docs/(messaging)/messaging.mdx>)           | Adapter architecture and platform setup                   |

---

## Contributing

Contributions welcome. Read [RUST_STYLE_GUIDE.md](RUST_STYLE_GUIDE.md) before writing any code, and [AGENTS.md](AGENTS.md) for the full implementation guide.

1. Fork the repo
2. Create a feature branch
3. Install `just` (https://github.com/casey/just) if it is not already available (for example: `brew install just` or `cargo install just --locked`)
4. Run `./scripts/install-git-hooks.sh` once (installs pre-commit formatting hook)
5. Make your changes
6. Run `just preflight` and `just gate-pr`
7. Submit a PR

### SpaceUI (Frontend Components)

The dashboard uses [`@spacedrive/*`](https://github.com/spacedriveapp/spaceui) packages from npm. For local development with linked packages, see [CONTRIBUTING.md](CONTRIBUTING.md).

Formatting is still enforced in CI, but the hook catches it earlier by running `cargo fmt --all` before each commit. `just gate-pr` mirrors the CI gate and includes migration safety, compile checks, and test verification.

---

## License

FSL-1.1-ALv2, [Functional Source License](https://fsl.software/), converting to Apache 2.0 after two years. See [LICENSE](LICENSE) for details.
