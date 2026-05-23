//! Agent configuration loading.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use urc_common::AgentConfig;

pub fn load_config(path: &Path) -> Result<AgentConfig> {
    if path.exists() {
        let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let mut cfg: AgentConfig = toml::from_str(&text).context("parse agent config")?;
        if cfg.host_id.is_none() {
            cfg.host_id = Some(hostname_id());
        }
        return Ok(cfg);
    }

    let mut cfg = AgentConfig::default();
    cfg.host_id = Some(hostname_id());
    Ok(cfg)
}

fn hostname_id() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_else(|_| "urc-host".into())
        .trim()
        .to_string()
}
