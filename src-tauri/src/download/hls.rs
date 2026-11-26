//! HLS (m3u8) engine for plain MPEG-TS playlists.
//!
//! Segments are fetched in parallel into a per-download temp dir, each written
//! to `*.tmp` and renamed when complete, so an interrupted run never leaves a
//! half segment that a resume would trust. ffmpeg then remuxes them.
//!
//! Encrypted, fMP4, byte-range and split-audio playlists are handed to yt-dlp
//! (`DownloadError::NeedsYtDlp`) instead of producing a broken file.

use crate::download::http_client::{open, HttpHeaders, Opened, RangeRequest};
use crate::download::job::JobContext;
use crate::download::limiter::{cancelled, sleep_or_cancel};
use crate::download::tools::find_ffmpeg;
use crate::download::DownloadError;
use crate::util::validate_fetch_url_async;
use futures::stream::{self, StreamExt};
use m3u8_rs::{AlternativeMediaType, KeyMethod, MasterPlaylist, MediaPlaylist, Playlist};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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

/// Concat demuxer `file 'name'` — reject anything that is not our `seg_NNNNN.ts`.
fn ffmpeg_concat_file_line(name: &str) -> Result<String, String> {
    if name.is_empty()
        || name.contains("..")
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
    {
        return Err("Invalid HLS segment name".into());
    }
    Ok(format!("file '{name}'\n"))
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
        segments,
        estimated_bytes: bandwidth
            .filter(|bw| *bw > 0 && duration > 0.0)
            .map(|bw| (bw as f64 / 8.0 * duration) as u64),
    })
}

type HostClient = Option<(String, Client)>;

/// One attempt: stream a segment into `tmp`. Returns the bytes written.
async fn fetch_segment_once(
    ctx: &JobContext,
    source: &Url,
    target: &Url,
    headers: &HttpHeaders,
    client: &mut HostClient,
    tmp: &Path,
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
        ctx.progress.add(chunk.len() as u64);
        ctx.limiter.acquire(chunk.len() as u64, &ctx.cancel).await?;
    }
    file.flush().await.map_err(|e| DownloadError::fatal(e.to_string()))?;
    if written == 0 {
        return Err(DownloadError::transient("segment is empty"));
    }
    if expected.is_some_and(|len| len != written) {
        return Err(DownloadError::transient("segment is incomplete"));
    }
    Ok(written)
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
) -> Result<u64, DownloadError> {
    let path = dir.join(segment_name(index));
    let tmp = dir.join(format!("{}.tmp", segment_name(index)));
    let mut cancel = ctx.cancel.clone();
    let mut attempt = 0u32;
    loop {
        ctx.check_cancel()?;
        let before = ctx.progress.downloaded();
        let result = tokio::select! {
            result = fetch_segment_once(ctx, &plan.source, &plan.segments[index], headers, client, &tmp) => result,
            _ = cancelled(&mut cancel) => Err(DownloadError::Cancelled),
        };
        match result {
            Ok(bytes) => {
                tokio::fs::rename(&tmp, &path)
                    .await
                    .map_err(|e| DownloadError::fatal(format!("segment rename failed: {e}")))?;
                return Ok(bytes);
            }
            Err(error) => {
                // Partial bytes of a failed attempt are not progress.
                ctx.progress.set_downloaded(before);
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
    let count = plan.segments.len();
    // Complete segments from an earlier run. `*.tmp` leftovers are never trusted.
    let mut have_bytes = 0u64;
    let mut missing = Vec::new();
    for index in 0..count {
        match tokio::fs::metadata(dir.join(segment_name(index))).await {
            Ok(meta) if meta.len() > 0 => have_bytes += meta.len(),
            _ => missing.push(index),
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
    let workers = (0..connections).map(|_| {
        let queue = queue.clone();
        let done_segments = done_segments.clone();
        let done_bytes = done_bytes.clone();
        async move {
            let mut client: HostClient = None;
            loop {
                let Some(index) = crate::util::lock_or_recover(&queue).pop_front() else {
                    return Ok::<(), DownloadError>(());
                };
                let bytes = fetch_segment(ctx, plan, headers, &mut client, index, dir).await?;
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
        Some("mkv") => "mkv",
        Some("mov") => "mov",
        Some("ts") => "ts",
        _ => "mp4",
    }
}

/// Remux the segments into one container with ffmpeg (stream copy, no re-encode).
async fn merge_segments(
    ctx: &JobContext,
    dir: &Path,
    names: &[String],
    output: &Path,
) -> Result<(), DownloadError> {
    let ffmpeg = find_ffmpeg().map_err(DownloadError::Fatal)?;
    let mut list = String::new();
    for name in names {
        list.push_str(&ffmpeg_concat_file_line(name).map_err(DownloadError::Fatal)?);
    }
    let list_path = dir.join("list.txt");
    tokio::fs::write(&list_path, list)
        .await
        .map_err(|e| DownloadError::fatal(format!("could not write concat list: {e}")))?;
    let _ = tokio::fs::remove_file(output).await;

    let mut child = tokio::process::Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&list_path)
        // ADTS → MP4 AAC is handled by ffmpeg's automatic bitstream filter.
        .args(["-map", "0", "-c", "copy", "-y"])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| DownloadError::fatal(format!("could not start ffmpeg: {e}")))?;
    let mut stderr = child.stderr.take();
    let read_stderr = async {
        let mut text = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            use tokio::io::AsyncReadExt;
            let _ = pipe.take(16 * 1024).read_to_end(&mut text).await;
        }
        String::from_utf8_lossy(&text).trim().to_string()
    };
    let mut cancel = ctx.cancel.clone();
    let (status, detail) = tokio::select! {
        result = async { let detail = read_stderr.await; (child.wait().await, detail) } => result,
        _ = cancelled(&mut cancel) => {
            return Err(DownloadError::Cancelled);
        }
    };
    let status = status.map_err(|e| DownloadError::fatal(format!("ffmpeg failed: {e}")))?;
    if !status.success() {
        let line = detail.lines().last().unwrap_or("unknown error");
        return Err(DownloadError::fatal(format!("ffmpeg merge failed: {line}")));
    }
    Ok(())
}

/// Download an HLS stream to `out_path`. Returns where the file landed.
pub async fn process_hls_stream(
    ctx: &JobContext,
    url: &str,
    out_path: &str,
    headers: HttpHeaders,
) -> Result<PathBuf, DownloadError> {
    let dest = PathBuf::from(out_path);
    let _ticker = ctx.progress.start();
    let plan = resolve_plan(ctx, url, &headers).await?;
    // Fail before hours of segments if the merge tool is missing.
    find_ffmpeg().map_err(DownloadError::Fatal)?;
    let dir = hls_session_dir(ctx.id);
    let names = download_segments(ctx, &plan, &headers, &dir).await?;

    ctx.progress.set_merging();
    let output = dir.join(format!("output.{}", output_extension(&dest)));
    merge_segments(ctx, &dir, &names, &output).await?;
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
    fn ffmpeg_concat_file_line_rejects_quotes_and_paths() {
        assert_eq!(ffmpeg_concat_file_line("seg_00001.ts").unwrap(), "file 'seg_00001.ts'\n");
        assert!(ffmpeg_concat_file_line("seg_00'01.ts").is_err());
        assert!(ffmpeg_concat_file_line("../secret.ts").is_err());
        assert!(ffmpeg_concat_file_line("a/b.ts").is_err());
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
        std::fs::write(dir.join("seg_00000.ts"), b"SEGMENT-0-DATA").unwrap();
        // A crash mid-write: must be fetched again.
        std::fs::write(dir.join("seg_00001.ts.tmp"), b"SEGM").unwrap();
        let (ctx, _tx) = JobContext::for_tests(2);
        let _g = crate::util::allow_loopback_for_tests();
        let plan = resolve_plan(&ctx, &format!("{}/high/index.m3u8", fixture.base), &headers(2))
            .await
            .unwrap();
        download_segments(&ctx, &plan, &headers(2), &dir).await.unwrap();
        let hits = fixture.hits.lock().unwrap().clone();
        assert!(!hits.contains_key("/high/seg0.ts"), "finished segment must not be re-fetched");
        assert_eq!(hits["/high/seg1.ts"], 1);
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

    /// Real MPEG-TS segments → one playable MP4. Skipped when ffmpeg is absent.
    #[tokio::test]
    async fn end_to_end_remux_produces_an_mp4() {
        let Ok(ffmpeg) = find_ffmpeg() else {
            eprintln!("ffmpeg not found — skipping HLS end-to-end test");
            return;
        };
        let src = std::env::temp_dir().join(format!("falcon-hls-src-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&src).unwrap();
        let made = std::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("testsrc=duration=3:size=160x120:rate=10")
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=3"])
            // H.264 like real HLS; MPEG-4 part 2 in TS cannot be stream-copied to MP4.
            .args(["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-g", "10"])
            .args(["-c:a", "aac", "-f", "hls", "-hls_time", "1"])
            .args(["-hls_playlist_type", "vod", "-hls_segment_filename"])
            .arg(src.join("part%d.ts"))
            .arg(src.join("index.m3u8"))
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !made {
            eprintln!("ffmpeg could not build a fixture — skipping HLS end-to-end test");
            let _ = std::fs::remove_dir_all(&src);
            return;
        }
        let mut files = HashMap::new();
        for entry in std::fs::read_dir(&src).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            files.insert(format!("/{name}"), std::fs::read(entry.path()).unwrap());
        }
        let segment_count = files.keys().filter(|k| k.ends_with(".ts")).count();
        assert!(segment_count >= 2, "fixture should have several segments");
        let fixture = serve(files, Vec::new()).await;

        let out_dir = std::env::temp_dir().join(format!("falcon-hls-out-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&out_dir).unwrap();
        let dest = out_dir.join("clip.mp4");
        let id = 900_000 + i64::from(std::process::id());
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
