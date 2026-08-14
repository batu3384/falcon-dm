use crate::native_messaging::PairRequest;
use crate::util::{lock_or_recover, LEGACY_DEFAULT_API_TOKEN};
use crate::{
    check_api_token, enqueue_download, extension_id_from_origin, is_valid_extension_id, AppState,
    ExternalDownloadPayload, MAX_PENDING_PAIR_REQUESTS,
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

fn check_rate_limit(state: &AppState) -> Result<(), StatusCode> {
    let mut bucket = lock_or_recover(&state.rate_bucket);
    let now = Instant::now();
    let window = Duration::from_secs(60);
    while bucket.front().is_some_and(|t| now.duration_since(*t) > window) {
        bucket.pop_front();
    }
    if bucket.len() >= 120 {
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

#[derive(Deserialize)]
pub(crate) struct AddDownloadRequest {
    url: String,
    filename: String,
    referrer: Option<String>,
    user_agent: Option<String>,
    cookies: Option<String>,
    cookie_url: Option<String>,
    format: Option<String>,
}

pub(crate) async fn handle_api_add(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<AddDownloadRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let state = app.state::<AppState>();
    check_api_token(&headers, &state, headers.get("origin").and_then(|v| v.to_str().ok()))?;

    let ext = ExternalDownloadPayload {
        url: payload.url,
        filename: Some(payload.filename),
        referrer: payload.referrer,
        user_agent: payload.user_agent,
        cookies: payload.cookies,
        cookie_url: payload.cookie_url,
        title: None,
        format: payload.format,
        save_path: None,
    };

    match enqueue_download(&app, ext).await {
        Ok(id) => Ok(Json(serde_json::json!({ "success": true, "id": id }))),
        Err(e) => Ok(Json(serde_json::json!({ "success": false, "error": e }))),
    }
}

#[derive(Deserialize, Clone, Serialize)]
pub(crate) struct InterceptRequest {
    pub url: String,
    pub page_url: Option<String>,
    pub media_type: Option<String>,
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
) -> Result<Json<serde_json::Value>, StatusCode> {
    let state = app.state::<AppState>();
    check_api_token(&headers, &state, headers.get("origin").and_then(|v| v.to_str().ok()))?;

    let ext = ExternalDownloadPayload {
        url: payload.url,
        filename: payload.filename,
        referrer: payload.referer.or(payload.page_url.clone()),
        user_agent: payload.user_agent,
        cookies: payload.cookies,
        cookie_url: payload.cookie_url,
        title: payload.title,
        format: payload.format,
        save_path: None,
    };

    match enqueue_download(&app, ext).await {
        Ok(id) => Ok(Json(serde_json::json!({ "success": true, "id": id }))),
        Err(e) => Ok(Json(serde_json::json!({ "success": false, "error": e }))),
    }
}

pub(crate) async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true, "service": "falcon-dm" }))
}

/// Pair browser extension ↔ desktop. Requires native proof + user approval.
pub(crate) async fn handle_pair(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
    Json(payload): Json<PairRequest>,
) -> Response {
    let origin = headers.get("origin").and_then(|v| v.to_str().ok()).unwrap_or("");
    let Some(id) = extension_id_from_origin(origin) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if id != payload.extension_id
        || !is_valid_extension_id(id)
        || payload.challenge.is_empty()
        || payload.challenge.len() > 256
        || payload.proof.is_empty()
        || payload.proof.len() > 128
    {
        return StatusCode::FORBIDDEN.into_response();
    }

    let state = app.state::<AppState>();
    if !state.pair_proofs.consume(&payload.challenge, id, &payload.proof) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let token = lock_or_recover(&state.api_token).clone();
    if token.trim().is_empty() || token == LEGACY_DEFAULT_API_TOKEN {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    let allowed = lock_or_recover(&state.settings).allowed_extension_ids.clone();
    if allowed.iter().any(|x| x == id) {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "token": token,
                "extension_id": id,
            })),
        )
            .into_response();
    }

    // Pending consent — UI must approve
    let mut pending = lock_or_recover(&state.pending_pair_ids);
    if !pending.iter().any(|pending_id| pending_id == id) {
        if pending.len() >= MAX_PENDING_PAIR_REQUESTS {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        pending.push_back(id.to_string());
    }
    let _ = app.emit("pair-request", serde_json::json!({ "extension_id": id }));

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "ok": false,
            "pending": true,
            "extension_id": id,
        })),
    )
        .into_response()
}

pub(crate) async fn handle_ping(
    AxumState(app): AxumState<AppHandle>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let state = app.state::<AppState>();
    check_api_token(&headers, &state, headers.get("origin").and_then(|v| v.to_str().ok()))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
