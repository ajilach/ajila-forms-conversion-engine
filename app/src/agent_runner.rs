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

/// Output-token cap per agent turn. Sized generously so authoring turns — which
/// emit large `set_aem_translated` / `insert_aem_translated_node` payloads — are
/// far less likely to be truncated mid-tool-call. A turn that still overflows is
/// handled explicitly (see the `max_tokens` branch in [`drive_agent`]) rather
/// than silently dropping the (incomplete) tool call.
const AGENT_MAX_TOKENS: u32 = 32000;
/// Max streamed turns before the loop bails out (the agent makes many calls).
const MAX_ITERATIONS: usize = 200;
/// How many consecutive `validate_aem_package` calls with identical output
/// are allowed before the loop gives up and finalizes with what's built.
const MAX_VALIDATE_REPEATS: usize = 3;
/// How many consecutive output-token-truncated turns are tolerated before the
/// loop stops nudging and finalizes with whatever tree exists. Bounds the cost
/// of a model that keeps re-attempting one oversized call instead of chunking.
const MAX_TRUNCATE_REPEATS: usize = 3;

/// Shown in the activity log when a turn is cut off at the output-token cap.
const TRUNCATED_TURN_NOTICE: &str = "⚠ The model's response hit the per-turn \
output limit and was cut off — asking it to author the tree in smaller steps.";

/// Fed back to the model after a truncated turn to steer it off one-shot
/// authoring (the usual cause of truncation) and toward incremental edits.
const TRUNCATED_TURN_GUIDANCE: &str = "Your previous response was cut off at the \
output-token limit before it finished, so any tool call it contained was \
incomplete and was NOT applied. Do not emit the whole AEM tree in a single \
set_aem_translated call. Author it incrementally instead: first set a skeleton \
with set_aem_translated (the root plus the top-level panels/sections and their \
titles), then add each section's fields with insert_aem_translated_node, and \
refine with the granular editors (set_aem_translated_field / \
replace_aem_translated_node). Keep every individual tool call small enough to \
fit within the output limit.";

/// Run the autonomous agent end-to-end, streaming activity into
/// `processing_state.agent_steps` and finalizing the result on completion.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent(
    pdfs: Vec<(String, Vec<u8>)>,
    profile: Option<String>,
    settings: crate::settings::AppSettings,
    session_label: String,
    mut processing_state: Signal<ProcessingState>,
    current_session: Signal<Option<String>>,
) {
    let start = std::time::Instant::now();

    // Structured history session (seeded empty); shown in the structured editor.
    let doc_hash = crate::db::document_hash(&pdfs);
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
        pdfs.clone(),
        settings.aem_connection(),
        structured_session.clone(),
    );

    let intro = format!(
        "{SYSTEM_PROMPT}{}",
        crate::settings::extra_instructions_block(&settings.agent_instructions)
    );
    let mut history: Vec<serde_json::Value> = Vec::new();
    history.push(serde_json::json!({"role": "user", "content": [{"type": "text", "text": intro}]}));

    drive_agent(
        agent,
        history,
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

    let intro = format!(
        "{SYSTEM_PROMPT}{}\n\n--- REFINEMENT ---\n\
A prior conversion already exists in your working structured tree (inspect it with \
get_structured). The user reviewed the result and gave this feedback:\n\n{feedback}\n\n\
Apply the requested changes to the structured tree, then re-convert to AEM \
(convert_structured_to_aem), rebuild the package (build_aem_package), and \
re-upload (upload_to_aem) if an AEM connection is configured, verifying as needed. \
Then call finish.",
        crate::settings::extra_instructions_block(&settings.agent_instructions)
    );

    let mut history: Vec<serde_json::Value> = Vec::new();
    history.push(serde_json::json!({"role": "user", "content": [{"type": "text", "text": intro}]}));

    drive_agent(
        agent,
        history,
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
    settings: crate::settings::AppSettings,
    profile: Option<String>,
    structured_session: String,
    start: std::time::Instant,
    mut processing_state: Signal<ProcessingState>,
    mut current_session: Signal<Option<String>>,
) {
    let tools = agent.tools();

    // Escape hatch for a stuck validate loop: track how many consecutive turns
    // called validate_aem_package and returned the same output.
    let mut last_validate_output: Option<String> = None;
    let mut validate_repeat_count: usize = 0;
    // Track consecutive turns truncated at the output-token cap (see below).
    let mut truncate_repeat_count: usize = 0;

    for _ in 0..MAX_ITERATIONS {
        let turn =
            match crate::platform::anthropic_stream_turn(
                &mut history,
                &tools,
                &settings.anthropic_api_key,
                &settings.anthropic_model,
                AGENT_MAX_TOKENS,
            )
            .await
            {
                Ok(t) => t,
                Err(e) => {
                    // Record the error but fall through to finalize instead of
                    // bailing out here: the agent may already have authored a
                    // working tree, and finalize/ensure_package will still build
                    // a downloadable package from it. (A truly empty run
                    // finalizes with no package and this error shown.)
                    processing_state.write().error = Some(format!("Agent failed: {e}"));
                    break;
                }
            };

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

        // The turn was cut off at the per-turn output-token cap — almost always
        // the model trying to emit the entire AEM tree in one giant
        // set_aem_translated call. Anthropic truncates mid-tool-input, so the
        // partial JSON does not parse and the tool call is dropped (never
        // executed). Breaking here would finalize with no tree and therefore no
        // downloadable package, so instead steer the model to author the tree
        // incrementally and let the loop continue so it can recover.
        if turn.stop_reason.as_deref() == Some("max_tokens") {
            truncate_repeat_count += 1;
            push_step(
                &mut processing_state,
                AgentStep {
                    id: String::new(),
                    kind: AgentStepKind::Thought,
                    label: TRUNCATED_TURN_NOTICE.into(),
                    detail: String::new(),
                    status: AgentStepStatus::Done,
                },
            );
            if truncate_repeat_count >= MAX_TRUNCATE_REPEATS {
                processing_state.write().warnings.push(
                    "The model repeatedly exceeded the output limit while authoring \
                     the tree — building what's available. The result may be \
                     incomplete and need manual follow-up."
                        .into(),
                );
                break;
            }
            // The assistant message (with any partial tool_use blocks) was
            // already appended to history; the API requires a tool_result for
            // every tool_use before the next turn, so answer them all with the
            // recovery guidance. With no tool calls, nudge via a user message.
            if turn.tool_calls.is_empty() {
                history.push(serde_json::json!({
                    "role": "user",
                    "content": [{"type": "text", "text": TRUNCATED_TURN_GUIDANCE}],
                }));
            } else {
                let results: Vec<(String, ToolReply)> = turn
                    .tool_calls
                    .iter()
                    .map(|tc| (tc.id.clone(), ToolReply::Error(TRUNCATED_TURN_GUIDANCE.into())))
                    .collect();
                history.push(tool_result_message(results));
            }
            continue;
        }
        truncate_repeat_count = 0;

        if turn.stop_reason.as_deref() != Some("tool_use") || turn.tool_calls.is_empty() {
            break;
        }

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
        &mut agent,
        &profile,
        structured_session,
        start,
        &mut processing_state,
        &mut current_session,
    );
}

/// Build the final `ProcessingState` from the agent's working trees.
fn finalize(
    agent: &mut ConversionAgent,
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
    // Guarantee a downloadable package: the agent may have finished right after
    // a tree edit (which invalidates the package) without a final rebuild, so
    // build one from the latest tree. Computed before taking the UI lock; the
    // build itself is panic-isolated inside the agent (see `build_package`).
    // Ok(None) = no tree authored; Err = a tree exists but packaging failed.
    let package_result = agent.ensure_package();

    let mut state = processing_state.write();
    state.step = ProcessingStep::Complete;
    state.ai_mode = true;
    state.envelope = Some(envelope);
    state.merged_json = merged_json;
    match package_result {
        Ok(pkg) => state.aem_package = pkg,
        Err(e) => {
            state.aem_package = None;
            // Surface the real packaging failure rather than a vague "no
            // download". Don't clobber an API error already recorded above.
            if state.error.is_none() {
                state.error = Some(format!("Could not build the AEM package: {e}"));
            }
        }
    }
    state.form_code = form_code;
    state.agent_aem_session = agent.aem_session();
    state.aem_uploaded = agent.aem_uploaded();
    state.aem_form_path = agent.aem_form_path();
    state.elapsed_secs = Some(start.elapsed().as_secs());
    // The run is Complete but there is nothing to download and no error to
    // explain it — never leave the user staring at a result screen with no
    // package and no reason. This means the tree was never authored (e.g. the
    // run was cut short), so say so.
    if state.aem_package.is_none() && state.error.is_none() {
        state.warnings.push(
            "The conversion finished without a downloadable package — the AEM tree \
             was never completed (the run may have been cut short). Re-run the \
             conversion, or use the feedback box to ask the agent to finish the tree."
                .into(),
        );
    }
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
