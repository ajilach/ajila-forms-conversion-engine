//! Wires the desktop app to the conversion run in the `runner` crate.
//!
//! Everything here is app-side glue: implementing [`pipeline::RunObserver`]
//! against the Dioxus signal the UI renders, and projecting the finished
//! [`runner::Completed`] onto [`ProcessingState`].
//!
//! Building the agent, opening the edit-history session and driving the
//! controller live in [`runner::run`] — shared with the CLI, so both start a run
//! the same way. The stage sequencing itself — Analyst → Author → (Reviewer →
//! Author-fix)* — lives in `pipeline::run`, where it can be tested without a
//! desktop runtime.

use dioxus::prelude::*;

use pipeline::{AbortFlag, RetryAction, RunEvent, RunObserver};
use runner::TurnPlan;

use crate::models::{AgentStep, AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep};

/// The choices the user made before starting a run.
pub struct RunConfig {
    pub profile: Option<String>,
    pub target: blueprint::OutputTarget,
    pub settings: crate::settings::AppSettings,
    /// Set by the Abort button to stop this run at its next checkpoint.
    pub abort: AbortFlag,
}

impl RunConfig {
    fn into_options(self) -> runner::RunOptions {
        runner::RunOptions {
            profile: self.profile,
            target: self.target,
            settings: self.settings,
            abort: self.abort,
        }
    }
}

// ── The progress seam: ProcessingState behind pipeline::RunObserver ──────────

/// Publishes the controller's progress into the signal the UI renders, and reads
/// the Retry button's answer back out.
struct DioxusObserver {
    state: Signal<ProcessingState>,
}

impl DioxusObserver {
    fn push(&mut self, step: AgentStep) {
        self.state.write().agent_steps.push(step);
    }

    fn thought(&mut self, label: impl Into<String>) {
        self.push(AgentStep {
            id: String::new(),
            kind: AgentStepKind::Thought,
            label: label.into(),
            detail: String::new(),
            status: AgentStepStatus::Done,
        });
    }
}

impl RunObserver for DioxusObserver {
    fn emit(&mut self, event: RunEvent) {
        match event {
            RunEvent::Stage { role, doing } => self.thought(format!("── {role} — {doing} ──")),
            RunEvent::Thought(text) => self.thought(text),
            RunEvent::ToolStarted {
                id,
                name,
                input_summary,
            } => self.push(AgentStep {
                id,
                kind: AgentStepKind::Tool,
                label: name,
                detail: input_summary,
                status: AgentStepStatus::Running,
            }),
            RunEvent::ToolFinished { id, ok } => {
                let mut s = self.state.write();
                if let Some(step) = s.agent_steps.iter_mut().rev().find(|s| s.id == id) {
                    step.status = ok.into();
                }
            }
            RunEvent::Warning(w) => self.state.write().warnings.push(w),
            RunEvent::ContextUsed(tokens) => self.state.write().context_used_tokens = tokens,
            // Emitted at every abort checkpoint, so record it only once.
            RunEvent::Aborted => {
                if !self.state.read().aborted {
                    self.state.write().aborted = true;
                    self.thought("Run aborted by the user.");
                }
            }
        }
    }

    fn retry_prompt(&mut self, role: &str, error: &str) {
        let mut s = self.state.write();
        s.error = Some(format!("Agent failed ({role}): {error}"));
        s.retry_pending = true;
        s.retry_action = None;
    }

    fn poll_retry(&mut self) -> Option<RetryAction> {
        self.state.read().retry_action
    }

    fn retry_resolved(&mut self, action: RetryAction) {
        let mut s = self.state.write();
        s.retry_pending = false;
        s.retry_action = None;
        if action == RetryAction::Retry {
            s.error = None;
        }
    }
}

// ── Public entry points ──────────────────────────────────────────────────────

/// Run the autonomous conversion pipeline end-to-end on a fresh upload.
pub async fn run_agent(
    files: Vec<(String, Vec<u8>)>,
    config: RunConfig,
    session_label: String,
    processing_state: Signal<ProcessingState>,
    current_session: Signal<Option<String>>,
) {
    let opts = config.into_options();
    let mut observer = announce(&opts, processing_state);
    let completed = runner::run_fresh(files, &opts, &session_label, &mut observer).await;
    publish(completed, opts.target, processing_state, current_session);
}

/// Resume on an existing session to apply the user's feedback. Skips the Analyst;
/// the feedback becomes the first pinned "review" and the Author applies it, then
/// the Reviewer→fix loop runs as usual.
pub async fn run_agent_feedback(
    feedback: String,
    pdfs: Vec<(String, Vec<u8>)>,
    config: RunConfig,
    structured_session: String,
    processing_state: Signal<ProcessingState>,
    current_session: Signal<Option<String>>,
) {
    let opts = config.into_options();
    let mut observer = announce(&opts, processing_state);
    let completed =
        runner::run_feedback(feedback, pdfs, &opts, structured_session, &mut observer).await;
    publish(completed, opts.target, processing_state, current_session);
}

/// Surface the run's token budget, so a mis-detected context window is visible,
/// and hand back the observer the run will report through.
fn announce(
    opts: &runner::RunOptions,
    mut processing_state: Signal<ProcessingState>,
) -> DioxusObserver {
    let plan = TurnPlan::for_settings(&opts.settings);
    processing_state.write().context_window = plan.context_window;

    let mut observer = DioxusObserver {
        state: processing_state,
    };
    observer.emit(RunEvent::Thought(plan.describe()));
    observer
}

/// Project a finished run onto the UI state.
fn publish(
    completed: Result<runner::Completed, String>,
    target: blueprint::OutputTarget,
    mut processing_state: Signal<ProcessingState>,
    mut current_session: Signal<Option<String>>,
) {
    let completed = match completed {
        Ok(completed) => completed,
        Err(e) => {
            processing_state.set(ProcessingState {
                step: ProcessingStep::Running,
                error: Some(e),
                ..ProcessingState::default()
            });
            return;
        }
    };

    // Aborted, or the user gave up at a retry prompt. The observer has already
    // recorded why; there is no result to publish.
    let Some(outcome) = completed.outcome else {
        return;
    };

    {
        let mut state = processing_state.write();
        state.warnings.extend(outcome.warnings);
        state.step = ProcessingStep::Complete;
        state.target = target;
        state.xsd_schema = outcome.xsd_schema;
        state.aem_package = outcome.aem_package;
        state.aem_package_bound = outcome.aem_package_bound;
        state.redacto_sql = outcome.redacto_sql;
        state.form_code = outcome.form_code;
        state.aem_uploaded = outcome.aem_uploaded;
        state.aem_form_path = outcome.aem_form_path;
        state.elapsed_secs = Some(completed.elapsed_secs);
    }

    current_session.set(Some(completed.session_id));
}

/// Describe a reference form so the reference store can match against it.
///
/// The same stage machinery as a conversion, over the same tool catalog — this
/// wrapper only supplies the transport and swallows progress, since the
/// references page reports status itself.
pub async fn describe_reference(
    profile: &str,
    pdfs: Vec<(String, Vec<u8>)>,
    package_zip: Vec<u8>,
    api_key: String,
    model: String,
) -> Result<String, String> {
    let turns = TurnPlan::for_model(&model).provider(api_key);
    pipeline::describe::describe_reference(
        profile,
        pdfs,
        package_zip,
        &AbortFlag::default(),
        &turns,
        &mut pipeline::NullObserver,
    )
    .await
}
