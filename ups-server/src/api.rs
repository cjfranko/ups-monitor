//! HTTP API: `GET /status` (JSON array of all UPS statuses, read straight from
//! the shared cache) and `GET /healthz` (liveness).

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use ups_common::UpsStatus;

use crate::state::{self, SharedState};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/status", get(get_status))
        .route("/healthz", get(healthz))
        .with_state(state)
}

async fn get_status(State(state): State<SharedState>) -> Json<Vec<UpsStatus>> {
    Json(state::snapshot(&state))
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}
