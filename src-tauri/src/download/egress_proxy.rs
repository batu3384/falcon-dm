//! Validating loopback egress proxy for yt-dlp's otherwise unvalidated traffic.
//!
//! CONNECT targets and absolute-form HTTP requests are resolved here, every
//! resolved address is checked, and the outbound TCP connection goes to one of
//! those exact addresses. DNS is never consulted again at connect time.

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::Response,
};
use hyper_util::rt::TokioIo;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, watch, OnceCell, OwnedSemaphorePermit, Semaphore};

const DNS_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TUNNEL_MAX_LIFETIME: Duration = Duration::from_secs(60 * 60);
const MAX_CONCURRENT_EGRESS: usize = 256;

static SHARED_PROXY: OnceCell<EgressProxy> = OnceCell::const_new();

#[derive(Clone, Copy, Default)]
struct ValidationPolicy {
    #[cfg(test)]
    allow_loopback: bool,
}

impl ValidationPolicy {
    #[cfg(test)]
    fn capture() -> Self {
        Self { allow_loopback: crate::util::loopback_exception_active_for_tests() }
    }

    #[cfg(not(test))]
    fn capture() -> Self {
        Self::default()
    }

    fn blocked_host(&self, host: &str) -> bool {
        #[cfg(test)]
        {
            crate::util::is_blocked_download_host_with_loopback(host, self.allow_loopback)
        }
        #[cfg(not(test))]
        {
            crate::util::is_blocked_download_host(host)
        }
    }

    fn blocked_ip(&self, ip: IpAddr) -> bool {
        #[cfg(test)]
        {
            crate::util::is_blocked_ip_with_loopback(ip, self.allow_loopback)
        }
        #[cfg(not(test))]
        {
            crate::util::is_blocked_ip(ip)
        }
    }
}

#[derive(Debug)]
struct ProxyError {
    status: StatusCode,
    message: String,
}

impl ProxyError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: message.into() }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self { status: StatusCode::FORBIDDEN, message: message.into() }
    }

    fn bad_gateway(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_GATEWAY, message: message.into() }
    }

    fn busy() -> Self {
        Self { status: StatusCode::SERVICE_UNAVAILABLE, message: "egress proxy is busy".into() }
    }

    fn into_response(self) -> Response {
        let mut response = Response::new(Body::from(self.message));
        *response.status_mut() = self.status;
        response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        response
    }
}

fn resolution_host(host: &str) -> &str {
    host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host)
}

async fn validated_addresses(
    host: &str,
    port: u16,
    policy: ValidationPolicy,
) -> Result<Vec<SocketAddr>, ProxyError> {
    if port == 0 {
        return Err(ProxyError::bad_request("CONNECT port must be in 1..=65535"));
    }
    let host = resolution_host(host.trim());
    if host.is_empty() {
        return Err(ProxyError::bad_request("target host is empty"));
    }
    if policy.blocked_host(host) {
        return Err(ProxyError::forbidden("Private/loopback hosts are not allowed"));
    }
    let addrs: Vec<SocketAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| ProxyError::bad_gateway("Host resolution timed out"))?
            .map_err(|e| ProxyError::bad_gateway(format!("Host resolution failed: {e}")))?
            .collect()
    };
    if addrs.is_empty() {
        return Err(ProxyError::bad_gateway("Host resolution returned no addresses"));
    }
    // Unlike enqueue-time validation, one public sibling must not rescue a
    // hostile DNS answer: any blocked address fails the whole target.
    if addrs.iter().any(|addr| policy.blocked_ip(addr.ip())) {
        return Err(ProxyError::forbidden("Host resolves to a blocked address"));
    }
    Ok(addrs)
}

async fn connect_validated(addrs: &[SocketAddr]) -> Result<tokio::net::TcpStream, ProxyError> {
    let connect = async {
        let mut last_error = None;
        for addr in addrs {
            match tokio::net::TcpStream::connect(*addr).await {
                Ok(stream) => return Ok(stream),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no validated address")
        }))
    };
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| ProxyError::bad_gateway("connect timed out"))?
        .map_err(|e: std::io::Error| ProxyError::bad_gateway(format!("connect failed: {e}")))?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

fn connect_target(uri: &Uri) -> Result<(String, u16), ProxyError> {
    let authority =
        uri.authority().ok_or_else(|| ProxyError::bad_request("CONNECT needs host:port"))?;
    if authority.as_str().contains('@') {
        return Err(ProxyError::bad_request("CONNECT userinfo is not supported"));
    }
    let port = authority
        .port_u16()
        .filter(|port| *port > 0)
        .ok_or_else(|| ProxyError::bad_request("CONNECT needs an explicit port"))?;
    Ok((authority.host().to_string(), port))
}

fn host_header_value(authority: &axum::http::uri::Authority) -> Result<HeaderValue, ProxyError> {
    let host = authority.host();
    let value = match authority.port_u16() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    HeaderValue::from_str(&value)
        .map_err(|_| ProxyError::bad_request("target host is not a valid Host header"))
}

fn remove_all(headers: &mut HeaderMap, name: &str) {
    while headers.remove(name).is_some() {}
}

fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let mut nominated = Vec::new();
    for value in headers.get_all(header::CONNECTION) {
        if let Ok(value) = value.to_str() {
            nominated.extend(
                value.split(',').map(str::trim).filter(|name| !name.is_empty()).map(str::to_string),
            );
        }
    }
    for name in [
        "connection",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "keep-alive",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        remove_all(headers, name);
    }
    for name in nominated {
        if name.eq_ignore_ascii_case("host") {
            continue;
        }
        if let Ok(name) = HeaderName::from_bytes(name.as_bytes()) {
            while headers.remove(&name).is_some() {}
        }
    }
}

fn prepare_forward_request(mut req: Request) -> Result<(Request, String, u16), ProxyError> {
    let uri = req.uri().clone();
    if uri.scheme_str() != Some("http") || uri.authority().is_none() {
        return Err(ProxyError::bad_request("proxy requests must use absolute http URIs"));
    }
    let authority = uri.authority().cloned().expect("authority checked above");
    let host = authority.host().to_string();
    let port = authority.port_u16().unwrap_or(80);
    let origin_form = uri.path_and_query().map(|path| path.as_str()).unwrap_or("/");
    *req.uri_mut() = origin_form
        .parse::<Uri>()
        .map_err(|_| ProxyError::bad_request("target URI path is invalid"))?;
    if !req.headers().contains_key(header::HOST) {
        req.headers_mut().insert(header::HOST, host_header_value(&authority)?);
    }
    strip_hop_by_hop(req.headers_mut());
    Ok((req, host, port))
}

fn acquire_permit(state: &ProxyState) -> Result<OwnedSemaphorePermit, ProxyError> {
    state.permits.clone().try_acquire_owned().map_err(|_| ProxyError::busy())
}

async fn forward_http(state: ProxyState, req: Request) -> Result<Response, ProxyError> {
    let permit = acquire_permit(&state)?;
    let (req, host, port) = prepare_forward_request(req)?;
    let addrs = validated_addresses(&host, port, state.policy).await?;
    let stream = connect_validated(&addrs).await?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(stream))
        .await
        .map_err(|e| ProxyError::bad_gateway(format!("HTTP forward setup failed: {e}")))?;
    tokio::spawn(async move {
        let _permit = permit;
        if let Err(error) = conn.await {
            log::debug!("egress forward connection ended with an error: {error}");
        }
    });
    let response = sender
        .send_request(req)
        .await
        .map_err(|e| ProxyError::bad_gateway(format!("HTTP forward failed: {e}")))?;
    let (parts, body) = response.into_parts();
    let mut response = Response::from_parts(parts, Body::new(body));
    strip_hop_by_hop(response.headers_mut());
    Ok(response)
}

async fn connect_tunnel(state: ProxyState, mut req: Request) -> Result<Response, ProxyError> {
    let permit = acquire_permit(&state)?;
    let (host, port) = connect_target(req.uri())?;
    let addrs = validated_addresses(&host, port, state.policy).await?;
    let remote = connect_validated(&addrs).await?;
    let upgrade = hyper::upgrade::on(&mut req);
    tokio::spawn(async move {
        let _permit = permit;
        let upgraded = match upgrade.await {
            Ok(upgraded) => upgraded,
            Err(error) => {
                log::debug!("egress CONNECT upgrade failed: {error}");
                return;
            }
        };
        let mut client = TokioIo::new(upgraded);
        let mut remote = remote;
        match tokio::time::timeout(
            TUNNEL_MAX_LIFETIME,
            tokio::io::copy_bidirectional(&mut client, &mut remote),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => log::debug!("egress tunnel ended with an error: {error}"),
            Err(_) => log::debug!("egress tunnel reached its one-hour lifetime cap"),
        }
    });
    Ok(Response::new(Body::empty()))
}

#[derive(Clone)]
struct ProxyState {
    permits: Arc<Semaphore>,
    policy: ValidationPolicy,
}

async fn handle_request(state: ProxyState, req: Request) -> Response {
    let result = if req.method() == Method::CONNECT {
        connect_tunnel(state, req).await
    } else {
        forward_http(state, req).await
    };
    result.unwrap_or_else(ProxyError::into_response)
}

async fn serve_connections(
    listener: tokio::net::TcpListener,
    state: ProxyState,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    loop {
        let (stream, _) = tokio::select! {
            accepted = listener.accept() => accepted?,
            _ = shutdown.changed() => break,
        };
        let _ = stream.set_nodelay(true);
        let state = state.clone();
        tokio::spawn(async move {
            let service =
                hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let state = state.clone();
                    async move {
                        let req = req.map(Body::new);
                        Ok::<_, std::convert::Infallible>(handle_request(state, req).await)
                    }
                });
            let conn = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades();
            if let Err(error) = conn.await {
                log::debug!("egress proxy connection ended with an error: {error}");
            }
        });
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) struct EgressProxy {
    port: u16,
    _shutdown: watch::Sender<bool>,
}

impl EgressProxy {
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }
}

/// Bind a fresh loopback port and serve it from a small dedicated runtime.
/// The runtime outlives individual job/test runtimes; two workers keep parallel
/// media tunnels from serializing their byte copies on one core.
pub(crate) async fn start() -> Result<EgressProxy, String> {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("egress proxy bind failed: {e}"))?;
    std_listener.set_nonblocking(true).map_err(|e| format!("egress proxy setup failed: {e}"))?;
    let port = std_listener.local_addr().map_err(|e| e.to_string())?.port();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let proxy = EgressProxy { port, _shutdown: shutdown_tx };
    // No proxy auth: any local process can already dial out directly, and this
    // proxy only ever connects to freshly validated public IP addresses.
    let state = ProxyState {
        permits: Arc::new(Semaphore::new(MAX_CONCURRENT_EGRESS)),
        policy: ValidationPolicy::capture(),
    };
    let (ready_tx, ready_rx) = oneshot::channel();
    let spawn = std::thread::Builder::new().name("falcon-egress-proxy".into()).spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("falcon-egress-io")
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = ready_tx.send(Err(format!("egress proxy runtime failed: {error}")));
                return;
            }
        };
        runtime.block_on(async move {
            let listener = match tokio::net::TcpListener::from_std(std_listener) {
                Ok(listener) => listener,
                Err(error) => {
                    let _ = ready_tx.send(Err(format!("egress proxy listener failed: {error}")));
                    return;
                }
            };
            if ready_tx.send(Ok(())).is_err() {
                return;
            }
            if let Err(error) = serve_connections(listener, state, shutdown_rx).await {
                log::error!("egress proxy server stopped: {error}");
            }
        });
    });
    if let Err(error) = spawn {
        return Err(format!("egress proxy thread failed: {error}"));
    }
    match ready_rx.await {
        Ok(Ok(())) => Ok(proxy),
        Ok(Err(error)) => Err(error),
        Err(_) => Err("egress proxy stopped during startup".into()),
    }
}

/// One process-wide proxy for every yt-dlp job. Startup is lazy so HTTP/HLS-only
/// sessions never pay for it; initialization failure is propagated to the job.
pub(crate) async fn shared() -> Result<EgressProxy, String> {
    SHARED_PROXY.get_or_try_init(start).await.cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn read_head(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 1024];
        while !out.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buf).await.unwrap();
            assert!(
                n > 0,
                "connection closed before response headers: {:?}",
                String::from_utf8_lossy(&out)
            );
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    async fn http_fixture(body: &'static str) -> (u16, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0, "fixture connection closed before request headers");
                request.extend_from_slice(&buf[..n]);
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            let _ = socket.shutdown().await;
            request
        });
        (port, task)
    }

    #[tokio::test]
    async fn validation_rejects_private_literals_and_accepts_public_literals() {
        let strict = ValidationPolicy::default();
        for host in ["127.0.0.1", "192.168.1.1", "169.254.169.254", "[::1]", "localhost"] {
            let err = validated_addresses(host, 443, strict).await.unwrap_err();
            assert_eq!(err.status, StatusCode::FORBIDDEN, "{host} must be forbidden");
        }
        let public = validated_addresses("8.8.8.8", 443, strict).await.unwrap();
        assert_eq!(public, vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 443)]);
    }

    #[tokio::test]
    async fn connect_tunnel_round_trips_http_bytes() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let proxy = start().await.unwrap();
        let (origin_port, origin) = http_fixture("tunnel-ok").await;
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", proxy.port())).await.unwrap();
        client
            .write_all(
                format!(
                    "CONNECT 127.0.0.1:{origin_port} HTTP/1.1\r\nHost: 127.0.0.1:{origin_port}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let head = read_head(&mut client).await;
        assert!(
            String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"),
            "CONNECT was rejected: {}",
            String::from_utf8_lossy(&head)
        );
        client
            .write_all(
                format!(
                    "GET /tunnel HTTP/1.1\r\nHost: 127.0.0.1:{origin_port}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).ends_with("tunnel-ok"));
        let request = String::from_utf8(origin.await.unwrap()).unwrap();
        assert!(request.starts_with("GET /tunnel HTTP/1.1"), "{request}");
    }

    #[tokio::test]
    async fn absolute_form_http_forward_streams_response_and_preserves_host() {
        let _loopback = crate::util::allow_loopback_for_tests();
        let proxy = start().await.unwrap();
        let (origin_port, origin) = http_fixture("forward-ok").await;
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", proxy.port())).await.unwrap();
        client
            .write_all(
                format!(
                    "GET http://127.0.0.1:{origin_port}/forward?x=1 HTTP/1.1\r\nHost: 127.0.0.1:{origin_port}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with("forward-ok"), "{response}");
        let request = String::from_utf8(origin.await.unwrap()).unwrap();
        assert!(request.starts_with("GET /forward?x=1 HTTP/1.1"), "{request}");
        assert!(
            request.to_ascii_lowercase().contains(&format!("host: 127.0.0.1:{origin_port}")),
            "{request}"
        );
    }

    #[tokio::test]
    async fn connect_to_loopback_is_rejected_without_the_test_exception() {
        let proxy = start().await.unwrap();
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", proxy.port())).await.unwrap();
        client
            .write_all(b"CONNECT 127.0.0.1:443 HTTP/1.1\r\nHost: 127.0.0.1:443\r\n\r\n")
            .await
            .unwrap();
        let head = read_head(&mut client).await;
        assert!(
            String::from_utf8_lossy(&head).starts_with("HTTP/1.1 403"),
            "loopback CONNECT was not rejected: {}",
            String::from_utf8_lossy(&head)
        );
    }
}
