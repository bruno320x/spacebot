# Jarvis Yol Haritası — Spacebot'u gerçek kişisel asistana taşıma (2026-09-14)

> Bu doküman, 2026-09-11 stabilizasyon haftasının sonunda belirlenen uzun vadeli
> hedefi planlar: Spacebot'u "çalışan bir agent framework'ü" olmaktan çıkarıp
> **hatırlayan, güvenilir ve otonom kişisel asistan** ("Jarvis") haline getirmek.
>
> Kısa vadeli operasyon planı `stabilization-plan-2026-09-11.md`'de,
> taşıma adımları `migration-to-new-machine-2026-09-11.md`'dedir. Bu doküman
> onların **üstündeki katman**dır: hangi sırayla ne inşa edeceğimizi söyler.

---

## 0. Hedef tanımı — "Jarvis" ne demek

| Yetenek | Bugün | Jarvis standardı |
|---|---|---|
| Hatırlama | 503 anı + 2715 ilişki var ama **active recall yok** | "3 ay önce şunu sorduğumda…" sorusuna güvenilir cevap |
| Güvenilirlik | Worker %26 hata, streaming'de fallback zinciri yok | Hata oranı < %10, her hata retriable, sessiz başarısızlık yok |
| Sonuç dönüşü | ACP worker "done" diyor ama sonuç kayboluyor | Her işin çıktısı `worker_runs.result`'ta okunabilir |
| Bütçe | Idle iken cortex kota yiyebiliyor (upstream #593) | Günlük token bütçesi + aşım alarmı |
| Otonomi | Plan/Goal/Vibe/Yolo modları var, resident channel yok | Sen yokken de iş yapıp bitirince haber veriyor |
| Erişim | Dashboard + TUI | Telegram + ses: "seslenince cevap veriyor" |

**Kural:** her fazın çıkış kriteri **ölçülebilir** olur; "bitti" demek için
test/ölçüm kanıtı gerekir (harness.md'nin "no silent failure" ilkesiyle uyumlu).

---

## 1. Mevcut durum (2026-09-14 ölçümü)

### 1.1 Çalışan ve güvenilir olanlar

| Bileşen | Durum | Kanıt |
|---|---|---|
| Supervisor (no-hang/no-orphan) | ✅ Çalışıyor | Faz 3, PID registry |
| Autonomy modları (Plan/Goal/Vibe/Yolo) | ✅ Çalışıyor | TaskMode persist + ceiling clamp |
| Evidence-gate + verify-wake | ✅ Çalışıyor | Faz 2.2/2.3 |
| MCP server + TUI + dashboard | ✅ Çalışıyor | Faz 1.2/1.3, 11-13 |
| SQLite havuz sınırları | ✅ Düzeltildi | `1fa53127`, 2 test |
| LLM dayanıklılığı (retry/timeout/sanitizer/eşzamanlılık) | ✅ Kod hazır | `211be8ca`, **1405 test geçti** |

### 1.2 Kırık veya eksik olanlar

| Boşluk | Etki | İlgili faz |
|---|---|---|
| ACP → Intent köprüsünde **sonuç dönüş yolu yok** | Worker 12-40 saat "done" görünür, iş sonucu kaybolur | **Faz J2** |
| Streaming'de **fallback zinciri yok** (kod içi yorumla doğrulandı) | Kanal + worker çağrıları yedek modele hiç geçemiyor | **Faz J1** |
| Cortex bakımı **bütçesiz** | Idle'da token yakabilir (upstream #593 kanıt vakası) | **Faz J3** |
| Çalışan binary bayat (5 Eyl) | Düzeltmeler canlı değil; release build taşımada yapılacak | **Faz J0** |
| `spacebot memory export/import` yok | Hafıza tek kopya halinde sadece dosya olarak taşınabilir | **Faz J4** |
| Active recall yok | Anılar var ama soru gelince geri çağrılmıyor | **Faz J4** |

### 1.3 Commit / issue / PR manzarası

| Depo | Durum |
|---|---|
| `coruhoorhan/spacebot` (fork) | **54 commit** fork noktasından beri, hepsi push'lu, **0 açık issue** |
| `spacedriveapp/spacebot` (upstream) | Son merge **17 Ağustos**, **68 açık PR** birikmiş → fork fiilen canlı dal |
| Upstream çakışan başlıklar | #593 (idle token), #652 (result relay), #637/#641 (chronicle), #578 (active recall), #604 (memory bloat), #650/#651 (autonomy channels) |

**Strateji:** upstream merge etmediği için ilginç PR'ları **elle port**
ediyoruz (`git remote set-url --push upstream DISABLED` ile upstream'e push
teknik olarak imkânsız — yanlışlıkla guard'ı 2026-09-11'de kuruldu).

---

## 2. Faz J0 — Çalışır temel (P0, bu hafta)

> Amaç: düzeltmelerin **canlıya alınması**. Bu faz bitmeden hiçbir ölçüm anlamlı değil.

| # | İş | Çıkış kriteri |
|---|---|---|
| J0.1 | Güçlü makineye taşıma (runbook hazır: `migration-to-new-machine-2026-09-11.md`) | Yeni makinede daemon açılıyor, mevcut hafıza erişilebilir |
| J0.2 | Yeni sağlayıcı bağlama (base_url, api_key, model listesi, reasoning desteği) | `spacebot doctor` veya smoke test ile 3 model yanıt veriyor |
| J0.3 | Routing: iş tipine göre model ataması + reasoning kapatma | Worker tur süresi ölçülüyor, reasoning çıktısı < %10 |
| J0.4 | `cargo build --release` + servis başlatma + 30 dk gözetimli ilk koşu | 30 dk'da 0 crash, worker hata oranı ilk ölçüm alınıyor |

**Temiz kurulum vs hafıza transplant kararı** bu fazda verilir (bkz. §4).

---

## 3. Faz J1 — Güvenilirlik (P1, gelecek hafta)

> Amaç: "çalışıyor ama sonucu yok" ve "tek model düşünce herşey ölüyor" problemlerini kapatmak.

| # | İş | Detay | Çıkış kriteri |
|---|---|---|---|
| J1.1 | **ACP ↔ Intent dönüş yolu** | Tasarım hazır: `acp-intent-bridge-result-path-2026-09-11.md` — `intentd.db`'de `event` + `event_subscription` tabloları doğrulandı | Worker tamamlanınca sonuç `worker_runs.result`'a yazılıyor; uçtan uca test var |
| J1.2 | **Streaming fallback zinciri** | `dispatch_stream`'in retry mantığı `dispatch_completion`'daki `attempt_with_retries` ile eşitlenir | Sanal sağlayıcı hatası testinde stream completion fallback'e geçiyor |
| J1.3 | **Cortex token bütçesi** | Bakım tick'ine günlük üst sınır + aşım logu (upstream #593 port'u) | Bütçe dolunca bakım erteleniyor, log'da görünür |
| J1.4 | Hata sınıflandırma tamamı | Kalan non-retriable 400 kalıpları tek tek gözden geçirilir | Tanımlı her hata retriable/non-retriable etiketli |

---

## 4. Hafıza kararı — temiz kurulum vs transplant

2026-09-11'de açık kalan soru. Ölçümler:

| Seçenek | Kazanç | Kayıp |
|---|---|---|
| **Transplant** (341 MB state kopyala) | 503 anı + 2715 ilişki + 1500 mesaj + 159 MB vektör korunur | Şema eski binary'ye göre; taşıma sonrası migration gerekebilir |
| **Temiz kurulum** (sıfırdan) | Şema güncel koda göre taze | **Amnezi** — "3 ay sonra hatırla" hedefinin verisi gider |

**Öneri (J0.1 sırasında test edilecek):**
1. Önce **temiz kurulum** ile sistemi ayağa kaldır (değişken azalt).
2. `agent.db`'yi ayrı kopyala, geri yükleme denemesi yap (`integrity_check = ok` doğrulandı).
3. Geri yükleme temizse transplant, bozukysa sadece anı tablolarını (memories + edges + vectors) seçici taşımak için **J4'teki export/import CLI'ını öne al**.

---

## 5. Faz J2 — Jarvis hafızası (P2)

> Amaç: anı **birikmekten** **kullanılabilirliğe** geçirmek.

| # | İş | Kaynak | Çıkış kriteri |
|---|---|---|---|
| J2.1 | **Active recall** — soru geldiğinde ilgili anıların otomatik çağrılması | upstream #578 port'u | Retriever testi: 10 soruluk altın sette >= 8 isabet |
| J2.2 | **Chronicle özetleme** — eski checkpoint'lar üst seviye özetlere katlanır | upstream #637 | 30 günden eski checkpoint sayısı ~0, özet yüzdesi ölçülür |
| J2.3 | **Memory merge bloat fix** — ingestion'da çoğalan anı birleştirme | upstream #604 | Merge sonrası anı sayısı monoton azalır (test ile) |
| J2.4 | **`spacebot memory export/import`** | Yeni | Export → import roundtrip testi; taşıma runbook'u buna bağlanır |
| J2.5 | Reflection kayıtları + aktivite zaman çizelgesi | upstream #641 | "Geçen hafta ne yaptım" sorusu chronicle'dan cevaplanıyor |

---

## 6. Faz J3 — Otonomi ve erişim (P3)

> Amaç: sen yokken de çalışan, seslenince cevap veren asistan.

| # | İş | Kaynak | Çıkış kriteri |
|---|---|---|---|
| J3.1 | Resident autonomy channels — uzun soluklu işlerin arka planda sürmesi | upstream #650/#651 fikri, kendi tasarımımız | Worker durduğunda kanal kendi kendine devam ediyor (2 saatlik test) |
| J3.2 | Telegram kanalı | Yeni adapter | Bot mesaj gönderip yanıt alıyor; kesintide mesaj kaybı yok |
| J3.3 | Ses girişi/çıkışı | Transkripsiyon endpoint'i | Sesli komut → görev → sesli yanıt döngüsü |
| J3.4 | Otonomi tavanları — Yolo modunda riskli işlem onayı | Mevcut ceiling clamp genişletilir | Tehlikeli komut listesi onay bekliyor, log'da izlenebilir |

---

## 7. Sıra ve bağımlılıklar

```
J0 (taşı + canlıya al)
 └─> J1 (güvenilirlik) ──> ölçüm: hata < %10, sessiz başarısızlık 0
      └─> J2 (hafıza) ──> ölçüm: recall isabeti, memory bloat durdu
           └─> J3 (otonomi + erişim) ──> Jarvis standardı tablosu tamam
```

- **J1.1** dönüş yolu, J2.5'in "ne yaptım" verisinin kaynağıdır → J1 bitmeden J2'ye geçmek anlamsız.
- **J2.4** export/import, J0'daki transplant kararı riskli çıkarsa **öne çekilebilir**.
- Her faz sonunda bu tablo güncellenir; tamamlanan satırlar kanıt commit'iyle işaretlenir.

---

## 8. Checklist (canlı takip)

- [ ] J0.1 Taşıma tamam, daemon yeni makinede açılıyor
- [ ] J0.2 Yeni sağlayıcı bağlı, 3 model smoke test'i yeşil
- [ ] J0.3 Routing tabanlı model ataması aktif
- [ ] J0.4 Release binary + 30 dk gözetimli ilk koşu temiz
- [ ] J1.1 ACP dönüş yolu uçtan uca test edildi
- [ ] J1.2 Streaming fallback zinciri
- [ ] J1.3 Cortex token bütçesi
- [ ] J2.1 Active recall
- [ ] J2.2 Chronicle özetleme
- [ ] J2.3 Memory bloat fix
- [ ] J2.4 memory export/import CLI
- [ ] J3.1 Resident autonomy channels
- [ ] J3.2 Telegram kanalı
- [ ] J3.3 Ses döngüsü
- [ ] J3.4 Otonomi tavanları
