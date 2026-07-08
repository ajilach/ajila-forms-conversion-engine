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
    ANALYST_ADDENDUM, AUTHOR_ADDENDUM, ConversionAgent, REVIEWER_ADDENDUM, SHARED_PREAMBLE,
    SYSTEM_PROMPT, ToolReply,
};
use blueprint::{DocumentEnvelope, StructuredNode};

use crate::models::{AgentStep, AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep};
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

/// How many consecutive `validate_aem_package` calls with identical output
/// are allowed before a stage gives up (avoids an endless validate loop).
const MAX_VALIDATE_REPEATS: usize = 3;
/// How many consecutive turns that overflow the output-token cap we nudge
/// toward incremental authoring before giving up (avoids an endless loop if the
/// model keeps trying to emit one oversized call regardless).
const MAX_MAX_TOKEN_NUDGES: usize = 3;
/// How many Reviewer→Author fix rounds before finalizing with a warning.
const MAX_REVIEW_ROUNDS: usize = 3;

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
}

const ANALYST: Role = Role {
    name: "Analyst",
    max_iterations: 25,
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

fn sys_analyst(extra: &str) -> String {
    format!("{SHARED_PREAMBLE}{extra}\n\n{ANALYST_ADDENDUM}")
}

/// The Author reuses the full [`SYSTEM_PROMPT`] authoring body, then the addendum,
/// then the pinned CONVERSION PLAN and every accumulated REVIEW FEEDBACK round.
fn sys_author(extra: &str, template_note: &str, plan: &str, reviews: &[String]) -> String {
    let mut s = format!("{SYSTEM_PROMPT}{extra}{template_note}\n\n{AUTHOR_ADDENDUM}");
    if !plan.trim().is_empty() {
        s.push_str("\n\n## CONVERSION PLAN\n");
        s.push_str(plan);
    }
    append_reviews(&mut s, "## REVIEW FEEDBACK — address every point across all rounds", reviews);
    s
}

fn sys_reviewer(extra: &str, plan: &str, reviews: &[String]) -> String {
    let mut s = format!("{SHARED_PREAMBLE}{extra}\n\n{REVIEWER_ADDENDUM}");
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
    );

    let template_note = if has_template {
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
    settings: crate::settings::AppSettings,
    structured_session: String,
    processing_state: Signal<ProcessingState>,
    current_session: Signal<Option<String>>,
) {
    let start = std::time::Instant::now();

    // Seed the agent with the latest structured tree from the continuing session
    // so feedback applies to the prior result. (The prior AEM tree itself is not
    // yet re-hydrated here — the Author re-derives it from the source guided by the
    // pinned feedback; full AEM-tree resume is a follow-up.)
    let prior: Vec<StructuredNode> = crate::db::latest_seq(&structured_session)
        .and_then(|seq| crate::db::snapshot_at(&structured_session, seq))
        .and_then(|json| serde_json::from_str::<Vec<StructuredNode>>(&json).ok())
        .unwrap_or_default();

    let mut agent = ConversionAgent::new(
        profile.clone(),
        pdfs,
        settings.aem_connection(),
        structured_session.clone(),
    );
    agent.seed_structured(prior);

    run_conversion(
        agent,
        settings,
        profile,
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
    structured_session: String,
    start: std::time::Instant,
    template_note: &str,
    feedback: Option<String>,
    mut processing_state: Signal<ProcessingState>,
    mut current_session: Signal<Option<String>>,
) {
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
            &ANALYST,
            &sys_analyst(&extra),
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
        &AUTHOR,
        &sys_author(&extra, template_note, &plan, &reviews),
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
    for round in 0..MAX_REVIEW_ROUNDS {
        push_step(&mut processing_state, stage_header("Reviewer", &format!("reviewing (round {})", round + 1)));
        if run_role(
            &mut agent,
            &REVIEWER,
            &sys_reviewer(&extra, &plan, &reviews),
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
                    &AUTHOR,
                    &sys_author(&extra, template_note, &plan, &reviews),
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

    ensure_built_and_uploaded(&mut agent, &settings, &mut processing_state).await;
    finalize(
        &agent,
        &profile,
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

/// Drive one role stage to completion: fresh bounded history seeded with
/// `seed_user_msg`, the role's filtered tool subset, and its `system` prompt, over
/// the SAME [`crate::platform::anthropic_stream_turn`] path as every other stage
/// (so eviction / caching / budget / retry / off-thread prep / the context-window
/// indicator are all inherited unchanged). Returns the last non-tool assistant
/// message; `None` if a fatal API error was surfaced.
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
        let turn = match crate::platform::anthropic_stream_turn(
            &mut history,
            &tools,
            &settings.anthropic_api_key,
            &settings.anthropic_model,
            agent_max_tokens,
            Some(system),
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                processing_state.write().error =
                    Some(format!("Agent failed ({}): {e}", role.name));
                return None;
            }
        };

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
            if tc.name == "validate_aem_package" {
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

/// Build the final `ProcessingState` from the agent's working trees.
fn finalize(
    agent: &ConversionAgent,
    profile: &Option<String>,
    structured_session: String,
    start: std::time::Instant,
    processing_state: &mut Signal<ProcessingState>,
    current_session: &mut Signal<Option<String>>,
) {
    let envelope = DocumentEnvelope {
        context: agent.context().clone(),
        content: agent.structured().to_vec(),
        state_count: 1,
    };
    let merged_json = serde_json::to_string_pretty(&envelope).ok();
    let form_code = agent.form_code();

    let mut state = processing_state.write();
    state.step = ProcessingStep::Complete;
    state.ai_mode = true;
    state.envelope = Some(envelope);
    state.merged_json = merged_json;
    state.aem_package = agent.package();
    state.form_code = form_code;
    state.agent_aem_session = agent.aem_session();
    state.aem_uploaded = agent.aem_uploaded();
    state.aem_form_path = agent.aem_form_path();
    state.elapsed_secs = Some(start.elapsed().as_secs());
    drop(state);

    let _ = profile;
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
    }

    #[test]
    fn sys_author_pins_plan_and_reviews() {
        let s = sys_author(
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
        let s = sys_analyst("");
        // No pinned plan section (the addendum mentions the phrase, but the
        // controller never appends a "## CONVERSION PLAN" block for the Analyst).
        assert!(!s.contains("## CONVERSION PLAN"));
        assert!(s.contains("Analyst"));
    }
}
