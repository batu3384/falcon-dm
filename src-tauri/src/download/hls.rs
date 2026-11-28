//! HLS (m3u8) engine for plain MPEG-TS playlists.
//!
//! Segments are fetched in parallel into a per-download temp dir, each written
//! to `*.tmp` and renamed when complete, so an interrupted run never leaves a
//! half segment that a resume would trust. The native remuxer assembles them.
//!
//! Encrypted, fMP4, byte-range and split-audio playlists are handed to yt-dlp
//! (`DownloadError::NeedsYtDlp`) instead of producing a broken file.

use crate::download::http_client::{open, HttpHeaders, Opened, RangeRequest};
use crate::download::job::JobContext;
use crate::download::limiter::{cancelled, sleep_or_cancel};
use crate::download::mux::{self, RemuxError};
use crate::download::DownloadError;
use crate::util::validate_fetch_url_async;
use futures::stream::{self, StreamExt};
use m3u8_rs::{AlternativeMediaType, KeyMethod, MasterPlaylist, MediaPlaylist, Playlist};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use url::Url;

const MAX_PLAYLIST_BYTES: usize = 16 * 1024 * 1024;
const MAX_SEGMENT_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_SEGMENTS: usize = 100_000;
const SEGMENT_ATTEMPTS: u32 = 4;

pub(crate) fn hls_session_dirname(download_id: i64) -> String {
    format!("hls-{download_id}.falcondm-temp")
}

pub(crate) fn hls_session_dir(download_id: i64) -> PathBuf {
    crate::util::app_data_dir().join("downloads_temp").join(hls_session_dirname(download_id))
}

/// Remove the segment cache of a download (remove, or retry of a failed row).
pub fn remove_artifacts(download_id: i64) {
    let _ = std::fs::remove_dir_all(hls_session_dir(download_id));
}

fn segment_name(index: usize) -> String {
    format!("seg_{index:05}.ts")
}

/// Segment names are engine-generated `seg_NNNNN.ts` files. Validate before
/// joining anyway so a bad cache or future caller cannot escape the session dir.
fn segment_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty()
        || name.contains("..")
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
    {
        return Err("Invalid HLS segment name".into());
    }
    Ok(dir.join(name))
}

/// Highest-bandwidth real variant. I-frame-only playlists are trick-play tracks.
fn pick_best_variant(variants: &[m3u8_rs::VariantStream]) -> Option<&m3u8_rs::VariantStream> {
    variants.iter().filter(|v| !v.is_i_frame).max_by_key(|v| v.bandwidth)
}

/// The variant's audio lives in a separate playlist the native path would drop.
fn variant_has_split_audio(master: &MasterPlaylist, variant: &m3u8_rs::VariantStream) -> bool {
    let Some(group) = variant.audio.as_deref() else {
        return false;
    };
    master.alternatives.iter().any(|media| {
        media.media_type == AlternativeMediaType::Audio
            && media.group_id == group
            && media.uri.as_deref().is_some_and(|uri| !uri.trim().is_empty())
    })
}

/// Why the native path cannot assemble this media playlist, if it cannot.
fn unsupported_feature(playlist: &MediaPlaylist) -> Option<&'static str> {
    for segment in &playlist.segments {
        if segment.key.as_ref().is_some_and(|key| key.method != KeyMethod::None) {
            return Some("encrypted segments");
        }
        if segment.map.is_some() {
            return Some("fMP4 init segment");
        }
        if segment.byte_range.is_some() {
            return Some("byte-range segments");
        }
        // Concat stream-copy cannot retimestamp codec/ad breaks correctly.
        if segment.discontinuity {
            return Some("discontinuity");
        }
    }
    None
}

async fn fetch_playlist(
    ctx: &JobContext,
    source: &Url,
    target: &Url,
    headers: &HttpHeaders,
) -> Result<(Vec<u8>, Url), DownloadError> {
    let mut cancel = ctx.cancel.clone();
    let work = async {
        let Opened { response, final_url, .. } =
            open(source, target, headers, &RangeRequest::default(), None).await?;
        let status = response.status();
        if !status.is_success() {
            return Err(DownloadError::from_status(status.as_u16()));
        }
        let mut body = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk
                .map_err(|e| DownloadError::transient(format!("playlist read failed: {e}")))?;
            if body.len() + chunk.len() > MAX_PLAYLIST_BYTES {
                return Err(DownloadError::fatal("HLS playlist is too large"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok((body, final_url))
    };
    tokio::select! {
        result = work => result,
        _ = cancelled(&mut cancel) => Err(DownloadError::Cancelled),
    }
}

struct Plan {
    /// Playlist URL the user gave us: cookies are only sent to its origin.
    source: Url,
    segments: Vec<Url>,
    /// `BANDWIDTH × duration` from the master playlist, when known.
    estimated_bytes: Option<u64>,
    /// Resolved media playlist URL — informational, for the cache manifest.
    media_url: String,
    /// FNV-1a over the ordered, query-stripped segment URIs. Query strings are
    /// excluded on purpose: signed-URL rotation must not invalidate the cache,
    /// while a re-cut or re-laddered playlist (different paths) must.
    fingerprint: String,
}

/// Stable fingerprint of the segment sequence (FNV-1a, length-prefixed URIs).
fn plan_fingerprint(segments: &[Url]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for segment in segments {
        let mut normalized = segment.clone();
        normalized.set_query(None);
        normalized.set_fragment(None);
        let text = normalized.as_str();
        feed(&(text.len() as u64).to_le_bytes());
        feed(text.as_bytes());
    }
    format!("{hash:016x}")
}

async fn resolve_plan(
    ctx: &JobContext,
    url: &str,
    headers: &HttpHeaders,
) -> Result<Plan, DownloadError> {
    let source = validate_fetch_url_async(url).await?;
    let (bytes, base) = fetch_playlist(ctx, &source, &source, headers).await?;
    let parse = |bytes: &[u8]| {
        m3u8_rs::parse_playlist_res(bytes)
            .map_err(|_| DownloadError::fatal("Failed to parse m3u8 playlist"))
    };
    let (media, media_base, bandwidth) = match parse(&bytes)? {
        Playlist::MediaPlaylist(media) => (media, base, None),
        Playlist::MasterPlaylist(master) => {
            let variant = pick_best_variant(&master.variants)
                .ok_or_else(|| DownloadError::fatal("No variants found in master playlist"))?;
            if variant_has_split_audio(&master, variant) {
                log::info!("download {}: HLS audio is a separate rendition → yt-dlp", ctx.id);
                return Err(DownloadError::NeedsYtDlp);
            }
            let variant_url = base
                .join(&variant.uri)
                .map_err(|e| DownloadError::fatal(format!("Invalid variant URL: {e}")))?;
            let variant_url = validate_fetch_url_async(variant_url.as_str()).await?;
            let (bytes, variant_base) = fetch_playlist(ctx, &source, &variant_url, headers).await?;
            match parse(&bytes)? {
                Playlist::MediaPlaylist(media) => (media, variant_base, Some(variant.bandwidth)),
                Playlist::MasterPlaylist(_) => {
                    return Err(DownloadError::fatal("Expected a media playlist in variant"))
                }
            }
        }
    };
    if let Some(feature) = unsupported_feature(&media) {
        log::info!("download {}: HLS playlist has {feature} → yt-dlp", ctx.id);
        return Err(DownloadError::NeedsYtDlp);
    }
    if !media.end_list {
        // A live window would save a few seconds and call it done.
        return Err(DownloadError::fatal("ERR_HLS_LIVE"));
    }
    if media.segments.is_empty() {
        return Err(DownloadError::fatal("No segments found"));
    }
    if media.segments.len() > MAX_SEGMENTS {
        return Err(DownloadError::fatal(format!(
            "Playlist contains too many segments: {}",
            media.segments.len()
        )));
    }
    let duration: f64 = media.segments.iter().map(|s| f64::from(s.duration)).sum();
    let mut segments = Vec::with_capacity(media.segments.len());
    for segment in &media.segments {
        let target = media_base
            .join(&segment.uri)
            .map_err(|e| DownloadError::fatal(format!("Invalid segment URL: {e}")))?;
        if !matches!(target.scheme(), "http" | "https") {
            return Err(DownloadError::fatal("Unsupported segment URL scheme"));
        }
        // Host safety is enforced per request (pinned DNS) when the segment is opened.
        segments.push(target);
    }
    Ok(Plan {
        source,
        fingerprint: plan_fingerprint(&segments),
        media_url: media_base.to_string(),
        segments,
        estimated_bytes: bandwidth
            .filter(|bw| *bw > 0 && duration > 0.0)
            .map(|bw| (bw as f64 / 8.0 * duration) as u64),
    })
}

type HostClient = Option<(String, Client)>;

/// Mutex serialising progress adjustments so a rollback cannot clobber the
/// bytes parallel workers added between its read and its write.
type ProgressLock = Arc<std::sync::Mutex<()>>;

/// Live byte accounting for one segment attempt. Bytes count as they arrive so
/// big segments stream progress; dropping the guard before `commit` rolls back
/// exactly what this attempt added — never what other workers committed.
struct AttemptProgress {
    progress: crate::download::progress::Progress,
    lock: ProgressLock,
    counted: u64,
    committed: bool,
}

impl AttemptProgress {
    fn new(progress: &crate::download::progress::Progress, lock: &ProgressLock) -> Self {
        Self { progress: progress.clone(), lock: lock.clone(), counted: 0, committed: false }
    }

    fn add(&mut self, bytes: u64) {
        let _guard = crate::util::lock_or_recover(&self.lock);
        self.progress.add(bytes);
        self.counted += bytes;
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for AttemptProgress {
    fn drop(&mut self) {
        if !self.committed && self.counted > 0 {
            let _guard = crate::util::lock_or_recover(&self.lock);
            self.progress.set_downloaded(self.progress.downloaded().saturating_sub(self.counted));
        }
    }
}

/// One attempt: stream a segment into `tmp`. Returns the bytes written.
async fn fetch_segment_once(
    ctx: &JobContext,
    source: &Url,
    target: &Url,
    headers: &HttpHeaders,
    client: &mut HostClient,
    tmp: &Path,
    progress_lock: &ProgressLock,
) -> Result<u64, DownloadError> {
    let host = target.host_str().unwrap_or_default().to_string();
    let reuse = client.as_ref().filter(|(h, _)| *h == host).map(|(_, c)| c);
    let Opened { response, final_url, client: used } =
        open(source, target, headers, &RangeRequest::default(), reuse).await?;
    if final_url.host_str() == Some(host.as_str()) {
        *client = Some((host, used));
    }
    let status = response.status();
    if !status.is_success() {
        return Err(DownloadError::from_status(status.as_u16()));
    }
    let expected = response.content_length();
    let mut file = tokio::fs::File::create(tmp)
        .await
        .map_err(|e| DownloadError::fatal(format!("could not create segment file: {e}")))?;
    let mut counted = AttemptProgress::new(&ctx.progress, progress_lock);
    let mut written = 0u64;
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk =
            chunk.map_err(|e| DownloadError::transient(format!("segment read failed: {e}")))?;
        written += chunk.len() as u64;
        if written > MAX_SEGMENT_BYTES {
            return Err(DownloadError::fatal("HLS segment is too large"));
        }
        file.write_all(&chunk).await.map_err(|e| {
            if e.raw_os_error() == Some(28) {
                DownloadError::fatal("ERR_DISK_FULL")
            } else {
                DownloadError::fatal(format!("segment write failed: {e}"))
            }
        })?;
        counted.add(chunk.len() as u64);
        ctx.limiter.acquire(chunk.len() as u64, &ctx.cancel).await?;
    }
    file.flush().await.map_err(|e| DownloadError::fatal(e.to_string()))?;
    // Resume trusts this file across restarts: it must be on disk, not in cache.
    file.sync_all().await.map_err(|e| DownloadError::fatal(format!("segment sync failed: {e}")))?;
    if written == 0 {
        return Err(DownloadError::transient("segment is empty"));
    }
    if expected.is_some_and(|len| len != written) {
        return Err(DownloadError::transient("segment is incomplete"));
    }
    counted.commit();
    Ok(written)
}

/// Write the byte-length sidecar that lets a later run trust the segment.
/// Best effort: without it the segment is simply fetched again on resume.
async fn write_len_sidecar(dir: &Path, index: usize, bytes: u64) {
    let sidecar = dir.join(format!("{}.len", segment_name(index)));
    let written = async {
        let mut file = tokio::fs::File::create(&sidecar).await.ok()?;
        file.write_all(bytes.to_string().as_bytes()).await.ok()?;
        file.sync_all().await.ok()
    };
    if written.await.is_none() {
        log::warn!("could not write the length sidecar of segment {index}");
    }
}

/// Download one segment with retries. The final name only exists once the
/// segment is complete.
async fn fetch_segment(
    ctx: &JobContext,
    plan: &Plan,
    headers: &HttpHeaders,
    client: &mut HostClient,
    index: usize,
    dir: &Path,
    progress_lock: &ProgressLock,
) -> Result<u64, DownloadError> {
    let path = dir.join(segment_name(index));
    let tmp = dir.join(format!("{}.tmp", segment_name(index)));
    let mut cancel = ctx.cancel.clone();
    let mut attempt = 0u32;
    loop {
        ctx.check_cancel()?;
        let result = tokio::select! {
            result = fetch_segment_once(ctx, &plan.source, &plan.segments[index], headers, client, &tmp, progress_lock) => result,
            _ = cancelled(&mut cancel) => Err(DownloadError::Cancelled),
        };
        match result {
            Ok(bytes) => {
                tokio::fs::rename(&tmp, &path)
                    .await
                    .map_err(|e| DownloadError::fatal(format!("segment rename failed: {e}")))?;
                // The sidecar lands after the segment: a crash in between just
                // re-downloads it, never trusts a partial file.
                write_len_sidecar(dir, index, bytes).await;
                return Ok(bytes);
            }
            Err(error) => {
                // Partial bytes of a failed attempt never reached the counter:
                // AttemptProgress rolled back exactly what this attempt added.
                let _ = tokio::fs::remove_file(&tmp).await;
                attempt += 1;
                match error {
                    DownloadError::Transient(_) if attempt < SEGMENT_ATTEMPTS => {
                        sleep_or_cancel(Duration::from_millis(300u64 << attempt), &ctx.cancel)
                            .await?;
                    }
                    DownloadError::Transient(message) => {
                        return Err(DownloadError::Transient(format!(
                            "segment {index} failed after retries: {message}"
                        )));
                    }
                    other => return Err(other),
                }
            }
        }
    }
}

/// What the cache remembers of the playlist the segments were fetched for.
/// Per-segment byte lengths live in `seg_NNNNN.ts.len` sidecars (written once
/// per segment, so parallel workers never serialize on a shared file).
#[derive(serde::Serialize, serde::Deserialize)]
struct CacheManifest {
    version: u32,
    playlist: String,
    fingerprint: String,
    segments: usize,
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}

async fn write_manifest(dir: &Path, plan: &Plan) -> Result<(), DownloadError> {
    let manifest = CacheManifest {
        version: 1,
        playlist: plan.media_url.clone(),
        fingerprint: plan.fingerprint.clone(),
        segments: plan.segments.len(),
    };
    let body = serde_json::to_vec(&manifest)
        .map_err(|e| DownloadError::fatal(format!("could not encode cache manifest: {e}")))?;
    let tmp = dir.join("manifest.json.tmp");
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| DownloadError::fatal(format!("could not write cache manifest: {e}")))?;
    tokio::fs::rename(&tmp, manifest_path(dir))
        .await
        .map_err(|e| DownloadError::fatal(format!("could not write cache manifest: {e}")))?;
    Ok(())
}

/// A cached segment is trusted only when its byte length matches the sidecar
/// written right after it completed — truncated nonzero files are corruption.
async fn trusted_segment_len(dir: &Path, index: usize) -> Option<u64> {
    let name = segment_name(index);
    let meta = tokio::fs::metadata(dir.join(&name)).await.ok()?;
    if meta.len() == 0 {
        return None;
    }
    let sidecar = tokio::fs::read(dir.join(format!("{name}.len"))).await.ok()?;
    let recorded = std::str::from_utf8(&sidecar).ok()?.trim().parse::<u64>().ok()?;
    (recorded == meta.len()).then_some(meta.len())
}

/// Drop anything a previous run left when the playlist no longer matches:
/// mixing segments of two different cuts produces a silently broken file.
async fn validate_cache(dir: &Path, plan: &Plan) -> Result<(), DownloadError> {
    let cached: Option<CacheManifest> = tokio::fs::read(manifest_path(dir))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let matches = cached.as_ref().is_some_and(|manifest| {
        manifest.version == 1
            && manifest.fingerprint == plan.fingerprint
            && manifest.segments == plan.segments.len()
    });
    if matches {
        return Ok(());
    }
    if cached.is_some() {
        log::info!("HLS playlist changed since the cache was written — restarting clean");
    }
    let _ = tokio::fs::remove_dir_all(dir).await;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| DownloadError::fatal(format!("could not create temp folder: {e}")))?;
    write_manifest(dir, plan).await
}

/// Fetch every missing segment. Returns the segment file names in play order.
async fn download_segments(
    ctx: &JobContext,
    plan: &Plan,
    headers: &HttpHeaders,
    dir: &Path,
) -> Result<Vec<String>, DownloadError> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| DownloadError::fatal(format!("could not create temp folder: {e}")))?;
    validate_cache(dir, plan).await?;
    let count = plan.segments.len();
    // Complete segments from an earlier run. `*.tmp` leftovers are never trusted.
    let mut have_bytes = 0u64;
    let mut missing = Vec::new();
    for index in 0..count {
        if let Some(bytes) = trusted_segment_len(dir, index).await {
            have_bytes += bytes;
        } else {
            missing.push(index);
            let name = segment_name(index);
            let _ = tokio::fs::remove_file(dir.join(&name)).await;
            let _ = tokio::fs::remove_file(dir.join(format!("{name}.len"))).await;
        }
    }
    let done_segments = Arc::new(AtomicU64::new((count - missing.len()) as u64));
    let done_bytes = Arc::new(AtomicU64::new(have_bytes));
    let estimate = plan.estimated_bytes.unwrap_or(0);
    ctx.progress.set_downloaded(have_bytes);
    ctx.progress.set_total(estimate.max(have_bytes));
    let connections = headers.max_connections.clamp(1, 16).min(missing.len().max(1));
    ctx.progress.set_connections(connections as u32);

    let queue = Arc::new(std::sync::Mutex::new(
        missing.into_iter().collect::<std::collections::VecDeque<_>>(),
    ));
    let progress_lock: ProgressLock = Arc::new(std::sync::Mutex::new(()));
    let workers = (0..connections).map(|_| {
        let queue = queue.clone();
        let done_segments = done_segments.clone();
        let done_bytes = done_bytes.clone();
        let progress_lock = progress_lock.clone();
        async move {
            let mut client: HostClient = None;
            loop {
                let Some(index) = crate::util::lock_or_recover(&queue).pop_front() else {
                    return Ok::<(), DownloadError>(());
                };
                let bytes =
                    fetch_segment(ctx, plan, headers, &mut client, index, dir, &progress_lock)
                        .await?;
                let segments = done_segments.fetch_add(1, Ordering::Relaxed) + 1;
                let total_bytes = done_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
                // Sizes seen so far project the total better than peak BANDWIDTH.
                let projected = total_bytes / segments * count as u64;
                let blended =
                    if segments * 10 >= count as u64 { projected } else { estimate.max(projected) };
                ctx.progress.set_total(blended.max(ctx.progress.downloaded()));
            }
        }
    });
    // First error wins; dropping the stream stops the other workers.
    let mut running = stream::iter(workers).buffer_unordered(connections);
    while let Some(result) = running.next().await {
        result?;
    }
    drop(running);
    ctx.check_cancel()?;
    let final_bytes = done_bytes.load(Ordering::Relaxed);
    ctx.progress.set_downloaded(final_bytes);
    ctx.progress.set_total(final_bytes);
    Ok((0..count).map(segment_name).collect())
}

fn output_extension(dest: &Path) -> &'static str {
    match dest.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("ts") => "ts",
        _ => "mp4",
    }
}

/// Playlist names and container extensions the native remuxer no longer
/// produces must not mislabel the bytes: `master.m3u8` and `clip.mkv` become
/// MP4 names. Any other name is the user's choice.
fn container_corrected_name(dest: &Path) -> Option<String> {
    let ext = dest.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "m3u8" | "m3u" | "mkv" | "mov") {
        return None;
    }
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
    Some(format!("{stem}.{}", output_extension(dest)))
}

/// Expired signed URLs surface as fatal auth errors on segments. Re-resolving
/// the playlist mints fresh tokens — but only once per run.
fn is_expired_credentials(error: &DownloadError) -> bool {
    matches!(error, DownloadError::Fatal(message) if matches!(message.as_str(), "HTTP 401" | "HTTP 403" | "HTTP 410"))
}

/// Assemble the segments into the selected container. MP4 is remuxed in
/// Rust; an explicit `.ts` destination is a byte-exact concat passthrough.
async fn merge_segments(
    ctx: &JobContext,
    dir: &Path,
    names: &[String],
    output: &Path,
) -> Result<(), DownloadError> {
    let mut inputs = Vec::with_capacity(names.len());
    for name in names {
        inputs.push(segment_path(dir, name).map_err(DownloadError::Fatal)?);
    }
    let ts_passthrough = output.extension().and_then(|e| e.to_str()) == Some("ts");
    let output = output.to_path_buf();
    let cancel = ctx.cancel.clone();
    let result = tokio::task::spawn_blocking(move || {
        let should_cancel = || *cancel.borrow();
        if ts_passthrough {
            mux::concat_ts_files(&inputs, &output, &should_cancel)
        } else {
            mux::remux_ts_files_to_mp4(&inputs, &output, &should_cancel)
        }
    })
    .await
    .map_err(|e| DownloadError::fatal(format!("merge task failed: {e}")))?;
    match result {
        Ok(()) => Ok(()),
        Err(RemuxError::Cancelled) => Err(DownloadError::Cancelled),
        Err(RemuxError::Fatal(message)) => Err(DownloadError::Fatal(message)),
    }
}

/// Download an HLS stream to `out_path`. Returns where the file landed.
pub async fn process_hls_stream(
    ctx: &JobContext,
    url: &str,
    out_path: &str,
    headers: HttpHeaders,
) -> Result<PathBuf, DownloadError> {
    let mut dest = PathBuf::from(out_path);
    let _ticker = ctx.progress.start();
    let mut plan = resolve_plan(ctx, url, &headers).await?;
    let dir = hls_session_dir(ctx.id);
    // Signed segment URLs can expire mid-download. Re-resolving the playlist
    // mints fresh tokens — once per run; cache validation decides whether the
    // re-resolved cut still matches the bytes on disk.
    let mut reresolved = false;
    let names = loop {
        match download_segments(ctx, &plan, &headers, &dir).await {
            Ok(names) => break names,
            Err(error) if !reresolved && is_expired_credentials(&error) => {
                reresolved = true;
                log::info!(
                    "download {}: segment URLs rejected ({}), re-resolving the playlist once",
                    ctx.id,
                    error.message()
                );
                plan = resolve_plan(ctx, url, &headers).await?;
            }
            Err(error) => return Err(error),
        }
    };

    ctx.progress.set_merging();
    let output = dir.join(format!("output.{}", output_extension(&dest)));
    merge_segments(ctx, &dir, &names, &output).await?;
    if let Some(wanted) = container_corrected_name(&dest) {
        dest = ctx.rename(&dest, &wanted);
    }
    let landed = ctx.finalize(&output, &dest).await?;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    Ok(landed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn hls_session_dir_is_per_download() {
        assert_eq!(hls_session_dirname(7), "hls-7.falcondm-temp");
        assert!(hls_session_dir(7).ends_with("downloads_temp/hls-7.falcondm-temp"));
    }

    #[test]
    fn segment_path_rejects_quotes_and_paths() {
        let dir = Path::new("/tmp/falcon-hls");
        assert_eq!(segment_path(dir, "seg_00001.ts").unwrap(), dir.join("seg_00001.ts"));
        assert!(segment_path(dir, "seg_00'01.ts").is_err());
        assert!(segment_path(dir, "../secret.ts").is_err());
        assert!(segment_path(dir, "a/b.ts").is_err());
    }

    fn master(text: &str) -> MasterPlaylist {
        match m3u8_rs::parse_playlist_res(text.as_bytes()).expect("playlist") {
            Playlist::MasterPlaylist(master) => master,
            Playlist::MediaPlaylist(_) => panic!("expected master"),
        }
    }

    fn media(text: &str) -> MediaPlaylist {
        match m3u8_rs::parse_playlist_res(text.as_bytes()).expect("playlist") {
            Playlist::MediaPlaylist(media) => media,
            Playlist::MasterPlaylist(_) => panic!("expected media"),
        }
    }

    #[test]
    fn best_variant_ignores_iframe_tracks() {
        assert!(pick_best_variant(&[]).is_none());
        let pl = master(
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nlow.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2500000\nhigh.m3u8\n\
             #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=9000000,URI=\"iframe.m3u8\"\n",
        );
        let best = pick_best_variant(&pl.variants).expect("variant");
        assert_eq!(best.bandwidth, 2_500_000);
        assert!(best.uri.contains("high.m3u8"));
    }

    #[test]
    fn split_audio_rendition_is_detected() {
        let split = master(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"en\",URI=\"audio.m3u8\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2500000,AUDIO=\"aud\"\nvideo.m3u8\n",
        );
        assert!(variant_has_split_audio(&split, &split.variants[0]));
        // Same group without a URI = audio is muxed into the variant.
        let muxed = master(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"en\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2500000,AUDIO=\"aud\"\nvideo.m3u8\n",
        );
        assert!(!variant_has_split_audio(&muxed, &muxed.variants[0]));
    }

    #[test]
    fn encrypted_fmp4_and_byterange_playlists_are_not_native() {
        let plain = media("#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\na.ts\n#EXT-X-ENDLIST\n");
        assert_eq!(unsupported_feature(&plain), None);
        let encrypted = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.key\"\n\
             #EXTINF:4.0,\na.ts\n#EXT-X-ENDLIST\n",
        );
        assert_eq!(unsupported_feature(&encrypted), Some("encrypted segments"));
        let fmp4 = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n\
             #EXTINF:4.0,\na.m4s\n#EXT-X-ENDLIST\n",
        );
        assert_eq!(unsupported_feature(&fmp4), Some("fMP4 init segment"));
        let ranged = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\n#EXT-X-BYTERANGE:100@0\na.ts\n\
             #EXT-X-ENDLIST\n",
        );
        assert_eq!(unsupported_feature(&ranged), Some("byte-range segments"));
    }

    #[test]
    fn fingerprint_ignores_query_but_not_paths_order_or_count() {
        let urls =
            |list: &[&str]| list.iter().map(|u| Url::parse(u).unwrap()).collect::<Vec<Url>>();
        let a = plan_fingerprint(&urls(&[
            "https://cdn.example.com/s/seg0.ts?tok=A",
            "https://cdn.example.com/s/seg1.ts?tok=A",
        ]));
        let rotated = plan_fingerprint(&urls(&[
            "https://cdn.example.com/s/seg0.ts?tok=B&expires=9",
            "https://cdn.example.com/s/seg1.ts?tok=B&expires=9",
        ]));
        assert_eq!(a, rotated, "signed-URL rotation must not change the fingerprint");
        let recut = plan_fingerprint(&urls(&[
            "https://cdn.example.com/s/seg0.ts?tok=A",
            "https://cdn.example.com/s/other.ts?tok=A",
        ]));
        assert_ne!(a, recut, "different paths are a different cut");
        let fewer = plan_fingerprint(&urls(&["https://cdn.example.com/s/seg0.ts?tok=A"]));
        assert_ne!(a, fewer, "segment count matters");
        let swapped = plan_fingerprint(&urls(&[
            "https://cdn.example.com/s/seg1.ts?tok=A",
            "https://cdn.example.com/s/seg0.ts?tok=A",
        ]));
        assert_ne!(a, swapped, "play order matters");
    }

    #[test]
    fn playlist_names_take_the_container_extension() {
        let corrected = |name: &str| container_corrected_name(Path::new(name));
        assert_eq!(corrected("/dl/master.m3u8").as_deref(), Some("master.mp4"));
        assert_eq!(corrected("/dl/SHOW.M3U8").as_deref(), Some("SHOW.mp4"));
        assert_eq!(corrected("/dl/a.m3u").as_deref(), Some("a.mp4"));
        assert_eq!(corrected("/dl/video.mkv").as_deref(), Some("video.mp4"));
        assert_eq!(corrected("/dl/video.mov").as_deref(), Some("video.mp4"));
        assert_eq!(corrected("/dl/clip.ts"), None);
        assert_eq!(corrected("/dl/plain"), None);
        assert_eq!(corrected("/dl/movie.mp4"), None);
    }

    #[test]
    fn attempt_progress_rolls_back_exactly_what_it_added() {
        let progress = crate::download::progress::Progress::new(None, None, 1);
        let lock: ProgressLock = Arc::new(std::sync::Mutex::new(()));
        let mut a = AttemptProgress::new(&progress, &lock);
        let mut b = AttemptProgress::new(&progress, &lock);
        a.add(100);
        b.add(50);
        assert_eq!(progress.downloaded(), 150);
        // The failed attempt gives back only its own bytes, never the parallel ones.
        drop(a);
        assert_eq!(progress.downloaded(), 50);
        b.commit();
        assert_eq!(progress.downloaded(), 50, "committed bytes stay counted");
    }

    struct Fixture {
        base: String,
        hits: Arc<std::sync::Mutex<HashMap<String, usize>>>,
    }

    /// Static file server. `fail_first` paths answer 503 once.
    async fn serve(files: HashMap<String, Vec<u8>>, fail_first: Vec<String>) -> Fixture {
        use tokio::io::AsyncReadExt;
        let files = Arc::new(files);
        let fail_first = Arc::new(fail_first);
        let hits = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits_c = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let (files, fail_first, hits) = (files.clone(), fail_first.clone(), hits_c.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .split('?')
                        .next()
                        .unwrap_or("/")
                        .to_string();
                    let seen = {
                        let mut hits = hits.lock().unwrap();
                        let entry = hits.entry(path.clone()).or_insert(0);
                        *entry += 1;
                        *entry
                    };
                    if seen == 1 && fail_first.contains(&path) {
                        let _ = sock.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    }
                    let Some(body) = files.get(&path) else {
                        let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(body).await;
                });
            }
        });
        Fixture { base: format!("http://127.0.0.1:{}", addr.port()), hits }
    }

    fn fake_stream() -> HashMap<String, Vec<u8>> {
        let mut files = HashMap::new();
        files.insert(
            "/master.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2500000\nhigh/index.m3u8\n".to_vec(),
        );
        files.insert(
            "/high/index.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nseg0.ts\n#EXTINF:1.0,\nseg1.ts\n#EXTINF:1.0,\nseg2.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        for i in 0..3 {
            files.insert(format!("/high/seg{i}.ts"), format!("SEGMENT-{i}-DATA").into_bytes());
        }
        files
    }

    fn headers(connections: usize) -> HttpHeaders {
        HttpHeaders { max_connections: connections, ..Default::default() }
    }

    #[tokio::test]
    async fn master_playlist_resolves_best_variant_and_fetches_in_order() {
        let fixture = serve(fake_stream(), vec!["/high/seg1.ts".into()]).await;
        let dir = std::env::temp_dir().join(format!("falcon-hls-{}", uuid::Uuid::new_v4()));
        let (ctx, _tx) = JobContext::for_tests(1);
        let _g = crate::util::allow_loopback_for_tests();
        let plan = resolve_plan(&ctx, &format!("{}/master.m3u8", fixture.base), &headers(3))
            .await
            .unwrap();
        assert_eq!(plan.segments.len(), 3);
        assert!(plan.segments[0].as_str().ends_with("/high/seg0.ts"));
        assert_eq!(plan.estimated_bytes, Some(2_500_000 / 8 * 3));

        let names = download_segments(&ctx, &plan, &headers(3), &dir).await.unwrap();
        assert_eq!(names, vec!["seg_00000.ts", "seg_00001.ts", "seg_00002.ts"]);
        let joined: Vec<u8> =
            names.iter().flat_map(|name| std::fs::read(dir.join(name)).unwrap()).collect();
        assert_eq!(joined, b"SEGMENT-0-DATASEGMENT-1-DATASEGMENT-2-DATA");
        // The 503 was retried, and failed bytes were not counted.
        assert_eq!(fixture.hits.lock().unwrap()["/high/seg1.ts"], 2);
        assert_eq!(ctx.progress.downloaded(), joined.len() as u64);
        assert_eq!(ctx.progress.total(), joined.len() as u64);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn resume_skips_complete_segments_and_distrusts_tmp_leftovers() {
        let fixture = serve(fake_stream(), Vec::new()).await;
        let dir = std::env::temp_dir().join(format!("falcon-hls-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (ctx, _tx) = JobContext::for_tests(2);
        let _g = crate::util::allow_loopback_for_tests();
        let plan = resolve_plan(&ctx, &format!("{}/high/index.m3u8", fixture.base), &headers(2))
            .await
            .unwrap();
        // A complete first run leaves the trusted cache: manifest + sidecars.
        download_segments(&ctx, &plan, &headers(2), &dir).await.unwrap();
        // Lose one segment (with its sidecar) and drop a crash leftover instead.
        std::fs::remove_file(dir.join("seg_00001.ts")).unwrap();
        std::fs::remove_file(dir.join("seg_00001.ts.len")).unwrap();
        std::fs::write(dir.join("seg_00001.ts.tmp"), b"SEGM").unwrap();
        let (ctx2, _tx2) = JobContext::for_tests(5);
        download_segments(&ctx2, &plan, &headers(2), &dir).await.unwrap();
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(hits["/high/seg0.ts"], 1, "finished segment must not be re-fetched");
        assert_eq!(hits["/high/seg1.ts"], 2, "the lost segment is fetched exactly once more");
        assert_eq!(hits["/high/seg2.ts"], 1, "finished segment must not be re-fetched");
        assert_eq!(std::fs::read(dir.join("seg_00001.ts")).unwrap(), b"SEGMENT-1-DATA");
        assert!(!dir.join("seg_00001.ts.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn missing_segment_fails_without_leaving_a_trusted_file() {
        let mut files = fake_stream();
        files.remove("/high/seg2.ts");
        let fixture = serve(files, Vec::new()).await;
        let dir = std::env::temp_dir().join(format!("falcon-hls-{}", uuid::Uuid::new_v4()));
        let (ctx, _tx) = JobContext::for_tests(3);
        let _g = crate::util::allow_loopback_for_tests();
        let plan = resolve_plan(&ctx, &format!("{}/high/index.m3u8", fixture.base), &headers(1))
            .await
            .unwrap();
        let result = download_segments(&ctx, &plan, &headers(1), &dir).await;
        assert!(matches!(result, Err(DownloadError::Fatal(_))), "{result:?}");
        assert!(!dir.join("seg_00002.ts").exists());
        assert!(!dir.join("seg_00002.ts.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn live_and_encrypted_playlists_are_refused_or_delegated() {
        let mut files = HashMap::new();
        files.insert(
            "/live.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\na.ts\n".to_vec(),
        );
        files.insert(
            "/enc.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\n#EXTINF:2.0,\na.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        let fixture = serve(files, Vec::new()).await;
        let (ctx, _tx) = JobContext::for_tests(4);
        let _g = crate::util::allow_loopback_for_tests();
        let live = resolve_plan(&ctx, &format!("{}/live.m3u8", fixture.base), &headers(1)).await;
        assert!(matches!(live, Err(DownloadError::Fatal(ref m)) if m == "ERR_HLS_LIVE"));
        let enc = resolve_plan(&ctx, &format!("{}/enc.m3u8", fixture.base), &headers(1)).await;
        assert!(matches!(enc, Err(DownloadError::NeedsYtDlp)));
    }

    /// Static file server whose contents the test can mutate mid-flight.
    async fn serve_mutable(
        files: HashMap<String, Vec<u8>>,
    ) -> (Fixture, Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>) {
        use tokio::io::AsyncReadExt;
        let files = Arc::new(std::sync::Mutex::new(files));
        let hits = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits_c = hits.clone();
        let files_c = files.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let (files, hits) = (files_c.clone(), hits_c.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .split('?')
                        .next()
                        .unwrap_or("/")
                        .to_string();
                    {
                        let mut hits = hits.lock().unwrap();
                        *hits.entry(path.clone()).or_insert(0) += 1;
                    }
                    let body = files.lock().unwrap().get(&path).cloned();
                    let Some(body) = body else {
                        let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                });
            }
        });
        (Fixture { base: format!("http://127.0.0.1:{}", addr.port()), hits }, files)
    }

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("falcon-hls-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A cache written for one playlist must never mix with a re-cut one.
    #[tokio::test]
    async fn mutated_playlist_invalidates_cache() {
        let mut files = HashMap::new();
        files.insert(
            "/index.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nseg0.ts\n#EXTINF:1.0,\nseg1.ts\n#EXTINF:1.0,\nseg2.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        for i in 0..3 {
            files.insert(format!("/seg{i}.ts"), format!("SEGMENT-{i}-DATA").into_bytes());
        }
        let (fixture, files) = serve_mutable(files).await;
        let dir = fresh_dir("mutate");
        let _g = crate::util::allow_loopback_for_tests();
        let (ctx, _tx) = JobContext::for_tests(20);
        let plan =
            resolve_plan(&ctx, &format!("{}/index.m3u8", fixture.base), &headers(2)).await.unwrap();
        download_segments(&ctx, &plan, &headers(2), &dir).await.unwrap();

        // The CDN re-cuts the stream: new segment names, one segment less.
        {
            let mut files = files.lock().unwrap();
            files.insert(
                "/index.m3u8".to_string(),
                b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nnew0.ts\n#EXTINF:1.0,\nnew1.ts\n#EXT-X-ENDLIST\n".to_vec(),
            );
            files.insert("/new0.ts".to_string(), b"NEW-0-DATA".to_vec());
            files.insert("/new1.ts".to_string(), b"NEW-1-DATA".to_vec());
        }
        let (ctx2, _tx2) = JobContext::for_tests(21);
        let plan2 = resolve_plan(&ctx2, &format!("{}/index.m3u8", fixture.base), &headers(2))
            .await
            .unwrap();
        let names = download_segments(&ctx2, &plan2, &headers(2), &dir).await.unwrap();
        assert_eq!(names.len(), 2);
        let joined: Vec<u8> =
            names.iter().flat_map(|name| std::fs::read(dir.join(name)).unwrap()).collect();
        assert_eq!(joined, b"NEW-0-DATANEW-1-DATA", "no stale bytes may survive a re-cut");
        assert!(!dir.join("seg_00002.ts").exists(), "orphaned segment of the old cut is removed");
        assert!(!dir.join("seg_00003.ts").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Resume must trust only byte-exact segments; a truncated nonzero file is
    /// fetched again instead of silently corrupting the merge.
    #[tokio::test]
    async fn resume_detects_truncated_segment_and_keeps_valid_cache() {
        let fixture = serve(fake_stream(), Vec::new()).await;
        let dir = fresh_dir("trunc");
        let _g = crate::util::allow_loopback_for_tests();
        let (ctx, _tx) = JobContext::for_tests(22);
        let plan = resolve_plan(&ctx, &format!("{}/high/index.m3u8", fixture.base), &headers(2))
            .await
            .unwrap();
        download_segments(&ctx, &plan, &headers(2), &dir).await.unwrap();
        assert_eq!(fixture.hits.lock().unwrap()["/high/seg0.ts"], 1);

        // Power loss after the rename: the file is nonzero but short.
        let path = dir.join("seg_00001.ts");
        let data = std::fs::read(&path).unwrap();
        std::fs::write(&path, &data[..data.len() - 3]).unwrap();

        let (ctx2, _tx2) = JobContext::for_tests(23);
        download_segments(&ctx2, &plan, &headers(2), &dir).await.unwrap();
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(hits["/high/seg0.ts"], 1, "intact segment stays cached");
        assert_eq!(hits["/high/seg2.ts"], 1, "intact segment stays cached");
        assert_eq!(hits["/high/seg1.ts"], 2, "truncated segment is fetched again");
        assert_eq!(std::fs::read(dir.join("seg_00001.ts")).unwrap(), b"SEGMENT-1-DATA");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn discontinuity_playlist_defers_to_ytdlp() {
        let discontinuity = media(
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4.0,\na.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:4.0,\nb.ts\n#EXT-X-ENDLIST\n",
        );
        assert_eq!(unsupported_feature(&discontinuity), Some("discontinuity"));
        let mut files = HashMap::new();
        files.insert(
            "/disc.m3u8".to_string(),
            b"#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\na.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:2.0,\nb.ts\n#EXT-X-ENDLIST\n".to_vec(),
        );
        let fixture = serve(files, Vec::new()).await;
        let (ctx, _tx) = JobContext::for_tests(24);
        let _g = crate::util::allow_loopback_for_tests();
        let result = resolve_plan(&ctx, &format!("{}/disc.m3u8", fixture.base), &headers(1)).await;
        assert!(matches!(result, Err(DownloadError::NeedsYtDlp)), "expected NeedsYtDlp");
    }

    /// ffmpeg `-f hls` output of a few seconds, or None when ffmpeg is missing.
    fn make_hls_parts(src: &Path) -> Option<usize> {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls");
        for name in ["index.m3u8", "part0.ts", "part1.ts", "part2.ts"] {
            std::fs::copy(fixture.join(name), src.join(name)).unwrap();
        }
        Some(3)
    }

    fn load_parts(src: &Path) -> HashMap<String, Vec<u8>> {
        let mut files = HashMap::new();
        for entry in std::fs::read_dir(src).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            files.insert(format!("/{name}"), std::fs::read(entry.path()).unwrap());
        }
        files
    }

    /// Signed-URL server: the token handed out in the playlist expires after
    /// the first segment it protects. The next playlist fetch mints a fresh
    /// token (and optionally re-cut segment names).
    struct Rotating {
        base: String,
        hits: Arc<std::sync::Mutex<HashMap<String, usize>>>,
    }

    async fn serve_signed_segments(segments: Vec<Vec<u8>>, recut: bool) -> Rotating {
        use tokio::io::AsyncReadExt;
        let hits = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let playlist_fetches = Arc::new(AtomicU64::new(0));
        let token_a_dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits_c = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let (hits, fetches, dead, segments) = (
                    hits_c.clone(),
                    playlist_fetches.clone(),
                    token_a_dead.clone(),
                    segments.clone(),
                );
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let target = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    {
                        let mut hits = hits.lock().unwrap();
                        *hits.entry(target.clone()).or_insert(0) += 1;
                    }
                    let path = target.split('?').next().unwrap_or("/");
                    let token =
                        target.split('?').nth(1).and_then(|q| q.strip_prefix("tok=")).unwrap_or("");
                    let reply = |status: &str, body: &[u8]| {
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes()
                        .into_iter()
                        .chain(body.iter().copied())
                        .collect::<Vec<u8>>()
                    };
                    if path == "/index.m3u8" {
                        let fetch = fetches.fetch_add(1, Ordering::Relaxed) + 1;
                        let (tok, prefix) = if fetch == 1 {
                            ("A", "seg")
                        } else if recut {
                            ("B", "cut")
                        } else {
                            ("B", "seg")
                        };
                        let body = format!(
                            "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\n{prefix}0.ts?tok={tok}\n#EXTINF:1.0,\n{prefix}1.ts?tok={tok}\n#EXTINF:1.0,\n{prefix}2.ts?tok={tok}\n#EXT-X-ENDLIST\n"
                        );
                        let _ = sock.write_all(&reply("200 OK", body.as_bytes())).await;
                        return;
                    }
                    let index = path
                        .trim_start_matches('/')
                        .trim_start_matches("seg")
                        .trim_start_matches("cut")
                        .trim_end_matches(".ts")
                        .to_string();
                    let Ok(index) = index.parse::<usize>() else {
                        let _ = sock.write_all(&reply("404 Not Found", b"")).await;
                        return;
                    };
                    let Some(segment) = segments.get(index) else {
                        let _ = sock.write_all(&reply("404 Not Found", b"")).await;
                        return;
                    };
                    let forbidden = match token {
                        "A" if dead.load(Ordering::Relaxed) => true,
                        "A" => {
                            // The first successful segment burns the token.
                            dead.store(true, Ordering::Relaxed);
                            false
                        }
                        "B" => false,
                        _ => true,
                    };
                    if forbidden {
                        let _ = sock.write_all(&reply("403 Forbidden", b"")).await;
                    } else {
                        let _ = sock.write_all(&reply("200 OK", segment)).await;
                    }
                });
            }
        });
        Rotating { base: format!("http://127.0.0.1:{}", addr.port()), hits }
    }

    fn signed_fixture_parts() -> (PathBuf, Vec<Vec<u8>>) {
        let src = std::env::temp_dir().join(format!("falcon-hls-src-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&src).unwrap();
        assert_eq!(make_hls_parts(&src), Some(3));
        let bytes =
            (0..3).map(|i| std::fs::read(src.join(format!("part{i}.ts"))).unwrap()).collect();
        (src, bytes)
    }

    /// Signed URLs expire mid-download: the engine re-resolves the playlist
    /// once and continues with fresh URLs, reusing the valid cache.
    #[tokio::test]
    async fn expired_signed_urls_reresolve_once_and_keep_cache() {
        let (src, segment) = signed_fixture_parts();
        let fixture = serve_signed_segments(segment, false).await;
        let out_dir = fresh_dir("signed-out");
        let dest = out_dir.join("clip.mp4");
        let id = 910_000 + i64::from(std::process::id());
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(1),
        )
        .await
        .unwrap();
        assert_eq!(landed, dest.canonicalize().unwrap());
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(hits["/seg0.ts?tok=A"], 1, "first segment succeeds before expiry");
        assert_eq!(hits["/seg1.ts?tok=A"], 1, "the expired token is rejected once");
        assert_eq!(hits["/seg1.ts?tok=B"], 1, "fresh token fetches the rest");
        assert_eq!(hits["/seg2.ts?tok=B"], 1);
        assert!(
            !hits.contains_key("/seg0.ts?tok=B"),
            "a cache-valid segment is not fetched again after re-signing"
        );
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(out_dir);
        remove_artifacts(id);
    }

    /// Server whose segments never authorize: every request is a 403, however
    /// fresh the token. Guards the re-resolve loop.
    async fn serve_always_forbidden() -> Rotating {
        use tokio::io::AsyncReadExt;
        let hits = Arc::new(std::sync::Mutex::new(HashMap::<String, usize>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits_c = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let hits = hits_c.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let target = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    {
                        let mut hits = hits.lock().unwrap();
                        *hits.entry(target.clone()).or_insert(0) += 1;
                    }
                    let path = target.split('?').next().unwrap_or("/");
                    if path == "/index.m3u8" {
                        let body = b"#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nseg0.ts\n#EXT-X-ENDLIST\n";
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                        let _ = sock.write_all(body).await;
                    } else {
                        let _ = sock
                            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .await;
                    }
                });
            }
        });
        Rotating { base: format!("http://127.0.0.1:{}", addr.port()), hits }
    }

    /// One re-resolve per run: permanently forbidden segments end fatal,
    /// not in a loop of playlist fetches.
    #[tokio::test]
    async fn permanently_forbidden_segments_fail_after_one_reresolve() {
        let fixture = serve_always_forbidden().await;
        let out_dir = fresh_dir("forbidden-out");
        let dest = out_dir.join("clip.mp4");
        let id = 940_000 + i64::from(std::process::id());
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let result = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(1),
        )
        .await;
        assert!(
            matches!(result, Err(DownloadError::Fatal(ref m)) if m == "HTTP 403"),
            "{result:?}"
        );
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(
            hits["/index.m3u8"], 2,
            "exactly one re-resolution: the initial fetch plus one retry"
        );
        let _ = std::fs::remove_dir_all(out_dir);
        remove_artifacts(id);
    }

    /// When the re-resolved playlist is a different cut, the cache cannot be
    /// trusted: everything restarts against the new segments.
    #[tokio::test]
    async fn reresolve_with_recut_playlist_restarts_clean() {
        let (src, segment) = signed_fixture_parts();
        let fixture = serve_signed_segments(segment, true).await;
        let out_dir = fresh_dir("signed-recut");
        let dest = out_dir.join("clip.mp4");
        let id = 920_000 + i64::from(std::process::id());
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(1),
        )
        .await
        .unwrap();
        let hits = fixture.hits.lock().unwrap().clone();
        assert_eq!(hits["/seg0.ts?tok=A"], 1);
        for i in 0..3 {
            assert_eq!(
                hits[&format!("/cut{i}.ts?tok=B")],
                1,
                "re-cut segments are all fetched once"
            );
        }
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(out_dir);
        remove_artifacts(id);
    }

    /// A `.m3u8` output name is corrected to the container ffmpeg produced.
    #[tokio::test]
    async fn m3u8_destination_is_renamed_to_the_container() {
        let src = std::env::temp_dir().join(format!("falcon-hls-src-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&src).unwrap();
        assert_eq!(make_hls_parts(&src), Some(3));
        let fixture = serve(load_parts(&src), Vec::new()).await;
        let out_dir = fresh_dir("rename-out");
        let dest = out_dir.join("master.m3u8");
        let id = 930_000 + i64::from(std::process::id());
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(3),
        )
        .await
        .unwrap();
        assert_eq!(
            landed.file_name().and_then(|n| n.to_str()),
            Some("master.mp4"),
            "the container extension wins over the playlist name"
        );
        let bytes = std::fs::read(&landed).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp", "output must be an MP4 container");
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(out_dir);
        remove_artifacts(id);
    }

    #[tokio::test]
    async fn failed_merge_deletes_partial_output() {
        let dir = fresh_dir("failmerge");
        let name = segment_name(0);
        std::fs::write(dir.join(&name), b"not an mpeg-ts stream").unwrap();
        let (ctx, _tx) = JobContext::for_tests(31);
        let output = dir.join("output.mp4");
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            merge_segments(&ctx, &dir, &[name], &output),
        )
        .await
        .expect("merge must not hang");
        assert!(result.is_err(), "invalid input fails the merge");
        assert!(!output.exists(), "partial output is removed after a failed merge");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn cancelled_merge_leaves_no_partial_output() {
        let dir = fresh_dir("cancelmerge");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls/part0.ts");
        let name = segment_name(0);
        std::fs::copy(source, dir.join(&name)).unwrap();
        let (ctx, tx) = JobContext::for_tests(32);
        tx.send(true).unwrap();
        let output = dir.join("output.mp4");
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            merge_segments(&ctx, &dir, &[name], &output),
        )
        .await
        .expect("cancelled merge must not hang");
        assert!(matches!(result, Err(DownloadError::Cancelled)), "{result:?}");
        assert!(!output.exists(), "a cancelled merge leaves no partial output");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn ts_output_is_concat_passthrough() {
        let dir = fresh_dir("tsmerge");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/hls");
        let mut names = Vec::new();
        let mut expected = Vec::new();
        for i in 0..2 {
            let bytes = std::fs::read(fixture.join(format!("part{i}.ts"))).unwrap();
            expected.extend_from_slice(&bytes);
            let name = segment_name(i);
            std::fs::write(dir.join(&name), bytes).unwrap();
            names.push(name);
        }
        let (ctx, _tx) = JobContext::for_tests(33);
        let output = dir.join("output.ts");
        merge_segments(&ctx, &dir, &names, &output).await.unwrap();
        assert_eq!(std::fs::read(output).unwrap(), expected);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Real MPEG-TS segments → one playable MP4 without any external tool.
    #[tokio::test]
    async fn end_to_end_remux_produces_an_mp4() {
        let src = std::env::temp_dir().join(format!("falcon-hls-src-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&src).unwrap();
        assert_eq!(make_hls_parts(&src), Some(3));
        let fixture = serve(load_parts(&src), Vec::new()).await;

        let out_dir = std::env::temp_dir().join(format!("falcon-hls-out-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&out_dir).unwrap();
        let dest = out_dir.join("clip.mp4");
        let id = 900_000 + i64::from(std::process::id());
        remove_artifacts(id);
        let (ctx, _tx) = JobContext::for_tests(id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_hls_stream(
            &ctx,
            &format!("{}/index.m3u8", fixture.base),
            dest.to_str().unwrap(),
            headers(3),
        )
        .await
        .unwrap();
        assert_eq!(landed, dest.canonicalize().unwrap());
        let bytes = std::fs::read(&dest).unwrap();
        assert!(bytes.len() > 1000);
        assert_eq!(&bytes[4..8], b"ftyp", "output must be an MP4 container");
        assert!(!hls_session_dir(id).exists(), "segment cache is removed after success");
        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(out_dir);
    }
}
