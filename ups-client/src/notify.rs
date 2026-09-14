//! Toast notifications via notify-rust.
//!
//! NOTE: notify-rust gives standard, auto-dismissing Windows toasts. For Tier 1
//! and Tier 2 we make them as loud/persistent as the crate allows (critical
//! urgency + a sound). If this proves too easy to miss, the planned upgrade is
//! native WinRT toasts (reminder/alarm scenario that repeats until dismissed) —
//! this module is the single place to swap that in.

use notify_rust::{Notification, Timeout, Urgency};
use tracing::warn;

use crate::detect::AlertEvent;

pub fn fire(event: &AlertEvent) {
    let res = match event {
        AlertEvent::WentOnBattery { id } => Notification::new()
            .summary("UPS On Battery")
            .body(&format!("{id} has switched to battery power."))
            .urgency(Urgency::Critical)
            .timeout(Timeout::Never)
            .show(),
        AlertEvent::LowBattery {
            id,
            pct,
            runtime_secs,
        } => {
            let runtime = runtime_secs
                .map(|r| format!(" (~{} min left)", r / 60))
                .unwrap_or_default();
            Notification::new()
                .summary("SHUT DOWN NOW — UPS Critical")
                .body(&format!(
                    "{id} is at {pct}% battery{runtime}. Shut down connected equipment now."
                ))
                .urgency(Urgency::Critical)
                .timeout(Timeout::Never)
                .show()
        }
        AlertEvent::Recovered { id } => Notification::new()
            .summary("UPS Back to Normal")
            .body(&format!("{id} is back on mains power."))
            .urgency(Urgency::Normal)
            .timeout(Timeout::Milliseconds(6000))
            .show(),
    };
    if let Err(e) = res {
        warn!(error = %e, "failed to show toast");
    }
}

pub fn fire_server_unreachable() {
    let _ = Notification::new()
        .summary("UPS Monitor — Server Unreachable")
        .body("Cannot reach the UPS server. UPS state may be stale.")
        .urgency(Urgency::Critical)
        .timeout(Timeout::Never)
        .show();
}

pub fn fire_server_recovered() {
    let _ = Notification::new()
        .summary("UPS Monitor — Server Reachable Again")
        .body("Reconnected to the UPS server.")
        .urgency(Urgency::Normal)
        .timeout(Timeout::Milliseconds(5000))
        .show();
}
