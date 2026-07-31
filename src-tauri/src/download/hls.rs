use crate::storage::{models::DownloadStatus, Database};
use crate::util::sanitize_header_value;
use futures::stream::{self, StreamExt};
use m3u8_rs::Playlist;
use reqwest::header::{HeaderMap, HeaderValue, COOKIE, REFERER, USER_AGENT};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter};
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
    if let Some(ref c) = headers.cookies {
        let v = sanitize_header_value(c);
        if !v.is_empty() {
            map.insert(
                COOKIE,
                HeaderValue::from_str(&v).map_err(|e| e.to_string())?,
            );
        }
    }
    if let Some(ref r) = headers.referrer {
        let v = sanitize_header_value(r);
        if !v.is_empty() {
            map.insert(
                REFERER,
                HeaderValue::from_str(&v).map_err(|e| e.to_string())?,
            );
        }
    }
    if let Some(ref ua) = headers.user_agent {
        let v = sanitize_header_value(ua);
        if !v.is_empty() {
            map.insert(
                USER_AGENT,
                HeaderValue::from_str(&v).map_err(|e| e.to_string())?,
            );
        }
    }
    Client::builder()
        .default_headers(map)
        .build()
        .map_err(|e| e.to_string())
}

fn pick_best_variant<'a>(
    variants: &'a [m3u8_rs::VariantStream],
) -> Option<&'a m3u8_rs::VariantStream> {
    variants.iter().max_by_key(|v| v.bandwidth)
}

pub async fn process_hls_stream(
    app_handle: &AppHandle,
    download_id: i64,
    url: &str,
    save_path: &str,
    rx: tokio::sync::watch::Receiver<bool>,
    headers: HlsHeaders,
    db: Option<Database>,
) -> Result<(), String> {
    let client = build_client(&headers)?;
    let base_url = Url::parse(url).map_err(|e| e.to_string())?;

    let res = client.get(url).send().await.map_err(|e| e.to_string())?;
    let bytes = res.bytes().await.map_err(|e| e.to_string())?;

    let playlist = match m3u8_rs::parse_playlist_res(&bytes) {
        Ok(p) => p,
        Err(_) => return Err("Failed to parse m3u8 playlist".into()),
    };

    let mut segment_urls = Vec::new();

    match playlist {
        Playlist::MasterPlaylist(pl) => {
            let variant = pick_best_variant(&pl.variants)
                .ok_or_else(|| "No variants found in master playlist".to_string())?;
            let variant_url = base_url.join(&variant.uri).map_err(|e| e.to_string())?;
            let m_res = client
                .get(variant_url.clone())
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let m_bytes = m_res.bytes().await.map_err(|e| e.to_string())?;
            match m3u8_rs::parse_playlist_res(&m_bytes) {
                Ok(Playlist::MediaPlaylist(m_pl)) => {
                    for seg in m_pl.segments {
                        segment_urls
                            .push(variant_url.join(&seg.uri).map_err(|e| e.to_string())?);
                    }
                }
                _ => return Err("Expected MediaPlaylist in variant".into()),
            }
        }
        Playlist::MediaPlaylist(pl) => {
            for seg in pl.segments {
                segment_urls.push(base_url.join(&seg.uri).map_err(|e| e.to_string())?);
            }
        }
    }

    if segment_urls.is_empty() {
        return Err("No segments found".into());
    }

    let total_segments = segment_urls.len() as u64;
    let out_path = Path::new(save_path);
    let parent = out_path.parent().unwrap_or_else(|| Path::new("."));
    let temp_dir = parent.join(format!(
        "{}-{}.falcondm-temp",
        out_path.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));

    fs::create_dir_all(&temp_dir)
        .await
        .map_err(|e| e.to_string())?;

    let _guard = TempDirGuard {
        path: temp_dir.clone(),
    };

    let concurrency_limit = 10;
    let downloaded_bytes = Arc::new(AtomicU64::new(0));
    let completed_segs = Arc::new(AtomicU64::new(0));
    let started = Instant::now();

    let segment_paths: Vec<PathBuf> = stream::iter(segment_urls.into_iter().enumerate())
        .map(|(idx, seg_url)| {
            let client = client.clone();
            let temp_dir = temp_dir.clone();
            let rx_clone = rx.clone();
            let downloaded_bytes = downloaded_bytes.clone();
            let completed_segs = completed_segs.clone();
            let app_handle = app_handle.clone();
            async move {
                if *rx_clone.borrow() {
                    return Err("Cancelled".to_string());
                }

                let seg_path = temp_dir.join(format!("seg_{:05}.ts", idx));
                let res = client
                    .get(seg_url)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let bytes = res.bytes().await.map_err(|e| e.to_string())?;

                if *rx_clone.borrow() {
                    return Err("Cancelled".to_string());
                }

                downloaded_bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                let done = completed_segs.fetch_add(1, Ordering::Relaxed) + 1;
                let bytes_so_far = downloaded_bytes.load(Ordering::Relaxed);
                let elapsed = started.elapsed().as_secs_f64().max(0.001);
                let speed = bytes_so_far as f64 / elapsed;

                let _ = app_handle.emit(
                    "download-progress",
                    HlsProgressEvent {
                        id: download_id,
                        downloaded_size: bytes_so_far,
                        total_size: total_segments.saturating_mul(bytes.len() as u64).max(bytes_so_far),
                        speed,
                        status: "Downloading".into(),
                        connections: concurrency_limit as u32,
                    },
                );

                // Approximate total by average segment size * count
                let _ = done;

                let mut file = fs::File::create(&seg_path)
                    .await
                    .map_err(|e| e.to_string())?;
                file.write_all(&bytes).await.map_err(|e| e.to_string())?;
                Ok::<PathBuf, String>(seg_path)
            }
        })
        .buffer_unordered(concurrency_limit)
        .collect::<Vec<Result<PathBuf, String>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<PathBuf>, String>>()?;

    let mut segment_paths = segment_paths;
    segment_paths.sort();

    let _ = app_handle.emit(
        "download-progress",
        HlsProgressEvent {
            id: download_id,
            downloaded_size: downloaded_bytes.load(Ordering::Relaxed),
            total_size: downloaded_bytes.load(Ordering::Relaxed),
            speed: 0.0,
            status: "Merging".into(),
            connections: 0,
        },
    );
    if let Some(ref db) = db {
        let _ = db.update_download_progress(
            download_id,
            downloaded_bytes.load(Ordering::Relaxed),
            0.0,
            &DownloadStatus::Merging,
        );
    }

    let list_path = temp_dir.join("list.txt");
    let mut list_file = fs::File::create(&list_path)
        .await
        .map_err(|e| e.to_string())?;
    for p in &segment_paths {
        list_file
            .write_all(format!("file '{}'\n", p.file_name().unwrap().to_string_lossy()).as_bytes())
            .await
            .map_err(|e| e.to_string())?;
    }

    let output = app_handle
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| format!("Failed to find ffmpeg sidecar: {}", e))?
        .args([
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list_path.to_str().unwrap(),
            "-c",
            "copy",
            "-bsf:a",
            "aac_adtstoasc",
            save_path,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to execute FFmpeg: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "FFmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    crate::util::validate_completed_file(std::path::Path::new(save_path))?;

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
}
