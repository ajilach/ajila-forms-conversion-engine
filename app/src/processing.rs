//! Unified processing invocation — shared between desktop and web.
//!
//! The `run_and_track` function abstracts away the platform-specific mechanism
//! (direct async pipeline on desktop and standalone web, or server function
//! polling on fullstack web) so that the `App` component stays completely
//! platform-agnostic.

use dioxus::prelude::*;

use crate::models::ProcessingState;
#[cfg(feature = "fullstack")]
use crate::models::ProcessingStep;

/// Run the blueprint pipeline and stream progress updates into the signal.
///
/// * **Desktop / Standalone web**: calls the async pipeline directly, updating
///   the signal between phases.
/// * **Fullstack web** (`feature = "fullstack"`): calls the `start_processing`
///   server function and polls `poll_progress` every 500 ms.
pub async fn run_and_track(
    files: Vec<(String, Vec<u8>)>,
    profile: Option<String>,
    mut processing_state: Signal<ProcessingState>,
) {
    #[cfg(not(feature = "fullstack"))]
    {
        crate::pipeline::run_blueprint_pipeline(&files, profile, |state| {
            processing_state.set(state.clone());
        })
        .await;
    }

    #[cfg(feature = "fullstack")]
    {
        use crate::platform::async_sleep_ms;
        use crate::server::{poll_progress, start_processing};

        match start_processing(files, profile).await {
            Ok(session_id) => loop {
                async_sleep_ms(500).await;
                match poll_progress(session_id.clone()).await {
                    Ok(state) => {
                        let done = state.step == ProcessingStep::Complete || state.error.is_some();
                        processing_state.set(state);
                        if done {
                            break;
                        }
                    }
                    Err(e) => {
                        processing_state.set(ProcessingState {
                            error: Some(format!("{e}")),
                            ..ProcessingState::new()
                        });
                        break;
                    }
                }
            },
            Err(e) => {
                processing_state.set(ProcessingState {
                    error: Some(format!("{e}")),
                    ..ProcessingState::new()
                });
            }
        }
    }
}
