use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub theme: String, // "system", "light", "dark"
    pub default_download_path: String,
    pub max_concurrent_downloads: u32,
    pub max_connections_per_server: u32,
    pub proxy: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            default_download_path: "~/Downloads".to_string(),
            max_concurrent_downloads: 3,
            max_connections_per_server: 16,
            proxy: None,
        }
    }
}

impl Settings {
    pub fn load(app_data_dir: &std::path::Path) -> Self {
        let settings_path = app_data_dir.join("settings.json");
        if settings_path.exists() {
            if let Ok(content) = fs::read_to_string(&settings_path) {
                if let Ok(settings) = serde_json::from_str(&content) {
                    return settings;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self, app_data_dir: &std::path::Path) -> Result<(), String> {
        let settings_path = app_data_dir.join("settings.json");

        // Ensure the directory exists
        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let content = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&settings_path, content).map_err(|e| e.to_string())
    }
}
