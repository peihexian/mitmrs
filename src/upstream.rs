use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::Uri;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpSocket, TcpStream};
use tokio_rustls::TlsConnector;
use tower::Service;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Source port range reserved for the proxy's own upstream sockets.
/// WinDivert's redirect filter excludes this range so outbound connections made
/// by MitmRs itself can never be redirected back into the proxy (a loop).
pub const UPSTREAM_PORT_MIN: u16 = 30000;
pub const UPSTREAM_PORT_MAX: u16 = 39999;
static NEXT_UPSTREAM_PORT: AtomicU16 = AtomicU16::new(UPSTREAM_PORT_MIN);

#[derive(Clone)]
pub struct UpstreamClient {
    http1: Client<Connector, Full<Bytes>>,
    http2: Client<Connector, Full<Bytes>>,
    h2c: Client<Connector, Full<Bytes>>,
}

impl UpstreamClient {
    pub fn new(insecure: bool) -> anyhow::Result<Self> {
        // Preserve the client's HTTP version on the upstream leg. Forcing
        // HTTP/2 for ordinary HTTP/1.1 clients breaks some gateways (APISIX
        // returns 400 when an h1 request is transformed into h2), while gRPC
        // needs a real h2 upstream. Separate pools also avoid cross-version
        // connection reuse.
        let h1_connector = Connector::new(insecure, &[b"http/1.1"])?;
        let h2_connector = Connector::new(insecure, &[b"h2"])?;
        let http1 = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_timer(hyper_util::rt::TokioTimer::new())
            .build(h1_connector.clone());
        let http2 = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_timer(hyper_util::rt::TokioTimer::new())
            .build(h2_connector);
        // Plaintext HTTP/2 (h2c prior knowledge) is common for local gRPC
        // services. The normal client would speak HTTP/1.1 to a cleartext
        // origin, so force h2 on a separate pool.
        let h2c = Client::builder(TokioExecutor::new())
            .http2_only(true)
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_timer(hyper_util::rt::TokioTimer::new())
            .build(h1_connector);
        Ok(Self { http1, http2, h2c })
    }

    pub async fn send(
        &self,
        req: hyper::Request<Full<Bytes>>,
    ) -> Result<hyper::Response<Incoming>, BoxError> {
        let native_grpc = is_native_grpc(req.headers());
        let is_http = req.uri().scheme_str() == Some("http");
        let (parts, body) = req.into_parts();
        let req = hyper::Request::from_parts(parts, body);
        // Ordinary web traffic uses HTTP/1.1 upstream even if the client spoke
        // HTTP/2 to us: some gateways reject h2-translated h1 requests, and
        // hyper can translate the downstream response back to h2 safely.
        // Native gRPC is the exception; it requires end-to-end HTTP/2.
        let resp = if native_grpc && is_http {
            self.h2c.request(req).await?
        } else if native_grpc {
            self.http2.request(req).await?
        } else {
            self.http1.request(req).await?
        };
        Ok(resp)
    }
}

pub fn is_native_grpc(headers: &http::HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            let ct = v.to_ascii_lowercase();
            ct.starts_with("application/grpc") && !ct.contains("grpc-web")
        })
        .unwrap_or(false)
}

#[derive(Clone)]
pub struct Connector {
    tls: TlsConnector,
}

impl Connector {
    pub fn new(insecure: bool, alpn: &[&[u8]]) -> anyhow::Result<Self> {
        let mut roots = RootCertStore::empty();
        let native = rustls_native_certs::load_native_certs();
        for cert in native.certs {
            let _ = roots.add(cert);
        }
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let mut config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        let config = if insecure {
            let mut cfg = ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerify::new()?))
                .with_no_client_auth();
            cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
            cfg
        } else {
            config
        };
        Ok(Self {
            tls: TlsConnector::from(Arc::new(config)),
        })
    }
}

impl Service<Uri> for Connector {
    type Response = TokioIo<UpstreamIo>;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            let scheme = uri.scheme_str().unwrap_or("http");
            let host = uri
                .host()
                .ok_or_else(|| format!("upstream URI missing host: {uri}"))?
                .to_string();
            let port = uri
                .port_u16()
                .unwrap_or(if scheme == "https" { 443 } else { 80 });
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
                .await?
                .collect();
            if addrs.is_empty() {
                return Err(format!("DNS resolved no addresses for {host}:{port}").into());
            }
            let mut last_err = None;
            let mut stream = None;
            for addr in addrs {
                match connect_bound(addr).await {
                    Ok(s) => {
                        let _ = s.set_nodelay(true);
                        stream = Some(s);
                        break;
                    }
                    Err(e) => last_err = Some(e),
                }
            }
            let stream = stream.ok_or_else(|| {
                format!(
                    "connect {host}:{port} failed: {}",
                    last_err
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "unknown error".into())
                )
            })?;
            if scheme == "https" {
                let server_name = ServerName::try_from(host.clone())
                    .map_err(|e| format!("invalid TLS server name {host}: {e}"))?;
                let tls = this
                    .tls
                    .connect(server_name, stream)
                    .await
                    .map_err(|e| format!("TLS handshake with {host}:{port} failed: {e}"))?;
                Ok(TokioIo::new(UpstreamIo::Tls(tls)))
            } else {
                Ok(TokioIo::new(UpstreamIo::Plain(stream)))
            }
        })
    }
}

pub enum UpstreamIo {
    Plain(TcpStream),
    Tls(tokio_rustls::client::TlsStream<TcpStream>),
}

async fn connect_bound(addr: std::net::SocketAddr) -> std::io::Result<TcpStream> {
    // Bind to a reserved source port so the WinDivert filter can exclude our own
    // upstream traffic. Fall back to an ephemeral port if the range is exhausted.
    for _ in 0..1024 {
        let port = NEXT_UPSTREAM_PORT.fetch_add(1, Ordering::Relaxed);
        if port > UPSTREAM_PORT_MAX {
            NEXT_UPSTREAM_PORT.store(UPSTREAM_PORT_MIN, Ordering::Relaxed);
            continue;
        }
        let socket = if addr.is_ipv4() {
            TcpSocket::new_v4()?
        } else {
            TcpSocket::new_v6()?
        };
        let bind_addr = if addr.is_ipv4() {
            std::net::SocketAddr::from(([0, 0, 0, 0], port))
        } else {
            std::net::SocketAddr::from(([0u16; 8], port))
        };
        if socket.bind(bind_addr).is_err() {
            continue;
        }
        match socket.connect(addr).await {
            Ok(s) => return Ok(s),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::AddrNotAvailable {
                    continue;
                }
                return Err(e);
            }
        }
    }
    TcpStream::connect(addr).await
}

impl AsyncRead for UpstreamIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match &mut *self {
            UpstreamIo::Plain(s) => Pin::new(s).poll_read(cx, buf),
            UpstreamIo::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for UpstreamIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            UpstreamIo::Plain(s) => Pin::new(s).poll_write(cx, buf),
            UpstreamIo::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            UpstreamIo::Plain(s) => Pin::new(s).poll_flush(cx),
            UpstreamIo::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            UpstreamIo::Plain(s) => Pin::new(s).poll_shutdown(cx),
            UpstreamIo::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
    fn is_write_vectored(&self) -> bool {
        match self {
            UpstreamIo::Plain(s) => s.is_write_vectored(),
            UpstreamIo::Tls(s) => s.is_write_vectored(),
        }
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            UpstreamIo::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            UpstreamIo::Tls(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }
}

impl Connection for UpstreamIo {
    fn connected(&self) -> Connected {
        match self {
            UpstreamIo::Plain(_) => Connected::new(),
            UpstreamIo::Tls(tls) => {
                if tls.get_ref().1.alpn_protocol() == Some(b"h2") {
                    Connected::new().negotiated_h2()
                } else {
                    Connected::new()
                }
            }
        }
    }
}

#[derive(Debug)]
struct NoVerify {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl NoVerify {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            provider: rustls::crypto::ring::default_provider().into(),
        })
    }
}

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
#[cfg(test)]
mod tests {
    use super::is_native_grpc;
    use http::HeaderMap;

    fn with_content_type(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(http::header::CONTENT_TYPE, value.parse().unwrap());
        h
    }

    #[test]
    fn detects_native_grpc_but_not_grpc_web() {
        assert!(is_native_grpc(&with_content_type("application/grpc")));
        assert!(is_native_grpc(&with_content_type("application/grpc+proto")));
        assert!(!is_native_grpc(&with_content_type("application/grpc-web")));
        assert!(!is_native_grpc(&with_content_type(
            "application/grpc-web+proto"
        )));
        assert!(!is_native_grpc(&with_content_type("application/json")));
        assert!(!is_native_grpc(&HeaderMap::new()));
    }
}
