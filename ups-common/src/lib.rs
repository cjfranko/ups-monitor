//! Shared types between `ups-server` and `ups-client`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// High-level power status of a single UPS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerState {
    /// Mains power present, running normally.
    Online,
    /// Running on battery (mains lost / discharging).
    OnBattery,
    /// Status could not be determined (read error, device missing, etc.).
    Unknown,
}

impl std::fmt::Display for PowerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PowerState::Online => write!(f, "Online"),
            PowerState::OnBattery => write!(f, "On Battery"),
            PowerState::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Snapshot of one UPS at a point in time. This is the exact JSON shape
/// served by `GET /status` on the server and consumed by the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsStatus {
    /// Human-readable id from the server config, e.g. `rack1-a`.
    pub id: String,
    pub status: PowerState,
    /// Battery charge percentage, if the UPS reports it.
    pub battery_pct: Option<u8>,
    /// Estimated runtime remaining, if the UPS reports it.
    pub runtime_secs: Option<u32>,
    /// When this snapshot was produced/refreshed on the server.
    pub last_updated: DateTime<Utc>,
}

impl UpsStatus {
    pub fn unknown(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            status: PowerState::Unknown,
            battery_pct: None,
            runtime_secs: None,
            last_updated: Utc::now(),
        }
    }
}
