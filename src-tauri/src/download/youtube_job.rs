//! Native YouTube download orchestration: extract → selector-pick → download
//! through the native HTTP engine → `merge_av_mp4` for adaptive pairs →
//! finalize → subtitles. No external tools are involved at any step.
//!
//! Extraction happens once at job start because the resolved googlevideo URLs
//! are time-limited and IP-bound; when the CDN rejects them mid-download with
//! 401/403/410 the extraction is repeated exactly once per run, mirroring the
//! HLS re-resolve contract. Partial child downloads resume across the rotation
//! by rewriting the resume state's URL — the engine's own validator machinery
//! still decides whether the bytes on disk may be trusted.

use crate::download::http;
use crate::download::http_client::HttpHeaders;
use crate::download::job::JobContext;
use crate::download::mux::merge_av_mp4;
use crate::download::progress::Progress;
use crate::download::subtitles::write_selected_subtitles_for_extraction;
use crate::download::youtube::{
    extract, ExtractError, ValidatedFetcher, YoutubeExtraction, YoutubeFormat, INNERTUBE_CLIENTS,
};
use crate::download::ytselect::{parse_chain, select, Selection, DEFAULT_VIDEO_FORMAT};
use crate::download::DownloadError;
use crate::storage::models::SubtitleTrackSelection;
use crate::util::{sanitize_filename, split_falcon_format};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct YoutubeJobOptions {
    pub cookies: Option<String>,
    pub referrer: Option<String>,
    pub proxy: Option<String>,
    pub max_connections: usize,
    pub subtitle_tracks: Option<Vec<SubtitleTrackSelection>>,
}

/// Working directory for adaptive child downloads, next to the destination so
/// interrupted pairs can resume. Lifecycle cleanup removes orphans.
pub fn run_dir_for(save_dir: &str, download_id: i64) -> PathBuf {
    Path::new(save_dir).join(format!(".falcon-dm-yt-{download_id}"))
}

/// `.falcon-dm-yt-<id>` (current) or legacy `.falcon-dm-ytdlp-<id>` / `-<uuid>`
/// run dirs — kept so startup cleanup keeps recognizing every generation.
pub fn parse_run_dir_name(name: &str) -> Option<i64> {
    for prefix in [".falcon-dm-yt-", ".falcon-dm-ytdlp-"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            let numeric = rest.split('-').next()?;
            if numeric.is_empty() || !numeric.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Ok(id) = numeric.parse::<i64>() else {
                continue;
            };
            // Plain `<id>` always matches; `<id>-<uuid>` only with a
            // well-formed uuid tail (legacy yt-dlp generation).
            if rest == numeric {
                return Some(id);
            }
            if let Some(tail) = rest.strip_prefix(numeric).and_then(|t| t.strip_prefix('-')) {
                if uuid::Uuid::parse_str(tail).is_ok() {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// Remove the adaptive-pair run dir. The engine's `.part`/`.state` siblings of
/// the final name are covered by `http::remove_artifacts` already.
pub fn remove_artifacts(save_dir: &str, download_id: i64) {
    let _ = std::fs::remove_dir_all(run_dir_for(save_dir, download_id));
}

fn is_expired_credentials(error: &DownloadError) -> bool {
    matches!(error, DownloadError::Fatal(message) if matches!(message.as_str(), "HTTP 401" | "HTTP 403" | "HTTP 410"))
}

fn classify_extract_error(error: &ExtractError) -> DownloadError {
    match error {
        ExtractError::Network(_) => DownloadError::Transient(error.to_string()),
        ExtractError::Http { status, .. }
            if matches!(status, 408 | 425 | 429) || (500..600).contains(status) =>
        {
            DownloadError::Transient(error.to_string())
        }
        _ => DownloadError::Fatal(error.to_string()),
    }
}

/// Media downloads go out with the extraction client's UA, the canonical watch
/// page as referrer and the row's cookies — the same identity that minted the
/// media URLs. The browser UA is deliberately not forwarded: googlevideo bot
/// filtering is sensitive to inconsistent client fingerprints.
fn media_headers(video_id: &str, options: &YoutubeJobOptions) -> HttpHeaders {
    HttpHeaders {
        cookies: options.cookies.clone(),
        referrer: Some(format!("https://www.youtube.com/watch?v={video_id}")),
        user_agent: INNERTUBE_CLIENTS.first().map(|client| client.user_agent.to_string()),
        proxy: options.proxy.clone(),
        max_connections: options.max_connections,
    }
}

/// Output extension for a selection: merges are always MP4; single files keep
/// their container, with the audio-only MP4 spelled `m4a`.
fn output_extension(selection: &Selection) -> &'static str {
    match selection {
        Selection::Merge { .. } => "mp4",
        Selection::Single(format) => match (format.container.as_str(), format.is_audio_only()) {
            ("mp4", true) => "m4a",
            ("mp4", false) => "mp4",
            ("webm", _) => "webm",
            _ => "mp4",
        },
    }
}

/// Mirror of the legacy rename contract: a placeholder stem (manual add of a
/// watch URL) is replaced by the video title; the extension always follows the
/// container the download actually produces.
fn corrected_dest(ctx: &JobContext, dest: &Path, title: &str, extension: &str) -> PathBuf {
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("download").to_string();
    let placeholder = stem.starts_with("falcon_download") || stem == "watch" || stem == "download";
    let stem = match sanitize_filename(title) {
        clean if placeholder && !clean.is_empty() => clean.chars().take(150).collect(),
        _ => stem,
    };
    let wanted = format!("{stem}.{extension}");
    if dest.file_name().and_then(|n| n.to_str()) == Some(wanted.as_str()) {
        return dest.to_path_buf();
    }
    ctx.rename(dest, &wanted)
}

fn child_context(ctx: &JobContext) -> JobContext {
    JobContext {
        id: ctx.id,
        app: None,
        db: None,
        cancel: ctx.cancel.clone(),
        limiter: ctx.limiter.clone(),
        progress: Progress::new(None, None, ctx.id),
    }
}

/// Rewrite the `.state` URL in place so a rotated (re-extracted) media URL can
/// resume the partial bytes of the same itag. Returns false when nothing was
/// rewritten — the engine then simply starts over, which is always correct.
fn rewrite_state_url(save_dir: &Path, filename: &str, id: i64, url: &str) -> bool {
    let Some(save_dir) = save_dir.to_str() else {
        return false;
    };
    let Some(path) = http::state_path_for(save_dir, filename, id) else {
        return false;
    };
    let Ok(original) = std::fs::read(&path) else {
        return false;
    };
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&original) else {
        return false;
    };
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    match object.get("url").and_then(|existing| existing.as_str()) {
        Some(existing) if existing != url => {
            object.insert("url".into(), serde_json::Value::String(url.to_string()));
        }
        _ => return false,
    }
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return false;
    };
    let tmp = path.with_extension("state.tmp");
    if std::fs::write(&tmp, &bytes).is_err() {
        return false;
    }
    std::fs::rename(&tmp, &path).is_ok()
}

/// A finished child stream: complete file present, no in-progress siblings.
/// googlevideo entities are immutable per itag, so a size match is strong
/// evidence; without a known length any non-zero complete file counts.
fn child_complete(path: &Path, expected_len: Option<u64>, id: i64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let size_matches = match expected_len {
        Some(len) => meta.len() == len,
        None => meta.len() > 0,
    };
    if !size_matches {
        return false;
    }
    // A `.part`/`.state` pair next to the file means the finalize never ran.
    http::hidden_sibling(path, id, "part").is_none_or(|p| !p.exists())
        && http::hidden_sibling(path, id, "state").is_none_or(|p| !p.exists())
}

/// Publish combined monotonic progress of both child downloads onto the parent
/// row: total is the sum once known, downloaded never goes backwards, and the
/// transferred delta feeds the speed meter even across child restarts.
fn spawn_progress_bridge(
    ctx: &JobContext,
    children: Vec<Progress>,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    let parent = ctx.progress.clone();
    tokio::spawn(async move {
        let mut high_water = 0u64;
        let mut last_transferred = 0u64;
        while !stop.load(Ordering::SeqCst) {
            let downloaded: u64 = children.iter().map(|p| p.downloaded()).sum();
            let transferred: u64 = children.iter().map(|p| p.transferred()).sum();
            high_water = high_water.max(downloaded);
            parent.set_downloaded(high_water);
            if transferred > last_transferred {
                parent.add_transferred(transferred - last_transferred);
                last_transferred = transferred;
            }
            let total: u64 = children.iter().map(|p| p.total()).sum();
            if total > parent.total() {
                parent.set_total(total);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
}

/// Download both adaptive streams concurrently into the run dir. A stream that
/// already finished in a previous attempt (same itag, complete file) is not
/// re-downloaded — the re-extract rotation must not throw away finished work.
async fn download_pair(
    ctx: &JobContext,
    run_dir: &Path,
    video: &YoutubeFormat,
    audio: &YoutubeFormat,
    headers: &HttpHeaders,
) -> Result<(PathBuf, PathBuf), DownloadError> {
    tokio::fs::create_dir_all(run_dir)
        .await
        .map_err(|e| DownloadError::fatal(format!("could not create work folder: {e}")))?;
    let video_path = run_dir.join("video.mp4");
    let audio_path = run_dir.join("audio.m4a");
    let video_done = child_complete(&video_path, video.content_length, ctx.id);
    let audio_done = child_complete(&audio_path, audio.content_length, ctx.id);
    // A complete-looking file that does not match the freshly extracted entity
    // is dropped: leaving it would make the engine's no-overwrite finalize
    // land the new download under a `(1)` suffix the merge never reads.
    for (path, done) in [(&video_path, video_done), (&audio_path, audio_done)] {
        if !done
            && path.exists()
            && http::hidden_sibling(path, ctx.id, "part").is_none_or(|p| !p.exists())
        {
            let _ = std::fs::remove_file(path);
        }
    }
    if video_done && audio_done {
        return Ok((video_path, audio_path));
    }

    let video_ctx = child_context(ctx);
    let audio_ctx = child_context(ctx);
    let video_progress = video_ctx.progress.clone();
    let audio_progress = audio_ctx.progress.clone();
    // Seed the combined total from the extraction's content lengths: the CDN
    // length only appears once the first response lands.
    let hint = video.content_length.unwrap_or(0) + audio.content_length.unwrap_or(0);
    if hint > ctx.progress.total() {
        ctx.progress.set_total(hint);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let bridge =
        spawn_progress_bridge(ctx, vec![video_progress, audio_progress], Arc::clone(&stop));

    let download_one = |child_ctx: JobContext, format: &YoutubeFormat, path: &Path, done: bool| {
        let headers = headers.clone();
        let path = path.to_path_buf();
        let format = format.clone();
        async move {
            if done {
                return Ok(());
            }
            if let Some(dir) = path.parent().and_then(|p| p.to_str()) {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    rewrite_state_url(Path::new(dir), name, child_ctx.id, &format.url);
                }
            }
            let out = path.to_string_lossy().to_string();
            http::process_http(&child_ctx, &format.url, &out, headers).await.map(|_| ())
        }
    };

    let video_future = download_one(video_ctx, video, &video_path, video_done);
    let audio_future = download_one(audio_ctx, audio, &audio_path, audio_done);
    let result = tokio::try_join!(video_future, audio_future);
    stop.store(true, Ordering::SeqCst);
    let _ = bridge.await;
    result?;
    Ok((video_path, audio_path))
}

/// Merge pair or single file: run the download phase for one selection. The
/// returned path is the finalized user-visible file.
async fn run_selection(
    ctx: &JobContext,
    extraction: &YoutubeExtraction,
    selection: &Selection,
    dest: &Path,
    options: &YoutubeJobOptions,
) -> Result<PathBuf, DownloadError> {
    let headers = media_headers(&extraction.video_id, options);
    match selection {
        Selection::Single(format) => {
            let out = dest.to_string_lossy().to_string();
            if let Some(dir) = dest.parent().and_then(|p| p.to_str()) {
                if let Some(name) = dest.file_name().and_then(|n| n.to_str()) {
                    rewrite_state_url(Path::new(dir), name, ctx.id, &format.url);
                }
            }
            http::process_http(ctx, &format.url, &out, headers).await
        }
        Selection::Merge { video, audio } => {
            let (video, audio) = (video.as_ref(), audio.as_ref());
            let Some(save_dir) = dest.parent() else {
                return Err(DownloadError::fatal("Invalid output path"));
            };
            let Some(save_dir_str) = save_dir.to_str() else {
                return Err(DownloadError::fatal("Invalid output path"));
            };
            let run_dir = run_dir_for(save_dir_str, ctx.id);
            let (video_path, audio_path) =
                download_pair(ctx, &run_dir, video, audio, &headers).await?;
            ctx.check_cancel()?;
            ctx.progress.set_merging();
            let merged = run_dir.join("merged.mp4");
            let (video_owned, audio_owned, merged_owned) =
                (video_path.clone(), audio_path.clone(), merged.clone());
            let cancel = ctx.cancel.clone();
            let merged_result = tokio::task::spawn_blocking(move || {
                merge_av_mp4(&video_owned, &audio_owned, &merged_owned, &|| *cancel.borrow())
            })
            .await
            .map_err(|e| DownloadError::fatal(format!("merge task failed: {e}")))?;
            match merged_result {
                Ok(()) => {}
                Err(crate::download::mux::MergeError::Cancelled) => {
                    return Err(DownloadError::Cancelled)
                }
                Err(crate::download::mux::MergeError::Fatal(message)) => {
                    let _ = tokio::fs::remove_dir_all(&run_dir).await;
                    return Err(DownloadError::Fatal(message));
                }
                Err(rejection @ crate::download::mux::MergeError::UnsupportedFormat(_)) => {
                    let _ = tokio::fs::remove_dir_all(&run_dir).await;
                    return Err(DownloadError::Fatal(format!(
                        "ERR_YT_MERGE_UNSUPPORTED: {rejection}"
                    )));
                }
            }
            ctx.progress.set_downloaded(ctx.progress.total());
            let landed = ctx.finalize(&merged, dest).await?;
            let _ = tokio::fs::remove_dir_all(&run_dir).await;
            Ok(landed)
        }
    }
}

async fn write_subtitles(
    extraction: &YoutubeExtraction,
    fetcher: &ValidatedFetcher,
    landed: &Path,
    selection: Option<&[SubtitleTrackSelection]>,
) {
    let Some(save_dir) = landed.parent() else {
        return;
    };
    let stem = landed.file_stem().and_then(|s| s.to_str()).unwrap_or("download").to_string();
    // Subtitle failures are logged inside the writer and never turn a finished
    // video into a failure.
    write_selected_subtitles_for_extraction(extraction, fetcher, save_dir, &stem, selection).await;
}

/// Native YouTube pipeline entry point. `watch_url` is the canonical watch
/// page possibly carrying the `#falconfmt=` selector fragment.
pub async fn process_youtube(
    ctx: &JobContext,
    watch_url: &str,
    out_path: &str,
    options: YoutubeJobOptions,
) -> Result<PathBuf, DownloadError> {
    // The fragment stays attached: `process_youtube_with_fetcher` splits the
    // selector out of it — stripping it here would silently drop the user's
    // quality choice.
    let fetcher = match options.proxy.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(proxy) => ValidatedFetcher::with_proxy(proxy),
        None => ValidatedFetcher::new(),
    };
    process_youtube_with_fetcher(ctx, watch_url, out_path, &options, fetcher).await
}

/// Core pipeline with an injected fetcher — the production shell builds it from
/// the proxy settings, tests point it at loopback fixtures.
async fn process_youtube_with_fetcher(
    ctx: &JobContext,
    watch: &str,
    out_path: &str,
    options: &YoutubeJobOptions,
    fetcher: ValidatedFetcher,
) -> Result<PathBuf, DownloadError> {
    let (watch, selector) = split_falcon_format(watch);
    let selector = selector.unwrap_or_else(|| DEFAULT_VIDEO_FORMAT.to_string());
    let chain = parse_chain(&selector)
        .map_err(|error| DownloadError::fatal(format!("ERR_YT_FORMAT_INVALID: {error}")))?;
    let extraction = extract(&watch, &fetcher, options.cookies.as_deref())
        .await
        .map_err(|e| classify_extract_error(&e))?;
    run_extraction(ctx, &watch, extraction, chain, out_path, options, &fetcher).await
}

/// The download rotation loop over one extraction. Separated from the extract
/// step so tests can patch fixture media URLs onto loopback servers without
/// weakening the HTTPS media-URL invariant of the extractor.
async fn run_extraction(
    ctx: &JobContext,
    watch: &str,
    mut extraction: YoutubeExtraction,
    chain: crate::download::ytselect::Chain,
    out_path: &str,
    options: &YoutubeJobOptions,
    fetcher: &ValidatedFetcher,
) -> Result<PathBuf, DownloadError> {
    ctx.check_cancel()?;
    let mut dest = PathBuf::from(out_path);
    let mut reextracted = false;
    let landed = loop {
        ctx.check_cancel()?;
        let Some(selection) = select(&extraction.formats, &chain) else {
            return Err(DownloadError::fatal(
                "ERR_YT_FORMAT_UNAVAILABLE: the requested quality is not available for this video",
            ));
        };
        dest = corrected_dest(ctx, &dest, &extraction.title, output_extension(&selection));
        match run_selection(ctx, &extraction, &selection, &dest, options).await {
            Ok(landed) => break landed,
            Err(error) if !reextracted && is_expired_credentials(&error) => {
                reextracted = true;
                log::info!(
                    "download {}: media URLs rejected ({}), re-extracting once",
                    ctx.id,
                    error.message()
                );
                extraction = extract(watch, fetcher, options.cookies.as_deref())
                    .await
                    .map_err(|e| classify_extract_error(&e))?;
            }
            Err(error) => return Err(error),
        }
    };

    write_subtitles(&extraction, fetcher, &landed, options.subtitle_tracks.as_deref()).await;
    Ok(landed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_dir_names_parse_across_generations() {
        assert_eq!(parse_run_dir_name(".falcon-dm-yt-42"), Some(42));
        assert_eq!(parse_run_dir_name(".falcon-dm-ytdlp-42"), Some(42));
        assert_eq!(
            parse_run_dir_name(".falcon-dm-ytdlp-7-123e4567-e89b-12d3-a456-426614174000"),
            Some(7)
        );
        assert_eq!(parse_run_dir_name(".falcon-dm-yt-x"), None);
        assert_eq!(parse_run_dir_name(".falcon-dm-ytdlp-7-notauuid"), None);
        assert_eq!(parse_run_dir_name(".falcon-dm-yt-7-extra"), None);
    }

    #[test]
    fn output_extension_follows_the_selection() {
        let audio = YoutubeFormat {
            itag: 140,
            url: String::new(),
            mime_type: String::new(),
            container: "mp4".into(),
            codec_family: crate::download::youtube::CodecFamily::Aac,
            codecs: None,
            width: None,
            height: None,
            bitrate: None,
            content_length: None,
            audio_quality: None,
            progressive: false,
            has_video: false,
            has_audio: true,
            quality_label: None,
            fps: None,
            was_ciphered: false,
        };
        assert_eq!(output_extension(&Selection::Single(Box::new(audio.clone()))), "m4a");
        let mut webm_audio = audio.clone();
        webm_audio.container = "webm".into();
        assert_eq!(output_extension(&Selection::Single(Box::new(webm_audio))), "webm");
        let mut video = audio.clone();
        video.has_video = true;
        video.has_audio = false;
        video.height = Some(720);
        assert_eq!(
            output_extension(&Selection::Merge { video: Box::new(video), audio: Box::new(audio) }),
            "mp4"
        );
    }

    #[test]
    fn expired_credentials_match_only_the_rotation_statuses() {
        assert!(is_expired_credentials(&DownloadError::Fatal("HTTP 401".into())));
        assert!(is_expired_credentials(&DownloadError::Fatal("HTTP 403".into())));
        assert!(is_expired_credentials(&DownloadError::Fatal("HTTP 410".into())));
        assert!(!is_expired_credentials(&DownloadError::Fatal("HTTP 404".into())));
        assert!(!is_expired_credentials(&DownloadError::Transient("HTTP 403".into())));
    }

    #[test]
    fn extract_errors_classify_network_and_statuses() {
        assert!(matches!(
            classify_extract_error(&ExtractError::Network("dns".into())),
            DownloadError::Transient(_)
        ));
        assert!(matches!(
            classify_extract_error(&ExtractError::Http { status: 429, context: "player API" }),
            DownloadError::Transient(_)
        ));
        assert!(matches!(
            classify_extract_error(&ExtractError::Http { status: 503, context: "watch page" }),
            DownloadError::Transient(_)
        ));
        assert!(matches!(
            classify_extract_error(&ExtractError::Http { status: 403, context: "player API" }),
            DownloadError::Fatal(_)
        ));
        let fatal = classify_extract_error(&ExtractError::BotCheck { reason: "captcha".into() });
        assert!(fatal.message().starts_with("ERR_YT_BOT_CHECK"));
    }
    // ---- end-to-end orchestration against loopback fixtures ----

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const E2E_WATCH_PAGE: &str = r#"<!doctype html><html><script>
ytcfg.set({"INNERTUBE_API_KEY":"fixture-key","INNERTUBE_CLIENT_VERSION":"2.20260101.00.00","VISITOR_DATA":"visitor-data","STS":23123});
</script></html>"#;

    struct E2eServer {
        origin: String,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for E2eServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn player_response(_origin: &str, video_len: u64, audio_len: u64) -> String {
        format!(
            r#"{{
  "playabilityStatus": {{"status": "OK"}},
  "videoDetails": {{
    "videoId": "dQw4w9WgXcQ",
    "title": "Fixture Video",
    "author": "Fixture Channel",
    "lengthSeconds": "33",
    "isLiveContent": false,
    "thumbnail": {{"thumbnails": []}}
  }},
  "streamingData": {{
    "expiresInSeconds": "21600",
    "adaptiveFormats": [
      {{
        "itag": 137,
        "url": "https://rr1---sn-fixture.googlevideo.com/videoplayback?itag=137",
        "mimeType": "video/mp4; codecs=\"avc1.640028\"",
        "bitrate": 9000000,
        "width": 1920,
        "height": 1080,
        "qualityLabel": "1080p",
        "contentLength": "{video_len}",
        "fps": 30
      }},
      {{
        "itag": 140,
        "url": "https://rr1---sn-fixture.googlevideo.com/videoplayback?itag=140",
        "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
        "bitrate": 128000,
        "qualityLabel": null,
        "contentLength": "{audio_len}"
      }}
    ]
  }}
}}"#
        )
    }

    /// One server for everything: the extraction endpoints (`/watch`,
    /// `/youtubei/v1/player`) plus range-capable media payloads.
    async fn start_e2e_server(video: Vec<u8>, audio: Vec<u8>) -> E2eServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let origin = format!("http://127.0.0.1:{port}");
        let player = player_response(&origin, video.len() as u64, audio.len() as u64);
        let video = Arc::new(video);
        let audio = Arc::new(audio);
        let player = Arc::new(player);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    continue;
                };
                let (video, audio, player) =
                    (Arc::clone(&video), Arc::clone(&audio), Arc::clone(&player));
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut chunk = [0u8; 8192];
                    loop {
                        if raw.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                        let read = socket.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 || raw.len() > 128 * 1024 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..read]);
                    }
                    let request = String::from_utf8_lossy(&raw).to_string();
                    let target = request
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("");
                    let path = target.split('?').next().unwrap_or("");
                    if path == "/watch" {
                        write_text(&mut socket, 200, "text/html", E2E_WATCH_PAGE).await;
                    } else if path == "/youtubei/v1/player" {
                        write_text(&mut socket, 200, "application/json", &player).await;
                    } else if path == "/video.mp4" {
                        write_media(&mut socket, &request, &video).await;
                    } else if path == "/audio.m4a" {
                        write_media(&mut socket, &request, &audio).await;
                    } else {
                        write_text(&mut socket, 404, "text/plain", "not found").await;
                    }
                });
            }
        });
        E2eServer { origin, task }
    }

    async fn write_text(
        socket: &mut tokio::net::TcpStream,
        status: u16,
        content_type: &str,
        body: &str,
    ) {
        let head = format!(
            "HTTP/1.1 {status} OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.write_all(body.as_bytes()).await;
    }

    async fn write_media(socket: &mut tokio::net::TcpStream, request: &str, bytes: &[u8]) {
        let range = request.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name.trim().eq_ignore_ascii_case("range")).then(|| value.trim().to_string())
        });
        let total = bytes.len();
        let (status, start, end) = match range.as_deref().and_then(|r| r.strip_prefix("bytes=")) {
            Some(spec) => {
                let mut parts = spec.split('-');
                let start: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let end: usize = parts
                    .next()
                    .and_then(|s| (!s.is_empty()).then(|| s.parse().ok()).flatten())
                    .unwrap_or(total - 1)
                    .min(total - 1);
                (206, start, end)
            }
            None => (200, 0, total - 1),
        };
        let body = &bytes[start..=end];
        let mut head = format!(
            "HTTP/1.1 {status} x\r\ncontent-type: video/mp4\r\ncontent-length: {}\r\naccept-ranges: bytes\r\netag: \"v1\"\r\nconnection: close\r\n",
            body.len()
        );
        if status == 206 {
            head.push_str(&format!("content-range: bytes {start}-{end}/{total}\r\n"));
        }
        head.push_str("\r\n");
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.write_all(body).await;
    }

    fn dash_fixture(name: &str) -> Vec<u8> {
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("test-fixtures/dash").join(name))
            .unwrap()
    }

    #[tokio::test]
    async fn end_to_end_merge_download_muxes_native_mp4() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let video = dash_fixture("video-frag.mp4");
        let audio = dash_fixture("audio-frag.m4a");
        let server = start_e2e_server(video.clone(), audio.clone()).await;
        let root = std::env::temp_dir().join(format!("falcon-ytjob-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let out = root.join("falcon_download.mp4");
        let (ctx, _cancel) = JobContext::for_tests(4242);
        let fetcher = ValidatedFetcher::with_endpoint_origin(&server.origin);
        let options = YoutubeJobOptions {
            cookies: None,
            referrer: None,
            proxy: None,
            max_connections: 4,
            subtitle_tracks: None,
        };
        // Real extraction against the fixture endpoints, then the media URLs
        // are patched onto the loopback media server (the extractor rightly
        // refuses to emit non-HTTPS URLs itself).
        let mut extraction = extract("https://www.youtube.com/watch?v=dQw4w9WgXcQ", &fetcher, None)
            .await
            .expect("fixture extraction");
        assert_eq!(extraction.title, "Fixture Video");
        for format in &mut extraction.formats {
            let path = if format.has_video { "/video.mp4" } else { "/audio.m4a" };
            format.url = format!("{}{path}?itag={}", server.origin, format.itag);
        }
        let chain = parse_chain(crate::download::ytselect::DEFAULT_VIDEO_FORMAT).unwrap();
        let landed = run_extraction(
            &ctx,
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            extraction,
            chain,
            &out.to_string_lossy(),
            &options,
            &fetcher,
        )
        .await
        .expect("orchestration must succeed");

        // Title rename happened, the merge produced a valid MP4, and the run
        // dir is gone.
        assert_eq!(landed.file_name().unwrap(), "Fixture Video.mp4");
        let bytes = std::fs::read(&landed).unwrap();
        assert!(bytes.len() > video.len() + audio.len() / 2, "merged output carries both streams");
        assert!(bytes.windows(4).take(64).any(|w| w == b"ftyp"), "output is an MP4 file");
        assert!(
            !run_dir_for(root.to_str().unwrap(), 4242).exists(),
            "run dir is cleaned after finalizing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn end_to_end_audio_selector_lands_m4a() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let video = dash_fixture("video-frag.mp4");
        let audio = dash_fixture("audio-frag.m4a");
        let server = start_e2e_server(video, audio.clone()).await;
        let root = std::env::temp_dir().join(format!("falcon-ytjob-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let out = root.join("falcon_download.m4a");
        let (ctx, _cancel) = JobContext::for_tests(4343);
        let fetcher = ValidatedFetcher::with_endpoint_origin(&server.origin);
        let options = YoutubeJobOptions {
            cookies: None,
            referrer: None,
            proxy: None,
            max_connections: 2,
            subtitle_tracks: None,
        };
        let mut extraction = extract("https://www.youtube.com/watch?v=dQw4w9WgXcQ", &fetcher, None)
            .await
            .expect("fixture extraction");
        for format in &mut extraction.formats {
            let path = if format.has_video { "/video.mp4" } else { "/audio.m4a" };
            format.url = format!("{}{path}?itag={}", server.origin, format.itag);
        }
        let chain = parse_chain("bestaudio[ext=m4a]/bestaudio").unwrap();
        let landed = run_extraction(
            &ctx,
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            extraction,
            chain,
            &out.to_string_lossy(),
            &options,
            &fetcher,
        )
        .await
        .expect("audio-only orchestration must succeed");
        assert_eq!(landed.file_name().unwrap(), "Fixture Video.m4a");
        assert_eq!(std::fs::metadata(&landed).unwrap().len(), audio.len() as u64);
        let _ = std::fs::remove_dir_all(&root);
    }
}
