mod config;
mod detect;
mod gui;
mod notify;
mod poller;
mod state;

use anyhow::Result;
use tracing::info;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = config::Config::load()?;
    info!(server = %cfg.base_url(), "starting ups-client");

    let shared = state::new_shared();

    // Poller on a background thread; GUI on the main thread.
    {
        let cfg = cfg.clone();
        let shared = shared.clone();
        std::thread::spawn(move || poller::run(cfg, shared));
    }

    gui::run(shared).map_err(|e| anyhow::anyhow!("GUI error: {e}"))?;

    Ok(())
}
