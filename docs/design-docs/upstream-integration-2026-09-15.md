# Upstream PR integration — 2026-09-15

This document is the audit ledger for integrating open pull requests from
`spacedriveapp/spacebot` into `bruno320x/spacebot`, whose base is the active
`coruhoorhan/spacebot` fork.

## Policy

The goal is feature integration, not commit-count parity. A pull request is not
merged blindly when the fork already contains a newer, broader, or incompatible
implementation. Every upstream PR receives one of these outcomes:

- **MERGED** — upstream branch can be merged safely into the integration branch.
- **COVERED** — the fork already contains the same fix/feature, or a stronger one.
- **SEMANTIC PORT** — useful behavior remains, but the upstream branch conflicts
  with newer fork code; only the behavior/invariants are ported.
- **ISOLATED** — large or experimental feature is kept on a dedicated branch
  until its own gates pass.
- **BLOCKED/WIP** — upstream itself is draft/WIP or has an unresolved dependency.

No `main` merge is permitted until accepted changes pass the integration quality
gates and the master integration PR is reviewed.

## Integration infrastructure

- Master staging branch: `integration/upstream-prs`
- Master PR: `bruno320x/spacebot#6`
- Rust CI: full `preflight` + `gate-pr --ci`, metrics feature compile, lib tests,
  integration-test compile, migration safety.
- API CI: OpenAPI generation and checked-in TypeScript schema diff.
- Interface CI: `bun ci`, TypeScript typecheck and production build.
- Container CI: Docker build smoke test.
- Release: gated release build, Linux amd64/arm64, macOS amd64/arm64, GHCR
  multi-arch image, release checksums. Windows is accepted only after the Windows
  daemon/sidecar port compiles against the current fork.

> GitHub Actions is currently not starting workflow runs on this fork. The
> workflow definitions are committed, but this repository/account-level Actions
> setting must be enabled before CI evidence can be produced. This does not make
> an untested port eligible for `main`.

## Open upstream PR ledger

| PR | Subject | Outcome / current state |
|---:|---|---|
| #15 | Ollama Cloud provider | PORT |
| #16 | Workers live panel / telemetry | PORT after #17 review |
| #17 | Core runtime refactor | SEMANTIC PORT; broad overlap with fork runtime |
| #121 | Mobile browser feel + frontend test infra | PORT/compare with #419/#602 |
| #152 | External browser containers via CDP | PORT |
| #177 | Local Whisper + worker transcription | PORT; reconcile with #382 |
| #200 | Windows daemon + named-pipe IPC | SEMANTIC PORT; direct branch conflicts |
| #208 | Podman updater socket support | PORT |
| #254 | ChatGPT OAuth/Codex reliability | SEMANTIC PORT; fork LLM layer changed heavily |
| #269 | Webchat reactions | PORT |
| #287 | Cortex topic synthesis | SEMANTIC PORT; memory/cortex overlap |
| #294 | Workspace filesystem explorer | PORT/SECURITY REVIEW |
| #324 | Configurable secret scanner | COVERED by fork implementation |
| #326 | Telegram forum topic isolation | PORT |
| #367 | Direct background-result relay | SEMANTIC PORT; superseded in part by newer relay work |
| #375 | Hierarchical AGENTS.md | **MERGED** into integration branch via #3 |
| #382 | Robust STT routing/transcription | PORT; reconcile with #177 |
| #401 | Flush memory persistence on shutdown | SEMANTIC PORT; fork contains related lifecycle fixes |
| #410 | ChatGPT reasoning effort mapping | PORT/compare with fork model routing |
| #414 | Reconstruct OpenAI Responses SSE | PORT/compare with fork streaming resilience |
| #419 | Responsive web UI retrofit | PORT/compare with #121/#602 |
| #430 | Anthropic OAuth | PORT |
| #474 | Searchable model dropdown/provider detection | PORT |
| #475 | Worker browser/compaction fixes | SEMANTIC PORT |
| #476 | Project directory browser | PORT |
| #490 | Chat input lag during SSE | PORT |
| #511 | Copilot provider API fixes | PORT |
| #512 | Flatten webchat rich cards | PORT |
| #519 | Cron bypass listen-only suppression | SEMANTIC PORT; direct branch conflicts (#2) |
| #534 | Shell pre-execution risk analysis | PORT/SECURITY REVIEW |
| #542 | Read-only sandbox paths | PORT |
| #543 | Decision provenance | **COVERED** by stronger fork implementation; test PR #1 closed |
| #544 | Discord channel topic context | PORT |
| #545 | Ingestion persistence-contract wiring | COVERED by fork ingestion changes |
| #547 | Email backfill age limit | PORT |
| #550 | Agent channel config application | COVERED by fork implementation |
| #551 | Interactive worker idle/outcome loop | SEMANTIC PORT/COVERED in part by fork worker lifecycle fixes |
| #552 | Provider/worktree/UTF-8/LiteLLM fixes | SEMANTIC PORT; several fixes already present |
| #554 | Live context/token usage | PORT |
| #556 | Per-call LLM timeout/activity tracking | COVERED by fork LLM resilience/supervisor work |
| #558 | Windows desktop/sidecar fixes | SEMANTIC PORT; direct branch conflicts (#5) |
| #561 | Hierarchical multi-agent behavior | SEMANTIC PORT; combine with TaskMode/evidence gates |
| #563 | Native Gemini provider | PORT |
| #571 | Cortex one-shot synthesis hardening | PORT/compare with current cortex |
| #572 | Redact persisted working-memory text | PORT/verify fork scrub path |
| #575 | Task update delta summaries | PORT |
| #576 | Memory architecture docs | PORT after memory behavior settles |
| #577 | Stable prompt-cache boundary | PORT |
| #578 | Active recall branch | HIGH PRIORITY SEMANTIC PORT |
| #579 | Memory triage hardening/proof tests | SEMANTIC PORT |
| #580 | Code Graph Memory | **ISOLATED**; very large feature, dedicated branch/gates |
| #582 | Retry retrigger relay delivery | SEMANTIC PORT/COVERED in part by fork messaging retry |
| #595 | OpenCode theme propagation + themes | PORT |
| #598 | Stick-to-bottom chat behavior | PORT |
| #600 | Configurable Enter behavior | PORT only if current `@spacedrive/ai` exposes dependency |
| #602 | Mobile-friendly dashboard | PORT/compare with #121/#419 |
| #604 | Ingestion reliability / merge bloat | COVERED by fork retry/quarantine/canonical merge work |
| #607 | Microsoft Teams adapter | PORT |
| #608 | Microsoft Teams settings UI | PORT after #607; upstream PR is stacked/draft |
| #613 | Task/DAG delegation and routines | **BLOCKED/WIP**; upstream explicitly says parts need rollback |
| #637 | Chronicle rollups | HIGH PRIORITY SEMANTIC PORT |
| #639 | Durable task comments/enrichment | HIGH PRIORITY SEMANTIC PORT |
| #641 | Durable skill reflection records | HIGH PRIORITY SEMANTIC PORT |
| #649 | Coding-worker context/observability | HIGH PRIORITY SEMANTIC PORT, stack base |
| #650 | Resident autonomy channels | HIGH PRIORITY SEMANTIC PORT, depends on #649 |
| #651 | Autonomy toggle commands | HIGH PRIORITY SEMANTIC PORT, depends on #650 |
| #652 | Reliable background result delivery | HIGH PRIORITY SEMANTIC PORT, depends on #651 |
| #653 | Agent-owned worker registry | HIGH PRIORITY SEMANTIC PORT, depends on #652 |

## Stack rules

The worker/autonomy stack must be integrated in this order:

`#649 -> #650 -> #651 -> #652 -> #653`

The current fork already introduced ACP workers, TaskMode, evidence-gated
completion, verify-after-work and its own supervisor. Any port from this stack
must preserve those invariants.

The memory/runtime sequence is handled after worker lifecycle stabilizes:

`#578 -> #637 -> #639 -> #641`, with `#577` and the remaining `#579` hardening
applied where compatible.

`#580` is never merged as a side-effect of unrelated commits. Its code-graph
delta is isolated from the extra historical commits carried by its source branch.
