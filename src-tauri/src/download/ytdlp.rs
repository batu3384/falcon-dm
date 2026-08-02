use crate::storage::Database;
use crate::util::sanitize_header_value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command; // ponytail: tokio async process — std::process would block the worker thread.

#[derive(Clone, Default)]
pub struct YtDlpHeaders {
    pub cookies: Option<String>,
    pub user_agent: Option<String>,
}

fn find_ytdlp(preferred: Option<&str>) -> Result<PathBuf, String> {
    if let Some(raw) = preferred.map(str::trim).filter(|s| !s.is_empty()) {
        let p = crate::util::expand_tilde(raw);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!(
            "yt-dlp yolu bulunamadı: {} — Settings’ten düzeltin veya boş bırakın",
            p.display()
        ));
    }
    for cand in [
        "yt-dlp",
        "/opt/homebrew/bin/yt-dlp",
        "/usr/local/bin/yt-dlp",
        "/Library/Frameworks/Python.framework/Versions/3.13/bin/yt-dlp",
        "/Library/Frameworks/Python.framework/Versions/3.12/bin/yt-dlp",
        "/Library/Frameworks/Python.framework/Versions/3.11/bin/yt-dlp",
    ] {
        let p = PathBuf::from(cand);
        if cand == "yt-dlp" {
            // Resolve via `command -v` equivalent: try running --version later; check common PATH
            if std::process::Command::new("yt-dlp")
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
            {
                return Ok(p);
            }
            continue;
        }
        if p.exists() {
            return Ok(p);
        }
    }
    Err("yt-dlp bulunamadı. YouTube için: brew install yt-dlp — veya Settings → yt-dlp path".into())
}

/// Download a YouTube (or yt-dlp supported) watch URL into `out_path`.
#[allow(clippy::too_many_arguments)]
pub async fn process_ytdlp(
    app_handle: &AppHandle,
    download_id: i64,
    url: &str,
    out_path: &str,
    mut cancel: tokio::sync::watch::Receiver<bool>,
    headers: YtDlpHeaders,
    db: Option<Database>,
    format: Option<&str>,
) -> Result<(), String> {
    let preferred = crate::settings::Settings::load(&crate::util::app_data_dir()).ytdlp_path;
    // ponytail: find_ytdlp runs `yt-dlp --version` (a subprocess) + filesystem
    // existence checks — all blocking. Run on a blocking-pool thread so we don't
    // stall a tokio worker thread (with max_concurrent downloads, several of these
    // could run concurrently and starve the runtime). Move an owned Option<String>
    // across the thread boundary (borrows can't satisfy 'static).
    let pref_opt: Option<String> = if preferred.trim().is_empty() { None } else { Some(preferred) };
    let bin = tokio::task::spawn_blocking(move || find_ytdlp(pref_opt.as_deref()))
        .await
        .map_err(|e| format!("yt-dlp lookup task failed: {e}"))??;
    let out = PathBuf::from(out_path);
    let dir = out.parent().ok_or_else(|| "Invalid output path".to_string())?;
    tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;

    let tmpl = out.with_extension("%(ext)s");
    let fmt = format.unwrap_or(
        "bestvideo[height<=1080][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=1080]+bestaudio/best[height<=1080]/bv*+ba/b",
    );

    // Cookies intentionally not passed by default — Netscape jar often breaks anonymous YT listing.
    // Do not write cookie files to disk until a gated opt-in path exists.
    let _ = &headers.cookies;

    let mut cmd = Command::new(&bin);
    cmd.arg("--no-playlist")
        .arg("--newline")
        .arg("--no-progress")
        .arg("-f")
        .arg(fmt)
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("-o")
        .arg(tmpl.to_string_lossy().as_ref())
        .arg("--no-mtime")
        // Remote EJS fetch removed (supply-chain). Use system yt-dlp with local JS runtime if needed.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(ref ua) = headers.user_agent {
        let ua = sanitize_header_value(ua);
        if !ua.is_empty() {
            cmd.arg("--user-agent").arg(ua);
        }
    }
    cmd.arg(url);

    let mut child = cmd.spawn().map_err(|e| format!("yt-dlp spawn failed: {e}"))?;
    let stderr = child.stderr.take();
    let stderr_buf = Arc::new(tokio::sync::Mutex::new(String::new()));
    let downloaded = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let start = Instant::now();

    if let Some(err) = stderr {
        let app = app_handle.clone();
        let dl_id = download_id;
        let downloaded_c = downloaded.clone();
        let total_c = total.clone();
        let stderr_c = stderr_buf.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                {
                    let mut buf = stderr_c.lock().await;
                    if buf.len() < 8000 {
                        buf.push_str(&line);
                        buf.push('\n');
                    }
                }
                if let Some(pct) = parse_percent(&line) {
                    let tot = total_c.load(Ordering::Relaxed);
                    if tot > 0 {
                        let cur = ((pct / 100.0) * tot as f64) as u64;
                        downloaded_c.store(cur, Ordering::Relaxed);
                    }
                }
                if let Some(sz) = parse_total_size(&line) {
                    total_c.store(sz, Ordering::Relaxed);
                }
                let cur = downloaded_c.load(Ordering::Relaxed);
                let tot = total_c.load(Ordering::Relaxed);
                let elapsed = start.elapsed().as_secs_f64().max(0.001);
                let speed = cur as f64 / elapsed;
                let _ = app.emit(
                    "download-progress",
                    serde_json::json!({
                        "id": dl_id,
                        "downloaded_size": cur,
                        "total_size": tot,
                        "speed": speed,
                        "status": "Downloading",
                        "connections": 1
                    }),
                );
            }
        });
    }

    loop {
        tokio::select! {
            _ = cancel.changed() => {
                if *cancel.borrow() {
                    let _ = child.kill().await;
                    return Err("Cancelled".into());
                }
            }
            status = child.wait() => {
                let status = status.map_err(|e| e.to_string())?;
                if !status.success() {
                    let detail = stderr_buf.lock().await.clone();
                    let tip = detail
                        .lines()
                        .rev()
                        .find(|l| l.contains("ERROR") || l.contains("error"))
                        .unwrap_or("YouTube koruması veya ağ hatası");
                    return Err(format!("yt-dlp: {tip}"));
                }
                break;
            }
        }
    }

    // Resolve final file (yt-dlp may change extension)
    // ponytail: resolve_output + metadata + rename are all blocking fs ops;
    // batch them onto a blocking-pool thread.
    let out_for_resolve = out.clone();
    let final_path = tokio::task::spawn_blocking(move || resolve_output(&out_for_resolve))
        .await
        .map_err(|e| format!("yt-dlp output resolve task failed: {e}"))??;

    let final_path_cloned = final_path.clone();
    let out_cloned = out.clone();
    let size = tokio::task::spawn_blocking(move || -> Result<u64, String> {
        let meta = std::fs::metadata(&final_path_cloned).map_err(|e| e.to_string())?;
        let size = meta.len();
        if size < 1024 {
            let _ = std::fs::remove_file(&final_path_cloned);
            return Err("Downloaded file too small — YouTube blocked the request".into());
        }
        // Rename to requested path if needed
        if final_path_cloned != out_cloned {
            let _ = std::fs::remove_file(&out_cloned);
            std::fs::rename(&final_path_cloned, &out_cloned)
                .or_else(|_| {
                    std::fs::copy(&final_path_cloned, &out_cloned).map(|_| {
                        let _ = std::fs::remove_file(&final_path_cloned);
                    })
                })
                .map_err(|e| e.to_string())?;
        }
        Ok(size)
    })
    .await
    .map_err(|e| format!("yt-dlp finalize task failed: {e}"))??;

    if let Some(ref db) = db {
        if let Ok(mut d) = db.get_download(download_id) {
            d.downloaded_size = size;
            d.total_size = size;
            d.filename =
                out.file_name().and_then(|s| s.to_str()).unwrap_or(&d.filename).to_string();
            // keep status update to caller
            let _ = db.update_download(download_id, &d);
        }
    }

    let _ = app_handle.emit(
        "download-progress",
        serde_json::json!({
            "id": download_id,
            "downloaded_size": size,
            "total_size": size,
            "speed": 0.0,
            "status": "Completed",
            "connections": 0
        }),
    );

    Ok(())
}

fn parse_percent(line: &str) -> Option<f64> {
    let idx = line.find('%')?;
    let start = line[..idx].rfind(' ')? + 1;
    line[start..idx].trim().parse().ok()
}

fn parse_total_size(line: &str) -> Option<u64> {
    // "of  70.17MiB"
    let lower = line.to_lowercase();
    let of = lower.find(" of ")?;
    let rest = line[of + 4..].trim_start();
    let end = rest.find(' ').unwrap_or(rest.len());
    parse_size_token(&rest[..end])
}

fn parse_size_token(tok: &str) -> Option<u64> {
    let t = tok.trim().to_lowercase();
    let (num, mult) = if let Some(n) = t.strip_suffix("gib") {
        (n, 1024u64.pow(3))
    } else if let Some(n) = t.strip_suffix("mib") {
        (n, 1024u64.pow(2))
    } else if let Some(n) = t.strip_suffix("kib") {
        (n, 1024)
    } else if let Some(n) = t.strip_suffix("gb") {
        (n, 1000u64.pow(3))
    } else if let Some(n) = t.strip_suffix("mb") {
        (n, 1000u64.pow(2))
    } else if let Some(n) = t.strip_suffix("kb") {
        (n, 1000)
    } else {
        return t.parse().ok();
    };
    let v: f64 = num.trim().parse().ok()?;
    Some((v * mult as f64) as u64)
}

fn resolve_output(requested: &Path) -> Result<PathBuf, String> {
    if requested.exists() {
        return Ok(requested.to_path_buf());
    }
    let stem = requested
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Missing filename".to_string())?;
    let parent = requested.parent().ok_or_else(|| "Missing parent".to_string())?;
    let mut candidates: Vec<_> = std::fs::read_dir(parent)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s == stem || s.starts_with(stem))
        })
        .collect();
    candidates.sort_by_key(|p| std::cmp::Reverse(p.metadata().map(|m| m.len()).unwrap_or(0)));
    candidates.into_iter().next().ok_or_else(|| "yt-dlp output file not found".into())
}
