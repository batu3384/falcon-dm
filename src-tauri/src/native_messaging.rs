use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const MAX_NATIVE_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_PAIR_PROOFS: usize = 1024;
const MAX_PAIR_CLIENTS: usize = 16;
const PAIRING_IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CHALLENGE_BYTES: usize = 256;

#[derive(Debug, Deserialize, Serialize)]
pub struct NativePairRequest {
    pub extension_id: String,
    pub challenge: String,
}

#[derive(Debug, Serialize)]
pub struct NativePairResponse {
    pub ok: bool,
    pub proof: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PairRequest {
    pub extension_id: String,
    pub challenge: String,
    pub proof: String,
}

struct StoredProof {
    proof: String,
    expires_at: Instant,
}

#[derive(Default)]
pub struct PairProofStore {
    entries: Mutex<HashMap<(String, String), StoredProof>>,
}

pub fn validate_native_pair_request(request: &NativePairRequest) -> Result<(), String> {
    if request.extension_id.len() != 32
        || !request.extension_id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
    {
        return Err("invalid extension id".into());
    }
    if request.challenge.is_empty() || request.challenge.len() > MAX_CHALLENGE_BYTES {
        return Err("invalid pairing challenge".into());
    }
    Ok(())
}

impl PairProofStore {
    pub fn issue(&self, challenge: &str, extension_id: &str) -> String {
        let proof = Uuid::new_v4().to_string();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge_expired(&mut entries);
        if entries.len() >= MAX_PAIR_PROOFS {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, stored)| stored.expires_at)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            (challenge.to_string(), extension_id.to_string()),
            StoredProof {
                proof: proof.clone(),
                expires_at: Instant::now() + Duration::from_secs(60),
            },
        );
        proof
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    pub fn consume(&self, challenge: &str, extension_id: &str, proof: &str) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge_expired(&mut entries);
        let key = (challenge.to_string(), extension_id.to_string());
        if entries.get(&key).is_some_and(|stored| stored.proof == proof) {
            entries.remove(&key);
            true
        } else {
            false
        }
    }

    fn purge_expired(entries: &mut HashMap<(String, String), StoredProof>) {
        let now = Instant::now();
        entries.retain(|_, stored| stored.expires_at > now);
    }
}

pub fn read_native_message<R: Read>(reader: &mut R) -> Result<Vec<u8>, String> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length).map_err(|e| e.to_string())?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_NATIVE_MESSAGE_BYTES {
        return Err("native message is too large".into());
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).map_err(|e| e.to_string())?;
    Ok(payload)
}

pub fn write_native_message<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), String> {
    if payload.len() > MAX_NATIVE_MESSAGE_BYTES {
        return Err("native message is too large".into());
    }
    let length = u32::try_from(payload.len()).map_err(|e| e.to_string())?;
    writer
        .write_all(&length.to_le_bytes())
        .and_then(|_| writer.write_all(payload))
        .map_err(|e| e.to_string())
}

#[cfg(unix)]
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("pairing.sock")
}

#[cfg(unix)]
pub fn start_pairing_server(
    data_dir: &Path,
    store: std::sync::Arc<PairProofStore>,
) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    let path = socket_path(data_dir);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                return Err("native pairing server is already running".into());
            }
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            UnixListener::bind(&path).map_err(|e| e.to_string())?
        }
        Err(error) => return Err(error.to_string()),
    };
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;

    let clients = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_PAIR_CLIENTS));
    tauri::async_runtime::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let store = store.clone();
            let clients = clients.clone();
            let Ok(permit) = clients.try_acquire_owned() else {
                continue;
            };
            tokio::spawn(async move {
                let result = tokio::time::timeout(PAIRING_IO_TIMEOUT, async {
                    let length = stream.read_u32_le().await.map_err(|e| e.to_string())? as usize;
                    if length > MAX_NATIVE_MESSAGE_BYTES {
                        return Err("native message is too large".to_string());
                    }
                    let mut bytes = vec![0u8; length];
                    stream.read_exact(&mut bytes).await.map_err(|e| e.to_string())?;
                    let request: NativePairRequest =
                        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                    validate_native_pair_request(&request)?;
                    let proof = store.issue(&request.challenge, &request.extension_id);
                    let response = serde_json::to_vec(&NativePairResponse {
                        ok: true,
                        proof: Some(proof),
                        error: None,
                    })
                    .map_err(|e| e.to_string())?;
                    stream.write_u32_le(response.len() as u32).await.map_err(|e| e.to_string())?;
                    stream.write_all(&response).await.map_err(|e| e.to_string())?;
                    Ok::<(), String>(())
                })
                .await
                .map_err(|_| "native pairing timed out".to_string())
                .and_then(|result| result);
                drop(permit);
                if let Err(error) = result {
                    log::warn!("native pairing request failed: {error}");
                }
            });
        }
    });
    Ok(())
}

#[cfg(not(unix))]
pub fn start_pairing_server(
    _data_dir: &Path,
    _store: std::sync::Arc<PairProofStore>,
) -> Result<(), String> {
    Err("native pairing requires a Unix platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_message_uses_little_endian_length_prefix() {
        let mut out = Vec::new();
        write_native_message(&mut out, br#"{"ok":true}"#).unwrap();
        assert_eq!(&out[..4], &(11u32).to_le_bytes());
        assert_eq!(&out[4..], br#"{"ok":true}"#);
    }

    #[test]
    fn native_message_rejects_oversized_frame() {
        let mut input = (65_537u32).to_le_bytes().to_vec();
        input.extend(std::iter::repeat_n(b'x', 65_537));
        assert!(read_native_message(&mut input.as_slice()).is_err());
    }

    #[test]
    fn pair_proof_is_single_use_and_extension_bound() {
        let store = PairProofStore::default();
        let proof = store.issue("challenge-1", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert!(store.consume("challenge-1", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &proof));
        assert!(!store.consume("challenge-1", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &proof));
        assert!(!store.consume("challenge-1", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &proof));
    }

    #[test]
    fn native_pair_requests_are_bounded() {
        assert!(validate_native_pair_request(&NativePairRequest {
            extension_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            challenge: "challenge".into(),
        })
        .is_ok());
        assert!(validate_native_pair_request(&NativePairRequest {
            extension_id: "short".into(),
            challenge: "challenge".into(),
        })
        .is_err());
    }

    #[test]
    fn proof_store_evicts_oldest_expiry_at_capacity() {
        let store = PairProofStore::default();
        for index in 0..MAX_PAIR_PROOFS {
            store.issue(&format!("challenge-{index}"), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        }
        assert_eq!(store.len(), MAX_PAIR_PROOFS);
        store.issue("newest", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(store.len(), MAX_PAIR_PROOFS);
        assert!(!store.consume("challenge-0", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", ""));
    }
}
