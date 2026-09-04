//! Chrome native-messaging host: length-prefixed stdin/stdout ↔ pairing.sock.
//!
//! Must not link Tauri/AppKit. The previous in-tree bin pulled `falcon_dm_lib`,
//! so Chrome's spawn hung before `main` (sendNativeMessage timed out).

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

const MAX_NATIVE_MESSAGE_BYTES: usize = 64 * 1024;

fn read_frame<R: Read>(reader: &mut R) -> Result<Vec<u8>, String> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length).map_err(|e| e.to_string())?;
    let n = u32::from_le_bytes(length) as usize;
    if n > MAX_NATIVE_MESSAGE_BYTES {
        return Err("native message is too large".into());
    }
    let mut payload = vec![0u8; n];
    reader.read_exact(&mut payload).map_err(|e| e.to_string())?;
    Ok(payload)
}

fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), String> {
    if payload.len() > MAX_NATIVE_MESSAGE_BYTES {
        return Err("native message is too large".into());
    }
    let n = u32::try_from(payload.len()).map_err(|e| e.to_string())?;
    writer
        .write_all(&n.to_le_bytes())
        .and_then(|_| writer.write_all(payload))
        .map_err(|e| e.to_string())
}

fn error_payload(error: &str) -> Vec<u8> {
    serde_json::json!({ "ok": false, "proof": null, "error": error }).to_string().into_bytes()
}

fn write_error(error: &str) {
    let mut stdout = io::stdout();
    let _ = write_frame(&mut stdout, &error_payload(error));
    let _ = stdout.flush();
}

fn data_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .ok_or_else(|| "application data directory is unavailable".to_string())
        .map(|dir| dir.join("com.falcondm.app"))
}

#[cfg(unix)]
fn run() -> Result<(), String> {
    use std::os::unix::net::UnixStream;

    let payload = {
        let mut stdin = io::stdin();
        read_frame(&mut stdin)?
    };

    let mut socket =
        UnixStream::connect(data_dir()?.join("pairing.sock")).map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .and_then(|_| socket.set_write_timeout(Some(Duration::from_secs(5))))
        .map_err(|e| e.to_string())?;
    write_frame(&mut socket, &payload)?;
    let response = read_frame(&mut socket)?;
    let mut stdout = io::stdout();
    write_frame(&mut stdout, &response)?;
    stdout.flush().map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn run() -> Result<(), String> {
    Err("native pairing requires a Unix platform".into())
}

fn main() {
    if let Err(error) = run() {
        write_error(&error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_roundtrip() {
        let payload = br#"{"ok":true}"#;
        let mut buf = Vec::new();
        write_frame(&mut buf, payload).unwrap();
        assert_eq!(&buf[..4], &(payload.len() as u32).to_le_bytes());
        let decoded = read_frame(&mut Cursor::new(buf)).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn frame_rejects_oversize_length() {
        let mut input = (65_537u32).to_le_bytes().to_vec();
        input.extend(std::iter::repeat_n(b'x', 8));
        assert!(read_frame(&mut Cursor::new(input)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn forwards_length_prefixed_unix_socket() {
        use std::os::unix::net::UnixListener;
        use std::thread;

        let dir = std::env::temp_dir().join(format!("fdm-nm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock_path = dir.join("pairing.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let incoming = read_frame(&mut stream).unwrap();
            assert_eq!(incoming, b"{\"challenge\":\"x\"}");
            write_frame(&mut stream, br#"{"ok":true,"proof":"p"}"#).unwrap();
        });

        let mut stream = std::os::unix::net::UnixStream::connect(&sock_path).unwrap();
        write_frame(&mut stream, br#"{"challenge":"x"}"#).unwrap();
        let response = read_frame(&mut stream).unwrap();
        assert_eq!(response, br#"{"ok":true,"proof":"p"}"#);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
