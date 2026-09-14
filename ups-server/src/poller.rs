//! Background polling: one task per UPS (real HID or mock), writing into the
//! shared state every poll interval.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use hidapi::HidApi;
use tracing::{error, info, warn};

use crate::config::UpsEntry;
use crate::hid;
use crate::mock;
use crate::state::{self, SharedState};

pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Stop flags for currently-running pollers, keyed by UPS id, so a poller can
/// be cancelled (e.g. on rename, where the id it was started with changes).
pub type PollerHandles = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

pub fn new_handles() -> PollerHandles {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Signal a running poller to stop. It notices within one poll interval.
pub fn stop_poller(handles: &PollerHandles, id: &str) {
    if let Ok(mut map) = handles.lock() {
        if let Some(flag) = map.remove(id) {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

fn register(handles: &PollerHandles, id: &str) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut map) = handles.lock() {
        map.insert(id.to_string(), flag.clone());
    }
    flag
}

/// Apply the global "simulate on battery" override (toggled from the GUI).
/// Replaces the real reading with an on-battery status that drains over
/// time so clients' low-battery thresholds eventually trip.
fn apply_simulation(state: &SharedState, mut s: ups_common::UpsStatus) -> ups_common::UpsStatus {
    if !state::simulate_on_battery() {
        return s;
    }
    // Drain from the previously stored (simulated) value so the gauge
    // actually falls over successive polls instead of re-reading the real
    // battery every time.
    let (prev_pct, prev_runtime) = state
        .read()
        .ok()
        .and_then(|m| m.get(&s.id).map(|p| (p.battery_pct, p.runtime_secs)))
        .unwrap_or((None, None));
    let pct = prev_pct
        .filter(|p| *p > 5)
        .map(|p| p.saturating_sub(1))
        .unwrap_or(100);
    let runtime = prev_runtime
        .filter(|r| *r > 300)
        .map(|r| r.saturating_sub(60))
        .unwrap_or(1800);
    s.status = ups_common::PowerState::OnBattery;
    s.battery_pct = Some(pct.max(5));
    s.runtime_secs = Some(runtime.max(300));
    s.last_updated = chrono::Utc::now();
    s
}

/// Spawn a polling task per configured UPS in real (HID) mode.
pub fn spawn_real_pollers(handles: &PollerHandles, state: SharedState, ups: Vec<UpsEntry>) {
    for entry in ups {
        spawn_real_poller(handles, state.clone(), entry);
    }
}

/// Spawn a single real-mode (HID) poller, registering it so it can later be
/// stopped (e.g. by `stop_poller` on rename).
pub fn spawn_real_poller(handles: &PollerHandles, state: SharedState, entry: UpsEntry) {
    let stop = register(handles, &entry.id);
    std::thread::spawn(move || poll_one_real(state, entry, stop));
}

fn poll_one_real(state: SharedState, entry: UpsEntry, stop: Arc<AtomicBool>) {
    let api = match HidApi::new() {
        Ok(a) => Arc::new(a),
        Err(e) => {
            error!(ups = %entry.id, error = %e, "failed to init HID API");
            return;
        }
    };
    info!(ups = %entry.id, serial = %entry.serial, "polling loop started (HID)");

    while !stop.load(Ordering::Relaxed) {
        let status = match hid::open_by_serial(&api, &entry.serial) {
            Some(dev) => match hid::read_status(&dev, &entry.id) {
                Ok(s) => s,
                Err(e) => {
                    warn!(ups = %entry.id, error = %e, "read failed");
                    ups_common::UpsStatus::unknown(&entry.id)
                }
            },
            None => {
                warn!(ups = %entry.id, serial = %entry.serial, "device not found");
                ups_common::UpsStatus::unknown(&entry.id)
            }
        };
        state::update(&state, apply_simulation(&state, status));
        std::thread::sleep(POLL_INTERVAL);
    }
    info!(ups = %entry.id, "polling loop stopped (HID)");
}

/// Spawn polling tasks in mock mode (no hardware needed).
pub fn spawn_mock_pollers(handles: &PollerHandles, state: SharedState, ids: Vec<String>, failing: HashSet<String>) {
    let units = mock::build_mock_units(&ids, &failing);
    for unit in units {
        let id = unit.id.clone();
        let stop = register(handles, &id);
        let st = state.clone();
        std::thread::spawn(move || run_mock(st, unit, stop));
    }
}

/// Spawn a single mock poller for a UPS added at runtime while in mock mode.
pub fn spawn_mock_poller(handles: &PollerHandles, state: SharedState, id: String) {
    let stop = register(handles, &id);
    let unit = mock::MockUps::new(id, false);
    std::thread::spawn(move || run_mock(state, unit, stop));
}

fn run_mock(state: SharedState, mut unit: mock::MockUps, stop: Arc<AtomicBool>) {
    info!(ups = %unit.id, "polling loop started (mock)");
    while !stop.load(Ordering::Relaxed) {
        let status = unit.tick();
        state::update(&state, status);
        std::thread::sleep(POLL_INTERVAL);
    }
    info!(ups = %unit.id, "polling loop stopped (mock)");
}

/// Log discovered devices once at startup (helps build config serials).
pub fn log_discovery() -> Result<()> {
    let api = HidApi::new()?;
    let found = hid::enumerate_ups(&api);
    if found.is_empty() {
        warn!("no APC UPS units discovered on USB HID");
    } else {
        for d in &found {
            info!(
                serial = %d.serial,
                product = %d.product,
                path = %d.path,
                "discovered UPS"
            );
        }
    }
    Ok(())
}
