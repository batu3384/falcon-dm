//! Locating external tools. A Finder-launched app gets a minimal PATH
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), so Homebrew and pip installs are searched
//! explicitly and handed to child processes.

use std::path::{Path, PathBuf};

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Directories where user-installed CLI tools usually live on macOS/Linux.
fn extra_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".deno/bin"));
        dirs.push(home.join("bin"));
        // pip --user: ~/Library/Python/3.x/bin
        if let Ok(entries) = std::fs::read_dir(home.join("Library/Python")) {
            dirs.extend(entries.flatten().map(|e| e.path().join("bin")));
        }
    }
    // python.org installer: /Library/Frameworks/Python.framework/Versions/3.x/bin
    if let Ok(entries) = std::fs::read_dir("/Library/Frameworks/Python.framework/Versions") {
        dirs.extend(entries.flatten().map(|e| e.path().join("bin")));
    }
    dirs
}

/// PATH for child processes: the inherited one plus the usual install dirs.
pub fn augmented_path() -> std::ffi::OsString {
    let mut parts: Vec<PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for dir in extra_bin_dirs() {
        if !parts.contains(&dir) {
            parts.push(dir);
        }
    }
    std::env::join_paths(parts).unwrap_or_default()
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&augmented_path()).map(|dir| dir.join(name)).find(|p| is_executable(p))
}

/// Directory of the running binary. Test binaries live one level deeper (`deps/`).
fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(if dir.ends_with("deps") { dir.parent()?.to_path_buf() } else { dir.to_path_buf() })
}

/// Bundled sidecar first (`Contents/MacOS/ffmpeg`, or `target/<profile>/ffmpeg`
/// under `tauri dev`), then a system install.
pub fn find_ffmpeg() -> Result<PathBuf, String> {
    if let Some(bundled) = exe_dir().map(|dir| dir.join("ffmpeg")).filter(|p| is_executable(p)) {
        return Ok(bundled);
    }
    find_in_path("ffmpeg").ok_or_else(|| "ERR_FFMPEG_MISSING".to_string())
}

pub fn find_ytdlp(preferred: Option<&str>) -> Result<PathBuf, String> {
    if let Some(raw) = preferred.map(str::trim).filter(|s| !s.is_empty()) {
        let path = crate::util::expand_tilde(raw);
        if is_executable(&path) {
            return Ok(path);
        }
        return Err(format!("ERR_YTDLP_PATH_INVALID: {}", path.display()));
    }
    find_in_path("yt-dlp").ok_or_else(|| "ERR_YTDLP_MISSING".to_string())
}

/// First yt-dlp release whose redirect handling stopped forwarding a caller-supplied Cookie header.
pub const MIN_YTDLP_VERSION: (u32, u32, u32) = (2023, 7, 6);

/// Parse `yt-dlp --version` (`YYYY.MM.DD`; nightly builds may append numeric segments).
pub fn parse_ytdlp_version(output: &str) -> Option<(u32, u32, u32)> {
    let text = output.split_whitespace().next()?;
    let mut parts = text.split('.');
    let year = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if !(2000..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if parts.any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    Some((year, month, day))
}

/// Unknown versions pass: only a detectably vulnerable release is rejected.
pub fn ytdlp_version_needs_upgrade(output: &str) -> bool {
    parse_ytdlp_version(output).is_some_and(|version| version < MIN_YTDLP_VERSION)
}

/// Best-effort `--version` probe. Failure means "unknown", not "unsafe".
pub async fn detect_ytdlp_version(path: &Path) -> Option<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        tokio::process::Command::new(path)
            .arg("--version")
            .env("PATH", augmented_path())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout.lines().next()?.trim();
    (!version.is_empty()).then(|| version.chars().take(40).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn augmented_path_keeps_inherited_entries_and_adds_homebrew() {
        let path = augmented_path();
        let parts: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert!(parts.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(parts.contains(&PathBuf::from("/usr/local/bin")));
        if let Some(first) = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).next())
        {
            assert_eq!(parts.first(), Some(&first), "inherited PATH order must win");
        }
    }

    #[test]
    fn ytdlp_version_dates_parse_and_compare_security_floor() {
        assert_eq!(parse_ytdlp_version("2023.07.06"), Some((2023, 7, 6)));
        assert_eq!(parse_ytdlp_version("2023.07.06.123456\n"), Some((2023, 7, 6)));
        assert_eq!(parse_ytdlp_version("2024.10.22"), Some((2024, 10, 22)));
        assert_eq!(parse_ytdlp_version("not-a-version"), None);
        assert_eq!(parse_ytdlp_version("2023.13.01"), None);
        assert!(ytdlp_version_needs_upgrade("2023.07.05"));
        assert!(!ytdlp_version_needs_upgrade("2023.07.06"));
        assert!(!ytdlp_version_needs_upgrade("2024.01.01"));
        assert!(!ytdlp_version_needs_upgrade("custom build"));
    }

    #[test]
    fn explicit_ytdlp_path_must_exist() {
        let err = find_ytdlp(Some("/nonexistent/falcon/yt-dlp")).unwrap_err();
        assert!(err.starts_with("ERR_YTDLP_PATH_INVALID"), "{err}");
        // Blank preference falls through to the search.
        assert_eq!(find_ytdlp(Some("  ")).is_ok(), find_ytdlp(None).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_files_are_not_tools() {
        let dir = std::env::temp_dir().join(format!("falcon-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("yt-dlp");
        std::fs::write(&plain, b"#!/bin/sh\n").unwrap();
        assert!(!is_executable(&plain));
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable(&plain));
        assert!(find_ytdlp(Some(plain.to_str().unwrap())).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
