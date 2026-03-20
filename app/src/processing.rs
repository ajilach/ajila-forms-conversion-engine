//! Unified processing invocation — shared between desktop and web.
//!
//! The `run_and_track` function abstracts away the platform-specific mechanism
//! (in-process pipeline on desktop, server function polling on fullstack web,
//! or direct in-browser pipeline on standalone web) so that the `App` component
//! stays completely platform-agnostic.

use dioxus::prelude::*;

use crate::models::{ProcessingState, ProcessingStep};

/// Run the blueprint pipeline and stream progress updates into the signal.
///
/// * **Desktop** (`feature = "desktop"`): spawns a blocking task with an mpsc
///   channel and receives updates directly.
/// * **Fullstack web** (`feature = "fullstack"`): calls the `start_processing`
///   server function and polls `poll_progress` every 500 ms.
/// * **Standalone web** (WASM without fullstack): runs the pipeline directly
///   in the browser.
pub async fn run_and_track(
    files: Vec<(String, Vec<u8>)>,
    profile: Option<String>,
    mut processing_state: Signal<ProcessingState>,
) {
    #[cfg(feature = "desktop")]
    {
        use crate::pipeline::run_blueprint_pipeline;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProcessingState>();
        tokio::task::spawn_blocking(move || {
            run_blueprint_pipeline(&files, profile, |state| {
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

    #[cfg(all(feature = "web", not(feature = "fullstack"), not(feature = "desktop")))]
    {
        use crate::pipeline::run_blueprint_pipeline;
        use std::cell::RefCell;

        let ps = RefCell::new(processing_state);
        let state = run_blueprint_pipeline(&files, profile, |state| {
            ps.borrow_mut().set(state.clone());
        });
        ps.into_inner().set(state);
    }
}
