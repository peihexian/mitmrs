use base64::{engine::general_purpose::STANDARD as B64, Engine};
use std::io::Read;

pub const MAX_DECODE_BYTES: usize = 8 * 1024 * 1024;

pub fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    B64.decode(cleaned.as_bytes()).ok()
}

pub fn decode_hex(s: &str) -> Option<Vec<u8>> {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>();
    if !cleaned.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    let bytes = cleaned.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

pub fn hexdump(data: &[u8], max: usize) -> String {
    let mut out = String::new();
    let take = data.len().min(max);
    for (offset, chunk) in data[..take].chunks(16).enumerate() {
        let mut hex = String::new();
        let mut ascii = String::new();
        for b in chunk {
            hex.push_str(&format!("{:02x} ", b));
            ascii.push(if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '.'
            });
        }
        out.push_str(&format!("{:08x}  {:<48} |{}|\n", offset * 16, hex, ascii));
    }
    if data.len() > take {
        out.push_str(&format!("... {} more bytes\n", data.len() - take));
    }
    out
}

pub fn is_probably_text(data: &[u8]) -> bool {
    if data.is_empty() {
        return true;
    }
    if std::str::from_utf8(data).is_err() {
        return false;
    }
    let sample = &data[..data.len().min(8192)];
    let control = sample
        .iter()
        .filter(|b| **b < 0x09 || (**b > 0x0d && **b < 0x20))
        .count();
    control * 100 / sample.len().max(1) < 2
}

pub fn printable_ratio(data: &[u8]) -> f32 {
    if data.is_empty() {
        return 1.0;
    }
    let good = data
        .iter()
        .filter(|b| **b == b'\n' || **b == b'\r' || **b == b'\t' || (**b >= 0x20 && **b < 0x7f))
        .count();
    good as f32 / data.len() as f32
}

pub fn decompress(data: &[u8], encoding: &str, limit: usize) -> Result<Vec<u8>, String> {
    let enc = encoding.trim().to_ascii_lowercase();
    if enc.is_empty() || enc == "identity" {
        return Ok(data.to_vec());
    }
    let mut out = Vec::new();
    let result = match enc.as_str() {
        "gzip" | "x-gzip" => flate2::read::GzDecoder::new(data)
            .take(limit as u64)
            .read_to_end(&mut out),
        "deflate" => {
            let mut out2 = Vec::new();
            let r = flate2::read::ZlibDecoder::new(data)
                .take(limit as u64)
                .read_to_end(&mut out2);
            match r {
                Ok(n) => {
                    out = out2;
                    Ok(n)
                }
                Err(_) => {
                    out.clear();
                    flate2::read::DeflateDecoder::new(data)
                        .take(limit as u64)
                        .read_to_end(&mut out)
                }
            }
        }
        "br" => brotli::Decompressor::new(data, 4096)
            .take(limit as u64)
            .read_to_end(&mut out),
        "zstd" => zstd::stream::read::Decoder::new(data)
            .map_err(|e| e.to_string())?
            .take(limit as u64)
            .read_to_end(&mut out),
        other => return Err(format!("unsupported content-encoding: {other}")),
    };
    result.map_err(|e| format!("decompress {enc} failed: {e}"))?;
    Ok(out)
}

/// Result of a tolerant decompression pass.
#[derive(Debug, Clone)]
pub struct Inflated {
    pub data: Vec<u8>,
    /// False when the stream was cut short (truncated capture) or the output
    /// limit was reached before the end of the stream.
    pub complete: bool,
}

/// Like [`decompress`], but keeps the bytes that could be inflated instead of
/// failing outright when the captured body is a truncated prefix of the
/// compressed stream. Used by the body viewer, which has to work with whatever
/// the capture size limit kept on disk.
pub fn inflate_relaxed(data: &[u8], encoding: &str, limit: usize) -> Result<Inflated, String> {
    let enc = encoding.trim().to_ascii_lowercase();
    if enc.is_empty() || enc == "identity" {
        return Ok(Inflated {
            data: data.to_vec(),
            complete: true,
        });
    }
    let limit = limit.max(1);
    let mut out = Vec::new();
    let read = match enc.as_str() {
        "gzip" | "x-gzip" => flate2::read::GzDecoder::new(data)
            .take(limit as u64)
            .read_to_end(&mut out),
        "deflate" => {
            let mut zlib_out = Vec::new();
            match flate2::read::ZlibDecoder::new(data)
                .take(limit as u64)
                .read_to_end(&mut zlib_out)
            {
                Ok(n) => {
                    out = zlib_out;
                    Ok(n)
                }
                Err(_) => {
                    let mut raw_out = Vec::new();
                    let r = flate2::read::DeflateDecoder::new(data)
                        .take(limit as u64)
                        .read_to_end(&mut raw_out);
                    out = raw_out;
                    r
                }
            }
        }
        "br" => brotli::Decompressor::new(data, 4096)
            .take(limit as u64)
            .read_to_end(&mut out),
        "zstd" => zstd::stream::read::Decoder::new(data)
            .map_err(|e| e.to_string())?
            .take(limit as u64)
            .read_to_end(&mut out),
        other => return Err(format!("unsupported content-encoding: {other}")),
    };
    match read {
        Ok(_) => {
            let complete = out.len() < limit;
            Ok(Inflated {
                data: out,
                complete,
            })
        }
        Err(e) => {
            if out.is_empty() {
                Err(format!("decompress {enc} failed: {e}"))
            } else {
                Ok(Inflated {
                    data: out,
                    complete: false,
                })
            }
        }
    }
}

pub fn safe_truncate_utf8(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s.push_str("\n… output truncated …");
    s
}

pub fn json_text(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub fn short_preview(data: &[u8], max: usize) -> String {
    let take = data.len().min(max);
    let s = String::from_utf8_lossy(&data[..take]).replace('\r', "");
    safe_truncate_utf8(s, max)
}

pub fn looks_like_json(data: &[u8]) -> bool {
    let trimmed = trim_ws(data);
    matches!(trimmed.first(), Some(b'{') | Some(b'['))
}

pub fn looks_like_xml(data: &[u8]) -> bool {
    let trimmed = trim_ws(data);
    trimmed.starts_with(b"<?xml") || (trimmed.starts_with(b"<") && trimmed.len() > 2)
}

pub fn trim_ws(data: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = data.len();
    while start < end && data[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && data[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &data[start..end]
}
