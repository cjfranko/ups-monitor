mod aumid;
mod config;
mod detect;
mod gui;
mod logbuf;
mod notify;
mod poller;
mod state;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use tracing::{error, info};
use tracing_subscriber::fmt::writer::MakeWriterExt;

/// Show a fatal error to the user. This is a windowed exe (no console), so
/// without a message box a startup failure or panic looks like an instant,
/// silent crash — the tray icon lingers (Windows doesn't clear a dead
/// process's icon until the mouse passes over it) with no other sign
/// anything went wrong.
fn fatal_popup(msg: &str) {
    eprintln!("ups-client fatal: {msg}");
    #[cfg(windows)]
    {
        let _ = native_dialog::MessageDialog::new()
            .set_type(native_dialog::MessageType::Error)
            .set_title("UPS Monitor — Client")
            .set_text(msg)
            .show_alert();
    }
}

/// Install a panic hook that surfaces panics (windowed exe swallows them).
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("ups-client panicked: {info}");
        error!("{msg}");
        fatal_popup(&msg);
    }));
}

/// Path for the log file: next to the executable.
fn log_path() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("ups-client.log");
        }
    }
    std::path::PathBuf::from("ups-client.log")
}

fn run() -> Result<()> {
    // Log to a file next to the exe (diagnosable even without opening the
    // in-app console), to stdout (useful when launched from a terminal in
    // dev), and into an in-memory ring buffer that the tray's "Show console"
    // item displays in an in-app window.
    let log_dir = log_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let file_appender = tracing_appender::rolling::never(log_dir, "ups-client.log");
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(file_appender.and(std::io::stdout).and(logbuf::BufferWriter))
        .with_ansi(false)
        .init();

    notify::init_app_id();
    aumid::ensure_registered();

    let cfg = config::Config::load()?;
    info!(server = ?cfg.base_url(), "starting ups-client");

    let shared = state::new_shared();
    let poller = Arc::new(poller::PollerControl::new());
    poller.restart(cfg.clone(), shared.clone());

    let gui_ctx = gui::GuiContext {
        state: shared,
        config: Arc::new(Mutex::new(cfg)),
        poller,
    };

    gui::run(gui_ctx).map_err(|e| anyhow::anyhow!("GUI error: {e}"))?;

    Ok(())
}

fn main() {
    install_panic_hook();
    if let Err(e) = run() {
        let msg = format!("{e:#}");
        error!("{msg}");
        fatal_popup(&msg);
        std::process::exit(1);
    }
}
