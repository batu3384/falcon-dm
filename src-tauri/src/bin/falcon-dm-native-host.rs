use falcon_dm_lib::native_messaging::{
    read_native_message, socket_path, write_native_message, NativePairRequest, NativePairResponse,
    MAX_NATIVE_MESSAGE_BYTES,
};
use std::io::{self, Write};

#[cfg(unix)]
fn run() -> Result<(), String> {
    use std::os::unix::net::UnixStream;

    let mut stdin = io::stdin().lock();
    let request_bytes = read_native_message(&mut stdin)?;
    let request: NativePairRequest =
        serde_json::from_slice(&request_bytes).map_err(|e| e.to_string())?;

    let data_dir = dirs::data_dir()
        .ok_or_else(|| "application data directory is unavailable".to_string())?
        .join("com.falcondm.app");
    let mut socket = UnixStream::connect(socket_path(&data_dir)).map_err(|e| e.to_string())?;
    let request_bytes = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    write_native_message(&mut socket, &request_bytes)?;

    let response_bytes = read_native_message(&mut socket)?;
    if response_bytes.len() > MAX_NATIVE_MESSAGE_BYTES {
        return Err("native pairing response is too large".into());
    }
    let mut stdout = io::stdout().lock();
    write_native_message(&mut stdout, &response_bytes)?;
    stdout.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(unix))]
fn run() -> Result<(), String> {
    Err("native pairing requires a Unix platform".into())
}

fn main() {
    if let Err(error) = run() {
        let response =
            serde_json::to_vec(&NativePairResponse { ok: false, proof: None, error: Some(error) })
                .unwrap_or_else(|_| br#"{"ok":false,"error":"native pairing failed"}"#.to_vec());
        let mut stdout = io::stdout().lock();
        let _ = write_native_message(&mut stdout, &response);
        let _ = stdout.flush();
    }
}
