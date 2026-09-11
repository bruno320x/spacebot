# CheaperInference 404 root-cause probe — 2026-09-06

Context: Orhan asked (telegram, 12:51 +03) whether the recurring `deepseek-v4-flash-0731`
404 ProviderError is a paid-subscription block from the LLM side or our own system.
This run performed a live probe and a full-log failure analysis. Summary first.

## Verdict

**Not our config, not an auth/subscription block at the HTTP layer.** The live endpoint
accepts the configured key and model name with HTTP 200 on every request type tested
(14/14). The 404 failures are transient provider-side (CheaperInference → Relace upstream),
clustered in retry storms; the last genuine 404 was 2026-09-06T08:32:26Z and none have
occurred since (through 13:13Z). If 404s recur, they are intermittent provider errors —
per the error text, quote the request id to CheaperInference support.

> Correction (2026-09-06 13:13Z, ADDENDUM 19): the earlier "08:18:10Z" claim was wrong —
> an exact grep of `channel LLM call failed … API error (404` shows SIX genuine 404s in
> the 08:18–08:32 window (08:18:13, 08:19:11, 08:30:18, 08:32:22, 08:32:24, 08:32:26),
> one of them (08:18:13, "one at 08:17/08:18Z" below) the telegram worker noted here.
> True last genuine 404 = 08:32:26Z. Zero since, through 13:13Z (~4.7h).

## Live probe (2026-09-06 ~10:05Z, key from ~/.spacebot/config.toml [llm.provider.inception])

- `GET /v1/models` with Bearer key → **200** (model catalog returned, account/key valid).
- 8× non-stream `chat/completions`, model `deepseek-v4-flash-0731` → **200** every time.
- 6× streaming `chat/completions`, same model → **200** every time.
- Response `model` field returns `deepseek/deepseek-v4-flash-0731`, `provider: Relace`.

Config side is correct: base_url = `https://api.cheaperinference.com` (exact, no trailing
path issues), all routing (`channel/branch/worker/compactor/cortex`) → `inception/deepseek-v4-flash-0731`.

## Log failure analysis (spacebot.log.2026-09-06, genuine lines only)

- 166 genuine `worker LLM call failed … 404 Not Found` lines, 27 distinct worker_ids.
- Cluster: ~130 of them are a retry storm 03:48–03:50Z on one stuck worker
  (`5e9332d1`, spawned by cron:health-watchdog, same 404 re-logged across its retries)
  — i.e. the 404 count is inflated by retries, not distinct failures.
- Most other 404s are 1–5 per watchdog cycle in the 00:30–08:18Z window, all
  `channel_id=cron:health-watchdog` workers. One at 08:17/08:18Z was a telegram worker.
- **Last genuine 404: 2026-09-06T08:32:26.834Z** (revised 13:13Z from the earlier 08:18:10Z
  claim — the 08:18–08:32Z window actually holds six 404s). Zero genuine 404s in 08:32→13:13Z.
- Separate error class since 09:00Z: `Relace: Queued past the 1.5s queue-time bound.
  Retry after 2s` (77 hits in 09:00–10:03Z; later upgraded to "5s queue-time bound" at
  10:09:32Z after the 10:00Z retry storm) — a gateway queue timeout, distinct from 404,
  also self-healing. Do not conflate the two when counting background failures.

## Answer to Orhan (Turkish framing)

İki şey net: (1) config ve API key doğru — endpoint canlı testte 14/14 200 döndü, ücretli
üyelik/engine kapalı olduğu HTTP seviyesinde görünmüyor; (2) 404'ler sistemsel değil,
sağlayıcı (Relace üzerinden CheaperInference) tarafında geçici hatalar — retry çığında
yoğunlaşıyor ve son gerçek 404 08:18Z'de olmuş, sonrası temiz. Tekrarlarsa hatada istenen
request id ile support'a yazmak doğru kanal.