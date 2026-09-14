# ACP ↔ Intent Köprüsü — Eksik Parça: Sonuç Dönüş Yolu (2026-09-11)

> Mimari bilinçli: **Spacebot cortex → Intent (mini harness) → `omp`/`veyyon` icra.**
> Bu doküman o mimariyi değiştirmiyor; yalnızca eksik olan **dönüş yolunu**
> tarif ediyor. Intent tarafındaki gerçekler `intentd`'nin kendi veritabanından
> ve log'undan doğrulandı (§3).

---

## 1. Sorun

Bugünkü köprü (`~/.local/bin/spacebot-acp-worker`) işi Intent'e **veriyor** ama
sonucu **almıyor**:

```python
ok, info = send_task_to_intent(prompt_text, session_id)   # intentd'e gönder
# … ve hemen dön:
write_msg({... "status": "completed",
           "content": {"text": f"[Intent Bridge] {info}\n"}})
return {"result": {"text": status_msg.strip()}}
```

Yani `session/prompt` çağrısının sonucu, işin çıktısı değil **yönlendirme onayı**
(`[Intent Bridge] Routed to 'spacebot-ile' … Turn ID: …`).

**Neden bu ciddi:**

- `worker_runs.result` alanı sadece `"Worker completed"` yazıyor — Spacebot'un
  kendi DB'sinde işin ne olduğuna dair **tek satır kanıt yok**.
- ACP worker'lar 12-40 saat "done" görünüyor (ort. 43.115 s) çünkü oturum açık
  kalıyor ama içinden anlamlı bir sonuç geçmiyor.
- `harness.md` Pillar 2'nin garantisi *"never get stuck, never silently fail"* —
  bu davranış tam olarak **sessiz başarısızlık**.

**Not:** Bu bir *regresyon*. 10 Eyl 08:49'dan önce config `command = "omp"` idi
ve ACP worker'lar gerçekten iş yapıp dönüyordu (`worker_runs.result` =
`"Worker completed"`, bir kayıt `failed: ACP prompt turn timed out after 600
seconds`). Köprü 10 Eyl 08:49'da 13.343 baytlık Intent köprüsüyle değiştirildi
(yedek: `spacebot-acp-worker.bak`, 1.044 b).

---

## 2. Hedef

`session/prompt` şunu yapmalı:

1. Görevi Intent'e iletmek (bugünkü gibi) — ama onay **ilerleme** olarak,
   sonuç değil.
2. Tur ilerledikçe Spacebot'a **canlı ilerleme** yayınlamak (`session/update`).
3. Tur bitince **gerçek nihai metni** `result.text` olarak döndürmek.
4. Bütçe aşılırsa veya Intent düşerse **açık bir hata** döndürmek — sessiz
   "completed" değil.

Böylece `worker_runs.result` gerçek çıktıyı taşır, kanal kullanıcıya işin
sonucunu gösterir ve harness'ın "kanıtsız done yok" ilkesiyle tutarlı olur.

---

## 3. Doğrulanan gerçekler (intentd tarafı)

Kaynak: `~/.local/share/intentd/intentd.db` şeması + `intentd.2026-09-10.log`.
Sürüm: **intentd 0.9.38**, protokol **9.11**.

### 3.1 Kullanılabilir RPC metotları (log'dan gözlenen)

`agent.sendMessage` · `agent.get` · `agent.stop` · `agent.sendQueuedMessageNow` ·
`workspace.list` · `workspace.get` · `workspace.create` · `event.query`

### 3.2 Olaylar (`event` tablosu: `event_type`, `timestamp`, `session_id`, `data_json`)

| `event_type` | Adet | `data_json` alanları (gerçek örnek) | Bizim için |
|---|---|---|---|
| `agent:stream:status` | 493 | `agentId, workspaceId, phase, message, level, timestamp` | **İlerleme** — `session/update` için |
| `agent:stream:end` | 266 | `agentId, messageId, turnId, lastAgentResponse` | **Nihai metin** |
| `agent:last-message` | 519 | `agentId, messageId, role, turnId, lastAgentResponse, lastToolUse` | Nihai metin + tool bilgisi |
| `agent:idle` | 222 | `agentId, reason:"stream_complete", finishReason:"end_turn", status:"idle", lastResponseSummary` | **Tur bitti sinyali** |
| `agent:status-changed` · `agent:updated` · `agent:queue:updated` | 472 / 364 / 79 | — | Yardımcı |

### 3.3 `event.query` gerçek kullanımı (log'dan)

```sql
SELECT id, workspace_id, timestamp, event_type, actor, session_id,
       correlation_id, parent_event_id, metadata_json, data_json
FROM event WHERE 1=1 AND workspace_id = ?
ORDER BY timestamp DESC, id DESC LIMIT ? OFFSET ?
```

→ **workspace bazlı, en yeniden eskiye, sayfalı.** Yani olay filtresi istemci
tarafında yapılır (`1=1` dinamik filtrelere açık görünüyor ama kullanılmamış).
Cursor olarak `timestamp` (veya en son görülen `id`) kullanılabilir.

### 3.4 `agent_session` tablosunda hazır durum alanları

`status` · `is_active` · `stop_reason` · `last_message_role` ·
`last_assistant_preview` · `completion_report` · `completion_report_timestamp` ·
`message_count` · `assistant_message_count` · `last_turn_model`

→ `agent.get` muhtemelen bunları döndürüyor; köprü zaten `agent.get`'i
kullanıyor (handshake'te `result.agent.lastUserMessage` okunuyor).

### 3.5 `agent_message` tablosu

`agent_id`, **`seq` (agent başına monoton)**, `role`, `content` (JSON block),
`created_at` + `agent_message_fts`.

→ Mesaj okumak için **monoton `seq` mükemmel bir cursor**. (Log'da
`ORDER BY seq ASC LIMIT ? OFFSET ?` sorgusu görülüyor.)

### 3.6 Performans uyarısı (intentd kendi log'unda)

```
WARN rpc_dispatch{method="event.query"}: slow statement elapsed=1.16s rows=101
WARN … agent_message … elapsed=1.29s / 1.39s
WARN rpc_profile: rpc dispatch exceeded duration budget method=event.query elapsed_ms=1174
```

→ `event.query` saniyede bir çağrılacak bir metot **değil**. Tasarım buna göre
kurulmalı (§5).

---

## 4. Tasarım seçenekleri

### A) Cursor'lı event akışı (önerilen)

```
session/prompt(prompt, sessionId)
  ├─ agent.list  → hedef agent (bugünkü gibi)
  ├─ event.query(workspaceId, limit=1)  → cursor = en yeni timestamp
  ├─ agent.sendMessage(content)          → turnId   (zaten var)
  ├─ session/update: "[Intent] görev iletildi (turn …)"      ← ilerleme, SONUÇ DEĞİL
  └─ döngü (2 sn aralık, bütçe: prompt_timeout_secs):
       event.query(workspaceId, limit=50)
         ├─ agent:stream:status (agentId bizim, ts > cursor)
         │     → session/update: phase/message        ← gerçek ilerleme
         └─ agent:stream:end | agent:idle (agentId bizim, ts > cursor)
               → result.text = lastAgentResponse       ← NİHAİ SONUÇ
               → session/update: status=completed
               → dön (session/prompt yanıtı gerçek metni taşır)
bütçe aşılırsa → session/update: status=error  +  JSON-RPC error (sessiz done YOK)
```

**Avantaj:** gerçek ilerleme + tek doğru kaynaktan nihai metin; yeni RPC
varsayımı yok (`event.query` doğrulandı).

**Dikkat:** en yeniden eskiye + `OFFSET` sayfalama → poll'lar arasında `limit`'ten
fazla olay birikirse atlama olur. Önlem: `limit=100`, görülen `id`'leri bir
kümede tut, `cursor`u ilerlet.

### B) `agent.get` durum poll'u (daha ucuz, daha az bilgi)

Tur bitişini `agent.get` → `status == "idle"` (veya `stop_reason` dolu) ile
anla; nihai metni `last_assistant_preview` / `completion_report`'tan al.

**Avantaj:** çok daha hafif (§3.6).
**Dezavantaj:** canlı ilerleme yok (kullanıcı uzun turlarda sessizlik görür);
`agent.get`'in alan adları doğrulanmadı.

### C) Outbox — sonucu Spacebot API'sine geri push etme

Köprü hemen döner; Intent turu bitince sonucu Spacebot'un HTTP API'sine
(`127.0.0.1:19898`) yazar.

**Dezavantaj:** "gönderen" tarafın sonradan tetiklenmesi gerekir (ek gözcü süreç).
ACPP oturumu zaten açık kalabildiği için gereksiz karmaşıklık. **Önerilmiyor.**

### Karar

**A + B birlikte:** bitiş tespiti için önce `agent.get` (ucuz), olay akışı yalnızca
ilerleme ve nihai metin için ve daha seyrek (2-3 sn). Tek başına A'nın en yeniden
eskiye sayfalama riskini B'nin ucuz durum kontrolü kapatır.

---

## 5. Uygulama iskeleti (köprü, Python)

Mevcut `handle_session_prompt` değişir; gerisi (initialize/new/exit) aynı kalır.

```python
def handle_session_prompt(req_id, params):
    session_id = params.get("sessionId", str(uuid.uuid4()))
    prompt_text = extract_text(params.get("prompt", ""))

    agent_id, agent_name = find_target_agent(TARGET_WORKSPACE)
    if not agent_id:
        return error(req_id, f"no agent in workspace '{TARGET_WORKSPACE}'")

    cursor = newest_event_timestamp(TARGET_WORKSPACE)      # event.query(limit=1)
    ok, info, turn_id = send_task_to_intent(prompt_text, session_id, agent_id)
    if not ok:
        return error(req_id, info)

    update(session_id, "running", f"[Intent] görev iletildi → {agent_name} (turn {turn_id})")

    deadline = time.monotonic() + RESULT_BUDGET_SECS        # ör. 900
    seen = set()
    while time.monotonic() < deadline:
        time.sleep(POLL_SECS)                              # 2-3 sn

        # (B) ucuz bitiş kontrolü
        status = intentd_call("agent.get", {"agentId": agent_id}) \
                     .get("result", {}).get("agent", {})
        if status.get("status") == "idle" and status.get("lastMessageRole") == "assistant":
            text = status.get("lastAssistantPreview") or status.get("completionReport")
            if text:
                update(session_id, "completed", text)
                return {"jsonrpc": "2.0", "id": req_id, "result": {"text": text}}

        # (A) ilerleme + kesin nihai metin
        for event in events_since(TARGET_WORKSPACE, cursor, limit=100):
            if event["id"] in seen:
                continue
            seen.add(event["id"])
            data = json.loads(event["data_json"])
            if data.get("agentId") != agent_id:
                continue
            if event["event_type"] == "agent:stream:status":
                update(session_id, "running", f"[Intent] {data.get('message','')}")
            elif event["event_type"] in ("agent:stream:end", "agent:last-message"):
                text = data.get("lastAgentResponse", "")
                if text:
                    update(session_id, "completed", text)
                    return {"jsonrpc": "2.0", "id": req_id, "result": {"text": text}}
            cursor = max(cursor, event["timestamp"])

    return error(req_id, f"Intent turu {RESULT_BUDGET_SECS} sn içinde tamamlanmadı")
```

**Kritik:** bütçe sonunda dönen şey **hata** olmalı, `"completed"` değil. Bugünkü
sessiz başarısızlığın tekrar etmemesi buraya bağlı.

### Parametreler (config'ten okunabilir)

| Ad | Varsayılan | Gerekçe |
|---|---|---|
| `INTENT_RESULT_BUDGET_SECS` | 900 | İş bitmezse de tur kapanmalı; `prompt_timeout_secs = 600`'ün altında **kalmamalı** yoksa Spacebot tarafı önce zaman aşımına uğrar |
| `INTENT_POLL_SECS` | 2 | §3.6: `event.query` pahalı |
| `INTENT_EVENT_LIMIT` | 100 | Atlama riski vs yük dengesi |

---

## 6. Kenar durumlar

| Durum | Davranış |
|---|---|
| Intent düşerse (socket yok) | `session/prompt` **hata** döner; worker `failed` olur (bugün "done" diyor) |
| Tur hiç bitmezse | Bütçe sonunda hata; `stop_reason` ve son `agent:stream:status` mesajı hataya eklenir |
| Aynı anda iki görev | `agentId` + `turnId` ile ayrıştırılır; `agentId` filtresi zorunlu |
| Intent `failed` durumuna geçerse | `agent:updated`/`status-changed` izlenir → hata olarak dön |
| Uzun tur (saatler) | `session/update` periyodik heartbeat ile oturum canlı tutulur; ACP oturumunun saatlerce açık kalması **mevcut ve beklenen** davranış |
| Nihai metin boş | `lastAgentResponse` boşsa `agent:idle.lastResponseSummary`'ye düş |

---

## 7. Doğrulanması gerekenler (dürüstlük notu)

Bu dokümandaki **tablo/şema/olay adları doğrulandı** (intentd.db + log).
Doğrulanmayanlar:

1. **`agent.get` yanıtının alan adları.** Şemada `status`, `last_message_role`,
   `last_assistant_preview` var; JSON'da camelCase bekleniyor
   (`lastAssistantPreview`) çünkü mevcut köprü `lastUserMessage` okuyor. →
   Çalıştırıp bakılmalı.
2. **`event.query`'nin ek filtre parametreleri** (`1=1 AND workspace_id = ?`
   dinamik filtreye açık görünüyor). Filtre varsa poll maliyeti ciddi düşer.
3. **`agent:stream:end` güvenilirliği** — 266 olaya karşı 222 `agent:idle`;
   ikisi de bitiş sinyali olarak kullanılabilir, hangisinin daha güvenilir
   olduğu gözlemle netleşir.

**Doğrulama planı:** köprüyü `--test` benzeri tek seferlik bir modla çalıştır →
tek bir küçük prompt gönder (`"2+2 kaç eder?"`) → dönen `result.text` gerçek cevabı
içeriyor mu; ayrıca Spacebot `worker_runs.result` alanı aynı metni taşıyor mu.

---

## 8. Riskler

| Risk | Önlem |
|---|---|
| `event.query` saniyede bir çağrılırsa intentd yavaşlar (§3.6) | 2-3 sn aralık + önce `agent.get` |
| Yeni köprü eski davranıştan yavaş görünür (sonuç artık bekleniyor) | Doğru davranış budur: eskisi sonuç vermiyordu. Bütçe ve heartbeat UX'i korur |
| Köprü hatası tüm ACP worker'larını kırar | `command = "omp"`'a dönüş her an mümkün (yedeği ve config geçmişi elimizde) |
| intentd protokolü değişir (0.9.38 / protokol 9.11) | Alan adları §7'de listelendi; kırılırsa hata açık görünür (sessiz değil) |

---

## 9. Sıradaki adım

1. `agent.get` ve `event.query` yanıt şekillerini **canlı** doğrula (tek komut,
   `--test` modu).
2. Köprüyü §5 iskeletiyle güncelle; eski sürümü `spacebot-acp-worker.bak-<tarih>`
   olarak sakla.
3. Tek bir küçük görevle uçtan uca dene: Telegram → spacebot → ACP → Intent →
   sonuç geri. `worker_runs.result` gerçek metni içeriyor mu?
4. Başarılıysa `docs/design-docs/fork-upstream-plan.md`'ye kalem olarak işle ve
   commit et.

---

## 10. Uygulama durumu (2026-09-14)

**v2 köprü yazıldı ve test edildi** — `scripts/acp/spacebot-acp-worker`
(kurulum: `~/.local/bin/spacebot-acp-worker`, önceki sürüm
`spacebot-acp-worker.v1-20260914` olarak saklandı).

| Tasarım kararı | Uygulama |
|---|---|
| §4 A+B (ucuz `agent.get` bitiş kontrolü + seyrek `event.query` olay akışı) | ✅ 2 sn poll, `agent.get` önce |
| Cursor: routing öncesi en yeni `timestamp` | ✅ Turun kendi olayları dışını görmez |
| En-yeniden-eskiye sayfalama riski (§4 dikkat) | ✅ `id` görüldü mü kümesi (`seen_ids`) + `cursor` ilerletme |
| Nabız (satır başına 600 sn parent timeout'a karşı) | ✅ 60 sn'de bir non-text `intent_heartbeat` |
| Sonuç kirletmeme (parent tüm text content'leri biriktiriyor) | ✅ Ara update'ler `intent_progress`/`intent_heartbeat`; **tek** `completed` update `text` taşır |
| Bütçe sonunda hata, sessiz "completed" yok | ✅ `INTENT_RESULT_BUDGET_SECS=900` > 600; aşım → JSON-RPC error |
| Socket yok / sendMessage reddi / `failed` durumu | ✅ Hepsi açık hata |

**Test:** `scripts/acp/test_spacebot_acp_worker.py` — sahte intentd (gerçek UNIX
socket) üzerinde 8 senaryo; **8/8 yeşil**. Red adımı doğrulandı: 6 senaryo eski
v1 script'e karşı gerçekten başarısız oldu (yönlendirme onayı sonuç gibi dönüyordu).

**Henüz yapılmadı (§9'un canlı adımları):** intentd kapalı olduğu için gerçek
daemon'a karşı `--test` doğrulaması ve `worker_runs.result` uçtan uca kontrolü
— J0'ın ilk çalıştırma adımında yapılacak.
