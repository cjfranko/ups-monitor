//! Mock UPS source for end-to-end testing without hardware.
//!
//! Run the server with `--mock` (optionally `--mock-fail <id>`) and it will
//! fabricate plausible, slowly-drifting UPS data. This exists specifically so
//! you can point real clients at the server and verify that:
//!   * the API serves sane data,
//!   * clients detect Online -> OnBattery transitions and fire Tier 1 alerts,
//!   * the battery percentage drifts down and crosses the low-battery
//!     threshold so clients fire Tier 2 "shut down now" alerts,
//!   * killing the server exercises the client's server-unreachable path.
//!
//! A unit marked failing goes OnBattery immediately, then its battery drains
//! ~1% every couple of polls so you can watch the threshold crossing happen
//! within a minute or two rather than waiting for a real battery to empty.

use chrono::Utc;
use rand::Rng;
use std::collections::HashSet;
use ups_common::{PowerState, UpsStatus};

pub struct MockUps {
    pub id: String,
    on_battery: bool,
    battery_pct: u8,
    runtime_secs: u32,
    load_pct: u8,
    voltage_v: f32,
}

impl MockUps {
    pub fn new(id: impl Into<String>, failing: bool) -> Self {
        Self {
            id: id.into(),
            on_battery: failing,
            battery_pct: 100,
            runtime_secs: 3600,
            load_pct: 35,
            voltage_v: 230.0,
        }
    }

    /// Advance the simulation one tick and return the current snapshot.
    pub fn tick(&mut self) -> UpsStatus {
        let mut rng = rand::thread_rng();
        if self.on_battery {
            // Drain faster than reality so threshold crossings are observable.
            self.battery_pct = self.battery_pct.saturating_sub(rng.gen_range(1..=3));
            self.runtime_secs = self.runtime_secs.saturating_sub(120);
        } else {
            // Trickle back up towards full.
            if self.battery_pct < 100 {
                self.battery_pct = (self.battery_pct + 1).min(100);
            }
        }
        // Load/voltage wander a little so the UI shows them changing.
        self.load_pct = (self.load_pct as i16 + rng.gen_range(-2..=2)).clamp(5, 90) as u8;
        self.voltage_v = (self.voltage_v + rng.gen_range(-1.0..=1.0)).clamp(215.0, 245.0);
        UpsStatus {
            id: self.id.clone(),
            status: if self.on_battery {
                PowerState::OnBattery
            } else {
                PowerState::Online
            },
            battery_pct: Some(self.battery_pct),
            runtime_secs: Some(self.runtime_secs),
            load_pct: Some(self.load_pct),
            voltage_v: Some(self.voltage_v),
            last_updated: Utc::now(),
        }
    }
}

pub fn build_mock_units(ids: &[String], failing: &HashSet<String>) -> Vec<MockUps> {
    ids.iter()
        .map(|id| MockUps::new(id.clone(), failing.contains(id)))
        .collect()
}
