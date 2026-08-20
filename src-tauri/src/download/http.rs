use crate::download::http_client::{
    add_request_headers, resolve_resource, split_byte_ranges, with_pinned_clients,
    ResolvedResource, MAX_HTTP_BYTES, MAX_REDIRECTS, MIN_PARALLEL_BYTES,
};
use crate::storage::{models::DownloadStatus, Database};
use crate::util::{copy_file_exclusive, validate_fetch_url_async};
use futures::stream::{self, StreamExt};
use reqwest::header::{CONTENT_RANGE, LOCATION, RANGE};
use reqwest::StatusCode;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use url::Url;

pub use crate::download::http_client::{HttpHeaders, HttpOptions};

struct TempFileGuard {
    path: PathBuf,
    delete: bool,
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.delete {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

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

fn temporary_path(destination: &Path, download_id: i64) -> Result<PathBuf, String> {
    let parent = destination.parent().ok_or_else(|| "Invalid output path".to_string())?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Invalid output filename".to_string())?;
    Ok(parent.join(format!(".{name}.{download_id}.falcon.part")))
}

fn segment_dir(destination: &Path, download_id: i64) -> Result<PathBuf, String> {
    let parent = destination.parent().ok_or_else(|| "Invalid output path".to_string())?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Invalid output filename".to_string())?;
    Ok(parent.join(format!(".{name}.{download_id}.falcon.segments")))
}

pub fn part_path_for(save_dir: &str, filename: &str, download_id: i64) -> Option<PathBuf> {
    let destination = crate::util::full_file_path(save_dir, filename);
    temporary_path(&destination, download_id).ok()
}

fn total_from_content_range(
    header: Option<&reqwest::header::HeaderValue>,
    resume_from: u64,
    content_length: Option<u64>,
) -> u64 {
    if let Some(total) = header
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit('/').next())
        .and_then(|value| value.parse::<u64>().ok())
    {
        return total;
    }
    resume_from.saturating_add(content_length.unwrap_or(0))
}

async fn redirect_target(base: &Url, location: &str) -> Result<Url, String> {
    let target = base.join(location).map_err(|e| format!("Invalid redirect: {e}"))?;
    validate_fetch_url_async(target.as_str()).await
}

fn emit_progress(
    app_handle: &AppHandle,
    download_id: i64,
    downloaded: u64,
    total: u64,
    speed: f64,
    connections: u32,
    db: Option<&Database>,
) {
    if let Some(db) = db {
        let _ = db.update_download_progress(
            download_id,
            downloaded,
            speed,
            &DownloadStatus::Downloading,
        );
    }
    let _ = app_handle.emit(
        "download-progress",
        serde_json::json!({
            "id": download_id,
            "downloaded_size": downloaded,
            "total_size": total.max(downloaded),
            "speed": speed,
            "status": "Downloading",
            "connections": connections
        }),
    );
}

pub(crate) fn range_byte_length(start: u64, end: u64) -> u64 {
    end.saturating_sub(start).saturating_add(1)
}

struct SpeedGate {
    limit_kbps: u32,
    started: Instant,
    downloaded: AtomicU64,
}

impl SpeedGate {
    fn new(limit_kbps: u32) -> Self {
        Self { limit_kbps, started: Instant::now(), downloaded: AtomicU64::new(0) }
    }

    fn add_bytes(&self, bytes: u64) {
        self.downloaded.fetch_add(bytes, Ordering::SeqCst);
    }

    async fn throttle(&self, cancel: &watch::Receiver<bool>) -> Result<(), String> {
        if self.limit_kbps == 0 {
            return Ok(());
        }
        let downloaded = self.downloaded.load(Ordering::SeqCst);
        let target = Duration::from_secs_f64(downloaded as f64 / (self.limit_kbps as f64 * 1024.0));
        if let Some(wait) = target.checked_sub(self.started.elapsed()) {
            if *cancel.borrow() {
                return Err("Cancelled".into());
            }
            tokio::time::sleep(wait).await;
            if *cancel.borrow() {
                return Err("Cancelled".into());
            }
        }
        Ok(())
    }
}

async fn apply_speed_limit(
    speed_limit_kbps: u32,
    baseline: u64,
    downloaded: u64,
    started: Instant,
    cancel: &watch::Receiver<bool>,
) -> Result<(), String> {
    if speed_limit_kbps == 0 {
        return Ok(());
    }
    let written = downloaded.saturating_sub(baseline) as f64;
    let target = Duration::from_secs_f64(written / (speed_limit_kbps as f64 * 1024.0));
    if let Some(wait) = target.checked_sub(started.elapsed()) {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        tokio::time::sleep(wait).await;
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
    }
    Ok(())
}

async fn stream_segment_to_file(
    response: reqwest::Response,
    segment_path: &Path,
    expected_bytes: u64,
    cancel: &watch::Receiver<bool>,
    speed_gate: &SpeedGate,
) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(segment_path)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let mut written = 0u64;
    while let Some(chunk) = stream.next().await {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let chunk = chunk.map_err(|e| format!("HTTP segment read failed: {e}"))?;
        written = written
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "HTTP segment size overflow".to_string())?;
        if written > expected_bytes {
            return Err(format!(
                "HTTP segment exceeds range ({written} bytes, expected {expected_bytes})"
            ));
        }
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        speed_gate.add_bytes(chunk.len() as u64);
        speed_gate.throttle(cancel).await?;
    }
    if written != expected_bytes {
        return Err(format!(
            "HTTP segment size mismatch: expected {expected_bytes}, got {written}"
        ));
    }
    file.sync_all().await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn download_http_segment(
    final_url: &Url,
    initial: &Url,
    headers: &HttpHeaders,
    proxy: Option<&str>,
    start: u64,
    end: u64,
    segment_path: &Path,
    cancel: &watch::Receiver<bool>,
    speed_gate: &SpeedGate,
) -> Result<(), String> {
    let expected_bytes = range_byte_length(start, end);
    let range_header = format!("bytes={start}-{end}");
    let final_url = final_url.clone();
    let initial = initial.clone();
    let headers = headers.clone();
    let response = with_pinned_clients(&final_url, proxy, Duration::from_secs(120), |client| {
        let final_url = final_url.clone();
        let initial = initial.clone();
        let headers = headers.clone();
        let range_header = range_header.clone();
        async move {
            let request =
                add_request_headers(client.get(final_url.clone()), &initial, &final_url, &headers)
                    .header(RANGE, range_header);
            request.send().await.map_err(|e| format!("HTTP segment failed: {e}"))
        }
    })
    .await?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(format!("HTTP segment {}", response.status().as_u16()));
    }
    stream_segment_to_file(response, segment_path, expected_bytes, cancel, speed_gate).await
}

async fn process_http_parallel(
    app_handle: &AppHandle,
    download_id: i64,
    resource: ResolvedResource,
    out_path: &str,
    cancel: watch::Receiver<bool>,
    headers: HttpHeaders,
    db: Option<Database>,
) -> Result<(), String> {
    let destination = PathBuf::from(out_path);
    let parent = destination.parent().ok_or_else(|| "Invalid output path".to_string())?;
    fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    let temp = temporary_path(&destination, download_id)?;
    let segments_root = segment_dir(&destination, download_id)?;
    let _segment_guard = TempDirGuard { path: segments_root.clone() };
    fs::create_dir_all(&segments_root).await.map_err(|e| e.to_string())?;

    let connections = headers.max_connections.clamp(1, 16);
    let ranges = split_byte_ranges(resource.total_bytes, connections);
    let segment_count = ranges.len();
    let downloaded_bytes = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let proxy = headers.options.proxy.clone();
    let initial = resource.initial.clone();
    let final_url = resource.final_url.clone();
    let speed_gate = Arc::new(SpeedGate::new(headers.options.speed_limit_kbps));

    let results: Vec<Result<(), String>> = stream::iter(ranges.into_iter().enumerate())
        .map(|(index, (start, end))| {
            let app_handle = app_handle.clone();
            let headers = headers.clone();
            let proxy = proxy.clone();
            let initial = initial.clone();
            let final_url = final_url.clone();
            let segments_root = segments_root.clone();
            let downloaded_bytes = downloaded_bytes.clone();
            let db = db.clone();
            let cancel = cancel.clone();
            let speed_gate = speed_gate.clone();
            async move {
                if *cancel.borrow() {
                    return Err("Cancelled".into());
                }
                let segment_path = segments_root.join(format!("seg_{index:02}"));
                download_http_segment(
                    &final_url,
                    &initial,
                    &headers,
                    proxy.as_deref(),
                    start,
                    end,
                    &segment_path,
                    &cancel,
                    &speed_gate,
                )
                .await?;

                if *cancel.borrow() {
                    return Err("Cancelled".into());
                }
                let chunk_len = range_byte_length(start, end);
                let downloaded =
                    downloaded_bytes.fetch_add(chunk_len, Ordering::SeqCst) + chunk_len;
                let speed = downloaded as f64 / started.elapsed().as_secs_f64().max(0.001);
                emit_progress(
                    &app_handle,
                    download_id,
                    downloaded,
                    resource.total_bytes,
                    speed,
                    connections as u32,
                    db.as_ref(),
                );
                Ok(())
            }
        })
        .buffer_unordered(connections)
        .collect()
        .await;

    for result in results {
        result?;
    }
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }

    let mut output = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)
        .await
        .map_err(|e| e.to_string())?;
    let mut guard = TempFileGuard { path: temp.clone(), delete: false };
    for index in 0..segment_count {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let segment_path = segments_root.join(format!("seg_{index:02}"));
        if !segment_path.exists() {
            guard.delete = true;
            return Err("Missing HTTP segment".into());
        }
        let bytes = fs::read(&segment_path).await.map_err(|e| e.to_string())?;
        output.write_all(&bytes).await.map_err(|e| e.to_string())?;
    }
    output.sync_all().await.map_err(|e| e.to_string())?;
    drop(output);

    let temp_for_validation = temp.clone();
    tokio::task::spawn_blocking(move || crate::util::validate_completed_file(&temp_for_validation))
        .await
        .map_err(|e| e.to_string())??;
    let destination_for_move = destination.clone();
    tokio::task::spawn_blocking(move || copy_file_exclusive(&temp, &destination_for_move))
        .await
        .map_err(|e| e.to_string())??;
    Ok(())
}

async fn process_http_single(
    app_handle: &AppHandle,
    download_id: i64,
    url: &str,
    out_path: &str,
    mut cancel: watch::Receiver<bool>,
    headers: HttpHeaders,
    db: Option<Database>,
) -> Result<(), String> {
    let initial = validate_fetch_url_async(url).await?;
    let destination = PathBuf::from(out_path);
    if fs::try_exists(&destination).await.map_err(|e| e.to_string())? {
        let dest = destination.clone();
        match tokio::task::spawn_blocking(move || crate::util::validate_completed_file(&dest)).await
        {
            Ok(Ok(_)) => return Ok(()),
            _ => {
                let _ = fs::remove_file(&destination).await;
            }
        }
    }
    let parent = destination.parent().ok_or_else(|| "Invalid output path".to_string())?;
    fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    let temp = temporary_path(&destination, download_id)?;
    let resume_from =
        fs::metadata(&temp).await.ok().map(|meta| meta.len()).filter(|len| *len > 0).unwrap_or(0);
    let mut current = initial.clone();
    let mut response = None;
    let mut restarted = false;
    let proxy = headers.options.proxy.as_deref();

    for redirect_count in 0..=MAX_REDIRECTS {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let headers_for_request = headers.clone();
        let current_for_request = current.clone();
        let candidate = with_pinned_clients(&current, proxy, Duration::from_secs(60), |client| {
            let current = current_for_request.clone();
            let initial = initial.clone();
            let headers = headers_for_request.clone();
            async move {
                let mut request =
                    add_request_headers(client.get(current.clone()), &initial, &current, &headers);
                if resume_from > 0 && !restarted {
                    request = request.header(RANGE, format!("bytes={resume_from}-"));
                }
                request.send().await.map_err(|e| format!("HTTP download failed: {e}"))
            }
        })
        .await?;

        if candidate.status().is_redirection() {
            if redirect_count == MAX_REDIRECTS {
                return Err("Too many HTTP redirects".into());
            }
            let location = candidate
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "HTTP redirect has no valid location".to_string())?;
            current = redirect_target(&current, location).await?;
            continue;
        }
        response = Some(candidate);
        break;
    }

    let response = response.ok_or_else(|| "HTTP response unavailable".to_string())?;
    let status = response.status();
    if status == StatusCode::RANGE_NOT_SATISFIABLE && resume_from > 0 {
        let temp_for_validation = temp.clone();
        tokio::task::spawn_blocking(move || {
            crate::util::validate_completed_file(&temp_for_validation)
        })
        .await
        .map_err(|e| e.to_string())??;
        let destination_for_move = destination.clone();
        tokio::task::spawn_blocking(move || copy_file_exclusive(&temp, &destination_for_move))
            .await
            .map_err(|e| e.to_string())??;
        return Ok(());
    }
    let resume_ok = resume_from > 0 && status == StatusCode::PARTIAL_CONTENT;
    if resume_from > 0 && status.is_success() && !resume_ok {
        restarted = true;
    }
    if !status.is_success() && status != StatusCode::PARTIAL_CONTENT {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let content_length = response.content_length();
    let total = if resume_ok {
        total_from_content_range(response.headers().get(CONTENT_RANGE), resume_from, content_length)
    } else {
        content_length.unwrap_or(0)
    };
    if total > MAX_HTTP_BYTES
        || content_length.is_some_and(|size| {
            let absolute = if resume_ok { resume_from.saturating_add(size) } else { size };
            absolute > MAX_HTTP_BYTES
        })
    {
        return Err("HTTP response exceeds maximum download size".into());
    }

    if restarted {
        let _ = fs::remove_file(&temp).await;
    }
    let mut file = if resume_ok {
        fs::OpenOptions::new()
            .write(true)
            .append(true)
            .open(&temp)
            .await
            .map_err(|e| e.to_string())?
    } else {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .await
            .map_err(|e| e.to_string())?
    };
    let mut guard = TempFileGuard { path: temp.clone(), delete: false };
    let mut stream = response.bytes_stream();
    let mut downloaded = if resume_ok { resume_from } else { 0 };
    let started = Instant::now();
    let baseline = downloaded;
    loop {
        let next_chunk = tokio::select! {
            changed = cancel.changed() => {
                let _ = changed;
                drop(file);
                return Err("Cancelled".into());
            }
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = next_chunk else {
            break;
        };
        let chunk = chunk.map_err(|e| e.to_string())?;
        downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| "HTTP download size overflow".to_string())?;
        if downloaded > MAX_HTTP_BYTES {
            drop(file);
            guard.delete = true;
            return Err("HTTP response exceeds maximum download size".into());
        }
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        apply_speed_limit(headers.options.speed_limit_kbps, baseline, downloaded, started, &cancel)
            .await?;
        let speed =
            downloaded.saturating_sub(baseline) as f64 / started.elapsed().as_secs_f64().max(0.001);
        emit_progress(
            app_handle,
            download_id,
            downloaded,
            total.max(downloaded),
            speed,
            1,
            db.as_ref(),
        );
    }
    file.sync_all().await.map_err(|e| e.to_string())?;
    drop(file);
    if *cancel.borrow() {
        return Err("Cancelled".into());
    }
    let temp_for_validation = temp.clone();
    tokio::task::spawn_blocking(move || crate::util::validate_completed_file(&temp_for_validation))
        .await
        .map_err(|e| e.to_string())??;
    let destination_for_move = destination.clone();
    tokio::task::spawn_blocking(move || copy_file_exclusive(&temp, &destination_for_move))
        .await
        .map_err(|e| e.to_string())??;
    Ok(())
}

pub async fn process_http(
    app_handle: &AppHandle,
    download_id: i64,
    url: &str,
    out_path: &str,
    cancel: watch::Receiver<bool>,
    headers: HttpHeaders,
    db: Option<Database>,
) -> Result<(), String> {
    let destination = PathBuf::from(out_path);
    let temp = temporary_path(&destination, download_id)?;
    let resume_from =
        fs::metadata(&temp).await.ok().map(|meta| meta.len()).filter(|len| *len > 0).unwrap_or(0);
    if resume_from > 0 {
        return process_http_single(app_handle, download_id, url, out_path, cancel, headers, db)
            .await;
    }

    let initial = validate_fetch_url_async(url).await?;
    let connections = headers.max_connections.clamp(1, 16);
    if connections > 1 {
        if let Ok(resource) = resolve_resource(&initial, &headers).await {
            if resource.accepts_ranges
                && resource.total_bytes >= MIN_PARALLEL_BYTES
                && resource.total_bytes > 0
            {
                match process_http_parallel(
                    app_handle,
                    download_id,
                    resource,
                    out_path,
                    cancel.clone(),
                    headers.clone(),
                    db.clone(),
                )
                .await
                {
                    Ok(()) => return Ok(()),
                    Err(err) => {
                        log::warn!("parallel HTTP failed for {download_id}, falling back to single connection: {err}");
                    }
                }
            }
        }
    }

    process_http_single(app_handle, download_id, url, out_path, cancel, headers, db).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn redirect_target_rejects_private_hosts() {
        let base = Url::parse("https://8.8.8.8/file").unwrap();
        assert!(redirect_target(&base, "http://127.0.0.1/secret").await.is_err());
        assert_eq!(redirect_target(&base, "/next").await.unwrap().as_str(), "https://8.8.8.8/next");
    }

    #[test]
    fn content_range_total_prefers_header() {
        let value = reqwest::header::HeaderValue::from_static("bytes 100-199/1000");
        assert_eq!(total_from_content_range(Some(&value), 100, Some(100)), 1000);
        assert_eq!(total_from_content_range(None, 50, Some(10)), 60);
    }

    #[test]
    fn range_byte_length_matches_inclusive_range() {
        assert_eq!(range_byte_length(0, 3), 4);
        assert_eq!(range_byte_length(4, 6), 3);
        assert_eq!(range_byte_length(100, 100), 1);
    }

    #[test]
    fn part_path_uses_download_id() {
        let path = part_path_for("/tmp", "movie.mp4", 7).unwrap();
        assert!(path.ends_with(".movie.mp4.7.falcon.part"));
    }
}
