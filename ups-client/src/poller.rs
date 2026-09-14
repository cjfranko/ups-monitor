//! Client polling loop: polls `GET /status`, feeds the transition detector,
//! fires toasts, tracks connectivity, and updates shared state for the GUI.

use std::thread;
use std::time::Duration;

use tracing::{info, warn};
use ups_common::UpsStatus;

use crate::config::Config;
use crate::detect::TransitionDetector;
use crate::notify;
use crate::state::{self, SharedClientState};

pub fn run(cfg: Config, shared: SharedClientState) {
    let url = format!("{}/status", cfg.base_url());
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

    loop {
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
}
