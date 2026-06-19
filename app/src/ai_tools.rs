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
/// When the source PDFs are available, the base is the full [`FormToolContext`]
/// — the exact same tools AI processing uses (states, on-demand renders,
/// structured layout, XFA). Otherwise (JSON/AEM input, or a reopened session
/// without the source PDFs) it falls back to the pre-rendered page images via
/// [`ImageToolContext`].
///
/// If `profile` has stored reference forms, the base is combined with a
/// [`ReferenceToolContext`] so the model can also search/read worked examples
/// for that profile (see [`CompositeToolExecutor`]).
pub async fn build_tools(
    source_pdfs: &[(String, Vec<u8>)],
    plain_images: &HashMap<String, String>,
    profile: Option<&str>,
) -> Box<dyn ToolExecutor> {
    let base: Box<dyn ToolExecutor> = if source_pdfs.is_empty() {
        Box::new(ImageToolContext::new(plain_images.clone()))
    } else {
        // Register the profile's fonts in the global font manager so on-demand
        // page renders (`get_plain_state_image`) resolve typefaces — otherwise
        // the render hard-fails when no conversion has loaded fonts this session.
        if let Some(p) = profile {
            let _ = blueprint::load_profile_fonts(p);
        }
        Box::new(FormToolContext::build(source_pdfs).await)
    };

    if let Some(p) = profile
        && crate::references::count(p) > 0
        && let Some(refs) = ReferenceToolContext::new(p)
    {
        return Box::new(CompositeToolExecutor::new(vec![base, Box::new(refs)]));
    }

    base
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
            if e.tools()
                .iter()
                .any(|t| t["name"].as_str() == Some(name))
            {
                return e.execute(name, input);
            }
        }
        ToolReply::Error(format!("Unknown tool: {name}"))
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

// ── Reference-form tools (worked examples, per profile) ──────────────────────

/// Tool executor backed by the per-profile reference-form store
/// ([`crate::references`]). Lets the model search worked examples (original
/// form + final AEM package + description), read their files, and view source
/// pages. Needs the embedding model + SQLite.
pub struct ReferenceToolContext {
    profile: String,
    matcher: blueprint::semantic::SemanticMatcher,
}

impl ReferenceToolContext {
    /// Load the embedding model for this profile's reference searches. Returns
    /// `None` if the model fails to load (the caller then omits reference tools).
    pub fn new(profile: &str) -> Option<Self> {
        let matcher = blueprint::semantic::SemanticMatcher::new().ok()?;
        Some(Self {
            profile: profile.to_string(),
            matcher,
        })
    }
}

impl ToolExecutor for ReferenceToolContext {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
                "name": "list_reference_forms",
                "description": "List the reference forms available for this profile. Each is a \
                    worked example: an original input form, its final AEM package, and a \
                    description. Returns ref_id, label, description, pdf_count, and the package's \
                    file paths. Consult these before converting an unfamiliar block.",
                "input_schema": {"type": "object", "properties": {}}
            }),
            serde_json::json!({
                "name": "search_references",
                "description": "Search the profile's reference forms. Hybrid match: semantic \
                    similarity over the descriptions, plus literal substring match in the \
                    descriptions and in the AEM package XML. Returns hits with ref_id, where the \
                    match was (a file path or 'description'), the matching signal, and a snippet.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "What to look for (a block type, field name, label, or AEM resource type)."},
                        "top_k": {"type": "integer", "description": "Max hits per signal (default 3)."}
                    },
                    "required": ["query"]
                }
            }),
            serde_json::json!({
                "name": "read_reference_file",
                "description": "Read a reference's description (path 'description') or one of its \
                    AEM package files (a path from list_reference_forms / search_references). \
                    Optional line offset/limit for large files.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "ref_id": {"type": "string"},
                        "path": {"type": "string", "description": "'description' or a package file path."},
                        "offset": {"type": "integer", "description": "First line (0-based, optional)."},
                        "limit": {"type": "integer", "description": "Max lines (optional)."}
                    },
                    "required": ["ref_id", "path"]
                }
            }),
            serde_json::json!({
                "name": "view_reference_page",
                "description": "Render and view a page of a reference's original input form, so you \
                    can see the visual layout the AEM package was produced from.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "ref_id": {"type": "string"},
                        "pdf_index": {"type": "integer", "description": "Which source PDF (default 0)."},
                        "page": {"type": "integer", "description": "Page number (default 0)."}
                    },
                    "required": ["ref_id"]
                }
            }),
        ]
    }

    fn execute(&self, name: &str, input: &serde_json::Value) -> ToolReply {
        match name {
            "list_reference_forms" => {
                let list: Vec<serde_json::Value> = crate::references::list_references(&self.profile)
                    .into_iter()
                    .map(|r| {
                        serde_json::json!({
                            "ref_id": r.ref_id,
                            "label": r.label,
                            "description": r.description,
                            "pdf_count": r.pdf_count,
                            "files": r.files,
                        })
                    })
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
            }
            "search_references" => {
                let query = input["query"].as_str().unwrap_or_default();
                if query.is_empty() {
                    return ToolReply::Error("search_references requires a non-empty query".into());
                }
                let top_k = input["top_k"].as_u64().unwrap_or(3).max(1) as usize;
                let hits = crate::references::search_references(
                    &self.profile,
                    query,
                    &self.matcher,
                    top_k,
                );
                let list: Vec<serde_json::Value> = hits
                    .into_iter()
                    .map(|h| {
                        serde_json::json!({
                            "ref_id": h.ref_id,
                            "label": h.label,
                            "where": h.location,
                            "matched": h.matched,
                            "score": h.score,
                            "snippet": h.snippet,
                        })
                    })
                    .collect();
                if list.is_empty() {
                    ToolReply::Text("No matching reference forms.".to_string())
                } else {
                    ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
                }
            }
            "read_reference_file" => {
                let ref_id = input["ref_id"].as_str().unwrap_or_default();
                let path = input["path"].as_str().unwrap_or_default();
                let offset = input["offset"].as_u64().unwrap_or(0) as usize;
                let limit = input["limit"].as_u64().unwrap_or(0) as usize;
                match crate::references::read_reference_file(ref_id, path, offset, limit) {
                    Ok(text) => ToolReply::Text(text),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "view_reference_page" => {
                let ref_id = input["ref_id"].as_str().unwrap_or_default();
                let pdf_index = input["pdf_index"].as_u64().unwrap_or(0) as usize;
                let page = input["page"].as_u64().unwrap_or(0) as usize;
                match crate::references::render_reference_page(ref_id, pdf_index, page) {
                    Ok(jpeg) => ToolReply::Image {
                        media_type: "image/jpeg",
                        b64: base64::prelude::BASE64_STANDARD.encode(&jpeg),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }
            other => ToolReply::Error(format!("Unknown reference tool: {other}")),
        }
    }
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
