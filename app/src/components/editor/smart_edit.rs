//! Smart edit: AI-assisted document editing via `gh copilot` CLI.
//!
//! Serialises the selected structured nodes to JSON, attaches rendered
//! page images, sends the bundle to GitHub Copilot, and parses the
//! response back into structured nodes along with a structured list of
//! proposed changes that the user can accept or reject individually.

use std::collections::HashMap;

use blueprint::StructuredNode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::platform::run_copilot_smart_edit;

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

/// Run the smart edit flow end-to-end.
///
/// * `content` – full document content.
/// * `selected_indices` – root-level indices of selected nodes (empty = all).
/// * `plain_images` – label→base64-PNG map from the plain render stage.
///
/// Returns a [`SmartEditResult`] containing the suggested nodes and the
/// structured change list.
pub async fn run_smart_edit(
    content: &[StructuredNode],
    selected_indices: &[usize],
    plain_images: &HashMap<String, String>,
    session_name: &str,
    resume_session: bool,
) -> Result<SmartEditResult, String> {
    let json_context = serialize_selected_nodes(content, selected_indices)?;
    let images: Vec<(String, String)> = plain_images
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let prompt = build_smart_edit_prompt(selected_indices, plain_images);
    let raw = run_copilot_smart_edit(
        &prompt,
        &json_context,
        &images,
        Some(session_name),
        resume_session,
    )
    .await?;
    let mut result = parse_with_same_session_repair(&raw, &images, session_name).await?;
    ensure_change_list(
        content,
        selected_indices,
        &images,
        &mut result,
        session_name,
    )
    .await;
    Ok(result)
}

/// Run a follow-up smart-edit call, informing Copilot which previously
/// proposed changes were rejected so it should avoid repeating them.
pub async fn run_smart_edit_with_feedback(
    content: &[StructuredNode],
    selected_indices: &[usize],
    plain_images: &HashMap<String, String>,
    rejected_changes: &[ChangeItem],
    session_name: &str,
) -> Result<SmartEditResult, String> {
    let json_context = serialize_selected_nodes(content, selected_indices)?;
    let images: Vec<(String, String)> = plain_images
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let prompt = build_feedback_prompt(selected_indices, plain_images, rejected_changes);
    let raw =
        run_copilot_smart_edit(&prompt, &json_context, &images, Some(session_name), true).await?;
    let mut result = parse_with_same_session_repair(&raw, &images, session_name).await?;
    ensure_change_list(
        content,
        selected_indices,
        &images,
        &mut result,
        session_name,
    )
    .await;
    Ok(result)
}

async fn parse_with_same_session_repair(
    raw: &str,
    images: &[(String, String)],
    session_name: &str,
) -> Result<SmartEditResult, String> {
    match parse_smart_edit_response(raw) {
        Ok(result) => Ok(result),
        Err(original_error) => {
            let repair_prompt = format!(
                "Your previous response was not parseable by the consumer. Re-emit the SAME answer in the exact required format.\n\
                 Return ONLY one valid JSON object with exactly two keys:\n\
                 - \"nodes\": array of StructuredNode JSON\n\
                 - \"changes\": array of {{\"id\": int, \"description\": string}}\n\
                 Do not add explanations, markdown, or code fences.\n\
                 PREVIOUS RESPONSE:\n{raw}"
            );

            if let Ok(repaired_raw) =
                run_copilot_smart_edit(&repair_prompt, "", images, Some(session_name), true).await
                && let Ok(parsed) = parse_smart_edit_response(&repaired_raw)
            {
                return Ok(parsed);
            }

            Err(original_error)
        }
    }
}

/// If the AI returned nodes but no change list, and the nodes actually
/// differ from the originals, ask Copilot for the change list in a
/// follow-up call.
async fn ensure_change_list(
    content: &[StructuredNode],
    selected_indices: &[usize],
    images: &[(String, String)],
    result: &mut SmartEditResult,
    session_name: &str,
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

    if let Ok(raw) =
        run_copilot_smart_edit(&followup_prompt, "", images, Some(session_name), true).await
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

fn build_smart_edit_prompt(
    selected_indices: &[usize],
    plain_images: &HashMap<String, String>,
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

    format!(
        "You are editing structured form nodes for a multilingual form engine.\n\
         Scope: {selection_scope}.\n\
         Visual references are attached as PNG page renderings.\n\
         \n\
         StructuredNode schema (tagged enum, JSON key is the variant name):\n\
         - Heading: {{ level: \"H1\"..\"H6\", content: InlineText }}\n\
         - Paragraph: {{ content: InlineText }}\n\
         - Field: {{ name: UUID, label: InlineText|null, input_type: FieldType, value: InputValue|null, placeholder: TranslatableString|null, required: bool }}\n\
         - Table: {{ header: {{ cells: [StructuredNode] }}|null, rows: [{{ cells: [StructuredNode] }}], caption: InlineText|null }}\n\
         - List: {{ list_style: \"Disc\"|\"Decimal\"|\"LowerAlpha\"|\"UpperAlpha\"|\"LowerRoman\"|\"UpperRoman\"|\"None\", items: [{{ content: InlineText, sublist: ListNode|null }}] }}\n\
         - Group: {{ children: [StructuredNode] }}\n\
         - Repeatable: {{ item: StructuredNode, min_occurrences: int, max_occurrences: int|null }}\n\
         - Conditional: {{ condition: {{ field_name: UUID, value: InputValue }}, content: StructuredNode }}\n\
         - GridLayout: {{ columns: int, elements: [{{ span: int, node: StructuredNode }}] }}\n\
         - Image: {{ data: base64, mime_type: string, alt: string|null }}\n\
         - Footnote: {{ content: InlineText, marker: string|null }}\n\
         - Empty: (unit)\n\
         \n\
         InlineText is an array of InlineNode:\n\
         - {{ Text: \"...\" }} – plain text\n\
         - {{ TranslatedText: {{ \"en\": \"...\", \"de\": \"...\" }} }} – multilingual text\n\
         - {{ Strong: InlineNode }} – bold\n\
         - {{ Emphasis: InlineNode }} – italic\n\
         - {{ Superscript: InlineNode }}\n\
         - {{ Link: {{ href: \"...\", content: InlineText }} }}\n\
         \n\
         FieldType variants: Text {{ regex, max_length, min_length }}, Number {{ min, max, step }}, Date, Email, Tel, Bool, Radio {{ options }}, Select {{ options }}\n\
         InputValue variants: {{ Text: \"...\" }}, {{ Number: \"...\" }}, {{ Bool: true/false }}\n\
         TranslatableString: {{ Plain: \"...\" }} or {{ Translated: {{ \"en\": \"...\", ... }} }}\n\
         \n\
         Primary goal:\n\
         - Improve structural layout and ordering so the form is logically organized and easy to read.\n\
         - Keep the output semantically faithful to the input.\n\
         - Set `required: true` on fields that contextually appear mandatory (e.g. fields marked with asterisks, labels containing \"required\"/\"mandatory\"/\"Pflichtfeld\", or fields that are clearly essential like name, signature, date fields in official forms). Default to `required: false` when uncertain.\n\
         \n\
         Hard constraints (must follow):\n\
         - Never invent, add, or hallucinate new textual content in any language.\n\
         - You may move, regroup, split, or merge existing text/nodes when needed for better structure.\n\
         - Preserve all source text meaning; do not paraphrase unless text is already duplicated and can be de-duplicated by moving existing content.\n\
         - Keep multilingual content aligned: if multiple languages exist in a node or sibling nodes, maintain consistent language pairing/order so translations remain correctly matched.\n\
         - Keep field identities stable whenever possible (names/som_path) and preserve valid schema shape for StructuredNode JSON.\n\
         - Do not emit markdown, explanations, or code fences.\n\
         \n\
         Output format:\n\
         - Return ONLY one valid JSON object with exactly two keys:\n\
           \"nodes\": a JSON array of the replacement StructuredNode objects\n\
           \"changes\": a JSON array of change objects, each with \"id\" (integer, 0-based) and \"description\" (string)\n\
         - The \"nodes\" array must be directly parseable as Vec<StructuredNode>.\n\
         - Each \"changes\" entry describes one logical change you made (e.g. moved, merged, split, reordered).\n\
         - No surrounding prose, no trailing notes, no backticks.\n\
         \n\
         Attached images: {}",
        plain_images.len()
    )
}

fn build_feedback_prompt(
    selected_indices: &[usize],
    plain_images: &HashMap<String, String>,
    rejected_changes: &[ChangeItem],
) -> String {
    let base = build_smart_edit_prompt(selected_indices, plain_images);
    let rejected_list = rejected_changes
        .iter()
        .map(|c| format!("  - [{}] {}", c.id, c.description))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{base}\n\n\
         IMPORTANT – The user reviewed your previous suggestion and rejected the following \
         changes. Do NOT apply these again in your new suggestion:\n\
         {rejected_list}\n\
         Please produce a revised suggestion that still improves the structure but avoids \
         the rejected changes."
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

/// Try to extract a JSON array of StructuredNode from the AI response.
///
/// The response might contain markdown fences or surrounding prose, so we
/// try to find the outermost `[…]` and parse that.
///
/// This is kept as a compatibility helper for the modal (which has its own
/// simpler flow).
#[allow(dead_code)]
pub fn parse_response_nodes(response: &str) -> Result<Vec<StructuredNode>, String> {
    parse_smart_edit_response(response).map(|r| r.nodes)
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
    use blueprint::{InlineText, ParagraphNode};

    fn make_paragraph(text: &str) -> StructuredNode {
        StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain(text),
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

    #[test]
    fn parse_response_nodes_extracts_json_array_from_markdown_fence() {
        let node = make_paragraph("Hello");
        let payload = serde_json::to_string(&vec![node]).expect("serialize");
        let response = format!("Here is the result:\n```json\n{payload}\n```");
        let parsed = parse_response_nodes(&response).expect("should parse fenced json");
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            StructuredNode::Paragraph(p) => assert_eq!(p.content.as_plain_text(), "Hello"),
            _ => panic!("expected paragraph"),
        }
    }

    #[test]
    fn parse_response_nodes_extracts_balanced_json_array_from_mixed_text() {
        let node = make_paragraph("Hello");
        let payload = serde_json::to_string(&vec![node]).expect("serialize");
        let response = format!("Result below:\n{payload}\nDone.");
        let parsed = parse_response_nodes(&response).expect("should parse balanced array");
        assert_eq!(parsed.len(), 1);
    }
}
