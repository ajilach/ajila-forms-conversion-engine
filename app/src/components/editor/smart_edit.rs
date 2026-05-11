//! Smart edit: AI-assisted document editing via `gh copilot` CLI.
//!
//! Serialises the selected structured nodes to JSON, attaches rendered
//! page images, sends the bundle to GitHub Copilot, and parses the
//! response back into structured nodes.

use std::collections::HashMap;

use blueprint::StructuredNode;
use serde_json::Value;

use crate::platform::run_copilot_smart_edit;

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
/// Returns the raw AI response text on success.
pub async fn run_smart_edit(
    content: &[StructuredNode],
    selected_indices: &[usize],
    plain_images: &HashMap<String, String>,
) -> Result<String, String> {
    let json_context = serialize_selected_nodes(content, selected_indices)?;

    let images: Vec<(String, String)> = plain_images
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let prompt = build_smart_edit_prompt(selected_indices, plain_images);

    run_copilot_smart_edit(&prompt, &json_context, &images).await
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
         Output format:\n\
         - Return ONLY one valid JSON array representing the replacement structured nodes.\n\
         - The JSON must be directly parseable as Vec<StructuredNode>.\n\
         - No surrounding prose, no trailing notes, no backticks.\n\
         \n\
         Attached images: {}",
        plain_images.len()
    )
}

/// Try to extract a JSON array of StructuredNode from the AI response.
///
/// The response might contain markdown fences or surrounding prose, so we
/// try to find the outermost `[…]` and parse that.
pub fn parse_response_nodes(response: &str) -> Result<Vec<StructuredNode>, String> {
    // Try direct parse first
    if let Ok(nodes) = serde_json::from_str::<Vec<StructuredNode>>(response) {
        return Ok(nodes);
    }
    if let Some(nodes) = parse_value_wrapped_nodes(response) {
        return Ok(nodes);
    }

    // Try fenced code blocks first (json and generic).
    let trimmed = response.trim();
    for block in extract_fenced_blocks(trimmed) {
        if let Ok(nodes) = serde_json::from_str::<Vec<StructuredNode>>(block) {
            return Ok(nodes);
        }
        if let Some(nodes) = parse_value_wrapped_nodes(block) {
            return Ok(nodes);
        }
    }

    // Try to find any balanced JSON array in the raw text.
    for candidate in extract_json_array_candidates(trimmed) {
        if let Ok(nodes) = serde_json::from_str::<Vec<StructuredNode>>(candidate) {
            return Ok(nodes);
        }
        if let Some(nodes) = parse_value_wrapped_nodes(candidate) {
            return Ok(nodes);
        }
    }

    Err(format!(
        "Could not parse structured nodes from AI response. Raw response:\n{response}"
    ))
}

fn parse_value_wrapped_nodes(input: &str) -> Option<Vec<StructuredNode>> {
    let value = serde_json::from_str::<Value>(input).ok()?;
    let object = value.as_object()?;

    for key in ["nodes", "structured_nodes", "result", "output"] {
        let array_value = object.get(key)?;
        if let Ok(nodes) = serde_json::from_value::<Vec<StructuredNode>>(array_value.clone()) {
            return Some(nodes);
        }
    }

    None
}

fn extract_fenced_blocks(input: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut rest = input;

    while let Some(start) = rest.find("```") {
        let after_start = &rest[start + 3..];
        let body_start = after_start.find('\n').map_or(after_start, |nl| &after_start[nl + 1..]);
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

    #[test]
    fn parse_response_nodes_extracts_json_array_from_markdown_fence() {
        let node = StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("Hello"),
            som_path: None,
            source_name: None,
        });
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
        let node = StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("Hello"),
            som_path: None,
            source_name: None,
        });
        let payload = serde_json::to_string(&vec![node]).expect("serialize");
        let response = format!("Result below:\n{payload}\nDone.");

        let parsed = parse_response_nodes(&response).expect("should parse balanced array");
        assert_eq!(parsed.len(), 1);
    }
}
