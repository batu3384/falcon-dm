use std::fs;
use std::path::{Path, PathBuf};

fn validate_extension_id(id: &str) -> Result<(), String> {
    if id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte)) {
        Ok(())
    } else {
        Err("invalid extension id".into())
    }
}

struct NativeHostTarget {
    dir: PathBuf,
    origin: String,
}

pub fn native_host_install_supported() -> bool {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        true
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

fn native_host_install_targets(chrome_extension_id: &str, edge_extension_id: &str) -> Result<Vec<NativeHostTarget>, String> {
    let home = dirs::home_dir().ok_or_else(|| "home directory unavailable".to_string())?;
    let chrome_origin = format!("chrome-extension://{chrome_extension_id}/");
    let edge_origin = format!("edge-extension://{edge_extension_id}/");
    let mut targets = Vec::new();

    #[cfg(target_os = "macos")]
    {
        targets.push(NativeHostTarget {
            dir: home
                .join("Library/Application Support/Google/Chrome/NativeMessagingHosts"),
            origin: chrome_origin.clone(),
        });
        targets.push(NativeHostTarget {
            dir: home
                .join("Library/Application Support/Microsoft Edge/NativeMessagingHosts"),
            origin: edge_origin.clone(),
        });
    }

    #[cfg(target_os = "linux")]
    {
        targets.push(NativeHostTarget {
            dir: home.join(".config/google-chrome/NativeMessagingHosts"),
            origin: chrome_origin.clone(),
        });
        targets.push(NativeHostTarget {
            dir: home.join(".config/chromium/NativeMessagingHosts"),
            origin: chrome_origin,
        });
        targets.push(NativeHostTarget {
            dir: home.join(".config/microsoft-edge/NativeMessagingHosts"),
            origin: edge_origin,
        });
    }

    #[cfg(target_os = "windows")]
    {
        let local = dirs::data_local_dir().ok_or_else(|| "local app data unavailable".to_string())?;
        targets.push(NativeHostTarget {
            dir: local.join("Google/Chrome/User Data/NativeMessagingHosts"),
            origin: format!("chrome-extension://{chrome_extension_id}/"),
        });
        targets.push(NativeHostTarget {
            dir: local.join("Microsoft/Edge/User Data/NativeMessagingHosts"),
            origin: format!("edge-extension://{edge_extension_id}/"),
        });
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (home, chrome_origin, edge_origin);
        return Err("native host install is not supported on this platform".into());
    }

    if targets.is_empty() {
        return Err("no native host install targets for this platform".into());
    }
    Ok(targets)
}

fn write_manifest(target_dir: &Path, executable: &Path, origin: &str) -> Result<(), String> {
    fs::create_dir_all(target_dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(target_dir, fs::Permissions::from_mode(0o700));
    }
    let manifest_path = target_dir.join("com.falcondm.native.json");
    let temp_path = target_dir.join(".com.falcondm.native.tmp");
    let body = serde_json::json!({
        "name": "com.falcondm.native",
        "description": "Falcon DM native pairing host",
        "path": executable.canonicalize().unwrap_or_else(|_| executable.to_path_buf()),
        "type": "stdio",
        "allowed_origins": [origin],
    });
    fs::write(&temp_path, serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&temp_path, &manifest_path).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn install_native_host_manifests(
    executable: &Path,
    chrome_extension_id: &str,
    edge_extension_id: &str,
) -> Result<(), String> {
    validate_extension_id(chrome_extension_id)?;
    validate_extension_id(edge_extension_id)?;
    if !executable.is_file() {
        return Err(format!("native host binary not found: {}", executable.display()));
    }
    let targets = native_host_install_targets(chrome_extension_id, edge_extension_id)?;
    let mut installed = 0usize;
    let mut errors = Vec::new();
    for target in targets {
        match write_manifest(&target.dir, executable, &target.origin) {
            Ok(()) => installed += 1,
            Err(err) => errors.push(format!("{}: {err}", target.dir.display())),
        }
    }
    if installed == 0 {
        return Err(errors.join("; "));
    }
    Ok(())
}

pub fn resolve_native_host_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("FALCON_NATIVE_HOST_BIN") {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            for name in ["falcon-dm-native-host", "../Resources/falcon-dm-native-host"] {
                let candidate = parent.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/falcon-dm-native-host");
    if dev.is_file() {
        return Some(dev);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_extension_id_rejects_short_ids() {
        assert!(validate_extension_id("abc").is_err());
    }

    #[test]
    fn native_host_targets_include_chrome_on_supported_platforms() {
        if !native_host_install_supported() {
            return;
        }
        let targets = native_host_install_targets(
            "abcdefghijklmnopqrstuvwxyzabcdef",
            "abcdefghijklmnopqrstuvwxyzabcdef",
        )
        .unwrap();
        assert!(!targets.is_empty());
        assert!(targets.iter().any(|target| target.origin.starts_with("chrome-extension://")));
    }
}
