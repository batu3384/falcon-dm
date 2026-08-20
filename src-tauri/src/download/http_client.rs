use crate::util::{sanitize_header_value, validate_fetch_url_async, with_pinned_http_clients};
use reqwest::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, LOCATION, RANGE};
use reqwest::{Client, StatusCode};
use std::time::Duration;
use url::Url;

#[derive(Clone, Default)]
pub struct HttpOptions {
    pub proxy: Option<String>,
    pub speed_limit_kbps: u32,
}

#[derive(Clone, Default)]
pub struct HttpHeaders {
    pub cookies: Option<String>,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
    pub options: HttpOptions,
    pub max_connections: usize,
}

pub(crate) const MAX_REDIRECTS: usize = 5;
pub(crate) const MAX_HTTP_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub(crate) const MIN_PARALLEL_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedResource {
    pub initial: Url,
    pub final_url: Url,
    pub total_bytes: u64,
    pub accepts_ranges: bool,
}

pub(crate) async fn with_pinned_clients<T, F, Fut>(
    url: &Url,
    proxy: Option<&str>,
    timeout: Duration,
    op: F,
) -> Result<T, String>
where
    F: FnMut(Client) -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    with_pinned_http_clients(url, proxy, timeout, None, op).await
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str().map(str::to_ascii_lowercase)
            == right.host_str().map(str::to_ascii_lowercase)
        && left.port_or_known_default() == right.port_or_known_default()
}

pub(crate) fn add_request_headers(
    request: reqwest::RequestBuilder,
    initial: &Url,
    current: &Url,
    headers: &HttpHeaders,
) -> reqwest::RequestBuilder {
    let same_origin = same_origin(initial, current);
    let mut request = request;
    if same_origin && initial.scheme() == "https" {
        if let Some(value) = headers.cookies.as_deref().map(sanitize_header_value) {
            if !value.is_empty() {
                request = request.header(reqwest::header::COOKIE, value);
            }
        }
        if let Some(value) = headers.referrer.as_deref().map(sanitize_header_value) {
            if !value.is_empty() {
                request = request.header(reqwest::header::REFERER, value);
            }
        }
    }
    if let Some(value) = headers.user_agent.as_deref().map(sanitize_header_value) {
        if !value.is_empty() {
            request = request.header(reqwest::header::USER_AGENT, value);
        }
    }
    request
}

async fn redirect_target(base: &Url, location: &str) -> Result<Url, String> {
    let target = base.join(location).map_err(|e| format!("Invalid redirect: {e}"))?;
    validate_fetch_url_async(target.as_str()).await
}

fn total_from_content_range(
    header: Option<&reqwest::header::HeaderValue>,
    resume_from: u64,
    content_length: Option<u64>,
) -> u64 {
    if let Some(total) = header
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit('/').next())
        .and_then(|value| value.parse::<u64>().ok())
    {
        return total;
    }
    resume_from.saturating_add(content_length.unwrap_or(0))
}

pub(crate) fn split_byte_ranges(total: u64, connections: usize) -> Vec<(u64, u64)> {
    let connections = connections.clamp(1, 16) as u64;
    if total == 0 || connections == 1 {
        return vec![(0, total.saturating_sub(1))];
    }
    let base = total / connections;
    let remainder = total % connections;
    let mut ranges = Vec::with_capacity(connections as usize);
    let mut start = 0u64;
    for index in 0..connections {
        let extra = u64::from(index < remainder);
        let len = base + extra;
        if len == 0 {
            break;
        }
        let end = start.saturating_add(len.saturating_sub(1));
        ranges.push((start, end));
        start = end.saturating_add(1);
    }
    ranges
}

pub(crate) async fn resolve_resource(
    initial: &Url,
    headers: &HttpHeaders,
) -> Result<ResolvedResource, String> {
    let proxy = headers.options.proxy.as_deref();
    let mut current = initial.clone();
    for redirect_count in 0..=MAX_REDIRECTS {
        let response = with_pinned_clients(&current, proxy, Duration::from_secs(60), |client| {
            let current = current.clone();
            let initial = initial.clone();
            let headers = headers.clone();
            async move {
                let request =
                    add_request_headers(client.head(current.clone()), &initial, &current, &headers);
                request.send().await.map_err(|e| format!("HTTP probe failed: {e}"))
            }
        })
        .await?;

        if response.status().is_redirection() {
            if redirect_count == MAX_REDIRECTS {
                return Err("Too many HTTP redirects".into());
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "HTTP redirect has no valid location".to_string())?;
            current = redirect_target(&current, location).await?;
            continue;
        }

        if response.status() == StatusCode::METHOD_NOT_ALLOWED
            || response.status() == StatusCode::NOT_IMPLEMENTED
        {
            break;
        }

        if !response.status().is_success() {
            return Err(format!("HTTP probe {}", response.status().as_u16()));
        }

        let accepts_ranges = response
            .headers()
            .get(ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("bytes"));
        let total_bytes = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        if total_bytes > MAX_HTTP_BYTES {
            return Err("HTTP response exceeds maximum download size".into());
        }
        return Ok(ResolvedResource {
            initial: initial.clone(),
            final_url: current,
            total_bytes,
            accepts_ranges,
        });
    }

    // Some CDNs reject HEAD — probe with a zero-byte range GET.
    let current = initial.clone();
    let response = with_pinned_clients(&current, proxy, Duration::from_secs(60), |client| {
        let current = current.clone();
        let initial = initial.clone();
        let headers = headers.clone();
        async move {
            let request =
                add_request_headers(client.get(current.clone()), &initial, &current, &headers)
                    .header(RANGE, "bytes=0-0");
            request.send().await.map_err(|e| format!("HTTP probe failed: {e}"))
        }
    })
    .await?;

    let status = response.status();
    if status != StatusCode::PARTIAL_CONTENT && !status.is_success() {
        return Err(format!("HTTP probe {}", status.as_u16()));
    }
    let accepts_ranges = status == StatusCode::PARTIAL_CONTENT
        || response
            .headers()
            .get(ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("bytes"));
    let total_bytes = if status == StatusCode::PARTIAL_CONTENT {
        total_from_content_range(
            response.headers().get(CONTENT_RANGE),
            0,
            response.content_length(),
        )
    } else {
        response.content_length().unwrap_or(0)
    };
    if total_bytes > MAX_HTTP_BYTES {
        return Err("HTTP response exceeds maximum download size".into());
    }
    Ok(ResolvedResource {
        initial: initial.clone(),
        final_url: current,
        total_bytes,
        accepts_ranges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_byte_ranges_covers_entire_file() {
        let ranges = split_byte_ranges(10, 3);
        assert_eq!(ranges, vec![(0, 3), (4, 6), (7, 9)]);
    }

    #[test]
    fn split_byte_ranges_single_connection() {
        assert_eq!(split_byte_ranges(100, 1), vec![(0, 99)]);
    }
}
