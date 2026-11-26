use std::collections::HashSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Reject path traversal and absolute paths in filenames.
pub fn sanitize_filename(name: &str) -> String {
    let name = name.trim().replace('\\', "/");
    let mut base = name
        .rsplit('/')
        .next()
        .unwrap_or("download")
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0'))
        .collect::<String>();
    base = base.trim().trim_start_matches('.').trim_end_matches([' ', '.']).to_string();
    if base.is_empty() || base == ".." {
        return "falcon_download".to_string();
    }
    if is_windows_reserved_stem(&base) {
        base = format!("dl_{base}");
    }
    base.chars().take(200).collect()
}

fn is_windows_reserved_stem(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    )
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

    // Never mkdir before the allowlist check — rejected paths must leave no dirs behind.
    let verified = resolve_without_creating(&clean)?;

    if !verified.starts_with(&root) {
        // Allow if under home Downloads as secondary root
        if let Some(dl) = dirs::download_dir() {
            let dl_root = dl.canonicalize().unwrap_or(dl);
            if verified.starts_with(&dl_root) {
                return Ok(verified);
            }
        }
        return Err("Save path is outside allowed directories".into());
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

/// True for `/`, `/etc`, `/tmp`, and other system prefixes that must never be a download root.
pub fn is_unsafe_download_root(path: &Path) -> bool {
    let normals: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str().map(|s| s.to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    if normals.len() < 2 {
        return true;
    }
    matches!(
        normals[0].as_str(),
        "etc"
            | "usr"
            | "bin"
            | "sbin"
            | "system"
            | "library"
            | "private"
            | "dev"
            | "proc"
            | "root"
            | "var"
            | "tmp"
            | "opt"
            | "cores"
            | "windows"
            | "program files"
            | "program files (x86)"
    )
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

/// Move a file without overwriting an existing destination.
///
/// Same volume: `hard_link` + `unlink` (atomic, O(1), fails if destination exists).
/// Other volume or no hard-link support (exFAT): exclusive byte copy, then remove source.
pub fn move_file_exclusive(source: &Path, destination: &Path) -> Result<(), String> {
    match std::fs::hard_link(source, destination) {
        Ok(()) => {
            // Destination now owns the data. A leftover source name is only clutter.
            let _ = std::fs::remove_file(source);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err("Destination file already exists".into())
        }
        Err(_) => copy_file_exclusive(source, destination),
    }
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
    if let Err(error) = std::fs::remove_file(source) {
        let _ = std::fs::remove_file(destination);
        return Err(error.to_string());
    }
    Ok(())
}

/// Move into `destination`, or the next free `name (n).ext` if it got taken meanwhile.
pub fn move_file_unique(
    source: &Path,
    destination: &Path,
    reserved: &HashSet<String>,
) -> Result<PathBuf, String> {
    let dir = destination.parent().ok_or_else(|| "Invalid output path".to_string())?;
    let name = destination
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid output filename".to_string())?;
    let dir_str = dir.to_str().ok_or_else(|| "Save path is not valid UTF-8".to_string())?;
    // Two workers may race for the same free name; the exclusive move decides the winner.
    for _ in 0..4 {
        let target = unique_download_target_reserved(dir_str, name, reserved)?;
        match move_file_exclusive(source, &target) {
            Ok(()) => return Ok(target),
            Err(error) if error == "Destination file already exists" => continue,
            Err(error) => return Err(error),
        }
    }
    Err("Could not pick a unique filename".into())
}

/// Free bytes on the volume holding `path` (or its nearest existing ancestor).
pub fn free_disk_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut probe = path;
        while !probe.exists() {
            probe = probe.parent()?;
        }
        let c_path = std::ffi::CString::new(probe.as_os_str().as_bytes()).ok()?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: c_path is NUL-terminated and stat points to writable memory of the right size.
        let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        // SAFETY: statvfs returned 0, so the struct is initialized.
        let stat = unsafe { stat.assume_init() };
        // Field widths differ per platform (u32 blocks on macOS, u64 on Linux).
        #[allow(clippy::unnecessary_cast)]
        let free = (stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64);
        Some(free)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// File types whose real content legitimately starts with `<` or `{`.
fn is_text_like_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).is_some_and(|ext| {
        matches!(
            ext.as_str(),
            "html"
                | "htm"
                | "xhtml"
                | "xml"
                | "svg"
                | "json"
                | "txt"
                | "csv"
                | "md"
                | "js"
                | "css"
                | "plist"
                | "rss"
                | "atom"
                | "opml"
                | "gpx"
                | "kml"
                | "srt"
                | "vtt"
                | "m3u"
                | "m3u8"
                | "mpd"
        )
    })
}

/// Reject completed downloads that are obvious error pages.
///
/// Only tiny files are sniffed, and only when the name promises a binary type:
/// a 200-byte `config.json` or `icon.svg` is a valid download.
pub fn validate_completed_file(path: &Path) -> Result<u64, String> {
    validate_completed_file_named(path, path)
}

/// Like `validate_completed_file`, but the type hint comes from `named`
/// (a `.part` file is validated against the final destination name).
pub fn validate_completed_file_named(path: &Path, named: &Path) -> Result<u64, String> {
    if !path.exists() {
        return Err("Downloaded file not found".into());
    }
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let size = meta.len();
    if size == 0 {
        return Err("Downloaded file is empty".into());
    }
    if size < 512 && !is_text_like_extension(named) {
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

#[derive(Debug, PartialEq, Eq)]
pub enum ExistingDownloadDest {
    Missing,
    Complete(u64),
    Occupied,
}

/// Classify dest without mutating it — never delete a file we did not create.
pub fn inspect_download_dest(path: &Path) -> ExistingDownloadDest {
    if !path.exists() {
        return ExistingDownloadDest::Missing;
    }
    match validate_completed_file(path) {
        Ok(size) => ExistingDownloadDest::Complete(size),
        Err(_) => ExistingDownloadDest::Occupied,
    }
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
        allowed.push(home);
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

fn resolve_without_creating(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return path.canonicalize().map_err(|e| e.to_string());
    }
    let mut ancestor = path.to_path_buf();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or_else(|| "Invalid save path".to_string())?;
        missing.push(name.to_os_string());
        ancestor = ancestor
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "Invalid save path".to_string())?;
    }
    let mut verified = ancestor.canonicalize().map_err(|e| e.to_string())?;
    for name in missing.into_iter().rev() {
        verified.push(name);
    }
    Ok(verified)
}

fn filename_is_taken(path: &Path, reserved: &HashSet<String>) -> bool {
    if path.exists() {
        return true;
    }
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| reserved.contains(n))
}

/// Next free name under `save_dir` (`file.mp4` → `file (1).mp4`) without overwriting.
pub fn unique_download_target(save_dir: &str, filename: &str) -> Result<PathBuf, String> {
    unique_download_target_reserved(save_dir, filename, &HashSet::new())
}

pub fn unique_download_target_reserved(
    save_dir: &str,
    filename: &str,
    reserved: &HashSet<String>,
) -> Result<PathBuf, String> {
    let dest = resolve_download_target(save_dir, filename)?;
    if !filename_is_taken(&dest, reserved) {
        return Ok(dest);
    }
    let stem =
        dest.file_stem().and_then(|s| s.to_str()).filter(|s| !s.is_empty()).unwrap_or("download");
    let ext = dest.extension().and_then(|s| s.to_str());
    for n in 1..=99 {
        let candidate_name = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dest.with_file_name(sanitize_filename(&candidate_name));
        if !filename_is_taken(&candidate, reserved) {
            return Ok(candidate);
        }
    }
    Err("Could not pick a unique filename".into())
}

pub fn unique_download_filename(save_dir: &str, filename: &str) -> Result<String, String> {
    unique_download_filename_reserved(save_dir, filename, &HashSet::new())
}

pub fn unique_download_filename_reserved(
    save_dir: &str,
    filename: &str,
    reserved: &HashSet<String>,
) -> Result<String, String> {
    unique_download_target_reserved(save_dir, filename, reserved)?
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "invalid filename".into())
}

/// Create a new file and write bytes. Never truncates an existing path.
pub fn write_file_exclusive(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut output = match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) => return Err(error.to_string()),
    };
    if let Err(error) = output.write_all(bytes) {
        let _ = std::fs::remove_file(path);
        return Err(error.to_string());
    }
    if let Err(error) = output.sync_all() {
        let _ = std::fs::remove_file(path);
        return Err(error.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename_blocks_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a/b/c.zip"), "c.zip");
        assert!(!sanitize_filename("foo\0bar").contains('\0'));
        assert_eq!(sanitize_filename("CON.txt"), "dl_CON.txt");
        assert_eq!(sanitize_filename("nul"), "dl_nul");
        assert_eq!(sanitize_filename("report.pdf."), "report.pdf");
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
    fn copy_file_exclusive_moves_bytes_and_removes_source() {
        let root = std::env::temp_dir().join(format!("falcon-dm-copy-ok-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.bin");
        let destination = root.join("destination.bin");
        std::fs::write(&source, b"payload").unwrap();
        copy_file_exclusive(&source, &destination).unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"payload");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_file_exclusive_renames_without_overwriting() {
        let root = std::env::temp_dir().join(format!("falcon-dm-move-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join(".a.part");
        let destination = root.join("a.bin");
        std::fs::write(&source, b"payload").unwrap();
        move_file_exclusive(&source, &destination).unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"payload");

        let second = root.join(".b.part");
        std::fs::write(&second, b"other").unwrap();
        assert_eq!(
            move_file_exclusive(&second, &destination).unwrap_err(),
            "Destination file already exists"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"payload");
        assert!(second.exists(), "a refused move must keep the source");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_file_unique_picks_next_free_name() {
        let root = std::env::temp_dir().join(format!("falcon-dm-moveu-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("clip.mp4"), b"existing").unwrap();
        let source = root.join(".clip.part");
        std::fs::write(&source, b"new").unwrap();
        let landed = move_file_unique(&source, &root.join("clip.mp4"), &HashSet::new()).unwrap();
        assert_eq!(landed.file_name().unwrap(), "clip (1).mp4");
        assert_eq!(std::fs::read(root.join("clip.mp4")).unwrap(), b"existing");
        assert_eq!(std::fs::read(&landed).unwrap(), b"new");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tiny_text_downloads_are_valid_but_tiny_binary_error_pages_are_not() {
        let root = std::env::temp_dir().join(format!("falcon-dm-tiny-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let json = root.join("config.json");
        std::fs::write(&json, b"{\"ok\":true}").unwrap();
        assert!(validate_completed_file(&json).is_ok());
        let svg = root.join("icon.svg");
        std::fs::write(&svg, b"<svg></svg>").unwrap();
        assert!(validate_completed_file(&svg).is_ok());
        let zip = root.join("archive.zip");
        std::fs::write(&zip, b"<html>403</html>").unwrap();
        assert!(validate_completed_file(&zip).is_err());
        // A part file is judged by the name it will get.
        let part = root.join(".config.json.7.falcon.part");
        std::fs::write(&part, b"{\"ok\":true}").unwrap();
        assert!(validate_completed_file_named(&part, &json).is_ok());
        assert!(validate_completed_file_named(&part, &zip).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn free_disk_space_reports_for_missing_child_path() {
        let missing = std::env::temp_dir().join("falcon-dm-free-space-missing/child/file.bin");
        assert!(free_disk_space(&missing).is_some_and(|bytes| bytes > 0));
    }

    #[test]
    fn test_validate_open_path_rejects_etc() {
        let res = validate_open_path("/etc/passwd");
        assert!(res.is_err());
    }

    #[test]
    fn unsafe_download_root_rejects_system_prefixes() {
        assert!(is_unsafe_download_root(Path::new("/")));
        assert!(is_unsafe_download_root(Path::new("/etc")));
        assert!(is_unsafe_download_root(Path::new("/tmp")));
        assert!(is_unsafe_download_root(Path::new("/usr/local")));
        assert!(is_unsafe_download_root(Path::new("/private/tmp/falcon")));
        assert!(!is_unsafe_download_root(Path::new("/Users/me/Downloads")));
        assert!(!is_unsafe_download_root(Path::new("/Volumes/SSD/Falcon")));
        assert!(!is_unsafe_download_root(Path::new("/home/me/dl")));
    }

    #[test]
    fn test_validate_completed_file_rejects_tiny() {
        let dir = std::env::temp_dir().join("falcon_test_tiny");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("error.mp4");
        std::fs::write(&p, b"<html>error</html>").unwrap();
        assert!(validate_completed_file(&p).is_err());
        assert_eq!(inspect_download_dest(&p), ExistingDownloadDest::Occupied);
        assert_eq!(std::fs::read(&p).unwrap(), b"<html>error</html>");
        let missing = dir.join("nope.bin");
        assert_eq!(inspect_download_dest(&missing), ExistingDownloadDest::Missing);
        let ok = dir.join("ok.bin");
        std::fs::write(&ok, vec![0u8; 1024]).unwrap();
        assert!(matches!(inspect_download_dest(&ok), ExistingDownloadDest::Complete(1024)));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&ok);
    }

    #[test]
    fn resolve_save_dir_does_not_mkdir_outside_root() {
        let root =
            std::env::temp_dir().join(format!("falcon-dm-save-root-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let outside =
            std::env::temp_dir().join(format!("falcon-dm-save-out-{}", uuid::Uuid::new_v4()));
        let nested = outside.join("nested");
        let err = resolve_save_dir(nested.to_str().unwrap(), &root).unwrap_err();
        assert!(err.contains("outside allowed"), "{err}");
        assert!(!outside.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unique_download_target_does_not_overwrite() {
        let root = std::env::temp_dir().join(format!("falcon-dm-unique-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let first = unique_download_target(root.to_str().unwrap(), "clip.mp4").unwrap();
        assert_eq!(first.file_name().unwrap(), "clip.mp4");
        std::fs::write(&first, b"one").unwrap();
        let second = unique_download_target(root.to_str().unwrap(), "clip.mp4").unwrap();
        assert_eq!(second.file_name().unwrap(), "clip (1).mp4");
        write_file_exclusive(&second, b"two").unwrap();
        assert_eq!(
            unique_download_filename(root.to_str().unwrap(), "clip.mp4").unwrap(),
            "clip (2).mp4"
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert!(write_file_exclusive(&first, b"nope").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unique_filename_after_extension_rewrite_does_not_collide() {
        let root = std::env::temp_dir().join(format!("falcon-dm-rewrite-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("clip.mp4"), b"existing").unwrap();
        assert_eq!(
            unique_download_filename(root.to_str().unwrap(), "clip.webm").unwrap(),
            "clip.webm"
        );
        assert_eq!(
            unique_download_filename(root.to_str().unwrap(), "clip.mp4").unwrap(),
            "clip (1).mp4"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unique_filename_skips_reserved_names_without_disk_file() {
        let root =
            std::env::temp_dir().join(format!("falcon-dm-reserved-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut reserved = std::collections::HashSet::new();
        reserved.insert("clip.mp4".into());
        assert_eq!(
            unique_download_filename_reserved(root.to_str().unwrap(), "clip.mp4", &reserved)
                .unwrap(),
            "clip (1).mp4"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
