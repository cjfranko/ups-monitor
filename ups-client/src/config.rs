//! Client configuration, loaded from `config.toml` next to the executable.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    pub address: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PollingSection {
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    /// Consecutive failed polls before the server is considered unreachable.
    #[serde(default = "default_missed")]
    pub unreachable_after_missed: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlertsSection {
    #[serde(default = "default_threshold")]
    pub low_battery_threshold_pct: u8,
}

fn default_port() -> u16 {
    8420
}
fn default_interval() -> u64 {
    5
}
fn default_missed() -> u32 {
    2
}
fn default_threshold() -> u8 {
    20
}

impl Default for PollingSection {
    fn default() -> Self {
        Self {
            interval_secs: default_interval(),
            unreachable_after_missed: default_missed(),
        }
    }
}
impl Default for AlertsSection {
    fn default() -> Self {
        Self {
            low_battery_threshold_pct: default_threshold(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerSection,
    #[serde(default)]
    pub polling: PollingSection,
    #[serde(default)]
    pub alerts: AlertsSection,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(cfg)
    }

    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.server.address, self.server.port)
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("config.toml");
            if p.exists() {
                return p;
            }
        }
    }
    PathBuf::from("config.toml")
}
