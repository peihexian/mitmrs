use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use mitmrs::decode;
use mitmrs::model::{FlowRecord, HeaderEntry};
use serde_json::json;
use std::io::Write;

fn record(req_body: &[u8], resp_body: &[u8]) -> FlowRecord {
    let mut rec = FlowRecord::new(
        "test-flow".into(),
        Utc::now(),
        "127.0.0.1:12345".into(),
        "https".into(),
        "api.example.com".into(),
        443,
        "POST".into(),
        "/pkg.Service/Call".into(),
        None,
        "HTTP/2.0".into(),
        Some("api.example.com".into()),
    );
    rec.request_headers = vec![HeaderEntry::new("content-type", "application/grpc")];
    rec.response_headers = vec![HeaderEntry::new("content-type", "application/grpc")];
    rec.request_body = req_body.to_vec();
    rec.request_body_size = req_body.len() as u64;
    rec.response_body = resp_body.to_vec();
    rec.response_body_size = resp_body.len() as u64;
    rec.protocol = "grpc".into();
    rec
}

fn grpc_frame(flag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![flag];
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

#[test]
fn decodes_grpc_protobuf_without_schema() {
    let payload = b"\x0a\x05hello\x10\x2a";
    let rec = record(b"", &grpc_frame(0, payload));
    let result = decode::decode_record(&rec, "response", "grpc", &json!({}), 1_000_000).unwrap();
    assert_eq!(result.format, "grpc");
    assert!(result.summary.contains("1 gRPC"));
    let frames = result.tree["frames"].as_array().unwrap();
    let protobuf = frames[0]["protobuf"].as_array().unwrap();
    assert_eq!(protobuf[0]["field"], 1);
    assert_eq!(protobuf[0]["string"], "hello");
    assert_eq!(protobuf[1]["field"], 2);
    assert_eq!(protobuf[1]["int64"], 42);
    assert!(protobuf[0].get("packed").is_none());
    assert!(result.text.unwrap().contains("hello"));
}

#[test]
fn decodes_compressed_grpc_message() {
    let payload = b"\x0a\x05hello\x10\x2a";
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(payload).unwrap();
    let compressed = encoder.finish().unwrap();
    let frame = grpc_frame(1, &compressed);
    let mut rec = record(b"", &frame);
    rec.response_headers
        .push(HeaderEntry::new("grpc-encoding", "gzip"));
    let result = decode::decode_record(&rec, "response", "grpc", &json!({}), 1_000_000).unwrap();
    assert!(result.summary.contains("1 compressed"));
    let protobuf = result.tree["frames"][0]["protobuf"].as_array().unwrap();
    assert_eq!(protobuf[1]["int64"], 42);
}

#[test]
fn auto_decompresses_gzip_json() {
    let raw = br#"{"gzip":true,"message":"hello"}"#;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw).unwrap();
    let gz = encoder.finish().unwrap();
    let mut rec = record(b"", &gz);
    rec.response_headers = vec![
        HeaderEntry::new("content-type", "application/json"),
        HeaderEntry::new("content-encoding", "gzip"),
    ];
    rec.response_mime = Some("application/json".into());
    let result = decode::decode_record(&rec, "response", "auto", &json!({}), 1_000_000).unwrap();
    assert_eq!(result.detected.as_deref(), Some("json"));
    assert_eq!(result.tree["gzip"], true);
    assert_eq!(result.tree["message"], "hello");
}

#[test]
fn decodes_nested_protobuf_and_packed_values() {
    // field 3 (message) { field 1: "abc" }, field 4 (packed): [1,2,300]
    let inner = b"\x0a\x03abc";
    let mut msg = Vec::new();
    msg.extend_from_slice(&[0x1a, inner.len() as u8]);
    msg.extend_from_slice(inner);
    msg.extend_from_slice(&[0x22, 4, 0x01, 0x02, 0xac, 0x02]);
    let rec = record(b"", &msg);
    let result =
        decode::decode_record(&rec, "response", "protobuf", &json!({}), 1_000_000).unwrap();
    let fields = result.tree["fields"].as_array().unwrap();
    assert_eq!(fields[0]["field"], 3);
    assert_eq!(fields[0]["message"][0]["string"], "abc");
    assert_eq!(fields[1]["packed"], json!([1, 2, 300]));
}

#[test]
fn decodes_gbk_text() {
    // "中文" in GBK.
    let rec = {
        let mut rec = record(b"", b"\xd6\xd0\xce\xc4");
        rec.response_headers = vec![HeaderEntry::new("content-type", "text/plain; charset=gbk")];
        rec
    };
    let result = decode::decode_record(&rec, "response", "text", &json!({}), 1_000_000).unwrap();
    assert_eq!(result.text.unwrap(), "中文");
}

#[test]
fn parses_tls_client_hello_sni() {
    let sni = b"api.example.com";
    let mut sni_ext = Vec::new();
    sni_ext.extend_from_slice(&((sni.len() + 3) as u16).to_be_bytes());
    sni_ext.push(0);
    sni_ext.extend_from_slice(&(sni.len() as u16).to_be_bytes());
    sni_ext.extend_from_slice(sni);
    let mut ext = Vec::new();
    ext.extend_from_slice(&0u16.to_be_bytes());
    ext.extend_from_slice(&(sni_ext.len() as u16).to_be_bytes());
    ext.extend_from_slice(&sni_ext);
    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]);
    body.extend_from_slice(&[0u8; 32]);
    body.push(0); // session id
    body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]); // cipher suites
    body.extend_from_slice(&[0x01, 0x00]); // compression
    body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
    body.extend_from_slice(&ext);
    let mut hs = vec![0x01];
    hs.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    hs.extend_from_slice(&body);
    assert_eq!(
        decode::formats::parse_client_hello_sni(&hs).as_deref(),
        Some("api.example.com")
    );
}

#[test]
fn parses_client_hello_metadata() {
    let sni = b"api.example.com";
    let mut sni_ext = Vec::new();
    sni_ext.extend_from_slice(&((sni.len() + 3) as u16).to_be_bytes());
    sni_ext.push(0);
    sni_ext.extend_from_slice(&(sni.len() as u16).to_be_bytes());
    sni_ext.extend_from_slice(sni);

    let mut alpn_list = Vec::new();
    for proto in [b"h2".as_slice(), b"http/1.1".as_slice()] {
        alpn_list.push(proto.len() as u8);
        alpn_list.extend_from_slice(proto);
    }
    let mut alpn_ext = Vec::new();
    alpn_ext.extend_from_slice(&(alpn_list.len() as u16).to_be_bytes());
    alpn_ext.extend_from_slice(&alpn_list);

    let mut versions_ext = vec![4u8];
    versions_ext.extend_from_slice(&0x0304u16.to_be_bytes());
    versions_ext.extend_from_slice(&0x0303u16.to_be_bytes());

    let mut ext = Vec::new();
    for (kind, body) in [(0u16, sni_ext), (16u16, alpn_ext), (43u16, versions_ext)] {
        ext.extend_from_slice(&kind.to_be_bytes());
        ext.extend_from_slice(&(body.len() as u16).to_be_bytes());
        ext.extend_from_slice(&body);
    }

    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]);
    body.extend_from_slice(&[0u8; 32]);
    body.push(0); // session id
    body.extend_from_slice(&[0x00, 0x06, 0x13, 0x01, 0x13, 0x02, 0x13, 0x03]);
    body.extend_from_slice(&[0x01, 0x00]); // compression
    body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
    body.extend_from_slice(&ext);
    let mut hs = vec![0x01];
    hs.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
    hs.extend_from_slice(&body);

    let info = decode::formats::parse_client_hello(&hs).expect("client hello should parse");
    assert_eq!(info.sni.as_deref(), Some("api.example.com"));
    assert_eq!(info.alpn, vec!["h2".to_string(), "http/1.1".to_string()]);
    assert_eq!(
        info.tls_versions,
        vec!["1.3".to_string(), "1.2".to_string()]
    );
    assert_eq!(info.cipher_count, 3);
    assert_eq!(info.extension_count, 3);
    assert_eq!(info.alpn_text(), "h2,http/1.1");
    assert!(decode::formats::parse_client_hello(&hs[..hs.len() - 1]).is_none());
}

fn gzip_bytes(raw: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(raw).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn auto_decodes_wire_body_from_header_or_magic() {
    let raw = br#"{"gzip":true,"note":"wire decode"}"#;
    let gz = gzip_bytes(raw);

    let from_header = decode::decode_wire_body(&gz, Some("gzip"), 1_000_000)
        .expect("declared gzip should decode");
    assert_eq!(from_header.encoding, "gzip");
    assert_eq!(from_header.data, raw);
    assert!(!from_header.truncated);

    // Captures imported from HAR or older builds may have lost the header.
    let sniffed =
        decode::decode_wire_body(&gz, None, 1_000_000).expect("gzip magic should be sniffed");
    assert_eq!(sniffed.encoding, "gzip");
    assert_eq!(sniffed.data, raw);

    // Plaintext must be left untouched.
    assert!(decode::decode_wire_body(raw, None, 1_000_000).is_none());
    assert!(decode::decode_wire_body(raw, Some("identity"), 1_000_000).is_none());
}

#[test]
fn auto_decodes_stacked_content_encoding() {
    let raw = br#"{"stacked":true}"#;
    // Content-Encoding: zstd, gzip means zstd was applied first, then gzip.
    let zstd = zstd::encode_all(&raw[..], 3).unwrap();
    let wire = gzip_bytes(&zstd);
    let found = decode::decode_wire_body(&wire, Some("zstd, gzip"), 1_000_000)
        .expect("stacked encodings should decode");
    assert_eq!(found.data, raw);
    assert_eq!(found.encoding, "gzip + zstd");
}

#[test]
fn keeps_prefix_of_truncated_gzip_body() {
    let raw = vec![b'a'; 64 * 1024];
    let gz = gzip_bytes(&raw);
    let cut = &gz[..gz.len() * 2 / 3];
    let found =
        decode::decode_wire_body(cut, Some("gzip"), 1_000_000).expect("partial gzip should decode");
    assert!(!found.data.is_empty());
    assert!(found.truncated);
}

#[test]
fn decodes_form_and_websocket() {
    let rec = record(b"name=%E5%BC%A0%E4%B8%89&n=42", b"");
    let result = decode::decode_record(&rec, "request", "form", &json!({}), 1_000_000).unwrap();
    assert_eq!(result.tree["fields"][0]["value"], "张三");

    let mut frame = vec![0x81, 0x05];
    frame.extend_from_slice(b"hello");
    let rec = record(b"", &frame);
    let result =
        decode::decode_record(&rec, "response", "websocket", &json!({}), 1_000_000).unwrap();
    assert_eq!(result.tree["frames"][0]["opcode_name"], "text");
    assert_eq!(result.tree["frames"][0]["text"], "hello");
}
