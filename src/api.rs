use crate::control;
use crate::decode;
use crate::model::*;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/config", get(get_config))
        .route("/api/proxy/start", post(proxy_start))
        .route("/api/proxy/stop", post(proxy_stop))
        .route("/api/proxy/pause", post(proxy_pause))
        .route("/api/proxy/resume", post(proxy_resume))
        .route("/api/proxy/restart", post(proxy_restart))
        .route("/api/proxy/config", post(proxy_config))
        .route("/api/windivert/start", post(windivert_start))
        .route("/api/windivert/stop", post(windivert_stop))
        .route("/api/flows", get(list_flows))
        .route("/api/flows", delete(clear_flows))
        .route("/api/flows/{id}", get(get_flow))
        .route("/api/flows/{id}", delete(delete_flow))
        .route("/api/flows/{id}/decode", post(decode_flow))
        .route("/api/flows/{id}/body", get(get_body))
        .route("/api/decoders", get(list_decoders))
        .route("/api/stats", get(stats))
        .route("/api/events", get(events))
        .route("/api/export", get(export))
        .route("/api/import", post(import))
        .route("/api/ca", get(ca_info))
        .route("/api/ca.crt", get(ca_download))
        .route("/api/mcp/info", get(mcp_info))
        .route("/mcp", post(crate::mcp::http_post))
        .route("/mcp", get(crate::mcp::http_get))
        .route("/mcp", delete(crate::mcp::http_delete))
        .fallback(get(static_handler))
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({"ok": true, "name": "mitmrs", "version": crate::state::VERSION}))
}

async fn status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.status().await)
}

async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.read().await.clone();
    Json(json!({
        "proxy_host": cfg.proxy_host,
        "proxy_port": cfg.proxy_port,
        "max_body_bytes": cfg.max_body_bytes,
        "capture_enabled": cfg.capture_enabled,
    }))
}

async fn proxy_start(State(state): State<Arc<AppState>>) -> Response {
    match control::action(&state, "start", None).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn proxy_stop(State(state): State<Arc<AppState>>) -> Response {
    match control::action(&state, "stop", None).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn proxy_pause(State(state): State<Arc<AppState>>) -> Response {
    match control::action(&state, "pause", None).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn proxy_resume(State(state): State<Arc<AppState>>) -> Response {
    match control::action(&state, "resume", None).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn proxy_restart(State(state): State<Arc<AppState>>) -> Response {
    match control::action(&state, "restart", None).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn proxy_config(
    State(state): State<Arc<AppState>>,
    Json(patch): Json<ConfigPatch>,
) -> Response {
    match control::apply_config(&state, patch).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn windivert_start(State(state): State<Arc<AppState>>) -> Response {
    #[cfg(windows)]
    {
        match crate::windivert::start(state.clone()).await {
            Ok(()) => Json(state.status().await).into_response(),
            Err(e) => api_error(
                StatusCode::BAD_REQUEST,
                format!("WinDivert start failed (run as Administrator): {e:#}"),
            ),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = state;
        api_error(
            StatusCode::BAD_REQUEST,
            "WinDivert is only supported on Windows",
        )
    }
}

async fn windivert_stop(State(state): State<Arc<AppState>>) -> Response {
    #[cfg(windows)]
    {
        match crate::windivert::stop(&state).await {
            Ok(()) => Json(state.status().await).into_response(),
            Err(e) => api_error(StatusCode::BAD_REQUEST, format!("{e:#}")),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = state;
        api_error(
            StatusCode::BAD_REQUEST,
            "WinDivert is only supported on Windows",
        )
    }
}

async fn list_flows(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Response {
    match state.store.search(&query).await {
        Ok(r) => Json(r).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

async fn get_flow(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<BodyParams>,
) -> Response {
    let mode = params.body.unwrap_or_else(|| "text".into());
    let max = params
        .max_bytes
        .unwrap_or(2 * 1024 * 1024)
        .clamp(1024, 32 * 1024 * 1024);
    match state.store.get(&id, &mode, max).await {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "flow not found"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

#[derive(Debug, Deserialize)]
struct BodyParams {
    body: Option<String>,
    max_bytes: Option<usize>,
}

async fn delete_flow(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match control::delete_flow(&state, &id).await {
        Ok(true) => Json(json!({"ok": true})).into_response(),
        Ok(false) => api_error(StatusCode::NOT_FOUND, "flow not found"),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

async fn clear_flows(State(state): State<Arc<AppState>>) -> Response {
    match control::clear_traffic(&state).await {
        Ok(n) => Json(json!({"ok": true, "deleted": n})).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

async fn decode_flow(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<DecodeRequest>,
) -> Response {
    let direction = if req.direction.eq_ignore_ascii_case("request") {
        "request"
    } else {
        "response"
    };
    let Some(record) = (match state.store.get_record(&id).await {
        Ok(r) => r,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }) else {
        return api_error(StatusCode::NOT_FOUND, "flow not found");
    };
    let format = req.format.clone().unwrap_or_else(|| "auto".into());
    let max = req
        .max_bytes
        .unwrap_or(1_000_000)
        .clamp(256, 8 * 1024 * 1024);
    match decode::decode_record(&record, direction, &format, &req.options, max) {
        Ok(r) => Json(r).into_response(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e),
    }
}

async fn get_body(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<BodyParams>,
) -> Response {
    let Some(record) = (match state.store.get_record(&id).await {
        Ok(r) => r,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }) else {
        return api_error(StatusCode::NOT_FOUND, "flow not found");
    };
    let direction = params.body.as_deref().unwrap_or("response");
    let (headers, body, size, truncated) = if direction == "request" {
        (
            &record.request_headers,
            &record.request_body,
            record.request_body_size,
            record.request_truncated,
        )
    } else {
        (
            &record.response_headers,
            &record.response_body,
            record.response_body_size,
            record.response_truncated,
        )
    };
    let ct = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.clone());
    Json(json!({
        "id": id,
        "direction": direction,
        "size": size,
        "truncated": truncated,
        "content_type": ct,
        "body_base64": decode::base64_body(body),
        "body_text": String::from_utf8_lossy(body).into_owned(),
    }))
    .into_response()
}

async fn list_decoders() -> impl IntoResponse {
    Json(json!({"items": decode::decoders()}))
}

async fn stats(State(state): State<Arc<AppState>>) -> Response {
    match state.store.stats().await {
        Ok(s) => Json(s).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

async fn events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let initial = state.status().await;
    let stream = futures_util::stream::unfold((rx, 0u8), move |(mut rx, mut tick)| async move {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Ok(ev) => {
                        let event = Event::default().event(ev.event).data(ev.data.to_string());
                        Some((Ok(event), (rx, tick)))
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        let event = Event::default().event("flow").data(json!({"lagged": n}).to_string());
                        Some((Ok(event), (rx, tick)))
                    }
                    Err(broadcast::error::RecvError::Closed) => None,
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(15)) => {
                tick = tick.wrapping_add(1);
                let event = Event::default().event("ping").data("{}");
                Some((Ok(event), (rx, tick)))
            }
        }
    });
    // Push an immediate status snapshot so the UI does not wait for the first flow.
    let initial_event = Event::default()
        .event("status")
        .data(serde_json::to_string(&initial).unwrap_or_default());
    let combined = futures_util::stream::once(async move { Ok(initial_event) }).chain(stream);
    Sse::new(combined).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(20))
            .text("ping"),
    )
}

#[derive(Debug, Deserialize)]
struct ExportParams {
    format: Option<String>,
    max_flows: Option<usize>,
    #[serde(flatten)]
    query: SearchQuery,
}

async fn export(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExportParams>,
) -> Response {
    let format = params.format.unwrap_or_else(|| "json".into());
    let max = params.max_flows.unwrap_or(5000).clamp(1, 20_000);
    let result = match format.as_str() {
        "har" => state.store.export_har(&params.query, max).await,
        _ => state.store.export_json(&params.query, max).await,
    };
    match result {
        Ok(body) => {
            let ext = if format == "har" { "har" } else { "json" };
            let filename = format!(
                "mitmrs-export-{}.{}",
                chrono::Utc::now().format("%Y%m%d-%H%M%S"),
                ext
            );
            let mut resp = Response::new(Body::from(body));
            resp.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            );
            resp.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")).unwrap(),
            );
            resp
        }
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

async fn import(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let text = match std::str::from_utf8(&body) {
        Ok(t) => t,
        Err(_) => return api_error(StatusCode::BAD_REQUEST, "import body must be UTF-8 JSON"),
    };
    match state.store.import_json(text).await {
        Ok(r) => {
            // Refresh cached counters after merge.
            if let Ok(stats) = state.store.stats().await {
                use std::sync::atomic::Ordering;
                state.flow_count.store(stats.total, Ordering::Relaxed);
                state.bytes_total.store(
                    stats.bytes.request + stats.bytes.response,
                    Ordering::Relaxed,
                );
            }
            Json(r).into_response()
        }
        Err(e) => api_error(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

async fn ca_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({
        "subject": "CN=MitmRs Local CA, O=MitmRs",
        "fingerprint": state.ca.fingerprint(),
        "path": state.ca.cert_path().display().to_string(),
        "pem_url": "/api/ca.crt",
        "not_before": state.ca.not_before().to_rfc3339(),
        "not_after": state.ca.not_after().to_rfc3339(),
        "install": [
            "Windows (admin): certutil -addstore -f Root \"%LOCALAPPDATA%\\MitmRs\\ca\\ca.crt\"",
            "Firefox: Settings > Privacy & Security > Certificates > View Certificates > Authorities > Import",
            "curl: curl --cacert ca.crt -x http://127.0.0.1:8080 https://example.com"
        ]
    }))
}

async fn ca_download(State(state): State<Arc<AppState>>) -> Response {
    let pem = state.ca.pem().to_string();
    let mut resp = Response::new(Body::from(pem));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-x509-ca-cert"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"mitmrs-ca.crt\""),
    );
    resp
}

async fn mcp_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({
        "endpoint": format!("http://{}:{}/mcp", state.web_host, state.web_port),
        "stdio_command": format!("mitmrs mcp-stdio --url http://{}:{}/mcp", state.web_host, state.web_port),
        "transport": "streamable-http (JSON-RPC 2.0)",
        "tools": crate::mcp::tool_descriptors(),
        "examples": {
            "recent": {"tool": "traffic_recent", "arguments": {"limit": 20}},
            "search": {"tool": "traffic_search", "arguments": {"host": "api.openai.com", "limit": 20}},
            "decode_grpc": {"tool": "traffic_decode", "arguments": {"id": "<flow-id>", "direction": "response", "format": "grpc"}},
            "status": {"tool": "proxy_control", "arguments": {"action": "status"}}
        },
        "claude_code": format!("claude mcp add --transport http mitmrs http://{}:{}/mcp", state.web_host, state.web_port),
        "curl": format!("curl -s http://{}:{}/mcp -H 'content-type: application/json' -d '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}}'", state.web_host, state.web_port)
    }))
}

fn api_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if path.starts_with("api/") || path == "mcp" {
        return api_error(StatusCode::NOT_FOUND, "not found");
    }
    if let Some(file) = Assets::get(path) {
        return embedded_response(path, file.data.into_owned());
    }
    if let Some(file) = Assets::get("index.html") {
        return embedded_response("index.html", file.data.into_owned());
    }
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "MitmRs frontend is not built. Run `pnpm --dir web build` and rebuild the Rust binary.",
    )
        .into_response()
}

fn embedded_response(path: &str, bytes: Vec<u8>) -> Response {
    let mime = match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        _ => "application/octet-stream",
    };
    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if path == "index.html" {
            "no-cache"
        } else {
            "public, max-age=31536000, immutable"
        }),
    );
    resp
}
