# Upstream PR integration — 2026-09-15

This document is the audit ledger for integrating open pull requests from
`spacedriveapp/spacebot` into `bruno320x/spacebot`, whose base is the active
`coruhoorhan/spacebot` fork.

## Policy

The goal is feature integration, not commit-count parity. A pull request is not
merged blindly when the fork already contains a newer, broader, or incompatible
implementation. Every upstream PR receives one of these outcomes:

- **MERGED** — upstream delta has been integrated into the staging branch.
- **COVERED** — the fork already contains the same fix/feature, or a stronger one.
- **PARTIALLY COVERED** — only part of the upstream behavior already exists.
- **SEMANTIC PORT** — useful behavior remains, but the upstream branch conflicts
  with newer fork code; only the behavior/invariants are ported.
- **ISOLATED** — large or experimental feature is kept on a dedicated branch
  until its own gates pass.
- **BLOCKED/WIP** — upstream itself is draft/WIP or has an unresolved dependency.
- **AUDIT REQUIRED** — a previous claim of coverage has not yet been proven by
  the current source tree and must not be treated as complete.

No runtime change is eligible for `main` until its accepted delta passes the
integration quality gates and the master integration PR is reviewed.

## Integration infrastructure

- Master staging branch: `integration/upstream-prs`
- Master PR: `bruno320x/spacebot#6`
- Preserved upstream runtime stack: `incoming/upstream-runtime-stack-649-653`
  (`bruno320x/spacebot#16`).
- Rust CI: full `preflight` + `gate-pr --ci`, metrics feature compile, lib tests,
  integration-test compile, migration safety.
- API CI: OpenAPI generation and checked-in TypeScript schema diff.
- Interface CI: `bun ci`, TypeScript typecheck and production build.
- Container CI: Docker build smoke test.
- Release: gated release build, Linux amd64/arm64, macOS amd64/arm64, GHCR
  multi-arch image and SHA-256 release checksums. Windows is intentionally not
  advertised until the conflicting Windows daemon/sidecar work is ported and
  proven against this fork.
- Controlled port engine: `upstream-port.yml` applies the exact upstream PR
  `base_sha..head_sha` delta with Git three-way apply. Clean results go to
  `port/*`; conflicts go to `diagnostics/*`. It can never write directly to
  `main`.

GitHub Actions is now enabled and running on the fork. The hardened CI entered
`main` in commit `2cee04a`; the first real run has already passed interface
TypeScript + production build and project preflight while the remaining gates
continue to execute.

## Verified architectural findings

1. **#543 is covered.** The current `cortex.md.j2` already has a stronger
   Decision Provenance contract than the upstream PR. Test PR #1 was closed as
   superseded.
2. **#324 is covered.** Current `SecretScanMode` implements `strict`,
   `own_secrets_only`, and `disabled` with shared scrub behavior.
3. **#545 is covered.** Current ingestion code wires
   `MemoryPersistenceContractState` into `SpacebotHook`.
4. **#604 is only partially covered.** Retry/backoff/quarantine is present, but
   the chunk still fails when the model does not call
   `memory_persistence_complete`; upstream #604 deliberately changes successful
   agent return to be the deterministic completion signal. That missing behavior
   must be ported. Source-file deletion/purge and canonical merge are audited
   separately before the PR can be marked covered.
5. **#650 is not covered.** Current autonomy still creates one channel per run
   with soft/hard timeout behavior. Upstream #650 changes this to a resident
   autonomy channel with durable epochs and restart reconciliation.
6. **#653 is not covered.** Current dispatch still uses channel-owned
   `worker_handles` as the active-worker source of truth and resumes workers
   back into channel state. Upstream #653 moves lifecycle/control to an
   agent-owned registry.
7. The fork's `supervisor.rs` is valuable but orthogonal to #653: it supervises
   OS subprocesses (`ChildRegistry`, timeouts, process-tree termination), not the
   logical worker-control registry proposed by #653.

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
| #208 | Podman updater socket support | SEMANTIC PORT; direct branch conflicts |
| #254 | ChatGPT OAuth/Codex reliability | SEMANTIC PORT; fork LLM layer changed heavily |
| #269 | Webchat reactions | SEMANTIC PORT; direct branch conflicts |
| #287 | Cortex topic synthesis | SEMANTIC PORT; memory/cortex overlap |
| #294 | Workspace filesystem explorer | PORT/SECURITY REVIEW |
| #324 | Configurable secret scanner | **COVERED** by current `SecretScanMode` |
| #326 | Telegram forum topic isolation | SEMANTIC PORT; direct branch conflicts |
| #367 | Direct background-result relay | SEMANTIC PORT; superseded in part by newer relay work |
| #375 | Hierarchical AGENTS.md | **MERGED** into integration branch via #3 |
| #382 | Robust STT routing/transcription | PORT; reconcile with #177 |
| #401 | Flush memory persistence on shutdown | SEMANTIC PORT; exact shutdown flush not found in current channel source |
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
| #519 | Cron bypass listen-only suppression | SEMANTIC PORT; exact sender-id bypass not present; direct branch conflicts (#2) |
| #534 | Shell pre-execution risk analysis | PORT/SECURITY REVIEW |
| #542 | Read-only sandbox paths | SEMANTIC PORT; `SandboxConfig` still lacks `readable_paths` |
| #543 | Decision provenance | **COVERED** by stronger fork implementation; test PR #1 closed |
| #544 | Discord channel topic context | PORT |
| #545 | Ingestion persistence-contract wiring | **COVERED**; hook wiring present |
| #547 | Email backfill age limit | PORT |
| #550 | Agent channel config application | **AUDIT REQUIRED**; earlier fork-plan claim is not sufficient proof |
| #551 | Interactive worker idle/outcome loop | SEMANTIC PORT/COVERED in part by fork worker lifecycle fixes |
| #552 | Provider/worktree/UTF-8/LiteLLM fixes | SEMANTIC PORT; several fixes may already be present |
| #554 | Live context/token usage | PORT |
| #556 | Per-call LLM timeout/activity tracking | PARTIALLY COVERED by fork LLM resilience/supervision; verify branch tracking semantics |
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
| #598 | Stick-to-bottom chat behavior | SEMANTIC PORT; direct branch conflicts (#11) |
| #600 | Configurable Enter behavior | PORT only after current `@spacedrive/ai` API is verified |
| #602 | Mobile-friendly dashboard | PORT/compare with #121/#419 |
| #604 | Ingestion reliability / merge bloat | **PARTIALLY COVERED**; deterministic completion still missing |
| #607 | Microsoft Teams adapter | PORT |
| #608 | Microsoft Teams settings UI | PORT after #607; upstream PR is stacked/draft |
| #613 | Task/DAG delegation and routines | **BLOCKED/WIP**; upstream explicitly says parts need rollback |
| #637 | Chronicle rollups | HIGH PRIORITY SEMANTIC PORT; direct branch conflicts (#12) |
| #639 | Durable task comments/enrichment | HIGH PRIORITY SEMANTIC PORT; direct branch conflicts (#14) |
| #641 | Durable skill reflection records | HIGH PRIORITY SEMANTIC PORT; direct branch conflicts (#13) |
| #649 | Coding-worker context/observability | HIGH PRIORITY SEMANTIC PORT; direct branch conflicts (#15) |
| #650 | Resident autonomy channels | **REQUIRED SEMANTIC PORT**, depends on #649; verified missing |
| #651 | Autonomy toggle commands | HIGH PRIORITY SEMANTIC PORT, depends on #650 |
| #652 | Reliable background result delivery | HIGH PRIORITY SEMANTIC PORT, depends on #651 |
| #653 | Agent-owned worker registry | **REQUIRED SEMANTIC PORT**, depends on #652; verified missing |

## Stack rules

The worker/autonomy stack must be integrated in this order:

`#649 -> #650 -> #651 -> #652 -> #653`

The current fork already introduced ACP workers, TaskMode, evidence-gated
completion, verify-after-work and subprocess supervision. Any port from this
stack must preserve those invariants. In particular, `supervisor.rs` must remain
for OS-process safety even after logical worker ownership moves out of channels.

The memory/runtime sequence is handled after worker lifecycle stabilizes:

`#578 -> #637 -> #639 -> #641`, with `#577`, the missing portions of `#579`,
and the missing portions of `#604` applied where compatible.

`#580` is never merged as a side-effect of unrelated commits. Its code-graph
delta is isolated from the extra historical commits carried by its source branch.
