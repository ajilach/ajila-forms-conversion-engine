//! The LLM agent loop that drives the headless [`agent::ConversionAgent`].
//!
//! This is the UI/LLM half of the autonomous conversion agent: it streams turns
//! from the configured model (Anthropic or local), dispatches the model's tool
//! calls to the engine, versions each tree change, streams activity into the
//! Dioxus `ProcessingState`, and finalizes the result on completion. The engine
//! tools themselves live in the framework-agnostic `agent` crate.

use dioxus::prelude::*;

use agent::{ConversionAgent, SYSTEM_PROMPT, ToolReply};
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
/// Max streamed turns before the loop bails out (the agent makes many calls).
const MAX_ITERATIONS: usize = 200;
/// How many consecutive `validate_aem_package` calls with identical output
/// are allowed before the loop gives up and finalizes with what's built.
const MAX_VALIDATE_REPEATS: usize = 3;
/// How many consecutive turns that overflow the output-token cap we nudge
/// toward incremental authoring before giving up (avoids an endless loop if the
/// model keeps trying to emit one oversized call regardless).
const MAX_MAX_TOKEN_NUDGES: usize = 3;

/// Injected when a turn is cut off at [`AGENT_MAX_TOKENS`] — almost always
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

/// Run the autonomous agent end-to-end, streaming activity into
/// `processing_state.agent_steps` and finalizing the result on completion.
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
        "\n\nA template AEM tree from an uploaded content package has been pre-loaded as your \
working tree. Inspect it with get_aem_translated_outline and modify it to match the source \
instead of authoring from scratch."
    } else {
        ""
    };
    // The agent's instructions live in the `system` request field (cached
    // statically across the run); the first user message is only a short trigger.
    let system = format!(
        "{SYSTEM_PROMPT}{}{template_note}",
        crate::settings::extra_instructions_block(&settings.agent_instructions)
    );
    let mut history: Vec<serde_json::Value> = Vec::new();
    history.push(serde_json::json!({
        "role": "user",
        "content": [{"type": "text", "text": "Begin the conversion now, following your instructions."}],
    }));

    drive_agent(
        agent,
        history,
        system,
        settings,
        profile,
        structured_session,
        start,
        processing_state,
        current_session,
    )
    .await;
}

/// Resume the agent on an existing session to refine the result based on the
/// user's feedback. The agent is seeded with the prior structured tree and
/// asked to apply the feedback, then re-convert / package / upload and finish.
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

    // Seed the agent with the latest structured tree from the continuing
    // session so feedback applies to the prior result, not a blank slate.
    let prior: Vec<StructuredNode> = crate::db::latest_seq(&structured_session)
        .and_then(|seq| crate::db::snapshot_at(&structured_session, seq))
        .and_then(|json| serde_json::from_str::<Vec<StructuredNode>>(&json).ok())
        .unwrap_or_default();

    let mut agent = ConversionAgent::new(profile.clone(), pdfs, settings.aem_connection(), structured_session.clone());
    agent.seed_structured(prior);

    // Instructions in the cached `system` field; the user message carries the
    // refinement request (the actual per-run task).
    let system = format!(
        "{SYSTEM_PROMPT}{}",
        crate::settings::extra_instructions_block(&settings.agent_instructions)
    );
    let user_msg = format!(
        "--- REFINEMENT ---\n\
A prior conversion already exists in your working structured tree (inspect it with \
get_structured). The user reviewed the result and gave this feedback:\n\n{feedback}\n\n\
Apply the requested changes to the structured tree, then re-convert to AEM \
(convert_structured_to_aem), rebuild the package (build_aem_package), and \
re-upload (upload_to_aem) if an AEM connection is configured, verifying as needed. \
Then call finish."
    );

    let mut history: Vec<serde_json::Value> = Vec::new();
    history.push(serde_json::json!({"role": "user", "content": [{"type": "text", "text": user_msg}]}));

    drive_agent(
        agent,
        history,
        system,
        settings,
        profile,
        structured_session,
        start,
        processing_state,
        current_session,
    )
    .await;
}

/// Drive the agent loop to completion: stream turns, execute tools, version
/// each step, and finalize the result. Shared by [`run_agent`] and
/// [`run_agent_feedback`].
#[allow(clippy::too_many_arguments)]
async fn drive_agent(
    mut agent: ConversionAgent,
    mut history: Vec<serde_json::Value>,
    system: String,
    settings: crate::settings::AppSettings,
    profile: Option<String>,
    structured_session: String,
    start: std::time::Instant,
    mut processing_state: Signal<ProcessingState>,
    mut current_session: Signal<Option<String>>,
) {
    let tools = agent.tools();
    let agent_max_tokens = max_output_tokens_for(&settings.anthropic_model);

    // Surface the context budget so a mis-detected window (which would evict the
    // transcript every turn) is visible rather than silent.
    let ctx_window = crate::platform::context_window_for(&settings.anthropic_model);
    let ctx_target = crate::platform::prompt_token_target(&settings.anthropic_model, agent_max_tokens);
    processing_state.write().context_window = ctx_window;
    push_step(
        &mut processing_state,
        AgentStep {
            id: String::new(),
            kind: AgentStepKind::Thought,
            label: format!(
                "Context window: {ctx_window} tokens · per-turn budget: {ctx_target} tokens · \
                 output cap: {agent_max_tokens} · model: {}",
                settings.anthropic_model
            ),
            detail: String::new(),
            status: AgentStepStatus::Done,
        },
    );

    // Escape hatch for a stuck validate loop: track how many consecutive turns
    // called validate_aem_package and returned the same output.
    let mut last_validate_output: Option<String> = None;
    let mut validate_repeat_count: usize = 0;
    // How many consecutive turns overflowed the output-token cap; reset on any
    // turn that ends with an executable tool call (i.e. real progress).
    let mut consecutive_max_tokens: usize = 0;

    for _ in 0..MAX_ITERATIONS {
        let turn =
            match crate::platform::anthropic_stream_turn(
                &mut history,
                &tools,
                &settings.anthropic_api_key,
                &settings.anthropic_model,
                agent_max_tokens,
                Some(&system),
            )
            .await
            {
                Ok(t) => t,
                Err(e) => {
                    processing_state.write().error = Some(format!("Agent failed: {e}"));
                    return;
                }
            };

        // Update the context-window fill indicator with this turn's real prompt size.
        if turn.prompt_tokens > 0 {
            processing_state.write().context_used_tokens = turn.prompt_tokens;
        }

        if !turn.text.trim().is_empty() {
            push_step(
                &mut processing_state,
                AgentStep {
                    id: String::new(),
                    kind: AgentStepKind::Thought,
                    label: turn.text.trim().to_string(),
                    detail: String::new(),
                    status: AgentStepStatus::Done,
                },
            );
        }

        if turn.stop_reason.as_deref() != Some("tool_use") || turn.tool_calls.is_empty() {
            // A turn cut off at the output-token cap didn't decide to stop — it
            // ran out of room, almost always mid-way through one oversized tool
            // call. Rather than ending the run (which finalizes an unbuilt or
            // partial tree), nudge the agent to author incrementally and retry.
            if turn.stop_reason.as_deref() == Some("max_tokens")
                && consecutive_max_tokens < MAX_MAX_TOKEN_NUDGES
            {
                consecutive_max_tokens += 1;
                // Any tool_use block in a truncated turn is incomplete and was
                // not executed; answer each with an error result so history
                // stays valid (every tool_use needs a matching tool_result),
                // and fold the incremental-authoring nudge into the same user
                // message.
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
                    &mut processing_state,
                    AgentStep {
                        id: String::new(),
                        kind: AgentStepKind::Thought,
                        label: "Turn hit the output-token limit — asking the agent to author the \
                                tree incrementally instead of in one call."
                            .into(),
                        detail: String::new(),
                        status: AgentStepStatus::Done,
                    },
                );
                continue;
            }
            break;
        }
        // The agent produced an executable tool call — real progress.
        consecutive_max_tokens = 0;

        let mut results: Vec<(String, ToolReply)> = Vec::new();
        let mut stuck = false;
        for tc in &turn.tool_calls {
            push_step(
                &mut processing_state,
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
                &mut processing_state,
                &tc.id,
                if ok {
                    AgentStepStatus::Done
                } else {
                    AgentStepStatus::Error
                },
            );

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
                // Any other tool means the agent is making progress; reset counter.
                validate_repeat_count = 0;
                last_validate_output = None;
            }

            results.push((tc.id.clone(), reply));
        }
        history.push(tool_result_message(results));

        if stuck {
            processing_state.write().warnings.push(
                "Validation produced the same result 3 times in a row — building what's available. \
                 Some issues (e.g. missing fragment paths) may require manual follow-up."
                    .into(),
            );

            // Ensure the package reflects the latest AEM tree, then upload.
            for (id, name, detail) in [
                ("recovery-build", "build_aem_package", "recovery"),
                ("recovery-upload", "upload_to_aem", "recovery"),
            ] {
                push_step(
                    &mut processing_state,
                    AgentStep {
                        id: id.into(),
                        kind: AgentStepKind::Tool,
                        label: name.into(),
                        detail: detail.into(),
                        status: AgentStepStatus::Running,
                    },
                );
                let reply = agent.execute(name, &serde_json::json!({})).await;
                let ok = !matches!(reply, ToolReply::Error(_));
                set_step_status(
                    &mut processing_state,
                    id,
                    if ok { AgentStepStatus::Done } else { AgentStepStatus::Error },
                );
                // Don't attempt upload if build failed.
                if name == "build_aem_package" && !ok {
                    break;
                }
            }

            break;
        }

        if agent.is_finished() {
            break;
        }
    }

    finalize(
        &agent,
        &profile,
        structured_session,
        start,
        &mut processing_state,
        &mut current_session,
    );
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
}
