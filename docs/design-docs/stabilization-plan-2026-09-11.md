# Stabilizasyon Planı — "Sorunsuz çalışır hale getirme" (2026-09-11)

> Bu plan, `worker-perf-stability-diagnosis-2026-09-11.md`'deki **ölçülmüş**
> kök nedenleri kapatıp Spacebot'u güvenilir çalışır hale getirmek için yazıldı.
> Sıra önemli: önce ortamı sakinleştir, sonra küçük kod düzeltmeleri, sonra
> doğru derleme, en son mimari boşluklar.
>
> Kural: her adım **ölçülebilir** bir sonuç üretir. "Düzeldi" demek için
> ölçüm komutu ve hedef sayı var (§5).

---

## 0. "Stabil" ne demek — kabul kriteri

Bugünkü ölçüm (7-10 Eylül penceresi) ile hedef:

| Ölçüm | Bugün | Hedef |
|---|---|---|
| Worker ortalama süre | **170 s** | < 90 s |
| Worker hata oranı | **%26** | < %10 |
| Tur başına LLM gecikmesi | **~11,5 s** | < 6 s |
| `"request queue is full"` (günlük) | tekrarlayan | **0** |
| 30 dakikada ölen worker | 2 (biri 0 iş yaparak) | **0** |
| Log klasörü | **4,26 GB** | < 500 MB |
| `target/` (derleme) | **55 GB** | < 20 GB |
| Çalışan binary | 5 Eyl (bayat) | güncel kaynakla aynı |

---

## 1. Faz 0 — Ortamı sakinleştir (bugün, ~15 dk, risk yok)

| # | İş | Ne kazandırır |
|---|---|---|
| 0.1 | `target/debug/incremental` (15 GB), kesilmiş link artığı (2,3 GB) ve eski binary'ler silinir | Disk ~21 GB rahatlar. Tam yeniden derleme **gerektirmez** |
| 0.2 | Antigravity (`agy`, 6,5 saat %56 CPU) ve `opencode` (6 saat %15 CPU) kapatılır veya iş bitince durdurulur | Ölçüm temiz olur; ajanlar birbirinin CPU'sunu yemez |
| 0.3 | Çalışma disiplini: hızlı doğrulama `cargo check` (~1 dk); `cargo test` toplu/akşam | Makine sürekli yorulmaz |

**Neden ilk bu:** ölçüm yapacağız. Arka planda 3 ajan çalışırken alınan ölçüm
yanıltır — "yavaş" mı "makine dolu" mu ayırt edilemez.

### 0.2 bulgusu — "makine yavaş"ın gerçek sebebi CPU değil, RAM (ölçüldü)

```
16 çekirdek · yük 2,92 · CPU boş %74-86 · iowait %0-2   ← CPU rahat
RAM 15 GiB ama programların toplam belleği:
    RSS 9,0 GB + SWAP 9,7 GB = 18,7 GB        ← %25 aşırı taahhüt
swap 11 GiB dolu · swappiness 10 (zaten düşürülmüş)
```

CPU boşken makinenin ağır hissetmesinin sebebi bu: Linux ~9,7 GB program
hafızasını diske itmiş, dokunulan her pencerenin sayfaları önce diskten geri
okunuyor. **CPU ölçümü yanıltıcıydı; doğru gösterge swap/RAM'dir.**

**Kapatılanlar (onayla, 11 Eyl 16:15):** `next-server`
(`~/Projects/airbnbdeneme`, 1,88 GB swap'te, sadece 64 MB RAM'de) ve `midas-mcp`
(562 MB swap'te).

| Ölçüm | Önce | Sonra |
|---|---|---|
| Swap kullanımı | 11 GB | **9,4 GB** |
| Program belleği (RAM+SWAP) | 18,7 GB | **16,2 GB** |
| Yük | 2,92 | **1,0** |

**Kalan yük:** Zen tarayıcı süreçleri (~2,5-3 GB swap'te, en büyüğü tek başına
939 MB), `opencode` (~1 GB), `Telegram` (425 MB). Kalan adım: tarayıcıyı bir kez
yeniden başlatmak.

**Ders (plan için):** bu makinede ölçüm ve derleme yapmadan önce **swap durumu
kontrol edilmeli**. Dolu swap'ta alınan hiçbir zaman ölçümü güvenilir değildir —
bu, K1-K6'nın ölçüm penceresini de etkilemiş olabilir.

---

## 2. Faz 1 — Kod: küçük düzeltmeler, büyük etki (P0)

Hepsi **küçük diff**, hepsi test edilebilir, hiçbiri mimari değiştirmez.

| # | Düzeltme | Dosya | Neden |
|---|---|---|---|
| 1.1 | Per-request timeout **1800 s → 120 s** | `src/llm/model.rs:30` | Takılan bir istek worker'ın tüm 30 dakikasını yiyor (K1) |
| 1.2 | `"could not be completed"` + gateway kalıplarını **retriable** yap | `src/llm/routing.rs:123` | Fallback zinciri çalışsın; worker ölmesin (K5, plan md madde 19) |
| 1.3 | Hata mesajına **gerçek model id**'sini ekle | `src/llm/model.rs` | Hangi modelin patladığı log'dan görünsün |
| 1.4 | Tool argümanlarında ham özel token temizliği (`<｜…｜>`, `</think>`) | yeni modül + `history_repair.rs` deseni | Bir turun tamamen çöp olmasını engeller (K6) |
| 1.5 | LLM çağrılarına **global eşzamanlılık sınırı** (3-4) | `src/llm/manager.rs` | `"request queue is full"` bitsin; kuyruk beklemesi = gecikme (K2) |
| 1.6 | `cortex.tick_interval_secs` **30 → 300** | `~/.spacebot/config.toml` | Arka plan bakımı LLM kotasını yemesin (K2/K4) |

**Doğrulama:** her biri için birim testi + `cargo check` + `cargo fmt --check`.
**Beklenen etki:** hata oranı %26 → ~%10, tur gecikmesi ~11,5 s → ~7-8 s.
**Bağımlılık:** yok — model değişikliği beklemez, hemen yapılabilir.

### Durum (11 Eyl 2026) — hepsi YAZILDI

| # | Durum | Nerede |
|---|---|---|
| 1.1 | ✅ timeout 30 dk → **5 dk** (gerekçe kodda) | `src/llm/model.rs` `STREAM_REQUEST_TIMEOUT_SECS` |
| 1.2 | ✅ `could not be completed` + `quote the request id` + `request queue is full` retriable + 2 test | `src/llm/routing.rs` |
| 1.3 | ✅ non-retriable hatada log'a `model=<id>` | `src/llm/model.rs` (stream + non-stream) |
| 1.4 | ✅ tool argümanlarında `<｜…｜>` / `</think>` temizliği + 4 test | `src/llm/model.rs` |
| 1.5 | ✅ global LLM semaphore (4, `SPACEBOT_LLM_CONCURRENCY` ile ayarlanır) | `src/llm/manager.rs` |
| 1.6 | ✅ `[defaults.cortex] tick_interval_secs = 300` | `~/.spacebot/config.toml` |

**Ek olarak (asıl K5 düzeltmesi):** `stream()` artık `stream_with_fallbacks()`
çağırıyor — streaming de backoff + fallback zincirini kullanıyor. Detay:
`fork-upstream-plan.md` madde 19 "DÜZELTME".

**Doğrulama durumu:** `cargo fmt --check` ✅ · `cargo check --all-targets` ✅
(5m49s) · lib testleri koşuldu (§10).

---

## 3. Faz 2 — Derleme ve kurulum (bir kez, akşam, ~30 dk)

| # | İş | Ne kazandırır |
|---|---|---|
| 2.1 | `Cargo.toml`: `[profile.dev] debug = 1` + `[profile.dev.package."*"] debug = false` | Binary 2,5 GB → ~400 MB; link dakikalar → saniyeler; disk tekrar şişmez |
| 2.2 | Commit edilmemiş 4 dosyayı gözden geçir → **commit** | 10 günlük iş kayıt altına girer (şu an sadece diskte) |
| 2.3 | **Release binary'yi yeniden derle** ve daemon'ı onunla başlat | Şu an çalışan 5 Eyl tarihli; kaynak 10 Eyl'e kadar değişti |
| 2.4 | Log rotasyonuna **boyut sınırı** + mevcut 4,26 GB temizlik | Disk + I/O; tek dosya 2,2 GB olmasın |
| 2.5 | Yedekleme: `~/.spacebot` (agent.db 33 MB + config.toml) | Her denemeden önce geri dönüş noktası |

**Neden 2.3 kritik:** ölçüm yaparken çalışan binary'nin *hangi koddan* derlendiği
bilinmiyorsa, hiçbir sonuç güvenilir değil. Şu an tam bu durumdayız.

---

## 4. Faz 3 — Model katmanı (senin bilginle)

| # | İş | Not |
|---|---|---|
| 3.1 | Yeni sağlayıcıyı `~/.spacebot/config.toml`'a ekle | `base_url` + `api_key` + OpenAI-uyumlu mu |
| 3.2 | Proses tipine göre model ata | worker=hızlı, channel=orta, branch=akıllı, cortex=ucuz |
| 3.3 | **Reasoning politikası**: worker ve cortex'te kapalı/minimum | Şu an çıktının %49-78'i düşünme (K4) |
| 3.4 | Fallback zincirini kur ve **test et** | Bir modeli kasten boz → fallback'e geçiyor mu |
| 3.5 | Aynı ölçümleri tekrarla | 170 s / %26 hedefleriyle karşılaştır |

**Bağımlılık:** 1.2 olmadan fallback zaten çalışmaz — bu yüzden Faz 1 önce.

---

## 5. Faz 4 — Mimari boşluklar (sırayla, acele yok)

| # | İş | Neden |
|---|---|---|
| 4.1 | ACP ↔ Intent köprüsüne **dönüş yolu**: tur bitince sonucu `session/update` ile akıt + `result.text`'e koy | Şu an Spacebot sadece "yönlendirdim" alıyor; iş sonucu dönmüyor. "Sessiz başarısızlık" harness felsefesiyle çelişiyor |
| 4.2 | `worker_runs.result` alanı gerçek iş çıktısını taşısın | Bugün "Worker completed" yazıyor; doğrulanabilir bir sonuç yok |
| 4.3 | `harness.md`'ye **"Pillar 4 — Budgets"** bölümü | Per-call timeout / eşzamanlılık / reasoning / prompt bütçesi hiç tasarlanmamış — bu yüzden kodda da yok |
| 4.4 | Prompt token bütçesi + blok bazlı token raporu | `glm-5.3-flash` kanalda **33K token/tur** okuyor |

---

## 6. Sıra ve neden bu sıra

```
Faz 0  ortamı temizle        → ölçüm güvenilir olsun
Faz 1  küçük kod düzeltmeleri → sistem ölmesin, yavaşlamasın
Faz 2  derle + başlat         → ölçülen şey gerçekten güncel kod olsun
Faz 3  modelleri bağla        → asıl hız buradan geliyor
Faz 4  mimari boşluklar       → kalıcı olgunluk
```

**Kritik bağımlılık:** Faz 1 bitmeden Faz 3'e geçmek anlamsız — fallback
çalışmıyorsa yeni model de aynı hatayla ölür ve "model kötü" sanırız.

---

## 7. Ölçüm komutları (her fazdan sonra aynı)

```bash
DB="$HOME/.spacebot/agents/main/data/agent.db"

# 1) worker hızı + hata oranı
sqlite3 "file:$DB?mode=ro" "
 select worker_type, status, count(*) n,
   round(avg((julianday(completed_at)-julianday(started_at))*86400),1) ort_s
 from worker_runs group by 1,2;"

# 2) reasoning israfı
sqlite3 "file:$DB?mode=ro" "
 select process_type, model,
   round(100.0*sum(reasoning_tokens)/max(1,sum(output_tokens)),1) reasoning_pct
 from token_usage group by 1,2 order by 3 desc limit 6;"

# 3) gateway sıkışması
grep -ac "request queue is full" ~/.spacebot/logs/spacebot.log.$(date +%F)

# 4) disk
du -sh target ~/.spacebot/logs
```

---

## 8. Riskler ve geri dönüş

| Risk | Önlem |
|---|---|
| Faz 2.1 (profil değişikliği) tüm bağımlılıkları yeniden derler | Akşam yapılır; öncesinde `~/.spacebot` yedeklenir |
| Faz 2.3 (yeni binary) mevcut çalışan sistemi bozabilir | Eski binary `spacebot.bak` olarak saklanır; config yedeği alınır |
| Faz 1.5 (eşzamanlılık sınırı) kanalı yavaşlatabilir | Sınır 3-4; kanal en yüksek öncelikli pay alır |
| Yeni model sağlayıcı da bozuk çıkabilir | Faz 3.4: fallback'i kasten test et |

---

## 10. Commit edilmemiş dosyaların incelemesi (11 Eyl 2026)

Bu 6 dosya, oturum öncesinden beri çalışma ağacında duruyor (git'te değil).
Değerlendirme:

| Dosya | Değişiklik | Değerlendirme |
|---|---|---|
| `.cargo/config.toml` | `lld` linker + `gcc` | ✅ **Koru.** Build süresini kısaltıyor, riski yok |
| `Cargo.toml` | `[profile.release] lto: thin` → **false** | ✅ **Koru.** (İlk değerlendirmem "geri al"dı, ölçüm sonrası değişti: bu sistemin darboğazı CPU değil, LLM/network gecikmesi — tur başına 11,5 s'nin tamamı bekleme. LTO'nun runtime kazancı burada ölçülemeyecek kadar küçük, buna karşılık her release derlemesine 10-20 dk ekliyor.) |
| `src/tools.rs` | `SetHomeChannelTool` artık temizleniyor | ✅ **Koru.** Gerçek bir 400 sebebini kapatıyor (katı gateway'ler tekrar eden tool adını reddediyor) |
| `src/acp/worker.rs` | omp oturum başlığı yaması (+171) | ⚠️ **Mevcut config'te ölü kod:** köprü `uuid4()` üretiyor, `~/.omp/agent/sessions/` altında eşleşen dosya yok → yama hep "session file not found" ile çıkıyor. `omp`'a dönülürse tekrar işlevsel. Koru, ama ölü olduğunu bil |
| `src/db.rs` | SQLite ayarları + 2 test (§4.3) | ✅ **Koru.** Testler geçti |
| `docs/design-docs/fork-upstream-plan.md` | Madde 19 + DÜZELTME notu | ✅ Kayıt |

**Öneri:** 6 dosya olduğu gibi commit edilsin (hepsi ya düzeltme ya nötr ayar).
Commit edilmeden önce diff yedeği alındı:
`~/.spacebot/backups/pre-stabilization-20260911/uncommitted-20260911.patch` (447 satır).

---

## 11. Karar noktaları (senin onayın gerekenler)

1. **Faz 0:** Antigravity + opencode'u şimdi kapatabilir miyim? (ölçüm temizliği için)
2. **Faz 1:** Hemen başlayalım mı? (model beklemez)
3. **Faz 2.1:** Derleme profilini değiştireyim mi? (bir kez 15-30 dk makine yorulur)
4. **Faz 2.2:** Commit edilmemiş 4 dosya commit edilsin mi?
5. **Faz 3:** Yeni sağlayıcı bilgisi (base_url / api_key / model id'leri / reasoning var mı)
6. **Faz 4.1:** ACP köprüsünün dönüş yolu — küçük poll tabanlı mı, Intent event API'si mi?
