use std::sync::Mutex;
use std::sync::OnceLock;

use serde::Serialize;

// ponytail: a small in-memory ring buffer that captures the last N log records
// so the frontend can surface them in a "Logs" panel (debugging + transparency).
// `env_logger` writes to stderr only; this sits alongside it via a custom logger
// that fans events out to BOTH env_logger and the ring buffer.

const CAPACITY: usize = 500;

#[derive(Clone, Serialize)]
pub struct LogEntry {
    pub ts: i64,        // unix millis
    pub level: String,  // "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE"
    pub target: String, // module path
    pub message: String,
}

struct Ring {
    entries: std::collections::VecDeque<LogEntry>,
}

static RING: OnceLock<Mutex<Ring>> = OnceLock::new();

fn ring() -> &'static Mutex<Ring> {
    RING.get_or_init(|| {
        Mutex::new(Ring { entries: std::collections::VecDeque::with_capacity(CAPACITY) })
    })
}

/// Push a record into the ring buffer (drops the oldest when full).
pub fn push(level: &str, target: &str, message: &str) {
    if let Ok(mut guard) = ring().lock() {
        if guard.entries.len() >= CAPACITY {
            guard.entries.pop_front();
        }
        guard.entries.push_back(LogEntry {
            ts: chrono::Utc::now().timestamp_millis(),
            level: level.to_string(),
            target: target.to_string(),
            message: message.to_string(),
        });
    }
}

/// Snapshot the buffered entries (oldest first).
pub fn snapshot() -> Vec<LogEntry> {
    ring().lock().map(|g| g.entries.iter().cloned().collect()).unwrap_or_default()
}

/// Clear the buffer (used by the UI "clear logs" action).
pub fn clear() {
    if let Ok(mut g) = ring().lock() {
        g.entries.clear();
    }
}

// ---- custom logger that fans out to env_logger + the ring buffer ----

struct FanoutLogger {
    /// stderr formatter/filter (env_logger). None if env_logger failed to build.
    stderr: Option<Box<dyn log::Log>>,
}

static ENSURED: OnceLock<()> = OnceLock::new();

impl log::Log for FanoutLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // 1) stderr (env_logger applies RUST_LOG filtering + formatting).
        if let Some(stderr) = &self.stderr {
            stderr.log(record);
        }
        // 2) in-memory ring buffer for the UI "Logs" panel.
        push(record.level().as_str(), record.target(), &record.args().to_string());
    }

    fn flush(&self) {
        if let Some(stderr) = &self.stderr {
            stderr.flush();
        }
    }
}

/// Install the fan-out logger as the global `log` facade.
/// Replaces the bare `env_logger::try_init` call: we still honour RUST_LOG for
/// stderr, but additionally mirror every record into the ring buffer.
pub fn install() {
    ENSURED.get_or_init(|| {
        // Build the env_logger target/filter first (RUST_LOG aware).
        let stderr_logger =
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
                .build();

        let logger = FanoutLogger { stderr: Some(Box::new(stderr_logger)) };
        let _ = log::set_boxed_logger(Box::new(logger));
        log::set_max_level(log::LevelFilter::Info);
    });
}
