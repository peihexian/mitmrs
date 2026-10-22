use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use mitmrs::ca::CertAuthority;
use mitmrs::config::{default_data_dir, load_file_config};
use mitmrs::state::AppState;
use mitmrs::store::Store;
use mitmrs::upstream::UpstreamClient;
use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "mitmrs",
    version,
    about = "AI-native HTTPS MITM traffic analysis for Windows",
    long_about = "MitmRs is a Windows MITM proxy with a mitmweb-style web UI, schema-less HTTP/gRPC/Protobuf decoding, import/export, lifecycle control, and an MCP server that lets AI agents query captured traffic."
)]
struct Cli {
    /// Web UI / API bind host.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Web UI / API port.
    #[arg(long, default_value_t = 8787)]
    web_port: u16,
    /// Proxy listen port (overrides saved config).
    #[arg(long)]
    proxy_port: Option<u16>,
    /// Proxy listen host (overrides saved config).
    #[arg(long)]
    proxy_host: Option<String>,
    /// Data directory for the CA, SQLite database and config.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Start WinDivert transparent redirect (requires Administrator + WinDivert DLL/SYS).
    #[arg(long)]
    windivert: bool,
    /// Do not verify upstream TLS certificates (useful for self-signed test servers).
    #[arg(long)]
    insecure_upstream: bool,
    /// Do not open the browser on startup.
    #[arg(long)]
    no_open: bool,
    /// Run the stdio MCP bridge, forwarding JSON-RPC lines to a running MitmRs server.
    #[arg(long)]
    mcp_stdio: bool,
    /// MCP HTTP endpoint used with --mcp-stdio.
    #[arg(long, default_value = "http://127.0.0.1:8787/mcp")]
    mcp_url: String,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the stdio MCP bridge (alias for --mcp-stdio).
    McpStdio {
        /// MCP HTTP endpoint of a running MitmRs server.
        #[arg(long, default_value = "http://127.0.0.1:8787/mcp")]
        url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("mitmrs=info,tower_http=warn,hyper=warn")),
        )
        .with_target(false)
        .init();

    if let Some(Command::McpStdio { url }) = cli.command {
        return tokio::task::spawn_blocking(move || mitmrs::mcp::run_stdio(url)).await?;
    }
    if cli.mcp_stdio {
        let url = cli.mcp_url.clone();
        return tokio::task::spawn_blocking(move || mitmrs::mcp::run_stdio(url)).await?;
    }

    // rustls 0.23 requires a process-level crypto provider before any
    // ServerConfig/ClientConfig builder is used.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let data_dir = cli.data_dir.clone().unwrap_or_else(default_data_dir);
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("create data dir {}", data_dir.display()))?;
    let file_cfg = load_file_config(&data_dir);
    let mut runtime_cfg = mitmrs::config::RuntimeConfig::from(&file_cfg);
    if let Some(port) = cli.proxy_port {
        runtime_cfg.proxy_port = port;
    }
    if let Some(host) = &cli.proxy_host {
        runtime_cfg.proxy_host = host.clone();
    }
    let db_path = data_dir.join("mitmrs.db");
    let store = Store::open(db_path.clone()).await?;
    let ca = Arc::new(CertAuthority::load_or_create(data_dir.join("ca")).await?);
    let startup_chain = match runtime_cfg.upstream_chain() {
        Ok(chain) => chain,
        Err(e) => {
            tracing::warn!("ignoring saved upstream chain proxy: {e}");
            None
        }
    };
    let upstream = UpstreamClient::new(cli.insecure_upstream, startup_chain.clone())?;
    let state = AppState::new(
        store,
        ca,
        upstream,
        runtime_cfg,
        data_dir.clone(),
        db_path,
        cli.host.clone(),
        cli.web_port,
        cli.insecure_upstream,
    )
    .await;

    if let Err(e) = mitmrs::proxy::start(state.clone()).await {
        tracing::error!("proxy failed to start: {e:#}");
        let mut rt = state.proxy.lock().await;
        rt.state = "error".into();
        rt.error = Some(format!("{e:#}"));
    }

    #[cfg(windows)]
    if cli.windivert {
        match mitmrs::windivert::start(state.clone()).await {
            Ok(()) => tracing::info!("WinDivert transparent redirect started"),
            Err(e) => tracing::error!("WinDivert start failed: {e:#}"),
        }
    }

    let app = mitmrs::api::router(state.clone());
    let addr = format!("{}:{}", cli.host, cli.web_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind web UI on {addr}"))?;
    let ui_url = format!("http://{}:{}/", cli.host, cli.web_port);
    let mcp_url = format!("http://{}:{}/mcp", cli.host, cli.web_port);
    println!();
    println!("  MitmRs {}", env!("CARGO_PKG_VERSION"));
    println!("  Web UI : {ui_url}");
    {
        let cfg = state.config.read().await;
        println!("  Proxy  : http://{}:{}", cfg.proxy_host, cfg.proxy_port);
    }
    if let Some(chain) = &startup_chain {
        println!("  Chain  : http://{}:{}", chain.host, chain.port);
    }
    println!("  MCP    : {mcp_url}");
    println!("  Data   : {}", data_dir.display());
    println!("  CA     : {}", state.ca.cert_path().display());
    println!();

    if !cli.no_open {
        open_browser(&ui_url);
    }

    let serve_state = state.clone();
    let shutdown_token = state.shutdown.clone();
    tokio::spawn(async move {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("failed to listen for Ctrl+C: {e}");
            return;
        }
        tracing::info!("shutting down (press Ctrl+C again to force quit)");
        shutdown_token.cancel();
        // A second Ctrl+C must always be able to kill the process, even if some
        // subsystem (WinDivert, a stuck connection) never finishes draining.
        let _ = tokio::signal::ctrl_c().await;
        tracing::warn!("second Ctrl+C received, forcing exit");
        std::process::exit(130);
    });

    let server = axum::serve(listener, app).with_graceful_shutdown({
        let shutdown = state.shutdown.clone();
        async move { shutdown.cancelled().await }
    });
    // Graceful shutdown waits for in-flight requests. Long-lived SSE streams are
    // ended by the `events` handler; the timeout is a safety net so that one
    // stuck connection can never keep the process alive.
    let mut server = std::pin::pin!(server.into_future());
    let shutdown = state.shutdown.clone();
    let mut server_done = false;
    tokio::select! {
        res = &mut server => {
            log_serve_result(res);
            server_done = true;
        }
        _ = shutdown.cancelled() => {}
    }
    if !server_done {
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut server).await {
            Ok(res) => log_serve_result(res),
            Err(_) => tracing::warn!("web server did not drain within 5s; continuing shutdown"),
        }
    }

    #[cfg(windows)]
    {
        if tokio::time::timeout(
            std::time::Duration::from_secs(5),
            mitmrs::windivert::stop(&serve_state),
        )
        .await
        .is_err()
        {
            tracing::warn!("WinDivert shutdown did not finish within 5s");
        }
    }
    if tokio::time::timeout(
        std::time::Duration::from_secs(5),
        mitmrs::proxy::stop(&serve_state),
    )
    .await
    .is_err()
    {
        tracing::warn!("proxy shutdown did not finish within 5s");
    }
    tracing::info!("mitmrs stopped");
    Ok(())
}

fn log_serve_result(result: std::io::Result<()>) {
    match result {
        Ok(()) => tracing::info!("web server stopped"),
        Err(e) => tracing::error!("web server error: {e:#}"),
    }
}

fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}
