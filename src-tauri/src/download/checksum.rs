//! Expected-checksum parsing and verification for finished downloads.
//!
//! Users paste a hash in lenient form (bare hex or `algo:`-prefixed, any
//! case); it is stored canonical as `algo:hex-lowercase` and verified against
//! the final file bytes inside `JobContext::finalize` — after content
//! validation, before the file moves into place. A mismatch deletes the temp
//! file: those bytes can neither be trusted nor incrementally repaired.

use sha2::Digest;
use std::io::Read;
use std::path::Path;

/// Error code surfaced when the file bytes do not match the expected hash.
/// Kept as a bare code (no details) so the UI can localize it.
pub const ERR_CHECKSUM_MISMATCH: &str = "ERR_CHECKSUM_MISMATCH";

/// Parse lenient user input into canonical `algo:hex` form (lowercase hex).
///
/// Accepted: bare hex of 64 chars (sha256) or 32 chars (md5), or the
/// prefixed forms `sha256:hex` / `sha-256:hex` / `md5:hex` in any case, with
/// surrounding whitespace tolerated. Anything else is not a checksum at all.
pub fn parse_expected_hash(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (algo, hex) = match trimmed.split_once(':') {
        Some((kind, hex)) => {
            let algo = match kind.trim().to_ascii_lowercase().as_str() {
                "sha256" | "sha-256" => "sha256",
                "md5" => "md5",
                _ => return None,
            };
            (algo, hex.trim())
        }
        None => match trimmed.len() {
            64 => ("sha256", trimmed),
            32 => ("md5", trimmed),
            _ => return None,
        },
    };
    let hex_len = if algo == "sha256" { 64 } else { 32 };
    if hex.len() != hex_len || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("{algo}:{}", hex.to_ascii_lowercase()))
}

/// Streaming hex digest of a file in 64 KiB reads — multi-GB files never
/// enter memory. Generic over the digest-0.10 trait both crates implement.
fn hex_digest<D: Digest>(path: &Path) -> std::io::Result<String> {
    use std::fmt::Write as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = D::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    // Byte-wise hex: the generic-array LowerHex impl needs typenum bounds a
    // generic caller cannot name.
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// Verify `path` against an expected hash (canonical or lenient form — it is
/// re-parsed here so a stored value is never trusted blindly). Returns
/// `Err(ERR_CHECKSUM_MISMATCH)` when the bytes hash to something else.
/// An unparsable expectation cannot be enforced and is skipped.
pub fn verify_file_hash(path: &Path, expected: &str) -> Result<(), String> {
    let Some(canonical) = parse_expected_hash(expected) else {
        log::warn!("stored expected hash {expected:?} is unparsable; skipping verification");
        return Ok(());
    };
    let (algo, hex) = canonical.split_once(':').expect("canonical form carries a prefix");
    let actual = match algo {
        "sha256" => hex_digest::<sha2::Sha256>(path),
        "md5" => hex_digest::<md5::Md5>(path),
        other => unreachable!("parse_expected_hash only emits sha256/md5, got {other}"),
    }
    .map_err(|error| format!("could not hash the finished file: {error}"))?;
    if actual == *hex {
        Ok(())
    } else {
        Err(ERR_CHECKSUM_MISMATCH.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const MD5_ABC: &str = "900150983cd24fb0d6963f7d28e17f72";

    #[test]
    fn bare_hex_lengths_pick_the_algorithm() {
        assert_eq!(
            parse_expected_hash(SHA256_ABC).as_deref(),
            Some(format!("sha256:{SHA256_ABC}").as_str())
        );
        assert_eq!(
            parse_expected_hash(MD5_ABC).as_deref(),
            Some(format!("md5:{MD5_ABC}").as_str())
        );
    }

    #[test]
    fn uppercase_hex_is_canonicalized_to_lowercase() {
        let upper = SHA256_ABC.to_uppercase();
        assert_eq!(
            parse_expected_hash(&upper).as_deref(),
            Some(format!("sha256:{SHA256_ABC}").as_str())
        );
        let mixed = format!("SHA256:{}", MD5_ABC.to_uppercase());
        // sha256 prefix with a 32-char hex is invalid even in uppercase.
        assert_eq!(parse_expected_hash(&mixed), None);
        let md5_upper = format!("md5:{}", MD5_ABC.to_uppercase());
        assert_eq!(
            parse_expected_hash(&md5_upper).as_deref(),
            Some(format!("md5:{MD5_ABC}").as_str())
        );
    }

    #[test]
    fn prefixed_forms_in_any_case() {
        for prefix in ["sha256:", "SHA256:", "Sha256:", "sha-256:", "SHA-256:"] {
            assert_eq!(
                parse_expected_hash(&format!("{prefix}{SHA256_ABC}")).as_deref(),
                Some(format!("sha256:{SHA256_ABC}").as_str()),
                "prefix {prefix}"
            );
        }
        for prefix in ["md5:", "MD5:", "Md5:"] {
            assert_eq!(
                parse_expected_hash(&format!("{prefix}{MD5_ABC}")).as_deref(),
                Some(format!("md5:{MD5_ABC}").as_str()),
                "prefix {prefix}"
            );
        }
    }

    #[test]
    fn whitespace_is_tolerated() {
        assert_eq!(
            parse_expected_hash(&format!("  sha256:  {SHA256_ABC}  ")).as_deref(),
            Some(format!("sha256:{SHA256_ABC}").as_str())
        );
        assert_eq!(
            parse_expected_hash(&format!("  {MD5_ABC}  ")).as_deref(),
            Some(format!("md5:{MD5_ABC}").as_str())
        );
    }

    #[test]
    fn rejects_garbage_and_wrong_lengths() {
        assert_eq!(parse_expected_hash(""), None);
        assert_eq!(parse_expected_hash("   "), None);
        assert_eq!(parse_expected_hash("abc"), None, "too short");
        assert_eq!(parse_expected_hash(&"a".repeat(63)), None);
        assert_eq!(parse_expected_hash(&"a".repeat(65)), None);
        assert_eq!(parse_expected_hash(&"a".repeat(31)), None);
        assert_eq!(parse_expected_hash(&"a".repeat(33)), None);
        // Non-hex characters.
        assert_eq!(parse_expected_hash(&"g".repeat(64)), None);
        assert_eq!(parse_expected_hash(&format!("{}g", &SHA256_ABC[..63])), None);
        // Prefix/length mismatch.
        assert_eq!(parse_expected_hash(&format!("sha256:{MD5_ABC}")), None);
        assert_eq!(parse_expected_hash(&format!("md5:{SHA256_ABC}")), None);
        // Unsupported algorithms.
        assert_eq!(parse_expected_hash(&format!("sha1:{}", "a".repeat(40))), None);
        assert_eq!(parse_expected_hash(&format!("sha512:{}", "a".repeat(128))), None);
        // Empty hex after prefix.
        assert_eq!(parse_expected_hash("sha256:"), None);
        assert_eq!(parse_expected_hash("md5:"), None);
    }

    #[test]
    fn canonical_output_roundtrips() {
        let canonical = parse_expected_hash(SHA256_ABC).unwrap();
        assert_eq!(parse_expected_hash(&canonical).as_deref(), Some(canonical.as_str()));
    }

    fn scratch_file(name: &str, bytes: &[u8]) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("falcon-sum-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("payload.bin");
        std::fs::write(&file, bytes).unwrap();
        (dir, file)
    }

    #[test]
    fn verify_accepts_matching_sha256_and_md5() {
        let (dir, file) = scratch_file("match", b"abc");
        assert_eq!(verify_file_hash(&file, &format!("sha256:{SHA256_ABC}")), Ok(()));
        assert_eq!(verify_file_hash(&file, &format!("md5:{MD5_ABC}")), Ok(()));
        // Lenient stored forms still verify.
        assert_eq!(verify_file_hash(&file, SHA256_ABC), Ok(()));
        assert_eq!(verify_file_hash(&file, &format!("SHA-256:{SHA256_ABC}")), Ok(()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn verify_rejects_mismatched_bytes() {
        let (dir, file) = scratch_file("mismatch", b"abc");
        let other_sha = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"; // sha256 of ""
        assert_eq!(
            verify_file_hash(&file, &format!("sha256:{other_sha}")),
            Err(ERR_CHECKSUM_MISMATCH.to_string())
        );
        let other_md5 = "d41d8cd98f00b204e9800998ecf8427e"; // md5 of ""
        assert_eq!(
            verify_file_hash(&file, &format!("md5:{other_md5}")),
            Err(ERR_CHECKSUM_MISMATCH.to_string())
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn verify_streams_large_files() {
        // Bigger than the read buffer: proves the loop feeds every chunk.
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let (dir, file) = scratch_file("large", &data);
        let expected = {
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            hasher.update(&data);
            format!("sha256:{:x}", hasher.finalize())
        };
        assert_eq!(verify_file_hash(&file, &expected), Ok(()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unparsable_stored_hash_is_not_enforced() {
        let (dir, file) = scratch_file("garbage", b"abc");
        assert_eq!(verify_file_hash(&file, "not a checksum"), Ok(()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_file_is_an_error_but_not_a_mismatch() {
        let missing = Path::new("/definitely/not/there.bin");
        let result = verify_file_hash(missing, SHA256_ABC);
        assert!(result.is_err());
        assert_ne!(result.unwrap_err(), ERR_CHECKSUM_MISMATCH);
    }
}
