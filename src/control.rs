use crate::config::{save_file_config, RuntimeConfig};
use crate::model::{ConfigPatch, Status};
use crate::proxy;
use crate::state::AppState;
use crate::upstream::UpstreamClient;
use anyhow::{anyhow, Result};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub async fn apply_config(state: &Arc<AppState>, patch: ConfigPatch) -> Result<Status> {
    let old = state.config.read().await.clone();
    let mut next = old.clone();
    if let Some(port) = patch.port {
        if port == 0 {
            return Err(anyhow!("proxy port must be between 1 and 65535"));
        }
        next.proxy_port = port;
    }
    if let Some(max) = patch.max_body_bytes {
        next.max_body_bytes = max.clamp(64 * 1024, 256 * 1024 * 1024);
    }
    if let Some(enabled) = patch.capture_enabled {
        next.capture_enabled = enabled;
    }
    if let Some(enabled) = patch.upstream_proxy_enabled {
        next.upstream_proxy_enabled = enabled;
    }
    if let Some(url) = patch.upstream_proxy_url {
        next.upstream_proxy_url = url.trim().to_string();
    }

    let old_chain = match old.upstream_chain() {
        Ok(chain) => chain,
        Err(e) => {
            // A stale/invalid address must not block unrelated edits; startup
            // behaves the same way and keeps running without a chain.
            tracing::warn!("ignoring invalid saved upstream chain proxy: {e}");
            None
        }
    };
    let new_chain = next
        .upstream_chain()
        .map_err(|e| anyhow!("invalid upstream proxy: {e}"))?;
    if let Some(chain) = &new_chain {
        if is_loopback(&chain.host) && chain.port == next.proxy_port {
            return Err(anyhow!(
                "upstream proxy cannot point to this MitmRs proxy port {}",
                next.proxy_port
            ));
        }
    }
    let chain_changed = old_chain != new_chain;
    let new_upstream = if chain_changed {
        Some(UpstreamClient::new(
            state.insecure_upstream,
            new_chain.clone(),
        )?)
    } else {
        None
    };

    {
        let mut cfg = state.config.write().await;
        *cfg = next.clone();
    }
    if let Some(upstream) = new_upstream {
        *state.upstream.write().await = upstream;
    }
    let file = state.file_config().await;
    if let Err(e) = save_file_config(&state.data_dir, &file) {
        tracing::warn!("failed to persist config: {e:#}");
    }
    if next.capture_enabled != old.capture_enabled {
        state.set_capture_enabled(next.capture_enabled).await;
    }
    let port_changed = next.proxy_port != old.proxy_port;
    if port_changed {
        let running = {
            let rt = state.proxy.lock().await;
            rt.state == "running" || rt.state == "paused"
        };
        if running {
            proxy::restart(state.clone()).await?;
        }
    }
    Ok(state.status().await)
}

fn is_loopback(host: &str) -> bool {
    // Windows resolves the whole 127.0.0.0/8 block (and ::1) to itself.
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

pub async fn action(
    state: &Arc<AppState>,
    action: &str,
    patch: Option<ConfigPatch>,
) -> Result<Status> {
    if let Some(patch) = patch {
        apply_config(state, patch).await?;
    }
    match action {
        "status" | "get" => {}
        "start" => proxy::start(state.clone()).await?,
        "stop" => proxy::stop(state).await?,
        "pause" => proxy::pause(state).await?,
        "resume" => proxy::resume(state).await?,
        "restart" => proxy::restart(state.clone()).await?,
        "set_config" | "configure" => {}
        other => return Err(anyhow!("unknown proxy action '{other}'")),
    }
    Ok(state.status().await)
}

pub async fn clear_traffic(state: &Arc<AppState>) -> Result<usize> {
    let n = state.store.clear().await?;
    state.flow_count.store(0, Ordering::Relaxed);
    state.bytes_total.store(0, Ordering::Relaxed);
    state.emit(
        "status",
        serde_json::to_value(state.status().await).unwrap_or_default(),
    );
    Ok(n)
}

pub async fn delete_flow(state: &Arc<AppState>, id: &str) -> Result<bool> {
    let old = state.store.get_summary(id).await?;
    let found = state.store.delete(id).await?;
    if let Some(old) = old {
        state.flow_count.fetch_sub(1, Ordering::Relaxed);
        let bytes = (old.request_size + old.response_size) as i64;
        state.bytes_total.fetch_sub(bytes, Ordering::Relaxed);
    }
    Ok(found)
}

pub fn runtime_to_file(cfg: &RuntimeConfig) -> crate::config::FileConfig {
    crate::config::FileConfig {
        proxy_host: cfg.proxy_host.clone(),
        proxy_port: cfg.proxy_port,
        max_body_bytes: cfg.max_body_bytes,
        capture_enabled: cfg.capture_enabled,
        upstream_proxy_enabled: cfg.upstream_proxy_enabled,
        upstream_proxy_url: cfg.upstream_proxy_url.clone(),
    }
}
