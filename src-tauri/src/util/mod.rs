/// Legacy insecure default — never accept as live token after first boot.
pub const LEGACY_DEFAULT_API_TOKEN: &str = "falcon-dm-local-v1";

/// Strip CR/LF from header values to prevent header injection.
pub fn sanitize_header_value(s: &str) -> String {
    s.chars().filter(|c| *c != '\r' && *c != '\n').collect()
}

/// Recover from a poisoned std::sync::Mutex instead of panicking — one panic must not freeze download management.
pub fn lock_or_recover<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

mod net;
mod path;

pub use net::*;
pub use path::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_header() {
        assert_eq!(sanitize_header_value("a\r\nX-Inject: 1"), "aX-Inject: 1");
    }
}
