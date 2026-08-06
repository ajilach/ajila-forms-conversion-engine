//! The LLM agent pipeline that drives the headless [`agent::ConversionAgent`].
//!
//! This is the UI/LLM half of the autonomous conversion agent. The run is split
//! into three specialized stages — an **Analyst** (read-only analysis + precedent
//! research → a conversion plan), a monolithic **Author** (builds the AEM tree),
//! and an independent **Reviewer** (approves, or returns a report that drives a
//! bounded fix loop). All three share one [`ConversionAgent`] (the working
//! `AemNodeTranslated` tree + edit-history session) and reuse the SAME turn
//! machinery ([`crate::platform::anthropic_stream_turn`]): eviction, prompt
//! caching, the context-window budget + 400-retry, off-thread request prep and
//! the context-window indicator. Each stage runs with its own system prompt and a
//! scoped subset of the tools, on a fresh bounded history — the Analyst's plan and
//! the Reviewer's reports are pinned into the `system` field so they survive the
//! whole run without being evicted.
//!
//! `run_role` is a thin generalization of the old single loop; the per-stage
//! sequencing lives in `run_conversion`.

use dioxus::prelude::*;

use agent::{
    ANALYST_ADDENDUM, AUTHOR_ADDENDUM, ConversionAgent, REDACTO_ANALYST_ADDENDUM,
    REDACTO_AUTHOR_ADDENDUM, REDACTO_REVIEWER_ADDENDUM, REDACTO_SHARED_PREAMBLE,
    REDACTO_SYSTEM_PROMPT, REVIEWER_ADDENDUM, SHARED_PREAMBLE, SYSTEM_PROMPT, ToolReply,
};
use blueprint::DocumentEnvelope;

use crate::models::{
    AgentStep, AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep, RetryAction,
};
use crate::platform::tool_result_message;

/// Fallback output-token cap per agent turn, used for models we don't recognize
/// in [`max_output_tokens_for`].
const AGENT_MAX_TOKENS: u32 = 16000;

/// The output-token ceiling to request for a given model. The agent loop streams
/// every turn (see `anthropic_stream_turn`), so we can request up to the model's
/// true max output without risking the HTTP timeouts that cap non-streaming
/// requests near 16k. `max_tokens` is a ceiling, not a target — we're billed only
/// for tokens actually generated — so requesting the full max costs nothing extra
/// and just lets a large authoring turn complete in one call.
///
/// Matches on family substrings so date/suffix variants (e.g. `-20251001`,
/// `[1m]`) still resolve; unrecognized models fall back to [`AGENT_MAX_TOKENS`].
fn max_output_tokens_for(model: &str) -> u32 {
    if model.contains("haiku") {
        64_000
    } else if model.contains("opus-4-8")
        || model.contains("opus-4-7")
        || model.contains("opus-4-6")
        || model.contains("sonnet-5")
        || model.contains("sonnet-4-6")
        || model.contains("fable-5")
    {
        128_000
    } else {
        AGENT_MAX_TOKENS
    }
}

/// How many times a *transient* API failure (timeout, dropped connection,
/// overload, rate limit, 5xx) is retried automatically before the run pauses and
/// asks the user. A turn that fails mid-stream has not been appended to the
/// stage history, so re-sending it is safe — the request is simply rebuilt from
/// the unchanged history.
const MAX_AUTO_RETRIES: usize = 6;
/// Base delay before the first automatic retry, doubled on each further attempt
/// and capped at [`MAX_RETRY_BACKOFF_SECS`].
const RETRY_BACKOFF_SECS: u64 = 5;
/// Ceiling for the exponential retry backoff.
const MAX_RETRY_BACKOFF_SECS: u64 = 60;
/// How often the paused loop checks whether the user pressed Retry.
const RETRY_POLL_MS: u64 = 200;

/// How many consecutive `validate_aem_package` calls with identical output
/// are allowed before a stage gives up (avoids an endless validate loop).
const MAX_VALIDATE_REPEATS: usize = 3;
/// How many consecutive turns that overflow the output-token cap we nudge
/// toward incremental authoring before giving up (avoids an endless loop if the
/// model keeps trying to emit one oversized call regardless).
const MAX_MAX_TOKEN_NUDGES: usize = 3;

/// Injected when a turn is cut off at the output-token cap — almost always
/// mid-way through one oversized tool call (a monolithic `set_aem_translated`
/// for a large form). Steers the agent to author the tree incrementally so no
/// single call has to fit under the output-token cap.
const MAX_TOKENS_NUDGE: &str = "\
Your previous turn was cut off at the output-token limit before it completed — that call \
was NOT executed. This almost always means you tried to emit too much in a single tool call \
(e.g. authoring a whole large form in one set_aem_translated). Do NOT retry it as one call. \
Instead author the tree incrementally so no single call is oversized:\n\
1. Call set_aem_translated with a SMALL skeleton only: the Root plus one empty Panel per \
top-level section (titles set, no inner fields yet).\n\
2. Then fill in each section one at a time with insert_aem_translated_node (add each field / \
sub-panel into its section's Panel), replace_aem_translated_node and set_aem_translated_field.\n\
Keep every individual call small. Proceed now.";

// ── Roles ────────────────────────────────────────────────────────────────────

/// A pipeline stage: a name, the subset of the agent's tools it may call, and a
/// per-stage turn budget. The system prompt and seed message are supplied per
/// invocation by `run_conversion` (so the Analyst's plan / Reviewer reports can be
/// pinned into `system`).
struct Role {
    name: &'static str,
    allowed_tools: &'static [&'static str],
    max_iterations: usize,
    /// The tool whose repeated identical output means the stage is going in
    /// circles. `None` for stages that have no such tool.
    stuck_tool: Option<&'static str>,
}

const ANALYST: Role = Role {
    name: "Analyst",
    max_iterations: 25,
    stuck_tool: None,
    allowed_tools: &[
        "get_source_info",
        "get_profile_info",
        "list_states",
        "explore_states",
        "get_xfa",
        "search_xfa",
        "get_plain_state_image",
        "get_annotated_state_image",
        "get_flattened_structure_for_state",
        "list_reference_docs",
        "read_reference_doc",
        "grep_reference_docs",
        "list_reference_forms",
        "search_references",
        "grep_references",
        "get_reference_package",
        "read_reference_file",
    ],
};

const AUTHOR: Role = Role {
    name: "Author",
    max_iterations: 110,
    stuck_tool: Some("validate_aem_package"),
    // No `finish` — the Author never terminates the run; a stage ends on the
    // natural no-tool-use turn. The Reviewer owns termination via `submit_review`.
    allowed_tools: &[
        "get_source_info",
        "list_states",
        "get_schema",
        "get_profile_info",
        "set_aem_translated",
        "get_aem_translated",
        "get_aem_translated_outline",
        "get_aem_translated_node",
        "set_aem_translated_field",
        "insert_aem_translated_node",
        "replace_aem_translated_node",
        "remove_aem_translated_node",
        "search_xfa",
        "get_xfa",
        "get_flattened_structure_for_state",
        "get_plain_state_image",
        "get_annotated_state_image",
        "search_references",
        "grep_references",
        "get_reference_package",
        "read_reference_file",
        "read_reference_doc",
        "grep_reference_docs",
        "build_aem_package",
        "get_package_info",
        "read_package_file",
        "validate_aem_package",
        "generate_xsd",
        "generate_html",
        "upload_to_aem",
        "fetch_aem_form_html",
        "fetch_aem_dor_pdf",
    ],
};

const REVIEWER: Role = Role {
    name: "Reviewer",
    max_iterations: 30,
    stuck_tool: Some("validate_aem_package"),
    allowed_tools: &[
        "build_aem_package",
        "get_package_info",
        "read_package_file",
        "validate_aem_package",
        "review_output",
        "generate_html",
        "get_aem_translated_outline",
        "get_aem_translated_node",
        "get_plain_state_image",
        "get_annotated_state_image",
        "search_xfa",
        "upload_to_aem",
        "fetch_aem_form_html",
        "fetch_aem_dor_pdf",
        "submit_review",
    ],
};

// ── Redacto roles ────────────────────────────────────────────────────────────
//
// A Redacto document is text only, so these stages never touch the AEM tree.
// They also drop `get_profile_info`, which reports the AEM configuration and
// would be misleading here; `get_source_info` is the authority on languages.
// The reference *packages* are AEM content and are pure token cost for a text
// document, so only the reference documentation is offered.

const REDACTO_ANALYST: Role = Role {
    name: "Analyst",
    max_iterations: 25,
    stuck_tool: None,
    allowed_tools: &[
        "get_source_info",
        "list_states",
        "explore_states",
        "get_xfa",
        "search_xfa",
        "get_plain_state_image",
        "get_annotated_state_image",
        "get_flattened_structure_for_state",
        "list_reference_docs",
        "read_reference_doc",
        "grep_reference_docs",
    ],
};

const REDACTO_AUTHOR: Role = Role {
    name: "Author",
    max_iterations: 110,
    stuck_tool: Some("build_redacto_dump"),
    // No `finish` — as with the AEM Author, the Reviewer owns termination.
    allowed_tools: &[
        "get_source_info",
        "list_states",
        "get_schema",
        "get_xfa",
        "search_xfa",
        "get_flattened_structure_for_state",
        "get_plain_state_image",
        "get_annotated_state_image",
        "seed_structured_from_state",
        "set_structured",
        "get_structured_outline",
        "get_structured_node",
        "set_structured_field",
        "replace_structured_node",
        "insert_structured_node",
        "remove_structured_node",
        "build_redacto_dump",
        "review_redacto_output",
        "read_reference_doc",
        "grep_reference_docs",
    ],
};

const REDACTO_REVIEWER: Role = Role {
    name: "Reviewer",
    max_iterations: 30,
    stuck_tool: Some("build_redacto_dump"),
    allowed_tools: &[
        "get_structured_outline",
        "get_structured_node",
        "build_redacto_dump",
        "review_redacto_output",
        "get_plain_state_image",
        "get_annotated_state_image",
        "search_xfa",
        "get_source_info",
        "submit_review",
    ],
};

/// The three stages for one output target.
struct TargetRoles {
    analyst: &'static Role,
    author: &'static Role,
    reviewer: &'static Role,
}

fn roles_for(target: blueprint::OutputTarget) -> TargetRoles {
    match target {
        blueprint::OutputTarget::Aem => TargetRoles {
            analyst: &ANALYST,
            author: &AUTHOR,
            reviewer: &REVIEWER,
        },
        blueprint::OutputTarget::Redacto => TargetRoles {
            analyst: &REDACTO_ANALYST,
            author: &REDACTO_AUTHOR,
            reviewer: &REDACTO_REVIEWER,
        },
    }
}

/// The agent's full tool catalog filtered to the names a role may call. Leaves
/// [`ConversionAgent::tools`]/`execute` untouched (so MCP keeps the flat catalog);
/// a role is simply never *offered* out-of-scope tools.
fn role_tools(agent: &ConversionAgent, allowed: &[&str]) -> Vec<serde_json::Value> {
    agent
        .tools()
        .into_iter()
        .filter(|t| {
            t["name"]
                .as_str()
                .is_some_and(|n| allowed.contains(&n))
        })
        .collect()
}

// ── Per-role system-prompt composition (plan + reviews pinned in `system`) ─────

fn sys_analyst(target: blueprint::OutputTarget, extra: &str) -> String {
    match target {
        blueprint::OutputTarget::Aem => format!("{SHARED_PREAMBLE}{extra}\n\n{ANALYST_ADDENDUM}"),
        blueprint::OutputTarget::Redacto => {
            format!("{REDACTO_SHARED_PREAMBLE}{extra}\n\n{REDACTO_ANALYST_ADDENDUM}")
        }
    }
}

/// The Author reuses the full [`SYSTEM_PROMPT`] authoring body, then the addendum,
/// then the pinned CONVERSION PLAN and every accumulated REVIEW FEEDBACK round.
fn sys_author(
    target: blueprint::OutputTarget,
    extra: &str,
    template_note: &str,
    plan: &str,
    reviews: &[String],
) -> String {
    let mut s = match target {
        blueprint::OutputTarget::Aem => {
            format!("{SYSTEM_PROMPT}{extra}{template_note}\n\n{AUTHOR_ADDENDUM}")
        }
        // No template note: an uploaded content package is an AEM artefact and
        // is not pre-loaded for this target.
        blueprint::OutputTarget::Redacto => {
            format!("{REDACTO_SYSTEM_PROMPT}{extra}\n\n{REDACTO_AUTHOR_ADDENDUM}")
        }
    };
    if !plan.trim().is_empty() {
        s.push_str("\n\n## CONVERSION PLAN\n");
        s.push_str(plan);
    }
    append_reviews(&mut s, "## REVIEW FEEDBACK — address every point across all rounds", reviews);
    s
}

fn sys_reviewer(
    target: blueprint::OutputTarget,
    extra: &str,
    plan: &str,
    reviews: &[String],
) -> String {
    let mut s = match target {
        blueprint::OutputTarget::Aem => format!("{SHARED_PREAMBLE}{extra}\n\n{REVIEWER_ADDENDUM}"),
        blueprint::OutputTarget::Redacto => {
            format!("{REDACTO_SHARED_PREAMBLE}{extra}\n\n{REDACTO_REVIEWER_ADDENDUM}")
        }
    };
    if !plan.trim().is_empty() {
        s.push_str("\n\n## CONVERSION PLAN\n");
        s.push_str(plan);
    }
    append_reviews(&mut s, "## PRIOR REVIEW FEEDBACK (verify each point is now fixed)", reviews);
    s
}

fn append_reviews(s: &mut String, heading: &str, reviews: &[String]) {
    if reviews.is_empty() {
        return;
    }
    s.push_str("\n\n");
    s.push_str(heading);
    s.push('\n');
    for (i, r) in reviews.iter().enumerate() {
        s.push_str(&format!("\n### Round {}\n{}\n", i + 1, r));
    }
}

// ── Public entry points ────────────────────────────────────────────────────────

/// Run the autonomous conversion pipeline end-to-end on a fresh upload.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent(
    files: Vec<(String, Vec<u8>)>,
    profile: Option<String>,
    target: blueprint::OutputTarget,
    settings: crate::settings::AppSettings,
    session_label: String,
    mut processing_state: Signal<ProcessingState>,
    current_session: Signal<Option<String>>,
) {
    let start = std::time::Instant::now();

    // An attached AEM content-package ZIP is pre-loaded as the agent's editable
    // working tree (the ConversionAgent splits PDFs vs. template internally).
    let has_template = files
        .iter()
        .any(|(_, bytes)| blueprint::detect_aem_zip(bytes));
    let pdfs: Vec<(String, Vec<u8>)> = files
        .iter()
        .filter(|(name, _)| name.to_ascii_lowercase().ends_with(".pdf"))
        .cloned()
        .collect();

    // Structured history session (seeded empty); shown in the structured editor.
    // Hash on the PDFs when present, otherwise on the template so the session id
    // is stable for template-only runs.
    let doc_hash = crate::db::document_hash(if pdfs.is_empty() { &files } else { &pdfs });
    crate::db::upsert_document(&doc_hash, &session_label);
    let Some(structured_session) =
        crate::db::create_session(&doc_hash, profile.as_deref(), &session_label)
    else {
        processing_state.set(ProcessingState {
            step: ProcessingStep::AiGenerating,
            ai_mode: true,
            error: Some("Could not create an edit-history session.".into()),
            ..ProcessingState::new()
        });
        return;
    };
    crate::db::insert_edit(&structured_session, "Initial (empty)", "[]");

    let agent = ConversionAgent::new(
        profile.clone(),
        files.clone(),
        settings.aem_connection(),
        structured_session.clone(),
        target,
    );

    // An uploaded content package is an AEM artefact; it is not pre-loaded for
    // any other target, so don't tell the Author it was.
    let template_note = if has_template && target == blueprint::OutputTarget::Aem {
        "\n\nA template AEM tree from an uploaded content package has been pre-loaded as the \
working tree. Inspect it with get_aem_translated_outline and modify it to match the source \
instead of authoring from scratch."
    } else {
        ""
    };

    run_conversion(
        agent,
        settings,
        profile,
        target,
        structured_session,
        start,
        template_note,
        None,
        processing_state,
        current_session,
    )
    .await;
}

/// Resume on an existing session to apply the user's feedback. Skips the Analyst;
/// the feedback becomes the first pinned "review" and the Author applies it, then
/// the Reviewer→fix loop runs as usual.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_feedback(
    feedback: String,
    pdfs: Vec<(String, Vec<u8>)>,
    profile: Option<String>,
    target: blueprint::OutputTarget,
    settings: crate::settings::AppSettings,
    structured_session: String,
    processing_state: Signal<ProcessingState>,
    current_session: Signal<Option<String>>,
) {
    let start = std::time::Instant::now();

    // Seed the agent from the continuing session so feedback applies to the prior
    // result: both the structured content and the AEM tree the last run authored,
    // so the Author refines that tree instead of re-deriving one from the source.
    let prior = crate::session::restore(&structured_session, profile.as_deref());

    let mut agent = ConversionAgent::new(
        profile.clone(),
        pdfs,
        settings.aem_connection(),
        structured_session.clone(),
        target,
    );
    if let Some(prior) = prior {
        agent.seed_structured(prior.envelope.content);
        // A no-op for a Redacto run, which has no AEM tree to seed.
        if let Some(tree) = prior.aem_translated {
            agent.seed_aem_translated(tree);
        }
    }

    run_conversion(
        agent,
        settings,
        profile,
        target,
        structured_session,
        start,
        "",
        Some(feedback),
        processing_state,
        current_session,
    )
    .await;
}

// ── Controller ─────────────────────────────────────────────────────────────────

/// Sequence the specialized stages over one shared [`ConversionAgent`]:
/// Analyst → Author → (Reviewer → Author-fix)* → finalize. The Analyst's plan and
/// each Reviewer report are pinned into the recomposed `system` prompt every stage
/// so they are never evicted.
#[allow(clippy::too_many_arguments)]
async fn run_conversion(
    mut agent: ConversionAgent,
    settings: crate::settings::AppSettings,
    profile: Option<String>,
    target: blueprint::OutputTarget,
    structured_session: String,
    start: std::time::Instant,
    template_note: &str,
    feedback: Option<String>,
    mut processing_state: Signal<ProcessingState>,
    mut current_session: Signal<Option<String>>,
) {
    // Load the selected profile's fonts so on-demand renders have the right
    // typefaces (the font store is global, shared with rendering). Both a fresh
    // run and a feedback re-run funnel through here, so this covers both.
    if let Some(profile_name) = profile.as_deref() {
        let _ = blueprint::load_profile_fonts(profile_name);
    }

    let model = settings.anthropic_model.clone();
    let agent_max_tokens = max_output_tokens_for(&model);
    let extra = crate::settings::extra_instructions_block(&settings.agent_instructions);

    // Surface the context budget once so a mis-detected window is visible.
    let ctx_window = crate::platform::context_window_for(&model);
    let ctx_target = crate::platform::prompt_token_target(&model, agent_max_tokens);
    processing_state.write().context_window = ctx_window;
    push_step(
        &mut processing_state,
        thought(format!(
            "Context window: {ctx_window} tokens · per-turn budget: {ctx_target} tokens · \
             output cap: {agent_max_tokens} · model: {model}"
        )),
    );

    let roles = roles_for(target);
    let mut plan = String::new();
    let mut reviews: Vec<String> = Vec::new();

    if let Some(fb) = feedback {
        // Feedback run: no Analyst; the user's request is the first pinned "review".
        reviews.push(format!("User feedback to apply to the form:\n{fb}"));
    } else {
        // ── Stage 1: Analyst → conversion plan ──────────────────────────────
        push_step(&mut processing_state, stage_header("Analyst", "analysing the source and researching precedents"));
        let Some(outcome) = run_role(
            &mut agent,
            roles.analyst,
            &sys_analyst(target, &extra),
            "Analyse the source form and produce the detailed CONVERSION PLAN. \
             Your final message is the plan.",
            agent_max_tokens,
            &settings,
            &mut processing_state,
        )
        .await
        else {
            return; // fatal API error, already surfaced
        };
        plan = outcome.final_text;
    }

    // ── Stage 2: Author → build the tree ────────────────────────────────────
    push_step(&mut processing_state, stage_header("Author", "building the AEM form"));
    let author_seed = if reviews.is_empty() {
        "Begin building the form per your CONVERSION PLAN. Author the full tree, then \
         build_aem_package and validate_aem_package."
    } else {
        "Apply the REVIEW FEEDBACK in your instructions to the working tree, then \
         build_aem_package and validate_aem_package."
    };
    if run_role(
        &mut agent,
        roles.author,
        &sys_author(target, &extra, template_note, &plan, &reviews),
        author_seed,
        agent_max_tokens,
        &settings,
        &mut processing_state,
    )
    .await
    .is_none()
    {
        return;
    }

    // ── Stage 3: Reviewer → (Author fix)* ───────────────────────────────────
    let mut approved = false;
    let max_review_rounds = settings.max_review_rounds.max(1);
    for round in 0..max_review_rounds {
        push_step(&mut processing_state, stage_header("Reviewer", &format!("reviewing (round {})", round + 1)));
        if run_role(
            &mut agent,
            roles.reviewer,
            &sys_reviewer(target, &extra, &plan, &reviews),
            "Review the built form end to end against the source and the CONVERSION PLAN, \
             then finish by calling submit_review.",
            agent_max_tokens,
            &settings,
            &mut processing_state,
        )
        .await
        .is_none()
        {
            return;
        }

        match agent.take_review() {
            Some(r) if r.approved => {
                approved = true;
                push_step(&mut processing_state, thought("Reviewer approved the form.".into()));
                break;
            }
            Some(r) => {
                push_step(
                    &mut processing_state,
                    thought(format!("Reviewer requested changes (round {}). Returning to the author.", round + 1)),
                );
                reviews.push(r.report);
                push_step(&mut processing_state, stage_header("Author", &format!("applying review feedback (round {})", round + 1)));
                if run_role(
                    &mut agent,
                    roles.author,
                    &sys_author(target, &extra, template_note, &plan, &reviews),
                    "Apply the REVIEW FEEDBACK in your instructions, then build_aem_package and \
                     validate_aem_package.",
                    agent_max_tokens,
                    &settings,
                    &mut processing_state,
                )
                .await
                .is_none()
                {
                    return;
                }
            }
            None => {
                // Reviewer ended without a verdict (budget/stuck). Stop the loop.
                processing_state.write().warnings.push(
                    "The reviewer ended without a verdict — finalizing with what's built.".into(),
                );
                break;
            }
        }
    }

    if !approved {
        processing_state.write().warnings.push(
            "Finalizing without a clean review — some issues may require manual follow-up.".into(),
        );
    }

    // Building and uploading a CRX package is AEM-only; for any other target the
    // dump the Author already validated is the artefact, and calling this would
    // paint a failed build step on an otherwise successful run.
    if target == blueprint::OutputTarget::Aem {
        ensure_built_and_uploaded(&mut agent, &settings, &mut processing_state).await;
    }
    finalize(
        &mut agent,
        &profile,
        target,
        structured_session,
        start,
        &mut processing_state,
        &mut current_session,
    );
}

/// The outcome of running one role stage: the role's final (non-tool) message,
/// used as the handoff brief (the Analyst's plan / a stage summary).
struct RoleOutcome {
    final_text: String,
}

// ── Turn-level failure recovery ────────────────────────────────────────────────

/// Whether an [`crate::platform::anthropic_stream_turn`] error looks transient —
/// i.e. worth re-sending the same turn unchanged. Covers the failure mode you get
/// when the machine is left alone during a long run (the connection is dropped
/// while the response streams, surfacing as `error decoding response body … timed
/// out`) plus the API's own retryable statuses.
fn is_transient_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    const TRANSIENT: &[&str] = &[
        "timed out",
        "timeout",
        "error decoding response body",
        "error reading a body from connection",
        "connection reset",
        "connection closed",
        "connection refused",
        "broken pipe",
        "incomplete message",
        "dns error",
        "os error 50", // network is down
        "os error 51", // network unreachable
        "os error 54", // connection reset by peer
        "os error 64", // host is down
        "os error 65", // no route to host
        "overloaded",
        "rate_limit",
        "rate limit",
        "internal server error",
        "api_error",
    ];
    if TRANSIENT.iter().any(|needle| e.contains(needle)) {
        return true;
    }
    // `Anthropic API error (429 Too Many Requests): …` — retry the statuses the
    // API documents as retryable, but not 4xx client errors we'd just repeat.
    ["(429", "(500", "(502", "(503", "(504", "(529"]
        .iter()
        .any(|status| e.contains(status))
}

/// Pause a stage on a failed turn and wait for the user to press Retry or give
/// up. Keeping the run's future alive is the whole point: the agent, its working
/// tree and this stage's history stay in memory, so Retry re-sends exactly the
/// turn that failed rather than restarting the conversion from scratch.
async fn await_user_retry(
    processing_state: &mut Signal<ProcessingState>,
    role: &str,
    err: &str,
) -> RetryAction {
    {
        let mut s = processing_state.write();
        s.error = Some(format!("Agent failed ({role}): {err}"));
        s.retry_pending = true;
        s.retry_action = None;
    }
    push_step(
        processing_state,
        thought(format!(
            "Paused after a failed request ({err}). Press Retry to resume from this step."
        )),
    );

    let action = loop {
        let pending = processing_state.read().retry_action;
        if let Some(action) = pending {
            break action;
        }
        tokio::time::sleep(std::time::Duration::from_millis(RETRY_POLL_MS)).await;
    };

    {
        let mut s = processing_state.write();
        s.retry_pending = false;
        s.retry_action = None;
        if action == RetryAction::Retry {
            s.error = None;
        }
    }
    if action == RetryAction::Retry {
        push_step(
            processing_state,
            thought("Retrying the failed request…".into()),
        );
    }
    action
}

/// Run one turn, absorbing transient failures: automatic retries with
/// exponential backoff first, then a pause that hands the decision to the user
/// via the Retry button. `None` means the user gave up.
async fn turn_with_retry(
    history: &mut Vec<serde_json::Value>,
    tools: &[serde_json::Value],
    role: &Role,
    system: &str,
    agent_max_tokens: u32,
    settings: &crate::settings::AppSettings,
    processing_state: &mut Signal<ProcessingState>,
) -> Option<crate::platform::TurnOutput> {
    let mut auto_retries = 0usize;
    loop {
        match crate::platform::anthropic_stream_turn(
            history,
            tools,
            &settings.anthropic_api_key,
            &settings.anthropic_model,
            agent_max_tokens,
            Some(system),
        )
        .await
        {
            Ok(turn) => return Some(turn),
            Err(e) => {
                if is_transient_error(&e) && auto_retries < MAX_AUTO_RETRIES {
                    let wait =
                        (RETRY_BACKOFF_SECS << auto_retries.min(4)).min(MAX_RETRY_BACKOFF_SECS);
                    auto_retries += 1;
                    push_step(
                        processing_state,
                        thought(format!(
                            "Request failed ({e}) — retrying in {wait}s \
                             (attempt {auto_retries} of {MAX_AUTO_RETRIES})."
                        )),
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    continue;
                }
                match await_user_retry(processing_state, role.name, &e).await {
                    RetryAction::Retry => {
                        // A user-driven retry resets the automatic budget, so a
                        // long unattended stall can be resumed repeatedly.
                        auto_retries = 0;
                        continue;
                    }
                    RetryAction::Cancel => return None,
                }
            }
        }
    }
}

/// Drive one role stage to completion: fresh bounded history seeded with
/// `seed_user_msg`, the role's filtered tool subset, and its `system` prompt, over
/// the SAME [`crate::platform::anthropic_stream_turn`] path as every other stage
/// (so eviction / caching / budget / retry / off-thread prep / the context-window
/// indicator are all inherited unchanged). Returns the last non-tool assistant
/// message; `None` if a request failed and the user gave up instead of retrying
/// (see [`turn_with_retry`]).
async fn run_role(
    agent: &mut ConversionAgent,
    role: &Role,
    system: &str,
    seed_user_msg: &str,
    agent_max_tokens: u32,
    settings: &crate::settings::AppSettings,
    processing_state: &mut Signal<ProcessingState>,
) -> Option<RoleOutcome> {
    let tools = role_tools(agent, role.allowed_tools);
    let mut history: Vec<serde_json::Value> = vec![serde_json::json!({
        "role": "user",
        "content": [{"type": "text", "text": seed_user_msg}],
    })];
    let mut final_text = String::new();

    // Stuck-validate + max-tokens-nudge guards (identical to the old single loop).
    let mut last_validate_output: Option<String> = None;
    let mut validate_repeat_count: usize = 0;
    let mut consecutive_max_tokens: usize = 0;

    for _ in 0..role.max_iterations {
        // Transient failures are retried automatically, then handed to the user's
        // Retry button — a dropped connection must not throw away a long run.
        let turn = turn_with_retry(
            &mut history,
            &tools,
            role,
            system,
            agent_max_tokens,
            settings,
            processing_state,
        )
        .await?;

        // Per-stage context-window fill indicator.
        if turn.prompt_tokens > 0 {
            processing_state.write().context_used_tokens = turn.prompt_tokens;
        }

        if !turn.text.trim().is_empty() {
            final_text = turn.text.trim().to_string();
            push_step(processing_state, thought(final_text.clone()));
        }

        if turn.stop_reason.as_deref() != Some("tool_use") || turn.tool_calls.is_empty() {
            // A turn cut off at the output-token cap didn't decide to stop — nudge
            // toward incremental authoring and retry rather than ending the stage.
            if turn.stop_reason.as_deref() == Some("max_tokens")
                && consecutive_max_tokens < MAX_MAX_TOKEN_NUDGES
            {
                consecutive_max_tokens += 1;
                let mut content: Vec<serde_json::Value> = turn
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": tc.id,
                            "is_error": true,
                            "content": [{
                                "type": "text",
                                "text": "This call was cut off at the output-token limit and was not executed.",
                            }],
                        })
                    })
                    .collect();
                content.push(serde_json::json!({"type": "text", "text": MAX_TOKENS_NUDGE}));
                history.push(serde_json::json!({"role": "user", "content": content}));
                push_step(
                    processing_state,
                    thought(
                        "Turn hit the output-token limit — asking the agent to author the tree \
                         incrementally instead of in one call."
                            .into(),
                    ),
                );
                continue;
            }
            break; // natural stage completion (no tool use)
        }
        consecutive_max_tokens = 0;

        let mut results: Vec<(String, ToolReply)> = Vec::new();
        let mut stuck = false;
        let mut terminal = false;
        for tc in &turn.tool_calls {
            push_step(
                processing_state,
                AgentStep {
                    id: tc.id.clone(),
                    kind: AgentStepKind::Tool,
                    label: tc.name.clone(),
                    detail: summarize_input(&tc.input),
                    status: AgentStepStatus::Running,
                },
            );
            let reply = agent.execute(&tc.name, &tc.input).await;
            let ok = !matches!(reply, ToolReply::Error(_));
            set_step_status(
                processing_state,
                &tc.id,
                if ok { AgentStepStatus::Done } else { AgentStepStatus::Error },
            );

            // `submit_review`/`finish` end the stage after their result is recorded.
            if tc.name == "submit_review" || tc.name == "finish" {
                terminal = true;
            }

            // Detect a stuck validate loop: same output N times in a row.
            if role.stuck_tool == Some(tc.name.as_str()) {
                let output = match &reply {
                    ToolReply::Text(s) => s.clone(),
                    ToolReply::Error(s) => format!("error:{s}"),
                    ToolReply::Image { .. } => "image".into(),
                };
                if last_validate_output.as_deref() == Some(&output) {
                    validate_repeat_count += 1;
                    if validate_repeat_count >= MAX_VALIDATE_REPEATS {
                        stuck = true;
                    }
                } else {
                    last_validate_output = Some(output);
                    validate_repeat_count = 1;
                }
            } else {
                validate_repeat_count = 0;
                last_validate_output = None;
            }

            results.push((tc.id.clone(), reply));
        }
        history.push(tool_result_message(results));

        if terminal {
            break;
        }
        if stuck {
            processing_state.write().warnings.push(format!(
                "{}: validation produced the same result {} times in a row — moving on.",
                role.name, MAX_VALIDATE_REPEATS
            ));
            break;
        }
    }

    Some(RoleOutcome { final_text })
}

/// Ensure the package reflects the latest tree (rebuild), then upload if an AEM
/// connection is configured and it hasn't been uploaded yet. Mirrors the old
/// stuck-recovery tail; reuses the agent's own tools.
async fn ensure_built_and_uploaded(
    agent: &mut ConversionAgent,
    settings: &crate::settings::AppSettings,
    processing_state: &mut Signal<ProcessingState>,
) {
    push_step(
        processing_state,
        AgentStep {
            id: "finalize-build".into(),
            kind: AgentStepKind::Tool,
            label: "build_aem_package".into(),
            detail: "finalize".into(),
            status: AgentStepStatus::Running,
        },
    );
    let built = !matches!(
        agent.execute("build_aem_package", &serde_json::json!({})).await,
        ToolReply::Error(_)
    );
    set_step_status(
        processing_state,
        "finalize-build",
        if built { AgentStepStatus::Done } else { AgentStepStatus::Error },
    );

    if built && settings.aem_connection().is_some() && !agent.aem_uploaded() {
        push_step(
            processing_state,
            AgentStep {
                id: "finalize-upload".into(),
                kind: AgentStepKind::Tool,
                label: "upload_to_aem".into(),
                detail: "finalize".into(),
                status: AgentStepStatus::Running,
            },
        );
        let ok = !matches!(
            agent.execute("upload_to_aem", &serde_json::json!({})).await,
            ToolReply::Error(_)
        );
        set_step_status(
            processing_state,
            "finalize-upload",
            if ok { AgentStepStatus::Done } else { AgentStepStatus::Error },
        );
    }
}

/// The artefacts a finished run produces, assembled from the agent's working
/// state.
///
/// Split out of [`finalize`] so the target-dependent part is a pure function —
/// the rule it enforces (the artefact that ships is the one the agent worked on)
/// is exactly what needs a test, and a `Signal` write cannot be tested.
struct Outputs {
    envelope: DocumentEnvelope,
    aem_translated: Option<blueprint::AemNodeTranslated>,
    redacto_sql: Option<String>,
    warnings: Vec<String>,
}

fn build_outputs(
    agent: &mut ConversionAgent,
    target: blueprint::OutputTarget,
    profile: &Option<String>,
) -> Outputs {
    let mut warnings: Vec<String> = Vec::new();

    match target {
        blueprint::OutputTarget::Redacto => {
            // The authored structured tree IS the document. Never fall back to
            // `structured_from_aem_tree` here: that conversion drops every
            // non-master language onto a `default` pseudo-language and strips
            // the inline markup, which would turn a loud failure into a dump
            // that imports a multilingual document as one fake locale.
            let content = agent.structured().to_vec();
            // Only the extractor's context carries the master-page header the
            // analysis recovered; `agent.context()` never has it.
            let context = agent.source_envelope().context;

            // Prefer the dump the Author last built and validated, so the SQL
            // that ships is the SQL that was reviewed.
            let redacto_sql = agent
                .redacto_dump()
                .filter(|dump| !dump.assets.is_empty())
                .map(|dump| dump.to_sql())
                .or_else(|| {
                    let envelope = DocumentEnvelope {
                        context: context.clone(),
                        content: content.clone(),
                        state_count: 1,
                    };
                    crate::processing::redacto_sql_for(&envelope, profile.as_deref())
                });

            if redacto_sql.is_none() {
                warnings.push(if content.is_empty() {
                    "No Redacto dump: the agent did not author any content.".to_string()
                } else {
                    "No Redacto dump: the authored document produced no text assets."
                        .to_string()
                });
            }

            Outputs {
                envelope: DocumentEnvelope {
                    context,
                    content,
                    state_count: 1,
                },
                aem_translated: None,
                redacto_sql,
                warnings,
            }
        }
        blueprint::OutputTarget::Aem => {
            // The agent authors the AEM tree directly and leaves its structured
            // tree empty, so lift the authored tree back into structured content
            // — otherwise both editors open on an empty document.
            let aem_translated = agent.aem_translated().cloned();
            let mut content = agent.structured().to_vec();
            if content.is_empty()
                && let Some(tree) = &aem_translated
            {
                content = crate::session::structured_from_aem_tree(tree, profile.as_deref());
            }

            // A Redacto dump is still offered as a byproduct when the profile has
            // a Redacto section, built from the converted source document — the
            // same content the CLI exports. `source_envelope()` rather than
            // `agent.context()` so the recovered master-page header survives.
            let redacto_sql = if profile.as_deref().is_some_and(blueprint::has_redacto_config) {
                let source = agent.source_envelope();
                let sql = crate::processing::redacto_sql_for(&source, profile.as_deref());
                // An empty source is not necessarily an empty document: when the
                // language variants are too dissimilar to merge, the engine
                // yields nothing and every derived output would silently be empty.
                if sql.is_none()
                    && let Some(reason) = agent.source_merge_error()
                {
                    warnings.push(format!(
                        "No Redacto dump: the source language variants could not be \
                         merged ({reason}). Convert with the Redacto output target to \
                         have the agent assemble the languages itself."
                    ));
                }
                sql
            } else {
                None
            };

            Outputs {
                envelope: DocumentEnvelope {
                    context: agent.context().clone(),
                    content,
                    state_count: 1,
                },
                aem_translated,
                redacto_sql,
                warnings,
            }
        }
    }
}

/// Build the final `ProcessingState` from the agent's working trees.
#[allow(clippy::too_many_arguments)]
fn finalize(
    agent: &mut ConversionAgent,
    profile: &Option<String>,
    target: blueprint::OutputTarget,
    structured_session: String,
    start: std::time::Instant,
    processing_state: &mut Signal<ProcessingState>,
    current_session: &mut Signal<Option<String>>,
) {
    let Outputs {
        envelope,
        aem_translated,
        redacto_sql,
        warnings,
    } = build_outputs(agent, target, profile);

    let merged_json = serde_json::to_string_pretty(&envelope).ok();
    let form_code = agent.form_code();

    // Derived exports for the done screen. The form code has to be resolved
    // first: it names the form the schema describes.
    let html_preview = crate::processing::html_preview_for(&envelope, profile.as_deref());
    let xsd_schema =
        crate::processing::xsd_schema_for(&envelope, profile.as_deref(), form_code.as_deref());

    // Record the result in the structured history, so the run can be reopened
    // from the session browser. Without this the session holds nothing but the
    // empty seed and there is nothing to load.
    if let Ok(json) = serde_json::to_string(&envelope) {
        crate::db::insert_edit(&structured_session, "Agent conversion", &json);
    }

    let mut state = processing_state.write();
    state.warnings.extend(warnings);
    state.step = ProcessingStep::Complete;
    state.ai_mode = true;
    state.envelope = Some(envelope);
    state.aem_translated = aem_translated;
    state.merged_json = merged_json;
    state.html_preview = html_preview;
    state.xsd_schema = xsd_schema;
    state.aem_package = agent.package();
    state.redacto_sql = redacto_sql;
    state.form_code = form_code;
    state.agent_aem_session = agent.aem_session();
    state.aem_uploaded = agent.aem_uploaded();
    state.aem_form_path = agent.aem_form_path();
    state.elapsed_secs = Some(start.elapsed().as_secs());
    drop(state);

    current_session.set(Some(structured_session));
}

// ── UI step helpers ──────────────────────────────────────────────────────────

fn thought(label: String) -> AgentStep {
    AgentStep {
        id: String::new(),
        kind: AgentStepKind::Thought,
        label,
        detail: String::new(),
        status: AgentStepStatus::Done,
    }
}

/// A visible boundary marker between pipeline stages.
fn stage_header(role: &str, doing: &str) -> AgentStep {
    thought(format!("── {role} — {doing} ──"))
}

fn push_step(processing_state: &mut Signal<ProcessingState>, step: AgentStep) {
    processing_state.write().agent_steps.push(step);
}

fn set_step_status(
    processing_state: &mut Signal<ProcessingState>,
    id: &str,
    status: AgentStepStatus,
) {
    let mut s = processing_state.write();
    if let Some(step) = s.agent_steps.iter_mut().rev().find(|s| s.id == id) {
        step.status = status;
    }
}

fn summarize_input(input: &serde_json::Value) -> String {
    let s = match input {
        serde_json::Value::Object(m) if m.is_empty() => String::new(),
        _ => input.to_string(),
    };
    if s.chars().count() > 120 {
        format!("{}…", s.chars().take(120).collect::<String>())
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_input_truncates() {
        assert_eq!(summarize_input(&serde_json::json!({})), "");
        let long = serde_json::json!({"q": "x".repeat(500)});
        assert!(summarize_input(&long).chars().count() <= 121);
    }

    #[test]
    fn transient_errors_are_retried_automatically() {
        // The failure seen when the machine is left alone mid-run.
        assert!(is_transient_error(
            "Anthropic API error: error decoding response body — error reading a body \
             from connection — timed out"
        ));
        assert!(is_transient_error(
            "Anthropic API error (529 ): {\"type\":\"overloaded_error\"}"
        ));
        assert!(is_transient_error(
            "Anthropic API error (429 Too Many Requests): rate limit"
        ));
        assert!(is_transient_error(
            "Anthropic API error (503 Service Unavailable): upstream"
        ));
        // Client-side mistakes would just fail again — those go straight to the
        // user's Retry prompt instead of burning automatic attempts.
        assert!(!is_transient_error(
            "Anthropic API error (401 Unauthorized): invalid x-api-key"
        ));
        assert!(!is_transient_error(
            "Anthropic API error (400 Bad Request): prompt is too long"
        ));
        assert!(!is_transient_error(
            "Anthropic API key is not configured. Open Settings and paste your API key."
        ));
    }

    #[test]
    fn role_tool_sets_are_scoped() {
        // Author may write the tree; Analyst and Reviewer may not.
        assert!(AUTHOR.allowed_tools.contains(&"set_aem_translated"));
        assert!(!ANALYST.allowed_tools.contains(&"set_aem_translated"));
        assert!(!REVIEWER.allowed_tools.contains(&"set_aem_translated"));
        // Only the Reviewer can submit a review; nobody but the Reviewer.
        assert!(REVIEWER.allowed_tools.contains(&"submit_review"));
        assert!(!AUTHOR.allowed_tools.contains(&"submit_review"));
        assert!(!ANALYST.allowed_tools.contains(&"submit_review"));
        // The Author must never call `finish` (the controller terminates the run).
        assert!(!AUTHOR.allowed_tools.contains(&"finish"));
        // The stuck detector is keyed off the role, not a hard-coded name.
        assert_eq!(AUTHOR.stuck_tool, Some("validate_aem_package"));
        assert_eq!(ANALYST.stuck_tool, None);
    }

    #[test]
    fn redacto_role_tool_sets_are_scoped() {
        // The Redacto stages author the structured tree...
        assert!(REDACTO_AUTHOR.allowed_tools.contains(&"seed_structured_from_state"));
        assert!(REDACTO_AUTHOR.allowed_tools.contains(&"set_structured_field"));
        assert!(REDACTO_AUTHOR.allowed_tools.contains(&"build_redacto_dump"));
        // ...and must never reach for the AEM machinery.
        for forbidden in [
            "set_aem_translated",
            "get_aem_translated",
            "build_aem_package",
            "validate_aem_package",
            "upload_to_aem",
            // Reports the AEM configuration, which would misinform this stage.
            "get_profile_info",
        ] {
            for role in [&REDACTO_ANALYST, &REDACTO_AUTHOR, &REDACTO_REVIEWER] {
                assert!(
                    !role.allowed_tools.contains(&forbidden),
                    "{} must not be offered {forbidden}",
                    role.name
                );
            }
        }
        // The Analyst never mutates; only the Reviewer terminates.
        assert!(!REDACTO_ANALYST.allowed_tools.contains(&"set_structured"));
        assert!(REDACTO_REVIEWER.allowed_tools.contains(&"submit_review"));
        assert!(!REDACTO_AUTHOR.allowed_tools.contains(&"submit_review"));
        assert!(!REDACTO_AUTHOR.allowed_tools.contains(&"finish"));
        assert_eq!(REDACTO_AUTHOR.stuck_tool, Some("build_redacto_dump"));
    }

    /// `role_tools` filters the catalog by name, so a typo silently removes a
    /// capability with no error anywhere. Catch it here instead.
    #[test]
    fn every_allowed_tool_exists_in_the_catalog() {
        for target in [blueprint::OutputTarget::Aem, blueprint::OutputTarget::Redacto] {
            let agent = ConversionAgent::new(None, Vec::new(), None, String::new(), target);
            let catalog: Vec<String> = agent
                .tools()
                .iter()
                .filter_map(|t| t["name"].as_str().map(String::from))
                .collect();

            let roles = roles_for(target);
            for role in [roles.analyst, roles.author, roles.reviewer] {
                for tool in role.allowed_tools {
                    assert!(
                        catalog.iter().any(|c| c == tool),
                        "{target:?} {} lists '{tool}', which is not in the tool catalog",
                        role.name
                    );
                }
                if let Some(stuck) = role.stuck_tool {
                    assert!(
                        role.allowed_tools.contains(&stuck),
                        "{target:?} {} watches '{stuck}' but is never offered it",
                        role.name
                    );
                }
            }
        }
    }

    fn fixture_agent(target: blueprint::OutputTarget) -> ConversionAgent {
        let pdf = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../core/input/AAEV_019_EN.pdf");
        let bytes = std::fs::read(&pdf).expect("read AAEV_019_EN.pdf");
        ConversionAgent::new(
            Some("ubs".into()),
            vec![("AAEV_019_EN.pdf".to_string(), bytes)],
            None,
            format!("test-outputs-{}", target.as_str()),
            target,
        )
    }

    /// The original defect: the Redacto dump was built from the engine's
    /// conversion of the source while the agent had authored something else, so
    /// what shipped was never what the run produced or the Reviewer approved.
    /// Under the Redacto target the authored tree is the document, full stop.
    #[test]
    fn redacto_outputs_are_built_from_the_authored_tree() {
        use blueprint::structured::{ParagraphNode, StructuredNode, TranslatedText};

        let mut agent = fixture_agent(blueprint::OutputTarget::Redacto);
        // A sentence that appears nowhere in the source PDF, so its presence in
        // the SQL can only come from the authored tree.
        agent.seed_structured(vec![StructuredNode::Paragraph(ParagraphNode {
            content: TranslatedText::plain("AUTHORED-BY-THE-AGENT-MARKER"),
            som_path: None,
            source_name: None,
        })]);

        let outputs = build_outputs(&mut agent, blueprint::OutputTarget::Redacto, &Some("ubs".into()));

        let sql = outputs.redacto_sql.expect("the authored document yields a dump");
        assert!(
            sql.contains("AUTHORED-BY-THE-AGENT-MARKER"),
            "the dump must be generated from the authored tree"
        );
        assert_eq!(
            outputs.envelope.content.len(),
            1,
            "the envelope is the authored tree, not the engine's parse of the source"
        );
        assert!(
            outputs.aem_translated.is_none(),
            "a Redacto run produces no AEM tree"
        );
        // The recovered master-page header must survive into the configuration.
        assert!(
            outputs.envelope.context.header.is_some(),
            "the context must come from the merged source envelope"
        );
    }

    /// An authored tree that produces no assets must yield no file and say why,
    /// rather than a valid-looking dump describing an empty document.
    #[test]
    fn an_empty_redacto_document_produces_no_sql() {
        let mut agent = fixture_agent(blueprint::OutputTarget::Redacto);

        let outputs = build_outputs(&mut agent, blueprint::OutputTarget::Redacto, &Some("ubs".into()));

        assert!(outputs.redacto_sql.is_none());
        assert!(
            outputs.warnings.iter().any(|w| w.contains("No Redacto dump")),
            "the reason must be reported: {:?}",
            outputs.warnings
        );
    }

    /// The AEM target is unchanged: it still offers the dump as a byproduct
    /// derived from the engine's conversion of the source.
    #[test]
    fn the_aem_target_still_derives_its_redacto_byproduct_from_the_source() {
        let mut agent = fixture_agent(blueprint::OutputTarget::Aem);

        let outputs = build_outputs(&mut agent, blueprint::OutputTarget::Aem, &Some("ubs".into()));

        let sql = outputs
            .redacto_sql
            .expect("a single-PDF source converts, so the byproduct exists");
        assert!(sql.contains("INSERT INTO app_redacto.documents "));
        // The envelope, by contrast, is the authored AEM tree lifted back into
        // structured content — empty here because this agent never authored one.
        assert!(outputs.envelope.content.is_empty());
        assert!(outputs.warnings.is_empty(), "{:?}", outputs.warnings);
    }

    /// The two prompt families are deliberate copies, so nothing stops a
    /// copy-paste of the wrong constant. This is what catches it.
    #[test]
    fn redacto_prompts_do_not_leak_aem_vocabulary() {
        let target = blueprint::OutputTarget::Redacto;
        let prompts = [
            sys_analyst(target, ""),
            sys_author(target, "", "", "", &[]),
            sys_reviewer(target, "", "", &[]),
        ];

        for prompt in &prompts {
            for leaked in [
                "AemNodeTranslated",
                "build_aem_package",
                "validate_aem_package",
                "affrg_",
                "fragRef",
                "wizard page",
            ] {
                assert!(
                    !prompt.contains(leaked),
                    "the Redacto prompt must not mention '{leaked}'"
                );
            }
            // …and must name its own vocabulary.
            assert!(prompt.contains("Redacto"), "{prompt}");
        }
        assert!(prompts[1].contains("seed_structured_from_state"));
        assert!(prompts[1].contains("build_redacto_dump"));

        // The AEM prompts must be untouched by the split.
        let aem = sys_author(blueprint::OutputTarget::Aem, "", "", "", &[]);
        assert!(aem.contains("AemNodeTranslated"));
        assert!(!aem.contains("build_redacto_dump"));
    }

    #[test]
    fn sys_author_pins_plan_and_reviews() {
        let s = sys_author(
            blueprint::OutputTarget::Aem,
            "",
            "",
            "PLAN-BODY-MARKER",
            &["FIRST-REVIEW".into(), "SECOND-REVIEW".into()],
        );
        assert!(s.contains("## CONVERSION PLAN"));
        assert!(s.contains("PLAN-BODY-MARKER"));
        assert!(s.contains("## REVIEW FEEDBACK"));
        assert!(s.contains("FIRST-REVIEW"));
        assert!(s.contains("SECOND-REVIEW"));
        assert!(s.contains("Round 1") && s.contains("Round 2"));
        // The authoring body is still present.
        assert!(s.contains("AemNodeTranslated"));
    }

    #[test]
    fn sys_analyst_has_no_plan_section_by_default() {
        let s = sys_analyst(blueprint::OutputTarget::Aem, "");
        // No pinned plan section (the addendum mentions the phrase, but the
        // controller never appends a "## CONVERSION PLAN" block for the Analyst).
        assert!(!s.contains("## CONVERSION PLAN"));
        assert!(s.contains("Analyst"));
    }
}
