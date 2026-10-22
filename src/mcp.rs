use crate::control;
use crate::decode;
use crate::model::*;
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub fn tool_names() -> Vec<String> {
    tool_descriptors()
        .iter()
        .filter_map(|t| t["name"].as_str().map(|s| s.to_string()))
        .collect()
}

pub fn tool_descriptors() -> Vec<Value> {
    vec![
        tool(
            "traffic_search",
            "Search captured HTTP/HTTPS/gRPC flows. Returns summaries only (no bodies). Use traffic_get or traffic_decode for content. Supports substring search across host, path, headers and bodies, plus structured filters. This is the primary discovery tool for an AI agent.",
            json!({
                "type": "object",
                "properties": {
                    "q": {"type": "string", "description": "Free-text substring across host, path, method, headers and captured bodies."},
                    "host": {"type": "string", "description": "Host filter, substring match."},
                    "method": {"type": "string", "description": "HTTP method, exact (GET, POST, ...)."},
                    "status": {"type": "string", "description": "Status filter: 200, 4xx, 5xx, error."},
                    "protocol": {"type": "string", "description": "http, http2, grpc, sse, ndjson, websocket, tls, other, or a custom stored protocol."},
                    "mime": {"type": "string", "description": "Request or response MIME substring, e.g. json or grpc."},
                    "body_contains": {"type": "string", "description": "Substring inside request and/or response body text."},
                    "direction": {"type": "string", "enum": ["both", "request", "response"], "description": "Limit body_contains to one direction."},
                    "since": {"type": "string", "description": "RFC3339 lower bound on started_at."},
                    "until": {"type": "string", "description": "RFC3339 upper bound on started_at."},
                    "has_error": {"type": "boolean", "description": "Only flows with a capture/upstream error."},
                    "tag": {"type": "string", "description": "Flow tag: transparent, tls-failed, tls-passthrough, client-rejected-cert."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 50},
                    "offset": {"type": "integer", "minimum": 0, "default": 0},
                    "order": {"type": "string", "enum": ["asc", "desc"], "default": "desc"}
                }
            }),
        ),
        tool(
            "traffic_recent",
            "List the most recent captured flows without building a query. Newest first, default 50, maximum 500 per call. Use next_offset to fetch older pages; total tells you how many matching flows exist. This is the quick 'show me recent traffic' tool for agents.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "minimum": 1, "maximum": 500, "default": 50},
                    "offset": {"type": "integer", "minimum": 0, "default": 0},
                    "host": {"type": "string", "description": "Optional host substring filter."},
                    "method": {"type": "string", "description": "Optional HTTP method filter."},
                    "status": {"type": "string", "description": "Optional status filter: 200, 4xx, 5xx, error."},
                    "protocol": {"type": "string", "description": "Optional protocol filter: http, http2, grpc, sse, ndjson, websocket, tls, other."}
                }
            }),
        ),
        tool(
            "traffic_get",
            "Get one captured flow by id, including request and response headers, bodies and metadata. Use body='none' for metadata only. Set decode=true to also run the auto decoder on both directions.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Flow id from traffic_search."},
                    "body": {"type": "string", "enum": ["text", "base64", "both", "none"], "default": "text"},
                    "max_bytes": {"type": "integer", "minimum": 1024, "maximum": 33554432, "default": 1048576},
                    "decode": {"type": "boolean", "default": false, "description": "Include auto-decoded request and response bodies."},
                    "format": {"type": "string", "default": "auto", "description": "Decoder format when decode=true."}
                },
                "required": ["id"]
            }),
        ),
        tool(
            "traffic_body",
            "Read a single direction of a flow as raw text/base64/hex or decoded by a chosen format. Prefer traffic_decode when investigating gRPC or protobuf.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "direction": {"type": "string", "enum": ["request", "response"], "default": "response"},
                    "format": {"type": "string", "enum": ["auto", "text", "base64", "hex", "http", "http2", "grpc", "protobuf", "json", "xml", "form", "multipart", "sse", "websocket", "tls", "gzip", "raw"], "default": "auto"},
                    "max_bytes": {"type": "integer", "minimum": 256, "maximum": 8388608, "default": 1000000},
                    "options": {"type": "object", "description": "Decoder options, e.g. {\"grpc_encoding\":\"gzip\"}."}
                },
                "required": ["id"]
            }),
        ),
        tool(
            "traffic_decode",
            "Decode a captured request or response body with a specific format and return a structured tree plus text. The user/AI can switch formats: auto, http, http2, grpc, protobuf, json, xml, form, multipart, sse, websocket, tls, text, base64, hex, gzip, raw. This is the 'try different packet formats' tool.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "direction": {"type": "string", "enum": ["request", "response"], "default": "response"},
                    "format": {"type": "string", "default": "auto"},
                    "options": {"type": "object", "description": "Format-specific options. gRPC: grpc_encoding; HTTP/2: raw_frames; gzip: encoding."},
                    "max_bytes": {"type": "integer", "minimum": 256, "maximum": 8388608, "default": 1000000}
                },
                "required": ["id"]
            }),
        ),
        tool(
            "decoders_list",
            "List all built-in decoders, their content types and options. Call this before trying an unfamiliar format.",
            json!({"type": "object", "properties": {}}),
        ),
        tool(
            "traffic_stats",
            "Aggregate capture statistics: total flows, top hosts with counts, status classes and byte totals. Useful to orient before searching.",
            json!({"type": "object", "properties": {}}),
        ),
        tool(
            "traffic_export",
            "Export matching flows as MitmRs JSON (full fidelity, base64 bodies) or HAR 1.2. Returns the serialized document in data. Keep max_flows small unless the user asked for a full export.",
            json!({
                "type": "object",
                "properties": {
                    "format": {"type": "string", "enum": ["json", "har"], "default": "json"},
                    "max_flows": {"type": "integer", "minimum": 1, "maximum": 20000, "default": 500},
                    "host": {"type": "string"},
                    "q": {"type": "string"},
                    "method": {"type": "string"},
                    "status": {"type": "string"},
                    "protocol": {"type": "string"}
                }
            }),
        ),
        tool(
            "traffic_import",
            "Import a MitmRs JSON export, a JSON array of flows, or a HAR document from a string. Existing ids are preserved when possible; conflicts get new ids.",
            json!({
                "type": "object",
                "properties": {"data": {"type": "string", "description": "The JSON/HAR document text."}},
                "required": ["data"]
            }),
        ),
        tool(
            "traffic_clear",
            "Delete every captured flow. Requires confirm=true. Does not stop the proxy or delete the CA.",
            json!({
                "type": "object",
                "properties": {"confirm": {"type": "boolean", "const": true}},
                "required": ["confirm"]
            }),
        ),
        tool(
            "traffic_delete",
            "Delete one captured flow by id.",
            json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        ),
        tool(
            "proxy_control",
            "Read or change proxy lifecycle: status, start, stop, pause, resume, restart, set_config. Can also set proxy_port, max_body_bytes, capture_enabled, and the optional upstream chain proxy (upstream_proxy_enabled/upstream_proxy_url). Pausing rejects new requests but keeps captured data.",
            json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["status", "start", "stop", "pause", "resume", "restart", "set_config"], "default": "status"},
                    "port": {"type": "integer", "minimum": 1, "maximum": 65535, "description": "New proxy port; restarts the listener if running."},
                    "max_body_bytes": {"type": "integer", "minimum": 65536, "maximum": 268435456},
                    "capture_enabled": {"type": "boolean"},
                    "upstream_proxy_enabled": {"type": "boolean", "description": "Route MitmRs upstream traffic through another HTTP proxy (next hop)."},
                    "upstream_proxy_url": {"type": "string", "description": "Next-hop proxy address such as 127.0.0.1:7890 or http://127.0.0.1:7890. Only http:// proxies are supported; basic auth in the URL is accepted."}
                }
            }),
        ),
        tool(
            "ca_certificate",
            "Return the MitmRs local CA certificate in PEM, its SHA-256 fingerprint and Windows trust/install instructions. The client must trust this CA for HTTPS interception.",
            json!({"type": "object", "properties": {}}),
        ),
        tool(
            "mitmrs_help",
            "Explain the MitmRs MCP endpoint, proxy configuration, CA trust, available tools and recommended workflows for AI agents.",
            json!({"type": "object", "properties": {}}),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

pub fn handle_rpc(state: Arc<AppState>, payload: Value) -> Option<Value> {
    let id = payload.get("id").cloned();
    let method = payload.get("method").and_then(|v| v.as_str()).unwrap_or("");
    if method.is_empty() {
        return Some(rpc_error(id, -32600, "invalid JSON-RPC request", None));
    }
    if id.is_none() && method.starts_with("notifications/") {
        return None;
    }
    let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("2025-03-26");
            let version = match requested {
                "2024-11-05" | "2025-03-26" | "2025-06-18" => requested,
                _ => "2025-03-26",
            };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {
                    "tools": {"listChanged": false},
                    "resources": {"subscribe": false, "listChanged": false},
                    "prompts": {"listChanged": false},
                    "logging": {}
                },
                "serverInfo": {"name": "mitmrs", "title": "MitmRs AI-native MITM traffic lab", "version": crate::state::VERSION},
                "instructions": "MitmRs captures HTTP/HTTPS/gRPC through a local MITM proxy. Start with traffic_stats or traffic_search; use traffic_get for headers/bodies, traffic_decode to switch formats (grpc, protobuf, http, json, ...), and proxy_control to start/stop/pause or change the port. Trust the CA from ca_certificate before HTTPS capture."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_descriptors()})),
        "tools/call" => call_tool(&state, &params),
        "resources/list" => Ok(json!({
            "resources": [
                {"uri": "mitmrs://status", "name": "Proxy and capture status", "mimeType": "application/json"},
                {"uri": "mitmrs://ca", "name": "MitmRs CA certificate", "mimeType": "application/x-pem-file"},
                {"uri": "mitmrs://help", "name": "MitmRs MCP usage guide", "mimeType": "text/markdown"}
            ]
        })),
        "resources/read" => read_resource(&state, &params),
        "prompts/list" => Ok(json!({"prompts": []})),
        "logging/setLevel" => Ok(json!({})),
        "completion/complete" => Ok(json!({"completion": {"values": [], "hasMore": false}})),
        "shutdown" => Ok(Value::Null),
        _ => Err(((-32601), format!("method not found: {method}"))),
    };
    match result {
        Ok(value) => Some(json!({"jsonrpc": "2.0", "id": id, "result": value})),
        Err((code, message)) => Some(rpc_error(id, code, &message, None)),
    }
}

fn rpc_error(id: Option<Value>, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut err = json!({"code": code, "message": message});
    if let Some(data) = data {
        err["data"] = data;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": err})
}

fn call_tool(state: &Arc<AppState>, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or((-32602, "tools/call requires name".to_string()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = match name {
        "traffic_search" => tool_search(state, &args),
        "traffic_recent" => tool_recent(state, &args),
        "traffic_get" => tool_get(state, &args),
        "traffic_body" => tool_body(state, &args),
        "traffic_decode" => tool_decode(state, &args),
        "decoders_list" => Ok(json!({"items": decode::decoders()})),
        "traffic_stats" => tool_stats(state),
        "traffic_export" => tool_export(state, &args),
        "traffic_import" => tool_import(state, &args),
        "traffic_clear" => tool_clear(state, &args),
        "traffic_delete" => tool_delete(state, &args),
        "proxy_control" => tool_proxy(state, &args),
        "ca_certificate" => tool_ca(state),
        "mitmrs_help" => Ok(help_payload(state)),
        other => return Ok(tool_error(format!("unknown tool '{other}'"))),
    };
    match result {
        Ok(value) => Ok(tool_ok(value)),
        Err(e) => Ok(tool_error(e)),
    }
}

fn tool_ok(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    let text = if text.len() > 2 * 1024 * 1024 {
        format!(
            "{}\n… tool output truncated at 2 MB",
            &text[..2 * 1024 * 1024]
        )
    } else {
        text
    };
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": value,
        "isError": false
    })
}

fn tool_error(message: String) -> Value {
    json!({
        "content": [{"type": "text", "text": format!("MitmRs tool error: {message}")}],
        "isError": true
    })
}

fn search_from_args(args: &Value) -> SearchQuery {
    let s = |key: &str| {
        args.get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    SearchQuery {
        q: s("q"),
        host: s("host"),
        method: s("method"),
        status: s("status"),
        protocol: s("protocol"),
        mime: s("mime"),
        body_contains: s("body_contains"),
        direction: s("direction"),
        since: s("since"),
        until: s("until"),
        has_error: args.get("has_error").and_then(|v| v.as_bool()),
        tag: s("tag"),
        limit: args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize),
        offset: args
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize),
        order: s("order"),
    }
}

fn tool_search(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    search_value(state, &search_from_args(args))
}

fn tool_recent(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let mut q = search_from_args(args);
    q.order = Some("desc".into());
    if q.limit.is_none() {
        q.limit = Some(50);
    }
    search_value(state, &q)
}

fn search_value(state: &Arc<AppState>, q: &SearchQuery) -> Result<Value, String> {
    let result = block(state.store.search(q)).map_err(|e| format!("{e:#}"))?;
    let mut value = serde_json::to_value(&result).map_err(|e| e.to_string())?;
    value["next_offset"] = json!(
        if (result.offset + result.items.len()) < result.total as usize {
            Some(result.offset + result.items.len())
        } else {
            None
        }
    );
    Ok(value)
}

fn tool_get(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("traffic_get requires id")?;
    let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("text");
    let max = args
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(1_000_000)
        .clamp(1024, 32 * 1024 * 1024);
    let detail = block(state.store.get(id, body, max))
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("flow {id} not found"))?;
    let mut value = serde_json::to_value(&detail).map_err(|e| e.to_string())?;
    if args
        .get("decode")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let format = args
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("auto");
        let record = block(state.store.get_record(id))
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| format!("flow {id} not found"))?;
        let req = decode::decode_record(&record, "request", format, &json!({}), max)
            .map_err(|e| format!("decode request: {e}"))?;
        let resp = decode::decode_record(&record, "response", format, &json!({}), max)
            .map_err(|e| format!("decode response: {e}"))?;
        value["decoded"] = json!({"request": req, "response": resp});
    }
    Ok(value)
}

fn tool_body(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("traffic_body requires id")?;
    let direction = match args
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("response")
    {
        "request" => "request",
        _ => "response",
    };
    let format = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");
    let max = args
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(1_000_000)
        .clamp(256, 8 * 1024 * 1024);
    let record = block(state.store.get_record(id))
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("flow {id} not found"))?;
    let (headers, body, size, truncated) = if direction == "request" {
        (
            &record.request_headers,
            &record.request_body,
            record.request_body_size,
            record.request_truncated,
        )
    } else {
        (
            &record.response_headers,
            &record.response_body,
            record.response_body_size,
            record.response_truncated,
        )
    };
    let ct = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.clone());
    let enc = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-encoding"))
        .map(|h| h.value.clone());
    let mut out = json!({
        "id": id,
        "direction": direction,
        "size": size,
        "truncated": truncated,
        "content_type": ct,
        "content_encoding": enc,
        "format": format,
    });
    if format == "base64" {
        out["body_base64"] = json!(B64.encode(body));
    } else if format == "text" {
        out["body_text"] = json!(String::from_utf8_lossy(body));
    } else {
        let options = args.get("options").cloned().unwrap_or_else(|| json!({}));
        let decoded = decode::decode_record(&record, direction, format, &options, max)
            .map_err(|e| e.to_string())?;
        out["decoded"] = serde_json::to_value(decoded).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn tool_decode(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("traffic_decode requires id")?;
    let direction = match args
        .get("direction")
        .and_then(|v| v.as_str())
        .unwrap_or("response")
    {
        "request" => "request",
        _ => "response",
    };
    let format = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");
    let options = args.get("options").cloned().unwrap_or_else(|| json!({}));
    let max = args
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(1_000_000)
        .clamp(256, 8 * 1024 * 1024);
    let record = block(state.store.get_record(id))
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("flow {id} not found"))?;
    let decoded = decode::decode_record(&record, direction, format, &options, max)
        .map_err(|e| e.to_string())?;
    serde_json::to_value(decoded).map_err(|e| e.to_string())
}

fn tool_stats(state: &Arc<AppState>) -> Result<Value, String> {
    let stats = block(state.store.stats()).map_err(|e| format!("{e:#}"))?;
    serde_json::to_value(stats).map_err(|e| e.to_string())
}

fn tool_export(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let format = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("json");
    let max = args
        .get("max_flows")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(500)
        .clamp(1, 20_000);
    let q = search_from_args(args);
    let data = if format == "har" {
        block(state.store.export_har(&q, max))
    } else {
        block(state.store.export_json(&q, max))
    }
    .map_err(|e| format!("{e:#}"))?;
    Ok(json!({
        "format": if format == "har" { "har" } else { "json" },
        "max_flows": max,
        "bytes": data.len(),
        "data": data
    }))
}

fn tool_import(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let data = args
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or("traffic_import requires data")?;
    let result = block(state.store.import_json(data)).map_err(|e| format!("{e:#}"))?;
    if let Ok(stats) = block(state.store.stats()) {
        state.flow_count.store(stats.total, Ordering::Relaxed);
        state.bytes_total.store(
            stats.bytes.request + stats.bytes.response,
            Ordering::Relaxed,
        );
    }
    serde_json::to_value(result).map_err(|e| e.to_string())
}

fn tool_clear(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    if args.get("confirm").and_then(|v| v.as_bool()) != Some(true) {
        return Err("traffic_clear requires confirm=true".into());
    }
    let n = block(control::clear_traffic(state)).map_err(|e| format!("{e:#}"))?;
    Ok(json!({"ok": true, "deleted": n}))
}

fn tool_delete(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("traffic_delete requires id")?;
    let found = block(control::delete_flow(state, id)).map_err(|e| format!("{e:#}"))?;
    Ok(json!({"ok": found, "id": id}))
}

fn tool_proxy(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status");
    let patch = if args.get("port").is_some()
        || args.get("max_body_bytes").is_some()
        || args.get("capture_enabled").is_some()
        || args.get("upstream_proxy_enabled").is_some()
        || args.get("upstream_proxy_url").is_some()
    {
        Some(ConfigPatch {
            port: args.get("port").and_then(|v| v.as_u64()).map(|v| v as u16),
            max_body_bytes: args.get("max_body_bytes").and_then(|v| v.as_u64()),
            capture_enabled: args.get("capture_enabled").and_then(|v| v.as_bool()),
            upstream_proxy_enabled: args.get("upstream_proxy_enabled").and_then(|v| v.as_bool()),
            upstream_proxy_url: args
                .get("upstream_proxy_url")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string()),
        })
    } else {
        None
    };
    let effective = if action == "status" && patch.is_some() {
        "set_config"
    } else {
        action
    };
    let status = block(control::action(state, effective, patch)).map_err(|e| format!("{e:#}"))?;
    serde_json::to_value(status).map_err(|e| e.to_string())
}

fn tool_ca(state: &Arc<AppState>) -> Result<Value, String> {
    let path = state.ca.cert_path();
    Ok(json!({
        "pem": state.ca.pem(),
        "fingerprint_sha256": state.ca.fingerprint(),
        "path": path.display().to_string(),
        "not_before": state.ca.not_before().to_rfc3339(),
        "not_after": state.ca.not_after().to_rfc3339(),
        "download_url": format!("http://{}:{}/api/ca.crt", state.web_host, state.web_port),
        "windows_install": format!("certutil -addstore -f Root \"{}\"", path.display()),
        "note": "Run the certutil command from an elevated prompt, or import ca.crt into Firefox. Clients that do not trust this CA will reject intercepted TLS."
    }))
}

fn help_payload(state: &Arc<AppState>) -> Value {
    json!({
        "name": "MitmRs",
        "version": crate::state::VERSION,
        "endpoint": format!("http://{}:{}/mcp", state.web_host, state.web_port),
        "transport": "Streamable HTTP JSON-RPC 2.0",
        "proxy": format!("{}:{}", state.config.try_read().map(|c| c.proxy_host.clone()).unwrap_or_else(|_| "127.0.0.1".into()), state.config.try_read().map(|c| c.proxy_port).unwrap_or(8080)),
        "upstream_proxy": state.config.try_read().ok().and_then(|c| {
            if c.upstream_proxy_enabled && !c.upstream_proxy_url.is_empty() {
                Some(c.upstream_proxy_url.clone())
            } else {
                None
            }
        }),
        "workflow": [
            "1. ca_certificate -> install/trust the CA for HTTPS.",
            "2. proxy_control action=start (the proxy auto-starts on launch).",
            "3. Configure the client to use the proxy, or enable WinDivert transparent redirect.",
            "4. traffic_recent for the latest flows, or traffic_stats / traffic_search to discover and page through everything.",
            "5. traffic_get for headers and bodies, traffic_decode to switch formats.",
            "6. traffic_export for handoff; traffic_import to merge captures."
        ],
        "formats": decode::decoders().iter().map(|d| d.id.clone()).collect::<Vec<_>>(),
        "tips": [
            "gRPC bodies are length-prefixed; use format=grpc, then read frames[].protobuf.",
            "Use format=protobuf for raw schema-less protobuf.",
            "Use format=base64 when you need exact bytes for an external tool.",
            "body_contains only searches captured/truncated text; check max_body_bytes if results are missing.",
            "To chain through a local proxy, call proxy_control with upstream_proxy_enabled=true and upstream_proxy_url like 127.0.0.1:7890; the address is remembered while chaining is disabled."
        ]
    })
}

fn read_resource(state: &Arc<AppState>, params: &Value) -> Result<Value, (i64, String)> {
    let uri = params
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or((-32602, "resources/read requires uri".to_string()))?;
    match uri {
        "mitmrs://status" => {
            let status = block(state.status());
            Ok(
                json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": serde_json::to_string_pretty(&status).unwrap_or_default()}]}),
            )
        }
        "mitmrs://ca" => Ok(
            json!({"contents": [{"uri": uri, "mimeType": "application/x-pem-file", "text": state.ca.pem()}]}),
        ),
        "mitmrs://help" => Ok(
            json!({"contents": [{"uri": uri, "mimeType": "text/markdown", "text": serde_json::to_string_pretty(&help_payload(state)).unwrap_or_default()}]}),
        ),
        other => Err((-32602, format!("unknown resource {other}"))),
    }
}

fn block<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

pub async fn http_post(
    State(state): State<Arc<AppState>>,
    _headers: HeaderMap,
    body: Bytes,
) -> Response {
    let value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return json_rpc_response(
                StatusCode::BAD_REQUEST,
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}}),
                false,
            )
        }
    };
    if value.is_array() {
        let arr = value.as_array().cloned().unwrap_or_default();
        let mut out = Vec::new();
        let mut has_request = false;
        for item in arr {
            if let Some(resp) = handle_rpc(state.clone(), item) {
                has_request = true;
                out.push(resp);
            }
        }
        if !has_request {
            return StatusCode::ACCEPTED.into_response();
        }
        return json_rpc_response(StatusCode::OK, Value::Array(out), false);
    }
    let is_init = value.get("method").and_then(|v| v.as_str()) == Some("initialize");
    match handle_rpc(state, value) {
        Some(resp) => json_rpc_response(StatusCode::OK, resp, is_init),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

fn json_rpc_response(status: StatusCode, value: Value, initialize: bool) -> Response {
    let body = serde_json::to_vec(&value).unwrap_or_default();
    let mut resp = Response::new(axum::body::Body::from(body));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if initialize {
        let sid = uuid::Uuid::new_v4().to_string();
        if let Ok(v) = HeaderValue::from_str(&sid) {
            resp.headers_mut().insert("mcp-session-id", v);
        }
        resp.headers_mut().insert(
            "mcp-protocol-version",
            HeaderValue::from_static("2025-03-26"),
        );
    }
    resp
}

pub async fn http_get() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "POST, DELETE")],
        "This MitmRs MCP server uses Streamable HTTP POST. It has no server-initiated SSE stream.",
    )
        .into_response()
}

pub async fn http_delete() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

pub fn run_stdio(url: String) -> anyhow::Result<()> {
    use std::io::{BufRead, Write};
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let mut session: Option<String> = None;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let mut req = client
            .post(&url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(line.clone());
        if let Some(sid) = &session {
            req = req.header("mcp-session-id", sid);
        }
        let resp = match req.send() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("mitmrs mcp-stdio: request failed: {e}");
                continue;
            }
        };
        if let Some(sid) = resp.headers().get("mcp-session-id") {
            if let Ok(v) = sid.to_str() {
                session = Some(v.to_string());
            }
        }
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        if !status.is_success() {
            eprintln!("mitmrs mcp-stdio: HTTP {status}: {text}");
            continue;
        }
        // The server replies with plain JSON. If a proxy ever returns SSE, print
        // only the data payloads so the stdio client still sees valid messages.
        if text.trim_start().starts_with("event:") || text.trim_start().starts_with("data:") {
            for l in text.lines() {
                if let Some(data) = l.strip_prefix("data:") {
                    writeln!(stdout, "{}", data.trim())?;
                }
            }
        } else {
            writeln!(stdout, "{}", text.trim())?;
        }
        stdout.flush()?;
    }
    Ok(())
}

#[allow(dead_code)]
fn _json_marker() {
    let _ = Json(Value::Null);
}
