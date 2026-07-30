use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Mutex;
use uuid::Uuid;
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;

#[derive(Debug)]
pub enum EngineError {
    RpcError(String),
    NetworkError(reqwest::Error),
    IoError(std::io::Error),
    TauriError(tauri::Error),
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

pub struct Aria2Engine {
    client: Client,
    rpc_url: String,
    secret: String,
    process: Mutex<Option<CommandChild>>,
}

impl Aria2Engine {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            rpc_url: "http://127.0.0.1:6800/jsonrpc".to_string(),
            secret: Uuid::new_v4().to_string(),
            process: Mutex::new(None),
        }
    }

    pub fn start(&self, app_handle: &tauri::AppHandle) -> Result<()> {
        let mut cmd = app_handle.shell().sidecar("aria2c").map_err(|e| EngineError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
        
        cmd = cmd.args([
            "--enable-rpc=true",
            "--rpc-listen-port=6800",
            &format!("--rpc-secret={}", self.secret),
            "--max-concurrent-downloads=10",
            "--max-connection-per-server=16",
            "--split=16",
            "--min-split-size=1M",
            "--continue=true",
            "--auto-file-renaming=false",
            "--allow-overwrite=true",
            "--quiet=true",
            "--daemon=false",
        ]);

        match cmd.spawn() {
            Ok((_rx, child)) => {
                let mut p = self.process.lock().unwrap();
                *p = Some(child);
                log::info!("Started aria2c daemon successfully.");
                Ok(())
            }
            Err(e) => {
                log::warn!("Failed to start aria2c: {}", e);
                // Return gracefully without crashing, as required by brief
                Ok(())
            }
        }
    }

    pub fn stop(&self) -> Result<()> {
        let mut p = self.process.lock().unwrap();
        if let Some(mut child) = p.take() {
            let _ = child.kill();
        }
        Ok(())
    }

    async fn call_rpc(&self, method: &str, mut params: Vec<Value>) -> Result<Value> {
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
        let parsed: Value = serde_json::from_str(&text).map_err(|e| EngineError::RpcError(e.to_string()))?;

        if let Some(err) = parsed.get("error") {
            return Err(EngineError::RpcError(err.to_string()));
        }

        Ok(parsed["result"].clone())
    }

    pub async fn add_download(&self, url: &str, options: Aria2Options) -> Result<String> {
        let mut aria_opts = serde_json::Map::new();
        aria_opts.insert("dir".to_string(), json!(options.dir));
        aria_opts.insert("out".to_string(), json!(options.filename));
        aria_opts.insert("split".to_string(), json!(options.split.to_string()));
        aria_opts.insert("max-connection-per-server".to_string(), json!(options.max_connections.to_string()));

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
}
