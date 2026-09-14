mod config;
mod detect;
mod gui;
mod notify;
mod poller;
mod state;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use tracing::info;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    notify::init_app_id();

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
