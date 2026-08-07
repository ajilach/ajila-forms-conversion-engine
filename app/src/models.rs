//! The state the agent run publishes to the UI.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cooperative cancellation for a run, shared between the Abort button and the
/// run itself.
///
/// An atomic rather than a field on [`ProcessingState`]: the network layer polls
/// it from inside a streamed response, where the UI's signals are out of reach,
/// and a long turn has to stop there rather than at the next turn boundary.
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

/// Two handles are the same flag when they share one cell — what a component
/// prop needs to know, and what comparing the booleans would get wrong.
impl PartialEq for AbortFlag {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProcessingStep {
    #[default]
    Idle,
    /// The agent run is under way.
    Running,
    Complete,
}

/// Kind of an agent activity step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentStepKind {
    /// The model's visible text for a turn.
    Thought,
    /// A tool call.
    Tool,
}

/// Status of an agent activity step (drives the spinner / checkmark).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentStepStatus {
    Running,
    Done,
    Error,
}

impl AgentStepStatus {
    /// The glyph that stands for this status wherever a step is rendered — the
    /// timeline dots and the Markdown transcript alike, so the three views can
    /// never drift apart.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Running => "…",
            Self::Done => "✓",
            Self::Error => "✗",
        }
    }

    /// Modifier class for the timeline dot that carries this status.
    pub fn dot_class(self) -> &'static str {
        match self {
            Self::Running => "run",
            Self::Done => "ok",
            Self::Error => "err",
        }
    }
}

impl From<bool> for AgentStepStatus {
    /// A finished tool call is `Done` when it succeeded and `Error` when it did
    /// not; the agent loop reports exactly that boolean.
    fn from(ok: bool) -> Self {
        if ok { Self::Done } else { Self::Error }
    }
}

/// What the user chose when an agent run paused on a failed API turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryAction {
    /// Re-send the failed turn and carry on.
    Retry,
    /// Give up on the run (the agent keeps whatever it had built).
    Cancel,
}

/// One entry in the Agent Processing activity panel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentStep {
    /// Tool-call id (for matching start→finish); empty for thoughts.
    pub id: String,
    pub kind: AgentStepKind,
    /// Tool name, or the thought text.
    pub label: String,
    /// Short input summary for tool steps.
    pub detail: String,
    pub status: AgentStepStatus,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessingState {
    pub step: ProcessingStep,
    /// What the finished run produced. Recorded here rather than read from the
    /// upload selector so the result panel always describes the run that
    /// actually happened.
    pub target: blueprint::OutputTarget,
    pub form_code: Option<String>,
    pub aem_package: Option<Vec<u8>>,
    pub xsd_schema: Option<String>,
    /// PostgreSQL dump for the Redacto platform (text-only documents).
    pub redacto_sql: Option<String>,
    pub error: Option<String>,
    /// The user stopped the run. Terminal like [`Self::error`], but not a
    /// failure — the box says so rather than reporting an error nobody hit.
    pub aborted: bool,
    /// `true` while an agent run is paused on a failed API turn, waiting for the
    /// user to press Retry (or give up). The run's future is still alive — the
    /// agent, its working tree and the stage history are all held in memory — so
    /// a retry resumes at the failed turn instead of restarting the run.
    pub retry_pending: bool,
    /// The user's answer to a pending retry prompt, set by the progress UI and
    /// consumed by the paused agent loop.
    pub retry_action: Option<RetryAction>,
    pub warnings: Vec<String>,
    /// Live activity log for the Agent Processing run (thoughts + tool calls).
    pub agent_steps: Vec<AgentStep>,
    /// `true` once the agent has successfully uploaded + installed the built
    /// package on the configured AEM instance during its run.
    pub aem_uploaded: bool,
    /// JCR path of the uploaded form on AEM, shown on the agent "done" screen.
    pub aem_form_path: Option<String>,
    /// Wall-clock duration of the most recent agent run, in seconds. Shown
    /// next to "Finished" on the agent "done" screen.
    pub elapsed_secs: Option<u64>,
    /// Latest real prompt-token count sent to the model this run (from the API's
    /// reported usage), for the context-window fill indicator. 0 before the first
    /// turn reports usage.
    pub context_used_tokens: usize,
    /// The model's context window in tokens — the denominator of the fill
    /// indicator. 0 until the agent run sets it.
    pub context_window: usize,
}
