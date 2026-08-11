use crate::util::LEGACY_DEFAULT_API_TOKEN;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use url::Url;
use uuid::Uuid;

// ponytail: per-site download profile. Matched against parsed URL origin
// boundaries, so an attacker-controlled hostname cannot inherit another
// site's cookies or headers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadProfile {
    /// Human label shown in Settings.
    pub name: String,
    /// Host or origin pattern (e.g. "example.com" or "https://example.com:8443").
    pub url_pattern: String,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    /// Subdirectory under the save root (e.g. "SiteA"). Empty = default folder.
    #[serde(default)]
    pub save_subdir: Option<String>,
}

impl DownloadProfile {
    /// Match exact hostname; explicit scheme/port in the pattern also match.
    pub fn matches(&self, url: &str) -> bool {
        let raw_pattern = self.url_pattern.trim();
        if raw_pattern.is_empty() {
            return false;
        }
        let explicit_scheme = raw_pattern.contains("://");
        let pattern_url = if explicit_scheme {
            Url::parse(raw_pattern).ok()
        } else {
            Url::parse(&format!("https://{raw_pattern}")).ok()
        };
        let target = Url::parse(url).ok();
        let (Some(pattern), Some(target)) = (pattern_url, target) else {
            return false;
        };
        if !matches!(target.scheme(), "http" | "https") {
            return false;
        }
        if self.cookies.as_deref().is_some_and(|cookies| !cookies.trim().is_empty())
            && target.scheme() != "https"
        {
            return false;
        }
        pattern.host_str().is_some()
            && pattern.host_str().map(str::to_ascii_lowercase)
                == target.host_str().map(str::to_ascii_lowercase)
            && (!explicit_scheme || pattern.scheme() == target.scheme())
            && (pattern.port().is_none() || pattern.port() == target.port())
    }
}

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
    /// Chrome/Edge extension IDs allowed to call the local API.
    #[serde(default)]
    pub allowed_extension_ids: Vec<String>,
    /// Optional absolute path to yt-dlp binary (empty = PATH + common locations).
    #[serde(default)]
    pub ytdlp_path: String,
    /// ponytail: per-site download profiles. Empty by default. When a download
    /// URL matches a profile's `url_pattern`, that profile's UA/referer/cookies/
    /// subdir override the request defaults.
    #[serde(default)]
    pub download_profiles: Vec<DownloadProfile>,
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
            download_profiles: Vec::new(),
        }
    }
}

impl Settings {
    fn ensure_secure_token(mut settings: Self) -> Self {
        let weak =
            settings.api_token.trim().is_empty() || settings.api_token == LEGACY_DEFAULT_API_TOKEN;
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
        // ponytail: atomic write — write to a temp sibling then rename. A direct
        // fs::write can leave a half-written settings.json if the app crashes
        // mid-write, silently wiping the user's config on the next load.
        let tmp_path = app_data_dir.join(format!("settings.json.tmp.{}", uuid::Uuid::new_v4()));
        fs::write(&tmp_path, &content).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
        }
        fs::rename(&tmp_path, &settings_path).map_err(|e| {
            // Best-effort cleanup of the temp file if rename failed.
            let _ = fs::remove_file(&tmp_path);
            e.to_string()
        })?;
        Ok(())
    }

    pub fn path_for_category(&self, category: &str) -> Option<String> {
        self.category_paths.get(category).cloned()
    }

    /// ponytail: first profile whose origin matches. Profiles are tried in
    /// declaration order so the user controls
    /// precedence by reordering them in Settings.
    pub fn profile_for_url(&self, url: &str) -> Option<&DownloadProfile> {
        self.download_profiles.iter().find(|p| p.matches(url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_matching_uses_hostname_boundaries() {
        let profile = DownloadProfile { url_pattern: "example.com".into(), ..Default::default() };
        assert!(profile.matches("https://example.com/video.mp4"));
        assert!(!profile.matches("https://example.com.evil.test/video.mp4"));
        assert!(!profile.matches("https://attacker.example.com/video.mp4"));
    }

    #[test]
    fn cookie_profiles_never_match_plain_http() {
        let profile = DownloadProfile {
            url_pattern: "example.com".into(),
            cookies: Some("sid=secret".into()),
            ..Default::default()
        };
        assert!(profile.matches("https://example.com/video.mp4"));
        assert!(!profile.matches("http://example.com/video.mp4"));
    }

    #[test]
    fn explicit_profile_origin_matches_scheme_and_port() {
        let profile = DownloadProfile {
            url_pattern: "https://example.com:8443".into(),
            ..Default::default()
        };
        assert!(profile.matches("https://example.com:8443/video.mp4"));
        assert!(!profile.matches("http://example.com:8443/video.mp4"));
        assert!(!profile.matches("https://example.com/video.mp4"));
    }
}
