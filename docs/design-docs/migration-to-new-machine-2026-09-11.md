# Yeni (daha güçlü) makineye taşıma — adım adım

Tarih: 11 Eylül 2026 · Kaynak makine: `coruho@arch` (15 GiB RAM, swap'e itiliyor)

Bu doküman taşımayı **kayıpsız** yapmak için gereken her şeyi içerir. Sayılar bu
makinede **ölçüldü**, tahmin değil.

---

## 0. Özet (TL;DR)

Taşınacak gerçek veri **341 MB**. Kod zaten GitHub'da. Ağır olan her şey
(`target/` 45 GB, `~/.omp` 35 GB, `~/.spacebot/night` 2,6 GB) **taşınmaz**.

```bash
# ESKİ makinede:
tar czf ~/spacebot-state.tar.gz -C "$HOME" \
  .spacebot/config.toml .spacebot/agents .spacebot/data .spacebot/backups \
  .config/systemd/user/spacebot.service \
  .local/bin/spacebot-acp-worker .local/bin/spacebot-bridge
# → ~341 MB

# YENİ makinede:
git clone https://github.com/coruhoorhan/spacebot
tar xzf spacebot-state.tar.gz -C "$HOME"
# ... sonra §6 (yollar), §9 (build), §10 (ilk çalıştırma)
```

**Tahmini süre:** ~1 saat (çoğu `cargo build --release`).

---

## 1. Ne taşınır, ne taşınmaz

| Ne | Boyut | Karar | Neden |
|---|---|---|---|
| Git repo (`~/Music/spacebot`) | — | **clone** | 52 commit GitHub'da (`4f39886d`) |
| `~/.spacebot/config.toml` | 2 KB | **TAŞI** | Routing, Telegram, insanlar, ACP |
| `~/.spacebot/agents/` | **251 MB** | **TAŞI** | **Asıl hafıza**: `agent.db` (32 M), `lancedb/` (159 M), `config.redb`, `settings.redb` |
| `~/.spacebot/data/` | 2 MB | **TAŞI** | `secrets.redb`, `spacebot.db` |
| `~/.spacebot/backups/` | 88 MB | **TAŞI** | `pre-stabilization-20260911` geri dönüş noktası |
| `~/.config/systemd/user/spacebot.service` | 293 B | **TAŞI** | Servis tanımı |
| `~/.local/bin/spacebot-acp-worker` | 13 KB | **TAŞI** | **Intent köprüsü** (config.toml bunu çağırıyor) |
| `~/.local/bin/spacebot-bridge` | 16 KB | **TAŞI** | Köprünün ikinci parçası |
| `~/.local/share/intentd/` | 216 MB | **TAŞI**\* | Intent Daemon state (`intentd.db`, config) — köprü kullanılıyorsa |
| `~/.cache/dfbin/` | 88 MB | **isteğe bağlı** | onnxruntime `.so` — yoksa otomatik iner |
| `~/.spacebot/embedding_cache/` | 128 MB | **isteğe bağlı** | Silinirse ilk açılış yavaş olur |
| `~/.spacebot/logs/` | 84 MB | **isteğe bağlı** | Teşhis geçmişi |
| `~/.omp/` | **35 GB** | **isteğe bağlı** | omp oturum geçmişi (`omp -r` için). Spacebot'u çalıştırmak için gerekmez |
| `~/.spacebot/night/` | 2,6 GB | **TAŞIMA** | Eski binary yedekleri (`night/`) — kaynaktan yeniden derlenir |
| `~/Music/spacebot/target/` | **45 GB** | **TAŞIMA** | Makineye özel derleme çıktısı |
| `interface/node_modules/` | — | **TAŞIMA** | `bun install` ile kurulur |
| `interface/dist/` | — | **TAŞIMA**\*\* | gitignore'da → yeniden derlenmeli (§12.3) |

\* Köprüyü kullanmıyorsan atla. \*\* Dashboard'u istiyorsan gerekli.

---

## 2. Ön koşullar (yeni makinede)

| Gereksinim | Neden | Kontrol |
|---|---|---|
| **Rust ≥ 1.97** | `edition = "2024"` | `rustc --version` |
| **lld** + gcc | `.cargo/config.toml` linker'ı `-fuse-ld=lld` | `which ld.lld` |
| **chromium** | Browser tool | `which chromium` |
| **sqlite3** | Teşhis sorguları | `which sqlite3` |
| **bun** | `interface/dist` derlemesi | `which bun` |
| Linux **kernel keyring** (`keyctl`) | Secret kilitleme/izolasyon | `which keyctl` |

> **Not (lld):** Yeni makinede `lld` yoksa `cargo build` linker hatası verir.
> Çözüm: `lld` kur, **veya** repodaki `.cargo/config.toml`'dan şu 3 satırı sil:
> ```toml
> [target.x86_64-unknown-linux-gnu]
> linker = "gcc"
> rustflags = ["-C", "link-arg=-fuse-ld=lld"]
> ```

Kısa kontrol:
```bash
rustc --version && which ld.lld chromium sqlite3 bun keyctl
```

---

## 3. Eski makinede: paketle

**Önce daemon durdurulmalı** — SQLite (`agent.db`) ve redb dosyaları açıkken
kopyalamak bozuk bir yedek üretir.

```bash
# 1) Daemon'ı durdur (kapalıysa "not running" der, sorun değil)
cd ~/Music/spacebot && ./target/release/spacebot stop

# 2) Çalışan süreç kalmadığını doğrula
pgrep -af "spacebot" | grep -v bash || echo "temiz ✅"

# 3) Git'in temiz olduğunu doğrula (hiçbir iş commit'siz kalmasın)
git status --short
# Çıktı sadece "?? .freebuff/" olmalı. Başka bir şey varsa COMMIT ET.

# 4) Paketle — ~341 MB
tar czf ~/spacebot-state.tar.gz -C "$HOME" \
  .spacebot/config.toml .spacebot/agents .spacebot/data .spacebot/backups \
  .config/systemd/user/spacebot.service \
  .local/bin/spacebot-acp-worker .local/bin/spacebot-bridge

# 5) Köprüyü kullanıyorsan intentd state'i de ekle
tar rf ~/spacebot-state.tar.gz -C "$HOME" .local/share/intentd

# 6) Doğrula
ls -lh ~/spacebot-state.tar.gz
tar tzf ~/spacebot-state.tar.gz | head -20
```

**İsteğe bağlı küçültme:** `agents/main/data/` içindeki iki eski `agent.db`
kopyası (27 MB + 29 MB) taşınmasa da olur — bunlar `agent.db.before-*` ve
`agent.db.bak-*` dosyaları, sadece geçmiş yedekler.

> **`night/` klasörünü pakete koymadım** (2,6 GB eski binary) — kaynaktan
> yeniden derlenir. Bilinçli bir arşivse ayrıca kopyala.

---

## 4. Yeni makinede: klonla

```bash
mkdir -p ~/Music && cd ~/Music

git clone https://github.com/coruhoorhan/spacebot

cd spacebot

# 52 commit geldi mi? (bugünkü 5 dahil)
git log --oneline -6

# Kilit nokta: upstream SADECE okunur olmalı
git remote set-url --push upstream DISABLED
git remote -v
# origin  ... (fetch/push)   ← kendi fork'un, push edilecek yer
# upstream ... (fetch)  DISABLED (push)   ← asla buraya push etme
```

Klonda `.cargo/config.toml`, `Cargo.toml` (lto=false), `src/` düzeltmeleri ve
`docs/design-docs/` içindeki tüm teşhis/plan dokümanları gelir.

---

## 5. State'i yerleştir

```bash
cd "$HOME"
tar xzf ~/spacebot-state.tar.gz

# İzinleri sıkılaştır (secret içerenler)
chmod 600 ~/.spacebot/config.toml ~/.spacebot/data/secrets.redb 2>/dev/null
chmod 700 ~/.spacebot/data ~/.spacebot/agents 2>/dev/null

# Köprü script'leri çalıştırılabilir olmalı
chmod +x ~/.local/bin/spacebot-acp-worker ~/.local/bin/spacebot-bridge 2>/dev/null

# Doğrula
du -sh ~/.spacebot/agents ~/.spacebot/data
ls -la ~/.spacebot/data/secrets.redb ~/.local/bin/spacebot-acp-worker
```

---

## 6. Mutlak yolları düzelt ⚠️ **EN KRİTİK ADIM**

Taşımada kırılan **tam olarak iki yer** var. İkisi de `coruho` ve
`/home/coruho/...` yolunu sabit kodluyor.

### 6.1 `~/.spacebot/config.toml` — satır 89

```toml
[defaults.acp]
enabled = true
command = "/home/coruho/.local/bin/spacebot-acp-worker"   # ← YENİ kullanıcı yolu
args = []
prompt_timeout_secs = 600
permissions = "auto_accept"
```

Yeni makinede:
```bash
NEWUSER=$(whoami)
sed -i "s|/home/coruho/|/home/$NEWUSER/|g" ~/.spacebot/config.toml

# Doğrula — başka mutlak yol kalmadı mı?
grep -n "/home/" ~/.spacebot/config.toml
```

Bu adımı atlarsan **ACP worker'lar hiç başlamaz** ve Spacebot'un iş
yaptırma yolu tamamen sessizce ölür.

### 6.2 `~/.config/systemd/user/spacebot.service`

```bash
NEWUSER=$(whoami)
REPO="$HOME/Music/spacebot"
sed -i "s|/home/coruho|/home/$NEWUSER|g" ~/.config/systemd/user/spacebot.service
cat ~/.config/systemd/user/spacebot.service
```

Sonuç şöyle olmalı (yol yeni makineye göre):
```ini
[Service]
Type=simple
WorkingDirectory=/home/<yeni-kullanıcı>/Music/spacebot
ExecStart=/home/<yeni-kullanıcı>/Music/spacebot/target/release/spacebot start -f
Restart=always
RestartSec=5
```

---

## 7. Model routing'i bağla

Yeni sağlayıcı/model bilgisi geldiğinde `~/.spacebot/config.toml` içindeki
`[llm.provider.*]` ve `[defaults.routing]` bölümleri güncellenir. Mevcut
durum (referans):

```toml
[llm.provider.experiential]      base_url = "https://api.experientiallabs.ai"
[llm.provider.cheaperinference]  base_url = "https://api.cheaperinference.com"

[defaults.routing]
channel   = "cheaperinference/deepseek-v4-flash"
branch    = "cheaperinference/deepseek-v4-flash"
worker    = "cheaperinference/deepseek-v4-flash-0731"
compactor = "cheaperinference/deepseek-v4-flash-0731"
cortex    = "cheaperinference/deepseek-v4-flash-0731"

[defaults.routing.fallbacks]
"experiential/deepseek-v4-flash-0731" = ["cheaperinference/claude-haiku-4.5", "cheaperinference/deepseek-v4-flash"]
"cheaperinference/deepseek-v4-flash"  = ["cheaperinference/glm-5.3-flash"]
```

**Not:** Bu makinedeki modellerin çalışmadığı söylendi — yeni `base_url` +
`api_key` + model id'leri gelince bu bölüm değişecek. **Sıra önemli:**
önce §10 ile mevcut hâlin çalıştığını gör, sonra routing'i değiştir. Aksi
hâlde "model kötü" ile "kurulum bozuk" ayırt edilemez.

---

## 8. Doğrulama (derlemeden önce, ucuz)

```bash
cd ~/Music/spacebot

cargo fmt -- --check          # temiz olmalı
cargo check --all-targets     # ~5-6 dk (ilk seferde bağımlılıklar)
cargo test --lib              # 1405 test, ~12 sn (derleme sonrası)
```

Beklenen: **`1405 passed; 0 failed`**. (11 Eyl'de bu makinede tam olarak bu
sonuç alındı.)

---

## 9. Release binary derle

```bash
cd ~/Music/spacebot

# 1) Mevcut binary'yi GERİ DÖNÜŞ için sakla (varsa)
cp target/release/spacebot ~/spacebot-binary-backup-$(date +%Y%m%d) 2>/dev/null

# 2) Derle
cargo build --release
```

**Süre:** güçlü makinede ~10-20 dk (bu makinede 40+ dk sürerdi).
`lto = false` olduğu için eski hâlinden belirgin şekilde hızlı.

> **Not:** Bu derleme **zorunlu** — 11 Eylül'de commit edilen 5 düzeltme
> (streaming fallback, 5 dk timeout, retriable gateway hataları, tool-arg
> temizliği, eşzamanlılık sınırı, SQLite ayarları) **ancak yeni binary ile
> yürürlüğe girer.** Eski binary (5 Eylül) bunların hiçbirini içermiyor.

**İsteğe bağlı — `target/` bir daha 45 GB olmasın:**
```toml
# Cargo.toml
[profile.dev]
debug = 1                    # satır bilgisi yeter (2,5 GB binary → ~400 MB)

[profile.dev.package."*"]
debug = false                # bağımlılıkları debug'lamıyoruz
```
Maliyeti: bir kez tüm bağımlılıkların yeniden derlenmesi.

---

## 10. İlk çalıştırma + izleme

```bash
cd ~/Music/spacebot

# Servisi yükle
systemctl --user daemon-reload
systemctl --user enable --now spacebot
systemctl --user status spacebot

# Canlı log
journalctl --user -u spacebot -f
```

**İlk 30 dakika boyunca şunlara bak** (bu, düzeltmelerin gerçekten
çalıştığının kanıtı olacak):

| Gözlenen | Anlamı |
|---|---|
| `streaming fallback model succeeded` | ✅ Düzeltme çalışıyor — eski kodda bu satır hiç olamazdı |
| `model stream failed (non-retriable) … model=<id>` | Model adı görünüyor → hangi modelin patladığı belli |
| `stripped leaked model control tokens from streamed tool arguments` | DSML temizliği devrede |
| `retriable streaming error` ardından başka model | Yedek zinciri gerçekten devrede |
| Hâlâ `1800s`'de ölen worker | ⚠️ Timeout değişmemiş → yanlış binary çalışıyor |

**Eski hatalı davranış (artık görülmemeli):** worker'ın tam `1800 saniyede`
ölmesi, `The request queue is full.`, shell'de `syntax error near unexpected token`.

Token/süre metriklerini karşılaştırmak için (teşhis dokümanındaki sorgular):
```bash
sqlite3 ~/.spacebot/agents/main/data/agent.db \
  "select status, count(*), round(avg(duration_secs)) from worker_runs group by status;"
```

---

## 11. Geri dönüş (rollback)

Her denemeden önce bir dönüş noktası kalsın:

```bash
# Binary dönüşü
systemctl --user stop spacebot
cp ~/spacebot-binary-backup-<tarih> ~/Music/spacebot/target/release/spacebot
systemctl --user start spacebot

# State dönüşü (bu makinedeki hazır yedek)
ls ~/.spacebot/backups/pre-stabilization-20260911/
# agent.db, config.toml, secrets.redb, uncommitted-20260911.patch
```

> `uncommitted-20260911.patch` artık gereksiz — o iş 11 Eyl'de commit edildi
> (`211be8ca`, `1fa53127`, `acdfeed0`, `46e54f29`, `4f39886d`). Yedek olarak
> dursun.

---

## 12. Bilinen tuzaklar (bunları bilmezsen gün kaybedersin)

### 12.1 `secrets.redb` bugün **şifresiz** — ama şifrelersen taşınamaz

Kontrol edildi: dosyada API anahtarı **düz metin** duruyor. Yani dosyayı
kopyalamak yeterli, hiçbir anahtar gerekmiyor.

**Ama** bir gün `spacebot secrets encrypt` çalıştırırsan: master key
**Linux kernel session keyring'inde** (`KEY_SPEC_SESSION_KEYRING`) tutulur —
yani **kernel belleğinde**, ne dosyada ne env'de. Sonuçları:

- Yeniden başlatmada gider (kalıcı değildir)
- **Yeni makineye asla taşınamaz** → `secrets.redb` açılamaz
- Tasarım gereği böyle: worker alt süreçleri anahtarı göremesin diye

**Şifreli kullanacaksan taşıma kuralı:**
```bash
# ESKİ makinede, daemon ÇALIŞIRKEN portable yedek al:
cd ~/Music/spacebot
./target/release/spacebot start
./target/release/spacebot secrets export -o ~/spacebot-secrets.json
./target/release/spacebot stop

# YENİ makinede:
./target/release/spacebot start
./target/release/spacebot secrets import -i ~/spacebot-secrets.json --overwrite
```
Alternatif: anahtarı `/run/spacebot/master_key` veya `/run/secrets/master_key`
yoluna 64 karakterlik hex olarak koy (kod bu yolu okuyor).

`secrets export` **daemon çalışırken** çalışır (yerel API üzerinden).

### 12.2 Servis şu an **`disabled`** — yeniden başlatmada kendiliğinden açılmaz

Bu makinede `systemctl --user is-enabled spacebot` → **`disabled`**. Yani unit
dosyası "auto-start" dese de servis boot'ta başlamıyor. Yeni makinede bilinçli
karar ver:
```bash
systemctl --user enable spacebot     # boot'ta otomatik başlasın
loginctl enable-linger "$USER"       # oturum kapansa da çalışsın (bu makinede: Linger=yes)
```

### 12.3 `interface/dist/` gitignore'da → dashboard kaybolur

Daemon frontend'i **gömülü** olarak sunar (`#[folder = "interface/dist/"]`).
Bu klasör repoya girmez, yani klonda yoktur:
```bash
cd ~/Music/spacebot/interface
bun install
bun run build
```
`bun` yoksa `~/Music/spacebot/interface/node_modules` de klonda gelmez.

Dashboard olmadan da daemon çalışır; sadece UI eksik olur.

### 12.4 `~/.cache/dfbin` — onnxruntime

Embedding (fastembed) için `.so` dosyaları burada (88 MB). Yoksa ilk açılışta
otomatik iner. Makine çevrimdışıysa kopyala:
```bash
tar czf ~/dfbin.tar.gz -C ~/.cache dfbin
```

### 12.5 `~/.omp` 35 GB — taşıma, sadece bil

`omp` oturum geçmişi. Spacebot'un çalışması için gerekmez; ACP worker'ın
`omp -r <id>` ile işe devam edebilmesi için gerekir. Taşımaya karar verirsen
35 GB'ı ağ üzerinden kopyalamak saatler alır — önce eski oturumları buda.

### 12.6 Portlar ve API

- Daemon API + UI: `http://127.0.0.1:19898`
- CLI aynı makinede konuşur: `spacebot status`, `spacebot secrets status`, …
- Başka makineden: `--url` + `--token` (`api.auth_token`)

---

## 13. Taşıma kontrol listesi

**Eski makine**
- [ ] `spacebot stop` — çalışan süreç yok
- [ ] `git status --short` temiz (sadece `?? .freebuff/`)
- [ ] `git push origin main` — yerel = uzak (`4f39886d`)
- [ ] `~/spacebot-state.tar.gz` oluştu (~341 MB) ve `tar tzf` ile doğrulandı
- [ ] (şifreliyse) `secrets export` alındı

**Yeni makine**
- [ ] Rust ≥ 1.97, `lld`, `chromium`, `sqlite3`, `bun` kurulu
- [ ] `git clone` → `git log` 52 commit gösteriyor
- [ ] `git remote set-url --push upstream DISABLED`
- [ ] State açıldı, izinler sıkılaştırıldı
- [ ] **§6.1** `config.toml` mutlak yol düzeltildi
- [ ] **§6.2** systemd unit yolları düzeltildi
- [ ] `cargo fmt --check` temiz
- [ ] `cargo test --lib` → **1405 passed; 0 failed**
- [ ] `cargo build --release` tamamlandı
- [ ] `interface/dist` derlendi (dashboard isteniyorsa)
- [ ] `systemctl --user enable --now spacebot`
- [ ] İlk 30 dk log izlendi (§10 tablosu)
- [ ] `enable-linger` kararı verildi

**Sonra**
- [ ] Yeni model bilgisi geldi → routing güncellendi (§7)
- [ ] Yedek: `~/.spacebot/backups/pre-stabilization-*` yeni makinede de duruyor

---

## Ek: hızlı komut kartı

```bash
# Durum
systemctl --user status spacebot
journalctl --user -u spacebot -f
sqlite3 ~/.spacebot/agents/main/data/agent.db "select status,count(*),round(avg(duration_secs)) from worker_runs group by status;"

# Yönetim
systemctl --user restart spacebot      # yeniden başlat
systemctl --user stop spacebot         # durdur
./target/release/spacebot status       # CLI'dan durum

# Teşhis
du -sh target/ ~/.spacebot/ ~/.omp/    # disk
tail -f ~/.spacebot/logs/spacebot.log  # uygulama logu
spacebot usage                        # token kullanımı
spacebot provider list                # sağlayıcılar
spacebot secrets status               # secret deposu
```

---

## 14. AÇIK SORU (pazartesi): temiz kurulum + hafıza nakli?

> "Kod zaten GitHub'da — neden buradan migration yapıyoruz ki?"

Haklı bir itiraz. Ama **kod** ile **hafıza** ayrı şeyler:

| Katman | Temiz kurulum uygun mu? |
|---|---|
| **Kod** | ✅ `git clone` — bu zaten migration değil |
| **config.toml** | ✅ Yeniden üretilebilir (2 KB, ~100 satır) — routing/insanlar/otonomi port edilir |
| **Hafıza** | ❌ **Yeniden üretilemez** |

### Neden hafıza taşınmak zorunda

`agent.db` sağlıklı (`integrity_check = ok`, `foreign_key_check` boş) ve içinde:

```
memories                        503   (fact 224, observation 100, event 91,
                                       decision 34, todo 24, preference 16,
                                       goal 8, human 5, identity 1)
associations                   2715   (related_to 2519, updates 148, part_of 39,
                                       result_of 4, contradicts 4, caused_by 1)
conversation_messages          1500
working_memory_events          1531
worker_runs                     534
wake_events                     304
channels / human_identities  12 / 5
cron_jobs / wake_defs         1 / 2
birikim aralığı      3 Eyl → 10 Eyl 2026
```

Vektörler: `agents/main/data/lancedb/` (159 MB) —
`memory_embeddings.lance` + `chronicle_embeddings.lance`.

**Kritik:** `spacebot memory` CLI'ında sadece `list` ve `search` var —
**export/import YOK.** Yani hafıza grafiği (503 düğüm + 2715 kenar) dışa
aktarılamaz; sadece **dosya olarak** taşınır. Temiz kurulum = **amnezi**.

### Pazartesi vereceğimiz karar

- **A) Temiz kurulum + hafıza nakli (öneri):** taze `~/.spacebot`, yeni config
  (sadece routing + insanlar + ACP port edilir), fakat **`agent.db` +
  `lancedb/` + `config.redb` + `settings.redb` + `secrets.redb`** taşınır.
  → **~195 MB.** Birikmiş config çöpü (`config.toml.bak-*` ×6) ve dünkü DB
  denemesinin izleri geride kalır.
- **B) §3'teki tam taşıma:** 341 MB, `backups/` + `logs/` de gelir. Daha
  muhafazakâr, ama eski çöpü de getirir.

**Karar için gereken bilgi:** hafızayı korumak istiyor muyuz (503 anı +
2715 ilişki), yoksa sıfırdan mı başlasın? `memory` CLI'da export olmadığı için
"sonra aktarırız" diye bir seçenek **yok**.

---

## İlgili dokümanlar

- `worker-perf-stability-diagnosis-2026-09-11.md` — altı kök neden, ölçümler
- `stabilization-plan-2026-09-11.md` — sıralı düzeltme planı
- `acp-intent-bridge-result-path-2026-09-11.md` — Intent köprüsünün dönüş yolu
- `fork-upstream-plan.md` — upstream durumu ve port listesi
