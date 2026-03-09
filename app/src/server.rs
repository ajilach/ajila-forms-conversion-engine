//! Server-side session store and server functions for fullstack mode.

use dioxus::prelude::*;
use std::collections::HashMap;

use crate::models::ProcessingState;

// ── Server-side session store for incremental progress ───────────────

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub static SESSIONS: std::sync::LazyLock<std::sync::Mutex<HashMap<String, ProcessingState>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
pub fn next_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("s{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

// ── Server functions (fullstack) ─────────────────────────────────────

/// Return the names of all embedded profiles.
#[server]
pub async fn get_profiles() -> Result<Vec<String>, ServerFnError> {
    use crate::profiles;
    Ok(profiles::list_profiles())
}

#[server]
pub async fn start_processing(
    files: Vec<(String, Vec<u8>)>,
    profile: Option<String>,
) -> Result<String, ServerFnError> {
    use crate::models::ProcessingStep;
    use crate::pipeline::run_blueprint_pipeline;

    let session_id = next_session_id();
    SESSIONS.lock().unwrap().insert(
        session_id.clone(),
        ProcessingState {
            step: ProcessingStep::Parsing,
            ..ProcessingState::new()
        },
    );

    let sid = session_id.clone();
    std::thread::spawn(move || {
        let final_state = run_blueprint_pipeline(&files, profile, |state| {
            SESSIONS.lock().unwrap().insert(sid.clone(), state.clone());
        });
        SESSIONS.lock().unwrap().insert(sid, final_state);
    });

    Ok(session_id)
}

#[server]
pub async fn poll_progress(session_id: String) -> Result<ProcessingState, ServerFnError> {
    use crate::models::ProcessingStep;

    let state = SESSIONS
        .lock()
        .unwrap()
        .get(&session_id)
        .cloned()
        .ok_or_else(|| ServerFnError::new("Session not found"))?;

    // Clean up completed sessions
    if state.step == ProcessingStep::Complete || state.error.is_some() {
        SESSIONS.lock().unwrap().remove(&session_id);
    }

    Ok(state)
}
