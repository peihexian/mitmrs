pub mod formats;
pub mod grpc;
pub mod protobuf;
pub mod util;

use crate::decode::formats::*;
use crate::decode::util::{
    decompress, is_probably_text, json_text, printable_ratio, safe_truncate_utf8, short_preview,
};
use crate::model::{DecodeResult, DecoderInfo, DecoderOption, FlowRecord, HeaderEntry};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use encoding_rs::Encoding;
use serde_json::{json, Value};

pub const DEFAULT_MAX_TEXT: usize = 1_000_000;

pub fn decoders() -> Vec<DecoderInfo> {
    let none = Vec::new();
    vec![
        DecoderInfo {
            id: "auto".into(),
            name: "Auto detect".into(),
            description: "Detect JSON, XML, form, multipart, gRPC, SSE, WebSocket, text or binary from the captured message.".into(),
            content_types: vec!["*/*".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "http".into(),
            name: "HTTP message".into(),
            description: "Reconstruct the HTTP request/status line, headers, trailers and body. Useful as a normalized view for humans and agents.".into(),
            content_types: vec!["message/http".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "http2".into(),
            name: "HTTP/2 message".into(),
            description: "Show the decrypted HTTP/2 stream metadata and, when raw frames are supplied, parse HTTP/2 frame headers.".into(),
            content_types: vec!["application/http2".into()],
            options: vec![
                text_limit_option(),
                DecoderOption { name: "raw_frames".into(), r#type: "boolean".into(), description: "Treat the body as raw HTTP/2 frames instead of the decrypted message body.".into(), default: Some(json!(false)) },
            ],
        },
        DecoderInfo {
            id: "grpc".into(),
            name: "gRPC / gRPC-Web".into(),
            description: "Split gRPC length-prefixed frames, decompress per-message compression, decode protobuf fields without a .proto schema, and surface trailers.".into(),
            content_types: vec!["application/grpc".into(), "application/grpc+proto".into(), "application/grpc-web".into(), "application/grpc-web-text".into()],
            options: vec![
                text_limit_option(),
                DecoderOption { name: "grpc_encoding".into(), r#type: "string".into(), description: "Message compression for compressed frames (gzip, deflate, br, zstd). Defaults to the grpc-encoding header.".into(), default: Some(json!("gzip")) },
            ],
        },
        DecoderInfo {
            id: "protobuf".into(),
            name: "Protocol Buffers".into(),
            description: "Schema-less protobuf wire decoder. Shows field numbers, wire types, varints, strings, nested messages, packed values and hex.".into(),
            content_types: vec!["application/protobuf".into(), "application/x-protobuf".into(), "application/octet-stream".into()],
            options: vec![
                text_limit_option(),
                DecoderOption { name: "skip_hex".into(), r#type: "boolean".into(), description: "Omit hex fallback from the tree.".into(), default: Some(json!(false)) },
            ],
        },
        DecoderInfo {
            id: "json".into(),
            name: "JSON".into(),
            description: "Pretty-print and parse JSON, including JSON embedded in a decoded body.".into(),
            content_types: vec!["application/json".into(), "text/json".into(), "+json".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "xml".into(),
            name: "XML / HTML".into(),
            description: "Indent and inspect XML or HTML markup.".into(),
            content_types: vec!["application/xml".into(), "text/xml".into(), "text/html".into(), "+xml".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "form".into(),
            name: "Form URL-encoded".into(),
            description: "Decode application/x-www-form-urlencoded fields with percent decoding.".into(),
            content_types: vec!["application/x-www-form-urlencoded".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "multipart".into(),
            name: "Multipart".into(),
            description: "Split multipart/form-data or multipart/mixed using the boundary in Content-Type.".into(),
            content_types: vec!["multipart/*".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "sse".into(),
            name: "Server-Sent Events".into(),
            description: "Parse text/event-stream frames into event, data, id and retry fields.".into(),
            content_types: vec!["text/event-stream".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "websocket".into(),
            name: "WebSocket frames".into(),
            description: "Parse unmasked/masked WebSocket frames with opcode, FIN, payload text and hex. Use with raw captured WebSocket bytes.".into(),
            content_types: vec!["websocket".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "tls".into(),
            name: "TLS records".into(),
            description: "Parse TLS record headers and ClientHello SNI. Useful for raw TLS bytes that were not terminated by the MITM proxy.".into(),
            content_types: vec!["application/tls".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "text".into(),
            name: "Text (charset aware)".into(),
            description: "Decode text using the Content-Type charset, BOM, or UTF-8 lossy fallback (supports GBK, Shift_JIS, etc.).".into(),
            content_types: vec!["text/*".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "base64".into(),
            name: "Base64".into(),
            description: "Decode a base64 body into bytes and text.".into(),
            content_types: vec!["application/base64".into()],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "hex".into(),
            name: "Hex dump / Hex decode".into(),
            description: "Decode a hex string, or render binary as an offset/hex/ASCII dump.".into(),
            content_types: vec![],
            options: vec![text_limit_option()],
        },
        DecoderInfo {
            id: "gzip".into(),
            name: "Content-Encoding".into(),
            description: "Decompress gzip, deflate, brotli or zstd and auto-detect the inner payload.".into(),
            content_types: vec![],
            options: vec![
                text_limit_option(),
                DecoderOption { name: "encoding".into(), r#type: "string".into(), description: "gzip | deflate | br | zstd".into(), default: Some(json!("gzip")) },
            ],
        },
        DecoderInfo {
            id: "raw".into(),
            name: "Raw bytes".into(),
            description: "Raw body as text when printable, otherwise as an offset/hex/ASCII dump.".into(),
            content_types: vec![],
            options: vec![text_limit_option()],
        },
    ]
    .into_iter()
    .map(|mut d| {
        if d.options.is_empty() {
            d.options = none.clone();
        }
        d
    })
    .collect()
}

fn text_limit_option() -> DecoderOption {
    DecoderOption {
        name: "max_bytes".into(),
        r#type: "integer".into(),
        description: "Maximum number of decoded bytes/characters to return.".into(),
        default: Some(json!(1000000)),
    }
}

pub fn decode_record(
    record: &FlowRecord,
    direction: &str,
    format: &str,
    options: &Value,
    max_bytes: usize,
) -> Result<DecodeResult, String> {
    let (headers, trailers, body) = if direction == "request" {
        (
            &record.request_headers,
            &record.request_trailers,
            &record.request_body,
        )
    } else {
        (
            &record.response_headers,
            &record.response_trailers,
            &record.response_body,
        )
    };
    decode_message(
        record, direction, headers, trailers, body, format, options, max_bytes,
    )
}

pub fn decode_message(
    record: &FlowRecord,
    direction: &str,
    headers: &[HeaderEntry],
    trailers: &[HeaderEntry],
    body: &[u8],
    format: &str,
    options: &Value,
    max_bytes: usize,
) -> Result<DecodeResult, String> {
    let max = options
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .map(|v| (v as usize).clamp(256, 8 * 1024 * 1024))
        .unwrap_or(max_bytes.min(DEFAULT_MAX_TEXT));
    let content_type = header(headers, "content-type");
    let content_encoding = header(headers, "content-encoding");
    let grpc_encoding = header(headers, "grpc-encoding")
        .or_else(|| {
            options
                .get("grpc_encoding")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| Some("identity".into()));
    let format = format.to_ascii_lowercase();
    if format == "auto" {
        return Ok(auto_decode(
            record,
            direction,
            headers,
            trailers,
            body,
            content_type.as_deref(),
            content_encoding.as_deref(),
            grpc_encoding.as_deref(),
            options,
            max,
        ));
    }
    let result = match format.as_str() {
        "http" => decode_http(record, direction, headers, trailers, body, max, false),
        "http2" | "h2" => {
            let raw = options
                .get("raw_frames")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if raw {
                decode_http2_frames(body, max)
            } else {
                decode_http(record, direction, headers, trailers, body, max, true)
            }
        }
        "grpc" | "grpc-web" => {
            let is_web = format == "grpc-web"
                || content_type
                    .as_deref()
                    .map(|s| s.contains("grpc-web"))
                    .unwrap_or(false)
                || record.path.contains("grpc-web");
            grpc::decode_grpc(
                body,
                content_type.as_deref(),
                grpc_encoding.as_deref(),
                is_web,
                max,
            )
        }
        "protobuf" | "proto" => decode_protobuf(body, max),
        "json" => decode_json(body, headers, max),
        "xml" | "html" => decode_xml(body, max),
        "form" | "urlencoded" => decode_form(body, max),
        "multipart" => decode_multipart(body, content_type.as_deref(), max),
        "sse" | "eventsource" => decode_sse(body, max),
        "websocket" | "ws" => decode_websocket(body, max),
        "tls" => decode_tls(body, max),
        "text" => decode_text_charset(body, headers, max),
        "base64" | "b64" => decode_base64_body(body, max),
        "hex" => {
            if is_probably_hex(body) {
                let mut r = decode_hex_body(body, max);
                r.detected = Some("hex-text".into());
                r
            } else {
                let text = util::hexdump(body, max.min(1_000_000));
                DecodeResult {
                    format: "hex".into(),
                    detected: Some("binary".into()),
                    summary: format!("{} bytes as hex dump", body.len()),
                    text: Some(text),
                    tree: json!({ "size": body.len() }),
                    meta: json!({}),
                }
            }
        }
        "gzip" | "deflate" | "br" | "brotli" | "zstd" => {
            let enc = options
                .get("encoding")
                .and_then(|v| v.as_str())
                .unwrap_or(format.as_str());
            decode_compressed(body, enc, max)
        }
        "raw" | "binary" => decode_raw(body, max),
        other => return Err(format!("unknown decoder format '{other}'")),
    };
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn auto_decode(
    record: &FlowRecord,
    _direction: &str,
    headers: &[HeaderEntry],
    _trailers: &[HeaderEntry],
    body: &[u8],
    content_type: Option<&str>,
    content_encoding: Option<&str>,
    grpc_encoding: Option<&str>,
    options: &Value,
    max: usize,
) -> DecodeResult {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    if ct.contains("grpc") {
        return grpc::decode_grpc(
            body,
            content_type,
            grpc_encoding,
            ct.contains("grpc-web"),
            max,
        );
    }
    let mut data = body.to_vec();
    let mut notes: Vec<String> = Vec::new();
    if let Some(enc) = content_encoding {
        if !enc.is_empty() && !enc.eq_ignore_ascii_case("identity") {
            match decompress(body, enc, max.saturating_mul(4).min(32 * 1024 * 1024)) {
                Ok(decoded) => {
                    notes.push(format!(
                        "decompressed {enc}: {} -> {} bytes",
                        body.len(),
                        decoded.len()
                    ));
                    data = decoded;
                }
                Err(e) => notes.push(format!("content-encoding {enc} not decoded: {e}")),
            }
        }
    }
    let mut result = if data.is_empty() {
        DecodeResult {
            format: "auto".into(),
            detected: Some("empty".into()),
            summary: "Empty body".into(),
            text: Some(String::new()),
            tree: json!({ "size": 0 }),
            meta: json!({}),
        }
    } else if ct.contains("json") || util::looks_like_json(&data) {
        decode_json(&data, headers, max)
    } else if ct.contains("xml") || ct.contains("html") || util::looks_like_xml(&data) {
        decode_xml(&data, max)
    } else if ct.contains("x-www-form-urlencoded") {
        decode_form(&data, max)
    } else if ct.contains("multipart/") {
        decode_multipart(&data, content_type, max)
    } else if ct.contains("text/event-stream") {
        decode_sse(&data, max)
    } else if ct.contains("protobuf") || ct.contains("x-protobuf") {
        decode_protobuf(&data, max)
    } else if ct.contains("text/")
        || ct.contains("javascript")
        || ct.contains("graphql")
        || (is_probably_text(&data) && printable_ratio(&data) > 0.9)
    {
        decode_text_charset(&data, headers, max)
    } else if data.len() >= 5 {
        let maybe_grpc = {
            let flag = data[0];
            let len = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;
            (flag == 0 || flag == 1) && len > 0 && len + 5 <= data.len()
        };
        if maybe_grpc {
            grpc::decode_grpc(&data, content_type, grpc_encoding, false, max)
        } else {
            decode_raw(&data, max)
        }
    } else {
        decode_raw(&data, max)
    };
    if !notes.is_empty() {
        if let Some(meta) = result.meta.as_object_mut() {
            meta.insert("notes".into(), json!(notes));
        }
    }
    result.format = "auto".into();
    if result.detected.is_none() {
        result.detected = Some("raw".into());
    }
    // include a record-level hint in the summary for humans
    if result.summary.is_empty() {
        result.summary = format!("decoded {} body", record.method);
    }
    let _ = options;
    result
}

fn decode_http(
    record: &FlowRecord,
    direction: &str,
    headers: &[HeaderEntry],
    trailers: &[HeaderEntry],
    body: &[u8],
    max: usize,
    h2: bool,
) -> DecodeResult {
    let start_line = if direction == "request" {
        format!("{} {} {}", record.method, record.path, record.http_version)
    } else {
        format!(
            "{} {} {}",
            record.http_version,
            record.status.unwrap_or(0),
            record.reason.clone().unwrap_or_default()
        )
    };
    let mut text = String::new();
    text.push_str(&start_line);
    text.push('\n');
    for h in headers {
        text.push_str(&format!("{}: {}\n", h.name, h.value));
    }
    text.push('\n');
    let body_text = if is_probably_text(body) {
        decode_text_charset(body, headers, max)
            .text
            .unwrap_or_default()
    } else {
        util::hexdump(body, max.min(1_000_000))
    };
    text.push_str(&body_text);
    if !trailers.is_empty() {
        text.push_str("\n\n[trailers]\n");
        for t in trailers {
            text.push_str(&format!("{}: {}\n", t.name, t.value));
        }
    }
    let header_json: Vec<Value> = headers
        .iter()
        .map(|h| json!({ "name": h.name, "value": h.value }))
        .collect();
    let trailer_json: Vec<Value> = trailers
        .iter()
        .map(|h| json!({ "name": h.name, "value": h.value }))
        .collect();
    DecodeResult {
        format: if h2 { "http2".into() } else { "http".into() },
        detected: Some(record.http_version.clone()),
        summary: format!(
            "{} {} {} — {} header(s), body {} bytes",
            if direction == "request" {
                "Request"
            } else {
                "Response"
            },
            record.method,
            record
                .status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".into()),
            headers.len(),
            body.len()
        ),
        text: Some(safe_truncate_utf8(text, max)),
        tree: json!({
            "start_line": start_line,
            "method": if direction == "request" { Value::String(record.method.clone()) } else { Value::Null },
            "status": record.status,
            "version": record.http_version,
            "headers": header_json,
            "trailers": trailer_json,
            "body": {
                "size": body.len(),
                "text": if is_probably_text(body) { Value::String(short_preview(body, 64 * 1024)) } else { Value::Null },
                "hex": if is_probably_text(body) { Value::Null } else { Value::String(util::hexdump(body, 4096)) },
            },
            "tls_sni": record.tls_sni,
        }),
        meta: json!({
            "http_version": record.http_version,
            "content_type": header(headers, "content-type"),
            "content_encoding": header(headers, "content-encoding"),
            "grpc_status": header(trailers, "grpc-status"),
            "grpc_message": header(trailers, "grpc-message"),
        }),
    }
}

fn decode_protobuf(body: &[u8], max: usize) -> DecodeResult {
    match protobuf::decode_message(body) {
        Ok(fields) if !fields.is_empty() => {
            let mut text = String::new();
            protobuf::text_dump(&fields, 0, &mut text);
            DecodeResult {
                format: "protobuf".into(),
                detected: Some("protobuf".into()),
                summary: format!("{} protobuf field(s), {} bytes", fields.len(), body.len()),
                text: Some(safe_truncate_utf8(text, max)),
                tree: json!({ "fields": protobuf::tree_value(&fields, 0) }),
                meta: json!({ "field_count": fields.len(), "schema": null }),
            }
        }
        Ok(_) => DecodeResult {
            format: "protobuf".into(),
            detected: Some("protobuf".into()),
            summary: "Empty protobuf message".into(),
            text: Some(String::new()),
            tree: json!({ "fields": [] }),
            meta: json!({ "field_count": 0 }),
        },
        Err(e) => DecodeResult {
            format: "protobuf".into(),
            detected: Some("invalid-protobuf".into()),
            summary: format!("Could not parse protobuf: {e}"),
            text: Some(short_preview(body, 64 * 1024)),
            tree: json!({ "hex": util::hexdump(body, 4096) }),
            meta: json!({ "error": e }),
        },
    }
}

fn decode_compressed(body: &[u8], encoding: &str, max: usize) -> DecodeResult {
    match decompress(body, encoding, max.saturating_mul(8).min(64 * 1024 * 1024)) {
        Ok(decoded) => {
            let inner = if util::looks_like_json(&decoded) {
                decode_json(&decoded, &[], max)
            } else if is_probably_text(&decoded) {
                decode_text(&decoded, max)
            } else {
                decode_raw(&decoded, max)
            };
            DecodeResult {
                format: "gzip".into(),
                detected: Some(format!("{encoding}-decompressed")),
                summary: format!(
                    "Decompressed {encoding}: {} -> {} bytes; inner {}",
                    body.len(),
                    decoded.len(),
                    inner.format
                ),
                text: inner.text,
                tree: json!({ "decompressed_size": decoded.len(), "inner": inner.tree }),
                meta: json!({ "encoding": encoding, "inner_format": inner.format }),
            }
        }
        Err(e) => DecodeResult {
            format: "gzip".into(),
            detected: None,
            summary: format!("Decompression failed: {e}"),
            text: Some(short_preview(body, 64 * 1024)),
            tree: Value::Null,
            meta: json!({ "error": e }),
        },
    }
}

fn decode_text_charset(body: &[u8], headers: &[HeaderEntry], max: usize) -> DecodeResult {
    let charset = header(headers, "content-type")
        .and_then(|ct| {
            ct.split(';').find_map(|p| {
                let p = p.trim();
                p.strip_prefix("charset=")
                    .map(|c| c.trim_matches('"').to_string())
            })
        })
        .unwrap_or_default();
    let (text, encoding) = if let Some(enc) = Encoding::for_label(charset.as_bytes()) {
        let (cow, _, _) = enc.decode(body);
        (cow.into_owned(), enc.name().to_string())
    } else if body.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (
            String::from_utf8_lossy(&body[3..]).into_owned(),
            "UTF-8 (BOM)".into(),
        )
    } else if let Some(enc) = Encoding::for_bom(body).map(|(e, _)| e) {
        let (cow, _, _) = enc.decode(body);
        (cow.into_owned(), enc.name().to_string())
    } else {
        (
            String::from_utf8_lossy(body).into_owned(),
            "UTF-8 (lossy)".into(),
        )
    };
    DecodeResult {
        format: "text".into(),
        detected: Some(encoding.clone()),
        summary: format!("{} chars, charset {}", text.chars().count(), encoding),
        text: Some(safe_truncate_utf8(text.clone(), max)),
        tree: json!({ "length": text.chars().count(), "charset": encoding }),
        meta: json!({ "charset": encoding }),
    }
}

fn decode_http2_frames(body: &[u8], max: usize) -> DecodeResult {
    let mut frames = Vec::new();
    let mut pos = 0usize;
    let type_name = |t: u8| match t {
        0 => "DATA",
        1 => "HEADERS",
        2 => "PRIORITY",
        3 => "RST_STREAM",
        4 => "SETTINGS",
        5 => "PUSH_PROMISE",
        6 => "PING",
        7 => "GOAWAY",
        8 => "WINDOW_UPDATE",
        9 => "CONTINUATION",
        _ => "UNKNOWN",
    };
    let mut text = String::new();
    while pos + 9 <= body.len() {
        let len =
            ((body[pos] as usize) << 16) | ((body[pos + 1] as usize) << 8) | body[pos + 2] as usize;
        let ty = body[pos + 3];
        let flags = body[pos + 4];
        let stream = u32::from_be_bytes([
            body[pos + 5] & 0x7f,
            body[pos + 6],
            body[pos + 7],
            body[pos + 8],
        ]);
        pos += 9;
        if pos + len > body.len() {
            break;
        }
        let payload = &body[pos..pos + len];
        pos += len;
        text.push_str(&format!(
            "{} stream={} flags=0x{:02x} len={}\n",
            type_name(ty),
            stream,
            flags,
            len
        ));
        frames.push(json!({
            "type": type_name(ty),
            "type_id": ty,
            "flags": flags,
            "stream_id": stream,
            "length": len,
            "payload_hex": payload.iter().take(256).map(|b| format!("{b:02x}")).collect::<String>(),
        }));
        if frames.len() > 10_000 {
            break;
        }
    }
    DecodeResult {
        format: "http2".into(),
        detected: Some("h2-frames".into()),
        summary: format!("{} HTTP/2 frame(s)", frames.len()),
        text: Some(safe_truncate_utf8(text, max)),
        tree: json!({ "frames": frames }),
        meta: json!({}),
    }
}

fn is_probably_hex(body: &[u8]) -> bool {
    let cleaned: Vec<u8> = body
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    !cleaned.is_empty()
        && cleaned.len().is_multiple_of(2)
        && cleaned.iter().all(|b| b.is_ascii_hexdigit())
        && cleaned.len() >= 8
}

fn header(headers: &[HeaderEntry], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.clone())
}

pub fn base64_body(body: &[u8]) -> String {
    B64.encode(body)
}

pub fn detected_summary(record: &FlowRecord, direction: &str) -> String {
    let (ct, body) = if direction == "request" {
        (
            header(&record.request_headers, "content-type"),
            record.request_body.len(),
        )
    } else {
        (
            header(&record.response_headers, "content-type"),
            record.response_body.len(),
        )
    };
    format!(
        "{} {} {} · {} bytes · {}",
        record.method,
        record.host,
        record.path,
        body,
        ct.unwrap_or_else(|| "unknown content-type".into())
    )
}

pub fn json_decode_result(result: &DecodeResult) -> String {
    json_text(&json!({
        "format": result.format,
        "detected": result.detected,
        "summary": result.summary,
        "text": result.text,
        "tree": result.tree,
        "meta": result.meta,
    }))
}
