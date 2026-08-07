//! LLM tool definitions and executors for the describe-a-reference step.
//!
//! Instead of inlining every page image and the entire XFA XML into the prompt,
//! the model pulls what it needs through tool calls (see
//! [`crate::llm::anthropic_agentic_turn`]). Two executors are provided:
//!
//! * [`FormToolContext`] — built from the source PDFs, it can list the
//!   discovered form states, render any state on demand, return the engine's
//!   structured layout for a state, and hand back the authoritative XFA XML.
//! * [`PackageToolContext`] — read access to the resulting AEM package's text
//!   files, so the model can inspect the `.content.xml` it produced.
//!
//! The conversion agent does not use these: it drives the far larger tool
//! catalog in the headless `agent` crate.

use std::collections::HashMap;

use base64::Engine;

use crate::llm::ToolReply;

/// Render scale for on-demand state images. Kept at 1.0 (below the core
/// pipeline default of 1.5) to cut vision-token cost: at this resolution form
/// text is still comfortably legible to the model, and vision tokens scale with
/// pixel area, so 1.0 vs 1.5 is roughly half the tokens per page.
const RENDER_SCALE: f32 = 1.0;

/// A source of LLM tools: it advertises tool definitions and answers calls.
pub trait ToolExecutor {
    /// The Anthropic tool definitions this executor answers.
    fn tools(&self) -> Vec<serde_json::Value>;
    /// Execute one tool call.
    fn execute(&self, name: &str, input: &serde_json::Value) -> ToolReply;
}

/// Build the tool executor for the **describe-a-reference** step (adding a
/// reference form in Settings): the source-PDF tools ([`FormToolContext`] —
/// states, page renders, structured layout, XFA) **plus** read access to the
/// uploaded AEM package files ([`PackageToolContext`]), so the model can
/// analyse both the input form and its final package before writing the
/// description.
pub async fn build_describe_tools(
    pdfs: Vec<(String, Vec<u8>)>,
    package_files: Vec<(String, String)>,
    profile: Option<&str>,
) -> Box<dyn ToolExecutor> {
    // Register the profile's fonts before any on-demand render, so the describe
    // agent's `get_plain_state_image` resolves typefaces instead of failing.
    if let Some(p) = profile {
        let _ = blueprint::load_profile_fonts(p);
    }
    let form = FormToolContext::build(&pdfs).await;
    Box::new(CompositeToolExecutor::new(vec![
        Box::new(form),
        Box::new(PackageToolContext::new(package_files)),
    ]))
}

/// Combine multiple [`ToolExecutor`]s into one: the advertised tools are the
/// concatenation, and a call is routed to the executor that advertises a tool
/// of that name (first match wins).
pub struct CompositeToolExecutor {
    executors: Vec<Box<dyn ToolExecutor>>,
}

impl CompositeToolExecutor {
    pub fn new(executors: Vec<Box<dyn ToolExecutor>>) -> Self {
        Self { executors }
    }
}

impl ToolExecutor for CompositeToolExecutor {
    fn tools(&self) -> Vec<serde_json::Value> {
        self.executors.iter().flat_map(|e| e.tools()).collect()
    }

    fn execute(&self, name: &str, input: &serde_json::Value) -> ToolReply {
        for e in &self.executors {
            if e.tools().iter().any(|t| t["name"].as_str() == Some(name)) {
                return e.execute(name, input);
            }
        }
        ToolReply::Error(format!("Unknown tool: {name}"))
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

/// Tool executor over the source PDFs of a reference form: the states, their
/// renders, their flattened structure and the raw XFA.
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
                // One image per page so tall multi-page forms don't exceed the
                // vision API's per-image size limit.
                match entry.state.render_plain_pages(RENDER_SCALE) {
                    Ok(imgs) => {
                        let encoded: Result<Vec<String>, String> = imgs
                            .iter()
                            .map(|img| {
                                agent::image_encode::encode_rgba_to_jpeg(img, 82)
                                    .map(|jpeg| base64::prelude::BASE64_STANDARD.encode(&jpeg))
                                    .map_err(|e| format!("Encode failed: {e}"))
                            })
                            .collect();
                        match encoded {
                            Ok(images) => ToolReply::Image {
                                media_type: "image/jpeg",
                                images,
                            },
                            Err(e) => ToolReply::Error(e),
                        }
                    }
                    Err(e) => ToolReply::Error(format!("Render failed: {e}")),
                }
            }
            "get_flattened_structure_for_state" => {
                let label = input["state_label"].as_str().unwrap_or_default();
                let Some(entry) = self.states.iter().find(|s| s.label == label) else {
                    return ToolReply::Error(format!("Unknown state_label: {label:?}"));
                };
                let envelope = entry.state.structured(entry.context.clone());
                ToolReply::Text(serde_json::to_string_pretty(&envelope.content).unwrap_or_default())
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

// ── Package-inspection tools (uploaded AEM package, not yet stored) ──────────

/// Tool executor over an unzipped AEM package's text files. Used by the
/// describe-a-reference step so the model can read the resulting `.content.xml`
/// while it analyses the input form.
pub struct PackageToolContext {
    /// path → file content (UTF-8 text entries only).
    files: Vec<(String, String)>,
}

impl PackageToolContext {
    pub fn new(files: Vec<(String, String)>) -> Self {
        Self { files }
    }
}

impl ToolExecutor for PackageToolContext {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
                "name": "list_package_files",
                "description": "List the file paths in the resulting AEM package (the FileVault \
                    ZIP, unzipped). Use read_package_file to read any of them — the form \
                    definition is usually a .content.xml.",
                "input_schema": {"type": "object", "properties": {}}
            }),
            serde_json::json!({
                "name": "read_package_file",
                "description": "Read one AEM package file by its path from list_package_files. \
                    Optional line offset/limit for large files.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "offset": {"type": "integer", "description": "First line (0-based, optional)."},
                        "limit": {"type": "integer", "description": "Max lines (optional)."}
                    },
                    "required": ["path"]
                }
            }),
        ]
    }

    fn execute(&self, name: &str, input: &serde_json::Value) -> ToolReply {
        match name {
            "list_package_files" => {
                let paths: Vec<&String> = self.files.iter().map(|(p, _)| p).collect();
                ToolReply::Text(serde_json::to_string_pretty(&paths).unwrap_or_default())
            }
            "read_package_file" => {
                let path = input["path"].as_str().unwrap_or_default();
                let Some((_, content)) = self.files.iter().find(|(p, _)| p == path) else {
                    return ToolReply::Error(format!("No such package file: {path:?}"));
                };
                let offset = input["offset"].as_u64().unwrap_or(0) as usize;
                let limit = input["limit"].as_u64().unwrap_or(0) as usize;
                if offset == 0 && limit == 0 {
                    return ToolReply::Text(content.clone());
                }
                let sliced: String = content
                    .lines()
                    .skip(offset)
                    .take(if limit == 0 { usize::MAX } else { limit })
                    .collect::<Vec<_>>()
                    .join("\n");
                ToolReply::Text(sliced)
            }
            other => ToolReply::Error(format!("Unknown package tool: {other}")),
        }
    }
}

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

    fn ctx() -> PackageToolContext {
        PackageToolContext::new(vec![(
            "jcr_root/.content.xml".to_string(),
            "one\ntwo\nthree\nfour".to_string(),
        )])
    }

    #[test]
    fn list_package_files_returns_paths() {
        let reply = ctx().execute("list_package_files", &serde_json::json!({}));
        match reply {
            ToolReply::Text(t) => assert!(t.contains("jcr_root/.content.xml"), "{t}"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn read_package_file_returns_the_whole_file_by_default() {
        let reply = ctx().execute(
            "read_package_file",
            &serde_json::json!({"path": "jcr_root/.content.xml"}),
        );
        match reply {
            ToolReply::Text(t) => assert_eq!(t, "one\ntwo\nthree\nfour"),
            _ => panic!("expected text"),
        }
    }

    /// Large `.content.xml` files are read in windows, so the offset/limit pair
    /// has to slice by line rather than return the whole file.
    #[test]
    fn read_package_file_honours_offset_and_limit() {
        let reply = ctx().execute(
            "read_package_file",
            &serde_json::json!({"path": "jcr_root/.content.xml", "offset": 1, "limit": 2}),
        );
        match reply {
            ToolReply::Text(t) => assert_eq!(t, "two\nthree"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn read_package_file_errors_for_an_unknown_path() {
        let reply = ctx().execute("read_package_file", &serde_json::json!({"path": "nope"}));
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
