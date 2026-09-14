//! Client configuration, loaded from `config.toml` next to the executable.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSection {
    /// The server's LAN address. `None` until the user sets one via the
    /// "Server settings" dialog (or edits config.toml by hand).
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollingSection {
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    /// Consecutive failed polls before the server is considered unreachable.
    #[serde(default = "default_missed")]
    pub unreachable_after_missed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertsSection {
    #[serde(default = "default_threshold")]
    pub low_battery_threshold_pct: u8,
}

pub fn default_port() -> u16 {
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

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            address: None,
            port: default_port(),
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerSection,
    #[serde(default)]
    pub polling: PollingSection,
    #[serde(default)]
    pub alerts: AlertsSection,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerSection::default(),
            polling: PollingSection::default(),
            alerts: AlertsSection::default(),
        }
    }
}

impl Config {
    /// Load `config.toml`, or fall back to defaults (no server address set)
    /// if it doesn't exist yet — the GUI's "Server settings" dialog handles
    /// that case rather than failing to start.
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(cfg)
    }

    /// Write this config back to `config.toml` next to the executable.
    pub fn save(&self) -> Result<()> {
        let path = config_path();
        let text = toml::to_string_pretty(self).context("failed to serialize config")?;
        std::fs::write(&path, text)
            .with_context(|| format!("failed to write config file {}", path.display()))?;
        Ok(())
    }

    pub fn has_server(&self) -> bool {
        self.server
            .address
            .as_deref()
            .is_some_and(|a| !a.trim().is_empty())
    }

    pub fn base_url(&self) -> Option<String> {
        let addr = self.server.address.as_deref()?;
        if addr.trim().is_empty() {
            return None;
        }
        Some(format!("http://{}:{}", addr.trim(), self.server.port))
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
