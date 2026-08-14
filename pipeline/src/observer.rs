//! The seam between the conversion controller and whatever is watching it.
//!
//! The controller reports progress and asks for retry decisions through
//! [`RunObserver`]; it never touches a UI framework. A desktop app implements it
//! against its own state, a test implements it as a recorder, and a headless
//! caller uses [`NullObserver`].
//!
//! Deliberately synchronous, and a trait rather than a channel: the exchange is
//! bidirectional (the controller *asks* for a retry decision and waits), so a
//! one-way channel would need a second channel back plus a correlation protocol.
//! Keeping it sync means no `async fn` in trait, no boxing, and the controller
//! keeps owning its own sleeps.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cooperative cancellation for a run, shared between the caller's stop control
/// and the run itself.
///
/// An atomic rather than a field on any state object: the network layer polls it
/// from inside a streamed response, where a UI's signals are out of reach, and a
/// long turn has to stop there rather than at the next turn boundary.
#[derive(Clone, Debug, Default)]
pub struct AbortFlag(Arc<AtomicBool>);

impl AbortFlag {
    /// Ask the run to stop at its next checkpoint.
    pub fn abort(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_aborted(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Clear the flag so the next run starts un-aborted.
    pub fn reset(&self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// Two handles are the same flag when they share one cell — what a UI component
/// prop needs to know, and what comparing the booleans would get wrong.
impl PartialEq for AbortFlag {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What the user chose when a run paused on a failed API turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryAction {
    /// Re-send the failed turn and carry on.
    Retry,
    /// Give up on the run (the agent keeps whatever it had built).
    Cancel,
}

/// Everything the controller tells the outside world while a run is in flight.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunEvent {
    /// A pipeline stage started.
    Stage { role: &'static str, doing: String },
    /// The model's visible text for a turn, or a note from the controller.
    Thought(String),
    ToolStarted {
        id: String,
        name: String,
        input_summary: String,
    },
    ToolFinished { id: String, ok: bool },
    /// Something the finished run should report but that did not stop it.
    Warning(String),
    /// Prompt tokens sent on the latest turn, for a context-fill indicator.
    ContextUsed(usize),
    /// The run stopped because the abort flag was set. May be emitted more than
    /// once — every abort checkpoint reports it, and implementations are
    /// expected to be idempotent.
    Aborted,
}

/// Receives a run's progress and answers its retry prompts.
pub trait RunObserver {
    fn emit(&mut self, event: RunEvent);

    /// A turn failed unrecoverably and the run is now paused. Called once,
    /// before [`poll_retry`](Self::poll_retry) starts.
    fn retry_prompt(&mut self, role: &str, error: &str);

    /// The operator's answer to the pending prompt, or `None` to keep waiting.
    /// Polled on a short interval while a turn is paused.
    fn poll_retry(&mut self) -> Option<RetryAction>;

    /// The pause ended, either by an answer or by the run being aborted.
    fn retry_resolved(&mut self, action: RetryAction);
}

/// Discards progress and never retries — for headless callers and tests that
/// only care about the outcome. A failed turn ends the run rather than hanging
/// forever waiting for an answer nobody is there to give.
#[derive(Debug, Default)]
pub struct NullObserver;

impl RunObserver for NullObserver {
    fn emit(&mut self, _event: RunEvent) {}
    fn retry_prompt(&mut self, _role: &str, _error: &str) {}
    fn poll_retry(&mut self) -> Option<RetryAction> {
        Some(RetryAction::Cancel)
    }
    fn retry_resolved(&mut self, _action: RetryAction) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Abort button holds one handle and the run another; setting the flag
    /// on either has to be visible to the other, or the button does nothing.
    #[test]
    fn every_handle_to_a_flag_observes_the_abort() {
        let flag = AbortFlag::default();
        let held_by_the_run = flag.clone();

        assert!(!held_by_the_run.is_aborted());
        flag.abort();
        assert!(held_by_the_run.is_aborted());

        held_by_the_run.reset();
        assert!(!flag.is_aborted());
    }

    #[test]
    fn separate_flags_are_not_equal() {
        let a = AbortFlag::default();
        let b = AbortFlag::default();
        assert_eq!(a, a.clone(), "clones share a cell");
        assert_ne!(a, b, "independent flags are distinct even when both are false");
    }

    #[test]
    fn the_null_observer_gives_up_rather_than_hanging() {
        let mut obs = NullObserver;
        assert_eq!(obs.poll_retry(), Some(RetryAction::Cancel));
    }
}
