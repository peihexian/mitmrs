use crate::ca::CertAuthority;
use crate::config::{FileConfig, RuntimeConfig};
use crate::model::*;
use crate::store::Store;
use crate::transparent::TransparentMap;
use crate::upstream::UpstreamClient;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
pub struct ServerEvent {
    pub event: String,
    pub data: serde_json::Value,
}

#[derive(Debug)]
pub struct ProxyRuntime {
    pub state: String,
    pub error: Option<String>,
    pub cancel: Option<CancellationToken>,
    pub task: Option<JoinHandle<()>>,
}

impl Default for ProxyRuntime {
    fn default() -> Self {
        Self {
            state: "stopped".into(),
            error: None,
            cancel: None,
            task: None,
        }
    }
}

#[derive(Debug)]
pub struct WinDivertRuntime {
    pub enabled: bool,
    pub state: String,
    pub error: Option<String>,
    pub packets: Arc<AtomicU64>,
    /// Set before the driver is shut down; the worker re-injects any packets
    /// still queued in the driver instead of rewriting them.
    pub stop_requested: Arc<AtomicBool>,
    /// Notified once the worker has closed the WinDivert handle.
    pub done: Arc<tokio::sync::Notify>,
    #[cfg(windows)]
    pub shutdown: Arc<std::sync::Mutex<Option<windivert::prelude::ShutdownHandle>>>,
    #[cfg(windows)]
    pub thread: Option<std::thread::JoinHandle<()>>,
}

impl Default for WinDivertRuntime {
    fn default() -> Self {
        Self {
            enabled: false,
            state: "off".into(),
            error: None,
            packets: Arc::new(AtomicU64::new(0)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            done: Arc::new(tokio::sync::Notify::new()),
            #[cfg(windows)]
            shutdown: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(windows)]
            thread: None,
        }
    }
}

pub struct AppState {
    pub store: Store,
    pub ca: Arc<CertAuthority>,
    pub upstream: RwLock<UpstreamClient>,
    pub config: RwLock<RuntimeConfig>,
    pub started_at: DateTime<Utc>,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub events: broadcast::Sender<ServerEvent>,
    pub proxy: Mutex<ProxyRuntime>,
    pub active_connections: AtomicU64,
    pub total_connections: AtomicU64,
    pub flow_count: AtomicI64,
    pub bytes_total: AtomicI64,
    pub capture_state: RwLock<String>,
    pub windivert: RwLock<WinDivertRuntime>,
    pub web_host: String,
    pub web_port: u16,
    pub insecure_upstream: bool,
    pub transparent: Arc<TransparentMap>,
    /// Process-wide shutdown signal. Cancelled on Ctrl+C so long-lived
    /// responses (SSE) stop streaming and the HTTP server can drain.
    pub shutdown: CancellationToken,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        store: Store,
        ca: Arc<CertAuthority>,
        upstream: UpstreamClient,
        config: RuntimeConfig,
        data_dir: PathBuf,
        db_path: PathBuf,
        web_host: String,
        web_port: u16,
        insecure_upstream: bool,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(2048);
        let stats = store.stats().await.unwrap_or_else(|_| Stats {
            total: 0,
            hosts: vec![],
            status_classes: vec![],
            bytes: ByteTotals::default(),
            first: None,
            last: None,
        });
        let flow_count = AtomicI64::new(stats.total);
        let bytes_total = AtomicI64::new(stats.bytes.request + stats.bytes.response);
        Arc::new(Self {
            store,
            ca,
            upstream: RwLock::new(upstream),
            config: RwLock::new(config),
            started_at: Utc::now(),
            data_dir,
            db_path,
            events,
            proxy: Mutex::new(ProxyRuntime::default()),
            active_connections: AtomicU64::new(0),
            total_connections: AtomicU64::new(0),
            flow_count,
            bytes_total,
            capture_state: RwLock::new("running".into()),
            windivert: RwLock::new(WinDivertRuntime::default()),
            web_host,
            web_port,
            insecure_upstream,
            transparent: Arc::new(TransparentMap::default()),
            shutdown: CancellationToken::new(),
        })
    }

    pub fn emit(&self, event: &str, data: serde_json::Value) {
        let _ = self.events.send(ServerEvent {
            event: event.to_string(),
            data,
        });
    }

    pub async fn capture_enabled(&self) -> bool {
        self.config.read().await.capture_enabled
    }

    pub async fn windivert_enabled(&self) -> bool {
        self.windivert.read().await.enabled
    }

    pub async fn record_new(&self, flow: FlowRecord) -> Option<FlowSummary> {
        if !self.capture_enabled().await {
            return None;
        }
        let summary = flow.summary(0);
        let seq = match self.store.insert(flow).await {
            Ok(seq) => seq,
            Err(e) => {
                tracing::error!("failed to persist flow: {e:#}");
                return None;
            }
        };
        let mut summary = summary;
        summary.seq = seq;
        self.flow_count.fetch_add(1, Ordering::Relaxed);
        self.bytes_total.fetch_add(
            (summary.request_size + summary.response_size) as i64,
            Ordering::Relaxed,
        );
        self.emit("flow", serde_json::to_value(&summary).unwrap_or_default());
        Some(summary)
    }

    pub async fn record_response(
        &self,
        id: &str,
        status: u16,
        reason: String,
        headers: Vec<HeaderEntry>,
        trailers: Vec<HeaderEntry>,
        body: Vec<u8>,
        body_size: u64,
        truncated: bool,
        ended_at: DateTime<Utc>,
        duration_ms: i64,
        error: Option<String>,
    ) {
        let old_resp_size = self
            .store
            .get_summary(id)
            .await
            .ok()
            .flatten()
            .map(|r| r.response_size)
            .unwrap_or(0);
        if let Err(e) = self
            .store
            .update_response(
                id.to_string(),
                status,
                reason,
                headers,
                trailers,
                body,
                body_size,
                truncated,
                ended_at,
                duration_ms,
                error,
            )
            .await
        {
            tracing::error!("failed to update flow {id}: {e:#}");
            return;
        }
        let delta = body_size.saturating_sub(old_resp_size) as i64;
        if delta > 0 {
            self.bytes_total.fetch_add(delta, Ordering::Relaxed);
        }
        if let Ok(Some(summary)) = self.store.get_summary(id).await {
            self.emit("flow", serde_json::to_value(&summary).unwrap_or_default());
        }
    }

    pub async fn record_error(
        &self,
        id: &str,
        error: String,
        ended_at: DateTime<Utc>,
        duration_ms: i64,
    ) {
        if let Err(e) = self
            .store
            .update_error(id.to_string(), error, ended_at, duration_ms)
            .await
        {
            tracing::error!("failed to mark flow {id} as error: {e:#}");
            return;
        }
        if let Ok(Some(summary)) = self.store.get_summary(id).await {
            self.emit("flow", serde_json::to_value(&summary).unwrap_or_default());
        }
    }

    pub async fn status(&self) -> Status {
        let config = self.config.read().await.clone();
        let proxy = self.proxy.lock().await;
        let state = proxy.state.clone();
        let proxy_error = proxy.error.clone();
        drop(proxy);
        let windivert = self.windivert.read().await;
        Status {
            version: VERSION.into(),
            started_at: self.started_at,
            proxy: ProxyStatusView {
                state: state.clone(),
                host: config.proxy_host.clone(),
                port: config.proxy_port,
                error: proxy_error,
                active_connections: self.active_connections.load(Ordering::Relaxed),
                total_connections: self.total_connections.load(Ordering::Relaxed),
                windivert: WinDivertStatusView {
                    enabled: windivert.enabled,
                    state: if windivert.state.is_empty() {
                        "off".into()
                    } else {
                        windivert.state.clone()
                    },
                    error: windivert.error.clone(),
                    packets: windivert.packets.load(Ordering::Relaxed),
                },
            },
            store: StoreStatusView {
                flows: self.flow_count.load(Ordering::Relaxed),
                bytes: self.bytes_total.load(Ordering::Relaxed),
                db_path: self.db_path.display().to_string(),
            },
            ca: CaStatusView {
                subject: "CN=MitmRs Local CA, O=MitmRs".to_string(),
                fingerprint: self.ca.fingerprint().to_string(),
                path: self.ca.cert_path().display().to_string(),
                pem_url: "/api/ca.crt".into(),
                not_before: self.ca.not_before().to_rfc3339(),
                not_after: self.ca.not_after().to_rfc3339(),
            },
            mcp: McpStatusView {
                endpoint: format!("http://{}:{}/mcp", self.web_host, self.web_port),
                stdio_command: format!(
                    "mitmrs mcp-stdio --url http://{}:{}/mcp",
                    self.web_host, self.web_port
                ),
                tools: crate::mcp::tool_names(),
            },
            config: RuntimeConfigView {
                max_body_bytes: config.max_body_bytes,
                capture_enabled: config.capture_enabled,
                upstream_proxy_enabled: config.upstream_proxy_enabled,
                upstream_proxy_url: config.upstream_proxy_url.clone(),
            },
        }
    }

    pub async fn file_config(&self) -> FileConfig {
        let c = self.config.read().await;
        FileConfig {
            proxy_host: c.proxy_host.clone(),
            proxy_port: c.proxy_port,
            max_body_bytes: c.max_body_bytes,
            capture_enabled: c.capture_enabled,
            upstream_proxy_enabled: c.upstream_proxy_enabled,
            upstream_proxy_url: c.upstream_proxy_url.clone(),
        }
    }

    pub async fn set_capture_enabled(&self, enabled: bool) {
        let mut cfg = self.config.write().await;
        cfg.capture_enabled = enabled;
        *self.capture_state.write().await = if enabled {
            "running".into()
        } else {
            "disabled".into()
        };
    }
}
