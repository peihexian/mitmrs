use super::connection::serve_mitm_connect;
use crate::model::*;
use crate::state::AppState;
use bytes::Bytes;
use chrono::Utc;
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full, StreamBody};
use hyper::body::{Body, Frame, Incoming};
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::http::uri::Authority;
use hyper::{Method, Request, Response, StatusCode, Uri, Version};
use std::collections::HashSet;
use std::convert::Infallible;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

pub type ProxyBody = BoxBody<Bytes, BoxError>;
type BoxError = Box<dyn std::error::Error + Send + Sync>;

const MAX_REQUEST_BODY: usize = 128 * 1024 * 1024;

#[derive(Clone)]
pub struct ConnMeta {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub tls_sni: Option<String>,
    pub client: std::net::SocketAddr,
    pub transparent: bool,
    pub cancel: CancellationToken,
}

pub async fn handle_request(
    state: Arc<AppState>,
    req: Request<Incoming>,
    meta: ConnMeta,
) -> Response<ProxyBody> {
    if req.method() == Method::CONNECT && !meta.transparent {
        return handle_connect(state, req, meta).await;
    }
    {
        let rt = state.proxy.lock().await;
        if rt.state == "paused" {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "MitmRs is paused. Resume the proxy in the web UI or call proxy_control.",
            );
        }
        if rt.state != "running" {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "MitmRs proxy is not running.",
            );
        }
    }
    match proxy_request(state, req, meta).await {
        Ok(resp) => resp,
        Err(e) => error_response(StatusCode::BAD_GATEWAY, &e),
    }
}

async fn handle_connect(
    state: Arc<AppState>,
    mut req: Request<Incoming>,
    meta: ConnMeta,
) -> Response<ProxyBody> {
    {
        let rt = state.proxy.lock().await;
        if rt.state == "paused" {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "MitmRs is paused.");
        }
        if rt.state != "running" {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "MitmRs proxy is not running.",
            );
        }
    }
    let authority = req.uri().authority().cloned().or_else(|| {
        req.headers()
            .get(hyper::header::HOST)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| Authority::from_str(s).ok())
    });
    let Some(authority) = authority else {
        return error_response(StatusCode::BAD_REQUEST, "CONNECT requires an authority");
    };
    let host = authority.host().to_string();
    let port = authority.port_u16().unwrap_or(443);
    if host.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "CONNECT authority has no host");
    }
    let on_upgrade = hyper::upgrade::on(&mut req);
    let client = meta.client;
    let cancel = meta.cancel.clone();
    tokio::spawn(serve_mitm_connect(
        state, on_upgrade, host, port, client, cancel,
    ));
    Response::builder()
        .status(StatusCode::OK)
        .body(empty_body())
        .unwrap_or_else(|_| error_response(StatusCode::INTERNAL_SERVER_ERROR, "response error"))
}

#[derive(Debug, Clone)]
struct Target {
    scheme: String,
    host: String,
    port: u16,
    path_and_query: String,
    absolute_uri: Uri,
    query: Option<String>,
}

async fn proxy_request(
    state: Arc<AppState>,
    req: Request<Incoming>,
    meta: ConnMeta,
) -> Result<Response<ProxyBody>, String> {
    let started = Utc::now();
    let start_instant = Instant::now();
    let target = resolve_target(&req, &meta)?;
    let cfg = state.config.read().await.clone();
    let max_capture = cfg.max_body_bytes as usize;
    let request_headers = header_entries(req.headers());
    let request_mime = mime_from_headers(&request_headers);
    let (parts, body) = req.into_parts();
    let collected = match body.collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => return Err(format!("read request body failed: {e}")),
    };
    if collected.len() > MAX_REQUEST_BODY {
        return Err(format!(
            "request body too large to buffer ({} bytes, limit {})",
            collected.len(),
            MAX_REQUEST_BODY
        ));
    }
    let request_size = collected.len() as u64;
    let capture_len = collected.len().min(max_capture);
    let request_capture = collected[..capture_len].to_vec();
    let request_truncated = capture_len < collected.len();
    let flow_id = uuid::Uuid::now_v7().to_string();
    let version_text = http_version_string(parts.version);
    let flow_meta = RecordInit {
        id: flow_id.clone(),
        started,
        version: version_text,
    };

    let upstream_req = build_upstream_request(
        &parts.method,
        &parts.headers,
        parts.version,
        &target,
        collected,
    )?;
    let response = match state.upstream.send(upstream_req).await {
        Ok(r) => r,
        Err(e) => {
            let msg = format!(
                "upstream request to {}://{}:{} failed: {e}",
                target.scheme, target.host, target.port
            );
            let mut rec = base_record(
                &flow_meta,
                &meta,
                &target,
                &parts.method,
                &request_headers,
                &request_capture,
                request_size,
                request_truncated,
                request_mime,
            );
            rec.status = Some(502);
            rec.reason = Some("Bad Gateway".into());
            rec.response_headers = vec![HeaderEntry::new(
                "content-type",
                "text/plain; charset=utf-8",
            )];
            rec.response_body = msg.as_bytes().to_vec();
            rec.response_body_size = msg.len() as u64;
            rec.response_mime = Some("text/plain".into());
            rec.error = Some(msg.clone());
            rec.ended_at = Some(Utc::now());
            rec.duration_ms = Some(start_instant.elapsed().as_millis() as i64);
            state.record_new(rec).await;
            return Ok(error_response(StatusCode::BAD_GATEWAY, &msg));
        }
    };

    let (resp_parts, resp_body) = response.into_parts();
    let status = resp_parts.status;
    let reason = status.canonical_reason().unwrap_or("").to_string();
    let response_headers = header_entries(&resp_parts.headers);
    let response_mime = mime_from_headers(&response_headers);
    let protocol = detect_protocol(&request_headers, &response_headers, parts.version);
    let mut rec = base_record(
        &flow_meta,
        &meta,
        &target,
        &parts.method,
        &request_headers,
        &request_capture,
        request_size,
        request_truncated,
        request_mime,
    );
    rec.status = Some(status.as_u16());
    rec.reason = Some(reason.clone());
    rec.response_headers = response_headers.clone();
    rec.response_mime = response_mime.clone();
    rec.protocol = protocol;
    let no_body = parts.method == Method::HEAD
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
        || resp_body.size_hint().exact() == Some(0);
    if no_body {
        rec.ended_at = Some(Utc::now());
        rec.duration_ms = Some(start_instant.elapsed().as_millis() as i64);
        state.record_new(rec).await;
        let mut builder = Response::builder().status(status);
        if let Some(headers) = builder.headers_mut() {
            apply_response_headers(headers, &resp_parts.headers);
        }
        return builder
            .body(empty_body())
            .map_err(|e| format!("build response: {e}"));
    }

    let captured = state.record_new(rec).await.is_some();

    let (tx, rx) = mpsc::channel::<Result<Frame<Bytes>, BoxError>>(32);
    let stream_body = StreamBody::new(ReceiverStream::new(rx)).boxed();

    // Build the response before spawning the drain task.
    let mut builder = Response::builder().status(status);
    if let Some(headers) = builder.headers_mut() {
        apply_response_headers(headers, &resp_parts.headers);
    }
    let response = builder
        .body(stream_body)
        .map_err(|e| format!("build response: {e}"))?;

    spawn_body_drain(
        state,
        flow_id,
        captured,
        resp_body,
        tx,
        response_headers,
        status.as_u16(),
        reason,
        start_instant,
        max_capture,
    );
    Ok(response)
}

struct RecordInit {
    id: String,
    started: chrono::DateTime<Utc>,
    version: String,
}

#[allow(clippy::too_many_arguments)]
fn spawn_body_drain(
    state: Arc<AppState>,
    flow_id: String,
    captured: bool,
    mut body: Incoming,
    tx: mpsc::Sender<Result<Frame<Bytes>, BoxError>>,
    response_headers: Vec<HeaderEntry>,
    status: u16,
    reason: String,
    start_instant: Instant,
    max_capture: usize,
) {
    tokio::spawn(async move {
        let mut capture: Vec<u8> = Vec::new();
        let mut total: u64 = 0;
        let mut truncated = false;
        let mut trailers: Vec<HeaderEntry> = Vec::new();
        let mut stream_error: Option<String> = None;
        loop {
            match body.frame().await {
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        total += data.len() as u64;
                        if capture.len() < max_capture {
                            let room = max_capture - capture.len();
                            let take = data.len().min(room);
                            capture.extend_from_slice(&data[..take]);
                            if take < data.len() {
                                truncated = true;
                            }
                        } else if !data.is_empty() {
                            truncated = true;
                        }
                    }
                    if let Some(t) = frame.trailers_ref() {
                        trailers = header_entries(t);
                    }
                    if tx.send(Ok(frame)).await.is_err() {
                        break;
                    }
                }
                Some(Err(e)) => {
                    stream_error = Some(e.to_string());
                    let _ = tx.send(Err(Box::new(e) as BoxError)).await;
                    break;
                }
                None => break,
            }
        }
        drop(tx);
        let elapsed = start_instant.elapsed().as_millis() as i64;
        if captured {
            state
                .record_response(
                    &flow_id,
                    status,
                    reason,
                    response_headers,
                    trailers,
                    capture,
                    total,
                    truncated,
                    Utc::now(),
                    elapsed,
                    stream_error,
                )
                .await;
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn base_record(
    init: &RecordInit,
    meta: &ConnMeta,
    target: &Target,
    method: &Method,
    request_headers: &[HeaderEntry],
    request_body: &[u8],
    request_size: u64,
    request_truncated: bool,
    request_mime: Option<String>,
) -> FlowRecord {
    let mut rec = FlowRecord::new(
        init.id.clone(),
        init.started,
        meta.client.to_string(),
        target.scheme.clone(),
        target.host.clone(),
        target.port,
        method.as_str().to_string(),
        target.path_and_query.clone(),
        target.query.clone(),
        init.version.clone(),
        meta.tls_sni.clone(),
    );
    rec.request_headers = request_headers.to_vec();
    rec.request_body = request_body.to_vec();
    rec.request_body_size = request_size;
    rec.request_truncated = request_truncated;
    rec.request_mime = request_mime;
    if meta.transparent {
        rec.tags.push("transparent".into());
    }
    rec
}

fn http_version_string(version: Version) -> String {
    match version {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2.0",
        Version::HTTP_3 => "HTTP/3.0",
        _ => "HTTP/1.1",
    }
    .to_string()
}

fn build_upstream_request(
    method: &Method,
    headers: &HeaderMap,
    version: Version,
    target: &Target,
    body: Bytes,
) -> Result<Request<Full<Bytes>>, String> {
    let mut builder = Request::builder()
        .method(method.clone())
        .uri(target.absolute_uri.clone())
        .version(version);
    let hop = connection_tokens(headers);
    for (name, value) in headers.iter() {
        if is_hop_by_hop(name.as_str()) || hop.contains(name.as_str()) {
            // HTTP/2 permits exactly `te: trailers`, and gRPC requires it.
            let te_trailers = name.as_str().eq_ignore_ascii_case("te")
                && value
                    .to_str()
                    .map(|v| v.trim().eq_ignore_ascii_case("trailers"))
                    .unwrap_or(false);
            if !te_trailers {
                continue;
            }
        }
        builder = builder.header(name, value);
    }
    if !headers.contains_key(hyper::header::HOST) {
        let authority = authority_string(&target.host, target.port, &target.scheme);
        if let Ok(v) = HeaderValue::from_str(&authority) {
            builder = builder.header(hyper::header::HOST, v);
        }
    }
    builder
        .body(Full::new(body))
        .map_err(|e| format!("build upstream request: {e}"))
}

fn apply_response_headers(dst: &mut HeaderMap, src: &HeaderMap) {
    let hop = connection_tokens(src);
    for (name, value) in src.iter() {
        if is_hop_by_hop(name.as_str()) || hop.contains(name.as_str()) {
            continue;
        }
        if name == hyper::header::TRANSFER_ENCODING {
            continue;
        }
        dst.append(name.clone(), value.clone());
    }
}

fn connection_tokens(headers: &HeaderMap) -> HashSet<String> {
    let mut set = HashSet::new();
    for value in headers.get_all(hyper::header::CONNECTION).iter() {
        if let Ok(s) = value.to_str() {
            for token in s.split(',') {
                let t = token.trim().to_ascii_lowercase();
                if !t.is_empty() {
                    set.insert(t);
                }
            }
        }
    }
    set
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn resolve_target(req: &Request<Incoming>, meta: &ConnMeta) -> Result<Target, String> {
    let uri = req.uri();
    let scheme = uri
        .scheme_str()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| meta.scheme.clone());
    if scheme != "http" && scheme != "https" {
        return Err(format!("unsupported scheme '{scheme}'"));
    }
    let host_header = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let (mut host, mut port) = if let Some(h) = uri.host() {
        (
            h.to_string(),
            uri.port_u16()
                .unwrap_or_else(|| fallback_port(meta, &scheme)),
        )
    } else if let Some(h) = host_header.as_deref() {
        parse_host_port(h, fallback_port(meta, &scheme))?
    } else if !meta.host.is_empty() {
        (meta.host.clone(), meta.port)
    } else {
        return Err("request has no host".into());
    };
    if host.is_empty() {
        return Err("request has an empty host".into());
    }
    host = host.trim_matches(['[', ']']).to_string();
    if port == 0 {
        port = if scheme == "https" { 443 } else { 80 };
    }
    let path_and_query = uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "/".into());
    let authority = authority_string(&host, port, &scheme);
    let absolute = format!("{scheme}://{authority}{path_and_query}");
    let absolute_uri =
        Uri::from_str(&absolute).map_err(|e| format!("invalid target URI {absolute}: {e}"))?;
    Ok(Target {
        scheme,
        host,
        port,
        path_and_query,
        absolute_uri,
        query: uri.query().map(|s| s.to_string()),
    })
}

fn parse_host_port(input: &str, default_port: u16) -> Result<(String, u16), String> {
    let authority =
        Authority::from_str(input).map_err(|e| format!("invalid Host header '{input}': {e}"))?;
    Ok((
        authority.host().to_string(),
        authority.port_u16().unwrap_or(default_port),
    ))
}

/// The MITM connection knows the original destination port (e.g. a CONNECT to
/// :18083). An HTTP/2 :authority often omits the port, so fall back to the
/// connection's port rather than assuming 443.
fn fallback_port(meta: &ConnMeta, scheme: &str) -> u16 {
    if meta.scheme == scheme && meta.port != 0 {
        meta.port
    } else if scheme == "https" {
        443
    } else {
        80
    }
}

fn authority_string(host: &str, port: u16, scheme: &str) -> String {
    let default = if scheme == "https" { 443 } else { 80 };
    let h = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == default {
        h
    } else {
        format!("{h}:{port}")
    }
}

fn mime_from_headers(headers: &[HeaderEntry]) -> Option<String> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| {
            h.value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|s| !s.is_empty())
}

fn detect_protocol(
    request_headers: &[HeaderEntry],
    response_headers: &[HeaderEntry],
    version: Version,
) -> String {
    let ct_req = mime_from_headers(request_headers).unwrap_or_default();
    let ct_resp = mime_from_headers(response_headers).unwrap_or_default();
    if ct_req.contains("grpc") || ct_resp.contains("grpc") {
        return "grpc".into();
    }
    if request_headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("upgrade") && h.value.to_ascii_lowercase().contains("websocket")
    }) {
        return "websocket".into();
    }
    if version == Version::HTTP_2 {
        "http2".into()
    } else {
        "http".into()
    }
}

fn empty_body() -> ProxyBody {
    Empty::<Bytes>::new()
        .map_err(|never| -> BoxError { match never {} })
        .boxed()
}

fn text_body(text: impl Into<Bytes>) -> ProxyBody {
    Full::new(text.into())
        .map_err(|never| -> BoxError { match never {} })
        .boxed()
}

fn error_response(status: StatusCode, message: &str) -> Response<ProxyBody> {
    let body = format!("MitmRs proxy error: {message}\n");
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .header("x-mitmrs-error", "1")
        .body(text_body(body))
        .unwrap_or_else(|_| Response::new(text_body("MitmRs proxy error")))
}

#[allow(dead_code)]
fn status_with_body(status: StatusCode, message: &str) -> Response<ProxyBody> {
    error_response(status, message)
}

#[allow(dead_code)]
fn sanitize_header_name(name: &str) -> Option<HeaderName> {
    HeaderName::from_bytes(name.as_bytes()).ok()
}

#[allow(dead_code)]
fn header_duration(start: Instant) -> Duration {
    start.elapsed()
}

#[allow(dead_code)]
fn _assert_send() {
    fn assert_send<T: Send>() {}
    assert_send::<ConnMeta>();
    assert_send::<Infallible>();
}
