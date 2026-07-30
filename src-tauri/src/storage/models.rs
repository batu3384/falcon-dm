use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed,
    Merging,
}

impl DownloadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadStatus::Queued => "Queued",
            DownloadStatus::Downloading => "Downloading",
            DownloadStatus::Paused => "Paused",
            DownloadStatus::Completed => "Completed",
            DownloadStatus::Failed => "Failed",
            DownloadStatus::Merging => "Merging",
        }
    }
}

impl fmt::Display for DownloadStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for DownloadStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Queued" => Ok(DownloadStatus::Queued),
            "Downloading" => Ok(DownloadStatus::Downloading),
            "Paused" => Ok(DownloadStatus::Paused),
            "Completed" => Ok(DownloadStatus::Completed),
            "Failed" => Ok(DownloadStatus::Failed),
            "Merging" => Ok(DownloadStatus::Merging),
            _ => Err(format!("Unknown DownloadStatus: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DownloadCategory {
    Video,
    Music,
    Document,
    Archive,
    Program,
    Other,
}

impl DownloadCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadCategory::Video => "Video",
            DownloadCategory::Music => "Music",
            DownloadCategory::Document => "Document",
            DownloadCategory::Archive => "Archive",
            DownloadCategory::Program => "Program",
            DownloadCategory::Other => "Other",
        }
    }

    pub fn from_filename(filename: &str) -> Self {
        let ext = filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" => {
                DownloadCategory::Video
            }
            "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" => {
                DownloadCategory::Music
            }
            "pdf" | "doc" | "docx" | "txt" | "epub" | "xls" | "xlsx" | "ppt" | "pptx" => {
                DownloadCategory::Document
            }
            "zip" | "tar" | "gz" | "7z" | "rar" | "bz2" | "xz" | "tgz" => {
                DownloadCategory::Archive
            }
            "dmg" | "pkg" | "exe" | "msi" | "app" | "deb" | "rpm" => {
                DownloadCategory::Program
            }
            _ => DownloadCategory::Other,
        }
    }
}

impl fmt::Display for DownloadCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for DownloadCategory {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Video" => Ok(DownloadCategory::Video),
            "Music" => Ok(DownloadCategory::Music),
            "Document" => Ok(DownloadCategory::Document),
            "Archive" => Ok(DownloadCategory::Archive),
            "Program" => Ok(DownloadCategory::Program),
            "Other" => Ok(DownloadCategory::Other),
            _ => Err(format!("Unknown DownloadCategory: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: Option<i64>,
    pub url: String,
    pub filename: String,
    pub save_path: String,
    pub total_size: u64,
    pub downloaded_size: u64,
    pub status: DownloadStatus,
    pub category: DownloadCategory,
    pub speed: f64,
    pub segments: u32,
    pub priority: u32,
    pub created_at: String, // ISO 8601
    pub completed_at: Option<String>,
    pub error_message: Option<String>,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    pub aria2_gid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DownloadFilter {
    pub status: Option<DownloadStatus>,
    pub category: Option<DownloadCategory>,
    pub search: Option<String>,
}
