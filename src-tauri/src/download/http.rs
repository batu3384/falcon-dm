use crate::storage::{models::DownloadStatus, Database};
use crate::util::{
    copy_file_exclusive, resolve_public_addresses_async, sanitize_header_value,
    validate_fetch_url_async,
};
use futures::StreamExt;
use reqwest::header::{COOKIE, REFERER, USER_AGENT};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use url::Url;

const MAX_REDIRECTS: usize = 5;
const MAX_HTTP_BYTES: u64 = 16 * 1024 * 1024 * 1024;

struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Clone, Default)]
pub struct HttpOptions {
    pub proxy: Option<String>,
    pub speed_limit_kbps: u32,
}

#[derive(Clone, Default)]
pub struct HttpHeaders {
    pub cookies: Option<String>,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
    pub options: HttpOptions,
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str().map(str::to_ascii_lowercase)
            == right.host_str().map(str::to_ascii_lowercase)
        && left.port_or_known_default() == right.port_or_known_default()
}

async fn redirect_target(base: &Url, location: &str) -> Result<Url, String> {
    let target = base.join(location).map_err(|e| format!("Invalid redirect: {e}"))?;
    validate_fetch_url_async(target.as_str()).await
}

async fn pinned_client(url: &Url, options: &HttpOptions) -> Result<Client, String> {
    let addresses = resolve_public_addresses_async(url).await?;
    let mut builder = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60));
    if url.host_str().and_then(|host| host.parse::<std::net::IpAddr>().ok()).is_none() {
        let host = url.host_str().ok_or_else(|| "URL has no host".to_string())?;
        builder = builder.resolve(host, addresses[0]);
    }
    if let Some(proxy) = options.proxy.as_deref() {
        builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|e| e.to_string())?);
    }
    builder.build().map_err(|e| format!("HTTP client build failed: {e}"))
}

fn add_request_headers(
    request: reqwest::RequestBuilder,
    initial: &Url,
    current: &Url,
    headers: &HttpHeaders,
) -> reqwest::RequestBuilder {
    let same_origin = same_origin(initial, current);
    let mut request = request;
    if same_origin && initial.scheme() == "https" {
        if let Some(value) = headers.cookies.as_deref().map(sanitize_header_value) {
            if !value.is_empty() {
                request = request.header(COOKIE, value);
            }
        }
        if let Some(value) = headers.referrer.as_deref().map(sanitize_header_value) {
            if !value.is_empty() {
                request = request.header(REFERER, value);
            }
        }
    }
    if let Some(value) = headers.user_agent.as_deref().map(sanitize_header_value) {
        if !value.is_empty() {
            request = request.header(USER_AGENT, value);
        }
    }
    request
}

fn temporary_path(destination: &Path, download_id: i64) -> Result<PathBuf, String> {
    let parent = destination.parent().ok_or_else(|| "Invalid output path".to_string())?;
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Invalid output filename".to_string())?;
    Ok(parent.join(format!(".{name}.{download_id}.falcon.part")))
}

pub async fn process_http(
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
        return Err("Destination file already exists".into());
    }
    let parent = destination.parent().ok_or_else(|| "Invalid output path".to_string())?;
    fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    let temp = temporary_path(&destination, download_id)?;
    let _ = fs::remove_file(&temp).await;
    let mut current = initial.clone();
    let mut response = None;

    for redirect_count in 0..=MAX_REDIRECTS {
        if *cancel.borrow() {
            return Err("Cancelled".into());
        }
        let client = pinned_client(&current, &headers.options).await?;
        let request =
            add_request_headers(client.get(current.clone()), &initial, &current, &headers);
        let candidate = tokio::select! {
            changed = cancel.changed() => {
                let _ = changed;
                return Err("Cancelled".into());
            }
            result = request.send() => {
                result.map_err(|e| format!("HTTP download failed: {e}"))?
            }
        };
        if candidate.status().is_redirection() {
            if redirect_count == MAX_REDIRECTS {
                return Err("Too many HTTP redirects".into());
            }
            let location = candidate
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "HTTP redirect has no valid location".to_string())?;
            current = redirect_target(&current, location).await?;
            continue;
        }
        response = Some(candidate);
        break;
    }

    let response = response.ok_or_else(|| "HTTP response unavailable".to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    if response.content_length().is_some_and(|size| size > MAX_HTTP_BYTES) {
        return Err("HTTP response exceeds maximum download size".into());
    }

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .await
        .map_err(|e| e.to_string())?;
    let _temp_guard = TempFileGuard(temp.clone());
    let total = response.content_length().unwrap_or(0);
    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;
    let started = Instant::now();
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
            let _ = fs::remove_file(&temp).await;
            return Err("HTTP response exceeds maximum download size".into());
        }
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        if headers.options.speed_limit_kbps > 0 {
            let target = Duration::from_secs_f64(
                downloaded as f64 / (headers.options.speed_limit_kbps as f64 * 1024.0),
            );
            if let Some(wait) = target.checked_sub(started.elapsed()) {
                tokio::select! {
                    changed = cancel.changed() => {
                        let _ = changed;
                        drop(file);
                        return Err("Cancelled".into());
                    }
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        }
        let speed = downloaded as f64 / started.elapsed().as_secs_f64().max(0.001);
        if let Some(ref db) = db {
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
                "connections": 1
            }),
        );
    }
    file.sync_all().await.map_err(|e| e.to_string())?;
    drop(file);
    if *cancel.borrow() {
        let _ = fs::remove_file(&temp).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn redirect_target_rejects_private_hosts() {
        let base = Url::parse("https://8.8.8.8/file").unwrap();
        assert!(redirect_target(&base, "http://127.0.0.1/secret").await.is_err());
        assert_eq!(redirect_target(&base, "/next").await.unwrap().as_str(), "https://8.8.8.8/next");
    }
}
