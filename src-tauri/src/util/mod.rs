/// Legacy insecure default — never accept as live token after first boot.
pub const LEGACY_DEFAULT_API_TOKEN: &str = "falcon-dm-local-v1";

/// Strip CR/LF and other ASCII controls from header values (header injection).
pub fn sanitize_header_value(s: &str) -> String {
    s.chars().filter(|c| *c >= ' ' && *c != '\u{7f}').take(8192).collect()
}

/// Recover from a poisoned std::sync::Mutex instead of panicking — one panic must not freeze download management.
pub fn lock_or_recover<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

mod http_client;
mod net;
mod path;

pub use http_client::*;
pub use net::*;
pub use path::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_header() {
        assert_eq!(sanitize_header_value("a\r\nX-Inject: 1"), "aX-Inject: 1");
        assert_eq!(sanitize_header_value("a\0b\u{7f}c"), "abc");
        assert_eq!(sanitize_header_value(&"x".repeat(9000)).len(), 8192);
    }
}
