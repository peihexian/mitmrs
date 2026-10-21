use crate::decode::util::{
    decode_base64, decode_hex, hexdump, is_probably_text, json_text, safe_truncate_utf8,
    short_preview,
};
use crate::model::{DecodeResult, HeaderEntry};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};

pub fn decode_json(data: &[u8], _headers: &[HeaderEntry], max: usize) -> DecodeResult {
    let text = String::from_utf8_lossy(data).into_owned();
    match serde_json::from_slice::<Value>(data) {
        Ok(v) => {
            let kind = if v.is_object() {
                format!(
                    "JSON object ({} keys)",
                    v.as_object().map(|o| o.len()).unwrap_or(0)
                )
            } else if v.is_array() {
                format!(
                    "JSON array ({} items)",
                    v.as_array().map(|a| a.len()).unwrap_or(0)
                )
            } else {
                format!("JSON {}", json_type(&v))
            };
            DecodeResult {
                format: "json".into(),
                detected: Some("json".into()),
                summary: kind,
                text: Some(safe_truncate_utf8(json_text(&v), max)),
                tree: v,
                meta: json!({ "valid": true }),
            }
        }
        Err(e) => DecodeResult {
            format: "json".into(),
            detected: Some("invalid-json".into()),
            summary: format!("invalid JSON: {e}"),
            text: Some(safe_truncate_utf8(text, max)),
            tree: Value::Null,
            meta: json!({ "valid": false, "error": e.to_string() }),
        },
    }
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub fn decode_xml(data: &[u8], max: usize) -> DecodeResult {
    let text = String::from_utf8_lossy(data).into_owned();
    let pretty = pretty_xml(&text);
    let tag_count = text.matches('<').count();
    DecodeResult {
        format: "xml".into(),
        detected: Some("xml".into()),
        summary: format!("XML document (~{tag_count} tags)"),
        text: Some(safe_truncate_utf8(pretty, max)),
        tree: json!({ "raw": safe_truncate_utf8(text, max) }),
        meta: json!({ "tag_count": tag_count }),
    }
}

fn pretty_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 64);
    let mut depth = 0usize;
    let mut i = 0usize;
    let bytes = input.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let end = input[i..]
                .find('>')
                .map(|p| i + p)
                .unwrap_or(bytes.len() - 1);
            let tag = &input[i..=end];
            let closing = tag.starts_with("</");
            let self_closing =
                tag.ends_with("/>") || tag.starts_with("<?") || tag.starts_with("<!");
            if closing {
                depth = depth.saturating_sub(1);
            }
            if !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&"  ".repeat(depth));
            out.push_str(tag.trim());
            if !closing && !self_closing {
                depth += 1;
            }
            i = end + 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] != b'<' {
                let next = input[i..].find('<').map(|p| i + p).unwrap_or(bytes.len());
                let text = input[i..next].trim();
                if !text.is_empty() {
                    out.push_str(text);
                }
                i = next;
            }
        } else {
            i += 1;
        }
    }
    out
}

pub fn decode_form(data: &[u8], max: usize) -> DecodeResult {
    let text = String::from_utf8_lossy(data);
    let mut pairs = Vec::new();
    for pair in text.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = percent_encoding::percent_decode_str(k)
            .decode_utf8_lossy()
            .into_owned();
        let v = percent_encoding::percent_decode_str(v)
            .decode_utf8_lossy()
            .into_owned();
        pairs.push(json!({ "name": k, "value": v }));
    }
    let mut text_out = String::new();
    for p in &pairs {
        text_out.push_str(&format!(
            "{} = {}\n",
            p["name"].as_str().unwrap_or(""),
            p["value"].as_str().unwrap_or("")
        ));
    }
    DecodeResult {
        format: "form".into(),
        detected: Some("form-urlencoded".into()),
        summary: format!("URL-encoded form with {} field(s)", pairs.len()),
        text: Some(safe_truncate_utf8(text_out, max)),
        tree: json!({ "fields": pairs }),
        meta: json!({}),
    }
}

pub fn decode_multipart(data: &[u8], content_type: Option<&str>, max: usize) -> DecodeResult {
    let boundary = content_type
        .and_then(|ct| {
            ct.split(';').find_map(|part| {
                let p = part.trim();
                p.strip_prefix("boundary=")
                    .map(|b| b.trim_matches('"').to_string())
            })
        })
        .unwrap_or_default();
    let text = String::from_utf8_lossy(data);
    let mut parts: Vec<Value> = Vec::new();
    let mut text_out = String::new();
    if !boundary.is_empty() {
        let sep = format!("--{boundary}");
        for raw in text.split(&sep) {
            let raw = raw.trim_matches(|c| c == '\r' || c == '\n');
            if raw.is_empty() || raw == "--" {
                continue;
            }
            let raw = raw.strip_suffix("--").unwrap_or(raw).trim();
            let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
            let mut headers = Vec::new();
            for line in head.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    headers.push(json!({ "name": k.trim(), "value": v.trim() }));
                }
            }
            let ct = headers
                .iter()
                .find(|h| {
                    h["name"]
                        .as_str()
                        .unwrap_or("")
                        .eq_ignore_ascii_case("content-type")
                })
                .and_then(|h| h["value"].as_str())
                .unwrap_or("");
            text_out.push_str(&format!(
                "--- part {} ({ct}, {} bytes) ---\n",
                parts.len() + 1,
                body.len()
            ));
            text_out.push_str(&short_preview(body.as_bytes(), 2000));
            text_out.push('\n');
            parts.push(json!({
                "headers": headers,
                "content_type": ct,
                "size": body.len(),
                "body": short_preview(body.as_bytes(), 4096),
            }));
        }
    } else {
        text_out.push_str("multipart boundary not found in Content-Type\n");
    }
    DecodeResult {
        format: "multipart".into(),
        detected: Some("multipart".into()),
        summary: if boundary.is_empty() {
            "multipart boundary missing".into()
        } else {
            format!("multipart/form-data with {} part(s)", parts.len())
        },
        text: Some(safe_truncate_utf8(text_out, max)),
        tree: json!({ "boundary": boundary, "parts": parts }),
        meta: json!({}),
    }
}

pub fn decode_sse(data: &[u8], max: usize) -> DecodeResult {
    let text = String::from_utf8_lossy(data);
    let mut events: Vec<Value> = Vec::new();
    let mut current = serde_json::Map::new();
    let mut data_lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            if !data_lines.is_empty() || !current.is_empty() {
                current.insert("data".into(), json!(data_lines.join("\n")));
                events.push(Value::Object(current.clone()));
                current.clear();
                data_lines.clear();
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            current.insert("event".into(), json!(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start().to_string());
        } else if let Some(rest) = line.strip_prefix("id:") {
            current.insert("id".into(), json!(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("retry:") {
            current.insert("retry".into(), json!(rest.trim()));
        }
    }
    if !data_lines.is_empty() {
        current.insert("data".into(), json!(data_lines.join("\n")));
        events.push(Value::Object(current));
    }
    DecodeResult {
        format: "sse".into(),
        detected: Some("text/event-stream".into()),
        summary: format!("Server-Sent Events: {} event(s)", events.len()),
        text: Some(safe_truncate_utf8(
            String::from_utf8_lossy(data).into_owned(),
            max,
        )),
        tree: json!({ "events": events }),
        meta: json!({}),
    }
}

pub fn decode_text(data: &[u8], max: usize) -> DecodeResult {
    let text = String::from_utf8_lossy(data).into_owned();
    DecodeResult {
        format: "text".into(),
        detected: Some(
            if is_probably_text(data) {
                "text"
            } else {
                "binary"
            }
            .into(),
        ),
        summary: format!("{} bytes of text", data.len()),
        text: Some(safe_truncate_utf8(text, max)),
        tree: json!({ "size": data.len() }),
        meta: json!({}),
    }
}

pub fn decode_base64_body(data: &[u8], max: usize) -> DecodeResult {
    let decoded = decode_base64(&String::from_utf8_lossy(data));
    let (text, tree, summary) = match decoded {
        Some(bytes) => (
            Some(safe_truncate_utf8(
                String::from_utf8_lossy(&bytes).into_owned(),
                max,
            )),
            json!({ "size": bytes.len(), "hex": hexdump(&bytes, 4096) }),
            format!("Base64 decoded to {} bytes", bytes.len()),
        ),
        None => (None, Value::Null, "Invalid base64 input".to_string()),
    };
    DecodeResult {
        format: "base64".into(),
        detected: Some("base64".into()),
        summary,
        text,
        tree,
        meta: json!({}),
    }
}

pub fn decode_hex_body(data: &[u8], max: usize) -> DecodeResult {
    let decoded = decode_hex(&String::from_utf8_lossy(data));
    let (text, tree, summary) = match decoded {
        Some(bytes) => (
            Some(hexdump(&bytes, max.min(1_000_000))),
            json!({ "size": bytes.len() }),
            format!("Hex decoded to {} bytes", bytes.len()),
        ),
        None => (None, Value::Null, "Invalid hex input".to_string()),
    };
    DecodeResult {
        format: "hex".into(),
        detected: Some("hex".into()),
        summary,
        text,
        tree,
        meta: json!({}),
    }
}

pub fn decode_raw(data: &[u8], max: usize) -> DecodeResult {
    let text = if is_probably_text(data) {
        Some(safe_truncate_utf8(
            String::from_utf8_lossy(data).into_owned(),
            max,
        ))
    } else {
        Some(hexdump(data, max.min(1_000_000)))
    };
    DecodeResult {
        format: "raw".into(),
        detected: Some(
            if is_probably_text(data) {
                "text"
            } else {
                "binary"
            }
            .into(),
        ),
        summary: format!("{} raw bytes", data.len()),
        text,
        tree: json!({ "size": data.len() }),
        meta: json!({ "hexdump": hexdump(data, 4096) }),
    }
}

pub fn decode_websocket(data: &[u8], max: usize) -> DecodeResult {
    let mut frames = Vec::new();
    let mut text_out = String::new();
    let mut pos = 0usize;
    while pos + 2 <= data.len() {
        let b0 = data[pos];
        let b1 = data[pos + 1];
        let fin = b0 & 0x80 != 0;
        let opcode = b0 & 0x0f;
        let masked = b1 & 0x80 != 0;
        let mut len = (b1 & 0x7f) as usize;
        let mut offset = pos + 2;
        if len == 126 {
            if offset + 2 > data.len() {
                break;
            }
            len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
            offset += 2;
        } else if len == 127 {
            if offset + 8 > data.len() {
                break;
            }
            len = u64::from_be_bytes(data[offset..offset + 8].try_into().unwrap()) as usize;
            offset += 8;
        }
        let mask = if masked {
            if offset + 4 > data.len() {
                break;
            }
            let m = [
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ];
            offset += 4;
            Some(m)
        } else {
            None
        };
        if offset + len > data.len() {
            break;
        }
        let mut payload = data[offset..offset + len].to_vec();
        if let Some(m) = mask {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= m[i % 4];
            }
        }
        let opcode_name = match opcode {
            0x0 => "continuation",
            0x1 => "text",
            0x2 => "binary",
            0x8 => "close",
            0x9 => "ping",
            0xa => "pong",
            _ => "reserved",
        };
        let text = String::from_utf8_lossy(&payload).into_owned();
        text_out.push_str(&format!(
            "[{} {} {} bytes{}]\n{}\n",
            opcode_name,
            if fin { "FIN" } else { "MORE" },
            len,
            if masked { " masked" } else { "" },
            short_preview(&payload, 4000)
        ));
        frames.push(json!({
            "fin": fin,
            "opcode": opcode,
            "opcode_name": opcode_name,
            "masked": masked,
            "length": len,
            "text": if opcode == 1 { Value::String(text) } else { Value::Null },
            "hex": payload.iter().take(256).map(|b| format!("{b:02x}")).collect::<String>(),
        }));
        pos = offset + len;
    }
    DecodeResult {
        format: "websocket".into(),
        detected: Some("websocket".into()),
        summary: format!("{} WebSocket frame(s)", frames.len()),
        text: Some(safe_truncate_utf8(text_out, max)),
        tree: json!({ "frames": frames }),
        meta: json!({}),
    }
}

pub fn decode_tls(data: &[u8], max: usize) -> DecodeResult {
    let mut records = Vec::new();
    let mut pos = 0usize;
    let mut sni = None;
    let mut text = String::new();
    while pos + 5 <= data.len() {
        let content_type = data[pos];
        let version = u16::from_be_bytes([data[pos + 1], data[pos + 2]]);
        let len = u16::from_be_bytes([data[pos + 3], data[pos + 4]]) as usize;
        pos += 5;
        if pos + len > data.len() {
            text.push_str(&format!("truncated TLS record of {len} bytes\n"));
            break;
        }
        let payload = &data[pos..pos + len];
        let name = match content_type {
            20 => "change_cipher_spec",
            21 => "alert",
            22 => "handshake",
            23 => "application_data",
            _ => "unknown",
        };
        let mut handshake = Value::Null;
        if content_type == 22 && !payload.is_empty() {
            let hs_type = payload[0];
            let hs_len = if payload.len() >= 4 {
                ((payload[1] as usize) << 16) | ((payload[2] as usize) << 8) | payload[3] as usize
            } else {
                0
            };
            handshake = json!({ "type": hs_type, "length": hs_len });
            if hs_type == 1 {
                if let Some(server_name) = parse_client_hello_sni(payload) {
                    text.push_str(&format!("SNI: {server_name}\n"));
                    sni = Some(server_name);
                }
            }
        }
        records.push(json!({
            "content_type": content_type,
            "type": name,
            "version": format!("0x{version:04x}"),
            "length": len,
            "handshake": handshake,
        }));
        pos += len;
        if records.len() > 5000 {
            break;
        }
    }
    if records.is_empty() {
        text.push_str("No TLS records found. The body may be decrypted application data.\n");
    } else {
        text.push_str(&format!("{} TLS record(s)\n", records.len()));
    }
    DecodeResult {
        format: "tls".into(),
        detected: Some("tls".into()),
        summary: format!(
            "{} TLS record(s){}",
            records.len(),
            sni.as_ref()
                .map(|s| format!(", SNI {s}"))
                .unwrap_or_default()
        ),
        text: Some(safe_truncate_utf8(text, max)),
        tree: json!({ "records": records, "sni": sni }),
        meta: json!({}),
    }
}

pub fn parse_client_hello_sni(data: &[u8]) -> Option<String> {
    // data starts with the handshake type byte (0x01).
    if data.len() < 4 || data[0] != 1 {
        return None;
    }
    let len = ((data[1] as usize) << 16) | ((data[2] as usize) << 8) | data[3] as usize;
    let body = data.get(4..4 + len.min(data.len().saturating_sub(4)))?;
    if body.len() < 34 {
        return None;
    }
    let mut p = 34usize;
    let sid_len = *body.get(p)? as usize;
    p += 1 + sid_len;
    if p + 2 > body.len() {
        return None;
    }
    let cs_len = u16::from_be_bytes([body[p], body[p + 1]]) as usize;
    p += 2 + cs_len;
    if p >= body.len() {
        return None;
    }
    let comp_len = body[p] as usize;
    p += 1 + comp_len;
    if p + 2 > body.len() {
        return None;
    }
    let ext_len = u16::from_be_bytes([body[p], body[p + 1]]) as usize;
    p += 2;
    let end = (p + ext_len).min(body.len());
    while p + 4 <= end {
        let ext_type = u16::from_be_bytes([body[p], body[p + 1]]);
        let elen = u16::from_be_bytes([body[p + 2], body[p + 3]]) as usize;
        p += 4;
        if p + elen > end {
            break;
        }
        if ext_type == 0 && elen >= 5 {
            let list_len = u16::from_be_bytes([body[p], body[p + 1]]) as usize;
            let mut q = p + 2;
            let list_end = (q + list_len).min(p + elen);
            while q + 3 <= list_end {
                let name_type = body[q];
                let name_len = u16::from_be_bytes([body[q + 1], body[q + 2]]) as usize;
                q += 3;
                if q + name_len > list_end {
                    break;
                }
                if name_type == 0 {
                    return String::from_utf8(body[q..q + name_len].to_vec()).ok();
                }
                q += name_len;
            }
        }
        p += elen;
    }
    None
}

pub fn decode_base64_string(data: &[u8]) -> String {
    B64.encode(data)
}
