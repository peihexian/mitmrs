use std::pin::Pin;
use std::task::{Context, Poll};
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

pub async fn peek_tls_sni(stream: &TcpStream) -> Option<String> {
    let mut buf = vec![0u8; 16384];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(150), stream.peek(&mut buf))
            .await
        {
            Ok(Ok(n)) if n >= 6 => {
                if buf[0] != 0x16 {
                    return None;
                }
                let rec_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
                let end = 5 + rec_len;
                if n >= end {
                    if let Some(sni) = crate::decode::formats::parse_client_hello_sni(&buf[5..end])
                    {
                        return Some(sni);
                    }
                }
                if n >= 10 && buf[5] == 0x01 {
                    let hs_len =
                        ((buf[6] as usize) << 16) | ((buf[7] as usize) << 8) | buf[8] as usize;
                    if n >= 9 + hs_len {
                        if let Some(sni) =
                            crate::decode::formats::parse_client_hello_sni(&buf[5..9 + hs_len])
                        {
                            return Some(sni);
                        }
                    }
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
