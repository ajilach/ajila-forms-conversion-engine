//! Wires the desktop app to the conversion controller in the `pipeline` crate.
//!
//! Everything here is app-side glue: building the [`ConversionAgent`] from an
//! upload or a resumed session, implementing [`pipeline::TurnProvider`] over the
//! Anthropic transport in [`crate::llm`], implementing [`pipeline::RunObserver`]
//! against the Dioxus signal the UI renders, and projecting the finished
//! [`pipeline::RunOutcome`] onto [`ProcessingState`].
//!
//! The stage sequencing itself — Analyst → Author → (Reviewer → Author-fix)* —
//! lives in `pipeline::run`, where it can be tested without a desktop runtime.

use dioxus::prelude::*;

use agent::ConversionAgent;
use pipeline::{AbortFlag, RetryAction, RunEvent, RunObserver, TurnOutput, TurnProvider};

use crate::models::{
    AgentStep, AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep,
};

/// The choices the user made before starting a run.
pub struct RunConfig {
    pub profile: Option<String>,
    pub target: blueprint::OutputTarget,
    pub settings: crate::settings::AppSettings,
    /// Set by the Abort button to stop this run at its next checkpoint.
    pub abort: AbortFlag,
}

// ── The LLM seam: Anthropic behind pipeline::TurnProvider ────────────────────

/// Runs the controller's turns against the Anthropic Messages API.
///
/// The model id and the output cap live here rather than in the controller —
/// they are provider knowledge, and keeping them on this side is what lets the
/// pipeline crate carry no model tables at all.
struct AnthropicTurns {
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl TurnProvider for AnthropicTurns {
    async fn turn(
        &self,
        history: &mut Vec<serde_json::Value>,
        tools: &[serde_json::Value],
        system: &str,
        abort: &AbortFlag,
    ) -> Result<TurnOutput, String> {
        crate::llm::anthropic_stream_turn(
            history,
            tools,
            &self.api_key,
            &self.model,
            self.max_tokens,
            Some(system),
            abort,
        )
        .await
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
    mut processing_state: Signal<ProcessingState>,
    current_session: Signal<Option<String>>,
) {
    // An attached AEM content-package ZIP is pre-loaded as the agent's editable
    // working tree (the ConversionAgent splits PDFs vs. template internally).
    let has_template = files
        .iter()
        .any(|(_, bytes)| blueprint::detect_aem_zip(bytes));

    // Structured history session (seeded empty). Hash on the PDFs when present,
    // otherwise on the template so the session id is stable for template-only runs.
    let pdfs: Vec<(String, Vec<u8>)> = files
        .iter()
        .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".pdf"))
        .cloned()
        .collect();
    let doc_hash = crate::db::document_hash(if pdfs.is_empty() { &files } else { &pdfs });
    crate::db::upsert_document(&doc_hash, &session_label);
    let Some(structured_session) =
        crate::db::create_session(&doc_hash, config.profile.as_deref(), &session_label)
    else {
        processing_state.set(ProcessingState {
            step: ProcessingStep::Running,
            error: Some("Could not create an edit-history session.".into()),
            ..ProcessingState::default()
        });
        return;
    };
    crate::db::insert_edit(&structured_session, "Initial (empty)", "[]");

    let agent = ConversionAgent::new(
        config.profile.clone(),
        files,
        config.settings.aem_connection(),
        structured_session.clone(),
        config.target,
    );

    // An uploaded content package is an AEM artefact; it is not pre-loaded for
    // any other target, so don't tell the Author it was.
    let template_note = if has_template && config.target == blueprint::OutputTarget::Aem {
        "\n\nA template AEM tree from an uploaded content package has been pre-loaded as the \
working tree. Inspect it with get_aem_translated_outline and modify it to match the source \
instead of authoring from scratch."
    } else {
        ""
    };

    drive(
        agent,
        config,
        pipeline::RunSeed::Fresh,
        template_note,
        structured_session,
        processing_state,
        current_session,
    )
    .await;
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
    // Seed the agent from the continuing session so feedback applies to the prior
    // result: both the structured content and the AEM tree the last run authored,
    // so the Author refines that tree instead of re-deriving one from the source.
    let prior = crate::session::restore(&structured_session, config.profile.as_deref());

    let mut agent = ConversionAgent::new(
        config.profile.clone(),
        pdfs,
        config.settings.aem_connection(),
        structured_session.clone(),
        config.target,
    );
    if let Some(prior) = prior {
        agent.seed_structured(prior.envelope.content);
        // A no-op for a Redacto run, which has no AEM tree to seed.
        if let Some(tree) = prior.aem_translated {
            agent.seed_aem_translated(tree);
        }
    }

    drive(
        agent,
        config,
        pipeline::RunSeed::Feedback(feedback),
        "",
        structured_session,
        processing_state,
        current_session,
    )
    .await;
}

/// Run the controller and project its outcome onto the UI state.
async fn drive(
    agent: ConversionAgent,
    config: RunConfig,
    seed: pipeline::RunSeed,
    template_note: &'static str,
    structured_session: String,
    mut processing_state: Signal<ProcessingState>,
    mut current_session: Signal<Option<String>>,
) {
    let started_at = std::time::Instant::now();
    let RunConfig {
        profile,
        target,
        settings,
        abort,
    } = config;

    let model = settings.anthropic_model.clone();
    let max_tokens = crate::llm::max_output_tokens_for(&model);

    // Surface the context budget once so a mis-detected window is visible. This
    // is provider knowledge, so it is reported here rather than by the controller.
    let ctx_window = crate::llm::context_window_for(&model);
    let ctx_target = crate::llm::prompt_token_target(&model, max_tokens);
    processing_state.write().context_window = ctx_window;

    let mut observer = DioxusObserver {
        state: processing_state,
    };
    observer.emit(RunEvent::Thought(format!(
        "Context window: {ctx_window} tokens · per-turn budget: {ctx_target} tokens · \
         output cap: {max_tokens} · model: {model}"
    )));

    let turns = AnthropicTurns {
        api_key: settings.anthropic_api_key.clone(),
        model,
        max_tokens,
    };

    let run_config = pipeline::RunConfig {
        profile: profile.clone(),
        target,
        abort,
        max_review_rounds: settings.max_review_rounds,
        extra_instructions: crate::settings::extra_instructions_block(&settings.agent_instructions),
        template_note,
        has_aem_connection: settings.aem_connection().is_some(),
    };

    let Some(outcome) = pipeline::run(agent, run_config, seed, &turns, &mut observer).await else {
        // Aborted, or the user gave up at a retry prompt. The observer has
        // already recorded why; there is no result to publish.
        return;
    };

    // Record the result in the structured history, so the run can be reopened
    // from the session browser. Without this the session holds nothing but the
    // empty seed and there is nothing to load.
    if let Ok(json) = serde_json::to_string(&outcome.envelope) {
        crate::db::insert_edit(&structured_session, "Agent conversion", &json);
    }

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
        state.elapsed_secs = Some(started_at.elapsed().as_secs());
    }

    current_session.set(Some(structured_session));
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
    let max_tokens = crate::llm::max_output_tokens_for(&model);
    let turns = AnthropicTurns {
        api_key,
        model,
        max_tokens,
    };
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
