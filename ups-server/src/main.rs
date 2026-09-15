mod api;
mod config;
mod gui;
mod hid;
mod hid_desc;
mod logbuf;
mod mock;
mod poller;
mod state;

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use tracing::{error, info, warn};
use tracing_subscriber::fmt::writer::MakeWriterExt;

/// Show a fatal error to the user. This is a windowed exe (no console), so
/// without a message box a startup failure looks like an instant, silent
/// crash.
fn fatal_popup(msg: &str) {
    eprintln!("ups-server fatal: {msg}");
    #[cfg(windows)]
    {
        let _ = native_dialog::MessageDialog::new()
            .set_type(native_dialog::MessageType::Error)
            .set_title("UPS Monitor — Server")
            .set_text(msg)
            .show_alert();
    }
}

/// Install a panic hook that surfaces panics (windowed exe swallows them).
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("ups-server panicked: {info}");
        error!("{msg}");
        fatal_popup(&msg);
    }));
}

/// Path for the log file: next to the executable.
fn log_path() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("ups-server.log");
        }
    }
    std::path::PathBuf::from("ups-server.log")
}

fn run() -> Result<()> {
    // Log to a file next to the exe, and also into an in-memory ring buffer
    // that the tray's "Show console" item displays in an in-app window —
    // this is a windowed exe with no real console to write to.
    let log_dir = log_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let file_appender = tracing_appender::rolling::never(log_dir, "ups-server.log");
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(file_appender.and(logbuf::BufferWriter))
        .with_ansi(false)
        .init();

    let args: Vec<String> = std::env::args().collect();
    let mock_mode = args.iter().any(|a| a == "--mock");
    let mock_fail: HashSet<String> = args
        .iter()
        .position(|a| a == "--mock-fail")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default();

    let cfg = config::Config::load()?;
    info!(
        bind = %cfg.server.bind_ip,
        port = cfg.server.port,
        ups = cfg.ups.len(),
        mock = mock_mode,
        "starting ups-server"
    );

    let shared = state::new_shared_state(&cfg.ups);
    let handles = poller::new_handles();

    // Start polling.
    if mock_mode {
        let ids: Vec<String> = cfg.ups.iter().map(|u| u.id.clone()).collect();
        if !mock_fail.is_empty() {
            warn!(?mock_fail, "mock mode: forcing these UPS on battery");
        }
        poller::spawn_mock_pollers(&handles, shared.clone(), ids, mock_fail);
    } else {
        if let Err(e) = poller::log_discovery() {
            warn!(error = %e, "HID discovery failed");
        }
        poller::spawn_real_pollers(&handles, shared.clone(), cfg.ups.clone());
    }

    // Start the HTTP API on a background tokio runtime.
    let api_state = shared.clone();
    let bind_ip = cfg.server.bind_ip.clone();
    let port = cfg.server.port;
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            let app = api::router(api_state);
            let addr = format!("{bind_ip}:{port}");
            let listener = match tokio::net::TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    error!(%addr, error = %e, "failed to bind API listener (port in use?)");
                    fatal_popup(&format!("Cannot bind {addr}: {e}\n\nIs another instance of ups-server already running?"));
                    std::process::exit(1);
                }
            };
            info!(%addr, "API listening");
            if let Err(e) = axum::serve(listener, app).await {
                error!(error = %e, "API server stopped");
            }
        });
    });

    // GUI runs on the main thread.
    let gui_ctx = gui::GuiContext {
        state: shared,
        handles,
        config: Arc::new(std::sync::Mutex::new(cfg)),
        mock_mode,
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
