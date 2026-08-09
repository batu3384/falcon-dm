# Falcon DM Hardening and UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** P0 güvenlik/veri bütünlüğü, P1 frontend/extension akışları ve P2
erişilebilirlik/UI polish gereksinimlerini mevcut Falcon DM mimarisini koruyarak
test edilebilir şekilde tamamlamak.

**Architecture:** Rust tarafında URL/path doğrulama, pairing proof, SQLite durum
geçişleri ve worker cleanup merkezi yardımcılarla güvence altına alınacak.
Frontend Zustand store tek fetch/selection/error kaynağı olacak; extension native
messaging ile pairing proof alacak ve başarısız yakalamalarda native browser
download fallback davranışını koruyacak.

**Tech Stack:** Tauri 2, Rust 2021, Axum 0.7, Tokio, reqwest, SQLite/rusqlite,
React 19, TypeScript 5.8, Zustand 5, Zod 4, Vitest, Chrome/Edge MV3.

## Global Constraints

- API base `http://127.0.0.1:14201` olarak kalacak.
- `Origin` authentication değildir; pairing proof olmadan pairing kabul edilmeyecek.
- YouTube source watch URL olacak, `googlevideo` CDN URL olmayacak.
- `format` explicit yt-dlp selector alanı olarak kalacak.
- Enqueue native browser cancellation işleminden önce gerçekleşecek.
- Falcon başarısız/offline/reject durumunda native browser download korunacak.
- HLS playlist, variant ve segment URL’leri her fetch noktasında doğrulanacak.
- Cookie yalnız gerçek request host için kullanılacak; frontend download JSON cookie taşımayacak.
- Aktif worker claim worker çıkana kadar map içinde tutulacak.
- Dosya taşıma DB güncellemesinden önce filesystem başarısını doğrulayacak.
- Yeni dependency yalnız mevcut paketler yetmediğinde eklenecek.
- UI mevcut Falcon dark-glass palette, dense desktop layout, blue primary,
  amber CTA ve Lucide vocabulary dışına çıkmayacak.
- Her davranış değişikliği önce minimal failing test ile başlayacak.

---

## Dosya haritası

| Alan | Sorumlu dosyalar |
|---|---|
| Rust URL/path güvenliği | `src-tauri/src/util.rs`, `src-tauri/src/download/hls.rs` |
| Pairing proof | `src-tauri/src/lib.rs`, yeni `src-tauri/src/native_messaging.rs`, yeni `src-tauri/src/bin/falcon-dm-native-host.rs`, yeni `scripts/install-native-host.sh`, `extension/background.js`, `extension/manifest.json` |
| Queue/DB atomicity | `src-tauri/src/download/queue.rs`, `src-tauri/src/download/hls.rs`, `src-tauri/src/download/ytdlp.rs`, `src-tauri/src/download/engine.rs`, `src-tauri/src/storage/database.rs` |
| Secret hygiene | `src-tauri/src/storage/models.rs`, `src/api/commands.ts`, `src/types.ts`, `src/lib/schema.ts`, `src/components/DownloadItem.tsx`, `src-tauri/src/lib.rs` |
| Frontend fetch/selection | `src/store/downloads.ts`, `src/store/downloads.test.ts`, `src/App.tsx`, `src/components/DownloadList.tsx` |
| Frontend action contract | yeni `src/lib/downloadCapabilities.ts`, yeni `src/lib/downloadCapabilities.test.ts`, `src/components/CommandPalette.tsx`, `src/components/DownloadItem.tsx`, `src/components/InspectorPanel.tsx` |
| Extension contracts | `extension/background.js`, `extension/content.js`, `extension/manifest.json`, `extension/smoke-test.mjs`, `extension/README.md` |
| UI/a11y/i18n | `src/components/SettingsModal.tsx`, `src/components/NewDownloadModal.tsx`, `src/components/SchedulerModal.tsx`, `src/components/LogPanel.tsx`, `src/components/StatsPanel.tsx`, `src/components/SpeedGraph.tsx`, `src/hooks/useModalA11y.ts`, `src/App.css`, `src/locales/en.json`, `src/locales/tr.json`, `src/i18n.ts` |
| CI/release | `scripts/provision-sidecars.sh`, `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `README.md`, `extension/README.md` |

## Ön kontrol

### Task 0: Baseline verification

**Files:**
- Read: `package.json`
- Read: `src-tauri/Cargo.toml`
- Read: `extension/smoke-test.mjs`

**Interfaces:**
- Produces: Başlangıç test çıktıları ve mevcut failure listesi.

- [ ] **Step 1: Frontend checks çalıştır**

Run:

```bash
rtk npm test
rtk npm run lint
rtk npm run build
```

Expected: Mevcut branch baseline sonuçları kaydedilir; yeni değişiklik öncesi
failure ile yeni failure ayrılır.

- [ ] **Step 2: Rust ve extension checks çalıştır**

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml
rtk cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
rtk node extension/smoke-test.mjs
```

Expected: Rust unit tests, formatting ve extension smoke sonucu alınır.

- [ ] **Step 3: Baseline sonrası planlanan ilk task dışında kod değiştirme**

Commit: Bu task için commit yok; baseline yalnızca ölçüm adımıdır.

---

## Faz P0 — Güvenlik ve veri bütünlüğü

### Task 1: HLS fetch policy ve cookie host isolation

**Files:**
- Modify: `src-tauri/src/util.rs`
- Modify: `src-tauri/src/download/hls.rs`
- Test: `src-tauri/src/util.rs` test module
- Test: `src-tauri/src/download/hls.rs` test module

**Interfaces:**
- Produces: `pub fn validate_fetch_url(raw: &str) -> Result<url::Url, String>`.
- Produces: `async fn read_bounded_response(response: reqwest::Response, max_bytes: usize) -> Result<Vec<u8>, String>`.
- Produces: `fn cookie_header_for_target(source: &url::Url, target: &url::Url, cookies: Option<&str>) -> Option<HeaderValue>`.
- Consumes: Existing `validate_download_url`, `sanitize_header_value`, `HlsHeaders`.

- [ ] **Step 1: URL policy için failing tests yaz**

```rust
#[test]
fn validate_fetch_url_rejects_private_literal_and_unsupported_scheme() {
    assert!(validate_fetch_url("http://127.0.0.1/live.m3u8").is_err());
    assert!(validate_fetch_url("http://[::1]/live.m3u8").is_err());
    assert!(validate_fetch_url("file:///tmp/live.m3u8").is_err());
}

#[test]
fn validate_fetch_url_accepts_public_http_url() {
    assert!(validate_fetch_url("https://cdn.example.com/live.m3u8").is_ok());
}
```

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml util::tests::validate_fetch_url
```

Expected: Testler yeni fonksiyon olmadığı için FAIL eder.

- [ ] **Step 2: Cookie host isolation için failing test yaz**

```rust
#[test]
fn cookies_are_not_sent_to_unrelated_segment_host() {
    let source = Url::parse("https://media.example.com/master.m3u8").unwrap();
    let same_host = Url::parse("https://media.example.com/seg.ts").unwrap();
    let other_host = Url::parse("https://cdn.example.net/seg.ts").unwrap();
    assert!(cookie_header_for_target(&source, &same_host, Some("sid=abc")).is_some());
    assert!(cookie_header_for_target(&source, &other_host, Some("sid=abc")).is_none());
}
```

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml hls::tests::cookies_are_not_sent
```

Expected: Test FAIL eder.

- [ ] **Step 3: Fetch helpers ve bounded response implement et**

`validate_fetch_url` yalnız `http`/`https` kabul eder, literal private/loopback/
link-local/unspecified host ve DNS private çözümünü reddeder. `hls.rs` içinde
reqwest client `Policy::custom` ile redirect hop’larını aynı policy’den geçirir.
`read_bounded_response` `content_length` limitini erken kontrol eder ve
`bytes_stream()` chunks toplamı limiti aşınca response’u hata ile keser.

Kullanılacak limitler:

```rust
const MAX_PLAYLIST_BYTES: usize = 16 * 1024 * 1024;
const MAX_SEGMENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_SEGMENTS: usize = 10_000;
const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
```

`process_hls_stream` başlangıç URL’sini, master variant URL’sini ve her segment
URL’sini fetch öncesi `validate_fetch_url` ile doğrular. Playlist/segment
response body `read_bounded_response` üzerinden okunur. Segment toplamı ve
indirilen toplam byte `MAX_SEGMENTS`/`MAX_OUTPUT_BYTES` ile sınırlandırılır.
Client default cookie header yerine her request’te `cookie_header_for_target`
kullanır.

- [ ] **Step 4: Redirect, size ve isolation testlerini geçir**

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml util::tests hls::tests
```

Expected: URL, cookie ve mevcut HLS unit testleri PASS eder.

- [ ] **Step 5: Commit**

```bash
rtk git add src-tauri/src/util.rs src-tauri/src/download/hls.rs
rtk git commit -m "fix: harden HLS fetch boundaries"
```

### Task 2: Native-messaging pairing proof

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/settings.rs`
- Create: `src-tauri/src/native_messaging.rs`
- Create: `src-tauri/src/bin/falcon-dm-native-host.rs`
- Create: `scripts/install-native-host.sh`
- Modify: `extension/background.js`
- Modify: `extension/manifest.json`
- Modify: `extension/README.md`
- Test: `src-tauri/src/native_messaging.rs` test module
- Test: `src-tauri/src/lib.rs` test module

**Interfaces:**
- `PairRequest { extension_id: String, challenge: String, proof: String }`.
- `NativePairRequest { extension_id: String, challenge: String }`.
- `PairProofStore::issue(challenge: &str, extension_id: &str) -> String`.
- `PairProofStore::consume(challenge: &str, extension_id: &str, proof: &str) -> bool`.
- `read_native_message<R: Read>(reader: &mut R) -> Result<Vec<u8>, String>`.
- `write_native_message<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), String>`.

**Threat boundary:** Browser-controlled `Origin` yalnız extension ID format ve
header/body consistency için kontrol edilir. Pairing proof, browser HTTP
request’inden değil, registered native host ile gerçek Falcon process
arasındaki 0600 Unix socket channel’dan gelir. Token deep-link query string’e
ve native host frame dışına çıkmaz.

- [ ] **Step 1: Native messaging framing için failing tests yaz**

```rust
#[test]
fn native_message_uses_little_endian_length_prefix() {
    let mut out = Vec::new();
    write_native_message(&mut out, br#"{"ok":true}"#).unwrap();
    assert_eq!(&out[..4], &(10u32).to_le_bytes());
    assert_eq!(&out[4..], br#"{"ok":true}"#);
}

#[test]
fn native_message_rejects_oversized_frame() {
    let mut input = (65_537u32).to_le_bytes().to_vec();
    input.extend(std::iter::repeat(b'x').take(65_537));
    assert!(read_native_message(&mut input.as_slice()).is_err());
}
```

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml native_messaging::tests
```

Expected: Testler yeni framing helper olmadığı için FAIL eder.

- [ ] **Step 2: Proof consume testini yaz**

```rust
#[test]
fn pair_proof_is_single_use_and_extension_bound() {
    let store = PairProofStore::default();
    let proof = store.issue("challenge-1", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert!(store.consume("challenge-1", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &proof));
    assert!(!store.consume("challenge-1", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &proof));
    assert!(!store.consume("challenge-1", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &proof));
}
```

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml native_messaging::tests::pair_proof
```

Expected: Test FAIL eder.

- [ ] **Step 3: App-side proof store ve Unix socket server ekle**

`AppState` içine expiry alanı olan `PairProofStore` ekle. `issue` UUID proof
üretir; `consume` challenge + extension ID + proof eşleşmesini atomik olarak
silip tek kullanımlı başarı döndürür; 60 saniyeden eski kayıtlar temizlenir.
App startup sırasında `<app_data_dir>/pairing.sock` stale socket’i silinir,
Unix listener başlatılır ve socket permissions `0600` yapılır. Socket mesajı
`NativePairRequest` alır, proof üretir ve framed response döner.

`handle_pair` JSON body alacak:

```rust
async fn handle_pair(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<PairRequest>,
) -> Response
```

Origin’den çıkan extension ID body `extension_id` ile eşleşmezse `403`.
Proof consume başarısızsa `403`. ID user-approved ise mevcut token + `200`,
değilse `202` pending response ve `pair-request` event döner. `check_api_token`
data-plane çağrılarında token + allowlisted extension origin kontrollerini
korur; empty allowlist artık “her extension” anlamına gelmez.

- [ ] **Step 4: Native host binary ve manifest ekle**

`falcon-dm-native-host` stdin/stdout üzerinde Chrome Native Messaging
little-endian framing kullanır, yalnız `NativePairRequest` kabul eder, app
Unix socket’ine bağlanır ve app response’unu geri verir. Frame boyutu 64 KiB
ile sınırlıdır. `scripts/install-native-host.sh` binary path’inden Chrome ve
Edge Native Messaging manifest’lerini üretir ve iki browser profile’ına kurar.
Host binary release pipeline’da ayrı native-host artifact olarak üretilir;
Tauri `externalBin` listesine eklenmez çünkü provisioning öncesi local
`cargo test` build’ini bloke eder. Host extension ID’yi kendisi üretmez;
request’teki ID’yi app’e iletir.

- [ ] **Step 5: Extension pairing akışını native proof ile değiştir**

`ensurePaired` önce random challenge üretir, `chrome.runtime.sendNativeMessage`
ile proof alır, sonra `/api/pair` body’sine `extension_id`, `challenge`, `proof`
gönderir. Native host unavailable, timeout, app offline veya pairing reject
durumunda `setState("offline")` ve mevcut fallback yolu çalışır. Pairing poll
bounded kalır. `manifest.json` içine `nativeMessaging` eklenir.

- [ ] **Step 6: Pairing contract testlerini geçir**

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml native_messaging::tests
rtk cargo test --manifest-path src-tauri/Cargo.toml lib::tests
rtk node extension/smoke-test.mjs
```

Expected: Frame, single-use proof, extension ID binding ve mevcut smoke testleri
PASS eder.

- [ ] **Step 7: Commit**

```bash
rtk git add src-tauri/src/lib.rs src-tauri/src/settings.rs src-tauri/src/native_messaging.rs src-tauri/src/bin/falcon-dm-native-host.rs scripts/install-native-host.sh extension/background.js extension/manifest.json extension/README.md
rtk git commit -m "fix: bind extension pairing to native proof"
```

### Task 3: Safe move/rename and overwrite protection

**Files:**
- Modify: `src-tauri/src/util.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/storage/database.rs`
- Test: `src-tauri/src/util.rs` test module
- Test: `src-tauri/src/storage/database.rs` test module

**Interfaces:**
- `pub fn resolve_download_target(save_dir: &str, filename: &str) -> Result<PathBuf, String>`.
- `fn copy_file_exclusive(source: &Path, destination: &Path) -> Result<(), String>`.
- `Database::update_download_if_status(id: i64, expected: &[DownloadStatus], download: &Download) -> Result<bool>`.

- [ ] **Step 1: Path and status rejection tests yaz**

```rust
#[test]
fn move_target_rejects_traversal_and_absolute_filename() {
    let root = std::env::temp_dir().join(format!("falcon-dm-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    assert!(resolve_download_target(root.to_str().unwrap(), "../outside.mp4").is_err());
    assert!(resolve_download_target(root.to_str().unwrap(), "/tmp/outside.mp4").is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn move_rejects_active_download_status() {
    let db = Database::in_memory().unwrap();
    let mut download = create_test_download("active.mp4");
    download.status = DownloadStatus::Downloading;
    let id = db.insert_download(&download).unwrap();
    assert!(db.update_download_if_status(id, &[DownloadStatus::Completed], &download).unwrap() == false);
}
```

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml util::tests::move_target
rtk cargo test --manifest-path src-tauri/Cargo.toml storage::database::tests::move_rejects_active
```

Expected: Testler yeni target helper/conditional update olmadan FAIL eder.

- [ ] **Step 2: Central target resolver implement et**

`move_download` destination için doğrudan `create_dir_all(p)` kullanmayacak.
`resolve_download_save_path` ile aynı allowed root/category policy’sini
kullanacak, mevcut symlink component’lerini canonicalize edip root dışına
çıkışı reddedecek ve filename’i `sanitize_filename` sonrası yeniden validate
edecek. Yalnız `Completed` ve `Failed` durumları move/rename edilebilir.

- [ ] **Step 3: No-overwrite filesystem operation implement et**

Destination varsa işlem başlamadan hata döndür. `copy_file_exclusive` final
dosyayı `OpenOptions::create_new(true)` ile oluşturur, source byte count ile
destination metadata’yı karşılaştırır, ardından source’u siler. Aynı filesystem
rename mümkün olduğunda da destination existence kontrolü korunur; hiçbir yol
mevcut destination’ı overwrite etmez. Filesystem tamamlanmadan DB satırı
değişmez.

- [ ] **Step 4: Symlink, collision ve DB ordering testlerini geçir**

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml util::tests
rtk cargo test --manifest-path src-tauri/Cargo.toml storage::database::tests
```

Expected: traversal, symlink escape, collision, active-state ve DB ordering
testleri PASS eder.

- [ ] **Step 5: Commit**

```bash
rtk git add src-tauri/src/util.rs src-tauri/src/lib.rs src-tauri/src/storage/database.rs
rtk git commit -m "fix: make file moves path-safe and non-destructive"
```

### Task 4: Queue, stream cancellation and aria2 claim atomicity

**Files:**
- Modify: `src-tauri/src/download/queue.rs`
- Modify: `src-tauri/src/download/hls.rs`
- Modify: `src-tauri/src/download/ytdlp.rs`
- Modify: `src-tauri/src/download/engine.rs`
- Modify: `src-tauri/src/storage/database.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: Rust unit tests in the modified modules

**Interfaces:**
- `Database::claim_aria2_download(id: i64, gid: &str) -> Result<bool>`.
- `Database::finish_stream_if_active(id: i64, size: u64) -> Result<bool>`.
- `Database::pause_stream_if_active(id: i64) -> Result<bool>`.
- `Database::clear_session_cookies(id: i64) -> Result<bool>`.
- `QueueManager::cancel_stream(&self, id: i64) -> bool` sends cancellation but does not remove the active claim.

- [x] **Step 1: Claim rollback testini yaz**

```rust
#[test]
fn aria2_claim_is_single_winner() {
    let db = Database::in_memory().unwrap();
    let download = create_test_download("claim.bin");
    let id = db.insert_download(&download).unwrap();
    assert!(db.claim_aria2_download(id, "gid-1").unwrap());
    assert!(!db.claim_aria2_download(id, "gid-2").unwrap());
    assert_eq!(db.get_download(id).unwrap().aria2_gid.as_deref(), Some("gid-1"));
}
```

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml storage::database::tests::aria2_claim
```

Expected: Test FAIL eder.

- [x] **Step 2: Stream terminal race testini yaz**

```rust
#[test]
fn completed_transition_does_not_override_paused_state() {
    let db = Database::in_memory().unwrap();
    let mut download = create_test_download("stream.mp4");
    download.status = DownloadStatus::Paused;
    let id = db.insert_download(&download).unwrap();
    assert!(!db.finish_stream_if_active(id, 100).unwrap());
    assert_eq!(db.get_download(id).unwrap().status, DownloadStatus::Paused);
}
```

- [x] **Step 3: Queue active claim lifecycle düzelt**

`cancel_stream` sender’ı map’ten çıkarmayacak. `run_stream_task` success,
cancel ve failure cleanup sonunda map entry’yi remove edecek. DB finalization
conditional transition ile yapılacak; cancelled worker `Completed` yazamayacak.
Cookie clear her terminal state’te çağrılacak.

Aria2 için:

1. `engine.add_download` ile GID al.
2. `Database::claim_aria2_download` ile `Queued -> Downloading` ve GID tek SQL update.
3. Claim false veya DB error olursa `engine.remove(gid)` çağır, DB satırı queued bırak.
4. `--allow-overwrite=false` kullan.
5. `resume_download` `Completed` ve active state’lerde açık hata döndürsün.

- [x] **Step 4: HLS/yt-dlp cancellation implement et**

HLS request ve retry backoff’ları `tokio::select!` ile cancellation receiver’a
bağla. ffmpeg `spawn` child handle üzerinden çalışsın; cancellation gelince
child kill, wait ve temp cleanup yapılsın. yt-dlp child için aynı cancellation
policy uygulanacak. `TempDirGuard` her exit path’te kalacak.

- [x] **Step 5: Worker lifecycle testlerini geçir**

Run:

```bash
rtk cargo test --manifest-path src-tauri/Cargo.toml download::queue::tests
rtk cargo test --manifest-path src-tauri/Cargo.toml download::hls::tests
rtk cargo test --manifest-path src-tauri/Cargo.toml storage::database::tests
```

Expected: single claim winner, cancellation state, no-late-completion ve queue
map cleanup testleri PASS eder.

- [x] **Step 6: Commit**

```bash
rtk git add src-tauri/src/download/queue.rs src-tauri/src/download/hls.rs src-tauri/src/download/ytdlp.rs src-tauri/src/download/engine.rs src-tauri/src/storage/database.rs src-tauri/src/lib.rs
rtk git commit -m "fix: make download worker claims and cancellation atomic"
```

### Task 5: Cookie exposure, permissions and sidecar cleanup

**Files:**
- Modify: `src-tauri/src/storage/models.rs`
- Modify: `src-tauri/src/storage/database.rs`
- Modify: `src-tauri/src/settings.rs`
- Modify: `src-tauri/src/download/engine.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/types.ts`
- Modify: `src/lib/schema.ts`
- Modify: `src/api/commands.ts`
- Modify: `src/components/DownloadItem.tsx`
- Test: `src-tauri/src/storage/database.rs` test module
- Test: `src/lib/schema.test.ts`

**Interfaces:**
- `Download.cookies` storage-only field; serialized frontend `DownloadModel`
  içinde bulunmaz.
- `DownloadSchema` cookies kabul etmez.
- `getLogs` ve `getStats` Zod ile parse edilmiş payload döndürür.

- [x] **Step 1: Frontend payload testini yaz**

```ts
it('does not expose cookies in download payload schema', () => {
  const parsed = DownloadSchema.parse({
    ...baseDownload,
    cookies: 'sid=secret',
  });
  expect(parsed).not.toHaveProperty('cookies');
});
```

Run:

```bash
rtk npm test -- src/lib/schema.test.ts
```

Expected: Schema strip davranışı tanımlı değilse test FAIL eder.

- [x] **Step 2: Storage-only cookie serialization implement et**

Rust `Download` serde output’unda `cookies` skip edilir. TypeScript
`DownloadModel`, test fixtures, `DownloadItem` curl/redownload yolları
cookie’siz API modeline geçirilir. `NewDownloadModal` ve Settings site profile
formları kullanıcıdan cookie almayı sürdürebilir; bu alanlar yalnız enqueue/
settings save request’inde backend’e gider.

- [x] **Step 3: Terminal cookie cleanup ve file permissions ekle**

`run_stream_task`, cancelled/failed/completed terminal geçişlerinde
`clear_session_cookies` çağırır. Startup recovery failed/completed rows için
cookie wipe yapar. Database dosyası, settings, aria2 config/secret ve geçici
pairing socket `0600` olur.

- [x] **Step 4: aria2 config cleanup bug’ını düzelt**

`Aria2Engine::stop` `secret_file.take()` sonrasında parent aramaya çalışmayacak.
Config path startup sırasında ayrı tracked field olarak tutulacak veya
`app_data_dir` path’i doğrudan kullanılacak. Stop testinde config, secret ve pid
dosyalarının üçünün de silindiği doğrulanır.

- [x] **Step 5: Testleri geçir ve commit**

Run:

```bash
rtk npm test -- src/lib/schema.test.ts src/store/downloads.test.ts
rtk cargo test --manifest-path src-tauri/Cargo.toml storage::database::tests
rtk cargo test --manifest-path src-tauri/Cargo.toml download::engine::tests
```

Commit:

```bash
rtk git add src-tauri/src/storage/models.rs src-tauri/src/storage/database.rs src-tauri/src/settings.rs src-tauri/src/download/engine.rs src-tauri/src/lib.rs src/types.ts src/lib/schema.ts src/api/commands.ts src/components/DownloadItem.tsx src/lib/schema.test.ts src/store/downloads.test.ts
rtk git commit -m "fix: keep session cookies out of frontend payloads"
```

### Task 6: Sidecar integrity and release artifact verification

**Files:**
- Modify: `scripts/provision-sidecars.sh`
- Create: `scripts/provision-sidecars.test.sh`
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/release.yml`
- Modify: `README.md`
- Modify: `extension/README.md`
- Test: shell script validation step in CI

**Interfaces:**
- `scripts/provision-sidecars.sh` `ARCH`, `FFMPEG_URL`, `FFMPEG_SHA256`,
  `ARIA2_SHA256` değerlerini doğrulayarak binary kopyalar.
- CI extension smoke command: `rtk node extension/smoke-test.mjs`.

- [ ] **Step 1: Sidecar checksum testini yaz**

`provision-sidecars.sh` içindeki `verify_sha256` helper’ı ana provisioning
akışından ayrılır; `scripts/provision-sidecars.test.sh` küçük fixture dosyası
üzerinde doğru hash’i kabul edip yanlış hash’i reddettiğini test eder:

```bash
printf 'falcon-sidecar-fixture' > "$tmp/fixture"
expected="$(shasum -a 256 "$tmp/fixture" | awk '{print $1}')"
verify_sha256 "$tmp/fixture" "$expected"
if verify_sha256 "$tmp/fixture" "000000"; then exit 1; fi
```

Run:

```bash
rtk bash scripts/provision-sidecars.test.sh
rtk bash -n scripts/provision-sidecars.sh
rtk node extension/smoke-test.mjs
```

Expected: Syntax ve extension contract check PASS eder; checksum değeri
olmayan remote download CI’da fail eder.

- [ ] **Step 2: Provision script’i pinle**

Homebrew binary için `shasum -a 256` ile beklenen architecture/version hash
kontrol edilir. x86_64 ffmpeg URL’si immutable version URL’ye ve sabit SHA-256
değerine geçirilir. `curl --fail --location --retry 3` kullanılır; indirme
hash’i eşleşmezse binary install edilmez. macOS’ta `readlink -f` yerine
`python3` veya `realpath` ile gerçek dosya yolu alınır.

- [ ] **Step 3: CI ve release guard ekle**

CI frontend job’ına extension smoke ve manifest JSON validation eklenir.
Release Node version `22` olur. Release checksum yalnız `.dmg`/`.app` değil
provision edilen sidecar binary’lerini ve `falcon-dm-native-host` artifact’ini
de kapsar. Release Tauri build’den önce architecture-specific native host
binary’sini `cargo build --bin falcon-dm-native-host --target ...` ile üretir,
installer’a verir. Signed/notarized build Apple credentials varsa, unsigned
fallback yoksa build fail eder; her architecture için checksum artifact upload
edilir.

- [ ] **Step 4: Dokümantasyonu güncelle ve commit**

README ve extension README pairing native host install/approval, cookie
fallback, sidecar verification ve signed/unsigned artifact davranışını mevcut
implementation ile eşleştirir.

```bash
rtk git add scripts/provision-sidecars.sh .github/workflows/ci.yml .github/workflows/release.yml README.md extension/README.md
rtk git commit -m "ci: verify sidecars and extension contracts"
```

---

## Faz P1 — Frontend ve extension akışları

### Task 7: Download store request sequencing and error states

**Files:**
- Modify: `src/store/downloads.ts`
- Modify: `src/store/downloads.test.ts`
- Modify: `src/App.tsx`
- Modify: `src/components/DownloadList.tsx`
- Modify: `src/api/commands.ts`
- Modify: `src/types.ts`

**Interfaces:**

```ts
interface DownloadsState {
  downloads: DownloadModel[];
  loading: boolean;
  error: string | null;
  archived: boolean;
  requestSequence: number;
  selectedDownload: DownloadModel | null;
  selectedIds: Set<number>;
  lastSelectId: number | null;
  fetchDownloads: (archived?: boolean) => Promise<void>;
  retryFetch: () => Promise<void>;
}
```

- [ ] **Step 1: Out-of-order ve archived polling testlerini yaz**

```ts
it('ignores an older response after a newer filter request', async () => {
  function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((next) => {
      resolve = next;
    });
    return { promise, resolve };
  }

  const first = deferred<DownloadModel[]>();
  const second = deferred<DownloadModel[]>();
  vi.mocked(getDownloads).mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
  const a = useDownloadsStore.getState().fetchDownloads(false);
  const b = useDownloadsStore.getState().fetchDownloads(true);
  second.resolve([{ ...baseDownload, archived: true }]);
  first.resolve([baseDownload]);
  await Promise.all([a, b]);
  expect(useDownloadsStore.getState().downloads[0].archived).toBe(true);
});

it('retry preserves archived query', async () => {
  await useDownloadsStore.getState().fetchDownloads(true);
  expect(vi.mocked(getDownloads)).toHaveBeenLastCalledWith({ archived: true });
});
```

Run:

```bash
rtk npm test -- src/store/downloads.test.ts
```

Expected: Existing store request lacks sequence/error/current archived state,
testler FAIL eder.

- [ ] **Step 2: Store request sequence implement et**

Her fetch `requestSequence` artırır; response yalnız kendi sequence’i halen
current ise state’i yazar. `error` set/clear edilir, `retryFetch` current
archived query’yi kullanır, archived mode polling sırasında kaybolmaz.
Başarılı fetch selected IDs’yi mevcut row ID’leri ile kesiştirir ve seçili
download’u live row’a bağlar.

- [ ] **Step 3: App ve list state ayrımını bağla**

`App.tsx` interval `fetchDownloads()` ile store’un current archived query’sini
kullanır. `activeCategory` değişince explicit archived value gönderilir.
`DownloadList` loading, error ve empty state’i ayrı render eder; error state
Retry CTA gösterir; Archived listeyi tekrar active category filtresiyle
yanlışlıkla daraltmaz.

- [ ] **Step 4: Store/UI testlerini geçir ve commit**

```bash
rtk npm test -- src/store/downloads.test.ts
rtk npm run build
rtk git add src/store/downloads.ts src/store/downloads.test.ts src/App.tsx src/components/DownloadList.tsx src/api/commands.ts src/types.ts
rtk git commit -m "fix: make download list polling race-safe"
```

### Task 8: Shared status capabilities, batch partial results and palette actions

**Files:**
- Create: `src/lib/downloadCapabilities.ts`
- Create: `src/lib/downloadCapabilities.test.ts`
- Modify: `src/App.tsx`
- Modify: `src/components/DownloadItem.tsx`
- Modify: `src/components/InspectorPanel.tsx`
- Modify: `src/components/CommandPalette.tsx`
- Modify: `src/components/ConfirmDialog.tsx`

**Interfaces:**

```ts
export interface DownloadCapabilities {
  pause: boolean;
  resume: boolean;
  remove: boolean;
  move: boolean;
  archive: boolean;
}

export function getDownloadCapabilities(status: DownloadStatus): DownloadCapabilities;
```

- [ ] **Step 1: Status matrix testini yaz**

```ts
it('does not expose pause or move for Merging', () => {
  expect(getDownloadCapabilities('Merging')).toMatchObject({
    pause: false,
    move: false,
  });
});

it('allows move only for terminal completed/failed rows', () => {
  expect(getDownloadCapabilities('Completed').move).toBe(true);
  expect(getDownloadCapabilities('Failed').move).toBe(true);
  expect(getDownloadCapabilities('Downloading').move).toBe(false);
});
```

- [ ] **Step 2: Capability map’i tüm action consumer’lara bağla**

DownloadItem, InspectorPanel, toolbar ve App batch handler kendi status
koşullarını tekrarlamak yerine `getDownloadCapabilities` kullanır. Merging
satırı pause/move gibi unsupported action göstermez.

- [ ] **Step 3: Batch result modelini implement et**

Batch `Promise.allSettled` ile her target sonucu toplar. Başarılı/başarısız ID
ve filename sayıları toast içinde raporlanır; failed item tekrar denenebilir,
başarılı item duplicate enqueue edilmez. Delete batch tek `ConfirmDialog`
sonrası çalışır.

- [ ] **Step 4: Async Command Palette davranışını düzelt**

`PaletteAction.run` tipi `() => void | Promise<void>` olur. Enter/click handler
`await action.run()` sonrası `onClose()` çağırır. Action hata verirse palette
kapanmaz ve toast callback ile kullanıcıya hata gösterilir.

- [ ] **Step 5: Clipboard rejection testini ve UI fixini ekle**

URL/cURL `navigator.clipboard.writeText` rejection yakalanır; success toast
yalnız promise resolve olunca gösterilir, failure toast gerçek hata verir.

- [ ] **Step 6: Test ve commit**

```bash
rtk npm test -- src/lib/downloadCapabilities.test.ts src/store/downloads.test.ts
rtk npm run lint
rtk git add src/lib/downloadCapabilities.ts src/lib/downloadCapabilities.test.ts src/App.tsx src/components/DownloadItem.tsx src/components/InspectorPanel.tsx src/components/CommandPalette.tsx src/components/ConfirmDialog.tsx
rtk git commit -m "feat: unify download actions and batch feedback"
```

### Task 9: Extension timeout, tab lifecycle, cookie targets and batch results

**Files:**
- Modify: `extension/background.js`
- Modify: `extension/content.js`
- Modify: `extension/manifest.json`
- Modify: `extension/smoke-test.mjs`
- Modify: `extension/README.md`

**Interfaces:**
- `withTimeout(promise, timeoutMs, label)`.
- `getCookiesHeader(url)`.
- `sendToFalcon(path, body)` rejection preserves caller fallback.
- `batch_download` response `{ success, results: [{ url, ok, id?, error? }] }`.

- [ ] **Step 1: Contract smoke assertions ekle**

`extension/smoke-test.mjs` manifest parse eder ve şu contract’ları assert eder:
`nativeMessaging` permission, localhost host permission, `format` field,
`suggest({ cancel: false })` fallback ve `Promise.allSettled` batch handling.

Run:

```bash
rtk node extension/smoke-test.mjs
```

Expected: Yeni source contract’ları mevcut kodda yoksa FAIL eder.

- [ ] **Step 2: Timeout wrapper ve native pairing timeout uygula**

Health, wake, `sendNativeMessage`, `/api/pair` poll ve `postFalcon` request’leri
bounded timeout kullanır. Timeout sonrası pairing state offline olur; browser
download listener `suggest({ cancel: false })` çağırır ve promise pending
bırakmaz.

- [ ] **Step 3: Tab navigation state cleanup ekle**

`chrome.tabs.onUpdated` URL değiştiğinde ilgili `MEDIA_URLS`, `MEDIA_META` ve
injected marker temizlenir. `onRemoved` cleanup korunur. YouTube SPA page
change sonrası stale quality/media source gönderilmez.

- [ ] **Step 4: Cookie collection’ı target URL’ye taşı**

Batch item başına `getCookiesHeader(it.url)` çağrılır. `page_url` cookie yalnız
item URL için cookie bulunamadığında fallback olur. Request body’ye bir host’un
cookie’si başka host item’ına kopyalanmaz.

- [ ] **Step 5: Batch partial result ve YouTube format contract’ını koru**

Batch `Promise.allSettled` ile item başına result döndürür. Successful item
retry listesine alınmaz. `content.js` watch URL + bounded height format üretmeye
devam eder; source URL olarak `googlevideo` gönderilmez.

- [ ] **Step 6: Test ve commit**

```bash
rtk node extension/smoke-test.mjs
rtk git add extension/background.js extension/content.js extension/manifest.json extension/smoke-test.mjs extension/README.md
rtk git commit -m "fix: make extension capture fallback deterministic"
```

---

## Faz P2 — UI polish ve accessibility

### Task 10: Modal/form state, validation and focus semantics

**Files:**
- Modify: `src/hooks/useModalA11y.ts`
- Modify: `src/components/ConfirmDialog.tsx`
- Modify: `src/components/SettingsModal.tsx`
- Modify: `src/components/NewDownloadModal.tsx`
- Modify: `src/components/SchedulerModal.tsx`
- Modify: `src/components/DownloadItem.tsx`
- Modify: `src/locales/en.json`
- Modify: `src/locales/tr.json`
- Test: yeni/var olan component tests

**Interfaces:**
- Modal opening: focus Cancel/close control first.
- Modal close: Escape, overlay click and Cancel all restore previous focus.
- Save/send buttons disable while request is active.
- Numeric bounds: concurrent downloads `1..32`, connections/server `1..16`,
  speed limit `0..1_048_576`.

- [ ] **Step 1: Modal focus testlerini yaz**

```tsx
it('focuses cancel before destructive confirm', async () => {
  render(<ConfirmDialog message="Delete?" onConfirm={onConfirm} onCancel={onCancel} />);
  expect(screen.getByRole('button', { name: /cancel/i })).toHaveFocus();
});
```

- [ ] **Step 2: useModalA11y focus orderunu düzelt**

Focusable list içinde `[data-modal-cancel]` işaretli Cancel/close control ilk
tercih olur; yoksa ilk visible focusable kullanılır. Escape propagation
durur, Tab/Shift+Tab trap edilir, unmount önceki active element’e döner.

- [ ] **Step 3: Form dirty/loading/validation state ekle**

Settings initial defaults async `getSettings` response gelmeden user input’u
ezmez. Save sırasında button disabled + pending label olur. Modal dirty ise
close confirmation gerekir. NewDownload ve Scheduler invalid numeric/time
input’u submit öncesi reddeder.

- [ ] **Step 4: i18n strings ekle ve testleri geçir**

Validation, retry, copy failure, dirty close, loading, modal cancel ve batch
partial result mesajlarını hem `en.json` hem `tr.json` içine ekle. Hardcoded UI
strings kaldırılır.

```bash
rtk npm test
rtk npm run lint
rtk git add src/hooks/useModalA11y.ts src/components/ConfirmDialog.tsx src/components/SettingsModal.tsx src/components/NewDownloadModal.tsx src/components/SchedulerModal.tsx src/components/DownloadItem.tsx src/locales/en.json src/locales/tr.json
rtk git commit -m "fix: make dialogs and forms keyboard-safe"
```

### Task 11: Validated logs/stats, responsive graph and light-theme contrast

**Files:**
- Modify: `src/api/commands.ts`
- Modify: `src/lib/schema.ts`
- Modify: `src/components/LogPanel.tsx`
- Modify: `src/components/StatsPanel.tsx`
- Modify: `src/components/SpeedGraph.tsx`
- Modify: `src/App.css`
- Modify: `src/locales/en.json`
- Modify: `src/locales/tr.json`
- Modify: `src/i18n.ts`
- Test: `src/lib/schema.test.ts`

**Interfaces:**
- `LogEntrySchema` validates `ts`, `level`, `target`, `message`.
- `DownloadStatsSchema` validates non-negative counters and finite speed.
- `SpeedGraph` renders `role="img"` with translated accessible summary.

- [ ] **Step 1: Malformed payload tests yaz**

```ts
it('rejects malformed stats payload', () => {
  expect(() => DownloadStatsSchema.parse({ active: -1 })).toThrow();
});

it('rejects malformed log entry', () => {
  expect(() => LogEntrySchema.parse({ level: 'ERROR' })).toThrow();
});
```

- [ ] **Step 2: API parse katmanı ekle**

`getLogs` `LogArraySchema.parse` kullanır; `getStats` `DownloadStatsSchema.parse`
kullanır. Log/Stats component malformed response’u error state/Retry ile
gösterir, zero-data ile sessizce maskelemez.

- [ ] **Step 3: SpeedGraph accessibility/responsive behavior ekle**

SVG width container’a göre `viewBox` ile ölçeklenir; `prefers-reduced-motion`
ile animated transition kapanır. Accessible summary current speed ve son
sample trend bilgisini translated text olarak verir.

- [ ] **Step 4: CSS contrast ve small-window layout düzelt**

Light theme text/border colors WCAG AA normal text için en az 4.5:1 olur.
Toolbar, batch bar, inspector ve list small desktop width’te overflow yerine
wrap/scroll davranışı kullanır. Focus ring tüm interactive controls’ta görünür.

- [ ] **Step 5: Test ve commit**

```bash
rtk npm test -- src/lib/schema.test.ts
rtk npm run build
rtk git status --short
rtk git add src/api/commands.ts src/lib/schema.ts src/components/LogPanel.tsx src/components/StatsPanel.tsx src/components/SpeedGraph.tsx src/App.css src/locales/en.json src/locales/tr.json src/i18n.ts src/lib/schema.test.ts
rtk git commit -m "feat: harden telemetry panels and accessibility"
```

### Task 12: Final integration verification

**Files:**
- Modify: `README.md`
- Modify: `extension/README.md`
- Test: all repository checks

- [ ] **Step 1: Rust verification**

```bash
rtk cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
rtk cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
rtk cargo test --manifest-path src-tauri/Cargo.toml
```

Expected: Rust format, clippy ve test suite PASS.

- [ ] **Step 2: Frontend verification**

```bash
rtk npm run format:check
rtk npm run lint
rtk npm test
rtk npm run build
```

Expected: Frontend format, lint, tests ve build PASS.

- [ ] **Step 3: Extension verification**

```bash
rtk node extension/smoke-test.mjs
rtk node --check extension/background.js
rtk node --check extension/content.js
```

Expected: Extension manifest/contracts ve syntax PASS.

- [ ] **Step 4: Security regression scan**

```bash
rtk rg -n "window\\.confirm|falconfmt=|suggest\\(\\{ cancel: true \\}\\)" src extension
rtk rg -n "cookies.*DownloadModel|cookies.*DownloadSchema" src
```

Expected: `window.confirm` ve legacy URL format kullanımı yok; frontend
Download model/schema cookie expose etmiyor.

- [ ] **Step 5: Worktree ve handoff kontrolü**

```bash
rtk git diff --check
rtk git status --short
```

Expected: whitespace error yok, yalnız beklenen dokümantasyon/implementation
değişiklikleri var. Tüm faz commit’leri review-ready olarak teslim edilir.
