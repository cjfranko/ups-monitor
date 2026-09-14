//! Toast notifications.
//!
//! On Windows we talk to WinRT toasts directly via `tauri-winrt-notification`
//! rather than through `notify-rust`'s generic wrapper, because Tier 2 (the
//! "shut down now" critical low-battery alert) needs things the cross-platform
//! API can't express: the `alarm` toast scenario (pre-expanded, stays on
//! screen, loops alarm audio until dismissed) as opposed to Tier 1's `reminder`
//! scenario (stays on screen, but a normal non-looping chime). Without this
//! distinction the two tiers were audibly and visually identical apart from
//! their text, which is exactly what the build plan flagged as insufficient.
//!
//! Non-Windows builds fall back to `notify-rust` so `cargo check --workspace`
//! still passes off-Windows; this app only ever ships on Windows.

use crate::detect::AlertEvent;

/// AppUserModelID for taskbar/tray identity (process-wide grouping only —
/// see `crate::aumid` for the toast-identity registration, which is a
/// separate mechanism).
pub const APP_ID: &str = "UpsMonitor.Client";

pub fn init_app_id() {
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use tracing::warn;
        let id: Vec<u16> = OsStr::new(APP_ID)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        extern "system" {
            fn SetCurrentProcessExplicitAppUserModelID(id: *const u16) -> i32;
        }
        unsafe {
            let hr = SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
            if hr != 0 {
                warn!(hr, "SetCurrentProcessExplicitAppUserModelID failed");
            }
        }
    }
}

pub fn fire(event: &AlertEvent) {
    imp::fire(event);
}

pub fn fire_server_unreachable() {
    imp::fire_server_unreachable();
}

pub fn fire_server_recovered() {
    imp::fire_server_recovered();
}

#[cfg(windows)]
mod imp {
    use super::AlertEvent;
    use crate::aumid;
    use tauri_winrt_notification::{IconCrop, LoopableSound, Result, Scenario, Sound, Toast};
    use tracing::{info, warn};

    fn report(tag: &str, res: Result<()>) {
        match res {
            Ok(_) => info!("toast shown: {tag}"),
            Err(e) => warn!(error = %e, "failed to show toast: {tag}"),
        }
    }

    fn base() -> Toast {
        // Send under our own AUMID once `aumid::ensure_registered` has set
        // up a Start Menu shortcut for it — Windows shows the shortcut's
        // name ("UPS Monitor Client") as the toast sender. If registration
        // failed this run, fall back to the well-known PowerShell AUMID
        // (always registered) rather than lose toasts silently: sending
        // under an *unregistered* custom AUMID makes `show()` report success
        // while nothing ever appears on screen.
        let app_id = if aumid::is_registered() {
            aumid::APP_ID
        } else {
            Toast::POWERSHELL_APP_ID
        };
        let mut toast = Toast::new(app_id);
        if let Some(icon) = aumid::toast_icon_path() {
            toast = toast.icon(&icon, IconCrop::Square, "UPS Monitor");
        }
        toast
    }

    pub fn fire(event: &AlertEvent) {
        let res = match event {
            AlertEvent::WentOnBattery { id } => base()
                .title("UPS On Battery")
                .text1(&format!("{id} has switched to battery power."))
                .scenario(Scenario::Reminder)
                .sound(Some(Sound::Reminder))
                .add_button("Dismiss", "dismiss")
                .show(),
            AlertEvent::LowBattery {
                id,
                pct,
                runtime_secs,
            } => {
                let runtime = runtime_secs
                    .map(|r| format!(" (~{} min left)", r / 60))
                    .unwrap_or_default();
                base()
                    .title("\u{26A0} SHUT DOWN NOW \u{2014} UPS Critical")
                    .text1(&format!(
                        "{id} is at {pct}% battery{runtime}. Shut down connected equipment now."
                    ))
                    .scenario(Scenario::Alarm)
                    .sound(Some(Sound::Loop(LoopableSound::Alarm2)))
                    .add_button("Dismiss", "dismiss")
                    .show()
            }
            AlertEvent::Recovered { id } => base()
                .title("UPS Back to Normal")
                .text1(&format!("{id} is back on mains power."))
                .scenario(Scenario::Default)
                .sound(Some(Sound::Default))
                .show(),
        };
        report(&format!("{event:?}"), res);
    }

    pub fn fire_server_unreachable() {
        let res = base()
            .title("UPS Monitor \u{2014} Server Unreachable")
            .text1("Cannot reach the UPS server. UPS state may be stale.")
            .scenario(Scenario::Reminder)
            .sound(Some(Sound::Reminder))
            .add_button("Dismiss", "dismiss")
            .show();
        report("server-unreachable", res);
    }

    pub fn fire_server_recovered() {
        let res = base()
            .title("UPS Monitor \u{2014} Server Reachable Again")
            .text1("Reconnected to the UPS server.")
            .scenario(Scenario::Default)
            .sound(Some(Sound::Default))
            .show();
        report("server-recovered", res);
    }
}

#[cfg(not(windows))]
mod imp {
    use super::AlertEvent;
    use notify_rust::{Notification, Timeout, Urgency};
    use tracing::{info, warn};

    fn report<T>(tag: &str, res: Result<T, notify_rust::error::Error>) {
        match res {
            Ok(_) => info!("toast shown: {tag}"),
            Err(e) => warn!(error = %e, "failed to show toast: {tag}"),
        }
    }

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
        report(&format!("{event:?}"), res);
    }

    pub fn fire_server_unreachable() {
        let res = Notification::new()
            .summary("UPS Monitor — Server Unreachable")
            .body("Cannot reach the UPS server. UPS state may be stale.")
            .urgency(Urgency::Critical)
            .timeout(Timeout::Never)
            .show();
        report("server-unreachable", res);
    }

    pub fn fire_server_recovered() {
        let res = Notification::new()
            .summary("UPS Monitor — Server Reachable Again")
            .body("Reconnected to the UPS server.")
            .urgency(Urgency::Normal)
            .timeout(Timeout::Milliseconds(5000))
            .show();
        report("server-recovered", res);
    }
}
