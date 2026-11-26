use crate::pairing::{is_valid_pair_code, PairRequestOutcome};
use crate::storage::models::{Download, DownloadStatus};
use crate::util::{lock_or_recover, LEGACY_DEFAULT_API_TOKEN};
use crate::{
    check_api_token, enqueue_download, extension_id_from_origin, ingest_uploaded_file,
    is_valid_extension_id, validate_upload_b64_len, AppState, ExternalDownloadPayload,
};
use axum::{
    body::Body,
    extract::State as AxumState,
    http::{HeaderMap, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Requests per minute. A 100-link grab plus popup polling stays well below.
const RATE_LIMIT_PER_MINUTE: usize = 600;

fn check_rate_limit(state: &AppState) -> Result<(), StatusCode> {
    let mut bucket = lock_or_recover(&state.rate_bucket);
    let now = Instant::now();
    let window = Duration::from_secs(60);
    while bucket.front().is_some_and(|t| now.duration_since(*t) > window) {
        bucket.pop_front();
    }
    if bucket.len() >= RATE_LIMIT_PER_MINUTE {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    bucket.push_back(now);
    Ok(())
}

pub(crate) async fn rate_limit_middleware(
    AxumState(app): AxumState<AppHandle>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if req.uri().path() == "/api/health" {
        return Ok(next.run(req).await);
    }
    let state = app.state::<AppState>();
    check_rate_limit(&state)?;
    Ok(next.run(req).await)
}

fn origin_of(headers: &HeaderMap) -> Option<&str> {
    headers.get("origin").and_then(|v| v.to_str().ok())
}

fn enqueue_response(result: Result<i64, String>) -> Response {
    match result {
        Ok(id) => {
            (StatusCode::OK, Json(serde_json::json!({ "success": true, "id": id }))).into_response()
        }
        Err(e) => {
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "success": false, "error": e })))
                .into_response()
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct AddDownloadRequest {
    url: String,
    filename: String,
    referrer: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
    cookie_url: Option<String>,
    format: Option<String>,
    title: Option<String>,
}

pub(crate) async fn handle_api_add(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<AddDownloadRequest>,
) -> Response {
    let state = app.state::<AppState>();
    if let Err(status) = check_api_token(&headers, &state, origin_of(&headers)) {
        return status.into_response();
    }
    enqueue_response(
        enqueue_download(
            &app,
            ExternalDownloadPayload {
                url: payload.url,
                filename: Some(payload.filename),
                referrer: payload.referrer,
                user_agent: payload.user_agent,
                cookies: payload.cookies,
                cookie_url: payload.cookie_url,
                title: payload.title,
                format: payload.format,
                save_path: None,
            },
        )
        .await,
    )
}

#[derive(Deserialize)]
pub(crate) struct InterceptRequest {
    pub url: String,
    pub page_url: Option<String>,
    pub title: Option<String>,
    pub cookies: Option<String>,
    pub cookie_url: Option<String>,
    pub user_agent: Option<String>,
    pub referer: Option<String>,
    pub filename: Option<String>,
    pub format: Option<String>,
}

pub(crate) async fn handle_intercept(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<InterceptRequest>,
) -> Response {
    let state = app.state::<AppState>();
    if let Err(status) = check_api_token(&headers, &state, origin_of(&headers)) {
        return status.into_response();
    }
    enqueue_response(
        enqueue_download(
            &app,
            ExternalDownloadPayload {
                url: payload.url,
                filename: payload.filename,
                referrer: payload.referer.or(payload.page_url),
                user_agent: payload.user_agent,
                cookies: payload.cookies,
                cookie_url: payload.cookie_url,
                title: payload.title,
                format: payload.format,
                save_path: None,
            },
        )
        .await,
    )
}

#[derive(Deserialize)]
pub(crate) struct UploadRequest {
    filename: String,
    data_base64: String,
    page_url: Option<String>,
}

pub(crate) async fn handle_api_upload(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<UploadRequest>,
) -> Response {
    let state = app.state::<AppState>();
    if let Err(status) = check_api_token(&headers, &state, origin_of(&headers)) {
        return status.into_response();
    }
    if let Err(e) = validate_upload_b64_len(payload.data_base64.len()) {
        return enqueue_response(Err(e));
    }
    let bytes = match base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        payload.data_base64.trim(),
    ) {
        Ok(bytes) => bytes,
        Err(e) => return enqueue_response(Err(format!("invalid upload data: {e}"))),
    };
    enqueue_response(ingest_uploaded_file(&app, &payload.filename, bytes, payload.page_url).await)
}

/// 2 = code pairing + `/api/downloads`.
pub(crate) const API_PROTOCOL: u32 = 2;

pub(crate) async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "service": "falcon-dm",
        "protocol": API_PROTOCOL,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Deserialize)]
pub(crate) struct PairRequest {
    pub extension_id: String,
    /// Six digits the extension shows to the user.
    pub code: String,
}

/// Pair browser extension ↔ desktop.
///
/// The browser sets `Origin` for extension requests and web pages cannot forge
/// a `chrome-extension://` origin. An unknown extension gets nothing until the
/// user compares the code in both windows and approves.
pub(crate) async fn handle_pair(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<PairRequest>,
) -> Response {
    let Some(id) = origin_of(&headers).and_then(extension_id_from_origin) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if id != payload.extension_id
        || !is_valid_extension_id(id)
        || !is_valid_pair_code(&payload.code)
    {
        return StatusCode::FORBIDDEN.into_response();
    }

    let state = app.state::<AppState>();
    let token = lock_or_recover(&state.api_token).clone();
    if token.trim().is_empty() || token == LEGACY_DEFAULT_API_TOKEN {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if lock_or_recover(&state.settings).allowed_extension_ids.iter().any(|x| x == id) {
        return Json(serde_json::json!({ "ok": true, "token": token, "extension_id": id }))
            .into_response();
    }

    let outcome = lock_or_recover(&state.pairing).request(id, &payload.code);
    match outcome {
        PairRequestOutcome::Rejected => StatusCode::FORBIDDEN.into_response(),
        PairRequestOutcome::TooMany => StatusCode::TOO_MANY_REQUESTS.into_response(),
        PairRequestOutcome::Pending { is_new } => {
            if is_new {
                let _ = app.emit(
                    "pair-request",
                    serde_json::json!({ "extension_id": id, "code": payload.code }),
                );
                // The prompt is useless behind other windows.
                crate::show_main_window(&app);
            }
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "ok": false, "pending": true, "extension_id": id })),
            )
                .into_response()
        }
    }
}

pub(crate) async fn handle_ping(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let state = app.state::<AppState>();
    check_api_token(&headers, &state, origin_of(&headers))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// What the popup shows per row. No URL, headers or paths leave the app.
#[derive(Serialize)]
struct ApiDownload {
    id: i64,
    filename: String,
    status: &'static str,
    downloaded_size: u64,
    total_size: u64,
    speed: f64,
    error_message: Option<String>,
}

impl From<Download> for ApiDownload {
    fn from(row: Download) -> Self {
        let live = matches!(row.status, DownloadStatus::Downloading | DownloadStatus::Merging);
        Self {
            id: row.id.unwrap_or_default(),
            filename: row.filename,
            status: row.status.as_str(),
            downloaded_size: row.downloaded_size,
            total_size: row.total_size,
            speed: if live { row.speed } else { 0.0 },
            error_message: (row.status == DownloadStatus::Failed)
                .then_some(row.error_message)
                .flatten(),
        }
    }
}

/// Live and recent downloads for the extension popup.
pub(crate) async fn handle_downloads(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let state = app.state::<AppState>();
    check_api_token(&headers, &state, origin_of(&headers))?;
    let rows = state.db.recent_activity(8).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let stats = state.db.download_stats().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let downloads: Vec<ApiDownload> = rows.into_iter().map(ApiDownload::from).collect();
    Ok(Json(serde_json::json!({
        "downloads": downloads,
        "active": stats.active,
        "queued": stats.queued,
        "speed": stats.current_speed,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::DownloadCategory;

    #[tokio::test]
    async fn health_is_unauthenticated_ok() {
        let Json(body) = handle_health().await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["service"], "falcon-dm");
        assert_eq!(body["protocol"], 2);
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn popup_rows_carry_no_url_cookies_or_paths() {
        let row = Download {
            id: Some(5),
            url: "https://secret.example/token=abc".into(),
            filename: "movie.mp4".into(),
            save_path: "/Users/me/Downloads".into(),
            total_size: 100,
            downloaded_size: 40,
            status: DownloadStatus::Paused,
            category: DownloadCategory::Video,
            speed: 99.0,
            segments: 4,
            priority: 1,
            created_at: "t".into(),
            completed_at: None,
            error_message: Some("old error".into()),
            referrer: Some("https://secret.example/page".into()),
            user_agent: None,
            cookies: Some("sid=1".into()),
            archived: false,
            attempts: 0,
            retry_at: None,
        };
        let json = serde_json::to_string(&ApiDownload::from(row)).unwrap();
        assert!(!json.contains("secret.example"), "{json}");
        assert!(!json.contains("sid=1"), "{json}");
        assert!(!json.contains("/Users/me"), "{json}");
        assert!(json.contains("\"speed\":0.0"), "a paused row has no speed: {json}");
        assert!(json.contains("\"error_message\":null"), "{json}");
    }
}
