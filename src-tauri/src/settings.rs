use crate::is_valid_extension_id;
use crate::util::LEGACY_DEFAULT_API_TOKEN;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    /// Host-only patterns (`example.com`) match any port. Explicit origins
    /// (`https://host[:port]`) use scheme + `port_or_known_default` (443/80).
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
            && (!explicit_scheme
                || pattern.port_or_known_default() == target.port_or_known_default())
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
    /// When true, yt-dlp reads Chrome cookies (`--cookies-from-browser`). Off by default.
    #[serde(default)]
    pub ytdlp_use_browser_cookies: bool,
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
            ytdlp_use_browser_cookies: false,
            download_profiles: Vec::new(),
        }
    }
}

impl Settings {
    pub fn clamp_runtime_bounds(&mut self) {
        self.max_concurrent_downloads = self.max_concurrent_downloads.clamp(1, 32);
        self.max_connections_per_server = self.max_connections_per_server.clamp(1, 16);
        self.speed_limit_kbps = self.speed_limit_kbps.min(1_048_576);
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = "system".into();
        }
        if self.proxy.as_ref().is_some_and(|proxy| proxy.trim().is_empty()) {
            self.proxy = None;
        }
        self.allowed_extension_ids.retain(|id| is_valid_extension_id(id));
        let mut seen = HashSet::new();
        self.allowed_extension_ids.retain(|id| seen.insert(id.clone()));
        if crate::util::is_unsafe_download_root(&crate::util::expand_tilde(
            &self.default_download_path,
        )) {
            self.default_download_path =
                crate::util::default_download_dir().to_string_lossy().into();
        }
        for path in self.category_paths.values_mut() {
            if crate::util::is_unsafe_download_root(&crate::util::expand_tilde(path)) {
                path.clear();
            }
        }
    }

    fn profile_cookies_blank(profile: &DownloadProfile) -> bool {
        profile.cookies.as_deref().map(str::trim).unwrap_or("").is_empty()
    }

    /// UI never sees stored cookies (empty = keep). Name/pattern edits must not drop them.
    pub fn restore_blank_profile_cookies(&mut self, current: &[DownloadProfile]) {
        let mut used = vec![false; current.len()];
        for profile in &mut self.download_profiles {
            if !Self::profile_cookies_blank(profile) {
                continue;
            }
            if let Some((idx, old)) = current.iter().enumerate().find(|(idx, old)| {
                !used[*idx] && old.name == profile.name && old.url_pattern == profile.url_pattern
            }) {
                used[idx] = true;
                profile.cookies = old.cookies.clone();
            }
        }
        for (idx, profile) in self.download_profiles.iter_mut().enumerate() {
            if !Self::profile_cookies_blank(profile) || idx >= current.len() || used[idx] {
                continue;
            }
            used[idx] = true;
            profile.cookies = current[idx].cookies.clone();
        }
    }

    /// Pairing writes the allowlist; Settings / speed-toggle snapshots must not.
    pub fn preserve_command_owned_fields(&mut self, current: &Settings) {
        self.allowed_extension_ids = current.allowed_extension_ids.clone();
    }

    /// Live schedule lives on the queue. Stale Settings mutex must not win.
    pub fn apply_live_schedule(
        &mut self,
        active: bool,
        start: Option<String>,
        stop: Option<String>,
    ) {
        self.schedule_active = active;
        self.schedule_start = start;
        self.schedule_stop = stop;
    }

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
        settings.clamp_runtime_bounds();
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
        assert!(profile.matches("https://example.com:8443/video.mp4"));
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
    fn clamp_runtime_bounds_caps_queue_and_drops_invalid_ids() {
        let mut settings = Settings {
            theme: "neon".into(),
            max_concurrent_downloads: 99,
            max_connections_per_server: 0,
            speed_limit_kbps: 9_999_999,
            proxy: Some("   ".into()),
            allowed_extension_ids: vec![
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                "not-an-id".into(),
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            ],
            default_download_path: "/etc".into(),
            category_paths: HashMap::from([("Video".into(), "/tmp".into())]),
            ..Default::default()
        };
        settings.clamp_runtime_bounds();
        assert_eq!(settings.theme, "system");
        assert_eq!(settings.max_concurrent_downloads, 32);
        assert_eq!(settings.max_connections_per_server, 1);
        assert_eq!(settings.speed_limit_kbps, 1_048_576);
        assert_eq!(settings.proxy, None);
        assert_eq!(
            settings.allowed_extension_ids,
            vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()]
        );
        assert!(!crate::util::is_unsafe_download_root(std::path::Path::new(
            &settings.default_download_path
        )));
        assert_eq!(settings.category_paths.get("Video").map(String::as_str), Some(""));
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

        let default_https =
            DownloadProfile { url_pattern: "https://example.com".into(), ..Default::default() };
        assert!(default_https.matches("https://example.com/video.mp4"));
        assert!(default_https.matches("https://example.com:443/video.mp4"));
        assert!(!default_https.matches("https://example.com:8443/video.mp4"));

        let colon443 =
            DownloadProfile { url_pattern: "https://example.com:443".into(), ..Default::default() };
        assert!(colon443.matches("https://example.com/video.mp4"));
        assert!(colon443.matches("https://example.com:443/video.mp4"));
        assert!(!colon443.matches("https://example.com:8443/video.mp4"));
    }

    #[test]
    fn save_payload_cannot_clobber_scheduler_or_allowlist() {
        let current = Settings {
            schedule_active: true,
            schedule_start: Some("22:00".into()),
            schedule_stop: Some("07:00".into()),
            allowed_extension_ids: vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()],
            ..Default::default()
        };
        let mut incoming = Settings::default();
        incoming.schedule_active = false;
        incoming.allowed_extension_ids = vec!["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into()];
        incoming.preserve_command_owned_fields(&current);
        incoming.apply_live_schedule(
            current.schedule_active,
            current.schedule_start.clone(),
            current.schedule_stop.clone(),
        );
        assert!(incoming.schedule_active);
        assert_eq!(incoming.schedule_start.as_deref(), Some("22:00"));
        assert_eq!(incoming.schedule_stop.as_deref(), Some("07:00"));
        assert_eq!(incoming.allowed_extension_ids, current.allowed_extension_ids);
    }

    #[test]
    fn blank_profile_cookies_survive_rename_not_delete() {
        let current = vec![
            DownloadProfile {
                name: "A".into(),
                url_pattern: "a.com".into(),
                cookies: Some("sid=1".into()),
                ..Default::default()
            },
            DownloadProfile {
                name: "B".into(),
                url_pattern: "b.com".into(),
                cookies: Some("sid=2".into()),
                ..Default::default()
            },
        ];
        let mut renamed = Settings {
            download_profiles: vec![
                DownloadProfile {
                    name: "A-renamed".into(),
                    url_pattern: "a.com".into(),
                    cookies: None,
                    ..Default::default()
                },
                DownloadProfile {
                    name: "B".into(),
                    url_pattern: "b.com".into(),
                    cookies: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        renamed.restore_blank_profile_cookies(&current);
        assert_eq!(renamed.download_profiles[0].cookies.as_deref(), Some("sid=1"));
        assert_eq!(renamed.download_profiles[1].cookies.as_deref(), Some("sid=2"));

        let mut rehost = Settings {
            download_profiles: vec![DownloadProfile {
                name: "A".into(),
                url_pattern: "a2.com".into(),
                cookies: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        rehost.restore_blank_profile_cookies(&current);
        assert_eq!(rehost.download_profiles[0].cookies.as_deref(), Some("sid=1"));

        let mut dropped_first = Settings {
            download_profiles: vec![DownloadProfile {
                name: "B".into(),
                url_pattern: "b.com".into(),
                cookies: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        dropped_first.restore_blank_profile_cookies(&current);
        assert_eq!(dropped_first.download_profiles[0].cookies.as_deref(), Some("sid=2"));

        let mut explicit = Settings {
            download_profiles: vec![DownloadProfile {
                name: "A".into(),
                url_pattern: "a.com".into(),
                cookies: Some("sid=new".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        explicit.restore_blank_profile_cookies(&current);
        assert_eq!(explicit.download_profiles[0].cookies.as_deref(), Some("sid=new"));
    }
}
