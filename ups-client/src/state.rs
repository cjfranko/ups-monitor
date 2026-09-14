//! Client-side shared state between the polling thread and the GUI.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use ups_common::UpsStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connectivity {
    Connected,
    Unreachable,
}

#[derive(Debug, Clone)]
pub struct ClientState {
    pub connectivity: Connectivity,
    pub last_success: Option<DateTime<Utc>>,
    pub statuses: Vec<UpsStatus>,
}

impl Default for ClientState {
    fn default() -> Self {
        Self {
            connectivity: Connectivity::Unreachable,
            last_success: None,
            statuses: Vec::new(),
        }
    }
}

pub type SharedClientState = Arc<RwLock<ClientState>>;

pub fn new_shared() -> SharedClientState {
    Arc::new(RwLock::new(ClientState::default()))
}

/// Merge a fresh server snapshot into the state, marking connectivity good.
pub fn apply_success(state: &SharedClientState, statuses: Vec<UpsStatus>) {
    if let Ok(mut s) = state.write() {
        s.statuses = statuses;
        s.connectivity = Connectivity::Connected;
        s.last_success = Some(Utc::now());
    }
}

pub fn apply_failure(state: &SharedClientState, unreachable: bool) {
    if let Ok(mut s) = state.write() {
        if unreachable {
            s.connectivity = Connectivity::Unreachable;
        }
    }
}

/// Snapshot keyed by id for the GUI.
pub fn snapshot(state: &SharedClientState) -> ClientState {
    state
        .read()
        .map(|s| s.clone())
        .unwrap_or_default()
}

/// Convenience: map form.
#[allow(dead_code)] // handy for future GUI/tests; not used yet
pub fn by_id(state: &SharedClientState) -> HashMap<String, UpsStatus> {
    snapshot(state)
        .statuses
        .into_iter()
        .map(|s| (s.id.clone(), s))
        .collect()
}
