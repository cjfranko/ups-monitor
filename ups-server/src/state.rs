//! Shared, cached UPS state. One entry per configured UPS, updated by the
//! polling tasks and read (lock-free-ish, briefly locked) by the API and GUI.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use ups_common::UpsStatus;

use crate::config::UpsEntry;

/// Global "simulate on battery" override, toggled from the server GUI.
/// When true, pollers report every UPS as OnBattery with a draining battery
/// regardless of real hardware state, so clients' alert paths can be tested
/// against a live server without pulling any plugs.
pub static SIMULATE_ON_BATTERY: AtomicBool = AtomicBool::new(false);

pub fn simulate_on_battery() -> bool {
    SIMULATE_ON_BATTERY.load(Ordering::Relaxed)
}

pub fn set_simulate_on_battery(v: bool) {
    SIMULATE_ON_BATTERY.store(v, Ordering::Relaxed);
}

pub type SharedState = Arc<RwLock<HashMap<String, UpsStatus>>>;

/// Build the initial state: every configured UPS starts as `Unknown` until
/// its polling task reports in for the first time.
pub fn new_shared_state(ups: &[UpsEntry]) -> SharedState {
    let mut map = HashMap::with_capacity(ups.len());
    for u in ups {
        map.insert(u.id.clone(), UpsStatus::unknown(&u.id));
    }
    Arc::new(RwLock::new(map))
}

/// Insert/update the status for one UPS.
pub fn update(state: &SharedState, status: UpsStatus) {
    if let Ok(mut map) = state.write() {
        map.insert(status.id.clone(), status);
    }
}

/// Snapshot of all current statuses (cloned out so no lock is held by callers).
pub fn snapshot(state: &SharedState) -> Vec<UpsStatus> {
    match state.read() {
        Ok(map) => {
            let mut v: Vec<UpsStatus> = map.values().cloned().collect();
            v.sort_by(|a, b| a.id.cmp(&b.id));
            v
        }
        Err(_) => Vec::new(),
    }
}
