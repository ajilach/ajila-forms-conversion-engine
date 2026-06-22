//! Smart edit: AI-assisted document editing via the Anthropic API.
//!
//! Serialises the selected structured nodes to JSON and exposes the rendered
//! page images through tool calls, sending the bundle to the configured Claude
//! model as a multi-turn conversation. Each smart-edit session maintains its own
//! [`ChatHistory`] so repair and follow-up calls benefit from full context
//! rather than repeating content. The response is parsed back into
//! structured nodes along with a structured list of proposed changes that
//! the user can accept or reject individually.

use std::collections::HashMap;

use blueprint::StructuredNode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai_tools::build_tools;
use crate::platform::{anthropic_agentic_turn, chat_turn};

/// Output-token cap for Smart Edit turns (edits of an existing selection).
const SMART_EDIT_MAX_TOKENS: u32 = 16000;

/// Domain guidance for the Smart Edit (restructure-existing-selection) prompt.
///
/// Restructuring rules that fix the recurring divergences observed against the
/// reference forms, expressed against the existing `StructuredNode` selection.
/// Must not invent text. See `specs/ai-prompts.md`.
const SMART_EDIT_GUIDANCE: &str = "\
RESTRUCTURING RULES (improve structure WITHOUT inventing, paraphrasing, or deleting text):\n\
\n\
Fix field-type misclassifications:\n\
- Convert Select => Radio when the option set is small (~2-4) and the field gates \
visibility of other nodes (account/person type, addressee, a \"Tipo\" selector). \
Keep the existing options and translations unchanged.\n\
- Convert Date => Text when the field's label names a person/role/agent rather \
than a calendar date (e.g. \"Legitimationspruefung durch\"). Conversely set Date \
when the label clearly denotes a date but the type is Text.\n\
- Promote multi-line free-text from Text to Textarea. Never change a field's \
label text when changing its type.\n\
\n\
Regroup flattened content:\n\
- Collapse a flat run of Paragraphs that share a heading into a Heading + Group.\n\
- Gather scattered address fields (street/number/ZIP/city/country) into one Group \
in postal order.\n\
- Gather signature-related fields into one Group per signer (signature line, \
place, date, name), under the signature Heading. Do not merge distinct signers.\n\
- Move an account/person-type Radio to the top of its section and wrap the \
Individual / Legal-entity bodies in Conditional nodes keyed to it, if those \
bodies already exist as separate nodes.\n\
\n\
Apply dynamic structure that is already implied:\n\
- If duplicated sibling blocks represent \"client 1 / client 2 / ...\" or repeated \
representatives, replace the duplicates with a single Repeatable (de-duplication \
by moving existing content is allowed; do not fabricate a new instance's text).\n\
- Where one node's visibility obviously depends on another field's value and that \
relationship is present in the data, express it as a Conditional.\n\
\n\
Multilingual alignment:\n\
- Keep every translated string's language keys consistent across sibling nodes; \
if a node carries de/en/it, its siblings in the same group must keep the same key \
set and order. Never drop a language present in the input.\n\
\n\
Special cases - auto-inserted standard sections:\n\
- The AEM pipeline auto-prepends the banking-relationship \"preface\" to the first \
page (entity-specific DE/IT) and auto-appends the appendix and the \"Summary of \
form information\" panel. If the input nodes already contain a banking-relationship \
/ summary / appendix block, REMOVE it so it is not duplicated and record this as a \
change. Never add one.\n\
- Do NOT remove the account-holder, signature, or addressee/Tipo type-radio \
sections - those are matched and re-templated downstream and must remain.\n\
\n\
Do NOT:\n\
- Add headings, labels, or option text that is not already in the input.\n\
- Reorder content in a way that breaks the reading order of legal text.\n\
- Split a single legal paragraph mid-sentence.\n";

/// Ordered list of LLM chat messages for a single smart-edit session.
pub type ChatHistory = Vec<serde_json::Value>;

/// A single proposed change returned by Copilot alongside the new nodes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeItem {
    /// Stable identifier used to refer back to this change when sending
    /// feedback to Copilot after a partial rejection.
    pub id: usize,
    /// Human-readable description of what was changed (e.g. "Moved field
    /// 'Name' before 'Address'").
    pub description: String,
}

/// The structured result returned by a smart-edit Copilot call.
#[derive(Clone, Debug)]
pub struct SmartEditResult {
    /// The full replacement node list.
    pub nodes: Vec<StructuredNode>,
    /// Ordered list of proposed changes that produced `nodes`.
    pub changes: Vec<ChangeItem>,
}

/// Extract the JSON representation of the nodes the user selected.
///
/// If `selected_indices` is empty, the whole content slice is used.
pub fn serialize_selected_nodes(
    content: &[StructuredNode],
    selected_indices: &[usize],
) -> Result<String, String> {
    let nodes: Vec<&StructuredNode> = if selected_indices.is_empty() {
        content.iter().collect()
    } else {
        selected_indices
            .iter()
            .filter_map(|&i| content.get(i))
            .collect()
    };
    serde_json::to_string_pretty(&nodes).map_err(|e| format!("JSON serialisation error: {e}"))
}

/// Shared inputs for a smart-edit run: the document context, the page images,
/// the source PDFs (for tool access) and the LLM credentials/model.
pub struct SmartEditCtx<'a> {
    /// Full document content.
    pub content: &'a [StructuredNode],
    /// Root-level indices of selected nodes (empty = all).
    pub selected_indices: &'a [usize],
    /// label→base64-PNG map from the plain render stage.
    pub plain_images: &'a HashMap<String, String>,
    /// Source PDF bytes, exposed to the model via tools.
    pub source_pdfs: &'a [(String, Vec<u8>)],
    /// API key for the provider.
    pub api_key: &'a str,
    /// Model identifier (e.g. "claude-opus-4-8").
    pub model: &'a str,
    /// Active profile name, if any.
    pub profile: Option<&'a str>,
}

impl SmartEditCtx<'_> {
    /// Drive one smart-edit agentic turn from a fully-built `prompt`, parsing
    /// (with repair) and backfilling the change list. Shared by the initial and
    /// feedback entry points.
    async fn run(&self, prompt: String) -> Result<SmartEditResult, String> {
        let json_context = serialize_selected_nodes(self.content, self.selected_indices)?;
        let user_text = build_initial_user_text(&prompt, &json_context);

        let tools = build_tools(self.source_pdfs, self.plain_images, self.profile).await;
        let mut history: ChatHistory = Vec::new();
        let raw = anthropic_agentic_turn(
            &mut history,
            &user_text,
            self.api_key,
            self.model,
            SMART_EDIT_MAX_TOKENS,
            &tools.tools(),
            |name, input| tools.execute(name, input),
        )
        .await?;
        let mut result = parse_with_repair(&raw, &mut history, self.api_key, self.model).await?;
        ensure_change_list(
            self.content,
            self.selected_indices,
            &mut history,
            &mut result,
            self.api_key,
            self.model,
        )
        .await;
        Ok(result)
    }
}

/// Run the smart edit flow end-to-end. Returns a [`SmartEditResult`] containing
/// the suggested nodes and the structured change list.
pub async fn run_smart_edit(
    ctx: &SmartEditCtx<'_>,
    instructions: &str,
) -> Result<SmartEditResult, String> {
    let prompt = build_smart_edit_prompt(ctx.selected_indices, ctx.plain_images, instructions);
    ctx.run(prompt).await
}

/// Run a follow-up smart-edit call, informing the AI which previously proposed
/// changes the user accepted (keep them) and which were rejected (avoid them).
pub async fn run_smart_edit_with_feedback(
    ctx: &SmartEditCtx<'_>,
    accepted_changes: &[ChangeItem],
    rejected_changes: &[ChangeItem],
    user_feedback: &str,
    instructions: &str,
) -> Result<SmartEditResult, String> {
    let prompt = build_feedback_prompt(
        ctx.selected_indices,
        ctx.plain_images,
        accepted_changes,
        rejected_changes,
        user_feedback,
        instructions,
    );
    ctx.run(prompt).await
}

/// Parse `raw` with `parse`; on failure, send one repair turn (echoing the
/// parse error plus `repair_instructions`) and re-parse. Provider/format
/// agnostic — used by both Smart Edit and AI generation.
async fn parse_with_repair_generic<T>(
    raw: &str,
    history: &mut ChatHistory,
    api_key: &str,
    model: &str,
    max_tokens: u32,
    repair_instructions: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<T, String> {
    match parse(raw) {
        Ok(value) => Ok(value),
        Err(original_error) => {
            // The bad response is already in history; include the parse error
            // so the model knows what to fix.
            let repair_prompt = format!(
                "Your previous response was not parseable by the consumer. \
                 Parse error: {original_error}\n\n{repair_instructions}"
            );

            if let Ok(repaired_raw) = chat_turn(
                history,
                &repair_prompt,
                &[],
                &[],
                api_key,
                model,
                max_tokens,
            )
            .await
                && let Ok(parsed) = parse(&repaired_raw)
            {
                return Ok(parsed);
            }

            Err(original_error)
        }
    }
}

async fn parse_with_repair(
    raw: &str,
    history: &mut ChatHistory,
    api_key: &str,
    model: &str,
) -> Result<SmartEditResult, String> {
    parse_with_repair_generic(
        raw,
        history,
        api_key,
        model,
        SMART_EDIT_MAX_TOKENS,
        "Re-emit the SAME answer in the exact required format.\n\
         Return ONLY one valid JSON object with exactly two keys:\n\
         - \"nodes\": array of StructuredNode JSON\n\
         - \"changes\": array of {\"id\": int, \"description\": string}\n\
         Do not add explanations, markdown, or code fences.",
        parse_smart_edit_response,
    )
    .await
}

async fn ensure_change_list(
    content: &[StructuredNode],
    selected_indices: &[usize],
    history: &mut ChatHistory,
    result: &mut SmartEditResult,
    api_key: &str,
    model: &str,
) {
    if !result.changes.is_empty() {
        return;
    }

    // Check whether anything actually changed.
    let original: Vec<&StructuredNode> = if selected_indices.is_empty() {
        content.iter().collect()
    } else {
        selected_indices
            .iter()
            .filter_map(|&i| content.get(i))
            .collect()
    };
    let original_owned: Vec<StructuredNode> = original.into_iter().cloned().collect();
    if compute_changed_indices(&original_owned, &result.nodes).is_empty() {
        return; // No structural changes – nothing to list.
    }

    // Build a follow-up prompt asking only for the change list.
    let original_json = serde_json::to_string_pretty(&original_owned).unwrap_or_default();
    let suggested_json = serde_json::to_string_pretty(&result.nodes).unwrap_or_default();
    let followup_prompt = format!(
        "You previously edited structured form nodes. I need a structured list of the changes you made.\n\n\
         ORIGINAL NODES:\n{original_json}\n\n\
         YOUR SUGGESTED NODES:\n{suggested_json}\n\n\
         Return ONLY a valid JSON array of change objects. Each object has:\n\
         - \"id\": integer (0-based sequential)\n\
         - \"description\": a concise human-readable description of the change\n\n\
         No surrounding prose, no markdown fences, no backticks."
    );

    if let Ok(raw) = chat_turn(
        history,
        &followup_prompt,
        &[],
        &[],
        api_key,
        model,
        SMART_EDIT_MAX_TOKENS,
    )
    .await
        && let Ok(changes) = parse_change_list(&raw)
        && !changes.is_empty()
    {
        result.changes = changes;
    }
}

/// Try to parse a JSON array of ChangeItem from a raw response.
fn parse_change_list(response: &str) -> Result<Vec<ChangeItem>, String> {
    let trimmed = response.trim();

    // Direct parse
    if let Ok(items) = serde_json::from_str::<Vec<ChangeItem>>(trimmed) {
        return Ok(items);
    }

    // Fenced blocks
    for block in extract_fenced_blocks(trimmed) {
        if let Ok(items) = serde_json::from_str::<Vec<ChangeItem>>(block) {
            return Ok(items);
        }
    }

    // Balanced JSON arrays
    for candidate in extract_json_array_candidates(trimmed) {
        if let Ok(items) = serde_json::from_str::<Vec<ChangeItem>>(candidate) {
            return Ok(items);
        }
    }

    Err("Could not parse change list".to_string())
}

/// Build the full user text for the initial smart-edit call by combining the
/// system prompt with the serialised JSON context.
fn build_initial_user_text(prompt: &str, json_context: &str) -> String {
    format!(
        "{prompt}\n\n\
         The structured JSON representation of the selected form nodes is included below. \
         Use the provided tools to inspect the source form as needed: `list_states` enumerates \
         the form states, `get_plain_state_image` renders a state, \
         `get_flattened_structure_for_state` returns the engine's structured layout for a state, \
         and `get_xfa` returns the authoritative XFA XML.\n\n\
         BEGIN STRUCTURED NODES JSON\n\
         {json_context}\n\
         END STRUCTURED NODES JSON\n\n\
         Return ONLY a valid JSON object with exactly two keys: \
         \"nodes\" (the replacement Vec<StructuredNode> array) and \
         \"changes\" (an array of objects, each with \"id\" (integer) and \"description\" (string), \
         describing each logical change you made). \
         No surrounding prose, no markdown fences, no trailing notes."
    )
}

fn build_smart_edit_prompt(
    selected_indices: &[usize],
    plain_images: &HashMap<String, String>,
    instructions: &str,
) -> String {
    let selection_scope = if selected_indices.is_empty() {
        "all root-level nodes".to_string()
    } else {
        format!(
            "root-level node indices: {}",
            selected_indices
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    let schema = serde_json::to_string_pretty(&blueprint::structured_schema()).unwrap_or_default();

    format!(
        "You are editing structured form nodes for a multilingual form engine.\n\
         Scope: {selection_scope}.\n\
         Tools are available to inspect the source form: `list_states`, `get_plain_state_image`, \
         `get_flattened_structure_for_state`, and `get_xfa` (the authoritative XFA XML).\n\
         \n\
         The \"nodes\" array must conform to the following JSON Schema (the schema for \
         Vec<StructuredNode>):\n\
         BEGIN JSON SCHEMA\n\
         {schema}\n\
         END JSON SCHEMA\n\
         \n\
         Primary goal:\n\
         - Improve structural layout and ordering so the form is logically organized and easy to read.\n\
         - Keep the output semantically faithful to the input.\n\
         \n\
         Hard constraints (must follow):\n\
         - Never invent, add, or hallucinate new textual content in any language.\n\
         - You may move, regroup, split, or merge existing text/nodes when needed for better structure.\n\
         - Preserve all source text meaning; do not paraphrase unless text is already duplicated and can be de-duplicated by moving existing content.\n\
         - Keep multilingual content aligned: if multiple languages exist in a node or sibling nodes, maintain consistent language pairing/order so translations remain correctly matched.\n\
         - Keep field identities stable whenever possible (names/som_path) and preserve valid schema shape for StructuredNode JSON.\n\
         - Do not emit markdown, explanations, or code fences.\n\
         \n\
         {SMART_EDIT_GUIDANCE}\n\
         If reference forms are available, call `list_reference_forms` and then \
         `search_references` (semantic over descriptions plus literal in descriptions and AEM XML) \
         / `read_reference_file` / `view_reference_page` to consult a real worked example before \
         converting an unfamiliar block.\n\
         Output format:\n\
         - Return ONLY one valid JSON object with exactly two keys:\n\
           \"nodes\": a JSON array of the replacement StructuredNode objects\n\
           \"changes\": a JSON array of change objects, each with \"id\" (integer, 0-based) and \"description\" (string)\n\
         - The \"nodes\" array must be directly parseable as Vec<StructuredNode>.\n\
         - Each \"changes\" entry describes one logical change you made (e.g. moved, merged, split, reordered).\n\
         - No surrounding prose, no trailing notes, no backticks.\n\
         \n\
         Available page images: {}{}",
        plain_images.len(),
        crate::settings::extra_instructions_block(instructions),
    )
}

fn build_feedback_prompt(
    selected_indices: &[usize],
    plain_images: &HashMap<String, String>,
    accepted_changes: &[ChangeItem],
    rejected_changes: &[ChangeItem],
    user_feedback: &str,
    instructions: &str,
) -> String {
    let base = build_smart_edit_prompt(selected_indices, plain_images, instructions);
    let format_list = |changes: &[ChangeItem]| {
        changes
            .iter()
            .map(|c| format!("  - [{}] {}", c.id, c.description))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let accepted_list = format_list(accepted_changes);
    let rejected_list = format_list(rejected_changes);

    let feedback_section = if user_feedback.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n\nThe user also gave the following additional feedback; follow it in your new \
             suggestion:\n{}",
            user_feedback.trim()
        )
    };

    format!(
        "{base}\n\n\
         IMPORTANT – The user reviewed your previous suggestion. They ACCEPTED the following \
         changes; keep them in your new suggestion:\n\
         {accepted_list}\n\
         \n\
         They REJECTED the following changes. Do NOT apply these again in your new suggestion:\n\
         {rejected_list}\n\
         Please produce a revised suggestion that keeps the accepted changes and still improves \
         the structure, but avoids the rejected changes.{feedback_section}"
    )
}

/// Parse the full structured response from Copilot.
///
/// Expects the response to be a JSON object with `"nodes"` and `"changes"` keys.
/// Falls back to treating the response as a plain node array if the new format is
/// not found, in which case `changes` will be empty.
pub fn parse_smart_edit_response(response: &str) -> Result<SmartEditResult, String> {
    let trimmed = response.trim();

    // Try every candidate text block (fenced or raw JSON objects/arrays).
    let mut candidates: Vec<&str> = vec![trimmed];
    candidates.extend(extract_fenced_blocks(trimmed));

    for candidate in &candidates {
        if let Some(result) = try_parse_result_object(candidate) {
            return Ok(result);
        }
    }

    // Fall back: parse as a raw node array (no change list).
    for candidate in &candidates {
        if let Ok(nodes) = serde_json::from_str::<Vec<StructuredNode>>(candidate) {
            return Ok(SmartEditResult {
                nodes,
                changes: vec![],
            });
        }
    }

    for candidate in extract_json_array_candidates(trimmed) {
        if let Ok(nodes) = serde_json::from_str::<Vec<StructuredNode>>(candidate) {
            return Ok(SmartEditResult {
                nodes,
                changes: vec![],
            });
        }
    }

    Err(format!(
        "Could not parse structured nodes from AI response. Raw response:\n{response}"
    ))
}

fn try_parse_result_object(input: &str) -> Option<SmartEditResult> {
    let value: Value = serde_json::from_str(input).ok()?;
    let obj = value.as_object()?;
    let nodes_val = obj.get("nodes")?;
    let nodes: Vec<StructuredNode> = serde_json::from_value(nodes_val.clone()).ok()?;
    let changes: Vec<ChangeItem> = obj
        .get("changes")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Some(SmartEditResult { nodes, changes })
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

fn extract_json_array_candidates(input: &str) -> Vec<&str> {
    let bytes = input.as_bytes();
    let mut candidates = Vec::new();

    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut start_idx: Option<usize> = None;

    for (i, b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if *b == b'\\' {
                escaped = true;
                continue;
            }
            if *b == b'"' {
                in_string = false;
            }
            continue;
        }

        match *b {
            b'"' => in_string = true,
            b'[' => {
                if depth == 0 {
                    start_idx = Some(i);
                }
                depth += 1;
            }
            b']' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = start_idx {
                        candidates.push(&input[start..=i]);
                    }
                    start_idx = None;
                }
            }
            _ => {}
        }
    }

    candidates
}

/// Compute root-level indices of nodes that differ between the original
/// selected nodes and the AI-suggested replacement nodes.
///
/// Comparison is done via JSON serialisation since `StructuredNode` does
/// not implement `PartialEq`.
pub fn compute_changed_indices(
    original: &[StructuredNode],
    suggested: &[StructuredNode],
) -> Vec<usize> {
    let max_len = original.len().max(suggested.len());
    let mut changed = Vec::new();
    for i in 0..max_len {
        let orig_json = original.get(i).and_then(|n| serde_json::to_string(n).ok());
        let sugg_json = suggested.get(i).and_then(|n| serde_json::to_string(n).ok());
        if orig_json != sugg_json {
            changed.push(i);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueprint::{ParagraphNode, TranslatedText};

    fn make_paragraph(text: &str) -> StructuredNode {
        StructuredNode::Paragraph(ParagraphNode {
            content: TranslatedText::plain(text),
            som_path: None,
            source_name: None,
        })
    }

    #[test]
    fn parse_smart_edit_response_handles_new_format() {
        let node = make_paragraph("Hello");
        let nodes_json = serde_json::to_string(&vec![&node]).unwrap();
        let response = format!(
            r#"{{"nodes":{nodes_json},"changes":[{{"id":0,"description":"Improved order"}}]}}"#
        );
        let result = parse_smart_edit_response(&response).expect("should parse new format");
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].id, 0);
        assert_eq!(result.changes[0].description, "Improved order");
    }

    #[test]
    fn parse_smart_edit_response_falls_back_to_array() {
        let node = make_paragraph("Hello");
        let payload = serde_json::to_string(&vec![node]).expect("serialize");
        let response = format!("Here is the result:\n```json\n{payload}\n```");
        let result = parse_smart_edit_response(&response).expect("should parse fenced array");
        assert_eq!(result.nodes.len(), 1);
        assert!(result.changes.is_empty());
        match &result.nodes[0] {
            StructuredNode::Paragraph(p) => assert_eq!(p.content.as_plain_text(), "Hello"),
            _ => panic!("expected paragraph"),
        }
    }
}
