# Fork Geliştirme Planı — upstream kaynak taraması

> Tarih: 2026-09-04 · Repo: coruhoorhan/spacebot (fork of spacedriveapp/spacebot)
> Kaynak: spacedriveapp/spacebot — 68 açık PR + ~166 issue tamamen okundu.
> Bu liste yalnızca **bu fork'un kurulumuna, yaşadığımız sorunlara ve Jarvis hedefine** göre seçilmiştir.
> Güncelleme 2: 68 PR'ın tamamının açıklamaları okundu → BÖLÜM 1 revize edildi.

---

# BÖLÜM 1 — Upstream PR seçimleri (REVİZE — 68 PR'ın tamamı okundu)

## 🔥 ÖNCELİK 1A — Hafıza boşluğu üçlüsü ("bot beni tanımıyor" sorunu)

### 1. #363 (issue) + #401 + #545 — Kısa Telegram sohbetleri hafızaya hiç işlenmiyor
- **Sorun üçlüsü:**
  - #363: Telegram/Discord konuşmaları agent hafızasına commit edilmiyor
  - #401: Memory persistence sadece her N mesajda tetikleniyor (varsayılan 50) — **kısa sohbet eşiğe ulaşamadan biterse HİÇ kaydedilmiyor**
  - #545: Memory persistence contract ingestion hattına bağlanmamış (branch.rs'de var, ingestion'da yok)
- **Bizdeki karşılığı:** "spacebot nedir" sorusunda bot kendini tanımıyordu; kısa Telegram konuşmaları unutuluyor.
- **Yapılacak:** #545'i cherry-pick (küçük, ingestion'a contract bağlar) → #401'i uyarla (kanal kapanışında flush) → #363 doğrulaması.
- **Doğrulama:** Kısa Telegram sohbeti yap, botu yeniden başlat, "ne konuşmuştuk" de — hafızadan cevap almalı.

## 🔥 ÖNCELİK 1A — DURUM RAPORU (2026-09-04, inceleme bitti — build yok)

### #545 (ingestion contract) → ✅ FORK'TA ZATEN ÇÖZÜLMÜŞ
- Kanıt: `src/agent/ingestion.rs:551` → `hook.with_memory_persistence_contract(contract_state.clone())`
- Ayrıca `branch.rs:73` ve `hooks/spacebot.rs:160` — contract mekanizması tam bağlı.
- **Aksiyon: YOK.**

### #401 → ⚠️ GERÇEK BUG DOĞRULANDI — "kısa sohbet hiç işlenmiyor"
- `channel.rs:4732 check_memory_persistence()` — 3 tetikleyici (mesaj sayısı / 900sn zaman / olay yoğunluğu).
- **AMA 5 çağrı yerinin HEPSİ mesaj/worker-bitiş yollarında** (2435, 2555, 2826, 3031, 4354).
  **Arka plan timer'ı YOK.** Zaman tetikleyicisi bile sadece YENİ MESAJ gelince kontrol ediliyor.
- Sonuç: kısa sohbet (örn. 19 mesaj) bitip kullanıcı dönmezse → HİÇ persistence çalışmaz → hafıza kaydı yok.
- Daha keskin bulgu: uzun sessizlikten sonraki İLK mesajda digest, mesaj CEVAPLANDIKTAN SONRA tetiklenir
  → bot yeni oturumun ilk sorusunda dünkü hafızayı daha sindirememiş olur ("beni tanımıyor" hissi).
- **DB kanıtı (canlı):** `telegram:474374538` 73 mesaj → 17 mp-run (işlenmiş) ✅; ama
  `portal:chat:main` 2 mesaj → 0 mp-run, `1e48...` 4 mesaj → 0, `cddc...` 4 mesaj → 0, `8902...` 2 mesaj → 0 ❌
  → kısa/bitmiş sohbetler hafızaya HİÇ girmemiş. (memories tablosu: 98 kayıt, kaynak tipleri user/assistant/system.)

### FIX TARİFİ (build sırasında uygulanacak, sırayla):
1. **A) Arka plan idle timer (ASIL FİX):** her kanalda `persistence_time_threshold_secs` (900sn) dolunca
   mesaj beklemeden `check_memory_persistence()` çağıran tokio tick. Telegram gibi hiç "kapanmayan"
   kanallar için tek gerçek çözüm bu.
2. **B) Oturum başı digest:** mesajı İŞLEMEDEN ÖNCE (veya kanal uyanınca) persistence kontrolü
   → dünkü/kısa sohbet, bot cevap vermeden önce hafızaya girer.
3. **C) Kanal kapanışında flush:** `process_control.rs:70/168` (channels.remove) ve
   `main.rs:1995/2035/2071` (active_channels.remove) yoluna son bir check ekle.
- **Doğrulama (build sonrası):** kısa portal sohbeti aç-kapat → mp-run görünmeli; Telegram'da 1 mesaj at,
   bot cevap vermeden önce önceki oturumun digest'i log'da görünmeli.

### #363 (umbrella) → KISMEN ÇÖZÜLDÜ
- Uzun Telegram sohbeti hafızaya İŞLENİYOR (17 mp-run, memories kayıtları var) — persistence çalışıyor.
- "Bot beni tanımıyor" şikayeti daha çok: (a) #401'deki oturum-başı digest gecikmesi + (b) kimlik/bağlam
  enjeksiyonu eksikliğiydi (o kısım dün identity dosyaları + spacebot_docs ile ayrıca düzeltildi).
- Bu madde #401 fix'i uygulanınca kapanabilir.

## 🔥 ÖNCELİK 1B — Telegram sonuç kaybı dörtlüsü

### 2. #581 (issue) + #582 + #652 + #367 — Worker sonucu Telegram'a ulaşmıyor
- **Sorun dörtlüsü:**
  - #581: retrigger relay fail → son cevap düşer (issue — dünkü log'umuzda birebir görüldü)
  - #582: retrigger/fallback gönderimine retry/backoff ekler (channel.rs) — #581'in ilk çözümü
  - #652: adapter'da onay + fail'de 1 retry + kuyrukta tut, sonraki turda tekrar dene
  - #367: modelin teaser yerine tam sonucu iletmesini garanti et
- **Yapılacak:** #582 önce (küçük, channel.rs) → #652 sonra (adapter seviyesi) → #367 son.
- **Doğrulama:** Worker bitir, mesaj düştüğünü gör; relay fail simüle et, retry çalışsın.

## 🔥 ÖNCELİK 1B — DURUM RAPORU (2026-09-04, inceleme + kod fix — canlı test akşam)

### Teslimat zinciri (kanal cevabı → platform) — retry YOKTU, ŞİMDİ VAR
- Zincir: `channel.send_outbound_text` → `send_routed` (mpsc, fire-and-forget) → `main.rs route_outbound` → `MessagingManager::respond()` → `adapter.respond()` → Telegram API.
- Eski davranış: adapter hatası (ağ/API) → sadece log (`failed to send outbound response`) → **mesaj kaybolur, retry yok.**
- Retry makinesi (`broadcast_proactive`, 3 deneme, 1s→8s üstel, transient/permanent ayrımı) **zaten vardı** ama sadece proaktif yollarda kullanılıyordu (cron, restart duyurusu) — kanal relay'lerinde DEĞİL.
- **FIX (uygulandı):** `src/messaging/manager.rs` → `respond()` artık geçici hatalarda aynı sınırlı retry/backoff'u kullanıyor (kalıcı hatalar anında döner, thread bağlamı korunur — `broadcast()`e geçmek thread reply'ları bozardı).
- **Kod dosyası:** `src/messaging/manager.rs` (respond). **Derleme:** cargo check ✅ (42sn, nice). **Canlı test:** akşam — Telegram'ı geçici kes, worker bitir, retry log'unu gör (`response delivery failed with retryable error, retrying`).

### #581 + #652 durumu (result düşmesi) — KISMEN ZATEN ÇÖZÜLMÜŞ, kalanı fix'lendi
Fork'ta zaten var olan sağlamlaştırmalar (önceki oturumlar):
- Worker/branch sonucu `pending_results` → debounce'lu retrigger (kanal spam önleme)
- Retrigger turunda LLM skip/raw-text dönerse fallback gönderimi + dedup (`is_duplicate_of_recent_assistant`, commit'siz) + leak/syntax engelleme
- Relay yine de başarısızsa sonuç history'ye işaret olarak yazılıyor (`[background work completed but relay to user failed — include this in your next response]`) → pasif koruma (sonraki kullanıcı mesajında iletilir)
- **Eksik olan:** adapter teslimatının kendisi başarısız olunca aktif retry — şimdi respond() retry ile kapandı. (#652'nin "adapter onayı + kuyruk + sonraki turda tekrar" derin hali için bu yeterli ilk katman.)

### #367 (teaser yerine tam sonuç) — kodda kesme yok, model davranışı
- Kod yolunda cevabı "teaser"a kırpan bir mantık görülmedi; önceki dedup/fix çalışması sonucun TAM iletilmesini hedefliyor. Bu madde canlı gözlemle kapatılmalı (akşam testinde doğrula).

## 🔥 ÖNCELİK 1C — Diğer kritik fix'ler

### 3. #550 — [agents.channel] config'i platform kanallarında uygulanmıyor
- **Sorun:** `response_mode`, `save_attachments` vb. TOML'dan yükleniyor ama runtime'da sessizce yutuluyor (`agent_default` hep `None`).
- **Bizdeki karşılığı:** YOLO/Sandbox ayarlarının Telegram'da beklenen etkiyi göstermeme sebebi olabilir! (YENİ KEŞİF)
- **Yapılacak:** Cherry-pick — `ResolvedConversationSettings::resolve()` çağrılarına config'i gerçekten bağla.
- **Doğrulama:** Telegram kanalında response_mode/listen_only değişikliği yap → davranışın değiştiğini gör.

## 🔥 ÖNCELİK 1C — UYGULAMA DURUMU (2026-09-04 akşam öncesi, kod YAZILDI + cargo check ✅)

### 1C'nin 3 fix'i de KODA İŞLENDİ ve derleme onaylı (68 sn nice'li cargo check, makine yorulmadı):

| Fix | Dosya(lar) | Kod durumu |
|---|---|---|
| **#550** config bağlama | `settings.rs` (helper) + `main.rs` (2 blok, 8 çağrı) + `control.rs` + `channel.rs` (hot-reload) | ✅ Yazıldı, check geçti |
| **#556** LLM timeout | `hooks/spacebot.rs` (`LLM_CALL_TIMEOUT_SECS=300` + prompt_once + stream_completion) | ✅ Yazıldı, check geçti |
| **#551** builtin interactive initial sonuç | `worker.rs` (follow-up loop öncesi WorkerInitialResult gönderimi) | ✅ Yazıldı, check geçti |

**#550 detay:** `ConversationSettings::from_agent_channel_config(&ChannelConfig)` helper'ı eklendi
(response_mode + save_attachments taşır, gerisi sistem default'u — binding/conversation override'larını ezmez).
`deps.runtime_config.channel_config` (ArcSwap, hot-reload'da güncellenir) tüm çağrı yerlerinde 3. parametre
(`agent_default`) olarak geçiliyor. Cron kanalları BİLİNÇLİ hariç (`default()` — kısa ömürlü otomatik kanal,
response_mode uygulanması yanlış olur).

**#556 detay:** Upstream ile birebir aynı desen — timeout → `PromptError::CompletionError` (rig 0.33
`CompletionError::RequestError(#[from] Box<dyn Error>)` ile derlendi). Branch/compactor/ingestion (prompt_once)
+ kanal (stream_completion isteği) korunuyor. Akış stream tüketim döngüsü istek kurulduktan sonra tool
çalıştırmalarını kesmez (sadece ağ beklemesi).

**#551 detay:** Builtin interactive worker, follow-up loop'a girmeden önce initial `result`'ı scrub'layıp
`WorkerInitialResult` event'iyle kanala gönderiyor (ACP/OpenCode worker'ların zaten yaptığı gibi). Kanal
yorumuyla (channel.rs "initial or follow-up") artık tutarlı. `follow_up_failure`/`follow_up_blocked`
değişkenleri korundu (bir ara düzenlemede silinmişti, geri eklendi).

**Akşam build listesi (hepsi koda işlendi, tek build + test + canlı doğrulama yeterli):**
1. #401 idle digest (channel.rs) — önceki oturum, check ✅
2. #582/#652 respond retry (manager.rs) — önceki oturum, check ✅
3. #550 config bağlama (4 dosya) — bugün, check ✅
4. #556 LLM timeout (hooks/spacebot.rs) — bugün, check ✅
5. #551 builtin interactive sonuç (worker.rs) — bugün, check ✅
6. Önceki Telegram dedup / ACP Content[] / ACP mkdir fix'leri (acp/, tools.rs) — önceki oturumlar
7. #538-2 memory dedup (store.rs + memory_save.rs + test) — bugün, check ✅
8. #552-a streaming tool-args .next() toleransi (model.rs) — bugün, check ✅
9. #552-b worktree remove --force (git.rs) — bugün, check ✅
10. #552-c signal.rs floor_char_boundary (UTF-8 slice) — bugün, check ✅
11. #324 secret scan modu (secret_scanner) — bugün, check ✅ — scrub.rs (enum+mode-aware),
    sandbox.rs (config alanı), hooks (guard/reply/event mode-aware), channel.rs (3 fallback),
    worker.rs/branch.rs/channel_dispatch.rs (egress mode-aware), spawn_worker_task (param)
12. #604 ingestion retry budget + canonical merge — bugün, check ✅ — YENİ migration
    `20260904000001_ingestion_retry_budget.sql` (additive: attempts + next_attempt_at),
    ingestion.rs (gating + backoff + quarantined + fail_ingestion_file), api/ingest.rs
    (delete = disk + progress purge), maintenance.rs (canonical merge) + 2 test güncellemesi
13. #324 boşluk kapatma + #538-3 kod yolu — bugün, check ✅ — acp/worker.rs + opencode/worker.rs
    (secret_scan_mode alanı + builder + 7 mode-aware çağrı), channel_dispatch.rs (4 spawn noktasında
    mode bağlama), config/types.rs (Auto + inception)
14. #438 spawn_worker task_type routing — bugün, check ✅ — tools/spawn_worker.rs (SpawnWorkerArgs'a
    task_type alanı + schema), channel_dispatch.rs (spawn_worker_from_state + spawn_worker_inner'a
    task_type parametresi; worker_model_override: conversation > task_overrides > routing default)

**Canlı doğrulama adımları (build + daemon restart sonrası):**
- #550: config'e `[agents.channel] response_mode = "observe"` ekle → restart → Telegram mesajına bot susmalı
- #556: yanlış/erişilemez model ver → mesaj at → ~5 dk sonra "Waiting for response" bitmeli, hata dönmeli
- #551: builtin interactive worker (interactive:true) spawn et → outcome + evidence ile bitir → sonuç Telegram'a gelmeli

## 🔥 ÖNCELİK 1C — DURUM RAPORU (2026-09-04, inceleme bitti — build yok)

### #550 → ❌ KESİN DOĞRULANDI — `[agents.channel]` config'i runtime'da HİÇBİR platform kanalına uygulanmıyor

**Kanıt zinciri (kod):**
1. `ConversationSettings` (settings.rs:228) — model / model_overrides / memory / delegation / response_mode / save_attachments / worker_context. Agent seviyesi `[agents.channel]` config'i (`ChannelConfig`, types.rs:1128) yalnızca **3 alan** taşıyor: `listen_only_mode`, `response_mode`, `save_attachments`. → `ResolvedAgentConfig.channel`'a resolve ediliyor (load.rs:2080, parse_response_mode ile listen_only_mode birleşiyor).
2. `ResolvedConversationSettings::resolve(conversation, channel, agent_default)` (settings.rs:303) — **3. parametre (`agent_default`) VAR ve düzgün çalışıyor** (model/memory/delegation/response_mode/save_attachments/worker_context uyguluyor).
3. **AMA tüm çağrı yerlerinde 3. parametre hep `None`:**
   - `main.rs:1427` (idle worker resume) → `resolve(conv.settings, None, None)`
   - `main.rs:1795-1855` (portal + platform kanalı spawn) → **6 çağrının hepsi** `resolve(x, binding_settings, None)` — `agent.config.channel` HİÇ geçilmiyor
   - `control.rs:161` (slash komut settings'i) → `resolve(db_settings, binding_settings, None)`
   - `channel.rs:1603` (hot-reload) → `resolve(new_settings, None, None)`
   - `agent_default`'ı gerçekten dolduran tek yer: **test** (settings.rs:489)
4. Yani config'te `[agents.channel] response_mode = "observe"` yazsan bile → kanal spawn'da `resolve(..., None)` → `ResolvedConversationSettings::default()` = **Active** → config SESSİZCE YUTULUYOR. Binding'den gelen `binding_settings` (main.rs:1689-1711) **başka şey** — o `[bindings]` mesaj-yönlendirme settings'i, agents.channel değil.
5. Kullanıcının config'inde (`~/.spacebot/config.toml`) şu an `[agents.channel]` bölümü YOK → bug şu an gizli ama dashboard'daki "channel listen_only_mode" toggle'ı (api/config.rs:443, 1085) config'e YAZIYOR ve runtime'da ETKİSİZ kalıyor.

**FIX TARİFİ (build sırasında uygulanacak):**
1. `ChannelConfig → ConversationSettings` çevirici: `ConversationSettings { response_mode: Some(channel.response_mode? veya parse_response_mode sonucu), save_attachments: Some(channel.save_attachments), ..Default::default() }` (listen_only_mode zaten load.rs:2073'te response_mode'a parse ediliyor — orada tek kaynak `response_mode`).
2. **`main.rs:1795-1855`** — kanal spawn bloğunda `agent.config.channel` erişilebilir; `resolve()`'un 3. parametresine `Some(&agent_channel_settings)` geç (6 çağrı). Portal bloğu + platform bloğu ikisi de.
3. **`main.rs:1427`** (idle resume) — aynısı.
4. **`control.rs:161`** — aynısı (deps üzerinden agent config'e erişim var).
5. **`channel.rs:1603`** (hot-reload) — aynısı; yoksa restart sonrası config'in etkisi DB settings'iyle ezilir.
- **Doğrulama (build sonrası):** config'e `[agents.channel] response_mode = "observe"` ekle → daemon restart → Telegram mesajına bot YANIT VERMEMELİ (Observe). Kaldır → restart → yanıt geri gelmeli.

### 4. #556 — Per-call LLM timeout + activity-based branch tracking
- **Sorun:** Asılan LLM çağrısı kanalı sonsuza dek bloklar.
- **Bizdeki karşılığı:** "Waiting for response" takılmaları. #557 (last_activity_at güncellemesi) ile birlikte.
- **Yapılacak:** Cherry-pick #556 + #557.

## 🔥 ÖNCELİK 1C — DURUM RAPORU 2 (2026-09-04, #556+#557 inceleme bitti — build yok)

### #556 (per-call LLM timeout) → ❌ FORK'TA YOK — "Waiting for response" takılmasının muhtemel kaynağı
**Upstream'in önerisi:** `hooks/spacebot.rs`'e `LLM_CALL_TIMEOUT_SECS = 300` (5 dk) + `prompt_once` ve
`prompt_once_streaming`'i `tokio::time::timeout` ile sarmak; timeout → `PromptError::CompletionError` →
mevcut retry/hata yolları devreye girer.

**Fork'taki durum (kod kanıtı):**
- `hooks/spacebot.rs:470 prompt_once` → **timeout YOK.** Kullananlar: `branch.rs:169` (branch ana döngüsü),
  `compactor.rs:339`, `ingestion.rs:557`, `chronicle.rs:800/892` — hepsi korumasız.
- `hooks/spacebot.rs:489 prompt_once_streaming` → **timeout YOK.** Kullanan: `channel.rs:3717/3743`
  (kanalın ANA cevap turu + tool-syntax recovery retry'i) — kanal korumasız.
- Mevcut tek koruma **provider seviyesinde ve çok uzak:** `llm/model.rs:30 STREAM_REQUEST_TIMEOUT_SECS = 30*60`
  (30 dk!) + `llm/manager.rs:114` reqwest client 120 sn. Yani asılan bir API çağrısı kanalı **30 dk'ya kadar**
  "Waiting for response"te tutabilir — dün yaşadığın takılmaların muhtemel açıklaması.
- Worker'lar korumalı (`worker.rs:540 run()` wall-clock `tokio::time::timeout` + `WorkerOutcome::Timeout`) ama
  bu #556'nın konusu değil — kanal/branch/compactor/ingestion'ın LLM çağrısı ayrı ve korumasız.

### #557 (activity-based branch tracking) → ❌ FORK MİMARİSİNE UYMUYOR (uyarlama gerekir)
- Upstream: `cortex.rs` BranchTracker'a `last_activity_at: Instant` + supervisor health tick'inde wall-clock yerine
  activity kontrolü. Fork'ta **BranchTracker/cortex supervisor/lagged_control YOK** (grep: sıfır eşleşme) —
  fork'ta branch'ler `channel_dispatch.rs` üzerinden kanal sahipliğinde spawn ediliyor.
- `src/supervisor.rs` farklı bir şey: subprocess (shell/omp/opencode) yürütme supervisor'ı — branch'lerle ilgisi yok.
- **Sonuç:** #557 cherry-pick edilemez, #556 uygulanınca fork'un kendi branch sahiplik modeline göre uyarlanmalı.

### FIX TARİFİ (build sırasında uygulanacak, #550'den sonra):
1. `hooks/spacebot.rs`: `const LLM_CALL_TIMEOUT_SECS: u64 = 300;`
2. `prompt_once`: `agent.prompt(...)` çağrısını `tokio::time::timeout(300s)` ile sar → hata → `PromptError::CompletionError`.
3. `prompt_once_streaming`: `stream_completion()` + stream tüketim döngüsünü timeout ile sar (tool çalıştırma dışarıda kalmalı —
   timeout yalnızca LLM ağ beklemesini ölçsün; her `stream.next()` beklemesi de ayrı idle-timeout alabilir).
   Timeout → `PromptError::CompletionError` → kanalın mevcut hata yolu (reply/skip) devreye girer.
- **Doğrulama (build sonrası):** provider'ı durdur (veya yanlış model ver), kanala mesaj at →
  ~5 dk sonra "Waiting for response" bitmeli, kanal hata mesajıyla cevap vermeli (sonsuz bekleme YOK).

### 5. #551 — Interactive worker outcome sonrası idle takılması
- **Sorun:** Worker `set_status(kind="outcome")` sonrası sonsuz idle → crash-loop.
- **Yapılacak:** Follow-up loop'a girme koşulunu düzelt. #325/#156 (zombi worker) ile ilişkili.

## 🔥 ÖNCELİK 1C — DURUM RAPORU 3 (2026-09-04, #551 inceleme bitti — build yok)

### #551 → ⚠️ KISMEN ÇÖZÜLMÜŞ — asıl boşluk: builtin interactive worker'ın İLK sonucu kanala hiç ulaşmıyor

**Upstream'in 2 fix'inin fork'taki durumu:**
1. **history.rs terminal-state koruması → ✅ ZATEN VAR (hatta daha güçlü).**
   `transition_worker` (history.rs:1014): `expected.is_terminal() || target.is_terminal() || !can_transition_to()` → conflict
   + DB sorgusu `WHERE lifecycle = ? AND completed_at IS NULL` (koşullu UPDATE). Upstream'in race fix'i fork'ta gereksiz.
2. **worker.rs follow-up loop'a outcome kontrolü → ❌ EKSİK (asıl bug).**

**Kritik bulgu — builtin interactive worker'ın İLK sonucu kayboluyor:**
- `worker.rs:912`: `if let Some(mut input_rx) = self.input_rx.take()` — **outcome kontrolü YOK.** Initial task outcome ile
  bitse bile worker koşulsuz follow-up loop'a girip `input_rx.recv().await`'te (930) sonsuza dek bekler.
- `WorkerInitialResult` event'i worker.rs'de **sadece follow-up cevaplarında** gönderiliyor (1064). Initial task sonrası
  (905-935 arası) gönderim YOK — `result` değişkende bekler ama run() dönmeden kanala ulaşmaz.
- run() ancak follow-up loop bitince döner → loop ancak sender (`input_tx`, kanalda `worker_inputs`'ta) drop edilince biter
  → kanal ancak `WorkerComplete` alınca `worker_inputs`'tan siler (channel.rs:4384) → **sirküler bağımlılık.**
- Kanal yorumu (channel.rs:4435) "completed a task **(initial or follow-up)**" diyor ama worker.rs'de initial için gönderim
  yok — **kod ile yorum tutarsız.** Kanal initial sonucu bekliyor, worker göndermiyor.
- Kontrast: ACP (acp/worker.rs:248) ve OpenCode (opencode/worker.rs:433) worker'ları kendi initial sonuçlarını
  `WorkerInitialResult` ile GÖNDERİYOR — sadece **builtin** interactive worker göndermiyor. Dünkü "worker bitti ama
  Telegram'a mesaj atmadı" şikayetinin muhtemel teknik sebebi.
- Non-interactive (fire-and-forget) worker'lar etkilenmez: input_rx yok → follow-up loop'a girmez → initial sonuç
  `WorkerOutcome::Success` → run() döner → `WorkerComplete` normal ateşlenir. ✅

**FIX TARİFİ (build sırasında uygulanacak, #556'dan sonra):**
1. `worker.rs:912` civarı — follow-up loop'a girmeden önce: `if self.hook.outcome_signaled() { ... }` kontrolü.
   Outcome sinyali verilmişse → initial `result` + outcome text'i `WorkerInitialResult` event'iyle kanala gönder
   (ACP/OpenCode worker'ların yaptığı gibi), sonra `input_rx`'i drop et (veya loop'a girme) → run() döner →
   `WorkerComplete` normal ateşlenir → kanal sonucu kullanıcıya iletir.
2. Not: `set_status.rs` testi `interactive_outcome_does_not_claim_terminal_lifecycle` (interactive worker outcome'ta
   lifecycle'ı Completing'e geçirmiyor) #551 fix'iyle ÇELİŞMEZ — set_status anlık lifecycle değiştirmemeli, ama
   worker run() doğal dönünce terminal olmalı.
- **Doğrulama (build sonrası):** Telegram'dan builtin interactive worker spawn et (interactive: true), görevi bitir
  (outcome + evidence) → kanala sonuç mesajı GELMELİ (şu an gelmiyor), worker "completed" olmalı, idle'da kalmamalı.

### 6. #324 — Secret scanner yanlış pozitif düzeltmesi
- **Sorun:** İstem dışı public key'ler (Algolia vb.) credential sanılıp worker öldürülüyor.
- **Bizdeki karşılığı:** Worker'ların "failed" olmasının gizli sebebi olabilir.
- **Yapılacak:** Cherry-pick — `SecretScanMode` (3 mod, yanlış pozitifi engelle).

### #324 → DURUM RAPORU 7 (2026-09-04: inceleme + KOD YAZILDI, check ✅ — canlı test akşam)

**Fork'ta mod KAVRAMI YOKTU ve yanlış pozitifin gerçek zararı kanıtlandı:**
- Regex katmanı `scan_for_leaks`/`scrub_leaks` her yerde koşulsuz. Zarar sınıfları:
  1. **Channel tipi** tool çıktısında leak → hook `Terminate` (guard_tool_result, spacebot.rs:877) — direct-mode kanal scrape yaparsa ölür.
  2. **channel.rs 3 retrigger/fallback bloğu** (3940/4018/4087) → leak eşleşirse **sonuç sessizce bloklanır, kullanıcı hiçbir şey görmez** — #581 ailesinin ikincil kayıp yolu.
  3. **Egress scrub** (worker.rs 940/1088, branch.rs 304, dispatch 520/1575) → içerik `[LEAKED_SECRET_REDACTED]` ile **manglenir** (public Algolia/Google Maps key'i olan sayfa içeriği bozulur).
- Builtin/ACP/OpenCode **worker'lar ölmez** (sadece warn + scrub) — upstream'in "worker öldürülüyor" iddiası fork'ta sadece Channel/Cortex/Compactor için geçerli; asıl semptom sessiz blok + mangling.

**Yapılan (upstream #324'nin fork'a küçültülmüş hali, 11 dosya):**
1. `scrub.rs`: `SecretScanMode { strict | own_secrets_only | disabled }` (serde lowercase, default Strict) +
   `scan_for_leaks_with_mode` / `scrub_leaks_with_mode` (non-strict'te regex katmanı atlanır, Layer-1
   stored-secret scrub **hep açık** kalır). Eski fonksiyonlar Strict wrapper — dokunulmayan çağrılar değişmedi.
2. `sandbox.rs`: `SandboxConfig.secret_scanner` alanı → `[agents.sandbox] secret_scanner = "own_secrets_only"`.
3. `hooks/spacebot.rs`: `with_secret_scan_mode` builder (mevcut with_* idiomu) + guard/reply-block/event scrub mode-aware.
4. Kurulum yerleri (8): channel, worker×2, branch, compactor, ingestion, cortex_chat, chronicle×2 — config'ten modu bağlar.
5. Egress: worker.rs/branch.rs/channel_dispatch.rs (spawn_branch + spawn_worker_task yeni `secret_scan_mode`
   parametresi — 10 çağrı yeri güncellendi) mode-aware.
6. channel.rs 3 fallback bloğu: sadece Strict'te leak eşleşmesi bloklar.
7. 3 yeni test: serde deserialize ×3 varyant, mode-aware scan skip, mode-aware scrub passthrough (akşam build'de çalışacak).

**Kalan boşluk → KAPATILDI (2026-09-04):** ACP (`acp/worker.rs`) ve OpenCode (`opencode/worker.rs`)
worker'larına `secret_scan_mode` alanı + `with_secret_scan_mode` builder eklendi; kendi egress
scrub/scan çağrılarının 7'si mode-aware yapıldı. Mod, spawn noktalarından bağlanıyor
(channel_dispatch: opencode ×2 dal + acp + resume). api/state.rs log scrub'ı Strict (display katmanı, zararsız).

**Canlı doğrulama (build + restart sonrası):** config'e `[agents.sandbox] secret_scanner = "own_secrets_only"`
ekle → worker'a "https://ornek.com sayfasındaki tüm metni getir" görevini ver (sayfa içinde AIza... key varsa)
→ sonuç `[LEAKED_SECRET_REDACTED]` OLMADAN gelmeli; strict'e dönünce tekrar redakt edilmeli.
Direct-mode kanalda scrape: strict'te Terminate, own_secrets_only'da normal cevap.

### 7. #538 (issue) — Hafıza duplike (non-Anthropic modellerde)
- **Sorun:** `memory_persistence_complete` sonrası model metin üretirse false negative → tekrar kayıt.
- **Bizdeki karşılığı:** Inception (non-Anthropic) kullanıyoruz → duplike riski yüksek.
- **Yapılacak:** Cherry-pick veya elle uyarla.

## 🔥 ÖNCELİK 1C — DURUM RAPORU 4 (2026-09-04, #538 inceleme bitti — build yok)

### #538 → ⚠️ 3 iddianın durumu: 1'i çözülmüş, 1'i kısmen, 1'i AYNEN VAR

**İddia 1 — "memory_persistence_complete sonrası metin üretimi false negative yapar" → ✅ FORK'TA ÇÖZÜLMÜŞ**
- `hooks/spacebot.rs:1490-1497`: `memory_persistence_complete` tool'u başarılı dönerse hook **anında `Terminate`
  dönüyor** (`MEMORY_PERSISTENCE_COMPLETE_REASON`) → sonradan metin üretme şansı kalmıyor.
- `ingestion.rs:588 classify_chunk_prompt_result`: `PromptCancelled + is_memory_persistence_complete_reason` →
  `Ok` kabul ediliyor. Contract state (`has_terminal_outcome`) de set ediliyor.
- **Sonuç:** Upstream'in "tool sonrası text → chunk failed" senaryosu fork'ta kapanmış. (branch.rs'de de aynı koruma.)

**İddia 2 — "retry duplike yaratır" → ⚠️ KISMEN ÇÖZÜLMÜŞ, bir boşluk kaldı**
- Çözülmüş kısım: `ingestion.rs:331-360` — `ingestion_progress` tablosu `content_hash + chunk_index` ile
  `INSERT OR IGNORE`; başarılı chunk'lar işaretleniyor (225-240), retry'de `completed.contains()` ile atlanıyor.
  Yani **başarılı chunk TEKRAR işlenmez** → progress düzeyinde duplike yok. Ayrıca file retry'de kaldığı yerden sürüyor.
- **Kalan boşluk:** Tek chunk İÇİNDE kısmi başarı — model `memory_save` çağırıp (DB'ye yazdı) ama
  `memory_persistence_complete`'e ulaşamadan (max_turns/hata) chunk failed olursa → o chunk'ın `memory_save`'leri
  DB'de KALIR, chunk progress'e işlenmez → retry'de aynı chunk YENİDEN işlenir → **aynı içerik ikinci kez
  `MemoryStore.save()` ile INSERT edilir** (store.rs:71 — düz INSERT, content-hash/dedup YOK).
  `memory_save` tool'unda da genel content dedup yok (sadece human anchor merge var, memory_save.rs:351).
- Prompt'ta (ingestion.md.j2) "önce memory_recall yap, duplike'ten kaçın" talimatı VAR ama bu modele güveniyor —
  garantili değil.

**İddia 3 — "ToolUseEnforcement::Auto gpt/codex dışı modelleri kapsamıyor" → ❌ AYNEN GEÇERLİ**
- `config/types.rs:274`: `Auto => model_lower.contains("gpt") || contains("codex")`.
- Bizim model `inception/mercury-2` → ikisini de içermiyor → **enforcement prompt'u enjekte EDİLMİYOR.**
- Kullanıcı config'inde `tool_use_enforcement` ayarı da yok → default `Auto` → Inception için kapalı.

### FIX TARİFİ (build sırasında uygulanacak — küçük, #551'den sonra):
1. **İddia 3 (en değerli, tek satır):** `config/types.rs` `Auto` dalına `|| model_lower.contains("inception")`
   ekle (veya kullanıcı config'ine `tool_use_enforcement = "always"` yaz — kod değişikliği GEREKMEZ,
   config-only çözüm). **Config yolu UYGULANDI:** `~/.spacebot/config.toml:23` → `[defaults]`
   `tool_use_enforcement = "always"` — hot-reload, build beklemez. Kod yolu da UYGULANDI (2026-09-04): `config/types.rs` `Auto` dalına
   `inception` eklendi — enforcement prompt'u artık config'e bağımlı olmadan da gidiyor. Inception diffusion modeli tool çağrısı yerine metin üretmeye meyilli → enforcement
   prompt'u gerçek fark yaratır.
2. **İddia 2 kalan boşluk → ✅ UYGULANDI (2026-09-04, build yok, check ✅):** `store.rs`'e
   `find_exact_duplicate(content, memory_type)` eklendi (canlı + forgotten=0 kayıtta aynı içerik+tür → mevcut id).
   `memory_save.rs` `call()`'da human HARIÇ her kayıt öncesi kontrol: duplike varsa yeni INSERT yerine mevcut
   kaydın id'siyle erken dönüş (embedding/consolidation atlanır, contract id'si yine kaydedilir — ingestion retry
   contract'ı bozulmaz). Seçenek (a)'nın uygulaması; agent_id memories tablosunda olmadığı için anahtar (content, type).
   Yeni test `duplicate_save_reuses_existing_memory` eklendi (akşam build'de çalışacak).
- **Doğrulama (build sonrası):** ingestion'a aynı içerikli dosya koy, modeli zorla yarıda kestir (kısa max_turns) →
  retry sonrası memories tablosunda aynı content'ten TEK kayıt olmalı. Enforce testi: Inception ile kanal mesajı →
  prompt'ta "call a tool" guidance'ı görülmeli (prompt inspector/log).

---

## 🟡 ÖNCELİK 2 — Yüksek değer

### 8. #653 + #16 — Worker'lar agent'a ait olmalı + live panel
- #653: Worker'lar channel değil **agent** sahipliğinde — bizim supervisor (PR #3) ile örtüşüyor, upstream'i izle
- #16: Workbench live panel + process telemetry — dashboard'daki Workbench'in temeli

### #653 + #16 → DURUM RAPORU 8 (2026-09-04, inceleme bitti — build yok)

**#653 (agent-owned worker registry) → ⚠️ ÇATLAK FORK'TA DA VAR — ama büyük mimari refactor**
- Fork'ta worker sahipliği **channel'da**: `ChannelState` `active_workers`, `worker_handles`, `worker_inputs`,
  `worker_injections`, `reserved_tasks` tutuyor (channel.rs:480-496) — upstream'in "split" dediği yapının aynısı.
- Fork'un agent seviyesi `ProcessControlRegistry` (process_control.rs:27) **sadece kanalları** kaydediyor;
  worker iptali kanal handle'ı üzerinden (`cancel_channel_worker`). Yani bir worker'ı iptal/route etmek için
  önce DOĞRU KANALI bulmak gerekiyor.
- **Kritik boşluk (somut):** cortex/autonomy worker'ları `channel_id: None` ile spawn ediliyor
  (tools/spawn_worker.rs:1132, dosya başı yorumu da söylüyor) → hiçbir kanalın map'inde değiller →
  kanal tabanlı arama (route/cancel/API) onları bulamaz. Upstream'in "resident autonomy SQLite'te worker
  görüyor ama route sadece o kanalın map'inde arıyor" tarif ettiği çatlak fork'ta da mevcut.
- Kanal kapanışı: channel.rs:949 `cancel_all_workers_and_branches` — kanal kapanınca worker'ları iptal ediyor;
  upstream bunu kaldırıp "channel shutdown worker lifecycle'ına dokunmaz" diyor (agent seviyesinde tek drain).
- Fork'un kendi paralel işleri: `src/supervisor.rs` (subprocess supervisor: shell/acp/opencode) + phase-21
  resident autonomy + `resume_idle_worker_into_state` (idle worker'ları origin channel'ı dirilterek restore
  ediyor — upstream registry'ye doğrudan restore ediyor). Upstream #649-653 stack'i fork'un phase-21/supervisor
  yönüyle aynı hizada — plan notu doğru.
- **Karar:** Bu maddeyi ŞİMDİ cherry-pick'lemek büyük refactor (channel.rs/dispatch/api/autonomy'de ~yüzlerce
  satır). Değer/risk oranı düşük — mevcut sistem çalışıyor, çatlak sadece kanal-dışı worker'ların
  cancel/route'unda. **Aksiyon: izle (upstream merge'ini bekle) + küçük yama olarak API/route aramasını
  agent seviyesine genişletmek istenirse ayrı iş.**

**#16 (Workbench live panel) → ✅ ÇOĞU ZATEN VAR**
- Fork dashboard'ında Workbench sayfası + live tool-call çıktısı var (api/state.rs `live_output`/
  `push_live_tool_call`, ProcessEvent WorkerStarted/ToolCompleted/WorkerStatus → UI). Workers sekmesi
  placeholder DEĞİL — phase-21/22 interface işi #16'nın özünü kapsıyor. (#16'nın geri kalanı UI detayı.)

**Akşam build listesine EKLENMİYOR.** Sıradaki adaylar: #604 (memory bloat — küçük) veya #543 (karar takibi).

### 9. #552 — ProviderError + streaming parser + worktree path fix
- LLM streaming'de `TrailingCharacters` çökmeleri + worktree çakışması — Telegram takılmalarıyla ilişkili olabilir.
- **→ ⚠️ 5 fix'in 5'i de tek tek karşılaştırıldı (2026-09-04): 1'i eksik, 1'i kısmen, 1'i N/A**

### #552 → DURUM RAPORU 6 (2026-09-04, build yok — kod okuma + upstream PR diff karşılaştırması)

**#552 upstream'te 5 ayrı fix taşıyan bir PR (açık, merge edilmemiş). PR diff'i alınıp fork'a satır satır karşılaştırıldı:**

| #552 fix'i | Fork'ta durum | Detay |
|---|---|---|
| 1. Streaming `.next()` ilk-geçerli-JSON (TrailingCharacters) | ❌ **EKSİK** | Fork `parse_streamed_tool_arguments` (model.rs:3017) düz `serde_json::from_str` + kontrol-karakter sanitizasyonu kullanıyor; upstream `.next()` ile ilk geçerli JSON bloğunu alıp trailing metni yutuyor. Fork'ta trailing metin → `ProviderError` → **tüm tur ölüyor** (panic değil ama sonuç kaybı). |
| 2a. `git worktree remove --force` | ❌ **EKSİK** | git.rs:254 `remove_worktree` — `--force` YOK → kirli (dirty) worktree kalıcı olarak silinemez |
| 2b. Multirepo worktree namespace | ⚠️ KISMEN | Fork'ta upstream'in `project_manage` tool'u YOK — yerine kendi API'si var (api/projects.rs:857). Multirepo'da worktree ismi sadece branch (`/`→`-`), repo adı prefix DEĞİL → iki repo aynı branch adını kullanırsa çakışma (upstream fix: `.worktrees/repo-worktree`) |
| 3. UTF-8 `floor_char_boundary` | ⚠️ **KISMEN** | channel.rs:4395 ✅ (zaten `floor_char_boundary(200)`); **signal.rs:1009 hâlâ `&body_text[..200]`** — Türkçe/emoji çok baytlı karakterde panic riski (sadece debug log yolu ama gerçek) |
| 4. Slack E prefix (target.rs) | ⚠️ KISMEN | Fork genel `UPPER+all-digit` yakalıyor (E-digit'lar dahil) ama **harf içeren** Slack ID'lerini (`T024BE7LD`) kaçırıyor; upstream `T/C/E + UPPER\|DIGIT` kapsıyor |
| 5. `litellm/` strip (model.rs) | ✅ **N/A** | Fork litellm KULLANMIYOR (src'te sıfır eşleşme); fork'un `remap_model_name_for_api` (model.rs:4487) sadece `zai/` strip yapıyor — kendi ihtiyacına uygun |
| bonus: `listen_only_mode` deprecation warn (config/load.rs) | ✅ VAR | load.rs:240 — upstream'le aynı |

**Sonuç:** #552'nin **gerçek değeri 3 maddede**: (1) streaming tool-args'ta trailing-text toleransı, (2) `remove_worktree --force`, (3) signal.rs UTF-8 slice. Üçü de küçük, bağımsız, akşam build listesine eklenebilir.

**Fix tarifi (hepsi <10 satır, akşam build'e eklenir):**
1. `model.rs` `parse_streamed_tool_arguments`: `serde_json::Deserializer::from_str(raw).into_iter::<Value>().next()` ile ilk geçerli JSON'u al (sanitize yoluna da aynısı) — trailing text artık turu öldürmez.
2. `projects/git.rs` `remove_worktree`: `.args(["worktree", "remove", "--force", ...])`.
3. `messaging/signal.rs:1009`: `&body_text[..200]` → `&body_text[..body_text.floor_char_boundary(200)]`.
4. (opsiyonel) `messaging/target.rs` `is_valid_instance_name`: `T/C/E + UPPER|DIGIT` kuralına genişlet — ama fork Slack aktif değilse düşük öncelik.
5. Multirepo worktree ismine repo prefix'i (fork'un kendi API'si için) — proje yönetimi kullanılıyorsa.

### 10. #504 — Worker'lar büyük tool çıktısını kırpsın
- `cat`/`find`/`journalctl` context'i dolduruyor → otomatik truncation.
- **→ ✅ ÇOĞU ÇÖZÜLMÜŞ (2026-09-04 inceleme — #504'ün özü fork'ta VAR):**

### #504 → DURUM RAPORU 5 (2026-09-04, build yok — kod okuma)

| #504 bileşeni | Fork'ta durum |
|---|---|
| Tool çıktı otomatik kırpma | ✅ VAR — `tools.rs:386` `MAX_TOOL_OUTPUT_BYTES = 50_000`; `truncate_output()` UTF-8 güvenli + not ekliyor |
| Kırpma notu | ✅ VAR — `[output truncated: showed X of Y bytes (N bytes omitted). Use head/tail/offset...]` |
| shell stdout/stderr | ✅ VAR — ikisi de AYRI AYRI 50KB'a kırpılıyor (shell.rs:419-425 sync, 575-576 stream) |
| file read | ✅ VAR — 50KB + ayrıca satır tabanlı offset/pagination notu (file.rs:255-268) |
| Hook/SSE event kırpması | ✅ VAR — on_tool_result event payload'ı da kırpılıyor (spacebot.rs:957, 1444) |
| Context-overflow kurtarma | ✅ VAR — worker.rs/branch.rs overflow'da compact + retry (`MAX_OVERFLOW_RETRIES`) |
| **Per-tool farklı limit** | ❌ YOK — tek global 50KB sabiti, config'ten ayarlanamıyor |
| **API öncesi proaktif context kontrolü** | ❌ YOK — sadece overflow OLUNCA kurtarma (reaktif), önleme yok |

**Kalan boşluk analizi:**
1. **Eşik upstream'in önerdiği 4-8KB'ın ~6-12 katı.** Tek shell çağrısı stdout+stderr **toplam 100KB** üretebilir;
   Inception 128k penceresinde 2-3 büyük çıktı + channel history fork'u overflow'a yaklaştırır. Kırpma VAR ama eşik yüksek.
2. Proaktif önleme yok — ama bu büyük mimari parça (worker fork'u zaten channel kompaktörüyle yönetiliyor), #504'ün
   "öncelik" kısmı DEĞİL; kurtarma mekanizması overflow'u öldürücü olmaktan çıkarıyor.

**Fix tarifi (opsiyonel — #504 zaten çalışıyor, bu iyileştirme):**
- `MAX_TOOL_OUTPUT_BYTES`'ı sabit yerine config'e taşı (`[defaults] tool_output_max_bytes`, default 16_000-24_000) —
  makul orta yol: upstream 4-8KB, biz 50KB. Düşük eşik kod işlerini kırpabilir, bu yüzden 16-24KB makul.
- Değeri düşürmeden ÖNCE canlı gözlem: worker log'larında kaç tool sonucu 8-50KB arası — kırpma gerçekten tetikleniyor mu?
- Proaktif context check'i ÖNCELİK 2'nin ilerisinde ayrı iş olarak değerlendir (#552 streaming ile birlikte).

### 11. #604 — Memory ingestion + merge bloat fix
- Duplike döngü düzeltmesi (additive migration var — dikkat).

### #604 → DURUM RAPORU 9 (2026-09-04: 4 fix tek tek karşılaştırıldı — Fix 1 dışı KOD YAZILDI, check ✅)

| #604 fix'i | Fork'ta durum |
|---|---|
| 1. Deterministic chunk completion (contract sadece observability) | ⚠️ FORK TERS YÖNDE — `ingestion.rs:556` `!contract_state.has_terminal_outcome() → Err`. Ama #538 korumasıyla çelişir; chunk-retry duplikesini bugün eklenen memory_save dedup'ı zaten önlüyor → **UYGULANMADI (bilinçli)** |
| 2. Bounded retries + backoff + quarantine | ❌ YOK — failed dosya **her 30 sn poll'da sonsuza dek** yeniden denenir (cap/backoff/quarantine yok); progress-resume başarılı chunk'ı atlar ama fail eden chunk her cycle LLM çağırır → token israfı (upstream'in birebir şikayeti). **KOD YAZILDI** (yeni migration + gating) |
| 3. Ingest delete disk dosyasını da silsin | ❌ YOK — `api/ingest.rs:246` sadece row siler; dosya diskte kalır → poll tekrar keşfeder ("geri gelir") + `ingestion_progress` purge edilmez. **KOD YAZILDI** |
| 4. Canonical merge (concat yerine) | ❌ CONCAT VAR — `maintenance.rs:329` `format!("{winner}\n\n{loser}")` (byte cap'lı ama bloat sürer). **KOD YAZILDI** |

**Fork'un iyi olan kısımları (upstream'in farkında olmadığı):** progress tablosu chunk-seviyesinde
(`content_hash + chunk_index`, INSERT OR IGNORE), başarılı chunk retry'de atlanıyor, dosya başarıda
diskten siliniyor (#48 düzeltmesi: failure'da dosya+progress retry için tutulur — bilinçli).

**Fix 2 detayı (yazılan):** `ingestion_files`'a `attempts` + `next_attempt_at` (YENİ additive migration
`20260904000001_ingestion_retry_budget.sql` — mevcut migration'lara dokunulmadı) + `IngestionConfig.max_attempts`
(default 5) + üstel backoff (60s × 2^attempt, tavan 1 sa) + max sonrası `quarantined`. Poll gating: satır
quarantined veya next_attempt_at gelecekteyse dosya atlanır. status'ta CHECK yok → quarantined schema değişikliği
gerektirmez (upstream notuyla aynı).

**Fix 3 detayı (yazılan):** `delete_ingest_file` — row'u silmeden önce filename'ı oku → `ingest_dir` içindeki
dosyayı diskten sil → `ingestion_progress`'i content_hash ile purge et.

**Fix 4 detayı (yazılan):** `merged_memory_content` → kazananın içeriğini canonical tutar (loser concat edilmez);
winner boşsa loser. Bugünkü exact-dedup (#538-2) near-duplicate'leri kapsamadığı için bu tamamlayıcı.

**Canlı doğrulama (akşam):** (2) ingest'e sürekli fail eden dosya koy → 5 deneme sonrası 'quarantined' görünmeli,
poll log'unda tekrar denenmemeli. (3) ingest dosyası yükle → delete API çağır → dosya diskte yok + progress boş.
(4) benzer 2 hafıza yaz → maintenance merge çalıştır → survivor content concat DEĞİL, tek canonical metin.

### 12. #543 — Cortex karar takibi (decision provenance)
- Kimin neye karar verdiğini Decision memory olarak yaz — uzun vadeli hafıza kalitesi.

### #543 → DURUM RAPORU 10 (2026-09-04: prompt-only PR — UYGULANDI, check gerekmez)

**Upstream #543 prompt-only'dir:** cortex prompt'una "Decision Provenance" bölümü + over/under-capture kuralı.

**Fork'ta zaten olanlar:** `MemoryType::Decision` uçtan uca çalışıyor (store/search/render "Decisions"/
API filtresi). Branch'ler `decision` türünde kayıt öğütleniyor (branch.md.j2 kural 6); compactor +
`memory_persistence` fragment'ı `decision`/`decision_revised` event + 0.6-0.8 importance kapsıyor;
çelişki bağlama (`Contradicts`/`Updates`) cortex Priority 2'de var.

**Eksik olan (uygulandı):**
1. `cortex.md.j2` → yeni **"Priority 3: Decision Provenance"** (kim karar verdi / ne / gerekçe / alternatifler /
   elevated importance 0.7-0.9 + revizyonu `Contradicts`/`Updates` ile bağla); Progression → **Priority 4** olarak
   yeniden numaralandı; Rules'a **#8** (over-capture vs under-capture) eklendi.
2. `branch.md.j2` kural 6 `decision` maddesi → kararın gerçek kayıt noktası (branch'ler) provenance yapısıyla
   genişletildi (who/what/rationale/alternatives + elevated importance + revizyon bağlama).

**Not:** Prompt'lar `include_str!` ile derleniyor → etki akşam build + daemon restart sonrası. Kod değişmedi,
compile riski yok. **ÖNCELİK 2 BÖYLECE TAMAMEN KAPANDI** (#552, #504, #324, #653+#16, #604, #543).

---

## 🟢 ÖNCELİK 3 — İyi ama acil değil

| # | Konu |
|---|---|
| #649 | Coding worker context + OpenCode session takibi (omp işimizle uyumlu) |
| #534 | Shell komut ön-analizi (güvenlik) |
| #542 | Sandbox read-only yollar (writable + readable orta seviye) |
| #287 | Cortex topic synthesis (tek bulletin yerine konu bazlı bağlam) |
| #554 | Gerçek zamanlı token/context göstergesi |
| #577 | Prompt cache sınırı (Anthropic — Inception'da kısmen) |
| #326 | Telegram forum topic ayrımı (forum kullanılıyorsa) |

---

## ❌ ELENENLER (bu fork için)
- Adaptörler: #607 Teams, #418 WhatsApp, #236 XMPP, #161 Twitch, #310 Signal, #404 Teams
- GUI/UI estetik: #200/#558 Windows, #121/#419/#602 mobil, #585/#589 tema, #586/#587 sidebar, #594/#598 scroll, #600 Enter
- Provider: #430 ChatGPT OAuth, #254 openai-chatgpt, #410 reasoning, #563 Gemini native (google native kullanıyorsan tekrar bak)
- Devasa: #580 Code Graph Memory, #613 Task/DAG (WIP), #561 hierarchy
- Kod kalitesi: #17 runtime refactor (geniş — dikkatli), #375/#576 docs
- Podman/docker: #208, #152 CDP browser

---

## ⚠️ Genel notlar
- Tüm PR'lar upstream'te **açık/merge edilmemiş** → kod önerisi, "hazır özellik" değil. Cherry-pick'te çakışma olabilir.
- Her PR öncesi: `git log` ile fork'ta zaten var mı kontrol et (bazı fix'ler upstream main'ine girmiş olabilir).
- #653/#651/#650 (upstream autonomy/worker işleri) bizim phase-21/supervisor ile örtüşüyor — çakışma değil, doğrulama: upstream aynı yöne gidiyor.
- Her ekleme sonrası: `cargo check` + ilgili testler + daemon restart + canlı doğrulama.

---

# BÖLÜM 2 — Upstream ISSUE'larından tespitler

> Kaynak: spacedriveapp/spacebot issues (tümü okundu: ~166 açık+kapalı).
> Bu bölüm yalnızca **bu fork'un kurulumuna ve yaşadığımız sorunlara** göre seçilmiştir.

---

## 🔥 KRİTİK — Yaşadığımız sorunların resmi kayıtları

### I-1. #363 — Universal Memory Gap: Telegram/Discord sohbetleri hafızaya işlenmiyor
- **Sorun:** Messaging adaptörleri mesajı işliyor, agent cevap veriyor, ama konuşma **agent'ın hafıza sistemine commit edilmiyor**.
- **Bizdeki karşılığı:** "spacebot nedir" sorununda bot kendini tanımıyordu; platform/oturum değişince bağlam kayboluyor. Kullanıcının "haftalarca konuştuk beni tanıyordu" şikayeti bunun ta kendisi.
- **Yapılacak:** Telegram/webchat konuşma geçmişinin hafıza (memory) boru hattına commit edildiğini doğrula; eksikse adaptör → ingestion bağlantısını kur.
- **Doğrulama:** Telegram'da uzun konuş, sonra "ben kimim / ne konuşmuştuk" diye sor — hafızadan cevap al.

### I-2. #581 + #195 — Worker sonucu Telegram'a ulaşmıyor (retrigger relay failure)
- **Sorun:** `worker completed, result queued for retrigger` → `retrigger relay failed` → kullanıcı kısaltılmış/eksik cevap görür.
- **Bizdeki karşılığı:** Dün log'da birebir görüldü (telegram:474374538, 15:27 civarı).
- **Not:** PR #652 ve #367 bu issue'nun çözüm adayları — Öncelik 1'de listelendi.

### I-3. #518 + #273 — UTF-8 char boundary panikleri (kanal donması)
- **Sorun:** `&s[..200]` gibi byte-kesme işlemleri çok baytlı karakterde (Türkçe, emoji, Kiril) panic → kanal çöker.
- **Bizdeki karşılığı:** Dün `spacebot.err`'de aynı panic görüldü (11:20). Türkçe mesajlarla tetikleniyor.
- **Yapılacak:** `src/agent/channel.rs` + `signal.rs` + tüm `&s[..n]` desenlerini tara, `floor_char_boundary` ile değiştir (kısmen yapıldı, eksikleri var).

### I-4. #325 + #156 — Zombi / phantom worker'lar (worker_runs "running"de takılı)
- **Sorun:** Worker beklenmedik ölünce (API hatası, timeout, kill) `worker_runs` kaydı sonsuza dek "running" kalır.
- **Bizdeki karşılığı:** DB'de "cancelled"/takılı worker'lar + "run marked dead" kayıtları (15h-20h önce). Dün 1 tanesini elle kapatmıştık.
- **Yapılacak:** Worker ölümünde kaydı reconcile eden mekanizma (restart reconciliation zaten kısmen var — genişlet).

### I-5. #593 — Cortex timer token israfı (fatura riski)
- **Sorun:** Boşta beklerken cortex 60sn'de bir sentez çalıştırıp token planını yakıyor ($100-200 / birkaç gün).
- **Bizdeki karşılığı:** Run History'deki onlarca gereksiz "Verified after work: CI yok" kaydı (25+ tekrar) benzer israf. Inception mercury-2 pahalı.
- **Yapılacak:** Autonomy interval'i (30dk → daha seyrek), verify-wake'in aynı sonucu tekrar tekrar üretmesini engelle (dedup), cortex tick intervalini gözden geçir.

---

## 🟡 AÇIK BUG'LAR — bizi etkileyebilir

| # | Konu | Neden bizim için önemli |
|---|---|---|
| #538 | Hafıza duplike: `memory_persistence_complete` sonrası model metin üretince false negative → tekrar kaydı | Inception (non-Anthropic) kullanıyoruz |
| #504 | Worker'lar büyük tool çıktısını otomatik kırpmıyor → context overflow | `cat`/`find`/`journalctl` worker'ları |
| #503 | `model_context_window_exceeded` yanlışlıkla "retriable" sayılıyor → sonsuz döngü | API hatası döngüsü riski |
| #436 | `task_update` "success" döner ama status değişmez (branch task'ları) | Task board güvenilirliği |
| #438 | `spawn_worker` task_type'ı routing'e iletmiyor — task_overrides yutuluyor | Model yönlendirme config'i |
| #429 | bwrap "Operation not permitted" — shell fail | Sandbox'ı açıp kapattığımız alan |
| #469 | Kanal mesajı gelirken "Already borrowed" → LLM çağrısı fail | Yoğun kanalda çökme riski |
| #258 | Worker stall + stale "completed" → tekrarlayan alakasız cevaplar | Dünkü tekrar eden cevap sorunuyla ilişkili |
| #240 | skip çağrısında 400 "text content blocks must be non-empty" → kanal asılır | Uzun konuşmalarda |

---

## 🟢 FAYDALI ÖZELLİK İSTEKLERİ (düşük maliyet)

| # | Konu |
|---|---|
| #282 | Telegram slash komut desteği |
| #355 | Agent başına LLM model/provider key |
| #354 | Agent başına secret izolasyonu |
| #99 | Tek instance'ta birden fazla Telegram bot token'ı |
| #320 | Secrets guard'ı kapatma seçeneği (test ortamı için) |
| #226 | Obsidian uyumlu hafıza |
| #438 | Per-repo worker modeli (registry'den) |

---

## ❌ ELENENLER (senin için gereksiz)
- Adaptörler: Teams (#404), WhatsApp (#418), XMPP (#236), Twitch (#161), Signal (#310)
- GUI: Windows/mac desktop (#234), Home Assistant (#445), Vercel AI SDK (#433), NVIDIA OpenShell (#488)
- Estetik: tema (#585/#589), sidebar (#586/#587), mobil (#601), scroll (#594)
- Multi-user RBAC (#584), GNAP (#435), Discord forum etiketleri (#424)

---

## ⚠️ Genel not
- Bazı issue'lar upstream'te kapatılmış (#611, #610, #391 vb.) — çözüm upstream main'ine girmiş olabilir; uygulamadan önce `git log` ile fork'ta var mı kontrol et.
- Issue #656 (12 agent prod raporu) ayrıca değerlendirilmeli — config schema + non-atomic yazma bizim elle config düzenleme akışımızı da etkiler.


---

# BÖLÜM 3 — SES / JARVİS (kullanıcı hedefi: Jarvis yapımı)

> Kullanıcı hedefi: Spacebot'u Jarvis benzeri sesli asistana dönüştürmek.
> Telegram sesli mesajları birincil giriş olacak.

---

## 🎯 MEVCUT DURUM (tespit edildi — 2026-09-04)

- Spacebot **sesli mesaj transcription'ı kod olarak ZATEN destekliyor**: `src/agent/channel_attachments.rs`
  - `audio/*` MIME → indir → `transcribe_audio_attachment()` → `routing.voice` modeliyle yazıya çevir
- **AMA config'te `voice` modeli AYARLANMAMIŞ** (`routing.voice` boş → sessizce atlıyor)
  - `~/.spacebot/config.toml`'da voice yok
  - Env: `SPACEBOT_VOICE_MODEL` ile set edilebilir
- Ses çıkışı (TTS): Spacebot'ta yok — Telegram'a metin cevap veriyor

## 📋 YAPILACAKLAR

### S1. Mevcut STT'yi aktifleştir (hızlı kazanım)
- `routing.voice`'a bir model tanımla: örn. `openai/whisper-1` veya yerel whisper
- VEYA PR #177'deki `whisper-local://<spec>` backend'ini bekle
- **Doğrulama:** Telegram'a sesli mesaj gönder → bot yazıya çevirip cevap versin

### S2. Upstream PR #382 — Transcription refactor (daha sağlam)
- Sesli mesaj → metin akışını güçlendirir (voice message handling iyileştirmeleri)
- PR açık (merge edilmemiş) — cherry-pick adayı

### S3. Upstream PR #177 — Yerel Whisper backend + transcribe_audio worker tool
- `whisper-local://` ile yerel (ücretsiz, çevrimdışı) transcription
- Worker'lara `transcribe_audio` tool'u ekler
- PR açık (merge edilmemiş) — Jarvis için değerli (API maliyeti yok)

### S4. Ses çıkışı (TTS) — Spacebot'ta YOK, Jarvis için gerekli
- Telegram'a sesli yanıt göndermek için TTS gerekir (örn. OpenAI TTS / yerel piper)
- Kodda yok → yeni geliştirme (adaptör: Telegram sendVoice + TTS backend)
- Ayrı tasarım gerektirir (kanal başına ses aç/kapa)

### S5. Uçtan uca Jarvis akışı (vizyon)
- Telegram sesli mesaj → STT (S1/S2/S3) → agent işle → TTS (S4) → sesli cevap
- İsteğe bağlı: always-on dinleme (webhook/stream), wake word

---

## 🔗 İlgili upstream kaynakları
- PR #382 (transcription refactor) — open, merge edilmemiş
- PR #177 (whisper-local + transcribe_audio tool) — open, merge edilmemiş
- Issue #656 bölüm 4 (dedicated voice-transcription endpoints — prod kullanıcı isteği)

## ❌ Şu an yapma (zamansız)
- Tam sesli arama/konuşma (realtime STT) — Spacebot mimarisi kanal-bazlı; önce S1-S4 olgunlaşsın


---

### #438 → DURUM RAPORU 11 (2026-09-04: spawn_worker task_type routing — KOD YAZILDI, check ✅ 45sn nice'li)

**Upstream:** `spawn_worker` `task_type`'ı routing'e iletmiyor — `[defaults.routing.task_overrides]`
parse ediliyor ama hiçbir etkisi yok. Önerisi: `SpawnWorkerArgs.task_type` → `Worker::new()` →
`routing.resolve(Worker, task_type)`.

**Fork'ta durum (birebir aynı çatlak):** `routing.resolve()`'in TÜM çağrıları `None` ile yapılıyor
(worker.rs:642, channel_dispatch.rs:791/2103, branch.rs:123...). `task_overrides` HashMap'i tanımlı,
testlerde dolu, ama üretim kodunda hiçbir spawn task kategorisi taşımıyor → config sessizce yutuluyor.

**Uygulanan (minimal riskli tasarım):**
- `SpawnWorkerArgs`'a `task_type: Option<String>` + JSON schema (açıklama: routing.task_overrides
  anahtarı; opencode/acp kendi model config'ini kullandığı için sadece builtin worker'ı etkiler)
- `spawn_worker_from_state` + `spawn_worker_inner`'a `task_type: Option<&str>` parametresi
  (tek üretim çağrısı: tools/spawn_worker.rs)
- `worker_model_override` çözümü: conversation override > **task override** > runtime routing default.
  Task override yalnızca `routing.worker` default'undan FARKLIYSa uygulanır (eşleşmeyen task_type
  runtime davranışını değiştirmez, hot-reload korunur)
- Worker::new imzalarına dokunulmadı (acp/opencode + test çağrıları etkilenmez)

**Doğrulama:** cargo fmt temiz + cargo check --all-targets ✅ (45 sn nice'li). Test çalıştırma akşam build'de.
**Canlı test tarifi:** config'e `[defaults.routing.task_overrides] coding = "<model B>"` ekle →
spawn_worker'a `task_type: "coding"` ver → worker model B ile çalışmalı; `task_type` verilmezse
davranış değişmemeli.


---

### #503 → DURUM RAPORU 12 (2026-09-04: context-overflow yanlış sınıflandırma — KOD YAZILDI, check ✅)

**Upstream (#503):** `model_context_window_exceeded` hatası (Anthropic tarzı stop_reason) içerik boş
gelince yanlışlıkla "retriable/transient" sayılıyor → sonsuz retry döngüsü ve token israfı.

**Fork'ta durum:** Fork pozitif liste (allowlist) kullanıyor — upstream'in denylist bug'ı birebir yok.
AMA kilit boşluk doğrulandı: Anthropic tarzı `stop_reason: context_window_exceeded` **boş içerikle**
gelince fork "empty response..." hatası üretiyor → `is_context_overflow_error` bunu tanımıyor (pozitif
listede "context_window_exceeded" yoktu), ama `is_retriable_error` "empty response" ile eşleşiyor →
overflow, transient retry yoluna düşüyor (upstream #503'ün fork'taki hali).

**Uygulanan (2 dosya):**
- `llm/routing.rs` `is_context_overflow_error`: (a) rate-limit reddi ("429/rate limit/too many
  requests") artık overflow sayılmıyor — öğrenilen context tavanı yanlışlıkla düşmüyor; (b)
  `context_window_exceeded` / `context_window_limit_reached` / `context_length_exceeded` stop_reason
  marker'ları overflow olarak tanınıyor.
- `llm/model.rs` Anthropic boş-yanıt yolu: stop_reason'ı context limiti olan boş completion artık
  **overflow hatası** olarak yüzeye çıkıyor (worker compact + tek retry; learned ceiling düşer) — mesaj
  bilinçli olarak "empty response" retriable anahtar kelimelerini içermiyor (transient yoluna düşmesin).

**Doğrulama:** cargo fmt temiz + cargo check --all-targets ✅ (45 sn nice'li, #438 ile birlikte).
**Canlı test tarifi:** config'te context penceresini aşan dev bir task ver (veya proxy ile
`stop_reason: model_context_window_exceeded` + boş içerik döndür) → worker compact edip tek retry
yapmalı, sonsuz döngüye girmemeli.


---

### #436 → DURUM RAPORU 13 (2026-09-04: task_update "success ama status değişmiyor" — fork'ta ÜRETİLEMİYOR, kod gerekmedi)

**Upstream (#436):** `task_update` "success" döndürüyor ama branch-created görevin durumu değişmiyor;
worker_id bağlı, completed_at null, görev "backlog"da kilitli kalıyor.

**Fork'ta durum (kanıt zinciri — satır satır):**

| #436 bileşeni | Fork'ta durum |
|---|---|
| Geçersiz geçişte "silent success" | ❌ **YOK** — `update_current_in_tx` (store.rs:1426-1433) Enforce altında geçersiz geçişte `TaskError::InvalidTransition` döner; tool bunu `map_err(TaskUpdateError)` ile hata olarak yüzeye çıkarır. `rejects_invalid_status_transition` testi (store.rs:2302) bunu assert ediyor |
| Worker'ı "backlog"a bağlanıp kilitlenme | ❌ **YOK (tasarım)** — `resolve_task_plan` (tools/spawn_worker.rs:84-95) `PendingApproval \| Backlog` göreve spawn'ı REDDEDİYOR: "must be approved (ready) before work starts". Bağlama (spawn_worker.rs:738) Ready→InProgress çevirir; yani bağlı worker her zaman InProgress → Done geçişi yasal |
| completed_at null kalması | ✅ ÇÖZÜLMÜŞ — store.rs:1458-1463: InProgress→Done geçişinde completed_at SET edilir |
| Operatör (dashboard/API) ihtiyacı | ✅ VAR — `update_with_status_override` / `update_with_dependencies_and_status_override` (Override policy) + testler (store.rs:2354, 2383) |

**Sonuç:** Upstream'in "silent success" bug'ı fork'ta **üretim kodunda yok** — Enforce + hata yayılımı + Override API üçlüsü bug'ı kapatmış. Kalan tek "sertlik": ajan, worker olmadan kendi bitirdiği görevi tek çağrıda Backlog→Done yapamaz (merdiven yürümeli: Backlog→Ready→InProgress→Done — her adım yasal). Bu bilinçli yaşam-döngüsü koruması; upstream'in "tek çağrıda done" beklentisiyle çelişiyor ama sessiz kayıp değil, açık hata.

**Aksiyon: kod değişikliği YOK.** Bu incelemelerde "baktık, gerek yok — zaten çözülmüş" kategorisinde kapandı. Akşam build listesine eklenmedi.


---

### #469 → DURUM RAPORU 14 (2026-09-04: "Already borrowed" — fork'ta REENTRANCY YOK; hata provider kaynaklı, 1 satırlık isteğe bağlı iyileştirme)

**Upstream (#469):** Kanalın kendi LLM çağrısı, branch/worker stream'leri aktifken "CompletionError:
ProviderError: OpenAI-compatible streaming error: Already borrowed" ile ölüyor. 5-6 eşzamanlı görevde
tetikleniyor. Issue sahibi "RefCell, rig ya da HTTP streaming katmanında" diye tahmin ediyor.

**Yöntem:** build yok — kaynak taraması (fork src + rig-core 0.33 + tüm registry) + upstream diff karşılaştırması + log geçmişi.

**Kanıt zinciri — fork'ta yerel reentrancy panik ihtimali YOK:**

| Kontrol | Sonuç |
|---|---|
| `RefCell` / `borrow_mut` fork src'sinde | ❌ **SIFIR** — tüm paylaşılan durum tokio `RwLock` + `ArcSwap` + atomics + std `Mutex` (bunlar panik değil bekler/asar) |
| rig-core 0.33.0 kaynağında RefCell | ❌ **SIFIR** (tüm `src/` tarandı) |
| `LlmManager` (tek çapraz-süreç paylaşım noktası) | `config`/`context_ceilings`: `ArcSwap`; `rate_limited`/oauth: tokio `RwLock` — stream boyunca guard tutulmuyor, per-call state `stream_openai*` içinde lokal (`Vec::new()` vb.) |
| Kanal LLM yolu eşzamanlı girebilir mi | ❌ HAYIR — tek run-loop task'ı `message_rx`'i seri tüketiyor, `handle_message(&mut self)`; `turn_active` atomic + `TurnActiveGuard` (busy policy) ikinci turu engelliyor; her sürecin kendi rig Agent'ı + kendi ToolServer'ı var |
| Hook içi std `Mutex`'ler (`outcome_text`, `loop_guard` vb.) | Per-süreç, prompt içinde seri — çift-borrow yok |

**Peki hata metni fork'ta görünebilir mi? EVET — ama kaynak provider:**

- Hem upstream (3092) hem fork (model.rs:3108-3111) SSE `error` event'ini **aynen relay** ediyor:
  `event_body["error"]["message"]` → `"OpenAI-compatible streaming error: {message}"`.
- "Already borrowed" std `RefCell` borrow panic mesajıdır — bu, **Chutes AI gateway'inin kendi içinde**
  (5-6 eşzamanlı istek altında) panikleyip hata event'ini SSE'ye basmasıdır. Spacebot sadece taşıyor.
- Bu makinenin logları: "Already borrowed" **hiç geçmemiş** (tüm `~/.spacebot/logs` tarandı) — inception
  gateway'i bu paniği üretmiyor.

**Kalan boşluk (isteğe bağlı iyileştirme, 1 satır):**

- Fork'un `is_retriable_error` allowlist'i (routing.rs:123-149) "already borrowed"u **kapsamıyor** →
  provider gateway anlık paniklerse kanal turu retry'siz ölür (upstream'in birebir semptomu).
- Öneri: allowlist'e `|| lower.contains("already borrowed")` eklenir → geçici provider iç-hatası
  otomatik retry edilir (#503 sınıflandırıcı fix'iyle aynı desen; yanlış-pozitif riski: bu metin yalnızca
  RefCell panik mesajıdır, istek-deterministik bir hata değildir — retry güvenli).
- KARAR: şimdilik kod değişikliği YAPILMADI (inceleme emri buydu). İstenirse akşam build listesine #16
  olarak eklenir — `cargo check` 1 dakika, risk sıfır.

**Sonuç:** #469 "yeniden giriş (reentrancy)" iddiası fork'ta **çürütüldü** — yerel kodda RefCell yok,
kanal seri, rig temiz. Semptom ancak provider gateway paniğiyle oluşur; tek makul yanıt o paniği retry
etmektir (opsiyonel 1 satır).


---

### B-kategorisi 3 kalem → DURUM RAPORU 15 (2026-09-04: #469 + #653 küçük yama + #504 config — KOD YAZILDI, check ✅ 37 sn nice'li)

**B1) #469 (1 satır):** `llm/routing.rs is_retriable_error` allowlist'ine `"already borrowed"` eklendi —
provider gateway'in kendi iç panic'ini (Chutes örneği) retry edilebilir sayar. Test güncellendi
(`is_retriable_error_catches_server_error_phrases` + "OpenAI-compatible streaming error: Already borrowed").

**B2) #653 küçük yama — agent seviyesinde detached worker iptali (kanal gerektirmez):**
- `AgentDeps.detached_workers` (lib.rs) + 3 üretim literal'ı (api/agents.rs ×2, main.rs) + 2 test
  fixture'ı (behavioral_fixtures, context_dump) boş registry ile başlatıldı.
- `tools/spawn_worker.rs` cortex/autonomy detached spawn'ı artık `spawn_worker_task`'ın dönen
  `WorkerTaskControl`'ünü **düşürmüyor** — agent registry'sine kaydediyor; 2 sn'lik watcher görev
  bitince kaydı siliyor (iptal edilmişse entry yok → watcher kendini durdurur).
- `ApiState.detached_worker_registries` + `register_detached_workers()` + `cancel_detached_worker()`
  (state.rs); main.rs'te her kanal kaydında agent registry'si API'ye veriliyor (2 yer).
- `api/channels.rs cancel-process`: kanal bulunamazsa DB fallback'inden ÖNCE agent-seviyesi arama —
  cortex worker'ı hâlâ canlıysa cancel_tx + 500 ms join + abort ile gerçekten durduruluyor
  (önceden sadece DB satırı "cancelled"a çekiliyordu, görev koşmaya devam ediyordu — #653 çatlağı).

**B3) #504 — tool çıktı eşiği config'e taşındı (`[defaults] max_tool_output_bytes`):**
- **Gözlem:** `[output truncated: showed` işareti log'larda ve DB'de hiç geçmiyor (50 KB eşik pratikte
  tetiklenmiyor) → davranışı bilinçsiz değiştirmemek için **varsayılan 50 KB korundu** (DURUM RAPORU
  5'in 16-24 KB önerisi GEREKSİZ risk — kullanıcı kendi büyük `ls`/`find` işlerini kırpardı). Knob
  açık: isteyen `max_tool_output_bytes = 24000` yazar.
- Zincir: `tools.rs` `TOOL_OUTPUT_LIMIT` AtomicUsize + `set_tool_output_limit()`/`tool_output_limit()`;
  config zinciri (toml_schema → DefaultsConfig → load.rs → ResolvedAgentConfig); `runtime.rs::new`
  başlangıçta globali config'ten besliyor; **14 kırpma çağrı noktasının tamamı** (shell ×4, file ×2,
  hooks ×2, worker_transcript ×3, opencode ×1, mcp ×1, read_skill ×1) artık `tool_output_limit()`
  okuyor — tek kaynak, restart ile uygulanır.

**Akşam build listesi artık 18 kalem** (önceki 15 + #16 #469 + #17 #653 + #18 #504). Doğrulama:
cargo fmt temiz + cargo check --all-targets ✅ (37 sn nice'li, test kodları dahil). Tek uyarı #604'ten
kalma ölü sabit (`MAX_MERGED_MEMORY_CONTENT_BYTES` maintenance.rs:18) — bu işle ilgisiz, istendiğinde
ayrıca silinir. Canlı doğrulama (akşam build sonrası): (1) config'e `max_tool_output_bytes = 24000`
yaz → restart → dev `cat` çıktısında kırpma notu; (2) cortex_chat'ten detached worker spawn et → API
`/channels/cancel-process` (kanal_id boş/yanlış) ile iptal et → worker_runs "cancelled" + görev durdu.


---

### I-3 (UTF-8 byte-kesme) → DURUM RAPORU 16 (2026-09-04: tüm src tarandı, 4 bug sınıfı KAPATILDI — check ✅ 43 sn nice'li)

**Yöntem:** tüm `[..n]`/`[..var]` dilimleri (~120) + `.truncate()` riskleri satır satır doğrulandı; güvenli
olanlar (floor_char_boundary öncesi, char_indices/find-ASCII, Vec/byte dilimleri) elendi.

**Kapatılan gerçek panik riskleri:**

| # | Dosya(lar) | Sorun | Fix |
|---|---|---|---|
| 1 | `acp/worker.rs`, `agent/channel.rs` ×3, `hooks/spacebot.rs` ×3, `opencode/worker.rs` ×2, `tools/reply.rs` ×2, `tools/set_outcome.rs` (12 yer) | `&leak[..leak.len().min(8)]` — leak Türkçe/emoji içerirse 8. bayt sınır değil → **panic** | `floor_char_boundary(min(8))` |
| 2 | `llm/model.rs` `truncate_body` | hata gövdesi `&body[..500]` ham byte kesme | `floor_char_boundary(500)` |
| 3 | `messaging/signal.rs` `redact_identifier` | `&id[..4]` + `&id[len-4..]` her iki uç sınır garantisi yok | her iki uca `floor_char_boundary` |
| 4 | `messaging/telegram.rs` + `twitch.rs` `split_message` | `remaining[..max_len]` ham kesme → sonra `.rfind` bile slice panic'ler | slack/discord deseni: `floor_char_boundary(max_len)` arama bölgesi |

**Doğrulanıp güvenli sayılanlar (değişmedi):** kanal/worker/status/working/discord/slack/mattermost
kırpmaları (zaten floor), `char_indices().nth()` kesmeleri (history_repair, daemon, lib, opencode), ASCII
işaretçi bulmaları (mention/marker/url/scheme), `truncate_at_char_boundary` (tools.rs), Vec/`[u8]`
dilimleri, email/scrub/participants walk-back'leri. Acp/types testi yalnızca ASCII `
` sıyırdığı için
güvenli.

**Doğrulama:** cargo fmt temiz + cargo check --all-targets ✅ (43 sn nice'li). Akşam build listesine
kalem eklenmedi (önceki 18 + ölü sabit temizliği ile birlikte derlenecek). Canlı doğrulama: Telegram'dan
emoji/Türkçe yoğun uzun mesaj gönder → split_message artık panic etmeden bölmeli; secret pattern'li
Türkçe içerik log'lanınca leak_prefix kırpılmalı.

## AKŞAM BUILD LİSTESİ — 19) #yeni: 400 "request could not be completed" retriable + hata etiketi (2026-09-07)
- CheaperInference gateway'inin generic 400'ü ("This request could not be completed...") aslında upstream kesintisi — `is_retriable_error`'a ekle (fallback zinciri çalışsın: claude→deepseek→glm).
- `src/llm/model.rs` hata formatı `provider.name` kullanıyor (statik etiket) → hataya **gerçek model id**'yi ekle (`model={model}` alanı), yoksa hangi modelin patladığı anlaşılamıyor.
- Kanıt: 10:57-11:04 arası 4x 400 (autonomy/portal/telegram), 11:11'de kendiliğinden düzeldi; her iki model de 60K+ payload'ı 200 ile karşılıyor.

### ⚠️ DÜZELTME (11 Eyl 2026) — bu kalemin teşhisi EKSİKTİ

`is_retriable_error`'a kalıbı eklemek **tek başına hiçbir şeyi düzeltmezdi**.
Sebep: `src/llm/model.rs::stream()` içinde şu yorum vardı —
*"Streaming has no fallback chain"* — ve **kanal ile worker'lar streaming
kullanıyor** (`prompt_once_streaming`). Yani sahadaki hataların tamamı
(`channel LLM call failed … OpenAI-compatible streaming error`) streaming
yolundan geliyordu ve o yolda ne retry ne fallback **vardı**.

Gerçek düzeltme iki parçalı (11 Eyl'de yazıldı):
1. `stream()` artık `stream_with_fallbacks()` çağırıyor — streaming de
   `attempt_with_retries` ile aynı backoff + fallback zincirini kullanıyor.
2. `is_retriable_error`'a gateway kalıpları eklendi (`could not be completed`,
   `quote the request id`, `request queue is full`) + retriable olmayan hatalarda
   log'a `model=<id>` alanı eklendi.

Ders: hata sınıflandırmasını düzeltmeden önce **hangi yolun o hatayı gördüğünü**
doğrula — yoksa doğru düzeltmeyi yanlış yere yaparsın.
