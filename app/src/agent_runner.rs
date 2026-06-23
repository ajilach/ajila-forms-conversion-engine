//! The LLM agent loop that drives the headless [`agent::ConversionAgent`].
//!
//! This is the UI/LLM half of the autonomous conversion agent: it streams turns
//! from the configured model (Anthropic or local), dispatches the model's tool
//! calls to the engine, versions each tree change, streams activity into the
//! Dioxus `ProcessingState`, and finalizes the result on completion. The engine
//! tools themselves live in the framework-agnostic `agent` crate.

use dioxus::prelude::*;

use agent::{ConversionAgent, SYSTEM_PROMPT, ToolReply};
use blueprint::DocumentEnvelope;

use crate::models::{AgentStep, AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep};
use crate::platform::tool_result_message;

/// Output-token cap per agent turn.
const AGENT_MAX_TOKENS: u32 = 16000;
/// Max streamed turns before the loop bails out (the agent makes many calls).
const MAX_ITERATIONS: usize = 200;

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

/// Drive the agent loop to completion: stream turns, execute tools, version
/// each step, and finalize the result. Shared by the agent entry points.
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
                    processing_state.write().error = Some(format!("Agent failed: {e}"));
                    return;
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

        if turn.stop_reason.as_deref() != Some("tool_use") || turn.tool_calls.is_empty() {
            break;
        }

        let mut results: Vec<(String, ToolReply)> = Vec::new();
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
            results.push((tc.id.clone(), reply));
        }
        history.push(tool_result_message(results));

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
