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
