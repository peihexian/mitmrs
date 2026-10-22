use axum::{routing::get, Json, Router};
use chrono::Utc;
use mitmrs::ca::CertAuthority;
use mitmrs::config::RuntimeConfig;
use mitmrs::model::{ConfigPatch, FlowRecord, HeaderEntry};
use mitmrs::state::AppState;
use mitmrs::store::Store;
use mitmrs::upstream::UpstreamClient;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
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
async fn shutdown_ends_sse_stream_and_drains_server() {
    let (state, _dir) = make_state("shutdown").await;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let app = mitmrs::api::router(state.clone());
    let token = state.shutdown.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { token.cancelled().await })
            .await
    });

    // Hold the UI's long-lived event stream open, exactly like a browser does.
    let client = reqwest::Client::new();
    let mut resp = client
        .get(format!("http://{addr}/api/events"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let first = tokio::time::timeout(Duration::from_secs(5), resp.chunk())
        .await
        .expect("SSE response should start streaming")
        .expect("could not read the first SSE chunk")
        .expect("SSE stream should not be empty");
    assert!(
        String::from_utf8_lossy(&first).contains("status"),
        "expected the initial status event, got {:?}",
        String::from_utf8_lossy(&first)
    );

    // A graceful shutdown must not wait for the SSE client to disconnect.
    state.shutdown.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("web server still waiting for the SSE connection after shutdown")
        .expect("web server task panicked");
    assert!(result.is_ok(), "web server error: {result:?}");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tls_rejection_is_recorded_and_learns_passthrough() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Origin presents a certificate the client trusts. The MitmRs local CA is
    // deliberately *not* trusted, which is what a pinned / custom-trust-store
    // client does.
    let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let origin_pem = certified.cert.pem();
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
    let origin_port = listener.local_addr().unwrap().port();
    let origin_task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut buf = [0u8; 4096];
                let n = tls.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let body = "passthrough works";
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });

    let (state, _dir) = make_state("tls-passthrough").await;
    mitmrs::proxy::start(state.clone()).await.unwrap();
    let proxy_port = state.config.read().await.proxy_port;
    let url = format!("https://127.0.0.1:{origin_port}/stream");

    let make_client = || {
        let origin_cert = reqwest::Certificate::from_pem(origin_pem.as_bytes()).unwrap();
        reqwest::Client::builder()
            .add_root_certificate(origin_cert)
            .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{proxy_port}")).unwrap())
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
    };

    let first = make_client().get(&url).send().await;
    assert!(
        first.is_err(),
        "the first request must fail: the client does not trust the MitmRs CA"
    );

    let mut failed = None;
    for _ in 0..100 {
        let query = mitmrs::model::SearchQuery {
            tag: Some("tls-failed".into()),
            ..Default::default()
        };
        if let Some(flow) = state
            .store
            .search(&query)
            .await
            .unwrap()
            .items
            .first()
            .cloned()
        {
            failed = Some(flow);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let failed = failed.expect("a TLS handshake failure should be recorded");
    assert_eq!(failed.protocol, "tls");
    assert_eq!(failed.host, "127.0.0.1");
    assert!(
        failed
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("alert"),
        "error should carry the client alert: {:?}",
        failed.error
    );
    assert!(
        state.is_passthrough("127.0.0.1"),
        "host should be learned for passthrough"
    );

    // The next connection is tunneled to the origin with the real certificate.
    let body = make_client()
        .get(&url)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "passthrough works");

    // Passthrough cannot show plaintext, but the wire counters must be recorded
    // so undecryptable hosts are still observable.
    let mut tunnel = None;
    for _ in 0..100 {
        let flows = state
            .store
            .search(&mitmrs::model::SearchQuery {
                tag: Some("tls-passthrough".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        if let Some(item) = flows.items.first() {
            if item.request_size > 0 && item.response_size > 0 && item.duration_ms.is_some() {
                tunnel = Some(item.clone());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let tunnel = tunnel.expect("passthrough traffic should be counted");
    assert!(tunnel.request_size > 0);
    assert!(
        tunnel.response_size > 17,
        "TLS handshake + response bytes should be counted, got {}",
        tunnel.response_size
    );

    mitmrs::proxy::stop(&state).await.unwrap();
    origin_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sse_body_is_visible_while_the_stream_is_open() {
    use axum::body::Body;
    use axum::response::Response as AxumResponse;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let upstream_port = listener.local_addr().unwrap().port();
    let app = Router::new().route(
        "/events",
        get(|| async {
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(8);
            tokio::spawn(async move {
                for i in 0..12 {
                    let chunk = format!("data: {{\"i\":{i},\"text\":\"{}\"}}\n\n", "x".repeat(200));
                    if tx.send(Ok(bytes::Bytes::from(chunk))).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            });
            AxumResponse::builder()
                .status(200)
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(
                    tokio_stream::wrappers::ReceiverStream::new(rx),
                ))
                .unwrap()
        }),
    );
    let upstream_task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let (state, _dir) = make_state("sse").await;
    mitmrs::proxy::start(state.clone()).await.unwrap();
    let proxy_port = state.config.read().await.proxy_port;

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{proxy_port}")).unwrap())
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let mut resp = client
        .get(format!("http://127.0.0.1:{upstream_port}/events"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let mut received = Vec::new();
    for _ in 0..4 {
        let chunk = resp.chunk().await.unwrap().expect("stream chunk");
        received.extend_from_slice(&chunk);
    }
    assert!(!received.is_empty());
    assert!(String::from_utf8_lossy(&received).contains("data:"));

    // While the stream is still open, the captured body must already be queryable.
    let mut partial = None;
    for _ in 0..40 {
        let flows = state.store.search(&Default::default()).await.unwrap();
        if let Some(item) = flows.items.first() {
            let detail = state
                .store
                .get(&item.id, "text", 1_000_000)
                .await
                .unwrap()
                .unwrap();
            if detail.response.body_size > 0 {
                partial = Some(detail);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let partial = partial.expect("SSE body should be persisted before the stream ends");
    assert_eq!(partial.summary.protocol, "sse");
    assert_eq!(
        partial.response.content_type.as_deref(),
        Some("text/event-stream")
    );
    assert!(partial
        .response
        .body_text
        .unwrap_or_default()
        .contains("data:"));
    assert!(!partial.response.truncated);

    // Drain the stream and verify the final body plus end time are committed.
    while let Ok(Some(_)) = resp.chunk().await {}
    let flow_id = partial.summary.id;
    let mut finished = None;
    for _ in 0..100 {
        if let Some(detail) = state.store.get(&flow_id, "text", 1_000_000).await.unwrap() {
            if detail.summary.duration_ms.is_some() {
                finished = Some(detail);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let finished = finished.expect("stream should be finalized");
    let final_body = finished.response.body_text.unwrap_or_default();
    assert!(
        final_body.contains("\"i\":11"),
        "final body should contain the last event, got {} bytes",
        final_body.len()
    );

    mitmrs::proxy::stop(&state).await.unwrap();
    upstream_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_upgrade_is_tunneled_and_frames_are_captured() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let origin_port = listener.local_addr().unwrap().port();
    let origin_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                return;
            }
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&head).to_lowercase();
        assert!(
            text.starts_with("get /ws?x=1 http/1.1"),
            "upstream should receive an origin-form request line, got: {text}"
        );
        assert!(
            text.contains("upgrade: websocket"),
            "upgrade header should be forwarded: {text}"
        );
        assert!(
            !text.contains("sec-websocket-extensions"),
            "extension negotiation should be stripped: {text}"
        );
        socket
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: dummy\r\n\r\n",
            )
            .await
            .unwrap();
        // Read the client's masked text frame, answer with an unmasked one.
        let mut frame = [0u8; 64];
        let n = socket.read(&mut frame).await.unwrap_or(0);
        assert!(n >= 2, "expected a websocket frame from the proxy");
        socket.write_all(b"\x81\x05world").await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let _ = socket.shutdown().await;
    });

    let (state, _dir) = make_state("websocket").await;
    mitmrs::proxy::start(state.clone()).await.unwrap();
    let proxy_port = state.config.read().await.proxy_port;

    let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    let handshake = format!(
        "GET http://127.0.0.1:{origin_port}/ws?x=1 HTTP/1.1\r\n\
         Host: 127.0.0.1:{origin_port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Extensions: permessage-deflate\r\n\r\n"
    );
    client.write_all(handshake.as_bytes()).await.unwrap();

    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = client.read(&mut byte).await.unwrap();
        assert_ne!(n, 0, "proxy closed the connection before the 101 response");
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head_text = String::from_utf8_lossy(&head).to_lowercase();
    assert!(head_text.starts_with("http/1.1 101"), "{head_text}");
    assert!(head_text.contains("upgrade: websocket"), "{head_text}");
    assert!(
        head_text.contains("sec-websocket-accept: dummy"),
        "{head_text}"
    );

    let mask = [1u8, 2, 3, 4];
    let payload = b"hello";
    let mut frame = vec![0x81u8, 0x80 | payload.len() as u8];
    frame.extend_from_slice(&mask);
    for (index, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[index % 4]);
    }
    client.write_all(&frame).await.unwrap();

    let mut reply = [0u8; 16];
    let n = client.read(&mut reply).await.unwrap();
    assert_eq!(&reply[..n], b"\x81\x05world");
    let _ = client.shutdown().await;

    let mut flow_id = None;
    for _ in 0..100 {
        let flows = state.store.search(&Default::default()).await.unwrap();
        if let Some(flow) = flows.items.first() {
            if flow.status == Some(101) {
                flow_id = Some(flow.id.clone());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let flow_id = flow_id.expect("the upgraded flow should be recorded");

    let mut record = None;
    for _ in 0..100 {
        if let Some(rec) = state.store.get_record(&flow_id).await.unwrap() {
            if rec.duration_ms.is_some() && rec.request_body_size > 0 && rec.response_body_size > 0
            {
                record = Some(rec);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let rec = record.expect("websocket frames should be captured");
    assert_eq!(rec.protocol, "websocket");
    assert_eq!(rec.request_body, frame);
    assert_eq!(rec.response_body, b"\x81\x05world");

    let decoded =
        mitmrs::decode::decode_record(&rec, "request", "websocket", &json!({}), 1_000_000).unwrap();
    assert!(
        decoded.text.unwrap_or_default().contains("hello"),
        "client frame should decode to text"
    );
    let decoded =
        mitmrs::decode::decode_record(&rec, "response", "websocket", &json!({}), 1_000_000)
            .unwrap();
    assert!(
        decoded.text.unwrap_or_default().contains("world"),
        "server frame should decode to text"
    );
    let auto =
        mitmrs::decode::decode_record(&rec, "request", "auto", &json!({}), 1_000_000).unwrap();
    assert_eq!(
        auto.format, "websocket",
        "auto decode should recognize upgraded flows"
    );

    mitmrs::proxy::stop(&state).await.unwrap();
    origin_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_handshake_rejection_is_relayed_as_http() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let origin_port = listener.local_addr().unwrap().port();
    let origin_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;
        let _ = socket
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\ncontent-type: text/plain\r\ncontent-length: 4\r\nconnection: close\r\n\r\nnope",
            )
            .await;
        let _ = socket.shutdown().await;
    });

    let (state, _dir) = make_state("ws-reject").await;
    mitmrs::proxy::start(state.clone()).await.unwrap();
    let proxy_port = state.config.read().await.proxy_port;

    let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    let handshake = format!(
        "GET http://127.0.0.1:{origin_port}/ws HTTP/1.1\r\n\
         Host: 127.0.0.1:{origin_port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    client.write_all(handshake.as_bytes()).await.unwrap();

    let mut response = Vec::new();
    let mut buf = [0u8; 1024];
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_millis(500), client.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => {
                response.extend_from_slice(&buf[..n]);
                if response.ends_with(b"nope") {
                    break;
                }
            }
            _ => break,
        }
    }
    let text = String::from_utf8_lossy(&response).to_lowercase();
    assert!(text.starts_with("http/1.1 401"), "{text}");
    assert!(text.ends_with("nope"), "{text}");

    let mut flow = None;
    for _ in 0..100 {
        let flows = state.store.search(&Default::default()).await.unwrap();
        if let Some(item) = flows.items.first() {
            // The row appears before the body is streamed, so wait for the body
            // as well; otherwise this races the drain task.
            if item.status == Some(401) && item.response_size == 4 {
                flow = Some(item.clone());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let flow = flow.expect("the rejected handshake should be recorded");
    assert_eq!(flow.protocol, "http");
    assert_eq!(flow.response_size, 4);
    assert!(flow
        .tags
        .iter()
        .any(|tag| tag == "websocket-handshake-rejected"));

    mitmrs::proxy::stop(&state).await.unwrap();
    origin_task.abort();
}

fn config_patch(enabled: Option<bool>, url: Option<String>) -> ConfigPatch {
    ConfigPatch {
        port: None,
        max_body_bytes: None,
        capture_enabled: None,
        upstream_proxy_enabled: enabled,
        upstream_proxy_url: url,
    }
}

async fn wait_for_flow(state: &Arc<AppState>) -> mitmrs::model::FlowSummary {
    for _ in 0..100 {
        let flows = state.store.search(&Default::default()).await.unwrap();
        if let Some(f) = flows.items.first() {
            return f.clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no captured flow");
}

/// Minimal forwarding proxy: records the absolute-form request line and answers
/// it directly, so the test proves traffic went through the next hop.
async fn spawn_forward_proxy() -> (u16, tokio::sync::mpsc::Receiver<String>, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                if let Some(line) = text.lines().next() {
                    let _ = tx.send(line.to_string()).await;
                }
                let body = "chained response";
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (port, rx, task)
}

/// Minimal CONNECT proxy: records the CONNECT line and tunnels to a fixed local
/// origin, which stands in for the real remote server.
async fn spawn_connect_proxy(
    origin_port: u16,
) -> (u16, tokio::sync::mpsc::Receiver<String>, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else {
                break;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                loop {
                    match client.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    head.push(byte[0]);
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    if head.len() > 16 * 1024 {
                        return;
                    }
                }
                let text = String::from_utf8_lossy(&head).to_string();
                let first = text.lines().next().unwrap_or_default().to_string();
                let _ = tx.send(first).await;

                let mut origin =
                    match tokio::net::TcpStream::connect(("127.0.0.1", origin_port)).await {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                let _ = client
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await;
                let _ = tokio::io::copy_bidirectional(&mut client, &mut origin).await;
            });
        }
    });
    (port, rx, task)
}

/// Local TLS origin with a self-signed certificate. The MITM upstream client is
/// created with `insecure=true` in tests, so the certificate is not verified.
async fn spawn_tls_origin() -> (u16, JoinHandle<()>) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certified = rcgen::generate_simple_self_signed(vec!["origin.test".to_string()]).unwrap();
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
                let mut buf = [0u8; 4096];
                let n = tls.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let body = "secure via chain";
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (port, task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chain_http_next_hop_is_used_and_address_is_remembered() {
    let (chain_port, mut chain_lines, chain_task) = spawn_forward_proxy().await;
    let (state, _dir) = make_state("chain-http").await;
    mitmrs::proxy::start(state.clone()).await.unwrap();
    let proxy_port = state.config.read().await.proxy_port;

    let chain_url = format!("127.0.0.1:{chain_port}");
    let status =
        mitmrs::control::apply_config(&state, config_patch(Some(true), Some(chain_url.clone())))
            .await
            .unwrap();
    assert!(status.config.upstream_proxy_enabled);
    assert_eq!(status.config.upstream_proxy_url, chain_url);

    let saved = mitmrs::config::load_file_config(&state.data_dir);
    assert!(saved.upstream_proxy_enabled);
    assert_eq!(saved.upstream_proxy_url, chain_url);

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{proxy_port}")).unwrap())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let body = client
        .get("http://chained.example/probe?q=1")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "chained response");

    let first = tokio::time::timeout(Duration::from_secs(5), chain_lines.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first, "GET http://chained.example/probe?q=1 HTTP/1.1");

    let flow = wait_for_flow(&state).await;
    assert_eq!(flow.host, "chained.example");
    assert_eq!(flow.path, "/probe?q=1");
    assert_eq!(flow.status, Some(200));

    // Unchecking keeps the last address for the next launch.
    let status = mitmrs::control::apply_config(&state, config_patch(Some(false), None))
        .await
        .unwrap();
    assert!(!status.config.upstream_proxy_enabled);
    assert_eq!(status.config.upstream_proxy_url, chain_url);
    let saved = mitmrs::config::load_file_config(&state.data_dir);
    assert!(!saved.upstream_proxy_enabled);
    assert_eq!(saved.upstream_proxy_url, chain_url);

    // Unsupported schemes are rejected without touching the active config.
    let err = mitmrs::control::apply_config(
        &state,
        config_patch(Some(true), Some("socks5://127.0.0.1:1080".into())),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("only http://"), "{err}");
    let cfg = state.config.read().await.clone();
    assert!(!cfg.upstream_proxy_enabled);
    assert_eq!(cfg.upstream_proxy_url, chain_url);

    mitmrs::proxy::stop(&state).await.unwrap();
    chain_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chain_https_uses_connect_tunnel_through_next_hop() {
    let (origin_port, origin_task) = spawn_tls_origin().await;
    let (chain_port, mut chain_lines, chain_task) = spawn_connect_proxy(origin_port).await;
    let (state, _dir) = make_state("chain-https").await;
    mitmrs::proxy::start(state.clone()).await.unwrap();
    let proxy_port = state.config.read().await.proxy_port;
    mitmrs::control::apply_config(
        &state,
        config_patch(Some(true), Some(format!("127.0.0.1:{chain_port}"))),
    )
    .await
    .unwrap();

    let ca = reqwest::Certificate::from_pem(state.ca.pem().as_bytes()).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(ca)
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{proxy_port}")).unwrap())
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let body = client
        .get("https://origin.test/secure")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "secure via chain");

    let first = tokio::time::timeout(Duration::from_secs(5), chain_lines.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first, "CONNECT origin.test:443 HTTP/1.1");

    let flow = wait_for_flow(&state).await;
    assert_eq!(flow.host, "origin.test");
    assert_eq!(flow.scheme, "https");
    assert_eq!(flow.status, Some(200));

    mitmrs::proxy::stop(&state).await.unwrap();
    origin_task.abort();
    chain_task.abort();
}
