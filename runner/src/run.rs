//! Starting a run: everything both consumers do around [`pipeline::run`].
//!
//! Building the [`ConversionAgent`] from a file set, opening (or continuing) an
//! edit-history session, resolving the turn provider from the operator settings,
//! and recording the finished envelope back into the history. What is left for a
//! consumer is the [`pipeline::RunObserver`] — how progress is shown and how a
//! retry prompt is answered — and what to do with the artefacts.

use std::time::Instant;

use agent::ConversionAgent;
use blueprint::OutputTarget;
use pipeline::{AbortFlag, RunObserver, RunOutcome, RunSeed};

use crate::settings::AppSettings;
use crate::turns::TurnPlan;

/// Reported when the edit-history session cannot be opened. A run without one
/// would build fine and then have nowhere to be reviewed from, so it is a hard
/// stop rather than a warning.
pub const NO_SESSION: &str = "Could not create an edit-history session.";

/// The choices the operator made before starting a run.
pub struct RunOptions {
    pub profile: Option<String>,
    pub target: OutputTarget,
    pub settings: AppSettings,
    /// Set by the caller's stop control to end this run at its next checkpoint.
    pub abort: AbortFlag,
}

/// What a finished — or stopped — run leaves behind.
pub struct Completed {
    /// The edit-history session the run was recorded into. Feed it back to
    /// [`run_feedback`] to refine the result.
    pub session_id: String,
    /// `None` when the run stopped before producing anything: the operator
    /// aborted, or gave up at a retry prompt. The observer already said why.
    pub outcome: Option<RunOutcome>,
    pub elapsed_secs: u64,
}

/// Run the autonomous conversion end to end on a fresh file set.
///
/// `files` may hold the source PDF(s), an AEM content-package ZIP to use as an
/// editable template, or both.
pub async fn run_fresh(
    files: Vec<(String, Vec<u8>)>,
    opts: &RunOptions,
    session_label: &str,
    obs: &mut impl RunObserver,
) -> Result<Completed, String> {
    // An attached AEM content-package ZIP is pre-loaded as the agent's editable
    // working tree (the ConversionAgent splits PDFs vs. template internally).
    let has_template = files
        .iter()
        .any(|(_, bytes)| blueprint::detect_aem_zip(bytes));

    // Hash on the PDFs when present, otherwise on the template, so the session
    // id is stable for template-only runs.
    let pdfs: Vec<(String, Vec<u8>)> = files
        .iter()
        .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".pdf"))
        .cloned()
        .collect();
    let doc_hash = agent::db::document_hash(if pdfs.is_empty() { &files } else { &pdfs });
    agent::db::upsert_document(&doc_hash, session_label);
    let session_id = agent::db::create_session(&doc_hash, opts.profile.as_deref(), session_label)
        .ok_or_else(|| NO_SESSION.to_string())?;
    agent::db::insert_edit(&session_id, "Initial (empty)", "[]");

    let agent = ConversionAgent::new(
        opts.profile.clone(),
        files,
        opts.settings.aem_connection(),
        session_id.clone(),
        opts.target,
    );

    // An uploaded content package is an AEM artefact; it is not pre-loaded for
    // any other target, so don't tell the Author it was.
    let template_note = if has_template && opts.target == OutputTarget::Aem {
        "\n\nA template AEM tree from an uploaded content package has been pre-loaded as the \
working tree. Inspect it with get_aem_translated_outline and modify it to match the source \
instead of authoring from scratch."
    } else {
        ""
    };

    Ok(drive(agent, opts, RunSeed::Fresh, template_note, session_id, obs).await)
}

/// Resume an existing session to apply the operator's feedback.
///
/// Skips the Analyst: the feedback becomes the first pinned "review", the Author
/// applies it, then the Reviewer→fix loop runs as usual.
pub async fn run_feedback(
    feedback: String,
    pdfs: Vec<(String, Vec<u8>)>,
    opts: &RunOptions,
    session_id: String,
    obs: &mut impl RunObserver,
) -> Result<Completed, String> {
    // Seed the agent from the continuing session so feedback applies to the prior
    // result: both the structured content and the AEM tree the last run authored,
    // so the Author refines that tree instead of re-deriving one from the source.
    let prior = agent::session::restore(&session_id, opts.profile.as_deref());

    let mut agent = ConversionAgent::new(
        opts.profile.clone(),
        pdfs,
        opts.settings.aem_connection(),
        session_id.clone(),
        opts.target,
    );
    if let Some(prior) = prior {
        agent.seed_structured(prior.envelope.content);
        // A no-op for a Redacto run, which has no AEM tree to seed.
        if let Some(tree) = prior.aem_translated {
            agent.seed_aem_translated(tree);
        }
    }

    Ok(drive(
        agent,
        opts,
        RunSeed::Feedback(feedback),
        "",
        session_id,
        obs,
    )
    .await)
}

/// Drive the controller over `agent` and record what it produced.
async fn drive(
    agent: ConversionAgent,
    opts: &RunOptions,
    seed: RunSeed,
    template_note: &'static str,
    session_id: String,
    obs: &mut impl RunObserver,
) -> Completed {
    let started_at = Instant::now();

    // The transport reads its eviction tuning from process-wide state, so a
    // consumer that never opened a settings screen still honours the saved
    // configuration. Idempotent, so applying it per run is free.
    opts.settings.apply_runtime_config();

    let turns = TurnPlan::for_settings(&opts.settings).provider(opts.settings.active_api_key());

    let run_config = pipeline::RunConfig {
        profile: opts.profile.clone(),
        target: opts.target,
        abort: opts.abort.clone(),
        max_review_rounds: opts.settings.max_review_rounds,
        extra_instructions: crate::settings::extra_instructions_block(
            &opts.settings.agent_instructions,
        ),
        template_note,
        has_aem_connection: opts.settings.aem_connection().is_some(),
    };

    let outcome = pipeline::run(agent, run_config, seed, &turns, obs).await;

    // Record the result in the structured history, so the run can be reopened
    // from the session browser. Without this the session holds nothing but the
    // empty seed and there is nothing to load.
    if let Some(outcome) = &outcome
        && let Ok(json) = serde_json::to_string(&outcome.envelope)
    {
        agent::db::insert_edit(&session_id, "Agent conversion", &json);
    }

    Completed {
        session_id,
        outcome,
        elapsed_secs: started_at.elapsed().as_secs(),
    }
}
