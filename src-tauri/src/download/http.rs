//! HTTP(S) engine.
//!
//! One `.part` file is preallocated to the final size and every connection
//! writes its byte range at the right offset. Per-range progress is persisted
//! next to it, so pause, crash, retry and app restart all continue where each
//! connection stopped instead of starting over.
//!
//! The file is split into one range per connection. A connection that runs out
//! of work takes the second half of the largest unfinished range, so a slow
//! connection never holds the download back and the request count stays low
//! (servers rate-limit clients that fire dozens of small range requests).

use crate::download::http_client::{
    content_disposition_filename, entity_validator, extension_for_content_type, open,
    parse_content_range, HttpHeaders, Opened, RangeRequest,
};
use crate::download::job::JobContext;
use crate::download::limiter::{cancelled, sleep_or_cancel};
use crate::download::DownloadError;
use crate::util::{
    free_disk_space, is_generic_download_filename, lock_or_recover, same_file_inode,
    validate_completed_file_named_with_hints, validate_fetch_url_async, FinalizeHints,
};
use futures::StreamExt;
use reqwest::header::{HeaderMap, CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use url::Url;

/// Below this a second connection costs more than it saves.
const MIN_SEGMENT_BYTES: u64 = 1024 * 1024;
/// An idle connection only splits a range when each half keeps this much.
const MIN_STEAL_BYTES: u64 = 4 * 1024 * 1024;
/// Buffered bytes per connection before one positioned write.
const FLUSH_BYTES: usize = 512 * 1024;
/// Consecutive failures without progress before a connection gives up.
const SEGMENT_MAX_FAILURES: u32 = 4;
const STATE_SAVE_EVERY: Duration = Duration::from_secs(3);

#[cfg(test)]
thread_local! {
    /// Test hook invoked inside every persist, cloned on the runtime thread
    /// before the blocking task spawns. Lets a test imitate a slow disk.
    static PERSIST_TEST_HOOK: std::cell::RefCell<Option<Arc<dyn Fn() + Send + Sync>>> =
        const { std::cell::RefCell::new(None) };
    /// Test override for the periodic save interval (milliseconds, 0 = off).
    static STATE_SAVE_EVERY_TEST_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn state_save_every() -> Duration {
    #[cfg(test)]
    {
        let ms = STATE_SAVE_EVERY_TEST_MS.with(|c| c.get());
        if ms > 0 {
            return Duration::from_millis(ms);
        }
    }
    STATE_SAVE_EVERY
}
const STATE_VERSION: u32 = 1;
/// Head-room kept free on the volume.
const DISK_MARGIN_BYTES: u64 = 64 * 1024 * 1024;
const REMOTE_CHANGED: &str = "ERR_REMOTE_CHANGED";
/// Stable codes the UI localizes; the English sentence lives in the locales.
const PARTIAL_RESPONSE: &str = "ERR_PARTIAL_RESPONSE";
const CONTENT_ENCODING: &str = "ERR_CONTENT_ENCODING";

fn hidden_sibling(destination: &Path, download_id: i64, suffix: &str) -> Option<PathBuf> {
    let parent = destination.parent()?;
    let name = destination.file_name()?.to_str()?;
    Some(parent.join(format!(".{name}.{download_id}.falcon.{suffix}")))
}

pub fn part_path_for(save_dir: &str, filename: &str, download_id: i64) -> Option<PathBuf> {
    hidden_sibling(&crate::util::full_file_path(save_dir, filename), download_id, "part")
}

pub fn state_path_for(save_dir: &str, filename: &str, download_id: i64) -> Option<PathBuf> {
    hidden_sibling(&crate::util::full_file_path(save_dir, filename), download_id, "state")
}

/// Delete every temp artifact of a download (remove, or cleanup of orphans).
pub fn remove_artifacts(save_dir: &str, filename: &str, download_id: i64) {
    let dest = crate::util::full_file_path(save_dir, filename);
    for suffix in ["part", "state"] {
        if let Some(path) = hidden_sibling(&dest, download_id, suffix) {
            let _ = std::fs::remove_file(path);
        }
    }
    // Directory written by the pre-offset engine.
    if let Some(path) = hidden_sibling(&dest, download_id, "segments") {
        let _ = std::fs::remove_dir_all(path);
    }
}

#[derive(Serialize, Deserialize)]
struct SavedState {
    version: u32,
    url: String,
    total: u64,
    validator: Option<String>,
    /// `[start, end_inclusive, done]`
    segments: Vec<[u64; 3]>,
}

struct Span {
    /// Inclusive. Shrinks when another connection takes the tail of this range.
    end: u64,
    /// Bytes accepted from the network (buffered or written).
    received: u64,
}

struct Segment {
    start: u64,
    span: Mutex<Span>,
    /// Bytes of this range already on disk.
    done: AtomicU64,
}

impl Segment {
    fn new(start: u64, end: u64, done: u64) -> Self {
        Self { start, span: Mutex::new(Span { end, received: done }), done: AtomicU64::new(done) }
    }

    fn end(&self) -> u64 {
        lock_or_recover(&self.span).end
    }

    fn complete(&self) -> bool {
        self.start + self.done.load(Ordering::Relaxed) > self.end()
    }

    /// Bytes still to come from the network.
    fn unreceived(&self) -> u64 {
        let span = lock_or_recover(&self.span);
        (span.end + 1).saturating_sub(self.start + span.received)
    }

    /// Reserve up to `len` bytes of an arriving chunk for this range.
    fn claim(&self, len: usize) -> usize {
        let mut span = lock_or_recover(&self.span);
        let left = (span.end + 1).saturating_sub(self.start + span.received);
        let take = (len as u64).min(left);
        span.received += take;
        take as usize
    }

    /// A new request continues from what is on disk.
    fn rewind_to_disk(&self) -> u64 {
        let done = self.done.load(Ordering::Relaxed);
        lock_or_recover(&self.span).received = done;
        self.start + done
    }
}

/// One range per connection, tiling `[0, total)`.
pub(crate) fn plan_segments(total: u64, connections: usize) -> Vec<(u64, u64)> {
    if total == 0 {
        return Vec::new();
    }
    let count = (connections.clamp(1, 16) as u64).min((total / MIN_SEGMENT_BYTES).max(1));
    let size = total / count;
    (0..count)
        .map(|index| {
            let end = if index + 1 == count { total - 1 } else { (index + 1) * size - 1 };
            (index * size, end)
        })
        .collect()
}

/// Parse and validate a state file: right version, same download, and ranges
/// tiling `[0, total)` exactly. Says nothing about the `.part` file itself.
fn read_state(path: &Path, url: &str) -> Option<SavedState> {
    let state: SavedState = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let mut tiles: Vec<[u64; 3]> = state.segments.clone();
    tiles.sort_by_key(|tile| tile[0]);
    let mut cursor = 0u64;
    for [start, end, done] in &tiles {
        // Ranges must tile [0, total) exactly; anything else is foreign or damaged.
        if *start != cursor || end < start || *done > end - start + 1 {
            return None;
        }
        cursor = end + 1;
    }
    (state.version == STATE_VERSION && state.url == url && state.total > 0 && cursor == state.total)
        .then_some(state)
}

fn load_state(path: &Path, url: &str, part: &Path) -> Option<SavedState> {
    let part_len = std::fs::metadata(part).ok()?.len();
    let state = read_state(path, url)?;
    (part_len == state.total).then_some(state)
}

/// A valid state whose every range is complete, without requiring the `.part`
/// file: proof that a crashed finalize had already delivered all bytes.
fn load_complete_state_without_part(path: &Path, url: &str) -> Option<SavedState> {
    let state = read_state(path, url)?;
    state.segments.iter().all(|[start, end, done]| *done == end - start + 1).then_some(state)
}

fn save_state(path: &Path, state: &SavedState) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_extension("state.tmp");
    let bytes = serde_json::to_vec(state).map_err(|e| e.to_string())?;
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        // Durable before the rename, so a crash never leaves a half-written state.
        file.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn io_error(context: &str, error: &std::io::Error) -> DownloadError {
    // ENOSPC
    if error.raw_os_error() == Some(28) {
        return DownloadError::fatal("ERR_DISK_FULL");
    }
    DownloadError::fatal(format!("{context}: {error}"))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
    context: &'static str,
) -> Result<T, DownloadError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| DownloadError::fatal(format!("{context}: task failed: {e}")))?
        .map_err(|e| io_error(context, &e))
}

/// Buffers one connection's bytes and writes them at their file offset.
struct OffsetWriter {
    file: Arc<std::fs::File>,
    offset: u64,
    buffer: Vec<u8>,
}

impl OffsetWriter {
    fn new(file: Arc<std::fs::File>, offset: u64) -> Self {
        Self { file, offset, buffer: Vec::with_capacity(FLUSH_BYTES) }
    }

    /// Returns how many bytes reached the file.
    async fn flush(&mut self) -> Result<u64, DownloadError> {
        if self.buffer.is_empty() {
            return Ok(0);
        }
        let file = self.file.clone();
        let offset = self.offset;
        let data = std::mem::take(&mut self.buffer);
        let mut data = blocking(
            move || {
                use std::os::unix::fs::FileExt;
                file.write_all_at(&data, offset).map(|()| data)
            },
            "write failed",
        )
        .await?;
        let written = data.len() as u64;
        self.offset += written;
        data.clear();
        self.buffer = data;
        Ok(written)
    }
}

struct Shared {
    initial: Url,
    final_url: Mutex<Url>,
    headers: HttpHeaders,
    validator: Option<String>,
    total: u64,
    file: Arc<std::fs::File>,
    /// Every range, including those created by splitting. Only grows.
    segments: Mutex<Vec<Arc<Segment>>>,
    /// Ranges nobody is working on.
    pending: Mutex<VecDeque<Arc<Segment>>>,
    /// A connection hit an error that makes continuing pointless for all.
    abort: watch::Sender<bool>,
    /// The entity changed or ranges are no longer honored: partial data is void.
    restart: AtomicBool,
}

impl Shared {
    fn snapshot(&self) -> SavedState {
        let segments = lock_or_recover(&self.segments);
        SavedState {
            version: STATE_VERSION,
            url: self.initial.to_string(),
            total: self.total,
            validator: self.validator.clone(),
            segments: segments
                .iter()
                .map(|seg| [seg.start, seg.end(), seg.done.load(Ordering::Relaxed)])
                .collect(),
        }
    }

    fn complete(&self) -> bool {
        lock_or_recover(&self.segments).iter().all(|seg| seg.complete())
    }

    /// Split the range with the most bytes still to come and return its tail.
    fn steal(&self) -> Option<Arc<Segment>> {
        // Holding the list lock keeps snapshots from seeing a half-done split.
        let mut segments = lock_or_recover(&self.segments);
        let victim = segments.iter().max_by_key(|seg| seg.unreceived())?.clone();
        let mut span = lock_or_recover(&victim.span);
        let position = victim.start + span.received;
        let left = (span.end + 1).saturating_sub(position);
        if left < 2 * MIN_STEAL_BYTES {
            return None;
        }
        let middle = position + left / 2;
        let tail = Arc::new(Segment::new(middle, span.end, 0));
        span.end = middle - 1;
        drop(span);
        segments.push(tail.clone());
        Some(tail)
    }

    /// Start a state save in the blocking pool. Callers decide when to await
    /// the handle — the periodic saver must never pause the worker select loop.
    fn spawn_persist(
        &self,
        state_path: &Path,
    ) -> tokio::task::JoinHandle<Result<(), DownloadError>> {
        let file = self.file.clone();
        // Snapshot first, then sync: the state never claims bytes the disk lacks.
        let snapshot = self.snapshot();
        let path = state_path.to_path_buf();
        #[cfg(test)]
        let hook = PERSIST_TEST_HOOK.with(|h| h.borrow().clone());
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if let Some(hook) = hook {
                hook();
            }
            // A failed sync means the state file would claim bytes the disk may
            // not have. Skip the save; the error ends the download instead.
            file.sync_data().map_err(|e| io_error("sync failed", &e))?;
            save_state(&path, &snapshot).map_err(DownloadError::fatal)
        })
    }

    async fn persist(&self, state_path: &Path) -> Result<(), DownloadError> {
        self.spawn_persist(state_path)
            .await
            .map_err(|e| DownloadError::fatal(format!("persist task failed: {e}")))?
    }
}

fn backoff(failures: u32) -> Duration {
    Duration::from_millis(500u64 << failures.min(5))
}

/// `Retry-After: <seconds>` on 429/503, capped so a hostile value cannot park us.
fn retry_after(response: &Response) -> Option<Duration> {
    if !matches!(response.status().as_u16(), 429 | 503) {
        return None;
    }
    let seconds: u64 = response.headers().get(RETRY_AFTER)?.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(seconds.clamp(1, 30)))
}

/// Requests ask for `identity` only. A server that compresses anyway breaks
/// the byte-range math (and even a plain stream would store gzip bytes under
/// the original name), so its responses are unusable for a download manager.
fn check_content_encoding(headers: &HeaderMap) -> Result<(), DownloadError> {
    let Some(value) = headers.get(reqwest::header::CONTENT_ENCODING) else {
        return Ok(());
    };
    let Ok(encoded) = value.to_str() else {
        return Err(DownloadError::fatal(CONTENT_ENCODING));
    };
    let encoded = encoded.trim();
    if encoded.is_empty() || encoded.eq_ignore_ascii_case("identity") {
        return Ok(());
    }
    Err(DownloadError::fatal(CONTENT_ENCODING))
}

/// The response labels itself as HTML: a strong hint that a small "file" is
/// really an error page, even when its size technically matches.
fn response_is_html(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/html"))
}

/// Does this response carry the bytes starting at `pos` of the same entity?
/// `validator` is the entity tag the partial file was downloaded under: a
/// server may legally ignore If-Range, so the response's own validator is the
/// only proof the entity did not change underneath us.
fn check_range_response(
    response: &Response,
    pos: u64,
    total: u64,
    validator: Option<&str>,
) -> Result<(), DownloadError> {
    let status = response.status();
    if status == StatusCode::PARTIAL_CONTENT {
        return match parse_content_range(response.headers()) {
            Some((start, _, entity_total))
                if start == pos && entity_total.is_none_or(|t| t == total) =>
            {
                check_content_encoding(response.headers())?;
                if let (Some(expected), Some(actual)) =
                    (validator, entity_validator(response.headers()))
                {
                    if actual != expected {
                        return Err(DownloadError::fatal(REMOTE_CHANGED));
                    }
                }
                Ok(())
            }
            _ => Err(DownloadError::fatal(REMOTE_CHANGED)),
        };
    }
    // If-Range mismatch answers 200 with the new entity; 416 means it shrank.
    if status == StatusCode::OK || status == StatusCode::RANGE_NOT_SATISFIABLE {
        return Err(DownloadError::fatal(REMOTE_CHANGED));
    }
    Err(DownloadError::from_status(status.as_u16()))
}

/// Pinned client of one connection, keyed by the host it is pinned to.
type HostClient = Option<(String, Client)>;

fn host_of(url: &Url) -> String {
    url.host_str().unwrap_or_default().to_string()
}

async fn open_range(
    shared: &Shared,
    client: &mut HostClient,
    pos: u64,
    end: u64,
) -> Result<Response, DownloadError> {
    let range =
        RangeRequest { start: Some(pos), end: Some(end), if_range: shared.validator.clone() };
    let target = lock_or_recover(&shared.final_url).clone();
    let reuse = client.as_ref().filter(|(host, _)| *host == host_of(&target)).map(|(_, c)| c);
    let mut opened = open(&shared.initial, &target, &shared.headers, &range, reuse).await?;
    // Signed CDN links expire mid-download: the original URL issues a fresh one.
    if matches!(opened.response.status().as_u16(), 401 | 403 | 410) && target != shared.initial {
        opened = open(&shared.initial, &shared.initial, &shared.headers, &range, None).await?;
    }
    let Opened { response, final_url, client: used } = opened;
    if let Some(wait) = retry_after(&response) {
        // The caller races this against cancel, so the wait is interruptible.
        tokio::time::sleep(wait).await;
    }
    check_range_response(&response, pos, shared.total, shared.validator.as_deref())?;
    *client = Some((host_of(&final_url), used));
    *lock_or_recover(&shared.final_url) = final_url;
    Ok(response)
}

enum StreamEnd {
    Complete,
    Stopped,
    Interrupted(String),
}

/// Stream one range to disk, resuming inside it after transient failures.
/// `Err(Cancelled)` covers both a user stop and a sibling's abort.
async fn run_segment(
    shared: &Shared,
    ctx: &JobContext,
    seg: &Segment,
    client: &mut HostClient,
    mut ready: Option<Response>,
) -> Result<(), DownloadError> {
    let mut cancel = ctx.cancel.clone();
    let mut abort = shared.abort.subscribe();
    let mut failures = 0u32;
    loop {
        let pos = seg.rewind_to_disk();
        if seg.unreceived() == 0 {
            return Ok(());
        }
        if ctx.is_cancelled() || *abort.borrow() {
            return Err(DownloadError::Cancelled);
        }
        let response = match ready.take() {
            Some(response) => Ok(response),
            None => tokio::select! {
                result = open_range(shared, client, pos, seg.end()) => result,
                _ = cancelled(&mut cancel) => return Err(DownloadError::Cancelled),
                _ = cancelled(&mut abort) => return Err(DownloadError::Cancelled),
            },
        };
        let response = match response {
            Ok(response) => response,
            Err(DownloadError::Transient(message)) => {
                failures += 1;
                if failures >= SEGMENT_MAX_FAILURES {
                    return Err(DownloadError::Transient(message));
                }
                sleep_or_cancel(backoff(failures), &ctx.cancel).await?;
                continue;
            }
            Err(other) => return Err(other),
        };

        let mut writer = OffsetWriter::new(shared.file.clone(), pos);
        let mut stream = response.bytes_stream();
        let end = loop {
            // Also true when another connection just took over the tail.
            if seg.unreceived() == 0 {
                break StreamEnd::Complete;
            }
            let chunk = tokio::select! {
                chunk = stream.next() => chunk,
                _ = cancelled(&mut cancel) => break StreamEnd::Stopped,
                _ = cancelled(&mut abort) => break StreamEnd::Stopped,
            };
            let chunk = match chunk {
                Some(Ok(chunk)) => chunk,
                Some(Err(error)) => {
                    break StreamEnd::Interrupted(format!("HTTP read failed: {error}"))
                }
                None => {
                    break StreamEnd::Interrupted(
                        "connection closed before the range was complete".into(),
                    )
                }
            };
            // The request may cover more than this range still owns.
            let take = seg.claim(chunk.len());
            writer.buffer.extend_from_slice(&chunk[..take]);
            ctx.progress.add(take as u64);
            if writer.buffer.len() >= FLUSH_BYTES {
                seg.done.fetch_add(writer.flush().await?, Ordering::Relaxed);
            }
            if ctx.limiter.acquire(take as u64, &ctx.cancel).await.is_err() {
                break StreamEnd::Stopped;
            }
        };
        // Whatever arrived is kept: a pause or a retry continues from here.
        seg.done.fetch_add(writer.flush().await?, Ordering::Relaxed);
        let message = match end {
            StreamEnd::Complete => return Ok(()),
            StreamEnd::Stopped => return Err(DownloadError::Cancelled),
            StreamEnd::Interrupted(message) => message,
        };
        // Bytes since the last failure prove the link works: count from one again.
        let advanced = seg.start + seg.done.load(Ordering::Relaxed) > pos;
        failures = if advanced { 1 } else { failures + 1 };
        if failures >= SEGMENT_MAX_FAILURES {
            return Err(DownloadError::Transient(message));
        }
        sleep_or_cancel(backoff(failures), &ctx.cancel).await?;
    }
}

/// One connection: finish queued ranges, then help with the largest one left.
async fn run_connection(
    shared: &Shared,
    ctx: &JobContext,
    mut ready: Option<(Arc<Segment>, Response, HostClient)>,
) -> Result<(), DownloadError> {
    let mut client: HostClient = None;
    loop {
        let (seg, response) = match ready.take() {
            Some((seg, response, used)) => {
                client = used;
                (seg, Some(response))
            }
            None => {
                let queued = lock_or_recover(&shared.pending).pop_front();
                match queued.or_else(|| shared.steal()) {
                    Some(seg) => (seg, None),
                    None => return Ok(()),
                }
            }
        };
        if let Err(error) = run_segment(shared, ctx, &seg, &mut client, response).await {
            // Hand the range back: another connection or the next run finishes it.
            lock_or_recover(&shared.pending).push_front(seg);
            if let DownloadError::Fatal(reason) = &error {
                if reason == REMOTE_CHANGED {
                    shared.restart.store(true, Ordering::Relaxed);
                }
                // Siblings would hit the same wall. `send_replace` stores the flag
                // even when no sibling is listening at this instant.
                shared.abort.send_replace(true);
            }
            return Err(error);
        }
    }
}

/// A better filename from the response, when the current one was only a guess.
pub(crate) fn better_name(current: &str, url: &Url, headers: &HeaderMap) -> Option<String> {
    let url_name = crate::util::infer_filename_from_url(url.as_str())
        .map(|name| crate::util::sanitize_filename(&name));
    let guessed = is_generic_download_filename(current)
        || current.starts_with("falcon_download")
        || !current.contains('.')
        || url_name.as_deref() == Some(current);
    if !guessed {
        return None;
    }
    if let Some(name) = content_disposition_filename(headers) {
        return (name != current).then_some(name);
    }
    let path = Path::new(current);
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if ext.is_none() || ext.as_deref() == Some("bin") {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("download");
        return extension_for_content_type(headers).map(|ext| format!("{stem}.{ext}"));
    }
    None
}

fn ensure_disk_space(dest: &Path, needed: u64) -> Result<(), DownloadError> {
    match free_disk_space(dest) {
        Some(free) if free < needed.saturating_add(DISK_MARGIN_BYTES) => {
            Err(DownloadError::fatal("ERR_DISK_FULL"))
        }
        _ => Ok(()),
    }
}

struct TempPaths {
    part: PathBuf,
    state: PathBuf,
}

impl TempPaths {
    fn for_dest(dest: &Path, id: i64) -> Result<Self, DownloadError> {
        match (hidden_sibling(dest, id, "part"), hidden_sibling(dest, id, "state")) {
            (Some(part), Some(state)) => Ok(Self { part, state }),
            _ => Err(DownloadError::fatal("Invalid output filename")),
        }
    }

    async fn remove(&self) {
        let _ = tokio::fs::remove_file(&self.part).await;
        let _ = tokio::fs::remove_file(&self.state).await;
    }
}

async fn load_saved_state(temp: &TempPaths, url: &str) -> Option<SavedState> {
    let (state, part, url) = (temp.state.clone(), temp.part.clone(), url.to_string());
    tokio::task::spawn_blocking(move || load_state(&state, &url, &part)).await.ok().flatten()
}

fn saved_downloaded(state: &SavedState) -> u64 {
    state.segments.iter().map(|segment| segment[2]).sum()
}

/// Sequential download for servers without range support or a known size.
/// Nothing can be resumed there, so every attempt starts at byte 0.
async fn download_stream(
    ctx: &JobContext,
    response: Response,
    part: &Path,
) -> Result<(), DownloadError> {
    let expected = response.content_length().unwrap_or(0);
    ctx.progress.set_connections(1);
    ctx.progress.set_total(expected);
    ctx.progress.set_downloaded(0);
    let part_owned = part.to_path_buf();
    let file = blocking(
        move || {
            std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(part_owned)
        },
        "could not create temp file",
    )
    .await?;
    let mut writer = OffsetWriter::new(Arc::new(file), 0);
    let mut cancel = ctx.cancel.clone();
    let mut stream = response.bytes_stream();
    let mut received = 0u64;
    loop {
        let chunk = tokio::select! {
            chunk = stream.next() => chunk,
            _ = cancelled(&mut cancel) => return Err(DownloadError::Cancelled),
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk =
            chunk.map_err(|e| DownloadError::transient(format!("HTTP read failed: {e}")))?;
        received += chunk.len() as u64;
        writer.buffer.extend_from_slice(&chunk);
        ctx.progress.add(chunk.len() as u64);
        if writer.buffer.len() >= FLUSH_BYTES {
            writer.flush().await?;
        }
        ctx.limiter.acquire(chunk.len() as u64, &ctx.cancel).await?;
    }
    writer.flush().await?;
    if expected > 0 && received != expected {
        return Err(DownloadError::transient(format!(
            "incomplete response: got {received} of {expected} bytes"
        )));
    }
    let file = writer.file.clone();
    blocking(move || file.sync_all(), "sync failed").await?;
    Ok(())
}

enum Attempt {
    Done(PathBuf),
    /// Partial data is void (entity changed, ranges no longer honored).
    Restart,
}

struct RangedJob {
    initial: Url,
    final_url: Url,
    headers: HttpHeaders,
    total: u64,
    validator: Option<String>,
    segments: Vec<[u64; 3]>,
    /// Range index the already-open response is delivering.
    ready: (usize, Response, Client),
    fresh: bool,
}

async fn run_ranged(
    ctx: &JobContext,
    job: RangedJob,
    dest: &Path,
    temp: &TempPaths,
) -> Result<Attempt, DownloadError> {
    let (part_owned, total, fresh) = (temp.part.clone(), job.total, job.fresh);
    let file = blocking(
        move || {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(fresh)
                .truncate(fresh)
                .open(part_owned)?;
            if fresh {
                // Sparse on APFS: reserves the size without writing the blocks.
                file.set_len(total)?;
            }
            Ok(file)
        },
        "could not create temp file",
    )
    .await?;

    let segments: Vec<Arc<Segment>> = job
        .segments
        .iter()
        .map(|[start, end, done]| Arc::new(Segment::new(*start, *end, *done)))
        .collect();
    let (ready_index, ready_response, ready_client) = job.ready;
    let ready_segment = segments[ready_index].clone();
    let pending: VecDeque<Arc<Segment>> = segments
        .iter()
        .enumerate()
        .filter(|(index, seg)| *index != ready_index && !seg.complete())
        .map(|(_, seg)| seg.clone())
        .collect();
    let already: u64 = segments.iter().map(|seg| seg.done.load(Ordering::Relaxed)).sum();
    // Enough connections for the queued ranges, plus helpers while splitting pays off.
    let useful = ((job.total - already) / MIN_STEAL_BYTES).max(1) as usize;
    let connections = job.headers.max_connections.clamp(1, 16).min(useful.max(pending.len() + 1));
    let ready_host = host_of(&job.final_url);
    let shared = Shared {
        initial: job.initial,
        final_url: Mutex::new(job.final_url),
        headers: job.headers,
        validator: job.validator,
        total: job.total,
        file: Arc::new(file),
        segments: Mutex::new(segments),
        pending: Mutex::new(pending),
        abort: watch::channel(false).0,
        restart: AtomicBool::new(false),
    };
    ctx.progress.set_total(job.total);
    ctx.progress.set_downloaded(already);
    ctx.progress.set_connections(connections as u32);
    shared.persist(&temp.state).await?;

    let results = {
        let mut ready = Some((ready_segment, ready_response, Some((ready_host, ready_client))));
        let workers = futures::future::join_all(
            (0..connections).map(|_| run_connection(&shared, ctx, ready.take())),
        );
        tokio::pin!(workers);
        let mut save = tokio::time::interval(state_save_every());
        save.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        save.tick().await;
        // Saves run in the blocking pool beside the workers: a slow disk must
        // never pause network reads or cancellation. At most one is in flight;
        // a tick that finds one still running simply skips its own save.
        let mut in_flight: Option<tokio::task::JoinHandle<Result<(), DownloadError>>> = None;
        let results = loop {
            tokio::select! {
                results = &mut workers => break results,
                _ = save.tick() => {
                    if in_flight.as_ref().is_some_and(|handle| handle.is_finished()) {
                        // Awaiting a finished task returns at once, so the
                        // workers are not paused here either.
                        match in_flight.take().unwrap().await {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => return Err(error),
                            Err(join) => {
                                return Err(DownloadError::fatal(format!(
                                    "persist task failed: {join}"
                                )))
                            }
                        }
                    }
                    if in_flight.is_none() {
                        in_flight = Some(shared.spawn_persist(&temp.state));
                    }
                }
            }
        };
        // Serialize with the final save below: both write the same tmp file.
        if let Some(handle) = in_flight.take() {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => return Err(error),
                Err(join) => {
                    return Err(DownloadError::fatal(format!("persist task failed: {join}")))
                }
            }
        }
        results
    };
    shared.persist(&temp.state).await?;

    if !shared.complete() {
        ctx.check_cancel()?;
        if shared.restart.load(Ordering::Relaxed) {
            return Ok(Attempt::Restart);
        }
        // Fatal beats transient: retrying a 404 would only burn the backoff.
        let errors: Vec<DownloadError> = results
            .into_iter()
            .filter_map(Result::err)
            .filter(|error| *error != DownloadError::Cancelled)
            .collect();
        return Err(errors
            .iter()
            .find(|error| matches!(error, DownloadError::Fatal(_)))
            .or(errors.first())
            .cloned()
            .unwrap_or_else(|| DownloadError::transient("download stopped before completion")));
    }

    ctx.progress.set_merging();
    let file = shared.file.clone();
    blocking(move || file.sync_all(), "sync failed").await?;
    let hints = FinalizeHints { expected_size: Some(total), ..Default::default() };
    let landed = ctx.finalize_with_hints(&temp.part, dest, hints).await?;
    let _ = tokio::fs::remove_file(&temp.state).await;
    Ok(Attempt::Done(landed))
}

/// First request of a fresh download. Open-ended, so it is the probe for size
/// and range support and already delivers the first range.
async fn open_first(
    ctx: &JobContext,
    initial: &Url,
    headers: &mut HttpHeaders,
) -> Result<Opened, DownloadError> {
    let probe = RangeRequest { start: Some(0), end: None, if_range: None };
    let mut cancel = ctx.cancel.clone();
    let mut opened = tokio::select! {
        result = open(initial, initial, headers, &probe, None) => result,
        _ = cancelled(&mut cancel) => return Err(DownloadError::Cancelled),
    };
    // A forwarded browser User-Agent can get the connection dropped or a 403
    // from bot filters. Try once more as ourselves before giving up.
    let refused = match &opened {
        Err(DownloadError::Transient(_)) => true,
        Ok(opened) => opened.response.status() == StatusCode::FORBIDDEN,
        Err(_) => false,
    };
    let forwarded = headers.user_agent.take().filter(|ua| !ua.trim().is_empty());
    if let (true, Some(forwarded)) = (refused, forwarded.clone()) {
        let retry = tokio::select! {
            result = open(initial, initial, headers, &probe, None) => result,
            _ = cancelled(&mut cancel) => return Err(DownloadError::Cancelled),
        };
        match retry {
            Ok(retry) if retry.response.status() != StatusCode::FORBIDDEN => {
                log::info!("download {}: server refused the browser User-Agent", ctx.id);
                opened = Ok(retry);
            }
            // Keep the original outcome: the page's own identity produced it.
            _ => headers.user_agent = Some(forwarded),
        }
    } else {
        headers.user_agent = forwarded;
    }
    let opened = opened?;
    // 416 = empty file or ranges refused: ask again without a Range header.
    if opened.response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        return open(initial, initial, headers, &RangeRequest::default(), None).await;
    }
    Ok(opened)
}

async fn attempt(
    ctx: &JobContext,
    initial: &Url,
    dest: &mut PathBuf,
    headers: &HttpHeaders,
    saved: Option<SavedState>,
) -> Result<Attempt, DownloadError> {
    let temp = TempPaths::for_dest(dest, ctx.id)?;

    let Some(saved) = saved else {
        temp.remove().await;
        let mut headers = headers.clone();
        let opened = open_first(ctx, initial, &mut headers).await?;
        let status = opened.response.status();
        if !status.is_success() {
            return Err(DownloadError::from_status(status.as_u16()));
        }
        check_content_encoding(opened.response.headers())?;
        if let Some(name) = dest
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|current| better_name(current, &opened.final_url, opened.response.headers()))
        {
            *dest = ctx.rename(dest, &name);
        }
        let temp = TempPaths::for_dest(dest, ctx.id)?;
        if status == StatusCode::PARTIAL_CONTENT {
            // Ranged only when the server proves it: 206 from byte 0 with a
            // concrete total. Anything else is a truncated portion, never the
            // file — fail closed instead of streaming it into place.
            let total = parse_content_range(opened.response.headers())
                .and_then(|(start, _, total)| if start == 0 { total } else { None })
                .filter(|total| *total > 0);
            let Some(total) = total else {
                return Err(DownloadError::fatal(PARTIAL_RESPONSE));
            };
            ensure_disk_space(dest, total)?;
            let job = RangedJob {
                initial: initial.clone(),
                validator: entity_validator(opened.response.headers()),
                segments: plan_segments(total, headers.max_connections)
                    .into_iter()
                    .map(|(start, end)| [start, end, 0])
                    .collect(),
                final_url: opened.final_url,
                headers,
                total,
                ready: (0, opened.response, opened.client),
                fresh: true,
            };
            return run_ranged(ctx, job, dest, &temp).await;
        }
        ensure_disk_space(dest, opened.response.content_length().unwrap_or(0))?;
        // Capture the response facts before the body stream consumes it.
        let hints = FinalizeHints {
            expected_size: opened.response.content_length(),
            response_is_html: response_is_html(opened.response.headers()),
        };
        download_stream(ctx, opened.response, &temp.part).await?;
        ctx.progress.set_merging();
        return Ok(Attempt::Done(ctx.finalize_with_hints(&temp.part, dest, hints).await?));
    };

    let next = saved.segments.iter().position(|[start, end, done]| *done < end - start + 1);
    let Some(index) = next else {
        // Every byte was on disk when the last run stopped: only the move is left.
        ctx.progress.set_total(saved.total);
        ctx.progress.set_downloaded(saved.total);
        ctx.progress.set_merging();
        let hints = FinalizeHints { expected_size: Some(saved.total), ..Default::default() };
        let landed = ctx.finalize_with_hints(&temp.part, dest, hints).await?;
        let _ = tokio::fs::remove_file(&temp.state).await;
        return Ok(Attempt::Done(landed));
    };
    let [start, end, done] = saved.segments[index];
    let range = RangeRequest {
        start: Some(start + done),
        end: Some(end),
        if_range: saved.validator.clone(),
    };
    let mut cancel = ctx.cancel.clone();
    let opened = tokio::select! {
        result = open(initial, initial, headers, &range, None) => result?,
        _ = cancelled(&mut cancel) => return Err(DownloadError::Cancelled),
    };
    match check_range_response(
        &opened.response,
        start + done,
        saved.total,
        saved.validator.as_deref(),
    ) {
        Ok(()) => {}
        Err(DownloadError::Fatal(reason)) if reason == REMOTE_CHANGED => {
            return Ok(Attempt::Restart)
        }
        Err(error) => return Err(error),
    }
    if saved.validator.is_none() {
        // Without any validator a same-size change is undetectable, so the
        // partial bytes can never be trusted. The probe above already proved
        // the file is still there; only bandwidth is lost, not progress on a
        // dead or changed server.
        log::warn!("download {}: no validator in resume state, restarting from zero", ctx.id);
        return Ok(Attempt::Restart);
    }
    let job = RangedJob {
        initial: initial.clone(),
        final_url: opened.final_url,
        headers: headers.clone(),
        total: saved.total,
        validator: saved.validator,
        segments: saved.segments,
        ready: (index, opened.response, opened.client),
        fresh: false,
    };
    run_ranged(ctx, job, dest, &temp).await
}

/// A crash inside finalize can leave proof that the download already finished.
/// Either `.part` and the destination are the same file object (the move's
/// hard-link happened, the unlink did not), or the move completed and only the
/// `.state` file was never cleaned up. Detect both before the fresh branch
/// would wipe the artifacts and download everything again.
async fn recover_completed(temp: &TempPaths, dest: &Path, url: &str) -> Option<PathBuf> {
    let landed = || Some(dest.canonicalize().unwrap_or_else(|_| dest.to_path_buf()));
    if same_file_inode(&temp.part, dest) {
        temp.remove().await;
        return landed();
    }
    if temp.part.exists() || !temp.state.exists() || !dest.exists() {
        return None;
    }
    let (state_path, dest_owned, url) = (temp.state.clone(), dest.to_path_buf(), url.to_string());
    let valid = tokio::task::spawn_blocking(move || {
        let state = load_complete_state_without_part(&state_path, &url)?;
        if std::fs::metadata(&dest_owned).ok()?.len() != state.total {
            return None;
        }
        validate_completed_file_named_with_hints(
            &dest_owned,
            &dest_owned,
            FinalizeHints { expected_size: Some(state.total), ..Default::default() },
        )
        .ok()?;
        Some(())
    })
    .await
    .ok()
    .flatten();
    valid?;
    let _ = tokio::fs::remove_file(&temp.state).await;
    landed()
}

/// Download `url` to `out_path`. Returns where the file finally landed (the
/// name may differ: server-provided filename, or `name (1).ext` on a clash).
pub async fn process_http(
    ctx: &JobContext,
    url: &str,
    out_path: &str,
    headers: HttpHeaders,
) -> Result<PathBuf, DownloadError> {
    let initial = validate_fetch_url_async(url).await?;
    let mut dest = PathBuf::from(out_path);
    let parent = dest.parent().ok_or_else(|| DownloadError::fatal("Invalid output path"))?;
    tokio::fs::create_dir_all(parent).await.map_err(|e| io_error("could not create folder", &e))?;
    if let Ok(temp) = TempPaths::for_dest(&dest, ctx.id) {
        if let Some(done) = recover_completed(&temp, &dest, initial.as_str()).await {
            return Ok(done);
        }
    }
    // Restore before the ticker can publish or persist an empty new run: even
    // when the resume probe fails, the row keeps the bytes the state can resume.
    // Compare canonical forms on both sides: the state file stores the parsed
    // Url while the row may hold a non-canonical spelling of the same target.
    let mut saved = match TempPaths::for_dest(&dest, ctx.id) {
        Ok(temp) => load_saved_state(&temp, initial.as_str()).await,
        Err(_) => None,
    };
    if let Some(state) = &saved {
        ctx.progress.set_total(state.total);
        ctx.progress.set_downloaded(saved_downloaded(state));
    }
    let _ticker = ctx.progress.start();
    // A void partial download is discarded once and the transfer starts clean.
    for _ in 0..2 {
        ctx.check_cancel()?;
        match attempt(ctx, &initial, &mut dest, &headers, saved.take()).await? {
            Attempt::Done(landed) => return Ok(landed),
            Attempt::Restart => {
                log::warn!("download {}: remote file changed, restarting from zero", ctx.id);
                ctx.progress.set_downloaded(0);
                TempPaths::for_dest(&dest, ctx.id)?.remove().await;
            }
        }
    }
    Err(DownloadError::transient("remote file keeps changing"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{Download, DownloadCategory, DownloadStatus};
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Default)]
    struct ServerOptions {
        /// Ignore Range and always answer 200 with the whole body.
        no_ranges: bool,
        /// Omit Content-Length on 200 responses (size unknown).
        no_length: bool,
        /// Close the socket after this many body bytes, for the first N requests.
        cut_after: Option<(usize, usize)>,
        /// Answer 503 to the first N requests.
        unavailable: usize,
        /// Answer this status once this many requests were already seen.
        status_after: Option<(usize, u16)>,
        etag: Option<&'static str>,
        disposition: Option<&'static str>,
        content_type: Option<&'static str>,
        /// Milliseconds to wait between 16 KiB body slices.
        slow_ms: u64,
        /// Close the socket without answering when the User-Agent contains this.
        drop_agent: Option<&'static str>,
        /// Emit this Content-Range verbatim instead of the honest one.
        content_range_override: Option<&'static str>,
        /// Answer 206 without any Content-Range header.
        omit_content_range: bool,
        /// Label responses with this Content-Encoding (body stays raw).
        content_encoding: Option<&'static str>,
        /// Honor the Range even when If-Range does not match (broken CDN).
        ignore_if_range: bool,
    }

    struct TestServer {
        base: String,
        requests: Arc<AtomicUsize>,
        ranges: Arc<Mutex<Vec<String>>>,
    }

    async fn serve(payload: Vec<u8>, options: ServerOptions) -> TestServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let payload = Arc::new(payload);
        let options = Arc::new(options);
        let (requests_c, ranges_c) = (requests.clone(), ranges.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let (payload, options) = (payload.clone(), options.clone());
                let (requests, ranges) = (requests_c.clone(), ranges_c.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let number = requests.fetch_add(1, Ordering::SeqCst);
                    let fail_status = if number < options.unavailable {
                        Some(503)
                    } else {
                        options
                            .status_after
                            .and_then(|(after, status)| (number >= after).then_some(status))
                    };
                    if let Some(status) = fail_status {
                        let reason =
                            if status == 404 { "Not Found" } else { "Service Unavailable" };
                        let _ = sock
                            .write_all(format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
                            .await;
                        return;
                    }
                    let header = |name: &str| {
                        request.lines().find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case(name).then(|| value.trim().to_string())
                        })
                    };
                    if let Some(needle) = options.drop_agent {
                        if header("user-agent").is_some_and(|ua| ua.contains(needle)) {
                            return;
                        }
                    }
                    let range = header("range").filter(|_| !options.no_ranges);
                    if let Some(spec) = &range {
                        ranges.lock().unwrap().push(spec.clone());
                    }
                    // If-Range with a stale validator → full 200, like a real server.
                    let range = match (range, header("if-range"), options.etag) {
                        (Some(_), Some(sent), Some(current))
                            if sent != current && !options.ignore_if_range =>
                        {
                            None
                        }
                        (range, _, _) => range,
                    };
                    let last = payload.len().saturating_sub(1);
                    let mut extra = String::new();
                    if let Some(etag) = options.etag {
                        extra.push_str(&format!("ETag: {etag}\r\n"));
                    }
                    if let Some(value) = options.disposition {
                        extra.push_str(&format!("Content-Disposition: {value}\r\n"));
                    }
                    if let Some(value) = options.content_type {
                        extra.push_str(&format!("Content-Type: {value}\r\n"));
                    }
                    if let Some(value) = options.content_encoding {
                        extra.push_str(&format!("Content-Encoding: {value}\r\n"));
                    }
                    let (status, start, end) = match range {
                        Some(spec) => {
                            let spec = spec.trim_start_matches("bytes=");
                            let (a, b) = spec.split_once('-').unwrap_or((spec, ""));
                            let start: usize = a.parse().unwrap_or(0);
                            let end = b.parse::<usize>().unwrap_or(last).min(last);
                            if payload.is_empty() || start > last {
                                let _ = sock
                                    .write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", payload.len()).as_bytes())
                                    .await;
                                return;
                            }
                            if let Some(override_spec) = options.content_range_override {
                                extra.push_str(&format!("Content-Range: {override_spec}\r\n"));
                            } else if !options.omit_content_range {
                                extra.push_str(&format!(
                                    "Content-Range: bytes {start}-{end}/{}\r\n",
                                    payload.len()
                                ));
                            }
                            ("206 Partial Content", start, end)
                        }
                        None => ("200 OK", 0, last),
                    };
                    let body: &[u8] = if payload.is_empty() { &[] } else { &payload[start..=end] };
                    if !(options.no_length && status.starts_with("200")) {
                        extra.push_str(&format!("Content-Length: {}\r\n", body.len()));
                    }
                    let head = format!("HTTP/1.1 {status}\r\n{extra}Connection: close\r\n\r\n");
                    if sock.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    let limit = match options.cut_after {
                        Some((bytes, first_n)) if number < first_n => bytes.min(body.len()),
                        _ => body.len(),
                    };
                    for slice in body[..limit].chunks(16 * 1024) {
                        if sock.write_all(slice).await.is_err() {
                            return;
                        }
                        if options.slow_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(options.slow_ms)).await;
                        }
                    }
                    let _ = sock.shutdown().await;
                });
            }
        });
        TestServer { base: format!("http://127.0.0.1:{}", addr.port()), requests, ranges }
    }

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 31 + i / 251) % 251) as u8).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("falcon-http-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn conns(n: usize) -> HttpHeaders {
        HttpHeaders { max_connections: n, ..Default::default() }
    }

    fn insert_live_row(
        db: &crate::storage::Database,
        url: &str,
        dir: &Path,
        filename: &str,
        total: u64,
        downloaded: u64,
    ) -> i64 {
        db.insert_download(&Download {
            id: None,
            url: url.into(),
            filename: filename.into(),
            save_path: dir.to_str().unwrap().into(),
            total_size: total,
            downloaded_size: downloaded,
            status: DownloadStatus::Downloading,
            category: DownloadCategory::Other,
            speed: 0.0,
            segments: 2,
            priority: 1,
            created_at: "t".into(),
            completed_at: None,
            error_message: None,
            referrer: None,
            user_agent: None,
            cookies: None,
            archived: false,
            attempts: 1,
            retry_at: None,
            expected_hash: None,
        })
        .unwrap()
    }

    fn db_job(db: &crate::storage::Database, id: i64) -> JobContext {
        let (_cancel, rx) = watch::channel(false);
        JobContext::new(
            id,
            None,
            Some(db.clone()),
            rx,
            Arc::new(crate::download::limiter::RateLimiter::new(0)),
        )
    }

    /// Sparse part file plus a bitmap where the first of two ranges has `kept` bytes.
    fn write_resume_artifacts(
        dir: &Path,
        filename: &str,
        id: i64,
        url: &str,
        total: u64,
        kept: u64,
        validator: Option<String>,
    ) {
        let part = part_path_for(dir.to_str().unwrap(), filename, id).unwrap();
        let part_file = std::fs::File::create(&part).unwrap();
        part_file.set_len(total).unwrap();
        drop(part_file);
        let state = SavedState {
            version: STATE_VERSION,
            url: url.into(),
            total,
            validator,
            segments: plan_segments(total, 2)
                .iter()
                .enumerate()
                .map(|(i, (start, end))| [*start, *end, if i == 0 { kept } else { 0 }])
                .collect(),
        };
        save_state(&state_path_for(dir.to_str().unwrap(), filename, id).unwrap(), &state).unwrap();
    }

    /// Put real bytes into the kept prefix of a fake partial file, so a
    /// wrongly-mixed result differs from the correct one byte for byte.
    fn fill_part_prefix(dir: &Path, filename: &str, id: i64, data: &[u8], kept: u64) {
        use std::os::unix::fs::FileExt;
        let part = part_path_for(dir.to_str().unwrap(), filename, id).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&part).unwrap();
        file.write_all_at(&data[..kept as usize], 0).unwrap();
    }

    #[test]
    fn plan_gives_one_range_per_connection_and_tiles_the_file() {
        for (total, connections, expected) in [
            (1u64, 8usize, 1usize),
            (MIN_SEGMENT_BYTES, 8, 1),
            (3 * MIN_SEGMENT_BYTES + 5, 8, 3),
            (100 * MIN_SEGMENT_BYTES + 12345, 8, 8),
            (700 * MIN_SEGMENT_BYTES + 7, 64, 16),
            (5 * MIN_SEGMENT_BYTES, 1, 1),
        ] {
            let plan = plan_segments(total, connections);
            assert_eq!(plan.len(), expected, "total {total}, connections {connections}");
            let mut cursor = 0;
            for (start, end) in &plan {
                assert_eq!(*start, cursor, "gap/overlap in {plan:?}");
                assert!(end >= start);
                cursor = end + 1;
            }
            assert_eq!(cursor, total, "plan must end at total for {total}");
        }
        assert!(plan_segments(0, 4).is_empty());
    }

    #[test]
    fn idle_connection_takes_the_tail_of_the_largest_range() {
        let total = 64 * MIN_SEGMENT_BYTES;
        let seg = Arc::new(Segment::new(0, total - 1, 0));
        let shared = Shared {
            initial: Url::parse("https://example.com/a").unwrap(),
            final_url: Mutex::new(Url::parse("https://example.com/a").unwrap()),
            headers: HttpHeaders::default(),
            validator: None,
            total,
            file: Arc::new(std::fs::File::open("/dev/null").unwrap()),
            segments: Mutex::new(vec![seg.clone()]),
            pending: Mutex::new(VecDeque::new()),
            abort: watch::channel(false).0,
            restart: AtomicBool::new(false),
        };
        // The owner already received 10 MiB (only 8 of them flushed).
        assert_eq!(seg.claim(10 * MIN_SEGMENT_BYTES as usize), 10 * MIN_SEGMENT_BYTES as usize);
        seg.done.store(8 * MIN_SEGMENT_BYTES, Ordering::Relaxed);
        let tail = shared.steal().expect("large range is split");
        // Split is above everything the owner has received, so no byte is fetched twice.
        assert!(tail.start >= 10 * MIN_SEGMENT_BYTES);
        assert_eq!(tail.start, seg.end() + 1);
        assert_eq!(tail.end(), total - 1);
        // The snapshot still tiles the file exactly.
        let mut tiles = shared.snapshot().segments;
        tiles.sort_by_key(|tile| tile[0]);
        assert_eq!(tiles[0][0], 0);
        assert_eq!(tiles[0][1] + 1, tiles[1][0]);
        assert_eq!(tiles[1][1], total - 1);
        // The owner stops exactly at its new end.
        let owner_left = seg.unreceived() as usize;
        assert_eq!(seg.claim(owner_left + 999), owner_left);
        assert_eq!(seg.claim(1), 0);
        // Small leftovers are not worth another connection.
        let small = Shared {
            segments: Mutex::new(vec![Arc::new(Segment::new(0, 2 * MIN_STEAL_BYTES - 2, 0))]),
            ..shared
        };
        assert!(small.steal().is_none());
    }

    #[test]
    fn artifact_paths_carry_the_download_id() {
        assert!(part_path_for("/tmp", "movie.mp4", 7)
            .unwrap()
            .ends_with(".movie.mp4.7.falcon.part"));
        assert!(state_path_for("/tmp", "movie.mp4", 7)
            .unwrap()
            .ends_with(".movie.mp4.7.falcon.state"));
    }

    #[test]
    fn response_name_only_replaces_a_guess() {
        let url = Url::parse("https://example.com/dl?id=5").unwrap();
        let mut headers = HeaderMap::new();
        headers
            .insert("content-disposition", "attachment; filename=\"report.pdf\"".parse().unwrap());
        assert_eq!(
            better_name("falcon_download.bin", &url, &headers).as_deref(),
            Some("report.pdf")
        );
        assert_eq!(better_name("download", &url, &headers).as_deref(), Some("report.pdf"));
        // A name the user or the browser chose is kept.
        assert_eq!(better_name("my-own-name.pdf", &url, &headers), None);
        // URL basename is a guess too.
        let named = Url::parse("https://example.com/files/get.php").unwrap();
        assert_eq!(better_name("get.php", &named, &headers).as_deref(), Some("report.pdf"));
        let mut typed = HeaderMap::new();
        typed.insert("content-type", "video/mp4".parse().unwrap());
        assert_eq!(
            better_name("falcon_download.bin", &url, &typed).as_deref(),
            Some("falcon_download.mp4")
        );
        assert_eq!(better_name("clip.webm", &url, &typed), None);
    }

    #[tokio::test]
    async fn multi_connection_download_matches_source() {
        let data = payload(6 * MIN_SEGMENT_BYTES as usize + 4321);
        let server =
            serve(data.clone(), ServerOptions { etag: Some("\"v1\""), ..Default::default() }).await;
        let dir = scratch("multi");
        let dest = dir.join("file.bin");
        let (ctx, _tx) = JobContext::for_tests(1);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_http(
            &ctx,
            &format!("{}/file.bin", server.base),
            dest.to_str().unwrap(),
            conns(4),
        )
        .await
        .unwrap();
        assert_eq!(landed, dest.canonicalize().unwrap());
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let requests = server.requests.load(Ordering::SeqCst);
        assert!((4..=8).contains(&requests), "one request per connection, got {requests}");
        assert!(!part_path_for(dir.to_str().unwrap(), "file.bin", 1).unwrap().exists());
        assert!(!state_path_for(dir.to_str().unwrap(), "file.bin", 1).unwrap().exists());
        assert_eq!(ctx.progress.downloaded(), data.len() as u64);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn small_file_needs_a_single_request() {
        let data = payload(4000);
        let server = serve(data.clone(), ServerOptions::default()).await;
        let dir = scratch("small");
        let dest = dir.join("small.bin");
        let (ctx, _tx) = JobContext::for_tests(2);
        let _g = crate::util::allow_loopback_for_tests();
        process_http(&ctx, &format!("{}/small.bin", server.base), dest.to_str().unwrap(), conns(8))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn server_without_ranges_streams_sequentially() {
        let data = payload(3 * MIN_SEGMENT_BYTES as usize);
        for no_length in [false, true] {
            let server = serve(
                data.clone(),
                ServerOptions { no_ranges: true, no_length, ..Default::default() },
            )
            .await;
            let dir = scratch("plain");
            let dest = dir.join("plain.bin");
            let (ctx, _tx) = JobContext::for_tests(3);
            let _g = crate::util::allow_loopback_for_tests();
            process_http(
                &ctx,
                &format!("{}/plain.bin", server.base),
                dest.to_str().unwrap(),
                conns(8),
            )
            .await
            .unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), data);
            assert_eq!(server.requests.load(Ordering::SeqCst), 1);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[tokio::test]
    async fn malformed_probe_206_is_never_streamed_as_the_whole_file() {
        let data = payload(3 * MIN_SEGMENT_BYTES as usize);
        let _g = crate::util::allow_loopback_for_tests();
        for (name, options) in [
            // 206 that does not start at byte 0.
            (
                "badstart",
                ServerOptions {
                    content_range_override: Some("bytes 100-199/3000000"),
                    ..Default::default()
                },
            ),
            // 206 with an unknown total (`bytes 0-999/*`).
            (
                "startotal",
                ServerOptions {
                    content_range_override: Some("bytes 0-999/*"),
                    ..Default::default()
                },
            ),
            // 206 without any Content-Range header.
            ("norange", ServerOptions { omit_content_range: true, ..Default::default() }),
        ] {
            let server = serve(data.clone(), options).await;
            let dir = scratch(name);
            let dest = dir.join("file.bin");
            let (ctx, _tx) = JobContext::for_tests(100);
            let result = process_http(
                &ctx,
                &format!("{}/file.bin", server.base),
                dest.to_str().unwrap(),
                conns(2),
            )
            .await;
            assert!(
                matches!(result, Err(DownloadError::Fatal(_))),
                "{name}: a malformed 206 must fail closed, got {result:?}"
            );
            assert!(!dest.exists(), "{name}: nothing may be saved from a partial response");
            assert!(
                !part_path_for(dir.to_str().unwrap(), "file.bin", 100).unwrap().exists(),
                "{name}: truncated bytes must not survive as a partial file"
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[tokio::test]
    async fn content_encoding_other_than_identity_is_fatal() {
        let data = payload(3 * MIN_SEGMENT_BYTES as usize + 99);
        let _g = crate::util::allow_loopback_for_tests();

        // Fresh ranged download: the probe itself arrives compressed.
        let server = serve(
            data.clone(),
            ServerOptions { content_encoding: Some("gzip"), ..Default::default() },
        )
        .await;
        let dir = scratch("enc-probe");
        let dest = dir.join("enc.bin");
        let (ctx, _tx) = JobContext::for_tests(101);
        let result = process_http(
            &ctx,
            &format!("{}/enc.bin", server.base),
            dest.to_str().unwrap(),
            conns(2),
        )
        .await;
        assert!(matches!(result, Err(DownloadError::Fatal(_))), "probe: {result:?}");
        assert!(!dest.exists());
        std::fs::remove_dir_all(dir).unwrap();

        // Stream fallback (server without ranges) is just as broken compressed.
        let server = serve(
            data.clone(),
            ServerOptions { no_ranges: true, content_encoding: Some("br"), ..Default::default() },
        )
        .await;
        let dir = scratch("enc-stream");
        let dest = dir.join("enc.bin");
        let (ctx, _tx) = JobContext::for_tests(102);
        let result = process_http(
            &ctx,
            &format!("{}/enc.bin", server.base),
            dest.to_str().unwrap(),
            conns(2),
        )
        .await;
        assert!(matches!(result, Err(DownloadError::Fatal(_))), "stream: {result:?}");
        assert!(!dest.exists());
        std::fs::remove_dir_all(dir).unwrap();

        // Resume: a compressed 206 must not mix into the partial file either.
        let server = serve(
            data.clone(),
            ServerOptions {
                etag: Some("\"v1\""),
                content_encoding: Some("deflate"),
                ..Default::default()
            },
        )
        .await;
        let dir = scratch("enc-resume");
        let dest = dir.join("enc.bin");
        let url = format!("{}/enc.bin", server.base);
        let total = data.len() as u64;
        write_resume_artifacts(&dir, "enc.bin", 103, &url, total, 1000, Some("\"v1\"".into()));
        let (ctx, _tx) = JobContext::for_tests(103);
        let result = process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await;
        assert!(matches!(result, Err(DownloadError::Fatal(_))), "resume: {result:?}");
        assert!(!dest.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn dropped_connections_resume_inside_the_range() {
        let data = payload(3 * MIN_SEGMENT_BYTES as usize + 99);
        // The first three responses die after 300 KB.
        let server = serve(
            data.clone(),
            ServerOptions { cut_after: Some((300_000, 3)), ..Default::default() },
        )
        .await;
        let dir = scratch("flaky");
        let dest = dir.join("flaky.bin");
        let (ctx, _tx) = JobContext::for_tests(4);
        let _g = crate::util::allow_loopback_for_tests();
        process_http(&ctx, &format!("{}/flaky.bin", server.base), dest.to_str().unwrap(), conns(2))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        // The cut range continued at the byte where it stopped, not at its start.
        let ranges = server.ranges.lock().unwrap().clone();
        assert!(ranges.iter().any(|spec| spec.starts_with("bytes=300000-")), "{ranges:?}");
        assert_eq!(ranges.iter().filter(|spec| spec.starts_with("bytes=0-")).count(), 1);
        assert_eq!(ctx.progress.downloaded(), data.len() as u64, "no byte counted twice");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn failed_attempt_persists_every_byte_written_before_failure() {
        let data = payload(8 * MIN_SEGMENT_BYTES as usize);
        let server = serve(
            data,
            ServerOptions {
                cut_after: Some((700_000, 2)),
                status_after: Some((2, 404)),
                ..Default::default()
            },
        )
        .await;
        let dir = scratch("failed-state");
        let dest = dir.join("failed.bin");
        let url = format!("{}/failed.bin", server.base);
        let (ctx, _tx) = JobContext::for_tests(13);
        let _g = crate::util::allow_loopback_for_tests();
        let result = process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await;
        assert!(matches!(result, Err(DownloadError::Fatal(_))), "{result:?}");

        let part = part_path_for(dir.to_str().unwrap(), "failed.bin", 13).unwrap();
        let state_path = state_path_for(dir.to_str().unwrap(), "failed.bin", 13).unwrap();
        let state = load_state(&state_path, &url, &part).expect("resume state survives failure");
        let kept: u64 = state.segments.iter().map(|seg| seg[2]).sum();
        assert_eq!(kept, 1_400_000, "all bytes written before the failure must be resumable");
        assert_eq!(ctx.progress.downloaded(), kept, "reported and resumable bytes diverged");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn pause_keeps_bytes_and_resume_requests_only_the_rest() {
        let data = payload(4 * MIN_SEGMENT_BYTES as usize);
        let server = serve(
            data.clone(),
            ServerOptions { slow_ms: 4, etag: Some("\"v1\""), ..Default::default() },
        )
        .await;
        let dir = scratch("pause");
        let dest = dir.join("pause.bin");
        let url = format!("{}/pause.bin", server.base);
        let _g = crate::util::allow_loopback_for_tests();

        let (ctx, tx) = JobContext::for_tests(5);
        let progress = ctx.progress.clone();
        tokio::spawn(async move {
            while progress.downloaded() < 700_000 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let _ = tx.send(true);
        });
        let paused = process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await;
        assert_eq!(paused, Err(DownloadError::Cancelled));
        assert!(!dest.exists());
        let part = part_path_for(dir.to_str().unwrap(), "pause.bin", 5).unwrap();
        let state_path = state_path_for(dir.to_str().unwrap(), "pause.bin", 5).unwrap();
        let state = load_state(&state_path, &url, &part).expect("resume state survives pause");
        let kept: u64 = state.segments.iter().map(|seg| seg[2]).sum();
        assert!(kept >= 600_000, "paused bytes must be on disk, got {kept}");
        assert!(kept < data.len() as u64);

        let before = server.ranges.lock().unwrap().len();
        let (ctx, _tx) = JobContext::for_tests(5);
        process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let resumed: Vec<String> = server.ranges.lock().unwrap()[before..].to_vec();
        assert!(!resumed.iter().any(|spec| spec.starts_with("bytes=0-")), "{resumed:?}");
        assert!(!part.exists() && !state_path.exists());
        assert_eq!(ctx.progress.downloaded(), data.len() as u64, "restored bytes counted twice");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn early_retry_failure_keeps_resumable_progress_in_db() {
        let total = 3 * MIN_SEGMENT_BYTES + 123;
        let kept = 700_000;
        let server = serve(
            payload(total as usize),
            ServerOptions { unavailable: usize::MAX, ..Default::default() },
        )
        .await;
        let dir = scratch("early-retry");
        let dest = dir.join("retry.bin");
        let url = format!("{}/retry.bin", server.base);
        let _g = crate::util::allow_loopback_for_tests();

        let db = crate::storage::Database::in_memory().unwrap();
        let id = insert_live_row(&db, &url, &dir, "retry.bin", total, kept);
        write_resume_artifacts(&dir, "retry.bin", id, &url, total, kept, None);
        let ctx = db_job(&db, id);
        let result = process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await;
        assert!(matches!(result, Err(DownloadError::Transient(_))), "{result:?}");

        let row = db.get_download(id).unwrap();
        assert_eq!(row.downloaded_size, kept, "early retry failure must not erase progress");
        assert_eq!(row.total_size, total);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn early_failure_without_resume_state_does_not_regress_db() {
        let total = 3 * MIN_SEGMENT_BYTES + 123;
        let kept = 700_000;
        let server = serve(
            payload(total as usize),
            ServerOptions { unavailable: usize::MAX, ..Default::default() },
        )
        .await;
        let dir = scratch("no-state-retry");
        let dest = dir.join("retry.bin");
        let url = format!("{}/retry.bin", server.base);
        let _g = crate::util::allow_loopback_for_tests();

        let db = crate::storage::Database::in_memory().unwrap();
        let id = insert_live_row(&db, &url, &dir, "retry.bin", total, kept);
        let ctx = db_job(&db, id);
        let result = process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await;
        assert!(matches!(result, Err(DownloadError::Transient(_))), "{result:?}");

        let row = db.get_download(id).unwrap();
        assert_eq!(
            row.downloaded_size, kept,
            "an attempt that never started must not regress the row"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn remote_change_restart_resets_progress_even_when_fresh_probe_fails() {
        let total = 3 * MIN_SEGMENT_BYTES + 123;
        let kept = 700_000;
        let server = serve(
            payload(total as usize),
            ServerOptions {
                status_after: Some((1, 503)),
                etag: Some("\"v2\""),
                ..Default::default()
            },
        )
        .await;
        let dir = scratch("restart-fails");
        let dest = dir.join("retry.bin");
        let url = format!("{}/retry.bin", server.base);
        let _g = crate::util::allow_loopback_for_tests();

        let db = crate::storage::Database::in_memory().unwrap();
        let id = insert_live_row(&db, &url, &dir, "retry.bin", total, kept);
        write_resume_artifacts(&dir, "retry.bin", id, &url, total, kept, Some("\"v1\"".into()));
        let ctx = db_job(&db, id);
        let result = process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await;
        assert!(matches!(result, Err(DownloadError::Transient(_))), "{result:?}");

        let row = db.get_download(id).unwrap();
        assert_eq!(row.downloaded_size, 0, "discarded partial data must not count as progress");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn changed_remote_file_restarts_instead_of_mixing_versions() {
        let old = payload(3 * MIN_SEGMENT_BYTES as usize);
        let dir = scratch("changed");
        let dest = dir.join("data.bin");
        let _g = crate::util::allow_loopback_for_tests();

        // Fake an interrupted download of the old version.
        let part = part_path_for(dir.to_str().unwrap(), "data.bin", 6).unwrap();
        std::fs::write(&part, &old).unwrap();
        let new: Vec<u8> = old.iter().map(|b| b.wrapping_add(1)).collect();
        let server =
            serve(new.clone(), ServerOptions { etag: Some("\"v2\""), ..Default::default() }).await;
        let url = format!("{}/data.bin", server.base);
        let plan = plan_segments(old.len() as u64, 2);
        let state = SavedState {
            version: STATE_VERSION,
            url: url.clone(),
            total: old.len() as u64,
            validator: Some("\"v1\"".into()),
            segments: plan
                .iter()
                .enumerate()
                .map(|(i, (s, e))| [*s, *e, if i == 0 { e - s + 1 } else { 0 }])
                .collect(),
        };
        save_state(&state_path_for(dir.to_str().unwrap(), "data.bin", 6).unwrap(), &state).unwrap();

        let (ctx, _tx) = JobContext::for_tests(6);
        process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), new, "result must be the new version only");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn if_range_ignoring_server_with_changed_validator_restarts() {
        // A CDN that ignores If-Range answers 206 even for a stale validator.
        // Only comparing the response validator keeps old and new bytes apart.
        let len = 3 * MIN_SEGMENT_BYTES as usize;
        let old = payload(len);
        let new: Vec<u8> = old.iter().map(|b| b.wrapping_add(1)).collect();
        let server = serve(
            new.clone(),
            ServerOptions { etag: Some("\"v2\""), ignore_if_range: true, ..Default::default() },
        )
        .await;
        let dir = scratch("ifrange-cdn");
        let dest = dir.join("data.bin");
        let url = format!("{}/data.bin", server.base);
        let _g = crate::util::allow_loopback_for_tests();

        let total = len as u64;
        let kept = 700_000;
        write_resume_artifacts(&dir, "data.bin", 110, &url, total, kept, Some("\"v1\"".into()));
        // The kept prefix holds the OLD version's bytes, like a real interruption.
        fill_part_prefix(&dir, "data.bin", 110, &old, kept);

        let (ctx, _tx) = JobContext::for_tests(110);
        process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), new, "old prefix must not mix into the new file");
        assert!(
            server.ranges.lock().unwrap().iter().any(|spec| spec.starts_with("bytes=0-")),
            "a stale entity must restart from byte zero"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn resume_without_any_validator_restarts_from_scratch() {
        // No ETag/Last-Modified was saved: a same-size change is undetectable,
        // so the partial file can never be trusted.
        let data = payload(3 * MIN_SEGMENT_BYTES as usize);
        let server = serve(data.clone(), ServerOptions::default()).await;
        let dir = scratch("novalidator");
        let dest = dir.join("data.bin");
        let url = format!("{}/data.bin", server.base);
        let _g = crate::util::allow_loopback_for_tests();

        let total = data.len() as u64;
        write_resume_artifacts(&dir, "data.bin", 111, &url, total, 700_000, None);

        let (ctx, _tx) = JobContext::for_tests(111);
        process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(
            server.ranges.lock().unwrap().iter().any(|spec| spec.starts_with("bytes=0-")),
            "unverifiable partial data must be discarded, not resumed"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn url_variant_that_normalizes_the_same_still_resumes() {
        // The DB may hold a non-canonical spelling (uppercase scheme/host).
        // The state file stores the canonical Url — resume must compare alike.
        let data = payload(3 * MIN_SEGMENT_BYTES as usize);
        let server =
            serve(data.clone(), ServerOptions { etag: Some("\"v1\""), ..Default::default() }).await;
        let dir = scratch("urlpattern");
        let dest = dir.join("data.bin");
        let _g = crate::util::allow_loopback_for_tests();

        let canonical =
            Url::parse(&format!("{}/data.bin", server.base)).unwrap().as_str().to_string();
        let raw = format!("{}/data.bin", server.base).replace("http://", "HTTP://");
        assert_ne!(raw, canonical, "test needs a raw spelling that differs pre-parse");
        let total = data.len() as u64;
        write_resume_artifacts(
            &dir,
            "data.bin",
            112,
            &canonical,
            total,
            700_000,
            Some("\"v1\"".into()),
        );
        fill_part_prefix(&dir, "data.bin", 112, &data, 700_000);

        let before = server.ranges.lock().unwrap().len();
        let (ctx, _tx) = JobContext::for_tests(112);
        process_http(&ctx, &raw, dest.to_str().unwrap(), conns(2)).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let resumed: Vec<String> = server.ranges.lock().unwrap()[before..].to_vec();
        assert!(
            !resumed.iter().any(|spec| spec.starts_with("bytes=0-")),
            "a spelling variant must resume, not restart: {resumed:?}"
        );
        assert!(
            resumed.iter().any(|spec| spec.starts_with("bytes=700000-")),
            "resume continues where the state stopped: {resumed:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn workers_keep_streaming_while_state_is_persisted() {
        // A slow disk must never freeze the connections: the periodic state
        // save has to run beside the workers, not inside their select loop.
        let data = payload(4 * MIN_SEGMENT_BYTES as usize);
        let server = serve(data.clone(), ServerOptions { slow_ms: 6, ..Default::default() }).await;
        let dir = scratch("persist-stall");
        let dest = dir.join("stall.bin");
        let _g = crate::util::allow_loopback_for_tests();

        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let persist_count = Arc::new(AtomicUsize::new(0));
        let hook = {
            let (entered, release, persist_count) =
                (entered.clone(), release.clone(), persist_count.clone());
            move || {
                // The first persist (before the workers start) passes through;
                // the first periodic one blocks until the test lets go.
                if persist_count.fetch_add(1, Ordering::SeqCst) == 1 {
                    entered.store(true, Ordering::SeqCst);
                    while !release.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        };
        STATE_SAVE_EVERY_TEST_MS.with(|c| c.set(100));
        PERSIST_TEST_HOOK.with(|h| *h.borrow_mut() = Some(Arc::new(hook)));

        let (ctx, _tx) = JobContext::for_tests(120);
        let progress = ctx.progress.clone();
        let url = format!("{}/stall.bin", server.base);
        let dest_str = dest.to_str().unwrap().to_string();
        let task = tokio::spawn(async move { process_http(&ctx, &url, &dest_str, conns(2)).await });

        // Wait for the periodic persist to grab the "slow disk".
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !entered.load(Ordering::SeqCst) {
            assert!(std::time::Instant::now() < deadline, "periodic persist never ran");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let first = progress.downloaded();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let second = progress.downloaded();
        release.store(true, Ordering::SeqCst);

        let landed = task.await.unwrap().expect("download finishes after the slow sync");
        assert_eq!(std::fs::read(&landed).unwrap(), data);
        assert!(
            second > first,
            "network reads stalled behind a slow persist ({first} -> {second})"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn unwritable_resume_state_fails_the_download() {
        // Fault injection: a directory sits where the state tmp file must be
        // written, so every save fails. The engine must surface that instead
        // of pretending the progress is safely resumable.
        let data = payload(3 * MIN_SEGMENT_BYTES as usize);
        let server = serve(data.clone(), ServerOptions::default()).await;
        let dir = scratch("state-io-error");
        let dest = dir.join("file.bin");
        let _g = crate::util::allow_loopback_for_tests();
        let tmp = dir.join(".file.bin.121.falcon.state.tmp");
        std::fs::create_dir_all(&tmp).unwrap();

        let (ctx, _tx) = JobContext::for_tests(121);
        let result = process_http(
            &ctx,
            &format!("{}/file.bin", server.base),
            dest.to_str().unwrap(),
            conns(2),
        )
        .await;
        assert!(matches!(result, Err(DownloadError::Fatal(_))), "{result:?}");
        assert!(!dest.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn server_errors_are_retried_and_missing_files_are_fatal() {
        let data = payload(2000);
        let _g = crate::util::allow_loopback_for_tests();
        let dir = scratch("errors");

        let flaky =
            serve(data.clone(), ServerOptions { unavailable: 1, ..Default::default() }).await;
        let (ctx, _tx) = JobContext::for_tests(7);
        let first = process_http(
            &ctx,
            &format!("{}/a.bin", flaky.base),
            dir.join("a.bin").to_str().unwrap(),
            conns(2),
        )
        .await;
        assert!(matches!(first, Err(DownloadError::Transient(_))), "{first:?}");

        let (ctx, _tx) = JobContext::for_tests(8);
        let blocked = process_http(
            &ctx,
            "http://192.168.1.1/secret.bin",
            dir.join("b.bin").to_str().unwrap(),
            conns(2),
        )
        .await;
        assert!(matches!(blocked, Err(DownloadError::Fatal(_))), "{blocked:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn server_filename_replaces_generic_name() {
        let data = payload(5000);
        let server = serve(
            data.clone(),
            ServerOptions {
                disposition: Some("attachment; filename=\"Quarterly Report.pdf\""),
                ..Default::default()
            },
        )
        .await;
        let dir = scratch("named");
        let (ctx, _tx) = JobContext::for_tests(9);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_http(
            &ctx,
            &format!("{}/download?id=9", server.base),
            dir.join("falcon_download.bin").to_str().unwrap(),
            conns(2),
        )
        .await
        .unwrap();
        assert_eq!(landed.file_name().unwrap(), "Quarterly Report.pdf");
        assert_eq!(std::fs::read(&landed).unwrap(), data);
        assert!(!dir.join("falcon_download.bin").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refused_browser_user_agent_falls_back_to_our_own() {
        let data = payload(5000);
        let server =
            serve(data.clone(), ServerOptions { drop_agent: Some("Chrome"), ..Default::default() })
                .await;
        let dir = scratch("agent");
        let dest = dir.join("agent.bin");
        let (ctx, _tx) = JobContext::for_tests(11);
        let _g = crate::util::allow_loopback_for_tests();
        let headers = HttpHeaders {
            max_connections: 2,
            user_agent: Some("Mozilla/5.0 Chrome/131.0 Safari/537.36".into()),
            ..Default::default()
        };
        process_http(&ctx, &format!("{}/agent.bin", server.base), dest.to_str().unwrap(), headers)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn finished_file_never_overwrites_an_existing_one() {
        let data = payload(5000);
        let server = serve(data.clone(), ServerOptions::default()).await;
        let dir = scratch("clash");
        let dest = dir.join("clash.bin");
        std::fs::write(&dest, b"user file").unwrap();
        let (ctx, _tx) = JobContext::for_tests(10);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_http(
            &ctx,
            &format!("{}/clash.bin", server.base),
            dest.to_str().unwrap(),
            conns(2),
        )
        .await
        .unwrap();
        assert_eq!(landed.file_name().unwrap(), "clash (1).bin");
        assert_eq!(std::fs::read(&dest).unwrap(), b"user file");
        assert_eq!(std::fs::read(&landed).unwrap(), data);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn redirect_to_private_host_is_refused() {
        let base = Url::parse("https://8.8.8.8/file").unwrap();
        let target = base.join("http://127.0.0.1/secret").unwrap();
        assert!(validate_fetch_url_async(target.as_str()).await.is_err());
    }

    /// State where every range is done, without writing any `.part` file.
    fn write_complete_state(dir: &Path, filename: &str, id: i64, url: &str, total: u64) {
        let state = SavedState {
            version: STATE_VERSION,
            url: url.into(),
            total,
            validator: Some("\"v1\"".into()),
            segments: plan_segments(total, 2)
                .iter()
                .map(|(start, end)| [*start, *end, end - start + 1])
                .collect(),
        };
        save_state(&state_path_for(dir.to_str().unwrap(), filename, id).unwrap(), &state).unwrap();
    }

    #[tokio::test]
    async fn empty_response_with_explicit_zero_length_finalizes() {
        // A deliberate empty file (Content-Length: 0) is not a broken download.
        let server = serve(Vec::new(), ServerOptions::default()).await;
        let dir = scratch("empty-ok");
        let dest = dir.join("empty.bin");
        let (ctx, _tx) = JobContext::for_tests(130);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_http(
            &ctx,
            &format!("{}/empty.bin", server.base),
            dest.to_str().unwrap(),
            conns(2),
        )
        .await
        .unwrap();
        assert_eq!(landed, dest.canonicalize().unwrap());
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn small_binary_starting_with_lt_finalizes_when_size_matches() {
        // `<` is a valid first byte of a small binary when the server delivered
        // exactly the promised size and did not label the response as HTML.
        let data = b"<\0\xff\x00valid-binary-payload".to_vec();
        let _g = crate::util::allow_loopback_for_tests();
        for (name, no_ranges) in [("lt-ranged", false), ("lt-stream", true)] {
            let server =
                serve(data.clone(), ServerOptions { no_ranges, ..Default::default() }).await;
            let dir = scratch(name);
            let dest = dir.join("firmware.bin");
            let (ctx, _tx) = JobContext::for_tests(131);
            process_http(
                &ctx,
                &format!("{}/firmware.bin", server.base),
                dest.to_str().unwrap(),
                conns(2),
            )
            .await
            .unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), data, "{name}");
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[tokio::test]
    async fn html_error_page_streamed_as_a_file_is_rejected() {
        // A small "file" that is really an HTML error page: the response's own
        // Content-Type gives it away even when the size technically matches.
        let data = b"<html><body>blocked</body></html>".to_vec();
        let server = serve(
            data,
            ServerOptions {
                no_ranges: true,
                content_type: Some("text/html"),
                ..Default::default()
            },
        )
        .await;
        let dir = scratch("html-page");
        let dest = dir.join("archive.zip");
        let (ctx, _tx) = JobContext::for_tests(132);
        let _g = crate::util::allow_loopback_for_tests();
        let result =
            process_http(&ctx, &format!("{}/get/7", server.base), dest.to_str().unwrap(), conns(2))
                .await;
        assert!(
            matches!(result, Err(DownloadError::Fatal(ref reason)) if reason.contains("error page")),
            "{result:?}"
        );
        assert!(!dest.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn part_hardlinked_to_dest_finishes_without_a_network_request() {
        // Crash between hard_link and unlink inside finalize: `.part` and the
        // destination are the same file object, so the download is done.
        let data = payload(5000);
        let server =
            serve(data.clone(), ServerOptions { no_ranges: true, ..Default::default() }).await;
        let dir = scratch("sameinode");
        let dest = dir.join("stream.bin");
        let url = format!("{}/stream.bin", server.base);
        let part = part_path_for(dir.to_str().unwrap(), "stream.bin", 133).unwrap();
        std::fs::write(&part, &data).unwrap();
        std::fs::hard_link(&part, &dest).unwrap();
        let (ctx, _tx) = JobContext::for_tests(133);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await.unwrap();
        assert_eq!(landed, dest.canonicalize().unwrap());
        assert_eq!(
            server.requests.load(Ordering::SeqCst),
            0,
            "a finished download must not be fetched again"
        );
        assert!(!part.exists(), "the leftover .part alias is cleaned up");
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn missing_part_with_complete_state_and_valid_dest_recovers() {
        // Crash after the move but before the state cleanup: only the `.state`
        // file survived. A complete, valid destination means the work is done.
        let data = payload(3000);
        let server = serve(data.clone(), ServerOptions::default()).await;
        let dir = scratch("state-leftover");
        let dest = dir.join("rec.bin");
        let url = format!("{}/rec.bin", server.base);
        std::fs::write(&dest, &data).unwrap();
        write_complete_state(&dir, "rec.bin", 134, &url, data.len() as u64);
        let (ctx, _tx) = JobContext::for_tests(134);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await.unwrap();
        assert_eq!(landed, dest.canonicalize().unwrap());
        assert_eq!(
            server.requests.load(Ordering::SeqCst),
            0,
            "a finished download must not be fetched again"
        );
        assert!(!state_path_for(dir.to_str().unwrap(), "rec.bin", 134).unwrap().exists());
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn missing_part_with_mismatched_dest_downloads_again() {
        // Same leftover state, but the destination is not the promised file:
        // recovery must not trust it and the normal download runs.
        let data = payload(3000);
        let server = serve(data.clone(), ServerOptions::default()).await;
        let dir = scratch("state-mismatch");
        let dest = dir.join("rec.bin");
        let url = format!("{}/rec.bin", server.base);
        std::fs::write(&dest, b"foreign").unwrap();
        write_complete_state(&dir, "rec.bin", 135, &url, data.len() as u64);
        let (ctx, _tx) = JobContext::for_tests(135);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_http(&ctx, &url, dest.to_str().unwrap(), conns(2)).await.unwrap();
        assert!(
            server.requests.load(Ordering::SeqCst) > 0,
            "a foreign destination must not short-circuit the download"
        );
        assert_eq!(
            landed.file_name().unwrap(),
            "rec (1).bin",
            "the foreign file is never overwritten"
        );
        assert_eq!(std::fs::read(&landed).unwrap(), data);
        assert_eq!(std::fs::read(&dest).unwrap(), b"foreign");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Row whose only job is to carry an expected checksum for finalize.
    fn insert_hashed_row(
        db: &crate::storage::Database,
        dir: &Path,
        filename: &str,
        expected_hash: &str,
    ) -> i64 {
        db.insert_download(&Download {
            id: None,
            url: "http://127.0.0.1/x".into(),
            filename: filename.into(),
            save_path: dir.to_str().unwrap().into(),
            total_size: 0,
            downloaded_size: 0,
            status: DownloadStatus::Downloading,
            category: DownloadCategory::Other,
            speed: 0.0,
            segments: 2,
            priority: 1,
            created_at: "t".into(),
            completed_at: None,
            error_message: None,
            referrer: None,
            user_agent: None,
            cookies: None,
            archived: false,
            attempts: 0,
            retry_at: None,
            expected_hash: Some(expected_hash.into()),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn download_completes_when_checksum_matches() {
        let data = payload(6 * MIN_SEGMENT_BYTES as usize + 77);
        let server =
            serve(data.clone(), ServerOptions { etag: Some("\"v1\""), ..Default::default() }).await;
        let dir = scratch("sum-ok");
        let dest = dir.join("file.bin");
        let db = crate::storage::Database::in_memory().unwrap();
        let expected = {
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            hasher.update(&data);
            let mut hex = String::new();
            for byte in hasher.finalize() {
                hex.push_str(&format!("{byte:02x}"));
            }
            format!("sha256:{hex}")
        };
        let id = insert_hashed_row(&db, &dir, "file.bin", &expected);
        let ctx = db_job(&db, id);
        let _g = crate::util::allow_loopback_for_tests();
        let landed = process_http(
            &ctx,
            &format!("{}/file.bin", server.base),
            dest.to_str().unwrap(),
            conns(4),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&landed).unwrap(), data);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn checksum_mismatch_fails_and_saves_nothing() {
        let data = payload(4000);
        let server = serve(data.clone(), ServerOptions::default()).await;
        let dir = scratch("sum-bad");
        let dest = dir.join("file.bin");
        let db = crate::storage::Database::in_memory().unwrap();
        // sha256 of the empty string — never matches real bytes.
        let id = insert_hashed_row(
            &db,
            &dir,
            "file.bin",
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
        let ctx = db_job(&db, id);
        let _g = crate::util::allow_loopback_for_tests();
        let result = process_http(
            &ctx,
            &format!("{}/file.bin", server.base),
            dest.to_str().unwrap(),
            conns(2),
        )
        .await;
        assert_eq!(result, Err(DownloadError::Fatal("ERR_CHECKSUM_MISMATCH".into())));
        assert!(!dest.exists(), "untrusted bytes must not land in the target folder");
        assert!(
            !part_path_for(dir.to_str().unwrap(), "file.bin", id).unwrap().exists(),
            "the partial file cannot be resumed — its bytes are untrusted"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
