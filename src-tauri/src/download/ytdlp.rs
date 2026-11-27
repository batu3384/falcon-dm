//! yt-dlp worker: YouTube, DASH and the HLS variants the native engine skips.
//!
//! Progress comes from machine-readable templates (yt-dlp prints nothing
//! parseable with `--no-progress`). Each download owns a stable work dir, so a
//! paused or retried job continues from yt-dlp's own `.part` files.

use crate::download::job::JobContext;
use crate::download::limiter::cancelled;
use crate::download::tools::{
    augmented_path, detect_ytdlp_version, find_ffmpeg, find_ytdlp, ytdlp_version_needs_upgrade,
    MIN_YTDLP_VERSION,
};
use crate::download::DownloadError;
use crate::util::{
    sanitize_filename, sanitize_header_value, sanitize_ytdlp_format, validate_fetch_url_async,
    write_file_private,
};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

pub const DEFAULT_VIDEO_FORMAT: &str =
    "bestvideo[height<=1080][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=1080]+bestaudio/best[height<=1080]/bv*+ba/b";

// `?` makes a missing live_status non-rejecting; missing is_live already passes `!is_live`.
const LIVE_MATCH_FILTER: &str = "!is_live & live_status !=? is_live & live_status !=? is_upcoming";

#[derive(Clone, Default)]
pub struct YtDlpOptions {
    pub cookies: Option<String>,
    pub user_agent: Option<String>,
    pub referrer: Option<String>,
    pub format: Option<String>,
    pub proxy: Option<String>,
    pub speed_limit_kbps: u32,
    pub ytdlp_path: Option<String>,
    pub use_browser_cookies: bool,
    pub max_connections: usize,
}

pub fn is_audio_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "m4a" | "mp3" | "opus" | "ogg" | "flac" | "wav" | "aac" | "weba"
    )
}

fn dest_is_audio(dest: &Path) -> bool {
    dest.extension().and_then(|e| e.to_str()).is_some_and(is_audio_extension)
}

/// Work dir next to the destination, keyed by download id.
pub fn run_dir_for(save_dir: &str, download_id: i64) -> PathBuf {
    Path::new(save_dir).join(format!(".falcon-dm-ytdlp-{download_id}"))
}

pub fn remove_artifacts(save_dir: &str, download_id: i64) {
    let _ = std::fs::remove_dir_all(run_dir_for(save_dir, download_id));
}

/// `.falcon-dm-ytdlp-<id>` (current) or `.falcon-dm-ytdlp-<id>-<uuid>` (older
/// builds). Returns the download id.
pub fn parse_run_dir_name(name: &str) -> Option<i64> {
    let rest = name.strip_prefix(".falcon-dm-ytdlp-")?;
    match rest.split_once('-') {
        None => rest.parse().ok(),
        Some((id, run)) => {
            uuid::Uuid::parse_str(run).ok()?;
            id.parse().ok()
        }
    }
}

fn parse_u64(field: &str) -> Option<u64> {
    let field = field.trim();
    if field.is_empty() || field == "NA" || field == "None" {
        return None;
    }
    field.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v as u64)
}

#[derive(Debug, PartialEq)]
enum Line {
    Meta { total: Option<u64>, title: Option<String>, live: bool },
    Download { finished: bool, downloaded: u64, total: Option<u64>, format: String },
    PostProcess { name: String },
    Other,
}

fn live_stream_metadata(live_status: &str, is_live: &str) -> bool {
    matches!(live_status.trim().to_ascii_lowercase().as_str(), "is_live" | "is_upcoming")
        || matches!(is_live.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes")
}

fn parse_line(line: &str) -> Line {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("FALCON_META|") {
        let mut fields = rest.splitn(4, '|');
        let (Some(total), Some(live_status), Some(is_live), Some(title)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Line::Other;
        };
        let title = title.trim();
        return Line::Meta {
            total: parse_u64(total),
            title: (!title.is_empty() && title != "NA").then(|| title.to_string()),
            live: live_stream_metadata(live_status, is_live),
        };
    }
    if let Some(rest) = line.strip_prefix("FALCON_DL|") {
        let fields: Vec<&str> = rest.split('|').collect();
        if fields.len() >= 5 {
            return Line::Download {
                finished: fields[0].trim() == "finished",
                downloaded: parse_u64(fields[1]).unwrap_or(0),
                total: parse_u64(fields[2]).or_else(|| parse_u64(fields[3])),
                format: fields[4].trim().to_string(),
            };
        }
    }
    if let Some(rest) = line.strip_prefix("FALCON_PP|") {
        let (status, name) = rest.split_once('|').unwrap_or((rest, ""));
        if status.trim() == "started" {
            return Line::PostProcess { name: name.trim().to_string() };
        }
    }
    Line::Other
}

/// Folds per-stream progress (video, then audio) into one monotonic counter.
#[derive(Default)]
struct Tracker {
    expected_total: Option<u64>,
    finished_bytes: u64,
    current_format: String,
    current_downloaded: u64,
    current_total: u64,
    /// The first report of a stream may include bytes resumed from disk.
    seen_current: bool,
}

struct Update {
    downloaded: u64,
    total: u64,
    /// Bytes that arrived over the network since the last report.
    transferred: u64,
}

impl Tracker {
    fn close_stream(&mut self) {
        self.finished_bytes += self.current_downloaded.max(self.current_total);
        self.current_downloaded = 0;
        self.current_total = 0;
        self.seen_current = false;
        self.current_format.clear();
    }

    fn on_download(
        &mut self,
        finished: bool,
        downloaded: u64,
        total: Option<u64>,
        format: &str,
    ) -> Update {
        if self.seen_current && !self.current_format.is_empty() && self.current_format != format {
            // yt-dlp moved on without a "finished" line.
            self.close_stream();
        }
        self.current_format = format.to_string();
        let transferred =
            if self.seen_current { downloaded.saturating_sub(self.current_downloaded) } else { 0 };
        self.seen_current = true;
        self.current_downloaded = self.current_downloaded.max(downloaded);
        if let Some(total) = total {
            self.current_total = total;
        }
        let downloaded = self.finished_bytes + self.current_downloaded;
        let known = self.finished_bytes + self.current_total.max(self.current_downloaded);
        let total = self.expected_total.unwrap_or(0).max(known);
        if finished {
            self.close_stream();
        }
        Update { downloaded, total, transferred }
    }
}

fn classify_failure(detail: &str) -> DownloadError {
    let lower = detail.to_lowercase();
    let transient = [
        "http error 429",
        "http error 5",
        "timed out",
        "timeout",
        "temporary failure",
        "connection reset",
        "connection refused",
        "connection aborted",
        "network is unreachable",
        "getaddrinfo",
        "name resolution",
        "nodename nor servname",
        "incompleteread",
        "incomplete read",
        "unable to download webpage",
        "unable to download video data",
        "fragment",
        "remote end closed",
        "ssl",
    ];
    let message = format!("yt-dlp: {detail}");
    if lower.contains("this live event will begin") {
        return DownloadError::fatal("ERR_HLS_LIVE");
    }
    if lower.contains("video unavailable") {
        return DownloadError::fatal("ERR_VIDEO_UNAVAILABLE");
    }
    if lower.contains("private video") {
        return DownloadError::fatal("ERR_VIDEO_PRIVATE");
    }
    if lower.contains("sign in to confirm") {
        return DownloadError::fatal("ERR_LOGIN_REQUIRED");
    }
    if lower.contains("ffmpeg") && (lower.contains("not installed") || lower.contains("not found"))
    {
        return DownloadError::fatal("ERR_FFMPEG_MISSING");
    }
    if lower.contains("http error 403") {
        return DownloadError::fatal("HTTP 403");
    }
    // 403/404/410 do not heal by waiting; 408 and 429 do.
    if let Some(rest) = lower.split("http error ").nth(1) {
        let code: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if code.starts_with('4') && code != "408" && code != "429" {
            return DownloadError::Fatal(message);
        }
    }
    if transient.iter().any(|needle| lower.contains(needle)) {
        DownloadError::Transient(message)
    } else {
        DownloadError::Fatal(message)
    }
}

/// Most useful line for the user: the last `ERROR:`, else the last line.
fn failure_detail(lines: &VecDeque<String>) -> String {
    let pick = lines.iter().rev().find(|l| l.contains("ERROR")).or_else(|| lines.back());
    let text = pick.map(String::as_str).unwrap_or("download failed");
    let text = text.trim().trim_start_matches("ERROR:").trim();
    text.chars().take(300).collect()
}

fn resolve_output(run_dir: &Path) -> Result<PathBuf, String> {
    let mut candidates: Vec<(u64, PathBuf)> = std::fs::read_dir(run_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            p.is_file()
                && p.file_stem().and_then(|s| s.to_str()) == Some("output")
                && !matches!(ext, "part" | "ytdl" | "temp" | "tmp")
        })
        .map(|p| (p.metadata().map(|m| m.len()).unwrap_or(0), p))
        .collect();
    candidates.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
    candidates
        .into_iter()
        .next()
        .map(|(_, p)| p)
        .ok_or_else(|| "yt-dlp output file not found".into())
}

/// ffmpeg writes yt-dlp's postprocess result as `output.temp.<ext>` and renames
/// it only at the end, so its size is the one progress signal available.
async fn postprocess_output_size(run_dir: &Path) -> u64 {
    let Ok(mut entries) = tokio::fs::read_dir(run_dir).await else {
        return 0;
    };
    let mut bytes = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("output.temp.") {
            bytes += entry.metadata().await.map(|m| m.len()).unwrap_or(0);
        }
    }
    bytes
}

const COOKIE_FILE_NAME: &str = "cookies.txt";

fn parse_cookie_header(header: &str) -> Vec<(String, String)> {
    header
        .split(';')
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((name.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Netscape jar for one download. A bare host plus `FALSE` is the host-only form.
fn netscape_cookie_file(host: &str, secure: bool, header: &str) -> Option<String> {
    if host.is_empty() || host.chars().any(|c| c <= ' ' || c == '\u{7f}') {
        return None;
    }
    let pairs = parse_cookie_header(&sanitize_header_value(header));
    if pairs.is_empty() {
        return None;
    }
    let secure = if secure { "TRUE" } else { "FALSE" };
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for (name, value) in pairs {
        out.push_str(host);
        out.push_str("\tFALSE\t/\t");
        out.push_str(secure);
        out.push_str("\t0\t");
        out.push_str(&name);
        out.push('\t');
        out.push_str(&value);
        out.push('\n');
    }
    Some(out)
}

struct CookieFileGuard(PathBuf);

impl Drop for CookieFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn write_cookie_file(
    run_dir: &Path,
    url: &url::Url,
    cookies: Option<&str>,
) -> Result<Option<CookieFileGuard>, DownloadError> {
    let Some(header) = cookies.map(sanitize_header_value).filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let host = url.host_str().ok_or_else(|| DownloadError::fatal("URL has no host"))?;
    let Some(contents) = netscape_cookie_file(host, url.scheme() == "https", &header) else {
        return Err(DownloadError::fatal("Cookie header has no valid name=value pairs"));
    };
    let path = run_dir.join(COOKIE_FILE_NAME);
    let write_path = path.clone();
    tokio::task::spawn_blocking(move || write_file_private(&write_path, contents.as_bytes()))
        .await
        .map_err(|e| DownloadError::fatal(format!("cookie file write failed: {e}")))?
        .map_err(|e| DownloadError::fatal(format!("could not write cookie file: {e}")))?;
    Ok(Some(CookieFileGuard(path)))
}

fn configured_user_proxy(opts: &YtDlpOptions) -> Option<&str> {
    opts.proxy.as_deref().map(str::trim).filter(|p| !p.is_empty())
}

fn build_command(
    bin: &Path,
    url: &str,
    run_dir: &Path,
    audio_only: bool,
    opts: &YtDlpOptions,
    cookie_file: Option<&Path>,
    egress_proxy: Option<&str>,
) -> Command {
    let format = opts
        .format
        .as_deref()
        .and_then(sanitize_ytdlp_format)
        .unwrap_or_else(|| DEFAULT_VIDEO_FORMAT.to_string());
    let mut cmd = Command::new(bin);
    cmd.env("PATH", augmented_path())
        .arg("--no-playlist")
        .arg("--newline")
        .arg("--progress")
        .args(["--progress-delta", "0.25"])
        .arg("--progress-template")
        .arg("download:FALCON_DL|%(progress.status)s|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(info.format_id)s")
        .arg("--progress-template")
        .arg("postprocess:FALCON_PP|%(progress.status)s|%(progress.postprocessor)s")
        .arg("--print")
        // pre_process is emitted before yt-dlp's own filter and downloader. The
        // filter is a same-process backstop if this parent is slow to read the pipe.
        .arg("pre_process:FALCON_META|%(filesize,filesize_approx)s|%(live_status)s|%(is_live)s|%(title)s")
        .arg("--match-filters")
        .arg(LIVE_MATCH_FILTER)
        .arg("--no-simulate")
        .args(["-f", &format])
        .arg("--no-mtime")
        .args(["--retries", "5", "--fragment-retries", "10", "--socket-timeout", "30"])
        .args(["-N", &opts.max_connections.clamp(1, 8).to_string()])
        .arg("-o")
        .arg(run_dir.join("output.%(ext)s"));
    if !audio_only {
        cmd.args(["--merge-output-format", "mp4", "--remux-video", "mp4"]);
    }
    if let Ok(ffmpeg) = find_ffmpeg() {
        cmd.arg("--ffmpeg-location").arg(ffmpeg);
    }
    if let Some(cookie_file) = cookie_file {
        cmd.arg("--cookies").arg(cookie_file);
    } else if opts.use_browser_cookies {
        cmd.args(["--cookies-from-browser", "chrome"]);
    }
    if let Some(referrer) =
        opts.referrer.as_deref().map(sanitize_header_value).filter(|r| !r.is_empty())
    {
        cmd.arg("--referer").arg(referrer);
    }
    if let Some(ua) =
        opts.user_agent.as_deref().map(sanitize_header_value).filter(|u| !u.is_empty())
    {
        cmd.arg("--user-agent").arg(ua);
    }
    if let Some(proxy) = configured_user_proxy(opts) {
        // The user's proxy choice wins; chaining it through the validating proxy is out of scope.
        cmd.arg("--proxy").arg(proxy);
    } else if let Some(proxy) = egress_proxy {
        cmd.arg("--proxy").arg(proxy);
        // yt-dlp honors --proxy itself; descendants such as ffmpeg often read only env.
        // NO_PROXY inherited from the login session would be a direct-network SSRF bypass.
        for key in
            ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"]
        {
            cmd.env(key, proxy);
        }
        cmd.env_remove("NO_PROXY").env_remove("no_proxy");
    }
    if opts.speed_limit_kbps > 0 {
        cmd.arg("--limit-rate").arg(format!("{}K", opts.speed_limit_kbps));
    }
    cmd.arg("--").arg(url);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    cmd
}

/// Kill yt-dlp's whole process group, then reap the direct child. ffmpeg and
/// other postprocessor grandchildren inherit the group; `start_kill` covers a
/// direct child that unexpectedly moved itself out of it.
async fn kill_process_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pgid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
            // SAFETY: `kill` only sends a signal. The negative pid targets the
            // process group created by `process_group(0)`, never this process.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        }
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Download `url` with yt-dlp into `out_path`. Returns where the file landed.
pub async fn process_ytdlp(
    ctx: &JobContext,
    url: &str,
    out_path: &str,
    opts: YtDlpOptions,
) -> Result<PathBuf, DownloadError> {
    let validated_url = validate_fetch_url_async(url).await?;
    let preferred = opts.ytdlp_path.clone();
    let bin = tokio::task::spawn_blocking(move || find_ytdlp(preferred.as_deref()))
        .await
        .map_err(|e| DownloadError::fatal(format!("yt-dlp lookup failed: {e}")))?
        .map_err(DownloadError::Fatal)?;
    if let Some(version) = detect_ytdlp_version(&bin).await {
        if ytdlp_version_needs_upgrade(&version) {
            let (year, month, day) = MIN_YTDLP_VERSION;
            return Err(DownloadError::fatal(format!(
                "yt-dlp {year:04}.{month:02}.{day:02} or newer is required (installed: {version})"
            )));
        }
    }
    let mut dest = PathBuf::from(out_path);
    let dir =
        dest.parent().ok_or_else(|| DownloadError::fatal("Invalid output path"))?.to_path_buf();
    let dir_str =
        dir.to_str().ok_or_else(|| DownloadError::fatal("Save path is not valid UTF-8"))?;
    let run_dir = run_dir_for(dir_str, ctx.id);
    tokio::fs::create_dir_all(&run_dir)
        .await
        .map_err(|e| DownloadError::fatal(format!("could not create work folder: {e}")))?;
    let cookie_file = write_cookie_file(&run_dir, &validated_url, opts.cookies.as_deref()).await?;
    let audio_only = dest_is_audio(&dest);
    let _ticker = ctx.progress.start();
    ctx.progress.set_connections(opts.max_connections.clamp(1, 8) as u32);

    let egress_proxy = if configured_user_proxy(&opts).is_some() {
        None
    } else {
        match crate::download::egress_proxy::shared().await {
            Ok(proxy) => Some(proxy.url()),
            Err(error) => {
                // A loopback ephemeral bind is practically infallible; continuing without
                // validation would silently reopen yt-dlp's redirect/segment SSRF gap.
                return Err(DownloadError::fatal(format!(
                    "validating egress proxy unavailable: {error}"
                )));
            }
        }
    };

    let cookie_path = cookie_file.as_ref().map(|guard| guard.0.as_path());
    let mut child = build_command(
        &bin,
        validated_url.as_str(),
        &run_dir,
        audio_only,
        &opts,
        cookie_path,
        egress_proxy.as_deref(),
    )
    .spawn()
    .map_err(|e| DownloadError::fatal(format!("yt-dlp could not start: {e}")))?;
    let stdout = child.stdout.take().ok_or_else(|| DownloadError::fatal("yt-dlp has no stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| DownloadError::fatal("yt-dlp has no stderr"))?;
    // Both pipes are drained: an unread pipe would block yt-dlp once it fills.
    let mut out_lines = BufReader::new(stdout).lines();
    let mut err_lines = BufReader::new(stderr).lines();
    let (mut out_open, mut err_open) = (true, true);
    let mut tracker = Tracker::default();
    let mut tail: VecDeque<String> = VecDeque::with_capacity(40);
    let mut title: Option<String> = None;
    let mut cancel = ctx.cancel.clone();
    let mut postprocess_tick = tokio::time::interval(Duration::from_millis(250));
    postprocess_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut postprocessing = false;
    let mut postprocess_bytes = 0u64;

    while out_open || err_open {
        let line = tokio::select! {
            line = out_lines.next_line(), if out_open => match line {
                Ok(Some(line)) => line,
                _ => { out_open = false; continue; }
            },
            line = err_lines.next_line(), if err_open => match line {
                Ok(Some(line)) => line,
                _ => { err_open = false; continue; }
            },
            _ = postprocess_tick.tick(), if postprocessing => {
                let bytes = postprocess_output_size(&run_dir).await;
                if bytes != postprocess_bytes {
                    postprocess_bytes = bytes;
                    if bytes > 0 {
                        ctx.progress.set_downloaded(bytes);
                    }
                }
                continue;
            }
            _ = cancelled(&mut cancel) => {
                kill_process_tree(&mut child).await;
                // The work dir stays: the next run resumes yt-dlp's part files.
                return Err(DownloadError::Cancelled);
            }
        };
        match parse_line(&line) {
            Line::Meta { total, title: found, live } => {
                if live {
                    kill_process_tree(&mut child).await;
                    return Err(DownloadError::fatal("ERR_HLS_LIVE"));
                }
                tracker.expected_total = total.filter(|t| *t > 0);
                title = found;
                if let Some(total) = tracker.expected_total {
                    ctx.progress.set_total(total);
                }
            }
            Line::Download { finished, downloaded, total, format } => {
                let update = tracker.on_download(finished, downloaded, total, &format);
                ctx.progress.set_downloaded(update.downloaded);
                ctx.progress.set_total(update.total);
                ctx.progress.add_transferred(update.transferred);
            }
            Line::PostProcess { name } => {
                if name != "MoveFiles" {
                    postprocessing = true;
                    ctx.progress.set_merging();
                }
            }
            Line::Other => {
                if tail.len() == 40 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        }
    }

    let status = tokio::select! {
        status = child.wait() => status.map_err(|e| DownloadError::fatal(e.to_string()))?,
        _ = cancelled(&mut cancel) => {
            kill_process_tree(&mut child).await;
            return Err(DownloadError::Cancelled);
        }
    };
    ctx.check_cancel()?;
    if !status.success() {
        return Err(classify_failure(&failure_detail(&tail)));
    }

    let run_dir_owned = run_dir.clone();
    let produced = tokio::task::spawn_blocking(move || resolve_output(&run_dir_owned))
        .await
        .map_err(|e| DownloadError::fatal(format!("yt-dlp output lookup failed: {e}")))?
        .map_err(DownloadError::Fatal)?;
    let size = tokio::fs::metadata(&produced).await.map(|m| m.len()).unwrap_or(0);
    if size < 1024 {
        let _ = tokio::fs::remove_dir_all(&run_dir).await;
        return Err(DownloadError::fatal(
            "Downloaded file is too small — the site blocked the request",
        ));
    }

    // The container yt-dlp produced decides the extension; a placeholder name
    // (manual add of a watch URL) takes the real title.
    let produced_ext = produced.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_string();
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("download").to_string();
    let placeholder = stem.starts_with("falcon_download") || stem == "watch" || stem == "download";
    let stem = match title.as_deref().map(sanitize_filename) {
        Some(title) if placeholder && !title.is_empty() => title.chars().take(150).collect(),
        _ => stem,
    };
    let wanted = format!("{stem}.{produced_ext}");
    if dest.file_name().and_then(|n| n.to_str()) != Some(wanted.as_str()) {
        dest = ctx.rename(&dest, &wanted);
    }
    ctx.progress.set_downloaded(size);
    ctx.progress.set_total(size);
    ctx.progress.set_merging();
    let landed = ctx.finalize(&produced, &dest).await?;
    let _ = tokio::fs::remove_dir_all(&run_dir).await;
    Ok(landed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines_parse_and_garbage_is_ignored() {
        assert_eq!(
            parse_line("FALCON_META|533067|not_live|False|Me at the zoo"),
            Line::Meta { total: Some(533_067), title: Some("Me at the zoo".into()), live: false }
        );
        assert_eq!(
            parse_line("FALCON_META|NA|NA|NA|NA"),
            Line::Meta { total: None, title: None, live: false }
        );
        assert_eq!(
            parse_line("FALCON_META|8192|is_live|True|Live stream"),
            Line::Meta { total: Some(8192), title: Some("Live stream".into()), live: true }
        );
        assert_eq!(
            parse_line("FALCON_META|8192|not_live|False|Title | with pipes"),
            Line::Meta { total: Some(8192), title: Some("Title | with pipes".into()), live: false }
        );
        assert_eq!(
            parse_line("FALCON_DL|downloading|130048|223779|NA|395"),
            Line::Download {
                finished: false,
                downloaded: 130_048,
                total: Some(223_779),
                format: "395".into()
            }
        );
        // Fragmented formats only have an estimate.
        assert_eq!(
            parse_line("FALCON_DL|downloading|1024|NA|5000.7|hls-720"),
            Line::Download {
                finished: false,
                downloaded: 1024,
                total: Some(5000),
                format: "hls-720".into()
            }
        );
        assert_eq!(
            parse_line("FALCON_PP|started|Merger"),
            Line::PostProcess { name: "Merger".into() }
        );
        assert_eq!(parse_line("FALCON_PP|finished|Merger"), Line::Other);
        assert_eq!(parse_line("[youtube] abc: Downloading webpage"), Line::Other);
        assert_eq!(parse_line("FALCON_DL|broken"), Line::Other);
    }

    #[test]
    fn only_current_or_upcoming_live_metadata_is_rejected() {
        assert!(live_stream_metadata("is_live", "False"));
        assert!(live_stream_metadata("is_upcoming", "False"));
        assert!(live_stream_metadata("not_live", "True"));
        assert!(!live_stream_metadata("was_live", "False"));
        assert!(!live_stream_metadata("not_live", "False"));
        assert!(!live_stream_metadata("NA", "NA"));
    }

    #[test]
    fn video_then_audio_adds_up_and_never_goes_backwards() {
        let mut tracker = Tracker { expected_total: Some(533_067), ..Default::default() };
        let mut last = 0;
        let mut transferred = 0;
        let script = [
            (false, 1024u64, 223_779u64, "395"),
            (false, 130_048, 223_779, "395"),
            (false, 223_779, 223_779, "395"),
            (true, 223_779, 223_779, "395"),
            (false, 1024, 309_288, "140"),
            (false, 261_120, 309_288, "140"),
            (true, 309_288, 309_288, "140"),
        ];
        for (finished, downloaded, total, format) in script {
            let update = tracker.on_download(finished, downloaded, Some(total), format);
            assert!(update.downloaded >= last, "progress went backwards");
            assert!(update.downloaded <= update.total);
            last = update.downloaded;
            transferred += update.transferred;
        }
        assert_eq!(last, 223_779 + 309_288);
        // The first report of each stream is not counted as traffic (may be resumed bytes).
        assert_eq!(transferred, (223_779 - 1024) + (309_288 - 1024));
    }

    #[test]
    fn unknown_total_grows_with_each_stream() {
        let mut tracker = Tracker::default();
        let a = tracker.on_download(false, 50, Some(100), "v");
        assert_eq!((a.downloaded, a.total), (50, 100));
        tracker.on_download(true, 100, Some(100), "v");
        let b = tracker.on_download(false, 10, Some(40), "a");
        assert_eq!((b.downloaded, b.total), (110, 140));
    }

    #[test]
    fn stream_switch_without_finished_line_is_closed() {
        let mut tracker = Tracker::default();
        tracker.on_download(false, 90, Some(100), "v");
        let next = tracker.on_download(false, 5, Some(50), "a");
        assert_eq!((next.downloaded, next.total), (105, 150));
    }

    #[test]
    fn failures_split_into_retry_classes() {
        assert!(matches!(
            classify_failure("unable to download video data: HTTP Error 503: Service Unavailable"),
            DownloadError::Transient(_)
        ));
        assert!(matches!(
            classify_failure("HTTP Error 429: Too Many Requests"),
            DownloadError::Transient(_)
        ));
        assert_eq!(
            classify_failure("[youtube] abc: Video unavailable"),
            DownloadError::fatal("ERR_VIDEO_UNAVAILABLE")
        );
        assert_eq!(
            classify_failure("[youtube] abc: Private video"),
            DownloadError::fatal("ERR_VIDEO_PRIVATE")
        );
        assert_eq!(
            classify_failure("[youtube] abc: Sign in to confirm you're not a bot"),
            DownloadError::fatal("ERR_LOGIN_REQUIRED")
        );
        assert_eq!(
            classify_failure("unable to download video data: HTTP Error 403: Forbidden"),
            DownloadError::fatal("HTTP 403")
        );
        assert_eq!(
            classify_failure("[youtube] abc: This live event will begin in 7 hours"),
            DownloadError::fatal("ERR_HLS_LIVE")
        );
        assert!(matches!(
            classify_failure("Requested format is not available"),
            DownloadError::Fatal(_)
        ));
        assert_eq!(
            classify_failure(
                "You have requested merging of multiple formats but ffmpeg is not installed"
            ),
            DownloadError::fatal("ERR_FFMPEG_MISSING")
        );
    }

    #[test]
    fn failure_detail_prefers_the_error_line() {
        let mut lines = VecDeque::new();
        lines.push_back("[youtube] Extracting URL".to_string());
        lines.push_back("ERROR: [youtube] abc: Private video".to_string());
        lines.push_back("WARNING: something else".to_string());
        assert_eq!(failure_detail(&lines), "[youtube] abc: Private video");
        assert_eq!(failure_detail(&VecDeque::new()), "download failed");
    }

    #[test]
    fn run_dir_names_round_trip() {
        assert_eq!(parse_run_dir_name(".falcon-dm-ytdlp-42"), Some(42));
        assert_eq!(
            parse_run_dir_name(".falcon-dm-ytdlp-7-123e4567-e89b-12d3-a456-426614174000"),
            Some(7)
        );
        assert_eq!(parse_run_dir_name(".falcon-dm-ytdlp-x"), None);
        assert_eq!(parse_run_dir_name(".falcon-dm-ytdlp-7-notauuid"), None);
        assert_eq!(parse_run_dir_name("Movies"), None);
        assert!(run_dir_for("/tmp/dl", 42).ends_with(".falcon-dm-ytdlp-42"));
    }

    #[test]
    fn netscape_cookie_file_scopes_pairs_to_the_exact_host() {
        let file = netscape_cookie_file(
            "media.example.com",
            true,
            "sid=secret; theme=dark; encoded=a=b; empty=; broken",
        )
        .unwrap();
        assert!(file.starts_with("# Netscape HTTP Cookie File\n"));
        assert!(file.contains("media.example.com\tFALSE\t/\tTRUE\t0\tsid\tsecret\n"));
        assert!(file.contains("media.example.com\tFALSE\t/\tTRUE\t0\ttheme\tdark\n"));
        assert!(file.contains("media.example.com\tFALSE\t/\tTRUE\t0\tencoded\ta=b\n"));
        assert!(file.contains("media.example.com\tFALSE\t/\tTRUE\t0\tempty\t\n"));
        assert!(
            !file.contains(".media.example.com"),
            "host-only entries must not use a leading dot"
        );
        assert!(!file.contains("broken"), "malformed pairs are not written");
        assert!(netscape_cookie_file("media.example.com", true, "not-a-cookie").is_none());
        let sanitized =
            netscape_cookie_file("media.example.com", true, "sid=secret\r\nX-Evil: 1").unwrap();
        assert_eq!(sanitized.lines().count(), 2, "CR/LF must not create cookie entries");
        assert!(sanitized.contains("sid\tsecretX-Evil: 1\n"));
    }

    #[test]
    fn resolve_output_skips_partials_and_other_stems() {
        let run_dir =
            std::env::temp_dir().join(format!("falcon-dm-ytdlp-run-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&run_dir).unwrap();
        assert!(resolve_output(&run_dir).is_err());
        std::fs::write(run_dir.join("output.f140.m4a.part"), vec![0u8; 5000]).unwrap();
        std::fs::write(run_dir.join("other.mp4"), vec![0u8; 9000]).unwrap();
        assert!(resolve_output(&run_dir).is_err(), "partials are not results");
        std::fs::write(run_dir.join("output.mp4"), vec![0u8; 100]).unwrap();
        assert_eq!(resolve_output(&run_dir).unwrap(), run_dir.join("output.mp4"));
        std::fs::remove_dir_all(run_dir).unwrap();
    }

    #[test]
    fn command_never_lets_the_url_or_format_become_a_flag() {
        let opts = YtDlpOptions {
            format: Some("best --exec calc".into()),
            cookies: Some("sid=1\r\nX-Evil: 1".into()),
            speed_limit_kbps: 512,
            max_connections: 16,
            ..Default::default()
        };
        let cmd = build_command(
            Path::new("yt-dlp"),
            "--exec=evil",
            Path::new("/tmp/run"),
            false,
            &opts,
            Some(Path::new("/tmp/run/cookies.txt")),
            None,
        );
        let args: Vec<String> =
            cmd.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let dashdash = args.iter().position(|a| a == "--").expect("-- separator");
        assert_eq!(args[dashdash + 1], "--exec=evil");
        assert_eq!(dashdash + 2, args.len(), "URL must be the last argument");
        let fmt = &args[args.iter().position(|a| a == "-f").unwrap() + 1];
        assert_eq!(fmt, DEFAULT_VIDEO_FORMAT, "unsafe selector falls back to the default");
        assert!(args.windows(2).any(|w| w == ["--cookies", "/tmp/run/cookies.txt"]));
        assert!(!args.iter().any(|a| a == "--add-header"), "cookies must not be a raw header");
        assert!(!args.iter().any(|a| a.starts_with("Cookie:")));
        assert!(args.windows(2).any(|w| w == ["--limit-rate", "512K"]));
        assert!(args.windows(2).any(|w| w == ["-N", "8"]));
        assert!(args.iter().any(|a| a == "--merge-output-format"));
        assert!(args.iter().any(|a| {
            a.starts_with("pre_process:FALCON_META|")
                && a.contains("%(live_status)s")
                && a.contains("%(is_live)s")
        }));
        assert!(args.windows(2).any(|w| {
            w[0] == "--match-filters"
                && w[1].contains("!is_live")
                && w[1].contains("live_status !=? is_live")
                && w[1].contains("live_status !=? is_upcoming")
        }));

        let audio = build_command(
            Path::new("yt-dlp"),
            "https://x",
            Path::new("/tmp/run"),
            true,
            &opts,
            Some(Path::new("/tmp/run/cookies.txt")),
            None,
        );
        assert!(!audio.as_std().get_args().any(|a| a == "--merge-output-format"));
    }

    fn explicit_env(cmd: &Command, name: &str) -> Option<Option<String>> {
        cmd.as_std()
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new(name))
            .map(|(_, value)| value.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn validating_egress_proxy_reaches_ytdlp_and_descendants_only_without_a_user_proxy() {
        let egress = "http://127.0.0.1:43210";
        let opts = YtDlpOptions::default();
        let cmd = build_command(
            Path::new("yt-dlp"),
            "https://media.example/video",
            Path::new("/tmp/run"),
            false,
            &opts,
            None,
            Some(egress),
        );
        let args: Vec<String> =
            cmd.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.windows(2).any(|w| w == ["--proxy", egress]));
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            assert_eq!(explicit_env(&cmd, name), Some(Some(egress.into())), "{name}");
        }
        assert_eq!(explicit_env(&cmd, "NO_PROXY"), Some(None));
        assert_eq!(explicit_env(&cmd, "no_proxy"), Some(None));

        let user_opts = YtDlpOptions { proxy: Some("http://user-proxy:8080".into()), ..opts };
        let user_cmd = build_command(
            Path::new("yt-dlp"),
            "https://media.example/video",
            Path::new("/tmp/run"),
            false,
            &user_opts,
            None,
            Some(egress),
        );
        let args: Vec<String> =
            user_cmd.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.windows(2).any(|w| w == ["--proxy", "http://user-proxy:8080"]));
        assert!(!args.windows(2).any(|w| w == ["--proxy", egress]));
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            assert_eq!(explicit_env(&user_cmd, name), None, "{name}");
        }
    }

    #[cfg(unix)]
    const TEST_INPUT_URL: &str = "https://127.0.0.1/stream.mpd";

    #[cfg(unix)]
    fn write_fake_ytdlp_with_version(dir: &Path, version: &str, body: &str) -> PathBuf {
        let path = dir.join("yt-dlp");
        let capture_args = dir.join("ytdlp.args").display().to_string();
        let capture_env = dir.join("ytdlp.env").display().to_string();
        let script = format!(
            r#"#!/bin/sh
if [ "${{1-}}" = "--version" ]; then
  printf '%s\n' '{version}'
  exit 0
fi
printf '%s\n' "$@" > '{capture_args}'
{{
  printf 'HTTP_PROXY=%s\n' "$HTTP_PROXY"
  printf 'HTTPS_PROXY=%s\n' "$HTTPS_PROXY"
  printf 'http_proxy=%s\n' "$http_proxy"
  printf 'https_proxy=%s\n' "$https_proxy"
  printf 'ALL_PROXY=%s\n' "$ALL_PROXY"
  printf 'all_proxy=%s\n' "$all_proxy"
  printf 'NO_PROXY=%s\n' "$NO_PROXY"
  printf 'no_proxy=%s\n' "$no_proxy"
}} > '{capture_env}'
out=
cookie_file=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) shift; out="$1" ;;
    --cookies) shift; cookie_file="$1" ;;
  esac
  shift || break
done
out=$(printf '%s' "$out" | sed 's/%(ext)s/mp4/g')
{body}
"#
        );
        std::fs::write(&path, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn write_fake_ytdlp(dir: &Path, body: &str) -> PathBuf {
        write_fake_ytdlp_with_version(dir, "2024.01.01", body)
    }

    #[cfg(unix)]
    fn fake_success_body() -> &'static str {
        r#"printf 'FALCON_DL|finished|2048|2048|NA|best\n'
dd if=/dev/zero of="$out" bs=1024 count=2 >/dev/null 2>&1
"#
    }

    #[cfg(unix)]
    async fn run_fake_ytdlp(root: &Path, id: i64, bin: &Path) -> Result<PathBuf, DownloadError> {
        let _loopback = crate::util::allow_loopback_for_tests();
        let (ctx, _cancel) = JobContext::for_tests(id);
        process_ytdlp(
            &ctx,
            TEST_INPUT_URL,
            root.join("video.mp4").to_str().unwrap(),
            YtDlpOptions {
                ytdlp_path: Some(bin.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_child_uses_shared_validating_proxy_when_user_proxy_is_absent() {
        let root =
            std::env::temp_dir().join(format!("falcon-ytdlp-egress-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let bin = write_fake_ytdlp(&root, fake_success_body());
        let result = run_fake_ytdlp(&root, 126, &bin).await;
        assert!(result.is_ok(), "fake yt-dlp run failed: {result:?}");

        let args: Vec<String> = std::fs::read_to_string(root.join("ytdlp.args"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        let proxy_index = args.iter().position(|arg| arg == "--proxy").expect("--proxy missing");
        let proxy = args[proxy_index + 1].clone();
        assert!(proxy.starts_with("http://127.0.0.1:"), "{proxy}");

        let env_text = std::fs::read_to_string(root.join("ytdlp.env")).unwrap();
        let env = |name: &str| {
            env_text.lines().find_map(|line| line.strip_prefix(format!("{name}=").as_str()))
        };
        for name in
            ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY", "all_proxy"]
        {
            assert_eq!(env(name), Some(proxy.as_str()), "{name}");
        }
        assert_eq!(env("NO_PROXY"), Some(""));
        assert_eq!(env("no_proxy"), Some(""));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn private_input_urls_are_rejected_before_ytdlp_runs() {
        for (offset, url) in [
            "http://127.0.0.1/stream.mpd",
            "http://[::1]/stream.mpd",
            "http://169.254.169.254/latest/meta-data",
            "http://192.168.1.1/stream.mpd",
        ]
        .iter()
        .enumerate()
        {
            let root =
                std::env::temp_dir().join(format!("falcon-ytdlp-ssrf-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let marker = root.join("started");
            let bin = write_fake_ytdlp(
                &root,
                &format!("touch \"{}\"\n{}", marker.display(), fake_success_body()),
            );
            let (ctx, _cancel) = JobContext::for_tests(120 + offset as i64);
            let out_path = root.join("video.mp4").to_string_lossy().into_owned();
            let result = process_ytdlp(
                &ctx,
                url,
                &out_path,
                YtDlpOptions {
                    ytdlp_path: Some(bin.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .await;
            match result {
                Err(DownloadError::Fatal(message)) => {
                    assert!(message.contains("Private/loopback"), "{message}")
                }
                other => panic!("private input was not rejected fatally: {other:?}"),
            }
            assert!(!marker.exists(), "yt-dlp ran for a private input URL");
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn detectably_old_ytdlp_is_refused_before_it_runs() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let root = std::env::temp_dir().join(format!("falcon-ytdlp-old-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join("started");
        let bin = write_fake_ytdlp_with_version(
            &root,
            "2023.07.05",
            &format!("touch \"{}\"\n{}", marker.display(), fake_success_body()),
        );
        let (ctx, _cancel) = JobContext::for_tests(124);
        let out_path = root.join("video.mp4").to_string_lossy().into_owned();
        let result = process_ytdlp(
            &ctx,
            TEST_INPUT_URL,
            &out_path,
            YtDlpOptions {
                ytdlp_path: Some(bin.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await;
        match result {
            Err(DownloadError::Fatal(message)) => {
                assert!(message.contains("yt-dlp 2023.07.06 or newer"), "{message}")
            }
            other => panic!("old yt-dlp was not refused: {other:?}"),
        }
        assert!(!marker.exists(), "old yt-dlp reached the download path");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cookies_use_scoped_netscape_file_and_are_removed_after_run() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let root =
            std::env::temp_dir().join(format!("falcon-ytdlp-cookies-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let captured = root.join("captured-cookies.txt");
        let captured_mode = root.join("captured-cookies.mode");
        let bin = write_fake_ytdlp(
            &root,
            &format!(
                "if [ -n \"$cookie_file\" ]; then\n  cp \"$cookie_file\" \"{}\"\n  ( stat -f '%Lp' \"$cookie_file\" 2>/dev/null || stat -c '%a' \"$cookie_file\" 2>/dev/null ) > \"{}\"\nfi\nprintf 'ERROR: stop after cookie capture\\n' >&2\nexit 1\n",
                captured.display(),
                captured_mode.display()
            ),
        );
        let (ctx, _cancel) = JobContext::for_tests(125);
        let out_path = root.join("video.mp4").to_string_lossy().into_owned();
        let result = process_ytdlp(
            &ctx,
            TEST_INPUT_URL,
            &out_path,
            YtDlpOptions {
                cookies: Some("sid=secret; encoded=a=b".into()),
                ytdlp_path: Some(bin.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await;
        assert!(result.is_err(), "the fixture should stop after capturing cookies");
        let written = std::fs::read_to_string(&captured).unwrap();
        assert!(written.starts_with("# Netscape HTTP Cookie File\n"));
        assert!(written.contains("127.0.0.1\tFALSE\t/\tTRUE\t0\tsid\tsecret\n"));
        assert!(written.contains("127.0.0.1\tFALSE\t/\tTRUE\t0\tencoded\ta=b\n"));
        assert_eq!(std::fs::read_to_string(&captured_mode).unwrap().trim(), "600");
        assert!(
            !run_dir_for(root.to_str().unwrap(), 125).join("cookies.txt").exists(),
            "the cookie file must be removed even when the run fails"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_metadata_aborts_before_output_is_written() {
        for (id, live_status, is_live) in [(91, "is_live", "True"), (92, "is_upcoming", "False")] {
            let root =
                std::env::temp_dir().join(format!("falcon-ytdlp-live-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let bin = write_fake_ytdlp(
                &root,
                &format!(
                    "printf 'FALCON_META|8192|{live_status}|{is_live}|Live stream\\n'\nsleep 1\n{}",
                    fake_success_body()
                ),
            );
            let result = run_fake_ytdlp(&root, id, &bin).await;
            assert_eq!(result, Err(DownloadError::fatal("ERR_HLS_LIVE")));
            assert!(
                !run_dir_for(root.to_str().unwrap(), id).join("output.mp4").exists(),
                "live output must not be written"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn finished_live_streams_download_normally() {
        let root = std::env::temp_dir().join(format!("falcon-ytdlp-vod-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let bin = write_fake_ytdlp(
            &root,
            &format!(
                "printf 'FALCON_META|2048|was_live|False|Finished live stream\\n'\n{}",
                fake_success_body()
            ),
        );
        let result = run_fake_ytdlp(&root, 93, &bin).await;
        assert!(result.is_ok(), "finished live stream was rejected: {result:?}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn postprocess_temp_file_growth_is_reported_as_bytes() {
        let root = std::env::temp_dir().join(format!("falcon-ytdlp-pp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let bin = write_fake_ytdlp(
            &root,
            r#"printf 'FALCON_META|4096|not_live|False|Merge me\n'
printf 'FALCON_DL|finished|4096|4096|NA|best\n'
printf 'FALCON_PP|started|Merger\n'
tmp="$(dirname "$out")/output.temp.mp4"
rm -f "$tmp"
dd if=/dev/zero bs=1024 count=1 >> "$tmp" 2>/dev/null
sleep 0.2
dd if=/dev/zero bs=1024 count=1 >> "$tmp" 2>/dev/null
sleep 0.2
dd if=/dev/zero bs=1024 count=1 >> "$tmp" 2>/dev/null
sleep 0.2
dd if=/dev/zero bs=1024 count=1 >> "$tmp" 2>/dev/null
printf 'FALCON_PP|finished|Merger\n'
mv "$tmp" "$out"
"#,
        );
        let _loopback = crate::util::allow_loopback_for_tests();
        let (ctx, _cancel) = JobContext::for_tests(94);
        let progress = ctx.progress.clone();
        let out_path = root.join("video.mp4").to_string_lossy().into_owned();
        let opts = YtDlpOptions {
            ytdlp_path: Some(bin.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let worker =
            tokio::spawn(async move { process_ytdlp(&ctx, TEST_INPUT_URL, &out_path, opts).await });
        let mut intermediate = std::collections::BTreeSet::new();
        while !worker.is_finished() {
            let bytes = progress.downloaded();
            if (1..4096).contains(&bytes) {
                intermediate.insert(bytes);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let result = worker.await.unwrap();
        assert!(result.is_ok(), "fake merge failed: {result:?}");
        assert!(
            intermediate.len() >= 2,
            "postprocess bytes did not grow visibly; samples: {intermediate:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_kills_ytdlp_process_group_including_grandchildren() {
        let root = std::env::temp_dir().join(format!("falcon-ytdlp-kill-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let heartbeat = root.join("heartbeat");
        let pid_file = root.join("grandchild.pid");
        let bin = write_fake_ytdlp(
            &root,
            &format!(
                "printf 'FALCON_DL|downloading|1|2048|NA|best\\n'\n\
                 heartbeat=\"{heartbeat}\"\n\
                 pid_file=\"{pid_file}\"\n\
                 sh -c 'while :; do printf x >> \"$1\"; sleep 0.05; done' sh \"$heartbeat\" &\n\
                 echo $! > \"$pid_file\"\n\
                 wait\n",
                heartbeat = heartbeat.display(),
                pid_file = pid_file.display()
            ),
        );
        let _loopback = crate::util::allow_loopback_for_tests();
        let (ctx, cancel) = JobContext::for_tests(95);
        let out_path = root.join("video.mp4").to_string_lossy().into_owned();
        let opts = YtDlpOptions {
            ytdlp_path: Some(bin.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let worker =
            tokio::spawn(async move { process_ytdlp(&ctx, TEST_INPUT_URL, &out_path, opts).await });
        for _ in 0..100 {
            if pid_file.exists() && heartbeat.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(pid_file.exists(), "fake ffmpeg did not start");
        cancel.send(true).unwrap();
        let result = worker.await.unwrap();

        // Give a survivor time to prove it is alive, then sample twice: a zombie
        // pid cannot keep appending, but a live orphan can.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let first = heartbeat.metadata().map(|m| m.len()).unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(250)).await;
        let second = heartbeat.metadata().map(|m| m.len()).unwrap_or(0);
        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid.trim().parse::<i32>() {
                // Test cleanup only: the assertion below decides whether production killed it.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        let heartbeat_stopped = first == second;
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(result, Err(DownloadError::Cancelled));
        assert!(heartbeat_stopped, "grandchild survived cancellation ({first} -> {second} bytes)");
    }

    #[test]
    fn audio_extensions_keep_their_container() {
        assert!(dest_is_audio(Path::new("/tmp/song.m4a")));
        assert!(dest_is_audio(Path::new("/tmp/song.MP3")));
        assert!(!dest_is_audio(Path::new("/tmp/clip.mp4")));
    }
}
