use super::io::{peek_client_hello, read_client_hello, read_first_byte, PrefixedStream};
use super::service::{self, ConnMeta, ProxyBody};
use crate::decode::formats::ClientHelloInfo;
use crate::model::{FlowRecord, HeaderEntry};
use crate::state::AppState;
use crate::transparent::TransparentTarget;
use chrono::Utc;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

fn make_service(
    state: Arc<AppState>,
    meta: ConnMeta,
) -> impl hyper::service::Service<
    Request<Incoming>,
    Response = hyper::Response<ProxyBody>,
    Error = Infallible,
    Future = impl std::future::Future<Output = Result<hyper::Response<ProxyBody>, Infallible>> + Send,
> + Clone
       + Send
       + 'static {
    service_fn(move |req: Request<Incoming>| {
        let state = state.clone();
        let meta = meta.clone();
        async move { Ok(service::handle_request(state, req, meta).await) }
    })
}

pub async fn serve_explicit(
    state: Arc<AppState>,
    stream: TcpStream,
    peer: std::net::SocketAddr,
    cancel: CancellationToken,
) {
    let meta = ConnMeta {
        scheme: "http".into(),
        host: String::new(),
        port: 80,
        tls_sni: None,
        client: peer,
        transparent: false,
        mirror: false,
        cancel: cancel.clone(),
    };
    let svc = make_service(state, meta);
    let conn = hyper::server::conn::http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .serve_connection(TokioIo::new(stream), svc)
        .with_upgrades();
    tokio::pin!(conn);
    tokio::select! {
        _ = cancel.cancelled() => {}
        result = &mut conn => {
            if let Err(e) = result {
                let msg = e.to_string();
                if !msg.contains("connection closed") && !msg.contains("reset") {
                    tracing::debug!("explicit proxy connection ended: {e}");
                }
            }
        }
    }
}

pub async fn serve_transparent(
    state: Arc<AppState>,
    stream: TcpStream,
    peer: std::net::SocketAddr,
    target: TransparentTarget,
    cancel: CancellationToken,
) {
    let mut first = [0u8; 1];
    let n = match tokio::time::timeout(Duration::from_secs(10), stream.peek(&mut first)).await {
        Ok(Ok(n)) => n,
        _ => return,
    };
    if n == 0 {
        return;
    }
    let is_tls = first[0] == 0x16;
    if is_tls {
        let started = Instant::now();
        let hello = peek_client_hello(&stream).await.unwrap_or_default();
        let sni = hello.sni.clone();
        // The leaf must match what the client verifies (the SNI). When there is
        // no SNI (rare, IP destinations), fall back to the original destination.
        let fallback_host = target.original_dst.ip().to_string();
        let host = sni.clone().unwrap_or_else(|| fallback_host.clone());
        let meta = ConnMeta {
            scheme: "https".into(),
            host: host.clone(),
            port: target.original_dst.port(),
            tls_sni: sni,
            client: target.client,
            transparent: true,
            mirror: false,
            cancel: cancel.clone(),
        };
        if state.is_passthrough(&host) {
            tracing::debug!("transparent TLS passthrough (learned) for {host}");
            tunnel(
                state,
                stream,
                Vec::new(),
                fallback_host,
                target.original_dst.port(),
                meta,
                hello,
                cancel,
            )
            .await;
            return;
        }
        let config = match state.ca.server_config_for(&host) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "transparent cert generation failed for {}: {e:#}",
                    meta.host
                );
                return;
            }
        };
        let acceptor = TlsAcceptor::from(config);
        match tokio::time::timeout(Duration::from_secs(15), acceptor.accept(stream)).await {
            Ok(Ok(tls)) => serve_mitm_io(state, tls, meta, cancel).await,
            Ok(Err(e)) => handle_handshake_error(&state, &meta, &hello, &e, started).await,
            Err(_) => {
                tracing::debug!("transparent TLS handshake timed out for {}", meta.host);
            }
        }
    } else {
        let meta = ConnMeta {
            scheme: "http".into(),
            host: target.original_dst.ip().to_string(),
            port: target.original_dst.port(),
            tls_sni: None,
            client: target.client,
            transparent: true,
            mirror: false,
            cancel: cancel.clone(),
        };
        let svc = make_service(state, meta);
        // Use the auto server so transparent h2c (gRPC prior knowledge) works
        // as well as HTTP/1.1.
        let builder = auto::Builder::new(TokioExecutor::new());
        let conn = builder.serve_connection_with_upgrades(TokioIo::new(stream), svc);
        tokio::pin!(conn);
        tokio::select! {
            _ = cancel.cancelled() => {}
            result = &mut conn => {
                if let Err(e) = result {
                    tracing::debug!("transparent HTTP connection ended: {e}");
                }
            }
        }
        let _ = peer;
    }
}

pub async fn serve_mitm_connect(
    state: Arc<AppState>,
    on_upgrade: hyper::upgrade::OnUpgrade,
    host: String,
    port: u16,
    client: std::net::SocketAddr,
    cancel: CancellationToken,
) {
    let started = Instant::now();
    let upgraded = match on_upgrade.await {
        Ok(u) => u,
        Err(e) => {
            tracing::debug!("CONNECT upgrade failed: {e}");
            return;
        }
    };
    let mut io = TokioIo::new(upgraded);
    let first = match read_first_byte(&mut io, Duration::from_secs(15)).await {
        Ok(Some(b)) => b,
        _ => return,
    };
    if first == 0x16 {
        // Read (and keep) the ClientHello so we can both sign a certificate for
        // the SNI the client actually verifies and replay it into the acceptor.
        let peek = read_client_hello(&mut io, first, Duration::from_secs(3)).await;
        let hello = peek.info.clone().unwrap_or_default();
        let sni = hello.sni.clone();
        let cert_host = sni.clone().unwrap_or_else(|| host.clone());
        let meta = ConnMeta {
            scheme: "https".into(),
            host: host.clone(),
            port,
            tls_sni: sni,
            client,
            transparent: false,
            mirror: false,
            cancel: cancel.clone(),
        };
        if state.is_passthrough(&host) || state.is_passthrough(&cert_host) {
            tracing::debug!("CONNECT passthrough (learned) for {host} (sni={cert_host})");
            tunnel(
                state,
                io,
                peek.prefix,
                host.clone(),
                port,
                meta,
                hello,
                cancel,
            )
            .await;
            return;
        }
        let config = match state.ca.server_config_for(&cert_host) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("certificate generation failed for {cert_host}: {e:#}");
                return;
            }
        };
        let acceptor = TlsAcceptor::from(config);
        match tokio::time::timeout(
            Duration::from_secs(15),
            acceptor.accept(PrefixedStream::new(peek.prefix, io)),
        )
        .await
        {
            Ok(Ok(tls)) => serve_mitm_io(state, tls, meta, cancel).await,
            Ok(Err(e)) => handle_handshake_error(&state, &meta, &hello, &e, started).await,
            Err(_) => {
                tracing::debug!(
                    "MITM TLS handshake timed out for {}:{}",
                    meta.host,
                    meta.port
                );
            }
        }
    } else {
        // CONNECT to a non-TLS service. Tunnel it raw, preserving the first byte.
        // When a next-hop proxy is configured, open the tunnel through it too.
        let mut upstream = match upstream_stream(&state, &host, port).await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("CONNECT tunnel to {host}:{port} failed: {e}");
                return;
            }
        };
        let mut client_io = PrefixedStream::new(vec![first], io);
        tokio::select! {
            _ = cancel.cancelled() => {}
            _ = tokio::io::copy_bidirectional(&mut client_io, &mut upstream) => {}
        }
        let _ = client_io.shutdown().await;
    }
}

async fn serve_mitm_io<I>(state: Arc<AppState>, io: I, meta: ConnMeta, cancel: CancellationToken)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let svc = make_service(state, meta);
    let builder = auto::Builder::new(TokioExecutor::new());
    let conn = builder.serve_connection_with_upgrades(TokioIo::new(io), svc);
    tokio::pin!(conn);
    tokio::select! {
        _ = cancel.cancelled() => {}
        result = &mut conn => {
            if let Err(e) = result {
                let msg = e.to_string();
                if !msg.contains("connection closed") && !msg.contains("reset") {
                    tracing::debug!("MITM connection ended: {e}");
                }
            }
        }
    }
}

/// Open a plain TCP connection to `host:port`, honoring the optional next-hop
/// proxy. Shared by raw CONNECT tunnels and TLS passthrough.
async fn upstream_stream(
    state: &AppState,
    host: &str,
    port: u16,
) -> Result<TcpStream, crate::upstream::BoxError> {
    let chain = {
        let cfg = state.config.read().await;
        match cfg.upstream_chain() {
            Ok(chain) => chain,
            Err(e) => {
                tracing::warn!("ignoring invalid upstream chain proxy: {e}");
                None
            }
        }
    };
    match chain {
        Some(chain) => crate::upstream::connect_via_chain(&chain, host, port).await,
        None => TcpStream::connect((host, port)).await.map_err(Into::into),
    }
}

/// Tunnel TLS bytes to the origin without decrypting them. `prefix` holds the
/// ClientHello we already consumed from the client and must be replayed to the
/// server first.
#[allow(clippy::too_many_arguments)]
async fn tunnel<I>(
    state: Arc<AppState>,
    client_io: I,
    prefix: Vec<u8>,
    upstream_host: String,
    upstream_port: u16,
    meta: ConnMeta,
    hello: ClientHelloInfo,
    cancel: CancellationToken,
) where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let started = Instant::now();
    let flow_id = record_passthrough_flow(&state, &meta, &hello).await;
    let mut upstream = match upstream_stream(&state, &upstream_host, upstream_port).await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(
                "TLS passthrough tunnel to {upstream_host}:{upstream_port} failed: {e}"
            );
            if let Some(id) = flow_id {
                state
                    .record_ended(&id, Utc::now(), started.elapsed().as_millis() as i64)
                    .await;
            }
            return;
        }
    };
    if !prefix.is_empty() {
        if let Err(e) = upstream.write_all(&prefix).await {
            tracing::debug!("failed to replay ClientHello to {upstream_host}: {e}");
            if let Some(id) = flow_id {
                state
                    .record_ended(&id, Utc::now(), started.elapsed().as_millis() as i64)
                    .await;
            }
            return;
        }
    }
    // The prefix was consumed from the client and replayed above, so the tunnel
    // continues from the live socket only. Wrapping it in a PrefixedStream here
    // would send the ClientHello upstream twice (the server answers with
    // BadRecordMac).
    //
    // Count both directions: a passthrough flow cannot show plaintext, but the
    // wire volume still tells you whether this host is actually streaming.
    let (client_reader, client_writer) = tokio::io::split(client_io);
    let (server_reader, server_writer) = tokio::io::split(upstream);
    let client_to_server = Arc::new(AtomicU64::new(prefix.len() as u64));
    let server_to_client = Arc::new(AtomicU64::new(0));
    let pumps = {
        let c2s = client_to_server.clone();
        let s2c = server_to_client.clone();
        async move {
            tokio::join!(
                pump_counted(client_reader, server_writer, c2s),
                pump_counted(server_reader, client_writer, s2c),
            )
        }
    };
    tokio::pin!(pumps);
    let mut ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(2),
        Duration::from_secs(2),
    );
    let mut last_request = 0u64;
    let mut last_response = 0u64;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = &mut pumps => break,
            _ = ticker.tick() => {
                if let Some(id) = flow_id.as_deref() {
                    let request = client_to_server.load(Ordering::Relaxed);
                    let response = server_to_client.load(Ordering::Relaxed);
                    if request != last_request || response != last_response {
                        last_request = request;
                        last_response = response;
                        state.record_tunnel_stats(id, request, response, None, None).await;
                    }
                }
            }
        }
    }
    if let Some(id) = flow_id.as_deref() {
        state
            .record_tunnel_stats(
                id,
                client_to_server.load(Ordering::Relaxed),
                server_to_client.load(Ordering::Relaxed),
                Some(Utc::now()),
                Some(started.elapsed().as_millis() as i64),
            )
            .await;
    }
}

/// Copy one direction of a passthrough tunnel and count the bytes forwarded.
async fn pump_counted<R, W>(mut reader: R, mut writer: W, counter: Arc<AtomicU64>)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                counter.fetch_add(n as u64, Ordering::Relaxed);
                if writer.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = writer.shutdown().await;
}

/// Persist a metadata-only flow for a tunneled TLS connection so passthrough
/// traffic is visible instead of silently disappearing. Returns the flow id.
async fn record_passthrough_flow(
    state: &AppState,
    meta: &ConnMeta,
    hello: &ClientHelloInfo,
) -> Option<String> {
    let flow_id = uuid::Uuid::now_v7().to_string();
    let mut rec = FlowRecord::new(
        flow_id.clone(),
        Utc::now(),
        meta.client.to_string(),
        "https".into(),
        meta.host.clone(),
        meta.port,
        "CONNECT".into(),
        "/".into(),
        None,
        "TLS".into(),
        hello.sni.clone().or_else(|| meta.tls_sni.clone()),
    );
    rec.protocol = "tls".into();
    rec.tags = vec!["tls-passthrough".into()];
    if meta.transparent {
        rec.tags.push("transparent".into());
    }
    rec.request_headers = client_hello_headers(hello);
    rec.error =
        Some("TLS passthrough: not decrypted (client pinning or untrusted local CA)".into());
    state.record_new(rec).await.map(|_| flow_id)
}

#[derive(Debug, Clone)]
struct HandshakeFailure {
    alert: Option<String>,
    detail: String,
    client_rejected_cert: bool,
    learn: bool,
    io_error: bool,
}

/// Classify a failed `TlsAcceptor::accept`. tokio-rustls wraps rustls errors in
/// `io::Error`, so we downcast to recover the alert the client sent. Alerts such
/// as `UnknownCA` or `CertificateUnknown` mean the client refused our
/// certificate (untrusted local CA or pinning) and are safe to learn from.
fn classify_handshake_failure(err: &std::io::Error) -> HandshakeFailure {
    let Some(rustls_err) = err
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
    else {
        return HandshakeFailure {
            alert: None,
            detail: err.to_string(),
            client_rejected_cert: false,
            learn: false,
            io_error: true,
        };
    };
    let detail = rustls_err.to_string();
    if let rustls::Error::AlertReceived(alert) = rustls_err {
        let alert_name = format!("{alert:?}");
        let client_rejected_cert = is_certificate_rejection(*alert);
        return HandshakeFailure {
            alert: Some(alert_name),
            detail,
            client_rejected_cert,
            learn: client_rejected_cert,
            io_error: false,
        };
    }
    HandshakeFailure {
        alert: None,
        detail,
        client_rejected_cert: false,
        learn: false,
        io_error: false,
    }
}

fn is_certificate_rejection(alert: rustls::AlertDescription) -> bool {
    matches!(
        alert,
        rustls::AlertDescription::UnknownCA
            | rustls::AlertDescription::BadCertificate
            | rustls::AlertDescription::UnsupportedCertificate
            | rustls::AlertDescription::CertificateRevoked
            | rustls::AlertDescription::CertificateExpired
            | rustls::AlertDescription::CertificateUnknown
            | rustls::AlertDescription::CertificateRequired
            | rustls::AlertDescription::HandshakeFailure
    )
}

/// Record the failure and, when the alert says the client rejected our
/// certificate, remember the host so the next connection is tunneled untouched.
async fn handle_handshake_error(
    state: &Arc<AppState>,
    meta: &ConnMeta,
    hello: &ClientHelloInfo,
    error: &std::io::Error,
    started: Instant,
) {
    let failure = classify_handshake_failure(error);
    if failure.io_error {
        tracing::debug!(
            "TLS handshake aborted for {}:{}: {}",
            meta.host,
            meta.port,
            failure.detail
        );
        return;
    }
    let key = hello.sni.as_deref().unwrap_or(&meta.host).to_string();
    let note = state.note_tls_failure(&key, &failure.detail, failure.learn);
    tracing::warn!(
        "TLS handshake failed for {}:{} sni={} alert={} passthrough={}",
        meta.host,
        meta.port,
        key,
        failure.alert.as_deref().unwrap_or("-"),
        note.learned
    );
    if note.duplicate {
        return;
    }
    let rec = handshake_failure_record(meta, hello, &failure, note.learned, started);
    state.record_new(rec).await;
}

fn handshake_failure_record(
    meta: &ConnMeta,
    hello: &ClientHelloInfo,
    failure: &HandshakeFailure,
    learned: bool,
    started: Instant,
) -> FlowRecord {
    let mut rec = FlowRecord::new(
        uuid::Uuid::now_v7().to_string(),
        Utc::now(),
        meta.client.to_string(),
        "https".into(),
        meta.host.clone(),
        meta.port,
        "CONNECT".into(),
        "/".into(),
        None,
        "TLS".into(),
        hello.sni.clone().or_else(|| meta.tls_sni.clone()),
    );
    rec.protocol = "tls".into();
    rec.tags = vec!["tls-failed".into()];
    if failure.client_rejected_cert {
        rec.tags.push("client-rejected-cert".into());
    }
    if learned {
        rec.tags.push("passthrough".into());
    }
    if meta.transparent {
        rec.tags.push("transparent".into());
    }
    rec.request_headers = client_hello_headers(hello);
    rec.error = Some(match &failure.alert {
        Some(alert) => format!("TLS handshake failed: alert {alert} ({})", failure.detail),
        None => format!("TLS handshake failed: {}", failure.detail),
    });
    rec.ended_at = Some(Utc::now());
    rec.duration_ms = Some(started.elapsed().as_millis() as i64);
    rec
}

fn client_hello_headers(hello: &ClientHelloInfo) -> Vec<HeaderEntry> {
    let mut headers = Vec::new();
    if let Some(sni) = &hello.sni {
        headers.push(HeaderEntry::new("x-mitmrs-sni", sni.clone()));
    }
    if !hello.alpn.is_empty() {
        headers.push(HeaderEntry::new("x-mitmrs-alpn", hello.alpn_text()));
    }
    if !hello.tls_versions.is_empty() {
        headers.push(HeaderEntry::new(
            "x-mitmrs-tls-versions",
            hello.versions_text(),
        ));
    }
    headers.push(HeaderEntry::new(
        "x-mitmrs-hello-ciphers",
        hello.cipher_count.to_string(),
    ));
    headers
}

#[cfg(test)]
mod tests {
    use super::{classify_handshake_failure, is_certificate_rejection};
    use rustls::AlertDescription;

    #[test]
    fn certificate_rejection_alerts_are_learned() {
        assert!(is_certificate_rejection(AlertDescription::UnknownCA));
        assert!(is_certificate_rejection(AlertDescription::BadCertificate));
        assert!(is_certificate_rejection(
            AlertDescription::CertificateUnknown
        ));
        assert!(!is_certificate_rejection(AlertDescription::CloseNotify));
        assert!(!is_certificate_rejection(AlertDescription::UserCanceled));
    }

    #[test]
    fn classifies_wrapped_rustls_alerts() {
        let err = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::AlertReceived(AlertDescription::UnknownCA),
        );
        let failure = classify_handshake_failure(&err);
        assert!(!failure.io_error);
        assert!(failure.client_rejected_cert);
        assert!(failure.learn);
        assert_eq!(failure.alert.as_deref(), Some("UnknownCA"));
    }

    #[test]
    fn plain_io_errors_are_not_recorded() {
        let err = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof");
        let failure = classify_handshake_failure(&err);
        assert!(failure.io_error);
        assert!(!failure.learn);
        assert!(failure.alert.is_none());
    }
}
