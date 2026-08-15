use std::path::{Component, Path, PathBuf};

/// Reject path traversal and absolute paths in filenames.
pub fn sanitize_filename(name: &str) -> String {
    let name = name.trim().replace('\\', "/");
    let base = name
        .rsplit('/')
        .next()
        .unwrap_or("download")
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0'))
        .collect::<String>();
    let base = base.trim().trim_start_matches('.');
    if base.is_empty() || base == ".." {
        "falcon_download".to_string()
    } else {
        base.chars().take(200).collect()
    }
}

/// Resolve save_path under allowed root; reject escape attempts.
pub fn resolve_save_dir(requested: &str, allowed_root: &Path) -> Result<PathBuf, String> {
    let expanded = expand_tilde(requested);
    let root = allowed_root.canonicalize().unwrap_or_else(|_| allowed_root.to_path_buf());

    let candidate = if expanded.is_absolute() { expanded } else { root.join(&expanded) };

    // Ensure no `..` components remain after normalize
    let mut clean = PathBuf::new();
    for comp in candidate.components() {
        match comp {
            Component::ParentDir => {
                if !clean.pop() {
                    return Err("Invalid save path".into());
                }
            }
            Component::CurDir => {}
            other => clean.push(other.as_os_str()),
        }
    }

    // If path doesn't exist yet, canonicalize parent and append
    let verified = if clean.exists() {
        clean.canonicalize().map_err(|e| e.to_string())?
    } else {
        if let Some(parent) = clean.parent() {
            let parent_canon = if parent.exists() {
                parent.canonicalize().map_err(|e| e.to_string())?
            } else {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                parent.canonicalize().map_err(|e| e.to_string())?
            };
            parent_canon.join(clean.file_name().unwrap_or_default())
        } else {
            clean
        }
    };

    if !verified.starts_with(&root) {
        // Allow if under home Downloads as secondary root
        if let Some(dl) = dirs::download_dir() {
            let dl_root = dl.canonicalize().unwrap_or(dl);
            if verified.starts_with(&dl_root) {
                return Ok(verified);
            }
        }
        return Err("Save path must be under the Downloads folder".into());
    }
    Ok(verified)
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

pub fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Downloads"))
}

pub fn app_data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("com.falcondm.app")
}

pub fn full_file_path(save_path: &str, filename: &str) -> PathBuf {
    PathBuf::from(save_path).join(sanitize_filename(filename))
}

/// Resolve a single safe filename under an already validated save directory.
pub fn resolve_download_target(save_dir: &str, filename: &str) -> Result<PathBuf, String> {
    let raw = filename.trim().replace('\\', "/");
    let path = Path::new(&raw);
    if raw.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err("Invalid download filename".into());
    }
    let dir =
        Path::new(save_dir).canonicalize().map_err(|e| format!("Invalid save directory: {e}"))?;
    if !dir.is_dir() {
        return Err("Save path is not a directory".into());
    }
    Ok(dir.join(sanitize_filename(&raw)))
}

/// Copy a file without overwriting an existing destination, then remove source.
pub fn copy_file_exclusive(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err("Destination file already exists".into());
    }
    let mut input = std::fs::File::open(source).map_err(|e| e.to_string())?;
    let mut output =
        match std::fs::OpenOptions::new().write(true).create_new(true).open(destination) {
            Ok(file) => file,
            Err(error) => return Err(error.to_string()),
        };
    if let Err(error) = std::io::copy(&mut input, &mut output) {
        let _ = std::fs::remove_file(destination);
        return Err(error.to_string());
    }
    if let Err(error) = output.sync_all() {
        let _ = std::fs::remove_file(destination);
        return Err(error.to_string());
    }
    let source_size = std::fs::metadata(source).map_err(|e| e.to_string())?.len();
    let destination_size = std::fs::metadata(destination).map_err(|e| e.to_string())?.len();
    if source_size != destination_size {
        let _ = std::fs::remove_file(destination);
        return Err("Copied file size does not match source".into());
    }
    std::fs::remove_file(source).map_err(|e| e.to_string())?;
    Ok(())
}

/// Reject completed downloads that are obvious error pages — any file type allowed.
pub fn validate_completed_file(path: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Err("Downloaded file not found".into());
    }
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let size = meta.len();
    if size == 0 {
        return Err("Downloaded file is empty".into());
    }
    // Only sniff tiny payloads — likely HTML/JSON/protobuf error responses
    if size < 512 {
        let mut head = [0u8; 64];
        let read = {
            use std::io::Read;
            let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
            f.read(&mut head).unwrap_or(0)
        };
        if read > 0 {
            let is_text = head[0] == b'<'
                || head[0] == b'{'
                || head.starts_with(b"HTTP")
                || head.starts_with(b"<!DOC");
            if is_text {
                return Err("Server returned an error page instead of a file".into());
            }
            // YouTube DASH/sabr error blobs
            if head.windows(4).any(|w| w == b"sabr") {
                return Err("Invalid stream response".into());
            }
        }
    }
    Ok(size)
}

/// Ensure path is under Downloads or configured root before opening.
pub fn validate_open_path(path: &str) -> Result<PathBuf, String> {
    let expanded = expand_tilde(path);
    let candidate = if expanded.exists() {
        expanded.canonicalize().map_err(|e| e.to_string())?
    } else if let Some(parent) = expanded.parent() {
        let parent_c = if parent.exists() {
            parent.canonicalize().map_err(|e| e.to_string())?
        } else {
            return Err("Path does not exist".into());
        };
        parent_c.join(expanded.file_name().unwrap_or_default())
    } else {
        return Err("Invalid path".into());
    };

    let mut allowed = vec![default_download_dir()];
    if let Some(home) = dirs::home_dir() {
        allowed.push(home.join("Downloads"));
        allowed.push(home.join("Movies"));
        allowed.push(home.join("Music"));
        allowed.push(home.join("Documents"));
    }
    allowed.push(app_data_dir());

    for root in allowed {
        let root_c = root.canonicalize().unwrap_or(root);
        if candidate.starts_with(&root_c) {
            return Ok(candidate);
        }
    }
    Err("Path is outside allowed directories".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename_blocks_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a/b/c.zip"), "c.zip");
        assert!(!sanitize_filename("foo\0bar").contains('\0'));
    }

    #[test]
    fn move_target_rejects_traversal_and_absolute_filename() {
        let root = std::env::temp_dir().join(format!("falcon-dm-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(resolve_download_target(root.to_str().unwrap(), "../outside.mp4").is_err());
        assert!(resolve_download_target(root.to_str().unwrap(), "/tmp/outside.mp4").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_file_exclusive_does_not_overwrite_existing_destination() {
        let root = std::env::temp_dir().join(format!("falcon-dm-copy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.bin");
        let destination = root.join("destination.bin");
        std::fs::write(&source, b"source").unwrap();
        std::fs::write(&destination, b"existing").unwrap();
        assert!(copy_file_exclusive(&source, &destination).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn test_validate_open_path_rejects_etc() {
        let res = validate_open_path("/etc/passwd");
        assert!(res.is_err());
    }

    #[test]
    fn test_validate_completed_file_rejects_tiny() {
        let dir = std::env::temp_dir().join("falcon_test_tiny");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("error.html");
        std::fs::write(&p, b"<html>error</html>").unwrap();
        assert!(validate_completed_file(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
