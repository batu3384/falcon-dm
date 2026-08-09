use crate::storage::{models::DownloadStatus, Database};
use crate::util::{sanitize_header_value, validate_fetch_url};
use futures::stream::{self, StreamExt, TryStreamExt};
use m3u8_rs::Playlist;
use reqwest::header::{HeaderMap, HeaderValue, COOKIE, REFERER, USER_AGENT};
use reqwest::{Client, RequestBuilder};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use url::Url;

struct TempDirGuard {
    path: PathBuf,
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Clone, Default)]
pub struct HlsHeaders {
    pub cookies: Option<String>,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
}

const MAX_PLAYLIST_BYTES: usize = 16 * 1024 * 1024;
const MAX_SEGMENT_BYTES: usize = 64 * 1024 * 1024;
const MAX_SEGMENTS: usize = 10_000;
const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, serde::Serialize)]
struct HlsProgressEvent {
    id: i64,
    downloaded_size: u64,
    total_size: u64,
    speed: f64,
    status: String,
    connections: u32,
}

fn build_client(headers: &HlsHeaders) -> Result<Client, String> {
    let mut map = HeaderMap::new();
    if let Some(ref r) = headers.referrer {
        let v = sanitize_header_value(r);
        if !v.is_empty() {
            map.insert(REFERER, HeaderValue::from_str(&v).map_err(|e| e.to_string())?);
        }
    }
    if let Some(ref ua) = headers.user_agent {
        let v = sanitize_header_value(ua);
        if !v.is_empty() {
            map.insert(USER_AGENT, HeaderValue::from_str(&v).map_err(|e| e.to_string())?);
        }
    }
    Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if validate_fetch_url(attempt.url().as_str()).is_ok() {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .default_headers(map)
        .build()
        .map_err(|e| e.to_string())
}

fn cookie_header_for_target(
    source: &Url,
    target: &Url,
    cookies: Option<&str>,
) -> Option<HeaderValue> {
    let source_host = source.host_str()?.to_ascii_lowercase();
    let target_host = target.host_str()?.to_ascii_lowercase();
    if source_host != target_host {
        return None;
    }
    let value = sanitize_header_value(cookies?.trim());
    if value.is_empty() {
        return None;
    }
    HeaderValue::from_str(&value).ok()
}

fn request_with_headers(
    client: &Client,
    source: &Url,
    target: &Url,
    headers: &HlsHeaders,
) -> RequestBuilder {
    let request = client.get(target.clone());
    match cookie_header_for_target(source, target, headers.cookies.as_deref()) {
        Some(cookie) => request.header(COOKIE, cookie),
        None => request,
    }
}

async fn read_bounded_response(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    if response.content_length().is_some_and(|size| size > max_bytes as u64) {
        return Err(format!("HTTP response exceeds {max_bytes} bytes"));
    }

    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("HTTP response exceeds {max_bytes} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn cancellation_requested(rx: &mut tokio::sync::watch::Receiver<bool>) -> bool {
    if *rx.borrow() {
        return true;
    }
    rx.changed().await.is_ok() && *rx.borrow()
}

fn pick_best_variant(variants: &[m3u8_rs::VariantStream]) -> Option<&m3u8_rs::VariantStream> {
    variants.iter().max_by_key(|v| v.bandwidth)
}

pub async fn process_hls_stream(
    app_handle: &AppHandle,
    download_id: i64,
    url: &str,
    save_path: &str,
    mut rx: tokio::sync::watch::Receiver<bool>,
    headers: HlsHeaders,
    db: Option<Database>,
) -> Result<(), String> {
    let client = build_client(&headers)?;
    let base_url = validate_fetch_url(url)?;

    let res = tokio::select! {
        _ = cancellation_requested(&mut rx) => return Err("Cancelled".into()),
        result = request_with_headers(&client, &base_url, &base_url, &headers).send() => {
            result.map_err(|e| e.to_string())?
        }
    };
    let bytes = tokio::select! {
        _ = cancellation_requested(&mut rx) => return Err("Cancelled".into()),
        result = read_bounded_response(res, MAX_PLAYLIST_BYTES) => result?,
    };

    let playlist = match m3u8_rs::parse_playlist_res(&bytes) {
        Ok(p) => p,
        Err(_) => return Err("Failed to parse m3u8 playlist".into()),
    };

    let mut segment_urls = Vec::new();
    // ponytail: capture variant bandwidth from the master playlist so we can
    // estimate total_size (BANDWIDTH × ΣEXTINF duration) for a smoother progress
    // bar. Previously total_size was `segment_count × last_segment_bytes`, which
    // wildly mis-estimated for variable-bitrate/ABR streams.
    let mut estimated_total_bytes: Option<u64> = None;

    match playlist {
        Playlist::MasterPlaylist(pl) => {
            let variant = pick_best_variant(&pl.variants)
                .ok_or_else(|| "No variants found in master playlist".to_string())?;
            // BANDWIDTH is bits/s (peak). With the total duration we can estimate bytes.
            if variant.bandwidth > 0 {
                estimated_total_bytes = variant.bandwidth.checked_div(8);
            }
            let variant_url = validate_fetch_url(
                base_url.join(&variant.uri).map_err(|e| e.to_string())?.as_str(),
            )?;
            let m_res = tokio::select! {
                _ = cancellation_requested(&mut rx) => return Err("Cancelled".into()),
                result = request_with_headers(&client, &base_url, &variant_url, &headers).send() => {
                    result.map_err(|e| e.to_string())?
                }
            };
            let m_bytes = tokio::select! {
                _ = cancellation_requested(&mut rx) => return Err("Cancelled".into()),
                result = read_bounded_response(m_res, MAX_PLAYLIST_BYTES) => result?,
            };
            match m3u8_rs::parse_playlist_res(&m_bytes) {
                Ok(Playlist::MediaPlaylist(m_pl)) => {
                    // Refine estimate: bandwidth(bps)/8 × total_duration(s).
                    if let Some(bw) = estimated_total_bytes.take() {
                        let total_dur: f64 = m_pl.segments.iter().map(|s| s.duration as f64).sum();
                        estimated_total_bytes = Some((bw as f64 * total_dur) as u64);
                    }
                    for seg in m_pl.segments {
                        segment_urls.push(validate_fetch_url(
                            variant_url.join(&seg.uri).map_err(|e| e.to_string())?.as_str(),
                        )?);
                    }
                }
                _ => return Err("Expected MediaPlaylist in variant".into()),
            }
        }
        Playlist::MediaPlaylist(pl) => {
            for seg in pl.segments {
                segment_urls.push(validate_fetch_url(
                    base_url.join(&seg.uri).map_err(|e| e.to_string())?.as_str(),
                )?);
            }
        }
    }

    if segment_urls.is_empty() {
        return Err("No segments found".into());
    }
    if segment_urls.len() > MAX_SEGMENTS {
        return Err(format!("Playlist contains too many segments: {}", segment_urls.len()));
    }

    let total_segments = segment_urls.len() as u64;
    let out_path = Path::new(save_path);
    // ponytail: temp dir lives under <data_dir>/downloads_temp/, NOT under the
    // save dir (Downloads). Crash leftovers there are swept by
    // cleanup_stale_temp_dirs on startup; previously those lived in Downloads and
    // were never cleaned (the sweep scanned the data dir root).
    let temp_root = crate::util::app_data_dir().join("downloads_temp");
    let _ = std::fs::create_dir_all(&temp_root);
    let temp_dir = temp_root.join(format!(
        "{}-{}.falcondm-temp",
        out_path.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));

    fs::create_dir_all(&temp_dir).await.map_err(|e| e.to_string())?;

    let _guard = TempDirGuard { path: temp_dir.clone() };

    let concurrency_limit = 10;
    let downloaded_bytes = Arc::new(AtomicU64::new(0));
    let completed_segs = Arc::new(AtomicU64::new(0));
    // ponytail: running max of total_size estimate. We never shrink it below the
    // actually-downloaded bytes (guard against >100% progress).
    let total_size_estimate = Arc::new(AtomicU64::new(estimated_total_bytes.unwrap_or(0)));
    let started = Instant::now();

    let segment_paths: Vec<PathBuf> = stream::iter(segment_urls.into_iter().enumerate())
        .map(|(idx, seg_url)| {
            let client = client.clone();
            let temp_dir = temp_dir.clone();
            let mut rx_clone = rx.clone();
            let downloaded_bytes = downloaded_bytes.clone();
            let completed_segs = completed_segs.clone();
            let total_size_estimate = total_size_estimate.clone();
            let app_handle = app_handle.clone();
            let source_url = base_url.clone();
            let headers = headers.clone();
            async move {
                if *rx_clone.borrow() {
                    return Err("Cancelled".to_string());
                }

                let seg_path = temp_dir.join(format!("seg_{:05}.ts", idx));
                // ponytail: cancel-safe segment download. If cancellation is
                // signalled mid-fetch, abort the request instead of waiting for
                // the whole segment to arrive (previous code always completed the
                // in-flight request even after cancel).
                //
                // tokio::select! keeps each branch's output around, and the watch
                // ponytail: transient-retry segment download. A single TLS blip
                // or a 502 from a CDN edge used to fail the whole HLS job; now we
                // retry up to 3 times with backoff. 4xx (the segment genuinely
                // doesn't exist / forbidden) is fatal and not retried.
                const SEG_MAX_ATTEMPTS: u32 = 3;
                let mut bytes: Vec<u8> = Vec::new();
                let mut last_err: Option<String> = None;
                for attempt in 0..SEG_MAX_ATTEMPTS {
                    if *rx_clone.borrow() {
                        return Err("Cancelled".to_string());
                    }
                    let response = tokio::select! {
                        _ = cancellation_requested(&mut rx_clone) => {
                            return Err("Cancelled".to_string());
                        }
                        result = request_with_headers(&client, &source_url, &seg_url, &headers).send() => result,
                    };
                    match response {
                        Ok(resp) => {
                            let status = resp.status();
                            if status.is_server_error() || status.as_u16() == 429 {
                                last_err = Some(format!("segment HTTP {}", status.as_u16()));
                                if attempt + 1 < SEG_MAX_ATTEMPTS {
                                    let backoff_ms = 200u64 * (1 << attempt); // 200, 400
                                    tokio::select! {
                                        _ = cancellation_requested(&mut rx_clone) => {
                                            return Err("Cancelled".to_string());
                                        }
                                        _ = tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)) => {}
                                    }
                                    continue;
                                }
                            } else if !status.is_success() {
                                // 4xx (except 429) — permanent, don't retry.
                                return Err(format!("segment HTTP {}", status.as_u16()));
                            } else {
                                let body = tokio::select! {
                                    _ = cancellation_requested(&mut rx_clone) => {
                                        return Err("Cancelled".to_string());
                                    }
                                    result = read_bounded_response(resp, MAX_SEGMENT_BYTES) => result,
                                };
                                match body {
                                    Ok(b) => {
                                        bytes = b;
                                        last_err = None;
                                        break;
                                    }
                                    Err(e) => {
                                        last_err = Some(e.to_string());
                                        if attempt + 1 < SEG_MAX_ATTEMPTS {
                                            tokio::select! {
                                                _ = cancellation_requested(&mut rx_clone) => {
                                                    return Err("Cancelled".to_string());
                                                }
                                                _ = tokio::time::sleep(std::time::Duration::from_millis(200 * (1 << attempt))) => {}
                                            }
                                            continue;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            last_err = Some(e.to_string());
                            if attempt + 1 < SEG_MAX_ATTEMPTS {
                                tokio::select! {
                                    _ = cancellation_requested(&mut rx_clone) => {
                                        return Err("Cancelled".to_string());
                                    }
                                    _ = tokio::time::sleep(std::time::Duration::from_millis(200 * (1 << attempt))) => {}
                                }
                                continue;
                            }
                        }
                    }
                }
                if let Some(err) = last_err {
                    return Err(format!("segment {idx} failed after retries: {err}"));
                }

                let seg_len = bytes.len() as u64;
                let previous = downloaded_bytes
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                        current.checked_add(seg_len).filter(|next| *next <= MAX_OUTPUT_BYTES)
                    })
                    .map_err(|_| "HLS output exceeds maximum size".to_string())?;
                let bytes_so_far = previous + seg_len;
                let done = completed_segs.fetch_add(1, Ordering::Relaxed) + 1;
                // Progressive total estimate: refine from observed bytes.
                // average_segment_size = downloaded / done; total ≈ avg × total_segments.
                let avg = bytes_so_far / done;
                let projected = avg.saturating_mul(total_segments);
                let cur = total_size_estimate.load(Ordering::Relaxed);
                let new_total = projected.max(cur).max(bytes_so_far);
                if new_total > cur {
                    total_size_estimate.store(new_total, Ordering::Relaxed);
                }
                let elapsed = started.elapsed().as_secs_f64().max(0.001);
                let speed = bytes_so_far as f64 / elapsed;

                let _ = app_handle.emit(
                    "download-progress",
                    HlsProgressEvent {
                        id: download_id,
                        downloaded_size: bytes_so_far,
                        total_size: new_total,
                        speed,
                        status: "Downloading".into(),
                        connections: concurrency_limit as u32,
                    },
                );

                let mut file = fs::File::create(&seg_path).await.map_err(|e| e.to_string())?;
                file.write_all(&bytes).await.map_err(|e| e.to_string())?;
                Ok::<PathBuf, String>(seg_path)
            }
        })
        .buffer_unordered(concurrency_limit)
        .try_collect::<Vec<PathBuf>>()
        .await?;

    let mut segment_paths = segment_paths;
    segment_paths.sort();

    let downloaded_final = downloaded_bytes.load(Ordering::Relaxed);
    let total_final = total_size_estimate.load(Ordering::Relaxed).max(downloaded_final);
    let _ = app_handle.emit(
        "download-progress",
        HlsProgressEvent {
            id: download_id,
            downloaded_size: downloaded_final,
            total_size: total_final,
            speed: 0.0,
            status: "Merging".into(),
            connections: 0,
        },
    );
    if let Some(ref db) = db {
        let _ = db.update_download_progress(
            download_id,
            downloaded_final,
            0.0,
            &DownloadStatus::Merging,
        );
    }

    let list_path = temp_dir.join("list.txt");
    let mut list_file = fs::File::create(&list_path).await.map_err(|e| e.to_string())?;
    for p in &segment_paths {
        // ponytail: segment filenames are always set (seg_NNNNN.ts), so unwrap is safe here.
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        list_file
            .write_all(format!("file '{}'\n", name).as_bytes())
            .await
            .map_err(|e| e.to_string())?;
    }

    // ponytail: to_str() is None on non-UTF8 paths (e.g. some internationalized
    // save dirs). Fall back to an explicit error instead of panicking.
    let list_path_str =
        list_path.to_str().ok_or_else(|| "HLS temp path is not valid UTF-8".to_string())?;
    let temp_output = temp_dir.join("output.mp4");
    let temp_output_str =
        temp_output.to_str().ok_or_else(|| "HLS output path is not valid UTF-8".to_string())?;

    let (mut events, child) = app_handle
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| format!("Failed to find ffmpeg sidecar: {}", e))?
        .args([
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list_path_str,
            "-c",
            "copy",
            "-bsf:a",
            "aac_adtstoasc",
            temp_output_str,
        ])
        .spawn()
        .map_err(|e| format!("Failed to execute FFmpeg: {}", e))?;

    let mut ffmpeg_stderr = Vec::new();
    loop {
        tokio::select! {
            _ = cancellation_requested(&mut rx) => {
                let _ = child.kill();
                while let Some(event) = events.recv().await {
                    if matches!(event, CommandEvent::Terminated(_)) {
                        break;
                    }
                }
                return Err("Cancelled".into());
            }
            event = events.recv() => {
                match event {
                    Some(CommandEvent::Stderr(bytes)) => {
                        if ffmpeg_stderr.len() < 8_000 {
                            ffmpeg_stderr.extend_from_slice(&bytes[..bytes.len().min(8_000 - ffmpeg_stderr.len())]);
                        }
                    }
                    Some(CommandEvent::Terminated(payload)) => {
                        if payload.code != Some(0) {
                            return Err(format!("FFmpeg failed: {}", String::from_utf8_lossy(&ffmpeg_stderr)));
                        }
                        break;
                    }
                    Some(CommandEvent::Error(error)) => return Err(format!("FFmpeg failed: {error}")),
                    Some(CommandEvent::Stdout(_)) => {}
                    Some(_) => {}
                    None => return Err("FFmpeg ended without exit status".into()),
                }
            }
        }
    }

    crate::util::validate_completed_file(&temp_output)?;
    let destination = PathBuf::from(save_path);
    tokio::task::spawn_blocking(move || {
        crate::util::copy_file_exclusive(&temp_output, &destination)
    })
    .await
    .map_err(|e| format!("HLS output move task failed: {e}"))??;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dummy_hls_parse() {
        let m3u8 = b"#EXTM3U\n#EXT-X-VERSION:3\n#EXTINF:10.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        let parsed = m3u8_rs::parse_playlist_res(m3u8);
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_pick_best_variant_bandwidth() {
        // Smoke: empty → None
        assert!(pick_best_variant(&[]).is_none());
    }

    #[test]
    fn cookies_are_not_sent_to_unrelated_segment_host() {
        let source = Url::parse("https://media.example.com/master.m3u8").unwrap();
        let same_host = Url::parse("https://media.example.com/seg.ts").unwrap();
        let other_host = Url::parse("https://cdn.example.net/seg.ts").unwrap();
        assert!(cookie_header_for_target(&source, &same_host, Some("sid=abc")).is_some());
        assert!(cookie_header_for_target(&source, &other_host, Some("sid=abc")).is_none());
    }

    #[tokio::test]
    async fn cancellation_receiver_reports_cancelled_state() {
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        tx.send(true).unwrap();
        assert!(cancellation_requested(&mut rx).await);
    }
}
