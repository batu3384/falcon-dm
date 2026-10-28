# Falcon DM — denetim notları (2026-10-28)

Salt okunur geçiş. Kaynak ağaç değiştirilmedi. `.gitignore` değiştirilmedi.

## Geçiş 1 — bağlam

- Tür: macOS masaüstü (Tauri 2 + React 19) + Chrome MV3 eklenti + Axum `127.0.0.1:14201`.
- Sürüm: uygulama `0.1.0` (`package.json`, `tauri.conf.json`); eklenti `1.6.0` (`extension/manifest.json`).
- Kritik varlıklar: indirme dosyaları, SQLite `falcon_dm.db`, `settings.json` içindeki `api_token`, profil/oturum çerezleri, `cookie.key`, `aria2_rpc_secret`, yt-dlp’ye giden Cookie başlığı.
- Dış yüzey: loopback API, native messaging, `falcondm://` (yalnız uyandırma), eklenti `<all_urls>` + `cookies` + `clipboardRead`, sidecar `aria2c`/`ffmpeg`, sistem `yt-dlp`.
- Tehdit modeli: birincil saldırgan aynı kullanıcı veya aynı makinedeki başka süreç. Ağdan gelen indirme URL’si SSRF/path için doğrulanıyor. Uzak RCE yüzeyi dar (webview CSP `script-src 'self'`). En kötü gerçekçi senaryo: ikinci süreç aynı DB’yi sürmek, veya duraklatma ile finalize yarışı yüzünden bitmiş dosyanın Paused kalması.
- Tek örnek kilidi yok (`tauri-plugin-single-instance` Cargo.toml’da yok).

## Geçiş 2 — kanıt

### Duraklatma vs tamamlanma
- `commands.rs` pause: `cancel_and_wait_stream` false olsa da `set_status_error_if_current(..., Paused)`.
- `lifecycle.rs` tray pause: aynı, timeout log + yine Paused.
- `finish_stream_if_active`: `status IN ('Downloading', 'Merging', 'Queued')` — Paused hariç.
- yt-dlp `child.kill` + ardından `spawn_blocking` kopya. Kopya 35 sn’yi aşarsa pause Paused yazar, sonra `Ok` yolu finish’i kaçırır.
- Schedule/concurrency timeout’ta demote etmiyor (`queue.rs` “leaving active until worker ends”). Kullanıcı/tray pause etmiyor.

### İkinci süreç
- `TcpListener::bind("127.0.0.1:14201")` Err yalnızca log.
- aria2 `RPC_PORT = 6800` sabit.
- SQLite aynı `app_data_dir()/falcon_dm.db`, exclusive uygulama kilidi yok.

### Ayarlar
- `Settings::load`: `serde_json::from_str(...).unwrap_or_default()`. Bozuk JSON bellek içi boş ayar. Sonraki UI save diske yazar.

### Çerez kripto
- `cookie.key` app data, 0600. Anahtar dosyası DB ile aynı UID.
- Nonce/anahtar `Uuid::new_v4()` baytları. UUID v4 sürüm/variant bitleri rastgele değil.
- `seal_cookies` hata → None (çerez düşer, düz metin yazılmaz).
- `Download.cookies` `#[serde(skip_serializing)]`.

### Tedarik
- `npm audit --omit=dev --audit-level=high` exit 1.
- Lock: `zod@4.4.3`. GHSA-mrhx-c7xc-m3w5 / CVE-2023-54404, düzeltme 4.6.6, yayın 2026-10-01, CVSS 7.5.
- `DownloadArraySchema = z.array(DownloadSchema)` üst sınır yok. Girdi yerel IPC, ağ gövdesi değil.
- CI `npm audit --audit-level=high` bu lock ile kırmızı olur.
- `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `tauri-apps/tauri-action@v0` tag, SHA değil.
- `git ls-files` içinde `.env` / `settings.json` / `cookie.key` yok.
- `git log -p -n 40 -S api_token` canlı token dosyası göstermedi (locale/docs).

### Kopuk / yarım
- CORS `moz-extension://` kabul; depoda Firefox eklentisi yok.
- CI uygulama derlemesi yalnız macOS. Linux/Windows paket yok.
- `signingIdentity: null` tauri.conf; release workflow sır varsa imzalar.

### UX
- `App.tsx` dört `.catch(() => {})`: dil, istatistik.
- ErrorBoundary kök + liste + inspector.
- TODO/FIXME/`unimplemented!` yok (src + extension + src-tauri/src).

### Boyut
- http.rs 984, hls.rs 799, queue.rs 808, queries.rs 819, background.js 1026, content.js 1049, App.css 2398, App.tsx 628.

## Komutlar (2026-10-28)

| Komut | Sonuç |
|---|---|
| `npx tsc --noEmit` | temiz |
| `npx eslint . --max-warnings 0` | temiz |
| `npm audit --omit=dev --audit-level=high` | 1 high: zod |
| `cargo clippy --all-targets -- -D warnings` | temiz |
| `cargo test --lib` | 131 geçti |
| `npx vitest run` | 21 dosya, 142 test geçti |
| `cargo audit` / `gitleaks` / `trivy` / `osv-scanner` | bu geçişte kurulmadığı için koşulmadı |
| `cargo deny` | `cargo deny` yok |

## Geçiş 3 — kaynaklar (erişim 2026-10-28)

- Motrix 2.0.1, 2026-10-25, imzalı macOS, HTTP/FTP/BitTorrent, tarayıcı eklentisi: https://motrix.app/download/ ve https://github.com/agalwood/Motrix/releases
- FDM 6.35.1, torrent + tarayıcı yakalama, kapalı kaynak: https://www.lynxdownloadmanager.com/guides/best-free-download-manager-mac (karşılaştırma 2026-10-23)
- AB Download Manager 1.11.1, Apache-2.0, macOS arm/intel, torrent yok: aynı Lynx rehberi
- Persepolis 5.2.0 (2025-07), GPL-3.0: https://github.com/persepolisdm/persepolis
- IDM sınıfı: kuyruk, tarayıcı yakalama, hız sınırı, zamanlayıcı, çoklu bağlantı. Falcon bunları HTTP/HLS/YouTube için yapıyor; BitTorrent ve Linux/Windows paket yok.
