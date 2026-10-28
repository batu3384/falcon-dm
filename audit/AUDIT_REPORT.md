# Falcon DM — mühendislik denetim raporu

**Tarih:** 2026-10-28  
**Kapsam:** Tauri 2 masaüstü + React/TS UI + Chrome MV3 eklenti + yerel API  
**Yöntem:** Dört geçiş. Kaynak dosyalar değiştirilmedi. Kanıt `audit/NOTES.md`.

Bulgu şablonu: önem, güven, `dosya:satır`, 1–3 satır alıntı, tetik, etki, çözüm.

---

## Bölüm 1: Proje Bağlamı, Tehdit Modeli ve Mühendislik Karnesi

### Bağlam ve tehdit modeli

Falcon DM, macOS’ta çalışan yerel bir indirme yöneticisi. Motor Rust (HTTP range, HLS+ffmpeg, yt-dlp). Arayüz Tauri webview. Chrome eklentisi indirmeyi `127.0.0.1:14201` üzerinden devralır. Eşleşme native host + kullanıcı onayı. Mutasyonlar `X-Falcon-Token` ve allowlist `Origin` ister.

Kritik veri: diskteki dosya, SQLite satırı, oturum çerezi, profil çerezi, API token, aria2 RPC sırrı. Ürün buluta çıkmaz.

En kötü gerçekçi senaryo uzak RCE değil. Aynı makinede ikinci Falcon süreci aynı SQLite’ı ve sabit aria2 portunu paylaşır. İkinci senaryo: kullanıcı duraklatması 35 sn’de worker’ı bitiremezse satır `Paused` olur, worker dosyayı bitirir, `finish_stream_if_active` `Paused` satırı `Completed` yapmaz.

Kabul edilen ürün sınırı (bulgu değil): YouTube imzalı URL ömrü; eklenti `<all_urls>` + `cookies` (genel yakalama olmadan ürün olmaz).

### Puan (100)

| Kriter | Puan | Gerekçe |
|---|---|---|
| Mimari ve kod kalitesi | 82 | Katmanlar net (queue / http / hls / ytdlp / local_api / zod IPC). `unwrap` üretim yolunda seyrek. Birkaç 800–1000 satırlık modül. |
| İş mantığı ve kararlılık | 76 | Schedule timeout demote etmiyor. Kullanıcı/tray pause hâlâ timeout sonrası Paused yazar. Tek örnek kilidi yok. |
| Güvenlik ve tedarik zinciri | 78 | Token+Origin, SSRF, path allowlist, çerez `skip_serializing`, ChaCha20-Poly1305. `npm audit` high: zod 4.4.3. Anahtar dosyası DB yanında. |
| Performans ve kaynak | 84 | WAL, havuz, status/category/archived indeks, sanal liste, 500 ms tick masaüstü için yeterli. |
| UX | 81 | EN/TR, ErrorBoundary, boş/hata durumları, 44px hedef. İstatistik hatası yutuluyor. |
| Test ve üretime hazırlık | 74 | 131 lib + 142 vitest geçti; clippy/tsc/eslint temiz. CI audit kapısı bu lock ile kırmızı. Canlı hijack yok. Yalnız macOS paket. Sürüm 0.1.0 / eklenti 1.6.0. |

**Özet:** 75–89 bandı. Çalışır yerel ürün. Canlıya “imzalı macOS indirme yöneticisi” diye çıkmadan önce tek örnek, pause/finalize ve zod kilidi kapanmalı. Kritik uzaktan istismar kanıtlanmadı.

---

## Bölüm 2: Bozuk Akışlar, İş Mantığı ve Eşzamanlılık

**[Önem: ORTA]** — **[Güven: Doğrulandı]** — `src-tauri/src/commands.rs:59-77` ve `src-tauri/src/storage/queries.rs:494-496` (tray aynı desen: `src-tauri/src/lifecycle.rs:498-511`, toplam 2 çağrı)

```rust
if !state.queue.cancel_and_wait_stream(id).await {
    log::warn!("pause timed out for download {id}");
}
```

```sql
WHERE id = ?3 AND status IN ('Downloading', 'Merging', 'Queued')
```

* **Sorun & Tetiklenme Yolu:** Pause ve tray Pause All, 35 sn’lik iptal bekleyişi başarısız olsa da satırı `Paused` yapar. yt-dlp finalize `spawn_blocking` kopyası veya `child.kill` bu süreyi aşarsa worker hâlâ `Ok` dönebilir. `finish_stream_if_active` `Paused` satırı tamamlamaz. Schedule/concurrency yolu timeout’ta demote etmez; kullanıcı pause eder.
* **Etki:** Büyük dosyada duraklatma: diskte tamamlanmış dosya, arayüzde Paused. Sürdür ikinci yazıcı veya “already exists” üretebilir. Kullanıcı bitmiş işi bitmemiş sanır.
* **Mühendislik Çözümü:** Timeout’ta Paused yazma; worker bitene kadar Downloading bırak (schedule ile aynı). Ya da `finish_stream_if_active` başarıda Paused’ı da Completed yapsın, yalnızca iptal (`Err("Cancelled")`) Paused kalsın. Büyük kopya için bekleyişi finalize süresine bağla. Test: pause sırasında 35 sn’den uzun `spawn_blocking`.

**[Önem: ORTA]** — **[Güven: Doğrulandı]** — `src-tauri/src/lifecycle.rs:200-212` ve `src-tauri/src/download/engine.rs:64`

```rust
Err(e) => {
    log::error!(
        "Failed to bind Axum server on port 14201: {}. Is another instance running?",
```

```rust
const RPC_PORT: u16 = 6800;
```

* **Sorun & Tetiklenme Yolu:** `Cargo.toml` içinde `tauri-plugin-single-instance` yok. İkinci süreç portu alamaz, loglar, UI ve aynı `falcon_dm.db` ile devam eder. aria2 RPC portu sabit 6800.
* **Etki:** İki kuyruk aynı satırları sürebilir. İkinci aria2 başlamaz veya birincinin RPC’sine çarpar. Eklenti hangi token’a gittiğini ayırt edemez (14201 doluysa birincisine gider).
* **Mühendislik Çözümü:** Tek örnek eklentisi: ikinci açılış pencereyi öne alsın ve çıksın. aria2 portunu boş porta bağla veya birinci sürecin RPC’sini kullan.

### Düşük özet (Bölüm 2)

| Konu | Güven | Not |
|---|---|---|
| Bozuk `settings.json` `unwrap_or_default` | Doğrulandı | `settings.rs:207`. Bellek boş ayar. Sonraki Kaydet diski ezer, eklenti token’ı düşer. |
| Geçici ağ hatası yeniden Queued | Doğrulandı | `queue.rs` “timed out” / “try again” içerirse retry. Sonsuz döngü riski sınırlı; Failed’a sayaç yok. |

---

## Bölüm 3: Yarım Kalan Özellikler, Kopuk Uçlar ve Ölü Kod

**[Önem: ORTA]** — **[Güven: Doğrulandı]** — `package.json:3` ve `extension/manifest.json:3` (toplam 2 sürüm kaynağı)

```json
"version": "0.1.0",
```

```json
"version": "1.6.0",
```

* **Sorun & Tetiklenme Yolu:** Uygulama 0.1.0, eklenti 1.6.0. Güncelleme kontrolü uygulama sürümünü GitHub release ile kıyaslar. Eklenti sürümü bu kıyasa girmez.
* **Etki:** Kullanıcı ve mağaza hangi yapının “güncel” olduğunu bilemez. Eklenti/uygulama protokol sapması sessiz kalır.
* **Mühendislik Çözümü:** Tek semver. Eklenti manifest sürümünü tauri `version` ile üret veya `/api/health` içine protokol sürümü koy; popup uyuşmazsa uyarı versin.

**[Önem: DÜŞÜK]** — **[Güven: Doğrulandı]** — `src-tauri/src/lifecycle.rs:174-176`

```rust
origin_str.starts_with("chrome-extension://")
    || origin_str.starts_with("moz-extension://")
    || origin_str.starts_with("edge-extension://")
```

* **Sorun & Tetiklenme Yolu:** API Firefox origin kabul eder. Repoda Firefox MV3 paketi yok (`wiki/Roadmap.md` Next).
* **Etki:** Özellik yarım. Sahte `moz-extension://` origin, id allowlist’te değilse 403 alır; tek başına yetki vermez.
* **Mühendislik Çözümü:** Firefox gelene kadar origin listesini gerçekten ship edilen tarayıcılarla sınırla, ya da eklentiyi bitir.

**[Önem: DÜŞÜK]** — **[Güven: Doğrulandı]** — `src-tauri/tauri.conf.json:51-53`

```json
"macOS": {
  "signingIdentity": null
}
```

* **Sorun & Tetiklenme Yolu:** Varsayılan paket imzasız. Release workflow Apple sırları tam ise imzalar; yarım sır reddedilir. Sır yoksa imzasız DMG.
* **Etki:** Gatekeeper ilk açılışı keser. Ürün “indir ve çalıştır” değil.
* **Mühendislik Çözümü:** Yayın kanalını yalnızca imzalı+notarize artefakt yap. İmzasızı `dev` etiketiyle ayır.

### Düşük özet (Bölüm 3)

| Konu | Güven | Not |
|---|---|---|
| TODO/FIXME/`unimplemented!` | Doğrulandı | `src`, `src-tauri/src`, `extension` içinde yok. Yarım iş yorum olarak değil, platform boşluğu olarak duruyor. |
| Linux/Windows paket | Doğrulandı | CI uygulama job’u `macos-14`. Yol haritası Next. Ölü kod değil, ship edilmemiş hedef. |

---

## Bölüm 4: Uygulama Güvenliği, Tedarik Zinciri ve Veri Gizliliği

**[Önem: ORTA]** — **[Güven: Doğrulandı]** — `package-lock.json:4632-4633` ve `src/lib/schema.ts:47`

```json
"version": "4.4.3",
```

```ts
export const DownloadArraySchema = z.array(DownloadSchema);
```

* **Sorun & Tetiklenme Yolu:** 2026-10-28 `npm audit --omit=dev --audit-level=high` çıktı: zod &lt; 4.6.6, GHSA-mrhx-c7xc-m3w5 (CVE-2023-54404), CVSS 7.5, yayın 2026-10-01, düzeltme 4.6.6 (2026-10-11). Sınırsız dizi doğrulaması bellek tüketebilir. Bu uygulamada dizi yerel IPC’den gelir, halka açık HTTP gövdesi değil. `get_downloads` istemci limiti 200 gönderir; şema `.max()` taşımaz. CI aynı audit seviyesinde kırmızı olur.
* **Etki:** Ağdan anonim DoS kanıtlanmadı. Webview içinden veya bozuk IPC’den büyük dizi UI sürecini şişirebilir. Asıl operasyonel etki: `main` CI audit kapısı fail.
* **Mühendislik Çözümü:** zod ≥ 4.6.6. `DownloadArraySchema` için `.max(500)` (sayfa zaten 200). Lock’u güncelle, audit’i tekrar koş.

**[Önem: DÜŞÜK]** — **[Güven: Doğrulandı]** — `src-tauri/src/util/cookie_crypto.rs:28-35` ve `38-50`

```rust
let chunk = Uuid::new_v4();
let bytes = chunk.as_bytes();
```

```rust
let path = dir.join("cookie.key");
if let Ok(bytes) = fs::read(&path) {
```

* **Sorun & Tetiklenme Yolu:** Çerez diskte `v1:` ChaCha20-Poly1305. Anahtar `cookie.key` (0600), DB ile aynı kullanıcı dizininde. Nonce ve anahtar UUID v4 baytlarından; sürüm/variant bitleri sabit. `seal` hatası `None` döner (düz metin yazılmaz).
* **Etki:** Aynı UID’deki süreç anahtarı okuyup çözer. Bu, disk şifrelemenin yerel kötü amaçlı yazılım karşısındaki tavanı. Uzak saldırgan bu dosyaya bu kodla ulaşmaz. Nonce çakışması pratik indirme sayısında olası değil; yine de CSPRNG değil.
* **Mühendislik Çözümü:** `getrandom` ile 32 bayt anahtar ve 12 bayt nonce. Anahtarı Keychain’e taşı. `seal` hata verirse indirmeyi fail et, çerezi sessiz düşürme.

**[Önem: DÜŞÜK]** — **[Güven: Muhtemel]** — `.github/workflows/ci.yml:21` ve `release.yml:133` (toplam: checkout, setup-node, rust-toolchain, rust-cache, cargo-deny-action, tauri-action)

```yaml
- uses: actions/checkout@v4
```

```yaml
uses: tauri-apps/tauri-action@v0
```

* **Sorun & Tetiklenme Yolu:** Üçüncü parti action’lar hareketli tag. Tag yeniden işaretlenirse CI/release başka kod koşar.
* **Etki:** Tedarik zinciri. Bugün istismar kanıtı yok.
* **Mühendislik Çözümü:** Tam commit SHA + Dependabot. `permissions:` bloğunu job başına daralt (workflow’da üst seviye `permissions` görünmüyor).

### Düşük özet (Bölüm 4)

| Konu | Güven | Not |
|---|---|---|
| API token `settings.json` düz metin, 0600 | Doğrulandı | Yerel tasarım. Çerez şifreli, token değil. Aynı UID okur. |
| `GET /api/health` tokensız | Doğrulandı | Yalnız loopback. Varlık sızıntısı: “Falcon ayakta”. Mutasyon kapalı. |
| Git geçmişi token dosyası | Muhtemel | `git ls-files` içinde `.env` / `settings.json` yok. `git log -p -n 40 -S api_token` canlı sır göstermedi. Tam tarih taraması (gitleaks) koşulmadı. |
| Eklenti `<all_urls>` + `cookies` + `clipboardRead` | Doğrulandı | Ürün için gerekli. İzin metni README’de. Ayrı zafiyet değil. |
| IPC çerez sızıntısı | Doğrulandı | `models.rs` `#[serde(skip_serializing)]` cookies. UI şeması alanı yok. |

---

## Bölüm 5: Mimari, Performans ve Kaynak

**[Önem: DÜŞÜK]** — **[Güven: Doğrulandı]** — `src-tauri/src/download/http.rs` (984), `extension/background.js` (1026), `extension/content.js` (1049), `src/App.css` (2398)

```rust
// http.rs tek dosyada range, resume, hız limiti, iptal
```

* **Sorun & Tetiklenme Yolu:** İş kuralları doğru dosyalarda (queue vs http vs hls) ama dosyalar büyümüş. CSS tek dosya.
* **Etki:** Bugün ölçülmüş yavaşlık yok. Değişiklik çakışması ve gözden kaçan iptal dalı riski (Bölüm 2 pause yolu buna örnek).
* **Mühendislik Çözümü:** http’yi range/resume ve tek bağlantı diye böl. CSS’i ancak bir ekran değişecekse böl. Erken soyutlama yok.

**[Önem: DÜŞÜK]** — **[Güven: Doğrulandı]** — `src-tauri/src/lifecycle.rs:217-221`

```rust
fn spawn_progress_poll(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
```

* **Sorun & Tetiklenme Yolu:** Her 500 ms kuyruk tick + aria2 poll. İndeks `idx_downloads_status` var (`database.rs`). N+1 kanıtı yok.
* **Etki:** Onlarca aktif işte CPU kabul edilebilir. Binlerce satırda tick tüm Downloading kümesini çeker; bu ürün için sınır uzak.
* **Mühendislik Çözümü:** Aktif küme 100’ü geçerse poll’u yalnızca aria2 gid’lerine indir. Şimdi optimize etme.

### Düşük özet (Bölüm 5)

| Konu | Güven | Not |
|---|---|---|
| `any` kullanımı | Doğrulandı | `src/**/*.ts(x)` içinde `as any` yok. |
| Bağlantı havuzu | Doğrulandı | r2d2, max 8, WAL. Sızıntı kanıtı yok. |
| ffmpeg `-safe 0` | Doğrulandı | `hls.rs` concat. Segment adı `ffmpeg_concat_file_line` ile `seg_NNNNN.ts` dışı reddedilir. |

---

## Bölüm 6: Arayüz, Kullanılabilirlik ve Durum

**[Önem: DÜŞÜK]** — **[Güven: Doğrulandı]** — `src/App.tsx:111-115` (aynı yutma: satır 97, 152, 194; toplam 4)

```tsx
api
  .getStats()
  .then((stats) => setLibraryCounts(libraryCountsFromStats(stats)))
  .catch(() => {});
```

* **Sorun & Tetiklenme Yolu:** İstatistik veya tepsi dili hatası boş `catch`. Kullanıcıya toast yok.
* **Etki:** Kenar çubuğu sayıları eski kalır. İndirme listesi kendi hata durumunu taşır; kabuk çökmez.
* **Mühendislik Çözümü:** İlk hatada status bar’da kısa metin. Sonraki 5 sn poll başarılıysa temizle. Dili logla.

**[Önem: DÜŞÜK]** — **[Güven: Şüpheli]** — `src/App.css` `--target: 44px` ve `tauri.conf.json` `minWidth: 800`

* **Sorun & Tetiklenme Yolu:** Araç çubuğu 44px hedefler + 800px minimum. Bu denetimde pencere ekran görüntüsü alınmadı.
* **Etki:** Dar pencerede araç çubuğu sarabilir veya yatay taşabilir.
* **Mühendislik Çözümü:** 800px ve 1100px’te araç çubuğunu elle gör. Taşma varsa ikincil eylemleri menüye al.

### Düşük özet (Bölüm 6)

| Konu | Güven | Not |
|---|---|---|
| ErrorBoundary | Doğrulandı | Kök, liste, inspector. Test var. |
| Boş liste / hata listesi | Doğrulandı | `DownloadList` empty ve error düğümleri mevcut. Ayrı bulgu yok. |
| EN/TR anahtar eşliği | Doğrulandı | `i18n.test.ts` kilit. |

---

## Bölüm 7: Dayanıklılık, Gözlemlenebilirlik, Test ve CI/CD

**[Önem: ORTA]** — **[Güven: Doğrulandı]** — `.github/workflows/ci.yml:41-42`

```yaml
- name: npm audit (production deps, high+critical fail)
  run: npm audit --omit=dev --audit-level=high
```

* **Sorun & Tetiklenme Yolu:** Kapı var. 2026-10-28 yerel audit high fail (zod). Bu rapor yazılırken `main` üzerindeki lock 4.4.3.
* **Etki:** Yeni push/PR frontend job’u kırmızı. Güvenlik kapısı kendi bağımlılığında takılı.
* **Mühendislik Çözümü:** Bölüm 4 zod yükseltmesi. CI’yi yeşile çekmeden sürüm tag’i atma.

**[Önem: DÜŞÜK]** — **[Güven: Doğrulandı]** — `extension/smoke-test.mjs` (string `includes`)

```js
assert(
  readFileSync(path.join(__dirname, '../src/App.css'), 'utf8').includes('--target: 44px'),
```

* **Sorun & Tetiklenme Yolu:** Smoke, davranış değil kaynak metni arar. Hijack akışı `flow-test.mjs` içinde mock Chrome. Canlı 14201 bu oturumda dinlemiyordu; flow “live Falcon API skipped” der.
* **Etki:** Sözleşme testi regresyonu yakalar. Gerçek Chrome `onDeterminingFilename` → `/api/add` yakalanmaz.
* **Mühendislik Çözümü:** Paketlenmiş uygulamada bir kez hijack (küçük dosya, loopback). Smoke’u davranışa çevirme; ek bir entegrasyon testi yaz.

**[Önem: DÜŞÜK]** — **[Güven: Doğrulandı]** — `src-tauri/src/lifecycle.rs:703-707`

```rust
if let tauri::RunEvent::ExitRequested { .. } = event {
    let state = app_handle.state::<AppState>();
    let _ = state.engine.stop();
```

* **Sorun & Tetiklenme Yolu:** Çıkışta aria2 durur. HTTP/HLS/yt-dlp child’ları için açık bekleme yok. Log `env_logger` + halka tampon, JSON alan yok.
* **Etki:** Kill sırasında parça dosya kalabilir; sonraki açılış resume bunu hedefliyor. Gözlemlenebilirlik masaüstü için yeterli, filo için değil.
* **Mühendislik Çözümü:** Exit’te stream cancel + kısa bekleme. Uzak hata toplayıcı ekleme (ürün yerel-only).

### Düşük özet (Bölüm 7)

| Konu | Güven | Not |
|---|---|---|
| Birim testler | Doğrulandı | `cargo test --lib` 131 geçti. `vitest` 142 geçti. clippy `-D warnings` temiz. `tsc --noEmit` temiz. eslint `--max-warnings 0` temiz. |
| Pause/finalize testi | Doğrulandı | Timeout testi worker’ın map’te kaldığını gösterir. Pause’un Paused yazıp finish’in kaçırması testte yok. |
| `cargo audit` / gitleaks / trivy | — | Kurulu değil, koşulmadı. `cargo deny` binary yok. CI’da deny job’u var; bu oturumda yeniden koşulmadı. |

---

## Bölüm 8: Sektör Kıyaslaması ve Ürün Önerileri

Konum: açık kaynak, yerel, macOS, tarayıcı yakalama + HLS + YouTube. aria2 birincil motor değil, artık yol.

| Ürün | Ne yapıyor | Falcon farkı | Kaynak | Tarih |
|---|---|---|---|---|
| Motrix 2.0.1 | HTTP/FTP/BitTorrent, imzalı macOS 13+, tarayıcı eklentisi | Falcon torrent yok; YouTube/HLS ve Rust motoru var. Motrix imzalı çoklu OS. | https://motrix.app/download/ | sürüm notu 2026-10-25 |
| Free Download Manager 6.35.1 | Parça indirme, torrent, Chrome/Firefox/Edge | Kapalı kaynak. Firefox yakalama Falcon’da yok. | https://www.lynxdownloadmanager.com/guides/best-free-download-manager-mac | karşılaştırma 2026-10-23 |
| AB Download Manager 1.11.1 | Apache-2.0, macOS arm/intel, kuyruk, eklenti | Video sayfası yakalama yok. Falcon’da var. | aynı Lynx rehberi | 2026-10-23 |
| Persepolis 5.2.0 | GPL aria2 GUI, zamanlayıcı, 64 bağlantı | Python. Falcon native + yt-dlp. | https://github.com/persepolisdm/persepolis | 5.2.0 Temmuz 2025 |

Yüksek değer, bu denetimin bug listesi değil:

1. BitTorrent/magnet — rakiplerin taban özelliği. SSRF ve dosya seçimi ayrı tehdit modeli ister. Yol haritasında Later doğru.
2. Firefox eklentisi — FDM/Motrix bunu listeliyor. API `moz-extension` hazır, paket yok.
3. İmzalı çoklu OS — Motrix 2.0.1 bunu 2026-10-25’te ship ediyor. Falcon yalnızca macOS DMG.
4. Tarayıcı mağazası eklentisi — unpacked yükleme rakipten sürtünmeli.

---

## Bölüm 9: Fazlandırılmış Eylem Planı

### Faz 1 — Acil

| İş | Bulgu |
|---|---|
| zod ≥ 4.6.6 ve dizi `.max()`; CI audit yeşil | Bölüm 4 ve 7 |
| Pause/tray timeout’ta Paused yazma; finish ile hizala; test | Bölüm 2 |
| Tek örnek + aria2 port çakışması | Bölüm 2 |

Doğrulanmış uzaktan RCE veya veri silme yok. Faz 1 bu üçü, çünkü biri CI’yi kırıyor, ikisi veri/durum bozuyor.

### Faz 2 — Tamamlama

| İş | Bulgu |
|---|---|
| Tek semver + health protokol sürümü | Bölüm 3 |
| `moz-extension` ya ship ya sil | Bölüm 3 |
| Bozuk settings.json’da save’i blokla, kullanıcıya söyle | Bölüm 2 düşük |
| Yayın kanalı yalnız imzalı | Bölüm 3 |

### Faz 3 — Mimari ve UX

| İş | Bulgu |
|---|---|
| Çerez anahtarı `getrandom` + Keychain | Bölüm 4 |
| İstatistik hatasını status bar’da göster | Bölüm 6 |
| 800px araç çubuğu kontrolü | Bölüm 6 |
| http.rs’yi ancak yeni HTTP işi gelince böl | Bölüm 5 |

### Faz 4 — Kalite ve ürün

| İş | Bulgu |
|---|---|
| Action SHA pin | Bölüm 4 |
| Canlı hijack entegrasyon testi | Bölüm 7 |
| Exit’te stream bekleme | Bölüm 7 |
| Firefox, Linux/Windows, torrent | Bölüm 8 — ayrı ürün kararı |

---

## Bölüm 10: Denetim Kapsam Beyanı

### 1. Tam okunanlar

`package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `deny.toml` (önceki oturum; bu geçişte baştan taranmadı, CI deny job’u dosyaya işaret ediyor), `.github/workflows/ci.yml`, `release.yml` (imza ve sidecar bölümü), `extension/manifest.json`, `README.md` (ilk 120 satır), `src/lib/schema.ts`, `src/api/commands.ts` (indirme IPC), `src/App.tsx` (efektler ve boundary), `src/components/ErrorBoundary.tsx`, `src-tauri/src/lib.rs` (AppState, `check_api_token`), `lifecycle.rs` (API bind, tray pause, exit), `commands.rs` pause/resume/remove, `local_api.rs` (rate limit, health), `download/queue.rs` (cancel, demote, `run_stream_task`), `download/ytdlp.rs` (argv, cancel, finalize), `download/hls.rs` (ffmpeg args, segment adı), `download/engine.rs` (RPC port, secret dosyası), `storage/queries.rs` (`finish_stream_if_active`, progress UPDATE), `storage/models.rs` (cookie skip), `storage/database.rs` (indeks satırları), `settings.rs` load/save, `util/cookie_crypto.rs`.

### 2. Kısmen okunan veya atlanan

| Yol | Neden |
|---|---|
| `src-tauri/src/download/http.rs` (984) | İptal dalları grep ile; dosyanın tamamı satır satır okunmadı. |
| `extension/background.js`, `content.js`, `media-utils.js` | Önceki düzeltmeler ve smoke sözleşmesi biliniyor; bu geçişte baştan satır satır okunmadı. |
| `src/App.css` (2398) | Hedef yükseklik grep. Görsel doğrulama yok. |
| `node_modules/`, `src-tauri/target/`, `dist/` | Üretilmiş / bağımlılık. |
| `docs/codebase-audit/*` | Eski rapor. Bu rapor onların yerine geçmez; güncel kod okundu. |
| Kullanıcı `~/Library/Application Support/com.falcondm.app` | Okunmadı. Canlı token veya çerez dökümü yok. |

### 3. Koşan komutlar

| Komut | Özet |
|---|---|
| `npx tsc --noEmit` | Hata yok |
| `npx eslint . --max-warnings 0` | Hata yok |
| `npm audit --omit=dev --audit-level=high` | 1 high, zod 4.4.3, exit 1 |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | Temiz |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | 131 geçti |
| `npx vitest run` | 142 geçti |
| `git log -p -n 40 -S api_token` | Token dosyası yok; locale/docs isabet |
| `git ls-files` sır adı süzgeci | Eşleşme yok |
| `wc -l` çekirdek dosyalar | Boyut tablosu NOTES’ta |

### 4. Koşulmayanlar

`cargo audit`, `gitleaks`, `trufflehog`, `trivy`, `osv-scanner`, `semgrep`: ortamda bu geçişte çalıştırılmadı (binary yok veya kurulumu yazma sayılır). `cargo deny`: CLI yok; CI job’u tanımlı, bu oturumda yeniden koşulmadı. Canlı Chrome hijack: API dinlemiyordu, başlatılmadı (yan etki). `npm audit fix` ve her türlü `--fix` / kurulum yasak olduğu için yapılmadı.

`./audit/` bu denetim için oluşturuldu. `.gitignore` değiştirilmedi. Dizin izlenmiyorsa commit’e girmez.
