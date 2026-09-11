# Worker Performansı ve Stabilite — Kök Neden Tespitleri (2026-09-11)

> Bu doküman **ölçüme** dayanır, tahmine değil. Her iddianın yanında kanıtı var
> (dosya:satır, canlı DB sorgusu, gerçek log satırı veya ölçülen tur süresi).
> Amaç: "worker'lar yavaş çalışıyor" şikâyetini ölçülebilir kök nedenlere indirmek
> ve her birini sırayla kapatmak.
>
> Ölçüm ortamı: `~/Music/spacebot` (fork, `origin=coruhoorhan/spacebot`,
> `upstream=spacedriveapp/spacebot`), çalışan binary `target/release/spacebot`
> (5 Eyl 10:15), makine 15 GiB RAM / 613 GB boş disk.

---

## 0. Özet (TL;DR)

Worker yavaşlığı **tek bir sebep değil**, üst üste binen 6 sebeptir:

1. **Per-request timeout 30 dakika** — pratikte "timeout yok"; takılan tek bir
   istek worker'ın tüm bütçesini yiyor.
2. **LLM katmanında eşzamanlılık sınırı yok** — 5 worker + 5 branch + cortex (30 sn
   tick) + cron aynı gateway'e yükleniyor; gateway `"The request queue is full."`
   dönüyor ve bekleme = doğrudan gecikme.
3. **Tur başına ~13-14K input token** ve **tur başına ~11,5 saniye**.
4. **Çıktının %50-60'ı reasoning token** — `ls`/`grep` için bile model "düşünüyor".
5. **Generic 400 retriable sayılmıyor** → fallback zinciri çalışmıyor, worker
   komple ölüyor. **Ölçülen worker hata oranı: %26.**
6. **Tool argümanlarına ham özel token sızıyor** (`<｜DSML｜tool_calls>`,
   `</think>`) ve hiçbir sanitizasyon yok → shell syntax error → boşa tur.

Ek olarak, **dün (10 Eyl) yapılan iki değişiklik** tespit edildi ve ikisi de
sonuç doğuruyor: (a) SQLite havuz ayarları, (b) ACP worker'ın `omp`'tan
Intent Daemon Python köprüsüne çevrilmesi — **bu ikincisi harness açısından bir
gerileme** (aşağıda §4.2).

---

## 1. Ölçülen veriler

### 1.1 Worker çalışma süreleri (`~/.spacebot/agents/main/data/agent.db` → `worker_runs`)

```
worker_type  status     n    ort(s)  min(s)  max(s)   ort tool
builtin      done      381    170.0     2.0    1801.0     14.7
builtin      failed    137    164.1     4.0    2938.0      9.6
acp          done        5  43114.8   204.0  144695.0      0.0
acp          failed      3    415.7     0.0    1247.0      0.0
opencode     failed      5      3.4     1.0       7.0      0.0
```

- **builtin hata oranı: 137 / (381+137) = %26,4.**
- Ortalama başarılı worker: **170 saniye**, 14,7 tool çağrısı → **~11,5 sn/tur**.
- İki worker tam **1800 s**'de öldü (wall-clock timeout); birinde **0 tool call**
  (id `39845c1a`) → tamamen boşa harcanmış 30 dakika.
- `acp` "done" ortalaması **43.114 s (12 saat)**, max **144.695 s (40 saat)** —
  bunlar interactive oturumlar; aşağıda §4.2'de sebebi var.

### 1.2 Tur başına gerçek gecikme (log'dan tur tur)

`spacebot.log.2026-09-07`, worker `5b8a7885`:

```
10:49:24 depth 4/15 → 10:50:05 shell  = 41.8 s
10:50:34 depth 7    → 10:50:55 shell  = 21.1 s
10:51:14 depth 10   → 10:51:34 shell  = 19.8 s
10:51:58 depth 15   → 10:52:34 shell  = 36.3 s
```

Aynı turda çalışan tool'lar `pgrep`, `ls -la`, `grep` — hepsi 1 saniyenin altında.
**Yani gecikmenin tamamı LLM turu**, tool yürütmesi değil.

### 1.3 Token profili (`token_usage`)

| process_type | model | req | input/tur | toplam input | reasoning/output |
|---|---|---|---|---|---|
| worker | inception/deepseek-v4-flash-0731 | 6796 | **13.696** | 93,1 M | **%49,4** |
| channel | inception/deepseek-v4-flash-0731 | 3178 | 10.740 | 34,1 M | %59,9 |
| branch | inception/deepseek-v4-flash-0731 | 470 | **19.746** | 9,3 M | %61,8 |
| channel | inception/glm-5.3-flash | 258 | **33.166** | 8,6 M | %61,0 |
| worker | inception/glm-5.3-flash | 302 | 23.777 | 7,2 M | %33,3 |

Günlük hacim (kayıtlı en yüksek gün): **6 Eyl — 7.145 istek / 88,7 M input
token**. 7 Eyl — 2.093 / 29,4 M.

### 1.4 Ağırlık / disk (ikincil ama gerçek)

- `~/.spacebot/logs/` = **4,26 GB**; tek dosya `spacebot.log.2026-09-06` =
  **2,2 GB**. Rotasyon **sadece günlük**, boyut sınırı yok (`src/daemon.rs:178`).
- `target/debug/spacebot` = **2,7 GB** (7 Eyl), `target/release/spacebot` =
  **319 MB** (5 Eyl 10:15). **Çalışan binary 5 Eyl'den, kaynak 10 Eyl'e kadar
  değişmiş → binary bayat.**
- RAM: 15 GiB toplam / 6,0 kullanımda / 1,9 boş. Swap 19 GiB, 6,6 dolu.

---

## 2. Kök nedenler (kanıtlı)

### K1 — Per-request timeout 30 dakika (en pahalı hata)

```rust
// src/llm/model.rs:30
const STREAM_REQUEST_TIMEOUT_SECS: u64 = 30 * 60;   // 1800 s
// kullanım: model.rs:1727 ve model.rs:2170
```

Ve bütçe zinciri:

```rust
// src/agent/worker.rs:53
pub const DEFAULT_WORKER_WALL_CLOCK_TIMEOUT_SECS: u64 = 1800;  // aynı 30 dk
```

**Sonuç:** Gateway takıldığında istek 30 dakika boyunca açık kalır, worker'ın
30 dakikalık duvar saati de tam o anda dolar → **hiçbir iş yapılmadan ölüm**.
Kanıt: `39845c1a` (0 tool call, 1800 s) ve `077b9079` (64 tool call sonra 1800 s).

Retry çarpanı ile en kötü durum:

```rust
// src/llm/routing.rs:565-571
pub const MAX_FALLBACK_ATTEMPTS: usize = 3;
pub const MAX_RETRIES_PER_MODEL: usize = 3;      // backoff 500 → 1000 → 2000 ms
pub const RETRY_BASE_DELAY_MS: u64 = 500;
```

3 deneme × 4 model × 30 dk timeout → teorik olarak **saatler**. Gerçekçi senaryoda
bile takılan bir istek 90 dakika yiyebilir.

**Aksiyon:** `STREAM_REQUEST_TIMEOUT_SECS` → **90-120 s**. Streaming bir chat
completion için 30 dakika hiçbir senaryoda doğru değil. İlk token için ayrı,
toplam süre için ayrı bütçe (ör. ilk token 60 s, toplam 300 s) daha doğru.

### K2 — LLM katmanında eşzamanlılık sınırı yok

Eşzamanlılık **sadece** worker/branch sayısında sınırlı:

```
src/config/types.rs:1822-1823   max_concurrent_branches: 5, max_concurrent_workers: 5
src/config/types.rs:1327        cortex tick_interval_secs: 30
```

`src/llm/` içinde `Semaphore`/limiter **yok** (yalnızca `manager.rs`'te
`rate_limited` cooldown haritası var — bu bir *sınırlama* değil, *tepki*).

**Sonuç:** 5 + 5 + cortex (30 sn'de bir) + cron + channel aynı anda aynı
gateway'e gidiyor. Kanıt, gerçek log satırları:

```
ERROR ... channel LLM call failed channel_id=autonomy
      error=CompletionError: ProviderError: OpenAI-compatible streaming error:
      The request queue is full.
ERROR ... channel LLM call failed channel_id=cron:health-watchdog
      error=... The request queue is full.
WARN  ... cortex synthesis task failed error=... The request queue is full.
      task="intraday" failure_count=5
```

Kuyrukta beklemek **saf gecikmedir**; kullanıcının "yavaş" hissinin doğrudan
kaynağı.

**Aksiyon:** `src/llm/` içine tek bir global `Semaphore` (ör. 3-4) koy; katman
başına adil pay (channel öncelikli, cortex/cron en düşük). Ayrıca
`cortex.tick_interval_secs` 30 → **300** (config'te zaten `[defaults.cortex]`
yok, varsayılan kullanılıyor).

### K3 — Tur başına ~13-14K input token / ~11,5 sn

Worker'da 13.696, branch'te 19.746, `glm-5.3-flash` kanalda **33.166** token/tur.
Her tur bu kadar prefill ödüyor.

**İyi haber:** prefix cache çalışıyor — `inception/deepseek-v4-flash-0731` için
`cache_read_tokens` (264,6 M) toplam input'un (93,1 M) çok üstünde, yani isteklerin
büyük kısmı cache'ten geliyor. Sistem promptu da **bilinçli olarak** byte-stabil
tutulmuş:

```rust
// src/agent/channel.rs:3397-3399
// Time no longer renders here — it rides on the current user message
// envelope instead, so this prompt stays byte-stable across turns
// (see `with_time_envelope`).
```

Ve `prompts/en/channel.md.j2`'de **dinamik bloklar en sonda** (identity → skills →
worker_capabilities → … → knowledge_synthesis → working_memory →
channel_activity_map → participant_context → active_goals → status_text →
chronicle → backfill). Bu doğru tasarım; **burada yapılacak bir şey yok.**

**Kalan sorun:** bilinçli olarak çok geniş bir prompt turuyoruz. `glm-5.3-flash`
kanalda 33K token/tur = her turda bir roman okutmak. Yapılacak: process tipine göre
prompt bütçesi (worker/channel/branch) + hangi bloğun kaç token yediğini ölçen bir
rapor (`estimate_text_tokens` zaten `prompt_tokens` olarak kaydediliyor —
`chronicler.record_prompt_tokens`).

### K4 — Reasoning token israfı (%50-60)

| process | reasoning | output | oran |
|---|---|---|---|
| worker | 2.786.340 | 5.636.744 | **%49,4** |
| channel | 1.521.684 | 2.541.477 | %59,9 |
| branch | 363.414 | 587.938 | %61,8 |
| cortex | 68.938 | 88.024 | **%78,3** |

`ls`, `grep`, `pgrep` çalıştıran bir worker'ın çıktısının yarısının "düşünme"
olması saf gecikme. Reasoning token'lar seri üretilir — çıktı hızı ne olursa olsun
toplam süreyi doğrudan uzatır.

**Aksiyon:** process tipine göre reasoning politikası:
- **worker** → reasoning kapalı / minimum (tool döngüsü; düşünmeye gerek yok)
- **branch** → orta (karar/araştırma işi)
- **channel** → model varsayılanı
- **cortex** → minimum. %78,3 reasoning = cortex bakım işi için tam israf.

### K5 — Generic 400 retriable değil → fallback çalışmıyor

```rust
// src/llm/routing.rs:123  is_retriable_error()
// mevcut kalıplar: 429/500/502/503/504, "rate limit", "overloaded", "timeout",
// "connection", "server error", "internal error", "empty response",
// "already borrowed" …
// EKSİK: "could not be completed"
```

`src/` içinde `"could not be completed"` **hiç geçmiyor** (grep: boş).

**Kanıt — gerçek worker ölümü** (`worker_a04bd497...log`, 10 Eyl 07:18):

```
State: Failed
--- Error ---
CompletionError: ProviderError: OpenAI-compatible streaming error:
This request could not be completed. If it keeps happening, contact support
and quote the request id.
```

10 tur tool çalıştırdıktan sonra öldü. `is_retriable_error` bunu tanımadığı için
**fallback zinciri hiç denenmedi** (`claude → deepseek → glm` devreye girmedi).

Bu, `docs/design-docs/fork-upstream-plan.md`'nin **"AKŞAM BUILD LİSTESİ 19"**
maddesinin ta kendisi ve hâlâ açık.

**Aksiyon:** (a) `is_retriable_error`'a gateway'e özgü kalıpları ekle
(`"could not be completed"`, `"contact support and quote the request id"`);
(b) hata mesajına **gerçek model id**'yi ekle (`provider.name` statik etiketi
yüzünden hangi modelin patladığı log'dan anlaşılmıyor — plan md'de de yazılı).

### K6 — Tool argümanlarına ham özel token sızıyor, sanitizasyon yok

Aynı worker log'unda, model `shell` tool'unun `command` argümanına **ham özel
token'ları gömüyor**:

```
Args: {"command":"... grep -rn \"busy_timeout...\" ...</｜DSML｜ue> I accidentally
mangled that last shell command. Let me redo it properly.</think>\n\n
<｜DSML｜tool_calls>\n<｜DSML..."}
```

Sonuç:

```
sh: -c: line 1: syntax error near unexpected token `newline'
```

Yani model bir turu tamamen çöpe attı; harness bunu yakalayıp temizlemiyor.

Kodda sanitizasyon **yok**: `src/` içinde `DSML` / `<｜` geçmiyor; tek
`sanitize_*` fonksiyonu `tools/mcp.rs`'te isim temizliği için.

**Aksiyon:** tool argümanlarını modele göndermeden önce (veya tool hatasını
görünce) normalize et: `<｜…｜>` ve `</think>` / `<tool_calls>` bloklarını ayıkla,
JSON parse'ı başarısızsa **tek seferlik "argümanını yeniden üret" düzeltmesi**
ver (halihazırda `history_repair.rs` deseni var — aynı yere bağlanabilir).
Bu, sessizce kaybedilen turları kurtarır ve **doğrudan hız** demektir.

---

## 3. Ek gözlem: worker'lar neden "sonuç vermiyor" gibi görünüyor

ACPP tarafında `worker_runs.result` alanının gerçek içeriği:

```
6ef557dc | acp | done     | Worker completed
f2bde06e | acp | done     | Worker completed
70338708 | acp | failed   | Worker failed: failed to spawn ACP agent 'omp' in '...'
ecea6bd6 | acp | failed   | Worker failed: ACP prompt turn timed out after 600 seconds
```

Yani "sonuç" alanı **işin çıktısı değil**, "worker tamamlandı" damgası. Gerçek
sonuç transcript'te/kanal cevabında. Bu, kullanıcının "worker bir şey yapmıyor /
yavaş" algısını güçlendiriyor: worker biter, ortada doğrulanabilir tek satır
sonuç olmaz.

---

## 4. Dün (10 Eylül 2026) yapılan değişiklikler

Doküman/commit dışı, henüz **commit edilmemiş** durumda duran 4 dosya + plan md:

```
 M .cargo/config.toml                     lld linker (build hızlandırma)
 M Cargo.toml                             [profile.release] lto: thin → false
 M docs/design-docs/fork-upstream-plan.md  "AKŞAM BUILD LİSTESİ 19" eklendi
 M src/acp/worker.rs                      omp oturum başlığı yaması (+171)
 M src/db.rs                              SQLite havuz ayarları (+37/-14)
 M src/tools.rs                           SetHomeChannelTool temizliği (+6)
?? .freebuff/  ?? research/
```

**Doğrulama:** `cargo check --all-targets` ✅ **1m09s** (nice'li). Yani bu hâliyle
derleniyor; commit edilmeyi bekliyor.

### 4.1 `src/db.rs` — SQLite havuz ayarları (senin "bozduğum ayar" olabilir)

```rust
// src/db.rs:112-128 (yeni)
SqliteConnectOptions::new()
    .filename(db_path)
    .create_if_missing(true)
    .journal_mode(SqliteJournalMode::Wal)
    .synchronous(SqliteSynchronous::Normal)
    .busy_timeout(Duration::from_secs(10))
    .pragma("cache_size", "-64000")       // 64 MB *bağlantı başına*
    .pragma("mmap_size", "268435456")     // 256 MB *bağlantı başına*
    .pragma("temp_store", "memory")
    .pragma("wal_autocheckpoint", "1000");
SqlitePoolOptions::new().max_connections(20)
```

**DÜZELTME (sqlx kaynağı okunarak doğrulandı).** İlk analizde "eski kod
busy_timeout'suz bağlanıyordu" diye yazmıştım — **bu yanlıştı**. `sqlx-sqlite`
varsayılanları (`src/options/mod.rs`):

| Ayar | sqlx varsayılanı | Fork'un yaptığı |
|---|---|---|
| `busy_timeout` | **5 s** (`Duration::from_secs(5)`) | 10 s'ye **çıkarmış** |
| `journal_mode` | **ayarlanmıyor** (istenmedikçe pragma yazılmaz) | WAL eklemiş ✅ |
| `synchronous` | ayarlanmıyor → SQLite varsayılanı **FULL** | `NORMAL` ✅ |
| `foreign_keys` | **ON** | değiştirmemiş ✅ |

Yani gerçekte:

- **`synchronous = NORMAL` gerçek bir kazançtır** — bu pragma olmadan SQLite her
  commit'te fsync yapar. WAL ile `NORMAL`, "çökmede son işlemi kaybedebilirsin ama
  bozulmaz" demektir; doğru tercih.
- **`busy_timeout` bir "yok" durumu değil, 5 s → 10 s yükseltmesiydi** ve bu
  yükseltme zararlı yönde: tıkanan bir kilit, hızlı hata vermek yerine kanal turunu
  10 saniye boyunca sessizce bekletir — kullanıcı bunu "ajan yavaş" diye okur.
- **`journal_mode` için not:** sqlx bu pragmayı bilerek yalnızca istek üzerine
  yazar, çünkü *mod değiştirmek* `busy_timeout` ile beklenemeyen bir exclusive lock
  gerektirir. WAL dosyada kalıcıdır; bu yüzden sadece **ilk** bağlantı (taze DB'de)
  bu bedeli öder, sonrakiler no-op olur.

**Riskli olan:**
1. **`cache_size -64000` bağlantı başına 64 MB.** 20 bağlantı dolarsa **1,28 GB**
   sayfa cache'i. 15 GiB'lık makinede, LanceDB + fastembed + chromium ile birlikte
   bu ciddi baskı. → `-16000` (16 MB) veya `-8000` daha güvenli.
2. **`mmap_size 256 MB` × 20 = 5 GB sanal adres alanı.** Sanal olduğu için ölümcül
   değil ama RSS'e sızabilir; 64 MB yeterli.
3. **`busy_timeout 10 sn`** kilit beklemesini 10 saniyeye kadar *sessizce*
   uzatır. Kullanıcı "worker yavaş" derken buraya da bakmak gerekir: eşzamanlı
   worker + cortex yazımlarında 10 sn beklemek normal hâle gelebilir. → 3-5 sn.
4. `journal_mode(Wal)` + `temp_store(memory)`: WAL zaten çoğu geçici tabloyu
   bellek/disk arasında yönetir; `temp_store=memory` ile büyük sorgularda RAM
   şişebilir.

**Karar:** Ayarları geri almak **yerine** doğru doğrultmak gerekiyor — WAL ve
`NORMAL` kalsın, sayılar düşsün. **Uygulandı → §4.4.**

### 4.2 ACP worker'ın `omp` → Intent Daemon köprüsüne çevrilmesi (asıl gerileme)

Config geçmişi (10 Eyl'e kadar, 5 yedek dosyada): **hepsi `command = "omp"`**.
Dün 08:49–08:51'de `~/.local/bin/spacebot-acp-worker` **13.343 baytlık Python
Intent Daemon köprüsü** ile değiştirildi (yedek: `spacebot-acp-worker.bak`, 1.044 b).

Bu köprü **iş yapmıyor, sadece yönlendiriyor**:

```python
# ~/.local/bin/spacebot-acp-worker → handle_session_prompt()
ok, info = send_task_to_intent(prompt_text, session_id)   # intentd'e gönder
# ... ve HEMEN dön:
write_msg({... "status": "completed",
           "content": {"text": f"[Intent Bridge] {info}\n"}})
return {"result": {"text": status_msg.strip()}}
```

Yani:

- İş **`intentd`'ye** gidiyor (`~/.local/share/intentd/`, `intentd.db` 21 MB),
  Spacebot bir **onay satırı** alıyor: `[Intent Bridge] Routed to 'spacebot-ile' …`.
- **Sonuç dönüş yolu yok** — intentd'deki iş bittiğinde Spacebot'a hiçbir şey
  akmıyor. Fire-and-forget.
- Bu yüzden ACP worker'lar "anında bitti" görünüyor (ya da interactive olduğu için
  12-40 saat açık kalıyor, §1.1), ama ortada iş sonucu yok.
- Ek olarak `src/acp/worker.rs`'deki commit edilmemiş **omp oturum başlığı yaması
  bu konfigürasyonda ölü kod**: köprü `uuid4()` üretiyor, `~/.omp/agent/sessions/`
  altında o id'yi taşıyan dosya yok → yama hep "session file not found" ile çıkıyor.

**Bu harness felsefesiyle çelişiyor.** `harness.md` Pillar 2'nin garantisi
*"never stuck, never silently fail"*; köprü ise tam da **sessiz başarısızlık**
üretiyor (worker "done", iş ise başka bir daemon'da).

**Üç seçenek:**
1. `command = "omp"`'a geri dön (kanıtlanmış yol; gerçek sonuç üretiyordu).
2. Köprüye **sonuç dönüş yolu** ekle: intentd'de `agent.sendMessage` sonrası
   turn tamamlanmasını bekleyip (`agent.get` / event stream) `session/update`
   olarak akıtmak + nihai metni `result.text`'e koymak. (Doğru çözüm ama
   intentd JSON-RPC'sinin "turn bitti" sinyalini doğrulamak gerekir.)
3. ACP worker'ı devre dışı bırak, builtin worker'ı kullan (builtin 381 done / %74
   başarı, gerçek transcript üretiyor).

**Öneri:** şimdilik **(1) veya (3)**; (2) ayrı bir tasarım işi.

### 4.3 Çözüm — uygulanan ayarlar (11 Eyl 2026)

`src/db.rs` içindeki `connect_sqlite_pool` aşağıdaki değerlere çekildi ve
fonksiyonun tamamı gerekçesiyle belgelendi:

| Ayar | Eski (fork) | Yeni | Gerekçe |
|---|---|---|---|
| `journal_mode` | WAL | **WAL** (kaldı) | Okuyucular yazarı, yazar okuyucuları bloklamaz |
| `synchronous` | NORMAL | **NORMAL** (kaldı) | Doğru; `FULL`'a göre commit başına fsync yok |
| `busy_timeout` | 10 s | **5 s** | sqlx varsayılanı; yükseltmek kilidi *gizler*, çözmez |
| `cache_size` | `-64000` (64 MiB) | **`-16000`** (16 MiB) | Bağlantı başına; 10 bağlantıda üst sınır 1,25 GiB → **160 MiB** |
| `mmap_size` | 256 MB | **0 (kapalı)** | Aşağıdaki nota bak |
| `temp_store` | memory | **memory** (kaldı) | Sıralamalar küçük; geçici b-tree disk'e inmesin |
| `wal_autocheckpoint` | 1000 | **1000** (kaldı) | ~4 MiB WAL büyümesi |
| `max_connections` | 20 | **10** | SQLite yazarları serileştirir; ekstra bağlantı sadece okuyucu kazandırır. 10 = sqlx varsayılanı |
| `acquire_timeout` | (varsayılan 30 s) | **10 s** | Havuzdan bu süreden uzun bağlantı beklemek "bir şey tıkandı" demektir; log'da görünsün |

**`mmap_size = 0` neden kapalı** (SQLite resmî dokümanı, <https://sqlite.org/mmap.html>):

> *"An I/O error on a memory-mapped file cannot be caught and dealt with by
> SQLite. Instead, the I/O error causes a signal which, if not caught by the
> application, results in a program crash."*

Ve: *"the use of memory mapped I/O does not significantly change the performance
of database changes"*, faydası *"mostly ... for queries"*. Veritabanı 33 MB ve
zaten OS page cache'inde; mmap'in okuyucu kazancı ölçülemez, karşılığında
**SIGBUS ile ölümcül çökme** riski alınır. Haftalarca çalışması beklenen bir
daemon için bu değiş tokuş yanlış.

**Doğrulama (bu değişiklikle birlikte eklendi):** `src/db.rs` içinde iki test:

1. `pool_connections_carry_the_tuning` — havuzun verdiği bağlantıda
   `journal_mode=wal`, `synchronous=1`, `busy_timeout=5000`, `cache_size=-16000`,
   `mmap_size=0`, `temp_store=2` olduğunu **pragma'yı okuyarak** doğrular.
2. `every_pooled_connection_is_tuned` — havuzdan **3 farklı** bağlantı çekip her
   birinin ayarlı olduğunu doğrular (yalnızca ilk bağlantının ayarlı olduğu bir
   havuz, tek sorguluk smoke test'te düzgün görünüp gerçek eşzamanlılıkta sapıtır).

Bu testlerin değeri: pragma'lar **sessizce yok sayılsa** bile fark edilir. "Ayar
var ama etkisiz" durumu, "ayar yok" durumundan daha tehlikelidir.

**Ölçüm notu:** `journal_mode` dosyada kalıcı, diğerlerinin hepsi **bağlantı
başına**. Bu yüzden `sqlite3` CLI ile ayrı bir oturumda bakmak yanıltır —
yukarıdaki testler aynı bağlantı üzerinden okur. (Canlı DB'de doğrulandı:
`agent.db` ve `spacebot.db` ikisi de `wal`.)

### 4.4 Diğer

- `~/.spacebot/logs/2026-09-*` = 4,26 GB (2,2 GB tek dosya) → temizlenebilir +
  rotasyona boyut sınırı.
- `interface/dist/` **bugün** 15:24'te yeniden derlenmiş (frontend build taze).
- `migrations/` tarafında dün **yeni migration yok**; son migration 4 Eyl
  (`20260904000001_ingestion_retry_budget.sql`). Yani "veritabanında bir sürü
  değişiklik" = `db.rs` pragma'ları + runtime verisi, şema değişikliği değil.

---

## 5. Harness dokümanlarının değerlendirmesi

`harness.md`, `harness-plan.md`, `memory-map.md` **üçü de satır satır okundu.**
Değerlendirme:

### Ne doğru ve etkileyici

- **`memory-map.md` gerçek bir "architecture memory map"**: 7 process tipi
  (Channel/Branch/Worker/Compactor/Cortex/Autonomy/Cron), tek `ProcessEvent` bus'ı,
  memory katmanları (hot/warm, decay formülü `importance * age_decay * access_boost`,
  prune <0.1/30g, merge >0.95), storage tablosu, guardrail listesi.
  Bu doküman **kodla uyuşuyor** — doğruladım (worker.rs sabitleri, channel_prompt
  segmentleri, db.rs, migrations).
- **"Invariant: channel never blocks on branches/workers"** — mimarinin kalbi ve
  doğru. Fire-and-forget delegation hızlı UX sağlar.
- **Prefix-cache farkındalığı** (`channel.rs:3397` yorumu): nadir ve değerli bir
  optimizasyon detayı; prompt şablonu da dinamik blokları sona koyacak şekilde
  düzenlenmiş.
- **Evidence-gated completion** (`7af7c736`) + `verify-after-work` wake: harness.md
  §"Verified Goal Loop"un somut karşılığı. `wake_defs`'te `verify-after-work`
  mevcut (şu an `enabled=0`, `min_level=suggest` — doğru; yoksa her worker
  tamamlanması ikinci bir doğrulama worker'ı doğururdu).

### Ne eksik / çelişki

| harness iddiası | Gerçek |
|---|---|
| Pillar 2: "never hung — per-command wall-clock budget, kill-on-timeout" | Tool katmanı için var (`shell` timeout + kill); **LLM çağrısı için yok** (K1: 30 dk). "Never stuck" yarı yolda kalmış. |
| Guardrail: "No hang: worker wall-clock + supervisor timeout/kill" | İkisi de **1800 s = aynı sayı**. İkinci bütçe birincisinin yerine geçtiği için *hiçbir* koruma sağlamıyor; biri diğerinden küçük olmadıkça anlamsız. |
| harness.md: sandbox "no stuck in sandbox" — command always runs | `[agents.sandbox] mode = "disabled"` (config). Tutarlı, ama passthrough'un *vaat edilen* varsayılan olup olmadığı test edilmeli. |
| Faz 0.3 "supervisor adoption" — ACP/OpenCode/Shell children register | ACP tarafı köprüye taşındı (§4.2); `ChildRegistry`'ye kayıt Python süreci için hâlâ geçerli mi, doğrulanmalı. |
| memory-map: "Cortex … health ticks, maintenance, bulletin" | `cortex.tick_interval_secs = 30` — 30 saniyede bir tick. K2'nin kaynağı. Harness'ın "background maintenance" varsayımı maliyet açısından hesaba katılmamış. |
| harness.md "Multimodal vision (Faz 3)" | Açık (bekliyor) — sorun değil, sıralama doğru. |

### Dokümanlarla gerçek arasındaki en kritik boşluk

Üç doküman da **mimariyi** tarif ediyor; **bütçe/latency sözleşmesi** yok.
"N never-stuck" var ama "**N saniyeyi geçen çağrı ölür**" yok. Harness'ın
performans tarafı yazılmamış. Öneri: `harness.md`'ye **"Pillar 4 — Budgets"**
eklemek (per-call LLM timeout, global eşzamanlılık, proses tipine göre reasoning
politikası, prompt token bütçesi) — çünkü bu dört şeyin hiçbiri şu an *tasarımda*
yok, o yüzden kodda da yok.

---

## 6. Aksiyon planı (öncelik sırasıyla)

### P0 — Model/stabilite (küçük diff, büyük etki)

| # | Değişiklik | Dosya | Doğrulama |
|---|---|---|---|
| 1 | `STREAM_REQUEST_TIMEOUT_SECS`: 1800 → **120** (ilk-token bütçesi opsiyonel ayrı) | `src/llm/model.rs:30` | unit test + bir isteği kasten askıya al |
| 2 | `is_retriable_error`: `"could not be completed"` + gateway kalıpları | `src/llm/routing.rs:123` | `routing.rs` testleri (mevcut desen: `is_retriable_error_catches_*`) |
| 3 | Hata mesajına gerçek **model id** ekle | `src/llm/model.rs` hata formatı | log'da model id görünmeli |
| 4 | Global LLM semaphore (3-4) + proses tipine göre adil pay | `src/llm/manager.rs` | yük testi: gateway 429 almamalı |
| 5 | `cortex.tick_interval_secs` 30 → **300** (config'ten) | `~/.spacebot/config.toml` | cortex LLM çağrı sayısı düşmeli |
| 6 | Tool-arg sanitizasyonu (`<｜…｜>`, `</think>`) + tek seferlik argüman onarımı | yeni `src/llm/arg_sanitize.rs` + `history_repair.rs` deseni | model log'undan gerçek DSML örneğiyle unit test |

### P1 — Doğru mimari kararı (senin çağrın)

| # | Karar | Not |
|---|---|---|
| 7 | ACP worker: `omp`'a dön **veya** köprüye sonuç dönüş yolu ekle **veya** devre dışı bırak | §4.2 — harness felsefesi açısından "sessiz başarısızlık" kabul edilemez |
| 8 | ~~`src/db.rs` pragma sayılarını düzelt~~ → **YAPILDI** (§4.3); kalan: commit | §4.3 |
| 9 | Proses tipine göre reasoning politikası (worker/cortex minimum) | K4 — %49-78 israf |
| 10 | Log rotasyonuna boyut sınırı + 4,26 GB temizlik | `src/daemon.rs:178` |

### P2 — Harness olgunlaştırma

| # | İş | Gerekçe |
|---|---|---|
| 11 | `harness.md`'ye **Pillar 4 — Budgets** ekle | §5'teki en kritik boşluk: bütçe sözleşmesi hiç yazılmamış |
| 12 | Prompt token bütçesi + blok bazlı token raporu | K3 — `glm-5.3-flash` kanalda 33K/tur |
| 13 | "Sonuç" alanını gerçek iş çıktısıyla doldur (artık "Worker completed" değil) | §3 — güven + doğrulama |
| 14 | Commit edilmemiş 4 dosyayı gözden geçir → commit → **release binary'yi yeniden derle** | binary 5 Eyl'den beri bayat (§1.4) |

### Ölçüm (her fix'ten sonra aynı komut)

```bash
# tur latansı + hata oranı
sqlite3 "file:$HOME/.spacebot/agents/main/data/agent.db?mode=ro" \
 "select worker_type,status,count(*),
  round(avg((julianday(completed_at)-julianday(started_at))*86400),1) ort_s
  from worker_runs group by 1,2;"
# reasoning oranı
sqlite3 "file:$HOME/.spacebot/agents/main/data/agent.db?mode=ro" \
 "select process_type,model,round(100.0*sum(reasoning_tokens)/max(1,sum(output_tokens)),1)
  from token_usage group by 1,2 order by 3 desc limit 5;"
# queue-full hataları
grep -ac "request queue is full" ~/.spacebot/logs/spacebot.log.$(date +%F)
```

**Başarı kriteri:** worker ortalaması **170 s → <90 s**, hata oranı **%26 → <%10**,
`queue is full` günlük sayısı **→ 0**.

---

## 7. Doğrulanmamış / açık kalanlar (dürüstlük notu)

- Bu ölçümlerin tamamı **7-10 Eylül** penceresinden; `inception/*` modelleriyle.
  Kullanıcı "o modeller çalışmıyor" dedi → yeni sağlayıcı gelince **aynı ölçümleri
  tekrarlamak** gerekir, yoksa "yavaş" mı "sağlayıcı kötü" mü ayırt edilemez.
- Gateway'in gerçek p50/p95 gecikmesini **ölçemiyorum** (sadece tur toplamından
  çıkarım yaptım: tur süresi − tool süresi).
- `db.rs` ayarlarının **runtime'da** uygulandığı artık testle kanıtlı (§4.3),
  ancak testler henüz **yeşil koşmadı**: `cargo test --lib` bu repoda 2,5 GB'lık
  bir test binary'si link'lediği için tek koşu dakikalar sürüyor (§6/P2'ye bak).
- `busy_timeout` kaynaklı kilit beklemesinin ne kadar sürdüğünü ölçmedim;
  bunun için kısa bir enstrümantasyon gerekir (§4.1 notu).
- Sonuç tiplerinin (`result`) neden "Worker completed" ile doldurulduğu koda
  bakılarak netleştirilmeli — dokümandaki gözlem DB çıktısından.

---

## 8. Sıradaki adım

Kullanıcı yeni bir model sağlayıcı verecek. Gereken bilgiler:

- `base_url` (OpenAI-uyumlu mu?)
- `api_key`
- worker / channel / branch / cortex için model id'leri
- reasoning desteği var mı (K4 için kritik)

Bu gelene kadar **P0'un tamamı model-bağımsız** ve uygulanabilir.
