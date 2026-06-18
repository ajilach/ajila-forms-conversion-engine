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

use crate::platform::chat_turn;
use crate::settings::LlmProvider;

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
The attached PNG images are rendered pages of the SOURCE form and are the ground truth for \
content, field types, grouping, and order.\n\
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

/// Run Smart AEM Edit on the whole tree.
pub async fn run_smart_aem_edit(
    root: &AemNode,
    plain_images: &HashMap<String, String>,
    provider: LlmProvider,
    api_key: &str,
    model: &str,
) -> Result<AemSmartEditResult, String> {
    let json_context =
        serde_json::to_string_pretty(root).map_err(|e| format!("JSON serialisation error: {e}"))?;
    let images: Vec<(String, String)> = plain_images
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let prompt = build_prompt(plain_images.len());
    let user_text = build_user_text(&prompt, &json_context);

    let mut history: ChatHistory = Vec::new();
    let raw = chat_turn(
        provider,
        &mut history,
        &user_text,
        &images,
        &[],
        api_key,
        model,
        SMART_AEM_EDIT_MAX_TOKENS,
    )
    .await?;
    let mut result = parse_with_repair(&raw, &mut history, provider, api_key, model).await?;
    ensure_change_list(root, &mut history, &mut result, provider, api_key, model).await;
    Ok(result)
}

/// Run a follow-up Smart AEM Edit informed by accepted / rejected changes.
pub async fn run_smart_aem_edit_with_feedback(
    root: &AemNode,
    plain_images: &HashMap<String, String>,
    accepted_changes: &[ChangeItem],
    rejected_changes: &[ChangeItem],
    user_feedback: &str,
    provider: LlmProvider,
    api_key: &str,
    model: &str,
) -> Result<AemSmartEditResult, String> {
    let json_context =
        serde_json::to_string_pretty(root).map_err(|e| format!("JSON serialisation error: {e}"))?;
    let images: Vec<(String, String)> = plain_images
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

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
        build_prompt(plain_images.len()),
        fmt(accepted_changes),
        fmt(rejected_changes),
        feedback_section,
    );
    let user_text = build_user_text(&prompt, &json_context);

    let mut history: ChatHistory = Vec::new();
    let raw = chat_turn(
        provider,
        &mut history,
        &user_text,
        &images,
        &[],
        api_key,
        model,
        SMART_AEM_EDIT_MAX_TOKENS,
    )
    .await?;
    let mut result = parse_with_repair(&raw, &mut history, provider, api_key, model).await?;
    ensure_change_list(root, &mut history, &mut result, provider, api_key, model).await;
    Ok(result)
}

fn build_prompt(image_count: usize) -> String {
    let schema = serde_json::to_string_pretty(&blueprint::aem_schema()).unwrap_or_default();
    format!(
        "{AEM_EDIT_GUIDANCE}\n\
         The \"root\" must conform to this JSON Schema (the schema for AemNode):\n\
         BEGIN JSON SCHEMA\n{schema}\nEND JSON SCHEMA\n\
         \n\
         Output format:\n\
         - Return ONLY one valid JSON object with exactly two keys:\n\
           \"root\": a single AemNode object (the corrected Root)\n\
           \"changes\": a JSON array of objects, each with \"id\" (integer, 0-based) and \"description\" (string)\n\
         - No surrounding prose, no markdown fences, no backticks.\n\
         \n\
         Attached images: {image_count}"
    )
}

fn build_user_text(prompt: &str, json_context: &str) -> String {
    format!(
        "{prompt}\n\n\
         The current AEM node tree (JSON) is below. The attached PNG images show the rendered \
         source form pages for visual reference.\n\n\
         BEGIN AEM NODE TREE JSON\n{json_context}\nEND AEM NODE TREE JSON\n\n\
         Return ONLY the JSON object with \"root\" and \"changes\"."
    )
}

async fn parse_with_repair(
    raw: &str,
    history: &mut ChatHistory,
    provider: LlmProvider,
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
                provider,
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
    provider: LlmProvider,
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
        provider,
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
