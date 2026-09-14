//! Server configuration, loaded from `config.toml` next to the executable.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind_ip")]
    pub bind_ip: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_bind_ip() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    8420
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsEntry {
    /// Human-readable id exposed to clients, e.g. `rack1-a`.
    pub id: String,
    /// USB serial number used to match the physical UPS.
    /// In mock mode this is ignored.
    #[serde(default)]
    pub serial: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub ups: Vec<UpsEntry>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_ip: default_bind_ip(),
            port: default_port(),
        }
    }
}

impl Config {
    /// Load `config.toml` from the directory containing the executable,
    /// falling back to the current working directory. If the file does not
    /// exist, a default config is written there and returned so a windowed
    /// deploy never dies silently on first run.
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            let cfg = Config::default();
            let _ = cfg.save(); // best effort: run with defaults even if this fails
            return Ok(cfg);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
        Ok(cfg)
    }

    /// Write this config back to `config.toml` next to the executable.
    /// Note: this round-trips through serde, so any hand-written comments in
    /// an existing file are lost on save.
    pub fn save(&self) -> Result<()> {
        let path = config_path();
        let text = toml::to_string_pretty(self).context("failed to serialize config")?;
        std::fs::write(&path, text)
            .with_context(|| format!("failed to write config file {}", path.display()))?;
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            ups: Vec::new(),
        }
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
