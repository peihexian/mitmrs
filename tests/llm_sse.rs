//! Reproduces the upstream LLM call issued by WorkBuddy Manager
//! (`POST {chatBase}/v2/chat/completions`, `Accept: application/json,
//! text/event-stream`, `stream: true`) and proves that MitmRs captures both the
//! prompt and the streamed completion.
//!
//! The backend builds its clients with `reqwest` + the `rustls-tls` feature,
//! which trusts **only** the bundled webpki roots and ignores the Windows
//! certificate store. `app_client(.., Some(ca))` models the small patch that
//! adds the MitmRs CA via `add_root_certificate`; `app_client(.., None)` is the
//! shipped behaviour and must end up as a recorded TLS failure + passthrough.

use mitmrs::ca::CertAuthority;
use mitmrs::config::RuntimeConfig;
use mitmrs::model::SearchQuery;
use mitmrs::state::AppState;
use mitmrs::store::Store;
use mitmrs::upstream::UpstreamClient;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn make_state(name: &str) -> (Arc<AppState>, tempfile::TempDir) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    let db_path = PathBuf::from(dir.path()).join(format!("{name}.db"));
    let store = Store::open(db_path.clone()).await.unwrap();
    let ca = Arc::new(
        CertAuthority::load_or_create(PathBuf::from(dir.path()).join("ca"))
            .await
            .unwrap(),
    );
    // The mock origin uses a self-signed certificate; accept it on the upstream
    // leg exactly like `--insecure-upstream` does.
    let upstream = UpstreamClient::new(true, None).unwrap();
    let config = RuntimeConfig {
        proxy_host: "127.0.0.1".into(),
        proxy_port: free_port(),
        max_body_bytes: 1024 * 1024,
        capture_enabled: true,
        upstream_proxy_enabled: false,
        upstream_proxy_url: String::new(),
    };
    let state = AppState::new(
        store,
        ca,
        upstream,
        config,
        PathBuf::from(dir.path()),
        db_path,
        "127.0.0.1".into(),
        free_port(),
        true,
    )
    .await;
    (state, dir)
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

/// Minimal TLS origin that speaks the provider's SSE chat endpoint.
async fn spawn_sse_origin() -> (u16, tokio::task::JoinHandle<()>) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let cert_der = CertificateDer::from(certified.cert.der().to_vec());
    let key_der = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let header_end = loop {
                    let n = tls.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(end) = find_header_end(&buf) {
                        break end;
                    }
                    if buf.len() > 1 << 20 {
                        return;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
                assert!(
                    head.starts_with("post /v2/chat/completions http/1.1"),
                    "unexpected request line: {head}"
                );
                assert!(
                    head.contains(&format!("host: 127.0.0.1:{port}")),
                    "Host must name the origin, not the proxy/mirror: {head}"
                );
                assert!(
                    head.contains("accept: application/json, text/event-stream"),
                    "the app's Accept header should reach the origin: {head}"
                );
                assert!(
                    head.contains("authorization: bearer test-token"),
                    "bearer token should reach the origin: {head}"
                );
                assert!(
                    head.contains("x-codebuddy-request: 1"),
                    "custom header should reach the origin: {head}"
                );
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while buf.len() < header_end + length {
                    let n = tls.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body =
                    String::from_utf8_lossy(&buf[header_end..(header_end + length).min(buf.len())])
                        .to_string();
                assert!(
                    body.contains("请用一句话介绍你自己"),
                    "prompt should reach the origin, got: {body}"
                );

                tls.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n",
                )
                .await
                .unwrap();
                let frames = [
                    "data: {\"id\":\"c1\",\"model\":\"claude-3-5-sonnet\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"你好\"}}]}\n\n",
                    "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"，我是 WorkBuddy\"}}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":8}}\n\n",
                    "data: [DONE]\n\n",
                ];
                for frame in frames {
                    if tls.write_all(frame.as_bytes()).await.is_err() {
                        return;
                    }
                    let _ = tls.flush().await;
                    tokio::time::sleep(Duration::from_millis(80)).await;
                }
                let _ = tls.shutdown().await;
            });
        }
    });
    (port, task)
}

/// Same client shape as `accounts::protocol::HttpPool::build`: reqwest with
/// `http1_only`, an explicit proxy, and rustls' bundled roots (plus the MitmRs
/// CA when `ca` is provided).
fn app_client(proxy_port: u16, ca: Option<&str>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .http1_only()
        .pool_idle_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(10))
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{proxy_port}")).unwrap());
    if let Some(pem) = ca {
        builder = builder.add_root_certificate(
            reqwest::Certificate::from_pem(pem.as_bytes()).expect("valid CA pem"),
        );
    }
    builder.build().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn captures_llm_prompt_and_streamed_completion() {
    let (origin_port, origin_task) = spawn_sse_origin().await;
    let (state, _dir) = make_state("llm-sse").await;
    mitmrs::proxy::start(state.clone()).await.unwrap();
    let proxy_port = state.config.read().await.proxy_port;

    let client = app_client(proxy_port, Some(state.ca.pem()));
    let request_body = json!({
        "model": "claude-3-5-sonnet",
        "stream": true,
        "messages": [{"role": "user", "content": "请用一句话介绍你自己"}]
    });
    let mut response = client
        .post(format!(
            "https://127.0.0.1:{origin_port}/v2/chat/completions"
        ))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("authorization", "Bearer test-token")
        .header("x-codebuddy-request", "1")
        .header("origin", "https://www.workbuddy.ai")
        .json(&request_body)
        .send()
        .await
        .expect("the LLM call should succeed through the proxy");
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let mut streamed = String::new();
    while let Some(chunk) = response.chunk().await.unwrap() {
        streamed.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(streamed.contains("你好"), "client saw: {streamed}");
    assert!(streamed.contains("[DONE]"), "client saw: {streamed}");

    let mut flow_id = None;
    for _ in 0..100 {
        let query = SearchQuery {
            host: Some("127.0.0.1".into()),
            ..Default::default()
        };
        if let Some(item) = state.store.search(&query).await.unwrap().items.first() {
            flow_id = Some(item.id.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let flow_id = flow_id.expect("the LLM call should be captured");

    let mut record = None;
    for _ in 0..100 {
        if let Some(rec) = state.store.get_record(&flow_id).await.unwrap() {
            if rec.duration_ms.is_some() && !rec.response_body.is_empty() {
                record = Some(rec);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let rec = record.expect("the SSE body should be persisted");
    assert_eq!(rec.protocol, "sse");
    assert_eq!(rec.path, "/v2/chat/completions");
    assert_eq!(rec.response_mime.as_deref(), Some("text/event-stream"));

    let captured_request = String::from_utf8_lossy(&rec.request_body);
    assert!(
        captured_request.contains("请用一句话介绍你自己"),
        "prompt should be captured: {captured_request}"
    );
    assert!(
        rec.request_headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("authorization")
                && h.value == "Bearer test-token"),
        "auth header should be captured"
    );

    let captured_response = String::from_utf8_lossy(&rec.response_body);
    assert!(
        captured_response.contains("你好"),
        "streamed content should be captured: {captured_response}"
    );
    assert!(
        captured_response.contains("[DONE]"),
        "stream terminator should be captured"
    );
    let decoded = mitmrs::decode::decode_record(&rec, "response", "sse", &json!({}), 1_000_000)
        .expect("SSE decode");
    assert!(decoded.text.unwrap_or_default().contains("WorkBuddy"));

    mitmrs::proxy::stop(&state).await.unwrap();
    origin_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn untrusted_local_ca_is_recorded_and_learns_passthrough() {
    let (origin_port, origin_task) = spawn_sse_origin().await;
    let (state, _dir) = make_state("llm-untrusted").await;
    mitmrs::proxy::start(state.clone()).await.unwrap();
    let proxy_port = state.config.read().await.proxy_port;

    // Exactly the shipped backend configuration: rustls' bundled roots only.
    let client = app_client(proxy_port, None);
    let result = client
        .post(format!(
            "https://127.0.0.1:{origin_port}/v2/chat/completions"
        ))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(&json!({"stream": true, "messages": []}))
        .send()
        .await;
    assert!(
        result.is_err(),
        "the bundled webpki roots must reject the MitmRs certificate"
    );

    let mut failed = None;
    for _ in 0..100 {
        let query = SearchQuery {
            tag: Some("tls-failed".into()),
            ..Default::default()
        };
        if let Some(item) = state.store.search(&query).await.unwrap().items.first() {
            failed = Some(item.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let failed = failed.expect("the certificate rejection should be recorded");
    assert_eq!(failed.protocol, "tls");
    assert!(
        failed
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("UnknownCA"),
        "alert should be visible: {:?}",
        failed.error
    );
    assert!(
        state.is_passthrough("127.0.0.1"),
        "the host should be learned for passthrough"
    );

    mitmrs::proxy::stop(&state).await.unwrap();
    origin_task.abort();
}

/// The source cannot be modified: point the app's configurable upstream base at
/// a plaintext mirror instead. The client keeps its bundled webpki roots and
/// still talks plain HTTP to 127.0.0.1, while MitmRs performs the real TLS leg
/// and records the full conversation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mirror_captures_llm_sse_without_any_trust_change() {
    let (origin_port, origin_task) = spawn_sse_origin().await;
    let (state, _dir) = make_state("llm-mirror").await;
    mitmrs::proxy::start(state.clone()).await.unwrap();

    let spec =
        mitmrs::mirror::MirrorSpec::parse(&format!("127.0.0.1:0=https://127.0.0.1:{origin_port}"))
            .unwrap();
    let mirror_addr = mitmrs::mirror::start(state.clone(), spec).await.unwrap();

    // Exactly the shipped backend: plain HTTP base, no proxy, no extra CA.
    let client = reqwest::Client::builder()
        .http1_only()
        .no_proxy()
        .read_timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let request_body = json!({
        "model": "claude-3-5-sonnet",
        "stream": true,
        "messages": [{"role": "user", "content": "请用一句话介绍你自己"}]
    });
    let mut response = client
        .post(format!("http://{mirror_addr}/v2/chat/completions"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("authorization", "Bearer test-token")
        .header("x-codebuddy-request", "1")
        .json(&request_body)
        .send()
        .await
        .expect("plaintext mirror call should succeed without any CA");
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let mut streamed = String::new();
    while let Some(chunk) = response.chunk().await.unwrap() {
        streamed.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(streamed.contains("你好"), "client saw: {streamed}");
    assert!(streamed.contains("[DONE]"), "client saw: {streamed}");

    let mut flow_id = None;
    for _ in 0..100 {
        let query = SearchQuery {
            tag: Some("mirror".into()),
            ..Default::default()
        };
        if let Some(item) = state.store.search(&query).await.unwrap().items.first() {
            flow_id = Some(item.id.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let flow_id = flow_id.expect("the mirrored call should be captured");

    let mut record = None;
    for _ in 0..100 {
        if let Some(rec) = state.store.get_record(&flow_id).await.unwrap() {
            if rec.duration_ms.is_some() && !rec.response_body.is_empty() {
                record = Some(rec);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let rec = record.expect("the mirrored SSE body should be persisted");
    assert_eq!(rec.protocol, "sse");
    assert_eq!(rec.scheme, "https");
    assert_eq!(rec.host, "127.0.0.1");
    assert_eq!(rec.port, origin_port);
    assert_eq!(rec.path, "/v2/chat/completions");
    assert!(
        String::from_utf8_lossy(&rec.request_body).contains("请用一句话介绍你自己"),
        "prompt should be captured"
    );
    assert!(
        String::from_utf8_lossy(&rec.response_body).contains("[DONE]"),
        "streamed completion should be captured"
    );
    // No TLS failure was involved: the client never saw the local CA.
    let failures = state
        .store
        .search(&SearchQuery {
            tag: Some("tls-failed".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(failures.total, 0);

    mitmrs::proxy::stop(&state).await.unwrap();
    origin_task.abort();
}
