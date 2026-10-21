use axum::{routing::get, Json, Router};
use chrono::Utc;
use mitmrs::ca::CertAuthority;
use mitmrs::config::RuntimeConfig;
use mitmrs::model::{FlowRecord, HeaderEntry};
use mitmrs::state::AppState;
use mitmrs::store::Store;
use mitmrs::upstream::UpstreamClient;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
    let upstream = UpstreamClient::new(true).unwrap();
    let config = RuntimeConfig {
        proxy_host: "127.0.0.1".into(),
        proxy_port: free_port(),
        max_body_bytes: 1024 * 1024,
        capture_enabled: true,
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

fn sample_flow() -> FlowRecord {
    let mut rec = FlowRecord::new(
        uuid::Uuid::now_v7().to_string(),
        Utc::now(),
        "127.0.0.1:4444".into(),
        "https".into(),
        "ai.example.com".into(),
        443,
        "POST".into(),
        "/v1/chat".into(),
        None,
        "HTTP/2.0".into(),
        Some("ai.example.com".into()),
    );
    rec.request_headers = vec![HeaderEntry::new("content-type", "application/json")];
    rec.response_headers = vec![HeaderEntry::new("content-type", "application/json")];
    rec.request_body =
        br#"{"model":"demo","messages":[{"role":"user","content":"hello"}]}"#.to_vec();
    rec.response_body =
        br#"{"id":"chatcmpl-1","choices":[{"message":{"content":"hi there"}}]}"#.to_vec();
    rec.request_body_size = rec.request_body.len() as u64;
    rec.response_body_size = rec.response_body.len() as u64;
    rec.request_mime = Some("application/json".into());
    rec.response_mime = Some("application/json".into());
    rec.status = Some(200);
    rec.reason = Some("OK".into());
    rec.duration_ms = Some(42);
    rec.ended_at = Some(Utc::now());
    rec.protocol = "http2".into();
    rec
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_initialize_list_search_and_decode() {
    let (state, _dir) = make_state("mcp").await;
    state.record_new(sample_flow()).await.unwrap();

    let init = mitmrs::mcp::handle_rpc(
        state.clone(),
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
    )
    .unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "mitmrs");

    let list = mitmrs::mcp::handle_rpc(
        state.clone(),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"traffic_search"));
    assert!(names.contains(&"traffic_recent"));
    assert!(names.contains(&"traffic_decode"));
    assert!(names.contains(&"proxy_control"));

    let recent = mitmrs::mcp::handle_rpc(
        state.clone(),
        json!({"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"traffic_recent","arguments":{"limit":1}}}),
    )
    .unwrap();
    assert_eq!(recent["result"]["isError"], false);
    assert_eq!(recent["result"]["structuredContent"]["total"], 1);
    assert_eq!(
        recent["result"]["structuredContent"]["items"][0]["host"],
        "ai.example.com"
    );

    let search = mitmrs::mcp::handle_rpc(
        state.clone(),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"traffic_search","arguments":{"q":"hello","limit":10}}}),
    )
    .unwrap();
    assert_eq!(search["result"]["isError"], false);
    assert_eq!(search["result"]["structuredContent"]["total"], 1);
    let id = search["result"]["structuredContent"]["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let decode = mitmrs::mcp::handle_rpc(
        state.clone(),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"traffic_decode","arguments":{"id":id,"direction":"response","format":"json"}}}),
    )
    .unwrap();
    assert_eq!(decode["result"]["isError"], false);
    assert_eq!(
        decode["result"]["structuredContent"]["tree"]["choices"][0]["message"]["content"],
        "hi there"
    );

    let status = mitmrs::mcp::handle_rpc(
        state.clone(),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"proxy_control","arguments":{"action":"status"}}}),
    )
    .unwrap();
    assert_eq!(status["result"]["isError"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_proxy_captures_flow_end_to_end() {
    let upstream_port = free_port();
    let upstream = Router::new().route(
        "/hello",
        get(|| async { Json(json!({"ok": true, "message": "hello from upstream"})) }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", upstream_port))
        .await
        .unwrap();
    let upstream_task = tokio::spawn(async move {
        let _ = axum::serve(listener, upstream).await;
    });

    let (state, _dir) = make_state("proxy").await;
    let proxy_port = state.config.read().await.proxy_port;
    mitmrs::proxy::start(state.clone()).await.unwrap();

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{proxy_port}")).unwrap())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let body: Value = client
        .get(format!("http://127.0.0.1:{upstream_port}/hello"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["message"], "hello from upstream");

    let mut found = 0;
    for _ in 0..100 {
        found = state.store.stats().await.unwrap().total;
        if found > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(found, 1);
    let mut flow = None;
    for _ in 0..100 {
        let flows = state.store.search(&Default::default()).await.unwrap();
        if let Some(f) = flows.items.first() {
            if f.response_size > 10 {
                flow = Some(f.clone());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let f = flow.expect("response body should be captured");
    assert_eq!(f.host, "127.0.0.1");
    assert_eq!(f.status, Some(200));
    assert!(f.response_size > 10);

    mitmrs::proxy::stop(&state).await.unwrap();
    upstream_task.abort();
}
