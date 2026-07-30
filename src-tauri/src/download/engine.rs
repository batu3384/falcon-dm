use reqwest::Client;
use serde_json::{json, Value};
use std::process::{Child, Command};
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Debug)]
pub enum EngineError {
    RpcError(String),
    NetworkError(reqwest::Error),
    IoError(std::io::Error),
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
    process: Mutex<Option<Child>>,
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

    pub fn start(&self, aria2_path: &str) -> Result<()> {
        let mut cmd = Command::new(aria2_path);
        cmd.arg("--enable-rpc=true")
            .arg("--rpc-listen-port=6800")
            .arg(format!("--rpc-secret={}", self.secret))
            .arg("--max-concurrent-downloads=10")
            .arg("--max-connection-per-server=16")
            .arg("--split=16")
            .arg("--min-split-size=1M")
            .arg("--continue=true")
            .arg("--auto-file-renaming=false")
            .arg("--allow-overwrite=true")
            .arg("--quiet=true")
            .arg("--daemon=false");

        match cmd.spawn() {
            Ok(child) => {
                let mut p = self.process.lock().unwrap();
                *p = Some(child);
                log::info!("Started aria2c daemon successfully.");
                Ok(())
            }
            Err(e) => {
                log::warn!("Failed to start aria2c (path: {}): {}", aria2_path, e);
                // Return gracefully without crashing, as required by brief
                Ok(())
            }
        }
    }

    pub fn stop(&self) -> Result<()> {
        let mut p = self.process.lock().unwrap();
        if let Some(mut child) = p.take() {
            let _ = child.kill();
            let _ = child.wait();
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
