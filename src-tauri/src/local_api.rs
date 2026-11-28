use crate::pairing::{is_valid_pair_code, PairRequestOutcome};
use crate::storage::models::{
    Download, DownloadCategory, DownloadFilter, DownloadStatus, SubtitleTrackSelection,
};
use crate::util::{lock_or_recover, LEGACY_DEFAULT_API_TOKEN};
use crate::{
    check_api_token, enqueue_download, extension_id_from_origin, ingest_uploaded_file,
    is_valid_extension_id, validate_upload_b64_len, AppState, ExternalDownloadPayload,
};
use axum::{
    body::Body,
    extract::{Query, State as AxumState},
    http::{HeaderMap, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Requests per minute. A 100-link grab plus popup polling stays well below.
const RATE_LIMIT_PER_MINUTE: usize = 600;
/// Blob uploads can write tens of megabytes each, so they get a much smaller bucket.
const UPLOAD_RATE_LIMIT_PER_MINUTE: usize = 10;
const RATE_WINDOW: Duration = Duration::from_secs(60);

// The local API is a loopback singleton, so its upload bucket is process-wide.
static UPLOAD_RATE_BUCKET: Mutex<VecDeque<Instant>> = Mutex::new(VecDeque::new());

fn consume_rate_token(bucket: &mut VecDeque<Instant>, now: Instant, limit: usize) -> bool {
    while bucket.front().is_some_and(|t| now.duration_since(*t) > RATE_WINDOW) {
        bucket.pop_front();
    }
    if bucket.len() >= limit {
        return false;
    }
    bucket.push_back(now);
    true
}

fn check_rate_limit(state: &AppState) -> Result<(), StatusCode> {
    let mut bucket = lock_or_recover(&state.rate_bucket);
    if consume_rate_token(&mut bucket, Instant::now(), RATE_LIMIT_PER_MINUTE) {
        Ok(())
    } else {
        Err(StatusCode::TOO_MANY_REQUESTS)
    }
}

fn check_upload_rate_limit() -> Result<(), StatusCode> {
    let mut bucket = lock_or_recover(&UPLOAD_RATE_BUCKET);
    if consume_rate_token(&mut bucket, Instant::now(), UPLOAD_RATE_LIMIT_PER_MINUTE) {
        Ok(())
    } else {
        Err(StatusCode::TOO_MANY_REQUESTS)
    }
}

fn check_rate_limit_for_path(state: &AppState, path: &str) -> Result<(), StatusCode> {
    // Uploads still count globally; the small upload bucket is additional.
    check_rate_limit(state)?;
    if path == "/api/upload" {
        check_upload_rate_limit()?;
    }
    Ok(())
}

pub(crate) async fn rate_limit_middleware(
    AxumState(app): AxumState<AppHandle>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    if path == "/api/health" {
        return Ok(next.run(req).await);
    }
    let state = app.state::<AppState>();
    check_rate_limit_for_path(&state, path)?;
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
    subtitle_tracks: Option<Vec<SubtitleTrackSelection>>,
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
                expected_hash: None,
                subtitle_tracks: payload.subtitle_tracks,
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
    /// `[{lang, kind?}]` from the YouTube picker. Unlike `format`, this is
    /// structured data and does not travel on the URL fragment.
    pub subtitle_tracks: Option<Vec<SubtitleTrackSelection>>,
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
                expected_hash: None,
                subtitle_tracks: payload.subtitle_tracks,
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

/// 3 = code pairing + `/api/downloads` + authenticated control/CLI endpoints.
pub(crate) const API_PROTOCOL: u32 = 3;

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

struct PairReply {
    status: StatusCode,
    body: Option<serde_json::Value>,
    prompt_user: bool,
}

impl PairReply {
    fn status_only(status: StatusCode) -> Self {
        Self { status, body: None, prompt_user: false }
    }
}

/// Pairing state machine without the UI side effects. An allowlisted id still
/// receives only a pending response until the user approves that exact code.
fn pair_reply(state: &AppState, id: &str, code: &str) -> PairReply {
    let token = lock_or_recover(&state.api_token).clone();
    if token.trim().is_empty() || token == LEGACY_DEFAULT_API_TOKEN {
        return PairReply::status_only(StatusCode::SERVICE_UNAVAILABLE);
    }

    match lock_or_recover(&state.pairing).request(id, code) {
        PairRequestOutcome::Approved => {
            // Approval is not a permanent bearer grant: revocation before the
            // handoff closes it, and the pairing state consumes it after one use.
            if !lock_or_recover(&state.settings).allowed_extension_ids.iter().any(|x| x == id) {
                return PairReply::status_only(StatusCode::FORBIDDEN);
            }
            PairReply {
                status: StatusCode::OK,
                body: Some(serde_json::json!({ "ok": true, "token": token, "extension_id": id })),
                prompt_user: false,
            }
        }
        PairRequestOutcome::Rejected => PairReply::status_only(StatusCode::FORBIDDEN),
        PairRequestOutcome::TooMany => PairReply::status_only(StatusCode::TOO_MANY_REQUESTS),
        PairRequestOutcome::Pending { is_new } => PairReply {
            status: StatusCode::ACCEPTED,
            body: Some(serde_json::json!({ "ok": false, "pending": true, "extension_id": id })),
            prompt_user: is_new,
        },
    }
}

/// Pair browser extension ↔ desktop.
///
/// The browser sets `Origin` for extension requests and web pages cannot forge
/// a `chrome-extension://` origin. Every pairing request — including one from
/// an already allowlisted extension — waits for user approval before the token
/// is handed over exactly once.
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
    let reply = pair_reply(&state, id, &payload.code);
    if reply.prompt_user {
        let _ = app
            .emit("pair-request", serde_json::json!({ "extension_id": id, "code": payload.code }));
        // The prompt is useless behind other windows.
        crate::show_main_window(&app);
    }
    match reply.body {
        Some(body) => (reply.status, Json(body)).into_response(),
        None => reply.status.into_response(),
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

fn action_response(result: Result<(), String>) -> Response {
    match result {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "success": true }))).into_response(),
        Err(e) => {
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "success": false, "error": e })))
                .into_response()
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct DownloadIdRequest {
    id: i64,
}

pub(crate) async fn handle_pause(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<DownloadIdRequest>,
) -> Response {
    let state = app.state::<AppState>();
    if let Err(status) = check_api_token(&headers, &state, origin_of(&headers)) {
        return status.into_response();
    }
    let result = crate::commands::pause_download_core(&state, payload.id, |id| {
        crate::emit_row(&app, &state.db, id);
    })
    .await;
    action_response(result)
}

pub(crate) async fn handle_resume(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<DownloadIdRequest>,
) -> Response {
    let state = app.state::<AppState>();
    if let Err(status) = check_api_token(&headers, &state, origin_of(&headers)) {
        return status.into_response();
    }
    let result = crate::commands::resume_download_core(&state, payload.id, |id| {
        crate::emit_row(&app, &state.db, id);
    });
    action_response(result)
}

#[derive(Deserialize)]
pub(crate) struct RemoveDownloadRequest {
    id: i64,
    delete_file: Option<bool>,
}

pub(crate) async fn handle_remove(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<RemoveDownloadRequest>,
) -> Response {
    let state = app.state::<AppState>();
    if let Err(status) = check_api_token(&headers, &state, origin_of(&headers)) {
        return status.into_response();
    }
    let result = crate::commands::remove_download_core(
        &state,
        payload.id,
        payload.delete_file.unwrap_or(false),
        |id| {
            let _ = app.emit("download-removed", id);
        },
    )
    .await;
    action_response(result)
}

pub(crate) async fn handle_pause_all(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let state = app.state::<AppState>();
    check_api_token(&headers, &state, origin_of(&headers))?;
    let paused = crate::pause_all(&app).await;
    Ok(Json(serde_json::json!({ "success": true, "paused": paused })))
}

pub(crate) async fn handle_resume_all(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let state = app.state::<AppState>();
    check_api_token(&headers, &state, origin_of(&headers))?;
    let resumed = crate::resume_all(&app);
    Ok(Json(serde_json::json!({ "success": true, "resumed": resumed })))
}

#[derive(Deserialize)]
pub(crate) struct SpeedLimitRequest {
    id: Option<i64>,
    kbps: u32,
}

pub(crate) async fn handle_speed_limit(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<SpeedLimitRequest>,
) -> Response {
    let state = app.state::<AppState>();
    if let Err(status) = check_api_token(&headers, &state, origin_of(&headers)) {
        return status.into_response();
    }
    let result = match payload.id {
        Some(id) => match crate::commands::update_download_speed_limit(
            &state.db,
            &state.queue,
            id,
            payload.kbps,
        ) {
            Ok(true) => {
                crate::emit_row(&app, &state.db, id);
                Ok(())
            }
            Ok(false) => Err("Download not found".into()),
            Err(e) => Err(e),
        },
        None => crate::commands::update_global_speed_limit(&state, payload.kbps).map(|_| ()),
    };
    action_response(result)
}

pub(crate) async fn handle_stats(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
) -> Response {
    let state = app.state::<AppState>();
    if let Err(status) = check_api_token(&headers, &state, origin_of(&headers)) {
        return status.into_response();
    }
    match crate::commands::download_stats_for_state(&state) {
        Ok(stats) => (StatusCode::OK, Json(stats)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn parse_status_query(raw: Option<&str>) -> Result<Option<DownloadStatus>, String> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    match raw.to_ascii_lowercase().as_str() {
        "queued" => Ok(Some(DownloadStatus::Queued)),
        "downloading" => Ok(Some(DownloadStatus::Downloading)),
        "paused" => Ok(Some(DownloadStatus::Paused)),
        "completed" => Ok(Some(DownloadStatus::Completed)),
        "failed" => Ok(Some(DownloadStatus::Failed)),
        "merging" => Ok(Some(DownloadStatus::Merging)),
        _ => Err(format!("unknown status '{raw}'")),
    }
}

fn parse_category_query(raw: Option<&str>) -> Result<Option<DownloadCategory>, String> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    match raw.to_ascii_lowercase().as_str() {
        "video" => Ok(Some(DownloadCategory::Video)),
        "music" => Ok(Some(DownloadCategory::Music)),
        "document" => Ok(Some(DownloadCategory::Document)),
        "archive" => Ok(Some(DownloadCategory::Archive)),
        "program" => Ok(Some(DownloadCategory::Program)),
        "other" => Ok(Some(DownloadCategory::Other)),
        _ => Err(format!("unknown category '{raw}'")),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct DownloadsAllQuery {
    status: Option<String>,
    category: Option<String>,
    search: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
    include: Option<String>,
}

/// Automation rows stay metadata-only by default: no cookies, headers or
/// paths ever, and no URL unless the caller explicitly passes `include=url`.
#[derive(Serialize)]
struct ApiDownloadDetail {
    id: i64,
    filename: String,
    status: &'static str,
    category: &'static str,
    downloaded_size: u64,
    total_size: u64,
    speed: f64,
    error_message: Option<String>,
    created_at: String,
    completed_at: Option<String>,
    speed_limit_kbps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

impl ApiDownloadDetail {
    fn from_download(row: Download, include_url: bool) -> Self {
        let live = matches!(row.status, DownloadStatus::Downloading | DownloadStatus::Merging);
        Self {
            id: row.id.unwrap_or_default(),
            filename: row.filename,
            status: row.status.as_str(),
            category: row.category.as_str(),
            downloaded_size: row.downloaded_size,
            total_size: row.total_size,
            speed: if live { row.speed } else { 0.0 },
            error_message: (row.status == DownloadStatus::Failed)
                .then_some(row.error_message)
                .flatten(),
            created_at: row.created_at,
            completed_at: row.completed_at,
            speed_limit_kbps: row.speed_limit_kbps,
            url: include_url.then_some(row.url),
        }
    }
}

fn downloads_all_payload(
    state: &AppState,
    query: DownloadsAllQuery,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let status =
        parse_status_query(query.status.as_deref()).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let category = parse_category_query(query.category.as_deref())
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    // Pages are one-based; page=0 is clamped to the first page for tolerant scripting.
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(200).clamp(1, 200);
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let include_url = query.include.as_deref().is_some_and(|include| {
        include.split(',').any(|part| part.trim().eq_ignore_ascii_case("url"))
    });
    let filter = crate::commands::bounded_download_filter(
        DownloadFilter { status, category, search: query.search, ..Default::default() },
        Some(page_size),
        Some(offset),
    );
    let rows = state
        .db
        .get_downloads(&filter)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let downloads: Vec<ApiDownloadDetail> =
        rows.into_iter().map(|row| ApiDownloadDetail::from_download(row, include_url)).collect();
    Ok(serde_json::json!({
        "downloads": downloads,
        "count": downloads.len(),
        "page": page,
        "page_size": filter.limit.unwrap_or(page_size),
    }))
}

pub(crate) async fn handle_downloads_all(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Query(query): Query<DownloadsAllQuery>,
) -> Response {
    let state = app.state::<AppState>();
    if let Err(status) = check_api_token(&headers, &state, origin_of(&headers)) {
        return status.into_response();
    }
    match downloads_all_payload(&state, query) {
        Ok(payload) => (StatusCode::OK, Json(payload)).into_response(),
        Err((status, error)) => {
            (status, Json(serde_json::json!({ "success": false, "error": error }))).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::DownloadCategory;

    #[test]
    fn intercept_deserializes_structured_subtitle_choice() {
        let payload: InterceptRequest = serde_json::from_str(
            r#"{
                "url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "subtitle_tracks":[{"lang":"en"},{"lang":"tr","kind":"asr"}]
            }"#,
        )
        .unwrap();
        let tracks = payload.subtitle_tracks.unwrap();
        assert_eq!(tracks[0].lang, "en");
        assert_eq!(tracks[0].kind, None);
        assert_eq!(tracks[1].kind.as_deref(), Some("asr"));

        let without: InterceptRequest =
            serde_json::from_str(r#"{"url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ"}"#)
                .unwrap();
        assert_eq!(without.subtitle_tracks, None, "omitted choice keeps the default policy");
    }

    fn insert_api_row(
        state: &AppState,
        filename: &str,
        status: DownloadStatus,
        category: DownloadCategory,
        url: &str,
    ) -> i64 {
        state
            .db
            .insert_download(&Download {
                id: None,
                url: url.into(),
                filename: filename.into(),
                save_path: "/Users/me/Downloads".into(),
                total_size: 1_000,
                downloaded_size: 250,
                status,
                category,
                speed: 42.0,
                segments: 4,
                priority: 1,
                created_at: "2025-01-01T00:00:00Z".into(),
                completed_at: None,
                error_message: None,
                referrer: Some("https://referrer.example/secret".into()),
                user_agent: Some("secret-agent".into()),
                cookies: Some("sid=secret".into()),
                archived: false,
                attempts: 0,
                retry_at: None,
                expected_hash: None,
                speed_limit_kbps: Some(128),
                subtitle_tracks: None,
            })
            .unwrap()
    }

    #[tokio::test]
    async fn health_is_unauthenticated_ok() {
        let Json(body) = handle_health().await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["service"], "falcon-dm");
        assert_eq!(body["protocol"], 3);
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn allowed_extension_pairing_requires_approval_before_token() {
        let ext = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let state = crate::test_app_state("live-token", ext);
        let reply = pair_reply(&state, ext, "123456");
        assert_eq!(reply.status, StatusCode::ACCEPTED);
        assert!(reply.prompt_user);
        let body = reply.body.expect("pending response body");
        assert_eq!(body["pending"], true);
        assert!(body.get("token").is_none(), "unapproved pairing must not leak a token: {body}");
    }

    #[test]
    fn approved_repair_returns_token_once_and_keeps_allowlist() {
        let ext = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let token = "live-token";
        let state = crate::test_app_state(token, ext);
        let first = pair_reply(&state, ext, "123456");
        assert_eq!(first.status, StatusCode::ACCEPTED);
        assert!(first.body.expect("pending body").get("token").is_none());

        assert!(crate::util::lock_or_recover(&state.pairing).approve(ext));
        let approved = pair_reply(&state, ext, "123456");
        assert_eq!(approved.status, StatusCode::OK);
        let body = approved.body.expect("approved response body");
        assert_eq!(body["token"], token);
        assert!(crate::util::lock_or_recover(&state.settings)
            .allowed_extension_ids
            .iter()
            .any(|id| id == ext));

        let replay = pair_reply(&state, ext, "123456");
        assert_eq!(replay.status, StatusCode::ACCEPTED);
        assert!(replay.body.expect("fresh pending body").get("token").is_none());
    }

    #[test]
    fn approved_handoff_requires_extension_to_stay_allowlisted() {
        let ext = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let state = crate::test_app_state("live-token", ext);
        assert_eq!(pair_reply(&state, ext, "123456").status, StatusCode::ACCEPTED);
        assert!(crate::util::lock_or_recover(&state.pairing).approve(ext));
        crate::util::lock_or_recover(&state.settings).allowed_extension_ids.clear();
        let reply = pair_reply(&state, ext, "123456");
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(reply.body.is_none());
    }

    #[tokio::test]
    async fn action_response_uses_success_envelope_and_bad_request_errors() {
        let response = action_response(Ok(()));
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["success"], true);

        let response = action_response(Err("boom".into()));
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["success"], false);
        assert_eq!(json["error"], "boom");
    }

    #[test]
    fn endpoint_cli_auth_rejects_missing_bad_token_and_wrong_client() {
        let state = crate::test_app_state("live-token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let mut headers = HeaderMap::new();
        assert_eq!(
            check_api_token(&headers, &state, None),
            Err(StatusCode::UNAUTHORIZED),
            "missing token"
        );

        headers.insert("x-falcon-token", "wrong".parse().unwrap());
        headers.insert("x-falcon-client", "cli".parse().unwrap());
        assert_eq!(
            check_api_token(&headers, &state, None),
            Err(StatusCode::UNAUTHORIZED),
            "bad token"
        );

        headers.insert("x-falcon-token", "live-token".parse().unwrap());
        headers.insert("x-falcon-client", "extension".parse().unwrap());
        assert_eq!(
            check_api_token(&headers, &state, None),
            Err(StatusCode::FORBIDDEN),
            "wrong client header"
        );

        headers.insert("x-falcon-client", "cli".parse().unwrap());
        assert!(check_api_token(&headers, &state, None).is_ok());
    }

    #[test]
    fn rate_bucket_blocks_at_limit_and_prunes_after_window() {
        let start = Instant::now();
        let mut bucket = std::collections::VecDeque::new();
        for _ in 0..10 {
            assert!(consume_rate_token(&mut bucket, start, 10));
        }
        assert!(!consume_rate_token(&mut bucket, start, 10));
        assert!(consume_rate_token(&mut bucket, start + Duration::from_secs(61), 10));
    }

    #[test]
    fn upload_path_is_limited_to_ten_requests_per_minute() {
        crate::util::lock_or_recover(&UPLOAD_RATE_BUCKET).clear();
        let ext = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let state = crate::test_app_state("live-token", ext);
        for _ in 0..UPLOAD_RATE_LIMIT_PER_MINUTE {
            assert!(check_rate_limit_for_path(&state, "/api/upload").is_ok());
        }
        assert_eq!(
            check_rate_limit_for_path(&state, "/api/upload"),
            Err(StatusCode::TOO_MANY_REQUESTS)
        );
        assert!(check_rate_limit_for_path(&state, "/api/add").is_ok());
        crate::util::lock_or_recover(&UPLOAD_RATE_BUCKET).clear();
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
            expected_hash: None,
            speed_limit_kbps: None,
            subtitle_tracks: None,
        };
        let json = serde_json::to_string(&ApiDownload::from(row)).unwrap();
        assert!(!json.contains("secret.example"), "{json}");
        assert!(!json.contains("sid=1"), "{json}");
        assert!(!json.contains("/Users/me"), "{json}");
        assert!(json.contains("\"speed\":0.0"), "a paused row has no speed: {json}");
        assert!(json.contains("\"error_message\":null"), "{json}");
    }

    #[test]
    fn downloads_all_defaults_to_metadata_only_and_include_url_is_explicit() {
        let state = crate::test_app_state("live-token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        insert_api_row(
            &state,
            "movie.mp4",
            DownloadStatus::Paused,
            DownloadCategory::Video,
            "https://secret.example/video?token=abc",
        );

        let payload = downloads_all_payload(&state, DownloadsAllQuery::default()).unwrap();
        let rows = payload["downloads"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].get("url").is_none(), "URL requires opt-in: {rows:?}");
        assert_eq!(rows[0]["filename"], "movie.mp4");
        assert_eq!(rows[0]["speed_limit_kbps"], 128);
        let serialized = payload.to_string();
        assert!(!serialized.contains("secret.example"), "{serialized}");
        assert!(!serialized.contains("sid=secret"), "{serialized}");
        assert!(!serialized.contains("secret-agent"), "{serialized}");
        assert!(!serialized.contains("referrer.example"), "{serialized}");
        assert!(!serialized.contains("/Users/me"), "{serialized}");

        let with_url = downloads_all_payload(
            &state,
            DownloadsAllQuery { include: Some("url".into()), ..Default::default() },
        )
        .unwrap();
        assert_eq!(with_url["downloads"][0]["url"], "https://secret.example/video?token=abc");
    }

    #[test]
    fn downloads_all_filters_are_case_insensitive_and_search_is_applied() {
        let state = crate::test_app_state("live-token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        insert_api_row(
            &state,
            "alpha.mp4",
            DownloadStatus::Paused,
            DownloadCategory::Video,
            "https://media.example/alpha",
        );
        insert_api_row(
            &state,
            "beta.mp3",
            DownloadStatus::Completed,
            DownloadCategory::Music,
            "https://media.example/beta",
        );
        insert_api_row(
            &state,
            "gamma.pdf",
            DownloadStatus::Failed,
            DownloadCategory::Document,
            "https://media.example/gamma",
        );

        let paused = downloads_all_payload(
            &state,
            DownloadsAllQuery { status: Some("pAuSeD".into()), ..Default::default() },
        )
        .unwrap();
        let rows = paused["downloads"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["filename"], "alpha.mp4");

        let music = downloads_all_payload(
            &state,
            DownloadsAllQuery { category: Some("MUSIC".into()), ..Default::default() },
        )
        .unwrap();
        let rows = music["downloads"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["filename"], "beta.mp3");

        let searched = downloads_all_payload(
            &state,
            DownloadsAllQuery { search: Some("gamma".into()), ..Default::default() },
        )
        .unwrap();
        let rows = searched["downloads"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], "Failed");
    }

    #[test]
    fn downloads_all_paginates_in_stable_newest_first_pages() {
        let state = crate::test_app_state("live-token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let first = insert_api_row(
            &state,
            "one.bin",
            DownloadStatus::Queued,
            DownloadCategory::Other,
            "https://media.example/one",
        );
        let _second = insert_api_row(
            &state,
            "two.bin",
            DownloadStatus::Queued,
            DownloadCategory::Other,
            "https://media.example/two",
        );
        let third = insert_api_row(
            &state,
            "three.bin",
            DownloadStatus::Queued,
            DownloadCategory::Other,
            "https://media.example/three",
        );

        let page_one = downloads_all_payload(
            &state,
            DownloadsAllQuery { page: Some(1), page_size: Some(2), ..Default::default() },
        )
        .unwrap();
        assert_eq!(page_one["count"], 2);
        assert_eq!(page_one["page"], 1);
        assert_eq!(page_one["page_size"], 2);
        assert_eq!(page_one["downloads"][0]["id"], third);

        let page_two = downloads_all_payload(
            &state,
            DownloadsAllQuery { page: Some(2), page_size: Some(2), ..Default::default() },
        )
        .unwrap();
        assert_eq!(page_two["count"], 1);
        assert_eq!(page_two["downloads"][0]["id"], first);

        // Zero and negative input clamp to the first page rather than reversing meaning.
        let clamped = downloads_all_payload(
            &state,
            DownloadsAllQuery { page: Some(0), page_size: Some(0), ..Default::default() },
        )
        .unwrap();
        assert_eq!(clamped["page"], 1);
        assert_eq!(clamped["page_size"], 1);
        assert_eq!(clamped["downloads"][0]["id"], third);
    }

    #[test]
    fn downloads_all_rejects_unknown_filters_as_bad_requests() {
        let state = crate::test_app_state("live-token", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let err = downloads_all_payload(
            &state,
            DownloadsAllQuery { status: Some("running".into()), ..Default::default() },
        )
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("unknown status"));
        let err = downloads_all_payload(
            &state,
            DownloadsAllQuery { category: Some("disk".into()), ..Default::default() },
        )
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("unknown category"));
    }
}
