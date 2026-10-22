use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileConfig {
    pub proxy_host: String,
    pub proxy_port: u16,
    pub max_body_bytes: u64,
    pub capture_enabled: bool,
    /// Optional next-hop HTTP proxy. Persisted even while disabled so the UI
    /// can remember the last address the user typed.
    #[serde(default)]
    pub upstream_proxy_enabled: bool,
    #[serde(default)]
    pub upstream_proxy_url: String,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            proxy_host: "127.0.0.1".into(),
            proxy_port: 8080,
            max_body_bytes: 10 * 1024 * 1024,
            capture_enabled: true,
            upstream_proxy_enabled: false,
            upstream_proxy_url: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub proxy_host: String,
    pub proxy_port: u16,
    pub max_body_bytes: u64,
    pub capture_enabled: bool,
    pub upstream_proxy_enabled: bool,
    pub upstream_proxy_url: String,
}

impl From<&FileConfig> for RuntimeConfig {
    fn from(c: &FileConfig) -> Self {
        Self {
            proxy_host: c.proxy_host.clone(),
            proxy_port: c.proxy_port,
            max_body_bytes: c.max_body_bytes,
            capture_enabled: c.capture_enabled,
            upstream_proxy_enabled: c.upstream_proxy_enabled,
            upstream_proxy_url: c.upstream_proxy_url.clone(),
        }
    }
}

impl RuntimeConfig {
    /// Parse the optional next-hop proxy. The address is parsed only while the
    /// user has chaining enabled, so a remembered-but-disabled address never
    /// blocks unrelated configuration changes.
    pub fn upstream_chain(&self) -> Result<Option<crate::upstream::ProxyChain>, String> {
        if !self.upstream_proxy_enabled {
            return Ok(None);
        }
        crate::upstream::ProxyChain::parse(&self.upstream_proxy_url).map(Some)
    }
}

pub fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MITMRS_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("MitmRs");
    }
    PathBuf::from(".mitmrs")
}

pub fn load_file_config(data_dir: &std::path::Path) -> FileConfig {
    let path = data_dir.join("config.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_file_config(data_dir: &std::path::Path, cfg: &FileConfig) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join("config.json");
    let tmp = data_dir.join("config.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(cfg)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
