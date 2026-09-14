mod api;
mod config;
mod gui;
mod hid;
mod hid_desc;
mod mock;
mod poller;
mod state;

use std::collections::HashSet;

use anyhow::Result;
use tracing::{info, warn};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
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

    // Start polling.
    if mock_mode {
        let ids: Vec<String> = cfg.ups.iter().map(|u| u.id.clone()).collect();
        if !mock_fail.is_empty() {
            warn!(?mock_fail, "mock mode: forcing these UPS on battery");
        }
        poller::spawn_mock_pollers(shared.clone(), ids, mock_fail);
    } else {
        if let Err(e) = poller::log_discovery() {
            warn!(error = %e, "HID discovery failed");
        }
        poller::spawn_real_pollers(shared.clone(), cfg.ups.clone());
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
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .expect("bind API listener");
            info!(%addr, "API listening");
            axum::serve(listener, app).await.expect("serve API");
        });
    });

    // GUI runs on the main thread.
    gui::run(shared).map_err(|e| anyhow::anyhow!("GUI error: {e}"))?;

    Ok(())
}
