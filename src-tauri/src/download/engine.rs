use reqwest::Client;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;
use uuid::Uuid;

use crate::util::lock_or_recover;

#[derive(Debug)]
pub enum EngineError {
    RpcError(String),
    NetworkError(reqwest::Error),
    IoError(std::io::Error),
    TauriError(tauri::Error),
    NotRunning(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::RpcError(s) => write!(f, "RPC error: {s}"),
            EngineError::NetworkError(e) => write!(f, "Network error: {e}"),
            EngineError::IoError(e) => write!(f, "IO error: {e}"),
            EngineError::TauriError(e) => write!(f, "Tauri error: {e}"),
            EngineError::NotRunning(s) => write!(f, "{s}"),
        }
    }
}

impl From<reqwest::Error> for EngineError {
    fn from(err: reqwest::Error) -> Self {
        EngineError::NetworkError(err)
    }
}

impl From<std::io::Error> for EngineError {
    fn from(err: std::io::Error) -> Self {
        EngineError::IoError(err)
    }
}

impl From<tauri::Error> for EngineError {
    fn from(err: tauri::Error) -> Self {
        EngineError::TauriError(err)
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;

pub struct Aria2Options {
    pub dir: String,
    pub filename: String,
    pub split: u32,
    pub max_connections: u32,
    pub headers: Vec<String>,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
}

const RPC_PORT: u16 = 6800;

pub struct Aria2Engine {
    client: Client,
    rpc_url: String,
    secret: String,
    secret_file: Mutex<Option<PathBuf>>,
    pid_file: Mutex<Option<PathBuf>>,
    process: Mutex<Option<CommandChild>>,
    running: Mutex<bool>,
}

impl Default for Aria2Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Aria2Engine {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            rpc_url: format!("http://127.0.0.1:{RPC_PORT}/jsonrpc"),
            secret: Uuid::new_v4().to_string(),
            secret_file: Mutex::new(None),
            pid_file: Mutex::new(None),
            process: Mutex::new(None),
            running: Mutex::new(false),
        }
    }

    pub fn is_running(&self) -> bool {
        *lock_or_recover(&self.running)
    }

    pub fn mark_not_running(&self) {
        *lock_or_recover(&self.running) = false;
    }

    /// Kill only our previously tracked aria2 PID — never blanket-kill port 6800.
    /// Verifies cmdline contains aria2 before kill (PID reuse guard).
    fn reclaim_own_pid(app_data_dir: &std::path::Path) {
        #[cfg(unix)]
        {
            let pid_path = app_data_dir.join("aria2.pid");
            if let Ok(s) = std::fs::read_to_string(&pid_path) {
                if let Ok(pid) = s.trim().parse::<i32>() {
                    let looks_aria2 = std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
                        .ok()
                        .map(|c| c.contains("aria2"))
                        .unwrap_or_else(|| {
                            // macOS: no /proc — use ps
                            Command::new("ps")
                                .args(["-p", &pid.to_string(), "-o", "comm="])
                                .output()
                                .ok()
                                .and_then(|o| String::from_utf8(o.stdout).ok())
                                .map(|c| c.contains("aria2"))
                                .unwrap_or(false)
                        });
                    if looks_aria2 {
                        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
                        log::warn!("Reclaimed own aria2 pid {pid}");
                    } else {
                        log::warn!("Stale aria2.pid {pid} is not aria2 — not killing");
                    }
                }
            }
            let _ = std::fs::remove_file(&pid_path);
            std::thread::sleep(Duration::from_millis(200));
        }
        #[cfg(not(unix))]
        {
            let _ = app_data_dir;
        }
    }

    /// Write RPC secret and start sidecar. Marks running only after caller confirms ready.
    pub fn start(
        &self,
        app_handle: &tauri::AppHandle,
        app_data_dir: &std::path::Path,
    ) -> Result<()> {
        let _ = self.stop();
        Self::reclaim_own_pid(app_data_dir);

        std::fs::create_dir_all(app_data_dir)?;
        // ponytail: write the RPC secret to an aria2 config file (0600) and pass
        // it via --conf-path, instead of `--rpc-secret=...` on the command line.
        // The CLI form leaks the secret to any local user via `ps`/`ps aux`.
        // aria2 has no --rpc-secret-file flag, but it reads `rpc-secret=<value>`
        // from a config file, which keeps it out of the process argument list.
        let conf_path = app_data_dir.join("aria2.conf");
        let conf_content = format!(
            "rpc-secret={}\nenable-rpc=true\nrpc-listen-port={}\n\
             rpc-allow-origin-all=false\nrpc-listen-all=false\n",
            self.secret, RPC_PORT
        );
        std::fs::write(&conf_path, &conf_content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&conf_path, std::fs::Permissions::from_mode(0o600));
        }

        let secret_path = app_data_dir.join("aria2_rpc_secret");
        std::fs::write(&secret_path, &self.secret)?;
        // ponytail: restrict secret file to 0600 (parity with settings.json).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600));
        }
        *lock_or_recover(&self.secret_file) = Some(secret_path);

        let mut cmd = app_handle
            .shell()
            .sidecar("aria2c")
            .map_err(|e| EngineError::IoError(std::io::Error::other(e.to_string())))?;

        let conf_arg = format!("--conf-path={}", conf_path.to_string_lossy());
        cmd = cmd.args([
            &conf_arg,
            "--max-concurrent-downloads=10",
            "--max-connection-per-server=16",
            "--split=16",
            "--min-split-size=1M",
            "--continue=true",
            "--auto-file-renaming=false",
            "--allow-overwrite=false",
            "--quiet=true",
            "--daemon=false",
        ]);

        match cmd.spawn() {
            Ok((_rx, child)) => {
                let pid = child.pid();
                let pid_path = app_data_dir.join("aria2.pid");
                let _ = std::fs::write(&pid_path, pid.to_string());
                *lock_or_recover(&self.pid_file) = Some(pid_path);
                let mut p = lock_or_recover(&self.process);
                *p = Some(child);
                // Do NOT mark running yet — wait_ready / mark_ready owns that.
                *lock_or_recover(&self.running) = false;
                log::info!("Spawned aria2c sidecar; waiting for RPC...");
                Ok(())
            }
            Err(e) => {
                *lock_or_recover(&self.running) = false;
                log::error!("Failed to start aria2c: {}", e);
                Err(EngineError::NotRunning(format!("Failed to start aria2c sidecar: {e}")))
            }
        }
    }

    pub fn mark_ready(&self) {
        *lock_or_recover(&self.running) = true;
    }

    pub async fn wait_ready(&self, attempts: u32) -> Result<()> {
        // Temporarily allow RPC during probe
        *lock_or_recover(&self.running) = true;
        for i in 0..attempts {
            match self.get_global_stat().await {
                Ok(_) => {
                    log::info!("aria2 RPC ready after {} attempts", i + 1);
                    return Ok(());
                }
                Err(e) => {
                    log::debug!("aria2 not ready yet: {}", e);
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        *lock_or_recover(&self.running) = false;
        Err(EngineError::NotRunning("aria2 RPC did not become ready".into()))
    }

    /// Ensure engine is up; restart once if needed.
    pub async fn ensure_running(
        &self,
        app_handle: &tauri::AppHandle,
        app_data_dir: &std::path::Path,
    ) -> Result<()> {
        if self.is_running() && self.get_global_stat().await.is_ok() {
            return Ok(());
        }
        log::warn!("aria2 not healthy — restarting");
        self.start(app_handle, app_data_dir)?;
        self.wait_ready(30).await?;
        Ok(())
    }

    pub async fn apply_speed_limit(&self, kbps: u32) -> Result<()> {
        let limit = if kbps == 0 { "0".to_string() } else { format!("{}K", kbps) };
        let opts = json!({ "max-overall-download-limit": limit });
        self.call_rpc("aria2.changeGlobalOption", vec![opts]).await?;
        Ok(())
    }

    pub async fn apply_proxy(&self, proxy: Option<&str>) -> Result<()> {
        let all_proxy = proxy.unwrap_or("");
        let opts = json!({ "all-proxy": all_proxy });
        self.call_rpc("aria2.changeGlobalOption", vec![opts]).await?;
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        let mut p = lock_or_recover(&self.process);
        if let Some(child) = p.take() {
            let _ = child.kill();
        }
        *lock_or_recover(&self.running) = false;
        if let Some(path) = lock_or_recover(&self.secret_file).take() {
            let _ = std::fs::remove_file(path);
        }
        // ponytail: also remove the aria2.conf (contains the rpc-secret) on stop.
        let _ = std::fs::remove_file(
            lock_or_recover(&self.secret_file)
                .as_ref()
                .and_then(|p| p.parent())
                .map(|d| d.join("aria2.conf"))
                .unwrap_or_else(|| std::path::PathBuf::from("aria2.conf")),
        );
        if let Some(path) = lock_or_recover(&self.pid_file).take() {
            #[cfg(unix)]
            if let Ok(s) = std::fs::read_to_string(&path) {
                if let Ok(pid) = s.trim().parse::<i32>() {
                    let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
                }
            }
            let _ = std::fs::remove_file(path);
        }
        Ok(())
    }

    async fn call_rpc(&self, method: &str, mut params: Vec<Value>) -> Result<Value> {
        if !*lock_or_recover(&self.running) {
            return Err(EngineError::NotRunning("aria2 is not running".into()));
        }

        let mut final_params = vec![json!(format!("token:{}", self.secret))];
        final_params.append(&mut params);

        let payload = json!({
            "jsonrpc": "2.0",
            "id": "falcon-dm",
            "method": method,
            "params": final_params
        });

        let res = self.client.post(&self.rpc_url).json(&payload).send().await?;
        let text = res.text().await?;
        let parsed: Value =
            serde_json::from_str(&text).map_err(|e| EngineError::RpcError(e.to_string()))?;

        if let Some(err) = parsed.get("error") {
            return Err(EngineError::RpcError(err.to_string()));
        }

        Ok(parsed["result"].clone())
    }

    pub async fn add_download(&self, url: &str, options: Aria2Options) -> Result<String> {
        let mut aria_opts = serde_json::Map::new();
        aria_opts.insert("dir".to_string(), json!(options.dir));
        if !options.filename.trim().is_empty() {
            aria_opts.insert("out".to_string(), json!(options.filename));
        }
        aria_opts.insert("split".to_string(), json!(options.split.to_string()));
        aria_opts.insert(
            "max-connection-per-server".to_string(),
            json!(options.max_connections.to_string()),
        );

        if !options.headers.is_empty() {
            aria_opts.insert("header".to_string(), json!(options.headers));
        }
        if let Some(ref r) = options.referrer {
            aria_opts.insert("referer".to_string(), json!(r));
        }
        if let Some(ref ua) = options.user_agent {
            aria_opts.insert("user-agent".to_string(), json!(ua));
        }

        let params = vec![json!([url]), json!(aria_opts)];
        let res = self.call_rpc("aria2.addUri", params).await?;
        Ok(res.as_str().unwrap_or("").to_string())
    }

    pub async fn pause(&self, gid: &str) -> Result<()> {
        self.call_rpc("aria2.pause", vec![json!(gid)]).await?;
        Ok(())
    }

    pub async fn resume(&self, gid: &str) -> Result<()> {
        self.call_rpc("aria2.unpause", vec![json!(gid)]).await?;
        Ok(())
    }

    pub async fn remove(&self, gid: &str) -> Result<()> {
        self.call_rpc("aria2.remove", vec![json!(gid)]).await?;
        Ok(())
    }

    pub async fn get_status(&self, gid: &str) -> Result<Value> {
        self.call_rpc("aria2.tellStatus", vec![json!(gid)]).await
    }

    pub async fn get_global_stat(&self) -> Result<Value> {
        self.call_rpc("aria2.getGlobalStat", vec![]).await
    }

    /// Batch-fetch all active downloads in one RPC (aria2.tellActive).
    /// Returns status objects, each containing a "gid" plus progress fields.
    pub async fn get_active_statuses(&self) -> Result<Vec<Value>> {
        let res = self.call_rpc("aria2.tellActive", vec![]).await?;
        Ok(res.as_array().cloned().unwrap_or_default())
    }
}
