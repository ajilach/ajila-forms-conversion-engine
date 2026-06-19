//! Autonomous conversion agent — the LLM drives the whole conversion engine via
//! tools, replacing manual interaction. It extracts from the source PDF, builds
//! and edits a **structured** node tree, converts to an **AEM** node tree, edits
//! that, packages it, optionally uploads to AEM and verifies, and can consult
//! reference forms / documentation. Every tree change is snapshotted into an
//! edit-history session so the user can review the agent's full history in the
//! structured / AEM editors.
//!
//! Tree mutations use a **whole-tree replace** model: the model reads a tree
//! (`get_*`) and writes the whole tree back (`set_*`); each write is versioned.
//!
//! Desktop-only (engine + network + SQLite).

use std::collections::HashMap;

use dioxus::prelude::*;

use blueprint::{AemConfig, AemConnection, AemNode, Context, DocumentEnvelope, StructuredNode};

use crate::models::{AgentStep, AgentStepKind, AgentStepStatus, ProcessingState, ProcessingStep};
use crate::platform::{ToolReply, anthropic_stream_turn, tool_result_message};

/// Output-token cap per agent turn.
const AGENT_MAX_TOKENS: u32 = 16000;
/// Max streamed turns before the loop bails out (the agent makes many calls).
const MAX_ITERATIONS: usize = 200;
/// Render scale for on-demand page images.
const RENDER_SCALE: f32 = 1.5;

// ── Per-source extraction (sync; cached) ─────────────────────────────────────

struct StateRec {
    label: String,
    pdf_name: String,
    selections: usize,
    state: blueprint::FormState,
    context: Context,
}

/// The engine's view of one input source (the uploaded form, or a reference):
/// discovered states (for listing / rendering / per-state structure), the XFA,
/// and the merged structured tree.
struct Extractor {
    states: Vec<StateRec>,
    xfa: Vec<(String, String)>,
    merged: Vec<StructuredNode>,
}

impl Extractor {
    fn build(pdfs: &[(String, Vec<u8>)]) -> Self {
        let multi = pdfs.len() > 1;
        let mut states = Vec::new();
        let mut xfa = Vec::new();
        let mut envelopes: Vec<DocumentEnvelope> = Vec::new();

        for (name, bytes) in pdfs {
            if let Ok(Some(x)) = blueprint::extract_xfa_from_pdf_bytes(bytes) {
                xfa.push((name.clone(), String::from_utf8_lossy(&x).into_owned()));
            }
            if let Ok(mut bp) = blueprint::Blueprint::from_pdf_bytes(bytes) {
                let context = bp.context();
                if let Ok(fs) = bp.states() {
                    for s in fs.iter() {
                        let label = if multi {
                            format!("{name}::{}", s.label)
                        } else {
                            s.label.clone()
                        };
                        let selections = s.selections.len();
                        states.push(StateRec {
                            label,
                            pdf_name: name.clone(),
                            selections,
                            state: s,
                            context: context.clone(),
                        });
                    }
                }
            }
            // Merged structured needs its own Blueprint (states()/merged both &mut).
            if let Ok(mut bp2) = blueprint::Blueprint::from_pdf_bytes(bytes)
                && let Ok(env) = bp2.merged_structured()
            {
                envelopes.push(env);
            }
        }

        let merged = match envelopes.len() {
            0 => Vec::new(),
            1 => envelopes.into_iter().next().unwrap().content,
            _ => blueprint::merge_translations(envelopes, None)
                .map(|e| e.content)
                .unwrap_or_default(),
        };

        Extractor {
            states,
            xfa,
            merged,
        }
    }

    fn find(&self, label: &str) -> Option<&StateRec> {
        self.states.iter().find(|s| s.label == label)
    }
}

// ── The agent ────────────────────────────────────────────────────────────────

pub struct ConversionAgent {
    profile: Option<String>,
    context: Context,
    aem_config: Option<AemConfig>,
    conn: Option<AemConnection>,
    current_pdfs: Vec<(String, Vec<u8>)>,
    extractors: HashMap<String, Extractor>,

    structured: Vec<StructuredNode>,
    aem: Option<AemNode>,
    package: Option<Vec<u8>>,

    structured_session: String,
    aem_session: Option<String>,

    finished: bool,
}

impl ConversionAgent {
    pub fn new(
        profile: Option<String>,
        pdfs: Vec<(String, Vec<u8>)>,
        conn: Option<AemConnection>,
        structured_session: String,
    ) -> Self {
        let context = pdfs
            .iter()
            .find_map(|(_, b)| blueprint::Blueprint::from_pdf_bytes(b).ok().map(|bp| bp.context()))
            .unwrap_or_else(|| Context::with_language("en"));
        Self {
            profile,
            context,
            aem_config: None,
            conn,
            current_pdfs: pdfs,
            extractors: HashMap::new(),
            structured: Vec::new(),
            aem: None,
            package: None,
            structured_session,
            aem_session: None,
            finished: false,
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn source_key(input: &serde_json::Value) -> String {
        match input["source"]["reference"].as_str() {
            Some(id) => format!("reference:{id}"),
            None => "current".to_string(),
        }
    }

    /// Get (building+caching if needed) the extractor for the requested source.
    fn extractor(&mut self, input: &serde_json::Value) -> Result<&Extractor, String> {
        let key = Self::source_key(input);
        if !self.extractors.contains_key(&key) {
            let pdfs = match input["source"]["reference"].as_str() {
                Some(id) => {
                    let bytes = crate::references::get_reference_pdf_bytes(id, 0)?;
                    vec![(format!("{id}.pdf"), bytes)]
                }
                None => self.current_pdfs.clone(),
            };
            self.extractors.insert(key.clone(), Extractor::build(&pdfs));
        }
        Ok(self.extractors.get(&key).unwrap())
    }

    fn config(&mut self) -> Result<AemConfig, String> {
        if self.aem_config.is_none() {
            let p = self
                .profile
                .clone()
                .ok_or("No profile selected — AEM conversion needs a profile.")?;
            self.aem_config = Some(blueprint::load_aem_config(&p, &self.context)?);
        }
        Ok(self.aem_config.clone().unwrap())
    }

    fn snapshot_structured(&mut self, label: &str) {
        if let Ok(json) = serde_json::to_string(&self.structured) {
            crate::db::insert_edit(&self.structured_session, label, &json);
        }
    }

    fn snapshot_aem(&mut self, label: &str) {
        let Some(ref aem) = self.aem else { return };
        let Ok(json) = serde_json::to_string(aem) else {
            return;
        };
        // Write to the same derived id the AEM editor reads
        // (`{structured_session}#aem`) so the agent's AEM history shows there.
        let sid = self
            .aem_session
            .get_or_insert_with(|| format!("{}#aem", self.structured_session))
            .clone();
        crate::db::insert_edit(&sid, label, &json);
    }

    // ── Tool definitions ───────────────────────────────────────────────────────

    pub fn tools(&self) -> Vec<serde_json::Value> {
        let source = serde_json::json!({
            "source": {
                "type": "object",
                "description": "Optional: which input to read. Omit for the uploaded form, or {\"reference\": \"<ref_id>\"} to run the engine on a reference's input.",
                "properties": { "reference": { "type": "string" } }
            }
        });
        let with_source = |props: serde_json::Value| {
            let mut m = props.as_object().cloned().unwrap_or_default();
            m.insert("source".to_string(), source["source"].clone());
            serde_json::Value::Object(m)
        };
        let t = |name: &str, desc: &str, props: serde_json::Value, required: serde_json::Value| {
            serde_json::json!({
                "name": name, "description": desc,
                "input_schema": { "type": "object", "properties": props, "required": required }
            })
        };
        let state_label = serde_json::json!({ "state_label": {"type": "string", "description": "A label from list_states."} });

        vec![
            // §1 extraction (source-parameterized)
            t("get_source_info", "Info about the source PDFs (name, language, state count).", with_source(serde_json::json!({})), serde_json::json!([])),
            t("explore_states", "Run exhaustive state discovery on the source; returns a count summary.", with_source(serde_json::json!({})), serde_json::json!([])),
            t("list_states", "List discovered form states (label, pdf, selection count).", with_source(serde_json::json!({})), serde_json::json!([])),
            t("get_xfa", "Return the source's authoritative XFA XML (all PDFs concatenated).", with_source(serde_json::json!({})), serde_json::json!([])),
            t("search_xfa", "Regex/substring search within the source's XFA; returns matching snippets.", with_source(serde_json::json!({"query": {"type":"string"}, "regex": {"type":"boolean"}})), serde_json::json!(["query"])),
            t("get_plain_state_image", "Render a state's page image (plain).", with_source(state_label.clone()), serde_json::json!(["state_label"])),
            t("get_annotated_state_image", "Render a state's page image with field-name overlays.", with_source(state_label.clone()), serde_json::json!(["state_label"])),
            t("get_flattened_structure_for_state", "Engine structured tree for one state.", with_source(state_label.clone()), serde_json::json!(["state_label"])),
            t("get_merged_structured", "The engine's full merged structured tree for the source (the usual seed for set_structured).", with_source(serde_json::json!({})), serde_json::json!([])),
            // §2 structured tree
            t("get_structured", "Return the current working structured tree (JSON).", serde_json::json!({}), serde_json::json!([])),
            t("set_structured", "Replace the whole structured tree. `nodes` is a JSON array parseable as Vec<StructuredNode>. Versioned.", serde_json::json!({"nodes": {"type":"array"}}), serde_json::json!(["nodes"])),
            // §3 conversion
            t("convert_structured_to_aem", "Convert the current structured tree to the AEM tree (replaces it). Versioned.", serde_json::json!({}), serde_json::json!([])),
            // §4 aem tree
            t("get_aem", "Return the current working AEM tree (JSON).", serde_json::json!({}), serde_json::json!([])),
            t("set_aem", "Replace the whole AEM tree. `root` is a JSON object parseable as AemNode. Versioned.", serde_json::json!({"root": {"type":"object"}}), serde_json::json!(["root"])),
            t("get_aem_xml", "Render the current AEM tree to the final JCR .content.xml string.", serde_json::json!({}), serde_json::json!([])),
            // §5 output
            t("build_aem_package", "Build the AEM FileVault package (ZIP) from the current AEM tree. Stores it for upload.", serde_json::json!({}), serde_json::json!([])),
            t("get_package_info", "Size and file list of the built package.", serde_json::json!({}), serde_json::json!([])),
            t("read_package_file", "Read a file from the built package by path.", serde_json::json!({"path": {"type":"string"}}), serde_json::json!(["path"])),
            t("generate_xsd", "Generate the XSD schema for the current structured tree.", serde_json::json!({}), serde_json::json!([])),
            t("generate_html", "Generate an HTML preview of the current structured tree.", serde_json::json!({}), serde_json::json!([])),
            // §6 deploy + verify
            t("upload_to_aem", "Upload and install the built package on the configured AEM instance.", serde_json::json!({}), serde_json::json!([])),
            t("fetch_aem_form_html", "Fetch the rendered Adaptive Form HTML from AEM (after upload) for verification.", serde_json::json!({}), serde_json::json!([])),
            t("fetch_aem_dor_pdf", "Fetch the Document-of-Record PDF from AEM and view its first page.", serde_json::json!({}), serde_json::json!([])),
            // §7 references
            t("list_reference_forms", "List the profile's reference forms (worked examples).", serde_json::json!({}), serde_json::json!([])),
            t("search_references", "Regex/substring search over reference descriptions + AEM package XML.", serde_json::json!({"query": {"type":"string"}, "regex": {"type":"boolean"}}), serde_json::json!(["query"])),
            t("read_reference_file", "Read a reference's description ('description') or a package file by path.", serde_json::json!({"ref_id": {"type":"string"}, "path": {"type":"string"}, "offset": {"type":"integer"}, "limit": {"type":"integer"}}), serde_json::json!(["ref_id","path"])),
            t("get_reference_package", "List the package files (known-good output) of a reference.", serde_json::json!({"ref_id": {"type":"string"}}), serde_json::json!(["ref_id"])),
            t("list_reference_docs", "List the profile's reference documentation (.md/.txt).", serde_json::json!({}), serde_json::json!([])),
            t("read_reference_doc", "Read a reference documentation doc by id.", serde_json::json!({"doc_id": {"type":"string"}, "offset": {"type":"integer"}, "limit": {"type":"integer"}}), serde_json::json!(["doc_id"])),
            t("grep_reference_docs", "Regex/substring search over reference documentation.", serde_json::json!({"query": {"type":"string"}, "regex": {"type":"boolean"}}), serde_json::json!(["query"])),
            // §8 control
            t("get_schema", "Return the JSON schema for the 'structured' or 'aem' tree.", serde_json::json!({"kind": {"type":"string","enum":["structured","aem"]}}), serde_json::json!(["kind"])),
            t("get_profile_info", "Profile/AEM config: form_code, languages, JCR paths, binding flags.", serde_json::json!({}), serde_json::json!([])),
            t("finish", "Finalize: persist the structured + AEM trees + package as the result and end.", serde_json::json!({"summary": {"type":"string"}}), serde_json::json!([])),
        ]
    }

    // ── Tool execution (async: some tools hit the network) ──────────────────────

    pub async fn execute(&mut self, name: &str, input: &serde_json::Value) -> ToolReply {
        match name {
            // §1 extraction
            "get_source_info" => match self.extractor(input) {
                Ok(ex) => {
                    let langs: Vec<&str> = ex.states.iter().map(|s| s.context.language()).collect();
                    ToolReply::Text(format!(
                        "states: {}, languages: {:?}, xfa_pdfs: {}",
                        ex.states.len(),
                        dedup(langs),
                        ex.xfa.len()
                    ))
                }
                Err(e) => ToolReply::Error(e),
            },
            "explore_states" => match self.extractor(input) {
                Ok(ex) => ToolReply::Text(format!("Discovered {} state(s).", ex.states.len())),
                Err(e) => ToolReply::Error(e),
            },
            "list_states" => match self.extractor(input) {
                Ok(ex) => {
                    let list: Vec<_> = ex
                        .states
                        .iter()
                        .map(|s| serde_json::json!({"label": s.label, "pdf": s.pdf_name, "selections": s.selections}))
                        .collect();
                    ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
                }
                Err(e) => ToolReply::Error(e),
            },
            "get_xfa" => match self.extractor(input) {
                Ok(ex) if ex.xfa.is_empty() => {
                    ToolReply::Error("No XFA present in the source.".into())
                }
                Ok(ex) => ToolReply::Text(
                    ex.xfa
                        .iter()
                        .map(|(n, x)| format!("BEGIN XFA ({n})\n{x}\nEND XFA ({n})"))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                ),
                Err(e) => ToolReply::Error(e),
            },
            "search_xfa" => {
                let query = input["query"].as_str().unwrap_or_default().to_string();
                let regex = input["regex"].as_bool().unwrap_or(false);
                match self.extractor(input) {
                    Ok(ex) => {
                        let mut out = String::new();
                        for (n, x) in &ex.xfa {
                            for line in x.lines().filter(|l| line_matches(l, &query, regex)) {
                                out.push_str(&format!("{n}: {}\n", line.trim()));
                                if out.len() > 4000 {
                                    break;
                                }
                            }
                        }
                        if out.is_empty() {
                            ToolReply::Text("No matches.".into())
                        } else {
                            ToolReply::Text(out)
                        }
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_plain_state_image" | "get_annotated_state_image" => {
                let label = input["state_label"].as_str().unwrap_or_default().to_string();
                let annotated = name == "get_annotated_state_image";
                match self.extractor(input) {
                    Ok(ex) => match ex.find(&label) {
                        Some(rec) => {
                            let img = if annotated {
                                rec.state.render_annotated(RENDER_SCALE)
                            } else {
                                rec.state.render_plain(RENDER_SCALE)
                            };
                            match img.map_err(|e| e.to_string()).and_then(|i| {
                                crate::pipeline::encode_rgba_to_jpeg(&i, 82).map_err(|e| e.to_string())
                            }) {
                                Ok(jpeg) => ToolReply::Image {
                                    media_type: "image/jpeg",
                                    b64: base64_encode(&jpeg),
                                },
                                Err(e) => ToolReply::Error(format!("Render failed: {e}")),
                            }
                        }
                        None => ToolReply::Error(format!("Unknown state_label: {label:?}")),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_flattened_structure_for_state" => {
                let label = input["state_label"].as_str().unwrap_or_default().to_string();
                match self.extractor(input) {
                    Ok(ex) => match ex.find(&label) {
                        Some(rec) => {
                            let env = rec.state.structured(rec.context.clone());
                            ToolReply::Text(serde_json::to_string_pretty(&env.content).unwrap_or_default())
                        }
                        None => ToolReply::Error(format!("Unknown state_label: {label:?}")),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_merged_structured" => match self.extractor(input) {
                Ok(ex) => ToolReply::Text(serde_json::to_string_pretty(&ex.merged).unwrap_or_default()),
                Err(e) => ToolReply::Error(e),
            },

            // §2 structured
            "get_structured" => {
                ToolReply::Text(serde_json::to_string_pretty(&self.structured).unwrap_or_default())
            }
            "set_structured" => {
                let v = input.get("nodes").cloned().unwrap_or_else(|| input.clone());
                match serde_json::from_value::<Vec<StructuredNode>>(v) {
                    Ok(nodes) => {
                        self.structured = nodes;
                        self.snapshot_structured("AI: set structured");
                        ToolReply::Text(format!("OK ({} top-level node(s)).", self.structured.len()))
                    }
                    Err(e) => ToolReply::Error(format!("Invalid StructuredNode JSON: {e}")),
                }
            }

            // §3 conversion
            "convert_structured_to_aem" => {
                if self.structured.is_empty() {
                    return ToolReply::Error("Structured tree is empty; set_structured first.".into());
                }
                let cfg = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                let aem = blueprint::convert_to_aem(&self.structured, &cfg);
                self.aem = Some(aem);
                self.snapshot_aem("AI: convert from structured");
                ToolReply::Text("OK — AEM tree generated from structured.".into())
            }

            // §4 aem
            "get_aem" => match &self.aem {
                Some(aem) => ToolReply::Text(serde_json::to_string_pretty(aem).unwrap_or_default()),
                None => ToolReply::Error("No AEM tree yet; call convert_structured_to_aem.".into()),
            },
            "set_aem" => {
                let v = input.get("root").cloned().unwrap_or_else(|| input.clone());
                match serde_json::from_value::<AemNode>(v) {
                    Ok(node) => {
                        self.aem = Some(node);
                        self.snapshot_aem("AI: set AEM tree");
                        ToolReply::Text("OK.".into())
                    }
                    Err(e) => ToolReply::Error(format!("Invalid AemNode JSON: {e}")),
                }
            }
            "get_aem_xml" => {
                let Some(aem) = self.aem.clone() else {
                    return ToolReply::Error("No AEM tree yet.".into());
                };
                match self.config() {
                    Ok(cfg) => ToolReply::Text(blueprint::generate_aem_xml(&aem, &cfg)),
                    Err(e) => ToolReply::Error(e),
                }
            }

            // §5 output
            "build_aem_package" => {
                let Some(aem) = self.aem.clone() else {
                    return ToolReply::Error("No AEM tree yet; convert first.".into());
                };
                let cfg = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                let translations =
                    blueprint::aem_translations_from_content(&self.structured, &cfg.master_language);
                let pkg = blueprint::to_aem_package_from_node_with_translations(&aem, &cfg, translations);
                let size = pkg.len();
                self.package = Some(pkg);
                ToolReply::Text(format!("Built package ({size} bytes)."))
            }
            "get_package_info" => match &self.package {
                Some(pkg) => {
                    let files = crate::references::unzip_package(pkg).unwrap_or_default();
                    let paths: Vec<&String> = files.iter().map(|(p, _)| p).collect();
                    ToolReply::Text(format!(
                        "size: {} bytes\nfiles:\n{}",
                        pkg.len(),
                        serde_json::to_string_pretty(&paths).unwrap_or_default()
                    ))
                }
                None => ToolReply::Error("No package built yet; call build_aem_package.".into()),
            },
            "read_package_file" => {
                let path = input["path"].as_str().unwrap_or_default();
                match &self.package {
                    Some(pkg) => match crate::references::unzip_package(pkg) {
                        Ok(files) => match files.iter().find(|(p, _)| p == path) {
                            Some((_, c)) => ToolReply::Text(c.clone()),
                            None => ToolReply::Error(format!("No such file: {path:?}")),
                        },
                        Err(e) => ToolReply::Error(e),
                    },
                    None => ToolReply::Error("No package built yet.".into()),
                }
            }
            "generate_xsd" => {
                let p = match self.profile.clone() {
                    Some(p) if blueprint::has_xsd_config(&p) => p,
                    _ => return ToolReply::Error("This profile has no XSD config.".into()),
                };
                match blueprint::load_xsd_config(&p) {
                    Ok(mut cfg) => {
                        if let Ok(c) = self.config() {
                            cfg.form_code = Some(c.form_code.clone());
                        }
                        ToolReply::Text(blueprint::to_xsd(&self.structured, &cfg))
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }
            "generate_html" => {
                let p = match self.profile.clone() {
                    Some(p) if blueprint::has_html_config(&p) => p,
                    _ => return ToolReply::Error("This profile has no HTML config.".into()),
                };
                match blueprint::load_html_custom_styles(&p) {
                    Ok(styles) => {
                        let cfg = blueprint::HtmlConfig {
                            custom_styles: Some(styles),
                            ..blueprint::HtmlConfig::default()
                        };
                        ToolReply::Text(blueprint::to_html(&self.structured, &cfg))
                    }
                    Err(e) => ToolReply::Error(e),
                }
            }

            // §6 deploy + verify (network)
            "upload_to_aem" => {
                let Some(conn) = self.conn.clone() else {
                    return ToolReply::Error("No AEM connection configured.".into());
                };
                let Some(pkg) = self.package.clone() else {
                    return ToolReply::Error("No package built yet; call build_aem_package.".into());
                };
                let cfg = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                match crate::aem_client::upload_and_install_package(&conn, pkg, &cfg.form_code).await {
                    Ok(()) => ToolReply::Text("Uploaded and installed on AEM.".into()),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "fetch_aem_form_html" => {
                let (Some(conn), Ok(cfg)) = (self.conn.clone(), self.config()) else {
                    return ToolReply::Error("No AEM connection / profile configured.".into());
                };
                let path = form_jcr_path(&cfg);
                match crate::aem_client::fetch_form_html(&conn, &path).await {
                    Ok(html) => ToolReply::Text(truncate(&html, 8000)),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "fetch_aem_dor_pdf" => {
                let (Some(conn), Ok(cfg)) = (self.conn.clone(), self.config()) else {
                    return ToolReply::Error("No AEM connection / profile configured.".into());
                };
                let path = form_jcr_path(&cfg);
                match crate::aem_client::fetch_dor_pdf(&conn, &path).await {
                    Ok(pdf) => match render_pdf_first_page(&pdf) {
                        Ok(jpeg) => ToolReply::Image {
                            media_type: "image/jpeg",
                            b64: base64_encode(&jpeg),
                        },
                        Err(e) => ToolReply::Error(e),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }

            // §7 references
            "list_reference_forms" => {
                let profile = self.profile.clone().unwrap_or_default();
                let list: Vec<_> = crate::references::list_references(&profile)
                    .into_iter()
                    .map(|r| serde_json::json!({"ref_id": r.ref_id, "label": r.label, "description": r.description, "pdf_count": r.pdf_count, "files": r.files}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
            }
            "search_references" => {
                let profile = self.profile.clone().unwrap_or_default();
                let query = input["query"].as_str().unwrap_or_default();
                let regex = input["regex"].as_bool().unwrap_or(false);
                let hits: Vec<_> = crate::references::grep_references(&profile, query, regex)
                    .into_iter()
                    .map(|h| serde_json::json!({"ref_id": h.ref_id, "label": h.label, "where": h.location, "snippet": h.snippet}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&hits).unwrap_or_default())
            }
            "read_reference_file" => {
                let ref_id = input["ref_id"].as_str().unwrap_or_default();
                let path = input["path"].as_str().unwrap_or_default();
                let offset = input["offset"].as_u64().unwrap_or(0) as usize;
                let limit = input["limit"].as_u64().unwrap_or(0) as usize;
                match crate::references::read_reference_file(ref_id, path, offset, limit) {
                    Ok(t) => ToolReply::Text(t),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_reference_package" => {
                let ref_id = input["ref_id"].as_str().unwrap_or_default();
                let files = crate::references::get_reference_package_files(ref_id);
                let paths: Vec<&String> = files.iter().map(|(p, _)| p).collect();
                ToolReply::Text(serde_json::to_string_pretty(&paths).unwrap_or_default())
            }
            "list_reference_docs" => {
                let profile = self.profile.clone().unwrap_or_default();
                let list: Vec<_> = crate::references::list_docs(&profile)
                    .into_iter()
                    .map(|d| serde_json::json!({"doc_id": d.doc_id, "label": d.label}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&list).unwrap_or_default())
            }
            "read_reference_doc" => {
                let doc_id = input["doc_id"].as_str().unwrap_or_default();
                let offset = input["offset"].as_u64().unwrap_or(0) as usize;
                let limit = input["limit"].as_u64().unwrap_or(0) as usize;
                match crate::references::read_doc(doc_id, offset, limit) {
                    Ok(t) => ToolReply::Text(t),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "grep_reference_docs" => {
                let profile = self.profile.clone().unwrap_or_default();
                let query = input["query"].as_str().unwrap_or_default();
                let regex = input["regex"].as_bool().unwrap_or(false);
                let hits: Vec<_> = crate::references::grep_docs(&profile, query, regex)
                    .into_iter()
                    .map(|(doc_id, label, snippet)| serde_json::json!({"doc_id": doc_id, "label": label, "snippet": snippet}))
                    .collect();
                ToolReply::Text(serde_json::to_string_pretty(&hits).unwrap_or_default())
            }

            // §8 control
            "get_schema" => match input["kind"].as_str() {
                Some("aem") => {
                    ToolReply::Text(serde_json::to_string_pretty(&blueprint::aem_schema()).unwrap_or_default())
                }
                _ => ToolReply::Text(
                    serde_json::to_string_pretty(&blueprint::structured_schema()).unwrap_or_default(),
                ),
            },
            "get_profile_info" => match self.config() {
                Ok(c) => ToolReply::Text(format!(
                    "form_code: {}\nlanguages: {:?}\nmaster_language: {}\nform_path: {}\nform_dir: {}\nbind_to_xsd: {}\nuse_fragments: {}",
                    c.form_code, c.languages, c.master_language, c.form_path, c.form_dir, c.bind_to_xsd, c.use_fragments
                )),
                Err(e) => ToolReply::Error(e),
            },
            "finish" => {
                self.finished = true;
                ToolReply::Text("Finalized.".into())
            }

            other => ToolReply::Error(format!("Unknown tool: {other}")),
        }
    }
}

// ── Orchestration ────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = "\
You are an autonomous conversion agent operating the form-conversion engine via tools, \
replacing manual interaction. Goal: produce a correct AEM Adaptive Form from the uploaded \
PDF(s).\n\n\
Typical workflow (call tools as needed; each step is a separate call):\n\
1. Inspect the input: get_source_info, list_states, get_xfa (authoritative text/fields), \
get_plain_state_image / get_annotated_state_image, get_flattened_structure_for_state.\n\
2. Seed the structured tree: get_merged_structured, then set_structured (whole tree). Call \
get_schema('structured') for the exact JSON shape. Fix field types and grouping.\n\
3. Convert: convert_structured_to_aem, then inspect get_aem / get_aem_xml and refine with \
set_aem (get_schema('aem') for the shape).\n\
4. Package: build_aem_package, get_package_info, read_package_file to verify.\n\
5. If an AEM connection is configured: upload_to_aem, then fetch_aem_form_html / \
fetch_aem_dor_pdf to verify the deployed result.\n\
Consult reference forms and documentation when unsure: list_reference_forms, \
search_references, read_reference_file, get_reference_package, list_reference_docs, \
read_reference_doc, grep_reference_docs. You may run the engine on a reference's input by \
passing source={\"reference\":\"<ref_id>\"} to the §1 tools, and compare with its known-good \
package.\n\n\
Do not invent text content; take labels/options verbatim from the XFA. When done, call finish. \
Keep tool inputs minimal and valid JSON.";

/// Run the autonomous agent end-to-end, streaming activity into
/// `processing_state.agent_steps` and finalizing the result on completion.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent(
    pdfs: Vec<(String, Vec<u8>)>,
    profile: Option<String>,
    api_key: String,
    model: String,
    conn: Option<AemConnection>,
    session_label: String,
    mut processing_state: Signal<ProcessingState>,
    mut current_session: Signal<Option<String>>,
) {
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

    let mut agent = ConversionAgent::new(
        profile.clone(),
        pdfs.clone(),
        conn,
        structured_session.clone(),
    );
    let tools = agent.tools();

    let mut history: Vec<serde_json::Value> = Vec::new();
    history.push(serde_json::json!({"role": "user", "content": [{"type": "text", "text": SYSTEM_PROMPT}]}));

    for _ in 0..MAX_ITERATIONS {
        let turn = match anthropic_stream_turn(&mut history, &tools, &api_key, &model, AGENT_MAX_TOKENS).await {
            Ok(t) => t,
            Err(e) => {
                processing_state.write().error = Some(format!("Agent failed: {e}"));
                return;
            }
        };

        if !turn.text.trim().is_empty() {
            push_step(&mut processing_state, AgentStep {
                id: String::new(),
                kind: AgentStepKind::Thought,
                label: turn.text.trim().to_string(),
                detail: String::new(),
                status: AgentStepStatus::Done,
            });
        }

        if turn.stop_reason.as_deref() != Some("tool_use") || turn.tool_calls.is_empty() {
            break;
        }

        let mut results: Vec<(String, ToolReply)> = Vec::new();
        for tc in &turn.tool_calls {
            push_step(&mut processing_state, AgentStep {
                id: tc.id.clone(),
                kind: AgentStepKind::Tool,
                label: tc.name.clone(),
                detail: summarize_input(&tc.input),
                status: AgentStepStatus::Running,
            });
            let reply = agent.execute(&tc.name, &tc.input).await;
            let ok = !matches!(reply, ToolReply::Error(_));
            set_step_status(&mut processing_state, &tc.id, if ok { AgentStepStatus::Done } else { AgentStepStatus::Error });
            results.push((tc.id.clone(), reply));
        }
        history.push(tool_result_message(results));

        if agent.finished {
            break;
        }
    }

    finalize(&agent, &profile, structured_session, &mut processing_state, &mut current_session);
}

/// Build the final `ProcessingState` from the agent's working trees.
fn finalize(
    agent: &ConversionAgent,
    profile: &Option<String>,
    structured_session: String,
    processing_state: &mut Signal<ProcessingState>,
    current_session: &mut Signal<Option<String>>,
) {
    let envelope = DocumentEnvelope {
        context: agent.context.clone(),
        content: agent.structured.clone(),
        state_count: 1,
    };
    let merged_json = serde_json::to_string_pretty(&envelope).ok();
    let form_code = agent.aem_config.as_ref().map(|c| c.form_code.clone());

    let mut state = processing_state.write();
    state.step = ProcessingStep::Complete;
    state.ai_mode = true;
    state.envelope = Some(envelope);
    state.merged_json = merged_json;
    state.aem_package = agent.package.clone();
    state.form_code = form_code;
    state.agent_aem_session = agent.aem_session.clone();
    drop(state);

    let _ = profile;
    current_session.set(Some(structured_session));
}

// ── UI step helpers ──────────────────────────────────────────────────────────

fn push_step(processing_state: &mut Signal<ProcessingState>, step: AgentStep) {
    processing_state.write().agent_steps.push(step);
}

fn set_step_status(processing_state: &mut Signal<ProcessingState>, id: &str, status: AgentStepStatus) {
    let mut s = processing_state.write();
    if let Some(step) = s.agent_steps.iter_mut().rev().find(|s| s.id == id) {
        step.status = status;
    }
}

// ── Small helpers ────────────────────────────────────────────────────────────

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::prelude::BASE64_STANDARD.encode(bytes)
}

fn dedup(mut v: Vec<&str>) -> Vec<String> {
    v.sort();
    v.dedup();
    v.into_iter().map(String::from).collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
}

fn line_matches(line: &str, query: &str, regex: bool) -> bool {
    if regex {
        regex_lite::Regex::new(query).map(|re| re.is_match(line)).unwrap_or(false)
    } else {
        line.to_lowercase().contains(&query.to_lowercase())
    }
}

fn summarize_input(input: &serde_json::Value) -> String {
    let s = match input {
        serde_json::Value::Object(m) if m.is_empty() => String::new(),
        _ => input.to_string(),
    };
    truncate(&s, 120)
}

/// The form's JCR node path from its AEM config.
fn form_jcr_path(cfg: &AemConfig) -> String {
    join_form_path(&cfg.form_path, &cfg.form_dir)
}

fn join_form_path(form_path: &str, form_dir: &str) -> String {
    format!(
        "/content/forms/af/{}/{}",
        form_path.trim_matches('/'),
        form_dir.trim_matches('/')
    )
}

/// Render the first page of a PDF (the DoR) to JPEG via the engine.
fn render_pdf_first_page(pdf: &[u8]) -> Result<Vec<u8>, String> {
    let mut bp = blueprint::Blueprint::from_pdf_bytes(pdf).map_err(|e| format!("PDF parse: {e}"))?;
    let states = bp.states().map_err(|e| format!("states: {e}"))?;
    let state = states.iter().next().ok_or("no state in DoR PDF")?;
    let img = state.render_plain(RENDER_SCALE).map_err(|e| format!("render: {e}"))?;
    crate::pipeline::encode_rgba_to_jpeg(&img, 82).map_err(|e| format!("encode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_key_defaults_to_current() {
        assert_eq!(ConversionAgent::source_key(&serde_json::json!({})), "current");
        assert_eq!(
            ConversionAgent::source_key(&serde_json::json!({"source": {"reference": "abc"}})),
            "reference:abc"
        );
    }

    #[test]
    fn line_matches_literal_and_regex() {
        assert!(line_matches("Account Holder", "holder", false));
        assert!(!line_matches("Account Holder", "nope", false));
        assert!(line_matches("field_42", r"field_\d+", true));
        assert!(!line_matches("field_x", r"field_\d+", true));
        // invalid regex → no match (not a panic)
        assert!(!line_matches("anything", "(", true));
    }

    #[test]
    fn form_path_trims_slashes() {
        assert_eq!(join_form_path("/ubs/all/", "/AF_FORM/"), "/content/forms/af/ubs/all/AF_FORM");
        assert_eq!(join_form_path("ubs", "AF_FORM"), "/content/forms/af/ubs/AF_FORM");
    }

    #[test]
    fn summarize_input_truncates() {
        assert_eq!(summarize_input(&serde_json::json!({})), "");
        let long = serde_json::json!({"q": "x".repeat(500)});
        assert!(summarize_input(&long).chars().count() <= 121);
    }
}
