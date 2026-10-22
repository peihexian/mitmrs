use crate::decode::formats::ClientHelloInfo;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

pub struct PrefixedStream<T> {
    prefix: Vec<u8>,
    pos: usize,
    inner: T,
}

impl<T> PrefixedStream<T> {
    pub fn new(prefix: Vec<u8>, inner: T) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for PrefixedStream<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.pos < self.prefix.len() {
            let remaining = &self.prefix[self.pos..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for PrefixedStream<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub async fn read_first_byte<T: AsyncRead + Unpin>(
    io: &mut T,
    timeout: std::time::Duration,
) -> std::io::Result<Option<u8>> {
    let mut b = [0u8; 1];
    match tokio::time::timeout(timeout, io.read(&mut b)).await {
        Ok(Ok(0)) => Ok(None),
        Ok(Ok(_)) => Ok(Some(b[0])),
        Ok(Err(e)) => Err(e),
        Err(_) => Ok(None),
    }
}

/// Bytes already consumed from the client plus, when parsing succeeded, the
/// ClientHello metadata. The bytes must be replayed to the TLS acceptor (or to
/// the upstream server when we fall back to a passthrough tunnel).
pub struct ClientHelloPeek {
    pub prefix: Vec<u8>,
    pub info: Option<ClientHelloInfo>,
}

const MAX_CLIENT_HELLO: usize = 16 * 1024;

/// Parse a ClientHello out of the first TLS record in `buf`. Returns `None`
/// while the record (or the handshake message inside it) is still incomplete.
pub fn parse_tls_record_client_hello(buf: &[u8]) -> Option<ClientHelloInfo> {
    if buf.len() < 5 || buf[0] != 0x16 {
        return None;
    }
    let record_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    if buf.len() < 5 + record_len {
        return None;
    }
    let handshake = &buf[5..5 + record_len];
    if handshake.len() < 4 || handshake[0] != 1 {
        return None;
    }
    let hs_len =
        ((handshake[1] as usize) << 16) | ((handshake[2] as usize) << 8) | handshake[3] as usize;
    if handshake.len() < 4 + hs_len {
        // A ClientHello fragmented across TLS records is vanishingly rare; give
        // up rather than buffering the whole handshake.
        return None;
    }
    crate::decode::formats::parse_client_hello(handshake)
}

/// Read from the client until the ClientHello can be parsed, then return every
/// byte consumed so the caller can replay it into the TLS acceptor.
pub async fn read_client_hello<T: AsyncRead + Unpin>(
    io: &mut T,
    first: u8,
    overall: Duration,
) -> ClientHelloPeek {
    let mut prefix = vec![first];
    let deadline = tokio::time::Instant::now() + overall;
    loop {
        if let Some(info) = parse_tls_record_client_hello(&prefix) {
            return ClientHelloPeek {
                prefix,
                info: Some(info),
            };
        }
        if prefix.len() >= MAX_CLIENT_HELLO {
            break;
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let mut chunk = [0u8; 4096];
        match tokio::time::timeout(left.min(Duration::from_secs(2)), io.read(&mut chunk)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(n)) => prefix.extend_from_slice(&chunk[..n]),
        }
    }
    ClientHelloPeek { prefix, info: None }
}

/// Peek (without consuming) a ClientHello from a transparently redirected
/// socket. Used to pick the leaf certificate and to detect learned passthrough
/// hosts before we answer the handshake.
pub async fn peek_client_hello(stream: &TcpStream) -> Option<ClientHelloInfo> {
    let mut buf = vec![0u8; MAX_CLIENT_HELLO];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        match tokio::time::timeout(Duration::from_millis(150), stream.peek(&mut buf)).await {
            Ok(Ok(n)) if n >= 5 => {
                if let Some(info) = parse_tls_record_client_hello(&buf[..n]) {
                    return Some(info);
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => return None,
            Err(_) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
    }
}
