//! Unified processing invocation — shared between desktop and web.
//!
//! The `run_and_track` function abstracts away the platform-specific mechanism
//! (in-process pipeline on desktop vs server function polling on web) so that
//! the `App` component stays completely platform-agnostic.

use dioxus::prelude::*;

use crate::models::{ProcessingState, ProcessingStep};

/// Run the blueprint pipeline and stream progress updates into the signal.
///
/// * **Desktop** (`feature = "desktop"`): spawns a blocking task with an mpsc
///   channel and receives updates directly.
/// * **Web** (default): calls the `start_processing` server function and polls
///   `poll_progress` every 200 ms.
pub async fn run_and_track(
    files: Vec<(String, Vec<u8>)>,
    mut processing_state: Signal<ProcessingState>,
) {
    #[cfg(feature = "desktop")]
    {
        use crate::pipeline::run_blueprint_pipeline;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProcessingState>();
        tokio::task::spawn_blocking(move || {
            run_blueprint_pipeline(&files, |state| {
                let _ = tx.send(state.clone());
            })
        });
        while let Some(state) = rx.recv().await {
            let done = state.step == ProcessingStep::Complete || state.error.is_some();
            processing_state.set(state);
            if done {
                break;
            }
        }
    }

    #[cfg(feature = "web")]
    {
        use crate::platform::async_sleep_ms;
        use crate::server::{poll_progress, start_processing};

        match start_processing(files).await {
            Ok(session_id) => {
                loop {
                    async_sleep_ms(200).await;
                    match poll_progress(session_id.clone()).await {
                        Ok(state) => {
                            let done = state.step == ProcessingStep::Complete
                                || state.error.is_some();
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
                }
            }
            Err(e) => {
                processing_state.set(ProcessingState {
                    error: Some(format!("{e}")),
                    ..ProcessingState::new()
                });
            }
        }
    }
}
