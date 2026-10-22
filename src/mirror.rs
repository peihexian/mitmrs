//! Upstream mirror: a plaintext reverse proxy for clients that cannot be
//! MITM'd.
//!
//! Some clients bundle their own TLS roots — Rust `reqwest` built with the
//! `rustls-tls` feature only trusts webpki roots and ignores the Windows store,
//! so they reject the MitmRs CA no matter what is installed. Their upstream base
//! URL is usually configurable though: point it at a mirror listener and MitmRs
//! terminates plaintext locally, forwards to the real origin over TLS, and
//! records the exchange exactly like a proxied flow (SSE streaming included).

use crate::proxy::service::{self, ConnMeta};
use crate::proxy::ConnGuard;
use crate::state::AppState;
use anyhow::{Context, Result};
use http::header::HOST;
use http::{HeaderValue, StatusCode, Uri};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use url::Url;

/// One `--mirror LISTEN=UPSTREAM` mapping.
#[derive(Clone, Debug)]
pub struct MirrorSpec {
    pub listen: SocketAddr,
    pub upstream: Url,
}

impl MirrorSpec {
    /// Parses `127.0.0.1:8899=https://copilot.tencent.com`.
    pub fn parse(input: &str) -> Result<Self, String> {
        let (listen, upstream) = input.split_once('=').ok_or_else(|| {
            format!(
                "invalid mirror '{input}': expected LISTEN=UPSTREAM, for example \
                 127.0.0.1:8899=https://copilot.tencent.com"
            )
        })?;
        let listen: SocketAddr = listen.trim().parse().map_err(|e| {
            format!(
                "invalid mirror listen address '{}': {e} (use IP:PORT)",
                listen.trim()
            )
        })?;
        let upstream = Url::parse(upstream.trim())
            .map_err(|e| format!("invalid mirror upstream '{}': {e}", upstream.trim()))?;
        if upstream.scheme() != "http" && upstream.scheme() != "https" {
            return Err(format!(
                "unsupported mirror upstream scheme '{}': only http:// and https:// are accepted",
                upstream.scheme()
            ));
        }
        if upstream.host_str().is_none() {
            return Err(format!("mirror upstream '{}' has no host", upstream));
        }
        Ok(Self { listen, upstream })
    }

    fn upstream_authority(&self) -> String {
        let host = self.upstream.host_str().unwrap_or_default();
        match self.upstream.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        }
    }

    fn upstream_scheme(&self) -> String {
        self.upstream.scheme().to_string()
    }

    fn upstream_port(&self) -> u16 {
        self.upstream.port_or_known_default().unwrap_or(443)
    }
}

/// Binds the mirror listener and returns the address it actually listens on.
pub async fn start(state: Arc<AppState>, spec: MirrorSpec) -> Result<SocketAddr> {
    let listener = TcpListener::bind(spec.listen)
        .await
        .with_context(|| format!("bind mirror listener on {}", spec.listen))?;
    let addr = listener.local_addr()?;
    let cancel = state.shutdown.clone();
    let task_state = state.clone();
    let task_spec = spec.clone();
    tokio::spawn(async move {
        accept_loop(task_state, listener, task_spec, cancel).await;
    });
    tracing::info!("mirror listening on {addr} -> {}", spec.upstream);
    Ok(addr)
}

async fn accept_loop(
    state: Arc<AppState>,
    listener: TcpListener,
    spec: MirrorSpec,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            incoming = listener.accept() => {
                match incoming {
                    Ok((stream, peer)) => {
                        state.total_connections.fetch_add(1, Ordering::Relaxed);
                        state.active_connections.fetch_add(1, Ordering::Relaxed);
                        let state = state.clone();
                        let spec = spec.clone();
                        let cancel = cancel.clone();
                        tokio::spawn(async move {
                            let _guard = ConnGuard(state.clone());
                            serve(state, stream, peer, spec, cancel).await;
                        });
                    }
                    Err(e) => {
                        tracing::warn!("mirror accept error: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
    }
}

async fn serve(
    state: Arc<AppState>,
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    spec: MirrorSpec,
    cancel: CancellationToken,
) {
    let state_for_svc = state.clone();
    let spec_for_svc = spec.clone();
    let cancel_for_svc = cancel.clone();
    let svc = service_fn(move |mut req: Request<Incoming>| {
        let state = state_for_svc.clone();
        let spec = spec_for_svc.clone();
        let cancel = cancel_for_svc.clone();
        async move {
            if let Err(message) = rewrite_request(&mut req, &spec) {
                return Ok::<_, Infallible>(service::error_response(
                    StatusCode::BAD_GATEWAY,
                    &message,
                ));
            }
            let meta = ConnMeta {
                scheme: spec.upstream_scheme(),
                host: spec.upstream.host_str().unwrap_or_default().to_string(),
                port: spec.upstream_port(),
                tls_sni: spec.upstream.host_str().map(|host| host.to_string()),
                client: peer,
                transparent: false,
                mirror: true,
                cancel,
            };
            Ok(service::handle_request(state, req, meta).await)
        }
    });
    let builder = auto::Builder::new(TokioExecutor::new());
    let conn = builder.serve_connection_with_upgrades(TokioIo::new(stream), svc);
    tokio::pin!(conn);
    tokio::select! {
        _ = cancel.cancelled() => {}
        result = &mut conn => {
            if let Err(e) = result {
                let message = e.to_string();
                if !message.contains("connection closed") && !message.contains("reset") {
                    tracing::debug!("mirror connection ended: {e}");
                }
            }
        }
    }
}

/// Turns the origin-form request the client sent to the mirror into the
/// absolute URL of the real upstream, and fixes the `Host` header so the origin
/// sees its own name rather than `127.0.0.1:8899`.
fn rewrite_request(req: &mut Request<Incoming>, spec: &MirrorSpec) -> Result<(), String> {
    let path = req
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let base = spec.upstream.as_str().trim_end_matches('/');
    let absolute = format!("{base}{path}");
    let uri = Uri::from_str(&absolute)
        .map_err(|e| format!("build mirror upstream URI '{absolute}': {e}"))?;
    *req.uri_mut() = uri;

    let authority = spec.upstream_authority();
    let host = HeaderValue::from_str(&authority)
        .map_err(|e| format!("invalid mirror host header '{authority}': {e}"))?;
    req.headers_mut().insert(HOST, host);
    req.headers_mut().remove("proxy-connection");
    // Keep captured bodies readable: do not let the origin compress the
    // mirrored leg (SSE bodies would then be flushed through gzip and the
    // capture would be unreadable until the stream closes).
    req.headers_mut().remove("accept-encoding");
    Ok(())
}
