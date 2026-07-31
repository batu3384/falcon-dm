use crate::util::LEGACY_DEFAULT_API_TOKEN;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub theme: String,
    pub default_download_path: String,
    pub max_concurrent_downloads: u32,
    pub max_connections_per_server: u32,
    pub proxy: Option<String>,
    #[serde(default)]
    pub api_token: String,
    #[serde(default)]
    pub speed_limit_kbps: u32,
    #[serde(default)]
    pub category_paths: HashMap<String, String>,
    #[serde(default)]
    pub schedule_active: bool,
    #[serde(default)]
    pub schedule_start: Option<String>,
    #[serde(default)]
    pub schedule_stop: Option<String>,
    /// Chrome/Firefox extension IDs allowed to call the local API (empty = any extension origin, token still required).
    #[serde(default)]
    pub allowed_extension_ids: Vec<String>,
    /// Optional absolute path to yt-dlp binary (empty = PATH + common locations).
    #[serde(default)]
    pub ytdlp_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            default_download_path: dirs::download_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "~/Downloads".to_string()),
            max_concurrent_downloads: 3,
            max_connections_per_server: 16,
            proxy: None,
            api_token: Uuid::new_v4().to_string(),
            speed_limit_kbps: 0,
            category_paths: HashMap::new(),
            schedule_active: false,
            schedule_start: None,
            schedule_stop: None,
            allowed_extension_ids: Vec::new(),
            ytdlp_path: String::new(),
        }
    }
}

impl Settings {
    fn ensure_secure_token(mut settings: Self) -> Self {
        let weak = settings.api_token.trim().is_empty()
            || settings.api_token == LEGACY_DEFAULT_API_TOKEN;
        if weak {
            settings.api_token = Uuid::new_v4().to_string();
        }
        settings
    }

    pub fn load(app_data_dir: &std::path::Path) -> Self {
        let settings_path = app_data_dir.join("settings.json");
        let mut settings = if settings_path.exists() {
            if let Ok(content) = fs::read_to_string(&settings_path) {
                serde_json::from_str::<Settings>(&content).unwrap_or_default()
            } else {
                Self::default()
            }
        } else {
            Self::default()
        };
        let before = settings.api_token.clone();
        settings = Self::ensure_secure_token(settings);
        if settings.api_token != before || !settings_path.exists() {
            let _ = settings.save(app_data_dir);
        }
        // Restrict settings.json permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&settings_path, fs::Permissions::from_mode(0o600));
        }
        settings
    }

    pub fn save(&self, app_data_dir: &std::path::Path) -> Result<(), String> {
        let settings_path = app_data_dir.join("settings.json");
        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let content = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&settings_path, content).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&settings_path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn path_for_category(&self, category: &str) -> Option<String> {
        self.category_paths.get(category).cloned()
    }
}
