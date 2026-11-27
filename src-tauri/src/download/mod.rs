pub mod checksum;
pub mod hls;
pub mod http;
pub mod http_client;
pub mod job;
pub mod limiter;
pub mod progress;
pub mod queue;
pub mod tools;
pub mod ytdlp;

/// Why a worker stopped. The queue decides between pause, retry and failure from this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// Pause, remove, schedule or shutdown asked the worker to stop.
    Cancelled,
    /// Network blip, 5xx, 429. The queue retries with backoff.
    Transient(String),
    /// Retrying will not help (404, invalid URL, disk full).
    Fatal(String),
    /// HLS playlist uses features the native path does not implement.
    NeedsYtDlp,
}

impl DownloadError {
    pub fn message(&self) -> String {
        match self {
            Self::Cancelled => "Cancelled".into(),
            Self::Transient(msg) | Self::Fatal(msg) => msg.clone(),
            Self::NeedsYtDlp => "ERR_NEEDS_YTDLP".into(),
        }
    }

    pub fn transient(msg: impl Into<String>) -> Self {
        Self::Transient(msg.into())
    }

    pub fn fatal(msg: impl Into<String>) -> Self {
        Self::Fatal(msg.into())
    }

    /// HTTP status → retry class. 408/425/429 and 5xx are worth another attempt.
    pub fn from_status(status: u16) -> Self {
        let msg = format!("HTTP {status}");
        if matches!(status, 408 | 425 | 429) || (500..600).contains(&status) {
            Self::Transient(msg)
        } else {
            Self::Fatal(msg)
        }
    }

    /// Transport errors from reqwest: everything except a malformed request is transient.
    pub fn from_reqwest(context: &str, err: &reqwest::Error) -> Self {
        // reqwest's Display is "error sending request for url (…)". The useful
        // part ("connection refused", "certificate has expired") is the root cause.
        let mut root: &dyn std::error::Error = err;
        while let Some(source) = root.source() {
            root = source;
        }
        let msg = format!("{context}: {root}");
        if err.is_builder() {
            Self::Fatal(msg)
        } else {
            Self::Transient(msg)
        }
    }
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// Legacy helpers return `String`. Classify by the wording they already use.
impl From<String> for DownloadError {
    fn from(msg: String) -> Self {
        if msg == "Cancelled" {
            return Self::Cancelled;
        }
        let lower = msg.to_lowercase();
        let transient = [
            "timed out",
            "timeout",
            "temporarily",
            "try again",
            "rate limit",
            "too many requests",
            "connection",
            "network",
            "dns",
            "resolution failed",
            "broken pipe",
            "unexpected eof",
            "incomplete",
        ];
        if transient.iter().any(|needle| lower.contains(needle)) {
            Self::Transient(msg)
        } else {
            Self::Fatal(msg)
        }
    }
}

impl From<&str> for DownloadError {
    fn from(msg: &str) -> Self {
        Self::from(msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::DownloadError;

    #[test]
    fn status_codes_split_into_retry_classes() {
        assert!(matches!(DownloadError::from_status(503), DownloadError::Transient(_)));
        assert!(matches!(DownloadError::from_status(429), DownloadError::Transient(_)));
        assert!(matches!(DownloadError::from_status(404), DownloadError::Fatal(_)));
        assert!(matches!(DownloadError::from_status(403), DownloadError::Fatal(_)));
    }

    #[test]
    fn legacy_strings_keep_cancel_and_transient_meaning() {
        assert_eq!(DownloadError::from("Cancelled"), DownloadError::Cancelled);
        assert!(matches!(
            DownloadError::from("Host resolution failed: nodename"),
            DownloadError::Transient(_)
        ));
        assert!(matches!(
            DownloadError::from("Destination file already exists"),
            DownloadError::Fatal(_)
        ));
    }
}
