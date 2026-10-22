mod connection;
mod io;
pub(crate) mod service;

use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub async fn start(state: Arc<AppState>) -> Result<()> {
    {
        let rt = state.proxy.lock().await;
        if rt.state == "running" || rt.state == "paused" {
            return Ok(());
        }
    }
    let cfg = state.config.read().await.clone();
    let addr = format!("{}:{}", cfg.proxy_host, cfg.proxy_port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind proxy listener on {addr}"))?;
    let actual = listener.local_addr()?;
    tracing::info!("proxy listening on {actual}");
    let cancel = CancellationToken::new();
    // Transparent IPv6 redirects are rewritten to [::1], so the listener must
    // also be reachable there. Best effort: a host without IPv6 keeps working.
    let v6_listener = TcpListener::bind(format!("[::1]:{}", actual.port()))
        .await
        .ok();
    let cancel_for_task = cancel.clone();
    let state_for_task = state.clone();
    let task = tokio::spawn(async move {
        accept_loop(state_for_task, listener, cancel_for_task).await;
    });
    if let Some(v6_listener) = v6_listener {
        let v6_state = state.clone();
        let v6_cancel = cancel.clone();
        tokio::spawn(async move {
            accept_loop(v6_state, v6_listener, v6_cancel).await;
        });
    }
    {
        let mut rt = state.proxy.lock().await;
        rt.state = "running".into();
        rt.error = None;
        rt.cancel = Some(cancel);
        rt.task = Some(task);
    }
    state.emit(
        "status",
        serde_json::to_value(state.status().await).unwrap_or_default(),
    );
    Ok(())
}

pub async fn stop(state: &Arc<AppState>) -> Result<()> {
    #[cfg(windows)]
    {
        let wd_enabled = state.windivert.read().await.enabled;
        if wd_enabled {
            let _ = crate::windivert::stop(state).await;
        }
    }
    let (cancel, task) = {
        let mut rt = state.proxy.lock().await;
        if rt.state == "stopped" {
            return Ok(());
        }
        rt.state = "stopping".into();
        (rt.cancel.take(), rt.task.take())
    };
    if let Some(cancel) = cancel {
        cancel.cancel();
    }
    if let Some(task) = task {
        task.abort();
        let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
    }
    {
        let mut rt = state.proxy.lock().await;
        rt.state = "stopped".into();
        rt.error = None;
    }
    // A restart is the user's "try intercepting again" action: forget learned
    // passthrough hosts so the next connections are MITM'd once more.
    state.clear_passthrough();
    state.emit(
        "status",
        serde_json::to_value(state.status().await).unwrap_or_default(),
    );
    Ok(())
}

pub async fn pause(state: &Arc<AppState>) -> Result<()> {
    {
        let mut rt = state.proxy.lock().await;
        if rt.state != "running" {
            return Err(anyhow!("proxy is not running"));
        }
        rt.state = "paused".into();
    }
    state.emit(
        "status",
        serde_json::to_value(state.status().await).unwrap_or_default(),
    );
    Ok(())
}

pub async fn resume(state: &Arc<AppState>) -> Result<()> {
    {
        let mut rt = state.proxy.lock().await;
        if rt.state != "paused" {
            return Err(anyhow!("proxy is not paused"));
        }
        rt.state = "running".into();
    }
    state.emit(
        "status",
        serde_json::to_value(state.status().await).unwrap_or_default(),
    );
    Ok(())
}

pub async fn restart(state: Arc<AppState>) -> Result<()> {
    #[cfg(windows)]
    let wd_was_enabled = state.windivert.read().await.enabled;
    stop(&state).await?;
    start(state.clone()).await?;
    #[cfg(windows)]
    if wd_was_enabled {
        if let Err(e) = crate::windivert::start(state.clone()).await {
            tracing::warn!("WinDivert restart failed: {e:#}");
            let mut rt = state.windivert.write().await;
            rt.state = "error".into();
            rt.error = Some(format!("{e:#}"));
        }
    }
    Ok(())
}

async fn accept_loop(state: Arc<AppState>, listener: TcpListener, cancel: CancellationToken) {
    let mut tasks: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            incoming = listener.accept() => {
                match incoming {
                    Ok((stream, peer)) => {
                        state.total_connections.fetch_add(1, Ordering::Relaxed);
                        state.active_connections.fetch_add(1, Ordering::Relaxed);
                        let state_for_conn = state.clone();
                        let cancel_for_conn = cancel.clone();
                        let target = if state.windivert_enabled().await {
                            state.transparent.take(peer.port())
                        } else {
                            None
                        };
                        tasks.spawn(async move {
                            let _guard = ConnGuard(state_for_conn.clone());
                            if let Some(target) = target {
                                connection::serve_transparent(
                                    state_for_conn,
                                    stream,
                                    peer,
                                    target,
                                    cancel_for_conn,
                                )
                                .await;
                            } else {
                                connection::serve_explicit(
                                    state_for_conn,
                                    stream,
                                    peer,
                                    cancel_for_conn,
                                )
                                .await;
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!("proxy accept error: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    tracing::info!("proxy accept loop stopped");
}

pub(crate) struct ConnGuard(pub(crate) Arc<AppState>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.active_connections.fetch_sub(1, Ordering::Relaxed);
    }
}
