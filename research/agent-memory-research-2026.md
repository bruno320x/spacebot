# Ajan Bellek Mimarileri Araştırması — 2026
Kaynak: GitHub API + resmi README'ler + arXiv API (tümü doğrulanmış, 2026-09-06 UTC)

## 1. Öne Çıkan Çerçeveler (doğrulanmış gerçekler)

| Sistem | Yıldız | Dil | Yaklaşım |
|---|---|---|---|
| **mem0** (mem0ai/mem0) | 64.7K | Python | LLM tabanlı ADD-only çıkarım (tek çağrı, UPDATE/DELETE yok), entity linking, multi-signal retrieval (semantik+BM25+entity füzyonu), temporal reasoning |
| **Cognee** (topoteretes/cognee) | 30.5K | Python | Self-hosted bilgi grafiği + vektör embedding; ontoloji kognitif bilim tabanlı; oturum belleği → kalıcı grafa senkron; `AUTO_FEEDBACK=false` ile LLM çağrısı kaldırma; recall()'da deterministik (LLM'siz) skill gate |
| **Graphiti** (getzep/graphiti) | 30.6K | Python | Temporal (bi-zamansal) bilgi grafiği; gerçekler zamanla nasıl değişiyor takibi, provenance, hybrid retrieval (semantik+keyword+graf), artımsal güncelleme (yeniden hesaplama yok). arXiv 2501.13956 |
| **Zep** | 4.9K | Python | Artık ticari "Zep Cloud" (OSS Community Edition deprecated). Alttaki graf motoru = Graphiti |
| **Letta** (f.k.a. MemGPT) | 24.6K | TS/Node | Stateful ajanlar; MemGPT "virtual context management" + self-editing memory (hafıza hiyerarşisi: ana/arşiv). Şimdi platform (letta-code, desktop, cloud) |
| **MemGPT** (cpacker) | — | — | LettA'ya taşındı (redirect: letta-ai/letta). OS benzeri sanal bağlam yönetimi. arXiv 2310.08560 |
| **OKF v0.2** (serradura/okf) | 153 | Ruby | Dosya tabanlı (Markdown+YAML), git-native, veritabanı yok. One-concept-one-file, index.md progressive-disclosure haritası, trust tier (verified), provenance, stale_after, MCP (14 araç), Apache-2.0 |
| **agentmemory** (rohitg00) | 28K | Node | BM25-first; keyless modda vektörler kapalı (BM25); hybrid BM25+vector+graph RRF füzyonu; token bütçesi 2000; opsiyonel local embedding (all-MiniLM-L6-v2); "92% daha az token" iddiası (LLM jsonsız recall) |

Not: "okf" Go sürümü (kullanıcının yapıştırdığı doküman) OKF v0.2 standardının uygulamalarından biridir; doğruladığım referans ekosistem serradura/okf'dur. Go/Ruby/Node çeşitli uygulamaları var.

## 2. Kavramlar → Hangi Sistemlerde

- **Progressive disclosure:** OKF (index.md + search), agentmemory (top-K token bütçesi), Cognee (oturum → grafa geç)
- **Search-before-write:** OKF (yazmadan önce sorgula zorunluluğu), Spacebot (yazım anında near-duplicate kontrolü), agentmemory (superseded sürüm zinciri)
- **Semantic memory:** mem0 (entity linking + multi-signal), Cognee (graf+vektör), Graphiti (hybrid), Spacebot (LanceDB vektör + Tantivy FTS + graf)
- **Memory rot/decay:** Spacebot (importance decay/prune, identity muaf), Graphiti (bi-temporal valid_at/invalid_at), OKF (stale_after), mem0 (temporal reasoning)

## 3. Akademik Kaynaklar (profitlenmiş, arXiv)

- 2404.13501 — "A Survey on the Memory Mechanism of LLM based Agents" (2024-04): ajan-env etkileşiminde hafıza mekanizması taksonomisi.
- 2309.02427 — "Cognitive Architectures for Language Agents" (CoALA, 2023): çalışma belleği (context) / episodik / semantik / prosedürel ayrımı + memory→retrieval→learning döngüsü. Alanın standart çerçevesi.
- 2310.08560 — "MemGPT: Towards LLMs as Operating Systems" (2023): sanal bağlam yönetimi.
- 2501.13956 — Graphiti: bi-temporal bilgi grafikleri (2025).
- 2505.24478 — "Optimizing the Interface Between Knowledge Graphs and LLMs" (Cognee, 2025).

## 4. 2026 Self-Host En İyi Pratik (kanıta dayalı)

1. **Katmanlı hafıza** (CoALA): çalışma belleği (context) + episodik (oturum logu) + semantik (tipli gerçekler) + prosedürel (skills).
2. **Yerel embedding** — API maliyetini kes (Spacebot FastEmbed, agentmemory local all-MiniLM, mem0 local Qwen).
3. **BM25 her zaman açık ucuz katman** + vektör/graf iyileştirme; RRF füzyonu (üç sistem de bunu yapıyor).
4. **Deterministik LLM'siz yollar:** Cognee AUTO_FEEDBACK=false / recall skill gate; Spacebot non-hybrid modları; agentmemory keyless BM25.
5. **Denetlenebilirlik + yaşam döngüsü:** git-native, provenance, trust tier, stale_after (OKF'nin kazanımı).
6. **Recall token bütçesi** sınırı (agentmemory 2000; Spacebot bounded view).

## 5. Spacebot'a Uyarlama Önerisi
- Değiştirme değil, ekleme: (a) hafıza grafiğinin git-denetlenebilir export'u, (b) fact'lere stale_after/doğrulama-tarihi metaverisi, (c) recall'da LLM'siz deterministik yol (havada duran #9 task'ının 3 yolu), (d) memory recall'ın MCP üzerinden dışa açılması. Tümü mevcut SQLite+LanceDB katmanına zarar vermeden eklenir.