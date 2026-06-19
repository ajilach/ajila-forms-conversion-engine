//! LLM tool definitions and executors for AI features.
//!
//! Instead of inlining every page image and the entire XFA XML into the prompt,
//! the model pulls what it needs through tool calls (see
//! [`crate::platform::anthropic_agentic_turn`]). Two executors are provided:
//!
//! * [`FormToolContext`] — for whole-document AI generation. Built from the
//!   source PDFs, it can list the discovered form states, render any state on
//!   demand, return the engine's structured layout for a state, and hand back
//!   the authoritative XFA XML.
//! * [`ImageToolContext`] — for Smart Edit, which only has the pre-rendered
//!   plain page images (no PDFs/states/XFA at its call site). It backs the
//!   `list_states` and `get_plain_state_image` tools from that image map.

use std::collections::HashMap;

use base64::Engine;

use crate::platform::ToolReply;

/// Render scale for on-demand state images — matches the core pipeline default
/// (`PipelineConfig::scale`) so tool images look like the staged renders.
const RENDER_SCALE: f32 = 1.5;

/// A source of LLM tools: it advertises tool definitions and answers calls.
pub trait ToolExecutor {
    /// The Anthropic tool definitions this executor answers.
    fn tools(&self) -> Vec<serde_json::Value>;
    /// Execute one tool call.
    fn execute(&self, name: &str, input: &serde_json::Value) -> ToolReply;
}

/// Build the tool executor for an editing session.
///
/// When the source PDFs are available, this is the full [`FormToolContext`] —
/// the exact same tools AI processing uses (states, on-demand renders,
/// structured layout, XFA). Otherwise (JSON/AEM input, or a reopened session
/// without the source PDFs) it falls back to the pre-rendered page images via
/// [`ImageToolContext`].
pub async fn build_tools(
    source_pdfs: &[(String, Vec<u8>)],
    plain_images: &HashMap<String, String>,
) -> Box<dyn ToolExecutor> {
    if source_pdfs.is_empty() {
        Box::new(ImageToolContext::new(plain_images.clone()))
    } else {
        Box::new(FormToolContext::build(source_pdfs).await)
    }
}

/// Detect the image media type of a base64 payload by its leading bytes.
/// PNG base64 begins with `iVBOR`; JPEG with `/9j/`.
fn media_type_of(b64: &str) -> &'static str {
    if b64.starts_with("/9j/") {
        "image/jpeg"
    } else {
        "image/png"
    }
}

// ── AI-generation tools (states + XFA, built from PDFs) ──────────────────────

/// One discovered form state plus the data needed to answer tool calls about it.
struct StateEntry {
    /// Unique label (prefixed with the PDF name when more than one PDF).
    label: String,
    /// Source PDF this state came from.
    pdf_name: String,
    /// Number of control selections that produced this state.
    selection_count: usize,
    /// The flattened state, for rendering / structuring on demand.
    state: blueprint::FormState,
    /// Context (language + XFA variables) for `structured()`.
    context: blueprint::Context,
}

/// Tool executor for whole-document AI generation.
pub struct FormToolContext {
    states: Vec<StateEntry>,
    /// PDF name → XFA XML (only PDFs that actually carry XFA).
    xfa_by_pdf: HashMap<String, String>,
}

impl FormToolContext {
    /// Build the context from the source PDFs.
    ///
    /// State discovery and XFA extraction are CPU-heavy, so the work runs on a
    /// blocking thread. Best-effort per PDF: a PDF that fails to parse simply
    /// contributes no states (and no XFA).
    pub async fn build(pdfs: &[(String, Vec<u8>)]) -> Self {
        let pdfs_owned: Vec<(String, Vec<u8>)> = pdfs.to_vec();
        let multi = pdfs_owned.len() > 1;

        tokio::task::spawn_blocking(move || {
            let mut states: Vec<StateEntry> = Vec::new();
            let mut xfa_by_pdf: HashMap<String, String> = HashMap::new();

            for (name, bytes) in &pdfs_owned {
                if let Ok(Some(xfa)) = blueprint::extract_xfa_from_pdf_bytes(bytes) {
                    xfa_by_pdf.insert(name.clone(), String::from_utf8_lossy(&xfa).into_owned());
                }

                let Ok(mut bp) = blueprint::Blueprint::from_pdf_bytes(bytes) else {
                    continue;
                };
                let context = bp.context();
                let Ok(form_states) = bp.states() else {
                    continue;
                };
                for state in form_states.iter() {
                    let label = if multi {
                        format!("{name}::{}", state.label)
                    } else {
                        state.label.clone()
                    };
                    states.push(StateEntry {
                        label,
                        pdf_name: name.clone(),
                        selection_count: state.selections.len(),
                        state,
                        context: context.clone(),
                    });
                }
            }

            FormToolContext { states, xfa_by_pdf }
        })
        .await
        .unwrap_or(FormToolContext {
            states: Vec::new(),
            xfa_by_pdf: HashMap::new(),
        })
    }
}

impl ToolExecutor for FormToolContext {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![
            tool_list_states(),
            tool_get_plain_state_image(),
            serde_json::json!({
                "name": "get_flattened_structure_for_state",
                "description": "Return the form engine's own structured layout (a JSON node tree) \
                    for one state, identified by its label from list_states. Useful as a starting \
                    reference for grouping and field types.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "state_label": {"type": "string", "description": "A label from list_states."}
                    },
                    "required": ["state_label"]
                }
            }),
            serde_json::json!({
                "name": "get_xfa",
                "description": "Return the raw XFA/XDP XML form definition. This is the \
                    AUTHORITATIVE source for fields, labels, options, and dynamic behaviour. \
                    Omit pdf_name to get all PDFs' XFA concatenated.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "pdf_name": {"type": "string", "description": "Optional source PDF name."}
                    }
                }
            }),
        ]
    }

    fn execute(&self, name: &str, input: &serde_json::Value) -> ToolReply {
        match name {
            "list_states" => {
                let list: Vec<serde_json::Value> = self
                    .states
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "label": s.label,
                            "pdf": s.pdf_name,
                            "selections": s.selection_count,
                        })
                    })
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
            }
            "get_plain_state_image" => {
                let label = input["state_label"].as_str().unwrap_or_default();
                let Some(entry) = self.states.iter().find(|s| s.label == label) else {
                    return ToolReply::Error(format!("Unknown state_label: {label:?}"));
                };
                match entry.state.render_plain(RENDER_SCALE) {
                    Ok(img) => match crate::pipeline::encode_rgba_to_jpeg(&img, 82) {
                        Ok(jpeg) => ToolReply::Image {
                            media_type: "image/jpeg",
                            b64: base64::prelude::BASE64_STANDARD.encode(&jpeg),
                        },
                        Err(e) => ToolReply::Error(format!("Encode failed: {e}")),
                    },
                    Err(e) => ToolReply::Error(format!("Render failed: {e}")),
                }
            }
            "get_flattened_structure_for_state" => {
                let label = input["state_label"].as_str().unwrap_or_default();
                let Some(entry) = self.states.iter().find(|s| s.label == label) else {
                    return ToolReply::Error(format!("Unknown state_label: {label:?}"));
                };
                let envelope = entry.state.structured(entry.context.clone());
                ToolReply::Text(
                    serde_json::to_string_pretty(&envelope.content).unwrap_or_default(),
                )
            }
            "get_xfa" => {
                if let Some(pdf) = input["pdf_name"].as_str() {
                    match self.xfa_by_pdf.get(pdf) {
                        Some(xml) => ToolReply::Text(xml.clone()),
                        None => ToolReply::Error(format!("No XFA for pdf_name: {pdf:?}")),
                    }
                } else if self.xfa_by_pdf.is_empty() {
                    ToolReply::Error("No XFA XML is present in the input PDFs.".to_string())
                } else {
                    let combined = self
                        .xfa_by_pdf
                        .iter()
                        .map(|(name, xml)| {
                            format!("BEGIN XFA XML ({name})\n{xml}\nEND XFA XML ({name})")
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    ToolReply::Text(combined)
                }
            }
            other => ToolReply::Error(format!("Unknown tool: {other}")),
        }
    }
}

// ── Smart Edit tools (pre-rendered images only) ──────────────────────────────

/// Tool executor backed only by the pre-rendered plain page images. Used by
/// Smart Edit, which has no source PDFs / states / XFA at its call site.
pub struct ImageToolContext {
    /// label → base64 image (JPEG/PNG).
    images: HashMap<String, String>,
}

impl ImageToolContext {
    pub fn new(images: HashMap<String, String>) -> Self {
        Self { images }
    }
}

impl ToolExecutor for ImageToolContext {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![tool_list_states(), tool_get_plain_state_image()]
    }

    fn execute(&self, name: &str, input: &serde_json::Value) -> ToolReply {
        match name {
            "list_states" => {
                let mut labels: Vec<&String> = self.images.keys().collect();
                labels.sort();
                let list: Vec<serde_json::Value> = labels
                    .iter()
                    .map(|l| serde_json::json!({"label": l}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
            }
            "get_plain_state_image" => {
                let label = input["state_label"].as_str().unwrap_or_default();
                match self.images.get(label) {
                    Some(b64) => ToolReply::Image {
                        media_type: media_type_of(b64),
                        b64: b64.clone(),
                    },
                    None => ToolReply::Error(format!("Unknown state_label: {label:?}")),
                }
            }
            other => ToolReply::Error(format!("Unknown tool: {other}")),
        }
    }
}

// ── Shared tool definitions ──────────────────────────────────────────────────

fn tool_list_states() -> serde_json::Value {
    serde_json::json!({
        "name": "list_states",
        "description": "List the available form states (each a distinct combination of radio / \
            checkbox / dropdown selections). Returns an array of objects with a `label` used to \
            address a state in the other tools.",
        "input_schema": {"type": "object", "properties": {}}
    })
}

fn tool_get_plain_state_image() -> serde_json::Value {
    serde_json::json!({
        "name": "get_plain_state_image",
        "description": "Return a rendered page image (plain, no overlays) for one form state, \
            identified by its label from list_states.",
        "input_schema": {
            "type": "object",
            "properties": {
                "state_label": {"type": "string", "description": "A label from list_states."}
            },
            "required": ["state_label"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ImageToolContext {
        let mut images = HashMap::new();
        // `/9j/` prefix marks a JPEG payload.
        images.insert("default".to_string(), "/9j/abc".to_string());
        ImageToolContext::new(images)
    }

    #[test]
    fn list_states_returns_labels() {
        let reply = ctx().execute("list_states", &serde_json::json!({}));
        match reply {
            ToolReply::Text(t) => assert!(t.contains("default")),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn get_image_returns_image_for_known_label() {
        let reply = ctx().execute(
            "get_plain_state_image",
            &serde_json::json!({"state_label": "default"}),
        );
        match reply {
            ToolReply::Image { media_type, b64 } => {
                assert_eq!(media_type, "image/jpeg");
                assert_eq!(b64, "/9j/abc");
            }
            _ => panic!("expected image"),
        }
    }

    #[test]
    fn get_image_errors_for_unknown_label() {
        let reply = ctx().execute(
            "get_plain_state_image",
            &serde_json::json!({"state_label": "nope"}),
        );
        assert!(matches!(reply, ToolReply::Error(_)));
    }

    #[test]
    fn unknown_tool_errors() {
        assert!(matches!(
            ctx().execute("bogus", &serde_json::json!({})),
            ToolReply::Error(_)
        ));
    }
}
