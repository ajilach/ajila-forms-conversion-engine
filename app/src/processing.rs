//! Unified processing invocation — shared between desktop and web.

use dioxus::prelude::*;

use crate::models::ProcessingState;

/// Run the blueprint pipeline and stream progress updates into the signal.
pub async fn run_and_track(
    files: Vec<(String, Vec<u8>)>,
    profile: Option<String>,
    mut processing_state: Signal<ProcessingState>,
) {
    crate::pipeline::run_blueprint_pipeline(&files, profile, |state| {
        processing_state.set(state.clone());
    })
    .await;
}
