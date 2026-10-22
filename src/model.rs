use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
}

impl HeaderEntry {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

pub fn header_entries(headers: &http::HeaderMap) -> Vec<HeaderEntry> {
    headers
        .iter()
        .map(|(name, value)| HeaderEntry {
            name: name.as_str().to_string(),
            value: String::from_utf8_lossy(value.as_bytes()).into_owned(),
        })
        .collect()
}

pub fn headers_to_map(headers: &[HeaderEntry]) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for h in headers {
        map.entry(h.name.to_ascii_lowercase())
            .or_default()
            .push(h.value.clone());
    }
    map
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowSummary {
    pub id: String,
    pub seq: i64,
    pub started_at: DateTime<Utc>,
    pub duration_ms: Option<i64>,
    pub client: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub method: String,
    pub path: String,
    pub http_version: String,
    pub status: Option<u16>,
    pub request_size: u64,
    pub response_size: u64,
    pub request_mime: Option<String>,
    pub response_mime: Option<String>,
    pub error: Option<String>,
    pub protocol: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BodyView {
    pub headers: Vec<HeaderEntry>,
    pub trailers: Vec<HeaderEntry>,
    pub body_size: u64,
    pub truncated: bool,
    pub body_base64: Option<String>,
    pub body_text: Option<String>,
    pub body_encoding: Option<String>,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowDetail {
    #[serde(flatten)]
    pub summary: FlowSummary,
    pub request: BodyView,
    pub response: BodyView,
    pub tls_sni: Option<String>,
    pub error_detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FlowRecord {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub client: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub http_version: String,
    pub status: Option<u16>,
    pub reason: Option<String>,
    pub request_headers: Vec<HeaderEntry>,
    pub response_headers: Vec<HeaderEntry>,
    pub request_trailers: Vec<HeaderEntry>,
    pub response_trailers: Vec<HeaderEntry>,
    pub request_body: Vec<u8>,
    pub response_body: Vec<u8>,
    pub request_body_size: u64,
    pub response_body_size: u64,
    pub request_truncated: bool,
    pub response_truncated: bool,
    pub request_mime: Option<String>,
    pub response_mime: Option<String>,
    pub error: Option<String>,
    pub tls_sni: Option<String>,
    pub protocol: String,
    pub tags: Vec<String>,
}

impl FlowRecord {
    pub fn new(
        id: String,
        started_at: DateTime<Utc>,
        client: String,
        scheme: String,
        host: String,
        port: u16,
        method: String,
        path: String,
        query: Option<String>,
        http_version: String,
        tls_sni: Option<String>,
    ) -> Self {
        Self {
            id,
            started_at,
            ended_at: None,
            duration_ms: None,
            client,
            scheme,
            host,
            port,
            method,
            path,
            query,
            http_version,
            status: None,
            reason: None,
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            request_trailers: Vec::new(),
            response_trailers: Vec::new(),
            request_body: Vec::new(),
            response_body: Vec::new(),
            request_body_size: 0,
            response_body_size: 0,
            request_truncated: false,
            response_truncated: false,
            request_mime: None,
            response_mime: None,
            error: None,
            tls_sni,
            protocol: "http".into(),
            tags: Vec::new(),
        }
    }

    pub fn summary(&self, seq: i64) -> FlowSummary {
        FlowSummary {
            id: self.id.clone(),
            seq,
            started_at: self.started_at,
            duration_ms: self.duration_ms,
            client: self.client.clone(),
            scheme: self.scheme.clone(),
            host: self.host.clone(),
            port: self.port,
            method: self.method.clone(),
            path: self.path.clone(),
            http_version: self.http_version.clone(),
            status: self.status,
            request_size: self.request_body_size,
            response_size: self.response_body_size,
            request_mime: self.request_mime.clone(),
            response_mime: self.response_mime.clone(),
            error: self.error.clone(),
            protocol: self.protocol.clone(),
            tags: self.tags.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub host: Option<String>,
    pub method: Option<String>,
    pub status: Option<String>,
    pub protocol: Option<String>,
    pub mime: Option<String>,
    pub body_contains: Option<String>,
    pub direction: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub has_error: Option<bool>,
    pub tag: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub order: Option<String>,
}

impl SearchQuery {
    pub fn normalized_limit(&self) -> usize {
        self.limit.unwrap_or(100).clamp(1, 500)
    }
    pub fn normalized_offset(&self) -> usize {
        self.offset.unwrap_or(0).min(10_000_000)
    }
    pub fn descending(&self) -> bool {
        !matches!(self.order.as_deref(), Some("asc"))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub items: Vec<FlowSummary>,
    pub total: i64,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostCount {
    pub host: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusClassCount {
    pub class: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub total: i64,
    pub hosts: Vec<HostCount>,
    pub status_classes: Vec<StatusClassCount>,
    pub bytes: ByteTotals,
    pub first: Option<DateTime<Utc>>,
    pub last: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ByteTotals {
    pub request: i64,
    pub response: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyStatusView {
    pub state: String,
    pub host: String,
    pub port: u16,
    pub error: Option<String>,
    pub active_connections: u64,
    pub total_connections: u64,
    pub windivert: WinDivertStatusView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinDivertStatusView {
    pub enabled: bool,
    pub state: String,
    pub error: Option<String>,
    pub packets: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreStatusView {
    pub flows: i64,
    pub bytes: i64,
    pub db_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaStatusView {
    pub subject: String,
    pub fingerprint: String,
    pub path: String,
    pub pem_url: String,
    pub not_before: String,
    pub not_after: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpStatusView {
    pub endpoint: String,
    pub stdio_command: String,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfigView {
    pub max_body_bytes: u64,
    pub capture_enabled: bool,
    pub upstream_proxy_enabled: bool,
    pub upstream_proxy_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub proxy: ProxyStatusView,
    pub store: StoreStatusView,
    pub ca: CaStatusView,
    pub mcp: McpStatusView,
    pub config: RuntimeConfigView,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigPatch {
    pub port: Option<u16>,
    pub max_body_bytes: Option<u64>,
    pub capture_enabled: Option<bool>,
    pub upstream_proxy_enabled: Option<bool>,
    pub upstream_proxy_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodeResult {
    pub format: String,
    pub detected: Option<String>,
    pub summary: String,
    pub text: Option<String>,
    pub tree: serde_json::Value,
    pub meta: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecoderOption {
    pub name: String,
    pub r#type: String,
    pub description: String,
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecoderInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content_types: Vec<String>,
    pub options: Vec<DecoderOption>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DecodeRequest {
    pub direction: String,
    pub format: Option<String>,
    #[serde(default)]
    pub options: serde_json::Value,
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub ok: bool,
    pub imported: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiErrorBody {
    pub error: String,
    pub detail: Option<String>,
}
