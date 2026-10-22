use super::io::{peek_tls_sni, read_first_byte, PrefixedStream};
use super::service::{self, ConnMeta, ProxyBody};
use crate::state::AppState;
use crate::transparent::TransparentTarget;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
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
        let sni = peek_tls_sni(&stream).await;
        let host = sni
            .clone()
            .unwrap_or_else(|| target.original_dst.ip().to_string());
        let meta = ConnMeta {
            scheme: "https".into(),
            host,
            port: target.original_dst.port(),
            tls_sni: sni,
            client: target.client,
            transparent: true,
            cancel: cancel.clone(),
        };
        let config = match state.ca.server_config_for(&meta.host) {
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
        let tls = match tokio::time::timeout(Duration::from_secs(15), acceptor.accept(stream)).await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                tracing::debug!("transparent TLS handshake failed for {}: {e}", meta.host);
                return;
            }
            Err(_) => {
                tracing::debug!("transparent TLS handshake timed out for {}", meta.host);
                return;
            }
        };
        serve_mitm_io(state, tls, meta, cancel).await;
    } else {
        let meta = ConnMeta {
            scheme: "http".into(),
            host: target.original_dst.ip().to_string(),
            port: target.original_dst.port(),
            tls_sni: None,
            client: target.client,
            transparent: true,
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
        let prefixed = PrefixedStream::new(vec![first], io);
        let meta = ConnMeta {
            scheme: "https".into(),
            host: host.clone(),
            port,
            tls_sni: Some(host.clone()),
            client,
            transparent: false,
            cancel: cancel.clone(),
        };
        let config = match state.ca.server_config_for(&host) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("certificate generation failed for {host}: {e:#}");
                return;
            }
        };
        let acceptor = TlsAcceptor::from(config);
        let tls =
            match tokio::time::timeout(Duration::from_secs(15), acceptor.accept(prefixed)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    tracing::debug!("MITM TLS handshake failed for {host}:{port}: {e}");
                    return;
                }
                Err(_) => {
                    tracing::debug!("MITM TLS handshake timed out for {host}:{port}");
                    return;
                }
            };
        serve_mitm_io(state, tls, meta, cancel).await;
    } else {
        // CONNECT to a non-TLS service. Tunnel it raw, preserving the first byte.
        // When a next-hop proxy is configured, open the tunnel through it too.
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
        let upstream: Result<TcpStream, crate::upstream::BoxError> = match chain {
            Some(chain) => crate::upstream::connect_via_chain(&chain, &host, port).await,
            None => TcpStream::connect((host.as_str(), port))
                .await
                .map_err(Into::into),
        };
        let upstream = match upstream {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("CONNECT tunnel to {host}:{port} failed: {e}");
                return;
            }
        };
        let mut client_io = PrefixedStream::new(vec![first], io);
        let mut upstream = upstream;
        let _ = tokio::io::copy_bidirectional(&mut client_io, &mut upstream).await;
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
