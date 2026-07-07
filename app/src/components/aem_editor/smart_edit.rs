//! Smart AEM Edit: AI-assisted editing of the AEM node tree.
//!
//! A direct analogue of the structured Smart Edit
//! ([`crate::components::editor::smart_edit`]): it serialises the current
//! `AemNode` tree to JSON, attaches the plain rendered page images, and makes a
//! single LLM call that proposes a corrected tree plus a structured change
//! list. No AEM HTML is fetched and there is no auto-iteration.

use std::collections::HashMap;

use blueprint::AemNode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai_tools::build_tools;
use crate::platform::{anthropic_agentic_turn, chat_turn};

/// Output-token cap for a Smart AEM Edit turn.
const SMART_AEM_EDIT_MAX_TOKENS: u32 = 16000;

type ChatHistory = Vec<serde_json::Value>;

/// A single proposed change returned alongside the new tree.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeItem {
    pub id: usize,
    pub description: String,
}

/// Result of a Smart AEM Edit call.
#[derive(Clone, Debug)]
pub struct AemSmartEditResult {
    pub root: AemNode,
    pub changes: Vec<ChangeItem>,
}

const AEM_EDIT_GUIDANCE: &str = "\
You are editing the intermediate AEM Adaptive Forms node tree for a form engine.\n\
Tools are available to inspect the SOURCE form, which is the ground truth for content, field \
types, grouping, and order: `list_states` enumerates the states, `get_plain_state_image` \
renders a state, `get_flattened_structure_for_state` returns the engine's structured layout, \
and `get_xfa` returns the authoritative XFA XML — call them as needed.\n\
\n\
Improve the AEM tree so it faithfully represents the source form:\n\
- Fix component/field-type mismatches: use TextField, NumberField, DatePicker, Dropdown, \
RadioButton, or Checkbox according to what the source field actually is. A small fixed \
single-choice set is a RadioButton; long lists are Dropdown; on/off groups are Checkbox; \
calendar dates are DatePicker; amounts are NumberField.\n\
- Group related fields under the correct Panel; keep page Panels (is_page) intact.\n\
- Preserve option lists (label/value) and their order on Dropdown/RadioButton/Checkbox.\n\
- Keep layout columns (colspan, 1-12) sensible; keep mandatory/visible flags consistent with \
the source.\n\
- Keep every node's uuid and name UNCHANGED so identities stay stable; only restructure, \
re-type, regroup, reorder, or fix labels.\n\
\n\
HARD CONSTRAINTS:\n\
- Never invent textual content that is not present in the source images.\n\
- Do not drop existing fields or panels unless they are clearly duplicated.\n\
- Keep the output a single valid AemNode object (the Root) parseable by the schema below.\n";

/// Shared inputs for a Smart AEM Edit run: the tree, the page images, the
/// source PDFs (for tool access) and the LLM credentials/model.
pub struct SmartAemEditCtx<'a> {
    /// Current AEM node tree (the Root).
    pub root: &'a AemNode,
    /// label→per-page base64 images from the plain render stage.
    pub plain_images: &'a HashMap<String, Vec<String>>,
    /// Source PDF bytes, exposed to the model via tools.
    pub source_pdfs: &'a [(String, Vec<u8>)],
    /// API key for the provider.
    pub api_key: &'a str,
    /// Model identifier (e.g. "claude-opus-4-8").
    pub model: &'a str,
    /// Active profile name, if any.
    pub profile: Option<&'a str>,
}

impl SmartAemEditCtx<'_> {
    /// Drive one Smart AEM Edit agentic turn from a fully-built `prompt`,
    /// parsing (with repair) and backfilling the change list. Shared by the
    /// initial and feedback entry points.
    async fn run(&self, prompt: String) -> Result<AemSmartEditResult, String> {
        let json_context = serde_json::to_string_pretty(self.root)
            .map_err(|e| format!("JSON serialisation error: {e}"))?;
        let user_text = build_user_text(&prompt, &json_context);

        let tools = build_tools(self.source_pdfs, self.plain_images, self.profile).await;
        let mut history: ChatHistory = Vec::new();
        let raw = anthropic_agentic_turn(
            &mut history,
            &user_text,
            self.api_key,
            self.model,
            SMART_AEM_EDIT_MAX_TOKENS,
            &tools.tools(),
            |name, input| tools.execute(name, input),
        )
        .await?;
        let mut result = parse_with_repair(&raw, &mut history, self.api_key, self.model).await?;
        ensure_change_list(
            self.root,
            &mut history,
            &mut result,
            self.api_key,
            self.model,
        )
        .await;
        Ok(result)
    }
}

/// Run Smart AEM Edit on the whole tree.
pub async fn run_smart_aem_edit(
    ctx: &SmartAemEditCtx<'_>,
    instructions: &str,
) -> Result<AemSmartEditResult, String> {
    let prompt = build_prompt(ctx.plain_images.len(), instructions);
    ctx.run(prompt).await
}

/// Run a follow-up Smart AEM Edit informed by accepted / rejected changes.
pub async fn run_smart_aem_edit_with_feedback(
    ctx: &SmartAemEditCtx<'_>,
    accepted_changes: &[ChangeItem],
    rejected_changes: &[ChangeItem],
    user_feedback: &str,
    instructions: &str,
) -> Result<AemSmartEditResult, String> {
    let fmt = |changes: &[ChangeItem]| {
        changes
            .iter()
            .map(|c| format!("  - [{}] {}", c.id, c.description))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let feedback_section = if user_feedback.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n\nThe user also gave this additional feedback; follow it:\n{}",
            user_feedback.trim()
        )
    };
    let prompt = format!(
        "{}\n\nThe user reviewed your previous suggestion. They ACCEPTED these changes; keep them:\n{}\n\nThey REJECTED these changes; do NOT apply them again:\n{}{}",
        build_prompt(ctx.plain_images.len(), instructions),
        fmt(accepted_changes),
        fmt(rejected_changes),
        feedback_section,
    );
    ctx.run(prompt).await
}

fn build_prompt(image_count: usize, instructions: &str) -> String {
    let schema = serde_json::to_string_pretty(&blueprint::aem_schema()).unwrap_or_default();
    let extra = crate::settings::extra_instructions_block(instructions);
    format!(
        "{AEM_EDIT_GUIDANCE}\n\
         If reference forms are available for this profile, call `list_reference_forms` and \
         `search_references` / `read_reference_file` / `view_reference_page` to consult a real \
         worked example (input form + final AEM package) before changing an unfamiliar block.\n\
         The \"root\" must conform to this JSON Schema (the schema for AemNode):\n\
         BEGIN JSON SCHEMA\n{schema}\nEND JSON SCHEMA\n\
         \n\
         Output format:\n\
         - Return ONLY one valid JSON object with exactly two keys:\n\
           \"root\": a single AemNode object (the corrected Root)\n\
           \"changes\": a JSON array of objects, each with \"id\" (integer, 0-based) and \"description\" (string)\n\
         - No surrounding prose, no markdown fences, no backticks.\n\
         \n\
         Available page images: {image_count}{extra}"
    )
}

fn build_user_text(prompt: &str, json_context: &str) -> String {
    format!(
        "{prompt}\n\n\
         The current AEM node tree (JSON) is below. Use the provided tools (`list_states`, \
         `get_plain_state_image`, `get_flattened_structure_for_state`, `get_xfa`) to inspect the \
         source form as needed.\n\n\
         BEGIN AEM NODE TREE JSON\n{json_context}\nEND AEM NODE TREE JSON\n\n\
         Return ONLY the JSON object with \"root\" and \"changes\"."
    )
}

async fn parse_with_repair(
    raw: &str,
    history: &mut ChatHistory,
    api_key: &str,
    model: &str,
) -> Result<AemSmartEditResult, String> {
    match parse_response(raw) {
        Ok(v) => Ok(v),
        Err(original_error) => {
            let repair = format!(
                "Your previous response was not parseable. Parse error: {original_error}\n\n\
                 Re-emit the SAME answer as ONE valid JSON object with exactly two keys:\n\
                 - \"root\": a single AemNode object\n\
                 - \"changes\": array of {{\"id\": int, \"description\": string}}\n\
                 No prose, no markdown fences."
            );
            if let Ok(repaired) = chat_turn(
                history,
                &repair,
                &[],
                &[],
                api_key,
                model,
                SMART_AEM_EDIT_MAX_TOKENS,
            )
            .await
                && let Ok(parsed) = parse_response(&repaired)
            {
                return Ok(parsed);
            }
            Err(original_error)
        }
    }
}

async fn ensure_change_list(
    original: &AemNode,
    history: &mut ChatHistory,
    result: &mut AemSmartEditResult,
    api_key: &str,
    model: &str,
) {
    if !result.changes.is_empty() {
        return;
    }
    // If nothing changed, leave the list empty.
    let before = serde_json::to_string(original).unwrap_or_default();
    let after = serde_json::to_string(&result.root).unwrap_or_default();
    if before == after {
        return;
    }
    let prompt = "List the changes you made to the AEM node tree. Return ONLY a JSON array of \
         objects, each with \"id\" (integer, 0-based) and \"description\" (string). No prose, no \
         markdown fences."
        .to_string();
    if let Ok(raw) = chat_turn(
        history,
        &prompt,
        &[],
        &[],
        api_key,
        model,
        SMART_AEM_EDIT_MAX_TOKENS,
    )
    .await
        && let Ok(changes) = parse_change_list(&raw)
        && !changes.is_empty()
    {
        result.changes = changes;
    }
}

/// Parse the `{ "root", "changes" }` response, with fallbacks.
pub fn parse_response(response: &str) -> Result<AemSmartEditResult, String> {
    let trimmed = response.trim();
    let mut candidates: Vec<&str> = vec![trimmed];
    candidates.extend(extract_fenced_blocks(trimmed));

    for candidate in &candidates {
        if let Some(result) = try_parse_object(candidate) {
            return Ok(result);
        }
    }
    // Fallback: a bare AemNode object.
    for candidate in &candidates {
        if let Ok(root) = serde_json::from_str::<AemNode>(candidate) {
            return Ok(AemSmartEditResult {
                root,
                changes: vec![],
            });
        }
    }
    Err(format!(
        "Could not parse AEM node tree from AI response. Raw response:\n{response}"
    ))
}

fn try_parse_object(input: &str) -> Option<AemSmartEditResult> {
    let value: Value = serde_json::from_str(input).ok()?;
    let obj = value.as_object()?;
    let root_val = obj.get("root")?;
    let root: AemNode = serde_json::from_value(root_val.clone()).ok()?;
    let changes: Vec<ChangeItem> = obj
        .get("changes")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Some(AemSmartEditResult { root, changes })
}

fn parse_change_list(response: &str) -> Result<Vec<ChangeItem>, String> {
    let trimmed = response.trim();
    if let Ok(items) = serde_json::from_str::<Vec<ChangeItem>>(trimmed) {
        return Ok(items);
    }
    for block in extract_fenced_blocks(trimmed) {
        if let Ok(items) = serde_json::from_str::<Vec<ChangeItem>>(block) {
            return Ok(items);
        }
    }
    Err("Could not parse change list".to_string())
}

fn extract_fenced_blocks(input: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find("```") {
        let after_start = &rest[start + 3..];
        let body_start = after_start
            .find('\n')
            .map_or(after_start, |nl| &after_start[nl + 1..]);
        if let Some(end) = body_start.find("```") {
            blocks.push(body_start[..end].trim());
            rest = &body_start[end + 3..];
        } else {
            break;
        }
    }
    blocks
}
