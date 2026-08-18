use std::future::Future;
use std::net::SocketAddr;

pub fn retryable_network_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("connection")
        || lower.contains("timeout")
        || lower.contains("dns")
        || lower.contains("network")
        || lower.contains("broken pipe")
}

pub fn build_pinned_http_client(
    url: &url::Url,
    pin: Option<SocketAddr>,
    proxy: Option<&str>,
    timeout: std::time::Duration,
    default_headers: Option<reqwest::header::HeaderMap>,
) -> Result<reqwest::Client, String> {
    let mut builder =
        reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(timeout);
    if let Some(headers) = default_headers {
        builder = builder.default_headers(headers);
    }
    if let Some(addr) = pin {
        if let Some(host) = url.host_str() {
            if host.parse::<std::net::IpAddr>().is_err() {
                builder = builder.resolve(host, addr);
            }
        }
    }
    if let Some(proxy) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|e| e.to_string())?);
    }
    builder.build().map_err(|e| format!("HTTP client build failed: {e}"))
}

pub async fn with_pinned_http_clients<T, F, Fut>(
    url: &url::Url,
    proxy: Option<&str>,
    timeout: std::time::Duration,
    default_headers: Option<reqwest::header::HeaderMap>,
    mut op: F,
) -> Result<T, String>
where
    F: FnMut(reqwest::Client) -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    let addresses = super::resolve_public_addresses_async(url).await?;
    let mut last_err = "HTTP request failed".to_string();
    for addr in addresses {
        let client = build_pinned_http_client(url, Some(addr), proxy, timeout, default_headers.clone())?;
        match op(client).await {
            Ok(value) => return Ok(value),
            Err(err) if retryable_network_error(&err) => last_err = err,
            Err(err) => return Err(err),
        }
    }
    Err(last_err)
}
