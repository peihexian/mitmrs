use crate::decode::protobuf::{decode_message, text_dump, tree_value};
use crate::decode::util::{decode_base64, decompress, is_probably_text, printable_ratio};
use crate::model::DecodeResult;
use serde_json::{json, Value};

pub fn decode_grpc(
    data: &[u8],
    content_type: Option<&str>,
    grpc_encoding: Option<&str>,
    is_web: bool,
    max_bytes: usize,
) -> DecodeResult {
    let mut notes: Vec<String> = Vec::new();
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    let web_text = is_web && ct.contains("grpc-web-text");
    let mut body = data.to_vec();
    if web_text {
        let s = String::from_utf8_lossy(data);
        match decode_base64(&s) {
            Some(decoded) => {
                notes.push("decoded gRPC-Web base64 body".into());
                body = decoded;
            }
            None => notes.push("body looked like gRPC-Web text but base64 decoding failed".into()),
        }
    }
    let encoding = grpc_encoding.unwrap_or("identity");
    let mut frames: Vec<Value> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut pos = 0usize;
    let mut count = 0usize;
    let mut compressed_count = 0usize;
    while pos + 5 <= body.len() {
        let flag = body[pos];
        let len = u32::from_be_bytes([body[pos + 1], body[pos + 2], body[pos + 3], body[pos + 4]])
            as usize;
        pos += 5;
        if len > body.len().saturating_sub(pos) {
            notes.push(format!(
                "truncated frame: declared {len} bytes, only {} available",
                body.len() - pos
            ));
            break;
        }
        let payload = &body[pos..pos + len];
        pos += len;
        count += 1;
        let is_trailer = flag & 0x80 != 0;
        if is_trailer {
            let text = String::from_utf8_lossy(payload).into_owned();
            notes.push("gRPC-Web trailer frame".into());
            frames.push(json!({
                "index": count,
                "kind": "trailers",
                "flag": flag,
                "length": len,
                "text": text,
            }));
            texts.push(format!("[trailer frame]\n{text}"));
            continue;
        }
        let compressed = flag & 1 != 0;
        if compressed {
            compressed_count += 1;
        }
        let decoded_payload = if compressed {
            match decompress(payload, encoding, max_bytes) {
                Ok(v) => v,
                Err(e) => {
                    notes.push(format!("message {count}: {e}"));
                    payload.to_vec()
                }
            }
        } else {
            payload.to_vec()
        };
        let mut message_json = serde_json::Map::new();
        message_json.insert("index".into(), json!(count));
        message_json.insert("kind".into(), json!("message"));
        message_json.insert("flag".into(), json!(flag));
        message_json.insert("length".into(), json!(len));
        message_json.insert("compressed".into(), json!(compressed));
        let mut text = String::new();
        match decode_message(&decoded_payload) {
            Ok(fields) if !fields.is_empty() => {
                text_dump(&fields, 0, &mut text);
                message_json.insert("protobuf".into(), tree_value(&fields, 0));
                message_json.insert("text".into(), json!(text));
            }
            _ => {
                if is_probably_text(&decoded_payload) && printable_ratio(&decoded_payload) > 0.85 {
                    let s = String::from_utf8_lossy(&decoded_payload).into_owned();
                    message_json.insert("text".into(), json!(s.clone()));
                    text.push_str(&s);
                } else {
                    let hex: String = decoded_payload
                        .iter()
                        .take(512)
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    message_json.insert("hex".into(), json!(hex.clone()));
                    text.push_str(&format!("[{} bytes] {hex}", decoded_payload.len()));
                }
                message_json.insert("protobuf_error".into(), json!(true));
            }
        }
        text = format!(
            "message #{count} ({} bytes)\n{}",
            decoded_payload.len(),
            text
        );
        texts.push(text);
        frames.push(Value::Object(message_json));
    }
    if pos < body.len() {
        notes.push(format!(
            "{} trailing bytes after last gRPC frame",
            body.len() - pos
        ));
    }
    if frames.is_empty() {
        notes.push("no gRPC length-prefixed frames found; trying Protobuf message directly".into());
        if let Ok(fields) = decode_message(&body) {
            if !fields.is_empty() {
                let mut text = String::new();
                text_dump(&fields, 0, &mut text);
                frames.push(json!({
                    "index": 1,
                    "kind": "message",
                    "length": body.len(),
                    "compressed": false,
                    "protobuf": tree_value(&fields, 0),
                    "text": text,
                }));
                count = 1;
            }
        }
    }
    let kind = if count > 1 { "stream" } else { "unary" };
    let summary = format!(
        "{} gRPC {} ({} message{}, {}{})",
        count,
        kind,
        count,
        if count == 1 { "" } else { "s" },
        if compressed_count > 0 {
            format!("{compressed_count} compressed, ")
        } else {
            String::new()
        },
        if is_web { "gRPC-Web" } else { "gRPC" }
    );
    DecodeResult {
        format: "grpc".into(),
        detected: Some(if is_web { "grpc-web" } else { "grpc" }.into()),
        summary,
        text: Some(texts.join("\n\n")),
        tree: json!({
            "frames": frames,
            "message_count": count,
            "notes": notes,
        }),
        meta: json!({
            "content_type": content_type,
            "grpc_encoding": encoding,
            "grpc_web": is_web,
            "message_count": count,
            "compressed_messages": compressed_count,
        }),
    }
}
