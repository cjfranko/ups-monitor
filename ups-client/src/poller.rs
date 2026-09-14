//! Client polling loop: polls `GET /status`, feeds the transition detector,
//! fires toasts, tracks connectivity, and updates shared state for the GUI.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tracing::{info, warn};
use ups_common::UpsStatus;

use crate::config::Config;
use crate::detect::TransitionDetector;
use crate::notify;
use crate::state::{self, SharedClientState};

/// Owns the currently-running poller (if any) so the GUI can restart it with
/// a new `Config` when the user changes the server address/port.
pub struct PollerControl {
    stop: Mutex<Option<Arc<AtomicBool>>>,
}

impl PollerControl {
    pub fn new() -> Self {
        Self {
            stop: Mutex::new(None),
        }
    }

    /// Stop any running poller and start a new one with this config.
    /// No-op (does not start) if `cfg` has no server address configured.
    pub fn restart(&self, cfg: Config, shared: SharedClientState) {
        self.stop_current();
        if !cfg.has_server() {
            return;
        }
        let flag = Arc::new(AtomicBool::new(false));
        *self.stop.lock().unwrap() = Some(flag.clone());
        thread::spawn(move || run(cfg, shared, flag));
    }

    pub fn stop_current(&self) {
        if let Some(flag) = self.stop.lock().unwrap().take() {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

fn run(cfg: Config, shared: SharedClientState, stop: Arc<AtomicBool>) {
    let Some(base_url) = cfg.base_url() else {
        warn!("poller started without a server address configured");
        return;
    };
    let url = format!("{base_url}/status");
    let interval = Duration::from_secs(cfg.polling.interval_secs);
    let unreachable_after = cfg.polling.unreachable_after_missed;
    let mut detector = TransitionDetector::new(cfg.alerts.low_battery_threshold_pct);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("http client");

    let mut missed: u32 = 0;
    let mut was_unreachable = false;

    info!(%url, interval_s = cfg.polling.interval_secs, "client poller started");

    while !stop.load(Ordering::Relaxed) {
        match client.get(&url).send() {
            Ok(resp) => match resp.json::<Vec<UpsStatus>>() {
                Ok(statuses) => {
                    // Fire alerts for transitions.
                    for s in &statuses {
                        for ev in detector.observe(s) {
                            info!(?ev, "alert");
                            notify::fire(&ev);
                        }
                    }
                    state::apply_success(&shared, statuses);
                    if was_unreachable {
                        notify::fire_server_recovered();
                    }
                    missed = 0;
                    was_unreachable = false;
                }
                Err(e) => {
                    warn!(error = %e, "failed to parse /status response");
                    missed = missed.saturating_add(1);
                }
            },
            Err(e) => {
                warn!(error = %e, "poll failed");
                missed = missed.saturating_add(1);
            }
        }

        if missed >= unreachable_after && !was_unreachable {
            was_unreachable = true;
            state::apply_failure(&shared, true);
            notify::fire_server_unreachable();
        }

        thread::sleep(interval);
    }
    info!(%url, "client poller stopped");
}
