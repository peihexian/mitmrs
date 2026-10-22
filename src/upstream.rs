use base64::Engine;
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
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpSocket, TcpStream};
use tokio_rustls::TlsConnector;
use tower::Service;
use url::Url;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Source port range reserved for the proxy's own upstream sockets.
/// WinDivert's redirect filter excludes this range so outbound connections made
/// by MitmRs itself can never be redirected back into the proxy (a loop).
pub const UPSTREAM_PORT_MIN: u16 = 30000;
pub const UPSTREAM_PORT_MAX: u16 = 39999;
static NEXT_UPSTREAM_PORT: AtomicU16 = AtomicU16::new(UPSTREAM_PORT_MIN);

/// Optional next-hop HTTP proxy ("chain proxy").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyChain {
    pub host: String,
    pub port: u16,
    pub authorization: Option<String>,
}

impl ProxyChain {
    /// Accepts `127.0.0.1:7890`, `http://127.0.0.1:7890`, and URL userinfo for
    /// Basic proxy authentication. HTTPS-to-proxy and SOCKS are intentionally
    /// not accepted here.
    pub fn parse(input: &str) -> Result<Self, String> {
        let raw = input.trim();
        if raw.is_empty() {
            return Err("upstream proxy address is empty".into());
        }
        let with_scheme = if raw.contains("://") {
            raw.to_string()
        } else {
            format!("http://{raw}")
        };
        let url = Url::parse(&with_scheme)
            .map_err(|e| format!("invalid upstream proxy address '{input}': {e}"))?;
        if url.scheme() != "http" {
            return Err(format!(
                "unsupported upstream proxy scheme '{}': only http:// is supported",
                url.scheme()
            ));
        }
        if !url.path().is_empty() && url.path() != "/" {
            return Err("upstream proxy address must not contain a path".into());
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err("upstream proxy address must not contain query/fragment".into());
        }
        let host = url
            .host_str()
            .map(|s| s.trim_matches(['[', ']']).to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "upstream proxy address has no host".to_string())?;
        let port = url.port_or_known_default().unwrap_or(80);
        if port == 0 {
            return Err("upstream proxy port must be between 1 and 65535".into());
        }
        let authorization = if url.username().is_empty() {
            None
        } else {
            let user = percent_encoding::percent_decode_str(url.username())
                .decode_utf8_lossy()
                .into_owned();
            let pass = url
                .password()
                .map(|p| {
                    percent_encoding::percent_decode_str(p)
                        .decode_utf8_lossy()
                        .into_owned()
                })
                .unwrap_or_default();
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
            Some(format!("Basic {token}"))
        };
        Ok(Self {
            host,
            port,
            authorization,
        })
    }

    fn connect_authority(&self, host: &str, port: u16) -> String {
        let h = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        format!("{h}:{port}")
    }
}

#[derive(Clone)]
pub struct UpstreamClient {
    http1: Client<Connector, Full<Bytes>>,
    http2: Client<Connector, Full<Bytes>>,
    h2c: Client<Connector, Full<Bytes>>,
}

impl UpstreamClient {
    pub fn new(insecure: bool, chain: Option<ProxyChain>) -> anyhow::Result<Self> {
        // Preserve the client's HTTP version on the upstream leg. Forcing
        // HTTP/2 for ordinary HTTP/1.1 clients breaks some gateways (APISIX
        // returns 400 when an h1 request is transformed into h2), while gRPC
        // needs a real h2 upstream. Separate pools also avoid cross-version
        // connection reuse.
        let chain = chain.map(Arc::new);
        let h1_connector = Connector::new(insecure, &[b"http/1.1"], chain.clone(), false)?;
        let h2_connector = Connector::new(insecure, &[b"h2"], chain.clone(), false)?;
        let h2c_connector = Connector::new(insecure, &[b"h2"], chain, true)?;
        let http1 = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_timer(hyper_util::rt::TokioTimer::new())
            .build(h1_connector);
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
            .build(h2c_connector);
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
    chain: Option<Arc<ProxyChain>>,
    force_connect: bool,
}

impl Connector {
    pub fn new(
        insecure: bool,
        alpn: &[&[u8]],
        chain: Option<Arc<ProxyChain>>,
        force_connect: bool,
    ) -> anyhow::Result<Self> {
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
            chain,
            force_connect,
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

            let (stream, proxy_plain) = if let Some(chain) = this.chain.as_ref() {
                if scheme == "https" || this.force_connect {
                    (connect_via_chain(chain, &host, port).await?, false)
                } else {
                    // Plain HTTP: hyper will send an absolute-form request to
                    // the upstream proxy because Connected::proxy(true).
                    let stream = connect_tcp(&chain.host, chain.port).await.map_err(|e| {
                        format!(
                            "connect upstream proxy {}:{} failed: {e}",
                            chain.host, chain.port
                        )
                    })?;
                    (stream, true)
                }
            } else {
                (connect_tcp(&host, port).await?, false)
            };

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
                Ok(TokioIo::new(UpstreamIo::Plain(stream, proxy_plain)))
            }
        })
    }
}

async fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, BoxError> {
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("DNS lookup {host}:{port} failed: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("DNS resolved no addresses for {host}:{port}").into());
    }
    let mut last_err = None;
    for addr in addrs {
        match connect_bound(addr).await {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(format!(
        "connect {host}:{port} failed: {}",
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown error".into())
    )
    .into())
}

/// Open a TCP tunnel to `host:port` through the configured next-hop proxy.
/// Used for HTTPS upstream connections and raw CONNECT tunnels.
pub async fn connect_via_chain(
    chain: &ProxyChain,
    host: &str,
    port: u16,
) -> Result<TcpStream, BoxError> {
    let stream = connect_tcp(&chain.host, chain.port).await.map_err(|e| {
        format!(
            "connect upstream proxy {}:{} failed: {e}",
            chain.host, chain.port
        )
    })?;
    tokio::time::timeout(
        Duration::from_secs(20),
        establish_tunnel(stream, host, port, chain),
    )
    .await
    .map_err(|_| {
        format!(
            "upstream proxy CONNECT {} via {}:{} timed out",
            chain.connect_authority(host, port),
            chain.host,
            chain.port
        )
    })?
}

async fn establish_tunnel(
    mut stream: TcpStream,
    host: &str,
    port: u16,
    chain: &ProxyChain,
) -> Result<TcpStream, BoxError> {
    let authority = chain.connect_authority(host, port);
    let mut request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if let Some(auth) = chain.authorization.as_deref() {
        request.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write CONNECT to upstream proxy failed: {e}"))?;
    stream
        .flush()
        .await
        .map_err(|e| format!("flush CONNECT to upstream proxy failed: {e}"))?;

    let mut head = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    loop {
        if head.len() >= 64 * 1024 {
            return Err("upstream proxy CONNECT response headers too large".into());
        }
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|e| format!("read CONNECT response from upstream proxy failed: {e}"))?;
        if n == 0 {
            return Err("upstream proxy closed the connection during CONNECT".into());
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head_text = String::from_utf8_lossy(&head);
    let status_line = head_text.lines().next().unwrap_or_default().trim();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok());
    match status {
        Some(code) if (200..300).contains(&code) => Ok(stream),
        Some(code) => Err(format!(
            "upstream proxy CONNECT {authority} returned {code} ({status_line})"
        )
        .into()),
        None => Err(format!("invalid upstream proxy CONNECT response: {}", status_line).into()),
    }
}

pub enum UpstreamIo {
    Plain(TcpStream, bool),
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
            UpstreamIo::Plain(s, _) => Pin::new(s).poll_read(cx, buf),
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
            UpstreamIo::Plain(s, _) => Pin::new(s).poll_write(cx, buf),
            UpstreamIo::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            UpstreamIo::Plain(s, _) => Pin::new(s).poll_flush(cx),
            UpstreamIo::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            UpstreamIo::Plain(s, _) => Pin::new(s).poll_shutdown(cx),
            UpstreamIo::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
    fn is_write_vectored(&self) -> bool {
        match self {
            UpstreamIo::Plain(s, _) => s.is_write_vectored(),
            UpstreamIo::Tls(s) => s.is_write_vectored(),
        }
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            UpstreamIo::Plain(s, _) => Pin::new(s).poll_write_vectored(cx, bufs),
            UpstreamIo::Tls(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }
}

impl Connection for UpstreamIo {
    fn connected(&self) -> Connected {
        match self {
            UpstreamIo::Plain(_, true) => Connected::new().proxy(true),
            UpstreamIo::Plain(_, false) => Connected::new(),
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
    use super::{is_native_grpc, ProxyChain};
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

    #[test]
    fn parses_next_hop_address_forms() {
        let plain = ProxyChain::parse(" 127.0.0.1:7890 ").unwrap();
        assert_eq!(plain.host, "127.0.0.1");
        assert_eq!(plain.port, 7890);
        assert_eq!(plain.authorization, None);

        let with_scheme = ProxyChain::parse("http://proxy.example:3128").unwrap();
        assert_eq!(with_scheme.host, "proxy.example");
        assert_eq!(with_scheme.port, 3128);

        let with_auth = ProxyChain::parse("http://user:p%40ss@127.0.0.1:7890").unwrap();
        assert_eq!(with_auth.host, "127.0.0.1");
        assert_eq!(
            with_auth.authorization.as_deref(),
            Some("Basic dXNlcjpwQHNz")
        );
    }

    #[test]
    fn rejects_unsupported_or_malformed_next_hop_addresses() {
        assert!(ProxyChain::parse("").unwrap_err().contains("empty"));
        assert!(ProxyChain::parse("socks5://127.0.0.1:1080")
            .unwrap_err()
            .contains("only http://"));
        assert!(ProxyChain::parse("http://127.0.0.1:7890/path")
            .unwrap_err()
            .contains("path"));
        assert!(ProxyChain::parse("http://127.0.0.1:7890?x=1")
            .unwrap_err()
            .contains("query"));
    }
}
