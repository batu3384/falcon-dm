//! ChaCha20-Poly1305 for cookies at rest (SQLite + settings.json).
//! Key: `cookie.key` in app data (0600). Tests use a process temp dir.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use uuid::Uuid;

const PREFIX: &str = "v1:";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

fn key_dir() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join(format!("falcon-dm-cookie-key-{}", std::process::id()))
    } else {
        super::app_data_dir()
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    let mut filled = 0;
    while filled < N {
        let chunk = Uuid::new_v4();
        let bytes = chunk.as_bytes();
        let n = (N - filled).min(bytes.len());
        out[filled..filled + n].copy_from_slice(&bytes[..n]);
        filled += n;
    }
    out
}

fn load_key() -> [u8; KEY_LEN] {
    static KEY: OnceLock<[u8; KEY_LEN]> = OnceLock::new();
    *KEY.get_or_init(|| {
        let dir = key_dir();
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("cookie.key");
        if let Ok(bytes) = fs::read(&path) {
            if let Ok(arr) = <[u8; KEY_LEN]>::try_from(bytes.as_slice()) {
                return arr;
            }
        }
        let key = random_bytes::<KEY_LEN>();
        if fs::write(&path, key).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
            }
        }
        key
    })
}

/// Encrypt cookies for disk. Empty/None stay None. Already-sealed values pass through.
pub fn seal_cookies(plain: Option<&str>) -> Option<String> {
    let plain = plain.map(str::trim).filter(|s| !s.is_empty())?;
    if plain.starts_with(PREFIX) {
        return Some(plain.to_string());
    }
    let cipher = ChaCha20Poly1305::new(&Key::from(load_key()));
    let nonce_bytes = random_bytes::<NONCE_LEN>();
    let nonce = Nonce::from(nonce_bytes);
    match cipher.encrypt(&nonce, plain.as_bytes()) {
        Ok(ciphertext) => {
            let mut packed = Vec::with_capacity(NONCE_LEN + ciphertext.len());
            packed.extend_from_slice(&nonce_bytes);
            packed.extend_from_slice(&ciphertext);
            Some(format!("{PREFIX}{}", STANDARD.encode(packed)))
        }
        Err(e) => {
            log::error!("cookie seal failed: {e}");
            None
        }
    }
}

/// Decrypt cookies from disk. Legacy plaintext (no `v1:` prefix) is returned as-is.
pub fn open_cookies(stored: Option<&str>) -> Option<String> {
    let stored = stored.map(str::trim).filter(|s| !s.is_empty())?;
    let Some(payload) = stored.strip_prefix(PREFIX) else {
        return Some(stored.to_string());
    };
    let packed = STANDARD.decode(payload).ok()?;
    if packed.len() <= NONCE_LEN {
        log::warn!("cookie ciphertext too short");
        return None;
    }
    let nonce_arr: [u8; NONCE_LEN] = packed[..NONCE_LEN].try_into().ok()?;
    let nonce = Nonce::from(nonce_arr);
    let cipher = ChaCha20Poly1305::new(&Key::from(load_key()));
    match cipher.decrypt(&nonce, &packed[NONCE_LEN..]) {
        Ok(plain) => String::from_utf8(plain).ok(),
        Err(e) => {
            log::warn!("cookie open failed: {e}");
            None
        }
    }
}

pub fn cookies_are_sealed(stored: Option<&str>) -> bool {
    stored.is_some_and(|s| s.trim().starts_with(PREFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip_hides_plaintext() {
        let sealed = seal_cookies(Some("sid=secret")).expect("seal");
        assert!(sealed.starts_with(PREFIX));
        assert!(!sealed.contains("sid=secret"));
        assert_eq!(open_cookies(Some(&sealed)).as_deref(), Some("sid=secret"));
        assert_eq!(open_cookies(Some("sid=legacy")).as_deref(), Some("sid=legacy"));
        assert!(seal_cookies(Some("  ")).is_none());
        assert!(open_cookies(None).is_none());
    }
}
