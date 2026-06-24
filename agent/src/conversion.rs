//! The conversion agent's engine surface — the tool catalog and the executor
//! that drives the form-conversion engine.
//!
//! The agent extracts from the source PDF, builds and edits a **structured**
//! node tree, converts to an **AEM** node tree, edits that, packages it,
//! optionally uploads to AEM and verifies, and can consult reference forms /
//! documentation. Every tree change is snapshotted into an edit-history session
//! ([`crate::db`]) so a UI can review the full history.
//!
//! Tree mutations use a **whole-tree replace** model: the caller reads a tree
//! (`get_*`) and writes the whole tree back (`set_*`); each write is versioned.
//!
//! This type holds no LLM and no UI state: an external loop streams turns,
//! calls [`ConversionAgent::tools`] / [`ConversionAgent::execute`], and surfaces
//! the results. Network tools hit the engine's AEM client.

use std::collections::HashMap;

use blueprint::{AemConfig, AemConnection, AemNode, Context, DocumentEnvelope, StructuredNode};

/// Render scale for on-demand page images.
const RENDER_SCALE: f32 = 1.5;

/// The workflow guidance that teaches a driving model how to operate the
/// conversion tools. Shared by every consumer so the app's autonomous loop and
/// the standalone MCP server present one source of truth: the app injects it as
/// the agent's opening message, and the MCP server advertises it as its server
/// `instructions`. Consumer-specific bits (e.g. the MCP-only `start_conversion`
/// / `write_package` bootstrap) are appended by the consumer.
pub const SYSTEM_PROMPT: &str = "\
You are an autonomous conversion agent operating the form-conversion engine via tools, \
replacing manual interaction. Goal: produce a correct AEM Adaptive Form from the uploaded \
PDF(s).\n\n\
Typical workflow (call tools as needed; each step is a separate call):\n\
1. Inspect the input: get_source_info, get_profile_info (form_code, languages, JCR paths, \
binding flags), list_states, explore_states, get_xfa (authoritative text/fields), search_xfa \
(find specific fields/labels), get_plain_state_image / get_annotated_state_image, \
get_flattened_structure_for_state. A form is multilingual whenever get_source_info (or the \
merged structured content) lists more than one language — trust that over get_profile_info if \
they disagree. For a multilingual form the translations ride along in the merged structured \
content and are bundled into the package automatically: you MUST carry every one of those \
languages through to the final tree — don't invent translations, and never drop a language the \
source contains.\n\
2. Find precedents (do this before building): work through the input section by section. For \
EACH section, do NOT search by form name or a single keyword — instead write a short \
natural-language DESCRIPTION of that section (its purpose, the kinds of fields it has and how \
they're grouped) and pass that description to search_references, which matches it semantically \
against the reference forms' descriptions. Use grep_references only when you need a specific \
string (a field name, label, or AEM resource type) verbatim. Also consult grep_reference_docs / \
list_reference_forms. Different sections will often match different reference forms; find the \
closest precedent for each so every section is built as accurately as possible. Study how those \
known-good forms were built: inspect their package XML \
with get_reference_package / read_reference_file, and optionally run the engine on a \
reference's input by passing source={\"reference\":\"<ref_id>\"} to the step-1 inspection tools \
to compare against its known-good package. Build the structured and AEM trees to match the \
references' structure and patterns rather than inventing your own.\n\
3. Seed the structured tree: ALWAYS start from get_merged_structured, then set_structured (whole \
tree); re-read the working tree with get_structured. Call get_schema('structured') for the exact \
JSON shape. The merged tree carries every language present in the source (each translatable label/ \
option holds all languages) — preserve ALL of them through set_structured and every later edit; \
never reduce the tree to a single language. The seeded tree is a best-effort heuristic guess and \
is NOT guaranteed accurate — review it against the XFA (get_xfa / search_xfa) and the reference \
forms, and fix field types, labels, options and grouping before converting. On a refinement run, \
if the working tree is missing a language that get_merged_structured still has, re-seed from the \
merged tree to restore it before applying your changes.\n\
4. Convert: convert_structured_to_aem, then inspect get_aem / get_aem_content_xml and refine \
with set_aem (get_schema('aem') for the shape).\n\
4b. You can also hand-edit the final JCR content XML directly with structure-aware tools — \
useful for tweaks the trees can't express. First map it with get_aem_xml_outline (node paths + \
key attributes) and inspect a node with get_aem_xml_node; then edit by path with \
set_aem_xml_attribute / remove_aem_xml_attribute (attribute values are taken verbatim, so pass \
JCR-typed values like {Boolean}true), remove_aem_xml_node, replace_aem_xml_node and \
insert_aem_xml_node. Nodes are addressed by a /-separated path of element names from the root \
(e.g. jcr:root/guideContainer/panel_<uuid>/textbox_<uuid>); add a 1-based index like default[2] \
only when sibling names repeat. Every edit is validated (rejected if it would produce malformed \
XML) and versioned. Last-resort tuning: prefer fixing the structured or AEM tree, because XML \
edits are discarded the moment you re-run set_structured / convert_structured_to_aem / set_aem.\n\
5. Package & validate: build_aem_package, then ALWAYS run validate_aem_package — it checks the \
required package structure and validates the form and DAM content XML against the AEM contract. \
If it reports problems, fix them (structured/AEM tree, or the content XML directly) and re-run \
the downstream steps; never upload or export an invalid package. Use get_package_info / \
read_package_file to inspect.\n\
6. Review: once the package validates, verify the result end to end. (a) Call review_output to \
compare the input against the converted AEM tree — it lists input text and elements missing from \
the output plus a coverage score. For EVERY miss, either fix it (edit the structured/AEM tree and \
re-run the downstream steps) or satisfy yourself it was an intentional drop; spot-check other \
languages with search_xfa, since review_output only compares the master language. Pay particular \
attention to input fields: every fillable field in the source (text boxes, numeric boxes, \
dates, dropdowns, checkboxes, radio/choice groups, signatures, …) MUST have a corresponding \
field in the output. review_output reports the input vs. output field counts — any field-count \
mismatch, or any individual input field that has no counterpart in the output, must be \
investigated and resolved (never silently dropped), since a lost input field means data the form \
can no longer capture. (b) If an AEM \
connection is configured, upload_to_aem, then fetch_aem_form_html / fetch_aem_dor_pdf to verify \
the deployed result. Do not finish with unexplained misses.\n\
Pipeline & invalidation: structured tree -> AEM tree -> content XML -> package. Edits cascade \
downward: editing the structured tree resets the AEM tree, content XML and package; updating \
the AEM tree invalidates the content XML and package; editing the content XML invalidates the \
package. After any edit, re-run the downstream steps (including validate_aem_package).\n\
Consult reference documentation when unsure: list_reference_docs, read_reference_doc, \
grep_reference_docs.\n\n\
Never invent text content: take all labels/options/help text verbatim from the XFA, and never \
write copy of your own. Likewise, the final form must contain EVERY language present in the \
source (get_source_info / the merged structured content list them) and ONLY those: never drop a \
language the source contains, and never invent a translation for a language it does not. When \
done, call finish. Keep tool inputs minimal and valid JSON.";

/// The result of executing one tool call, to be returned to the model as a
/// `tool_result` content block.
pub enum ToolReply {
    /// A textual result (JSON, plain text, …).
    Text(String),
    /// An image result (base64 + media type), e.g. a rendered page.
    Image {
        media_type: &'static str,
        b64: String,
    },
    /// The tool failed; the message is surfaced to the model as an error result.
    Error(String),
}

/// Settings key under which the desktop app persists its serialized settings
/// blob in the shared `history.db` (see `app`'s `AppSettings`).
const APP_SETTINGS_KEY: &str = "app";

/// Build an AEM connection from the app settings stored in the shared
/// `history.db`, so a conversion driven headlessly (e.g. over MCP) can
/// upload/verify against the same instance the desktop app is configured for.
///
/// Reads the `aem_host` / `aem_username` / `aem_password` fields out of the
/// settings blob — mirroring `AppSettings::aem_connection` — and returns `None`
/// when no settings are stored or host/username are blank.
pub fn aem_connection_from_settings() -> Option<AemConnection> {
    let json = crate::db::get_setting(APP_SETTINGS_KEY)?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let host = v.get("aem_host").and_then(|h| h.as_str()).unwrap_or_default().trim();
    let username = v.get("aem_username").and_then(|u| u.as_str()).unwrap_or_default().trim();
    if host.is_empty() || username.is_empty() {
        return None;
    }
    let password = v.get("aem_password").and_then(|p| p.as_str()).unwrap_or_default();
    Some(AemConnection {
        host: host.trim_end_matches('/').to_string(),
        username: username.to_string(),
        password: password.to_string(),
    })
}

/// Validate FileVault package bytes (session-agnostic).
///
/// Runs the same checks as the `validate_aem_package` tool: required FileVault
/// structure, form `.content.xml` (`cq:Page`) validation, and DAM
/// `.content.xml` (`dam:Asset`) validation. Returns `Ok(success message)` when
/// the package is valid, or `Err(problem report)` listing every violation.
pub fn validate_package_bytes(pkg: &[u8]) -> Result<String, String> {
    let files = crate::references::unzip_package(pkg)
        .map_err(|e| format!("Could not read package: {e}"))?;

    let mut problems: Vec<String> = Vec::new();

    // 1. Required FileVault package structure.
    const REQUIRED: &[&str] = &[
        "META-INF/MANIFEST.MF",
        "META-INF/vault/config.xml",
        "META-INF/vault/nodetypes.cnd",
        "META-INF/vault/filter.xml",
        "META-INF/vault/properties.xml",
        "META-INF/vault/definition/.content.xml",
        "jcr_root/.content.xml",
        "jcr_root/content/.content.xml",
        "jcr_root/content/forms/.content.xml",
        "jcr_root/content/forms/af/.content.xml",
        "jcr_root/content/dam/.content.xml",
        "jcr_root/content/dam/formsanddocuments/.content.xml",
    ];
    for path in REQUIRED {
        if !files.iter().any(|(p, _)| p == path) {
            problems.push(format!("missing required package entry: {path}"));
        }
    }

    // 2. Validate the form content XML (the cq:Page under forms/af).
    let form_xml = files.iter().find(|(p, c)| {
        p.starts_with("jcr_root/content/forms/af/")
            && p.ends_with("/.content.xml")
            && c.contains("\"cq:Page\"")
    });
    match form_xml {
        Some((path, xml)) => {
            if let Err(violations) = blueprint::validate_aem_form_xml(xml) {
                problems.push(format!(
                    "form {path} failed {} validation check(s):\n    - {}",
                    violations.len(),
                    violations.join("\n    - ")
                ));
            }
        }
        None => problems.push(
            "no form .content.xml (jcr:primaryType cq:Page) found under \
             jcr_root/content/forms/af/"
                .into(),
        ),
    }

    // 3. Validate the DAM content XML (the dam:Asset).
    let dam_xml = files.iter().find(|(p, c)| {
        p.starts_with("jcr_root/content/dam/formsanddocuments/")
            && p.ends_with("/.content.xml")
            && c.contains("\"dam:Asset\"")
    });
    match dam_xml {
        Some((path, xml)) => {
            if let Err(violations) = blueprint::validate_aem_dam_xml(xml) {
                problems.push(format!(
                    "DAM {path} failed {} validation check(s):\n    - {}",
                    violations.len(),
                    violations.join("\n    - ")
                ));
            }
        }
        None => problems.push(
            "no DAM .content.xml (jcr:primaryType dam:Asset) found under \
             jcr_root/content/dam/formsanddocuments/"
                .into(),
        ),
    }

    if problems.is_empty() {
        Ok(format!(
            "✓ Package valid: {} entries; required FileVault structure present; \
             form and DAM content XML pass AEM validation.",
            files.len()
        ))
    } else {
        Err(format!(
            "Package validation found {} problem(s):\n- {}",
            problems.len(),
            problems.join("\n- ")
        ))
    }
}

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
    /// The stored, hand-editable `.content.xml`. `None` = not materialized;
    /// materialized lazily from the AEM tree on first read/edit and used
    /// verbatim for the package build until something upstream invalidates it.
    aem_xml: Option<String>,
    package: Option<Vec<u8>>,

    structured_session: String,
    aem_session: Option<String>,

    /// Set once the package has been uploaded + installed on AEM.
    aem_uploaded: bool,
    /// JCR path of the uploaded form on AEM (for the "done" screen).
    aem_form_path: Option<String>,

    /// Sentence-embedding model backing semantic `search_references`. Loaded
    /// lazily on first use (~200ms) and reused for the rest of the run.
    matcher: Option<blueprint::semantic::SemanticMatcher>,

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
            .find_map(|(_, b)| {
                blueprint::Blueprint::from_pdf_bytes(b)
                    .ok()
                    .map(|bp| bp.context())
            })
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
            aem_xml: None,
            package: None,
            structured_session,
            aem_session: None,
            aem_uploaded: false,
            aem_form_path: None,
            matcher: None,
            finished: false,
        }
    }

    /// Lazily load (and cache) the sentence-embedding model used by semantic
    /// `search_references`.
    fn matcher(&mut self) -> Result<&blueprint::semantic::SemanticMatcher, String> {
        if self.matcher.is_none() {
            self.matcher =
                Some(blueprint::semantic::SemanticMatcher::new().map_err(|e| e.to_string())?);
        }
        Ok(self.matcher.as_ref().unwrap())
    }

    /// Seed the working structured tree (used when resuming a session to apply
    /// user feedback to a prior result).
    pub fn seed_structured(&mut self, nodes: Vec<StructuredNode>) {
        self.structured = nodes;
    }

    // ── Public accessors (for the driving loop's result finalization) ─────────

    /// `true` once the agent has called the `finish` tool.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The detected document context (language, …).
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// The current working structured tree.
    pub fn structured(&self) -> &[StructuredNode] {
        &self.structured
    }

    /// The most recently built AEM package (ZIP), if any.
    pub fn package(&self) -> Option<Vec<u8>> {
        self.package.clone()
    }

    /// The resolved form code, if the AEM config has been loaded.
    pub fn form_code(&self) -> Option<String> {
        self.aem_config.as_ref().map(|c| c.form_code.clone())
    }

    /// The derived AEM edit-history session id, if any AEM snapshot was taken.
    pub fn aem_session(&self) -> Option<String> {
        self.aem_session.clone()
    }

    /// Whether the package has been uploaded + installed on AEM.
    pub fn aem_uploaded(&self) -> bool {
        self.aem_uploaded
    }

    /// The JCR path of the uploaded form, once uploaded.
    pub fn aem_form_path(&self) -> Option<String> {
        self.aem_form_path.clone()
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
        let cfg = self.aem_config.clone().unwrap();
        // Reflect the languages actually present in the document so
        // get_profile_info and the package builder never misreport a
        // multilingual form as en-only. `resolve_aem_languages` only overrides
        // when it detects ≥1 language, so monolingual flows keep the default.
        // Resolved per-call (not cached) because set_structured mutates
        // self.structured without touching self.aem_config. Prefer the working
        // tree once seeded; otherwise fall back to the merged source extraction
        // so the languages are reported even before the tree is seeded.
        if !self.structured.is_empty() {
            Ok(blueprint::resolve_aem_languages(&self.structured, &cfg))
        } else if let Ok(ex) = self.extractor(&serde_json::Value::Null) {
            Ok(blueprint::resolve_aem_languages(&ex.merged, &cfg))
        } else {
            Ok(cfg)
        }
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

    fn snapshot_aem_xml(&mut self, label: &str) {
        let Some(ref xml) = self.aem_xml else { return };
        let sid = format!("{}#aem-xml", self.structured_session);
        crate::db::insert_edit(&sid, label, xml);
    }

    /// Ensure `self.aem_xml` is materialized from the current AEM tree, returning
    /// the stored XML. Errors if there is no AEM tree yet.
    fn ensure_aem_xml(&mut self) -> Result<String, String> {
        if self.aem_xml.is_none() {
            let aem = self
                .aem
                .clone()
                .ok_or("No AEM tree yet; call convert_structured_to_aem.")?;
            let cfg = self.config()?;
            self.aem_xml = Some(blueprint::generate_aem_xml(&aem, &cfg));
        }
        Ok(self.aem_xml.clone().unwrap())
    }

    /// Apply a structure-aware edit to the materialized content XML: run `f` on
    /// the current XML, and on success store the result, invalidate the package,
    /// and snapshot it under `label`. The core editor rejects edits that would
    /// produce non-well-formed XML, so a returned `Err` leaves `self.aem_xml`
    /// untouched.
    fn apply_aem_xml_edit(
        &mut self,
        label: &str,
        f: impl FnOnce(&str) -> Result<String, String>,
    ) -> ToolReply {
        let xml = match self.ensure_aem_xml() {
            Ok(xml) => xml,
            Err(e) => return ToolReply::Error(e),
        };
        match f(&xml) {
            Ok(updated) => {
                let len = updated.len();
                self.aem_xml = Some(updated);
                // Editing the content XML invalidates the package.
                self.package = None;
                self.snapshot_aem_xml(label);
                ToolReply::Text(format!(
                    "OK — content XML is now {len} bytes (package invalidated)."
                ))
            }
            Err(e) => ToolReply::Error(e),
        }
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
            t(
                "get_source_info",
                "Info about the source PDFs (name, language, state count).",
                with_source(serde_json::json!({})),
                serde_json::json!([]),
            ),
            t(
                "explore_states",
                "Run exhaustive state discovery on the source; returns a count summary.",
                with_source(serde_json::json!({})),
                serde_json::json!([]),
            ),
            t(
                "list_states",
                "List discovered form states (label, pdf, selection count).",
                with_source(serde_json::json!({})),
                serde_json::json!([]),
            ),
            t(
                "get_xfa",
                "Return the source's authoritative XFA XML (all PDFs concatenated).",
                with_source(serde_json::json!({})),
                serde_json::json!([]),
            ),
            t(
                "search_xfa",
                "Regex/substring search within the source's XFA; returns matching snippets.",
                with_source(
                    serde_json::json!({"query": {"type":"string"}, "regex": {"type":"boolean"}}),
                ),
                serde_json::json!(["query"]),
            ),
            t(
                "get_plain_state_image",
                "Render a state's page image (plain).",
                with_source(state_label.clone()),
                serde_json::json!(["state_label"]),
            ),
            t(
                "get_annotated_state_image",
                "Render a state's page image with field-name overlays.",
                with_source(state_label.clone()),
                serde_json::json!(["state_label"]),
            ),
            t(
                "get_flattened_structure_for_state",
                "Engine structured tree for one state.",
                with_source(state_label.clone()),
                serde_json::json!(["state_label"]),
            ),
            t(
                "get_merged_structured",
                "The engine's full merged structured tree for the source (the usual seed for set_structured).",
                with_source(serde_json::json!({})),
                serde_json::json!([]),
            ),
            // §2 structured tree
            t(
                "get_structured",
                "Return the current working structured tree (JSON).",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "set_structured",
                "Replace the whole structured tree. `nodes` is a JSON array parseable as Vec<StructuredNode>. Resets the AEM tree, content XML and package (re-run convert_structured_to_aem after). Versioned.",
                serde_json::json!({"nodes": {"type":"array"}}),
                serde_json::json!(["nodes"]),
            ),
            // §3 conversion
            t(
                "convert_structured_to_aem",
                "Convert the current structured tree to the AEM tree (replaces it). Requires a non-empty structured tree (seed it with set_structured first). Invalidates the content XML and package. Versioned.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            // §4 aem tree
            t(
                "get_aem",
                "Return the current working AEM tree (JSON).",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "set_aem",
                "Replace the whole AEM tree. `root` is a JSON object parseable as AemNode. Invalidates the content XML and package. Versioned.",
                serde_json::json!({"root": {"type":"object"}}),
                serde_json::json!(["root"]),
            ),
            t(
                "get_aem_content_xml",
                "Return the whole AEM .content.xml (the final JCR XML). Materialized from the AEM tree on first access, then reflects any structure-aware XML edits. For a map of node paths use get_aem_xml_outline; to read one node use get_aem_xml_node.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            // Structure-aware editing of the final JCR content XML. Nodes are
            // addressed by a `/`-separated path of element names from the root,
            // e.g. `jcr:root/guideContainer/panel_<uuid>/textbox_<uuid>`; add a
            // 1-based index like `default[2]` only when sibling names repeat.
            // Every edit materializes the XML from the AEM tree first if needed,
            // is rejected if it would produce non-well-formed XML, invalidates the
            // package, and is versioned. These edits are expert-mode escape
            // hatches: the edited XML is used verbatim for the package while
            // everything else (XSD, translations, DAM) still derives from the AEM
            // tree, and converting/setting the structured or AEM tree discards them.
            t(
                "get_aem_xml_outline",
                "Map the content XML: one line per element with its full node path and key attributes (name, jcr:title, jcr:primaryType, guideNodeClass). Use it to find the path to edit before calling the set/remove/replace/insert tools.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "get_aem_xml_node",
                "Return just one element's subtree (start tag through end tag) from the content XML, addressed by its node `path`.",
                serde_json::json!({"path": {"type":"string"}}),
                serde_json::json!(["path"]),
            ),
            t(
                "set_aem_xml_attribute",
                "Set (or add) an attribute on the content-XML node at `path`. `value` is used verbatim — pass JCR-typed values such as `{Boolean}true` directly. Rejected if the result is not well-formed (e.g. an unescaped `&`).",
                serde_json::json!({"path": {"type":"string"}, "attribute": {"type":"string"}, "value": {"type":"string"}}),
                serde_json::json!(["path", "attribute", "value"]),
            ),
            t(
                "remove_aem_xml_attribute",
                "Remove an attribute from the content-XML node at `path`. Errors if the node has no such attribute.",
                serde_json::json!({"path": {"type":"string"}, "attribute": {"type":"string"}}),
                serde_json::json!(["path", "attribute"]),
            ),
            t(
                "remove_aem_xml_node",
                "Delete the content-XML node at `path` (its whole subtree). The document root cannot be removed.",
                serde_json::json!({"path": {"type":"string"}}),
                serde_json::json!(["path"]),
            ),
            t(
                "replace_aem_xml_node",
                "Replace the content-XML node at `path` (its whole subtree) with `xml`, a well-formed XML fragment. The document root cannot be replaced.",
                serde_json::json!({"path": {"type":"string"}, "xml": {"type":"string"}}),
                serde_json::json!(["path", "xml"]),
            ),
            t(
                "insert_aem_xml_node",
                "Insert `xml` (a well-formed fragment) as a child of the content-XML node at `parent_path`. `position` is \"first\", \"last\", {\"before\":\"<child_segment>\"} or {\"after\":\"<child_segment>\"}, where child_segment is a direct child's name (e.g. textbox_<uuid> or default[2]).",
                serde_json::json!({"parent_path": {"type":"string"}, "xml": {"type":"string"}, "position": {"type":["string","object"]}}),
                serde_json::json!(["parent_path", "xml", "position"]),
            ),
            // §5 output
            t(
                "build_aem_package",
                "Build the AEM FileVault package (ZIP) from the current AEM tree. Requires an AEM tree (run convert_structured_to_aem first). Stores it for upload/export.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "get_package_info",
                "Size and file list of the built package.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "read_package_file",
                "Read a file from the built package by path.",
                serde_json::json!({"path": {"type":"string"}}),
                serde_json::json!(["path"]),
            ),
            t(
                "validate_aem_package",
                "Validate the built package: checks the required FileVault structure (META-INF + jcr_root boilerplate) and validates the form and DAM .content.xml against the AEM contract (well-formedness, escaping, JCR/CQ/FD/Sling structure). Run after build_aem_package, before upload_to_aem.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "review_output",
                "Fidelity review: compare the input (the engine's merged structured parse) against the converted AEM tree and report input text/elements missing from the output, with a coverage score. Compares the master language only (spot-check other languages with search_xfa). Reads the AEM tree, so edits made only to the content XML are not reflected. Run after convert_structured_to_aem and before finish; investigate every miss (fix the tree, or confirm it was intentionally dropped) and re-run.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "generate_xsd",
                "Generate the XSD schema for the current structured tree.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "generate_html",
                "Generate an HTML preview of the current structured tree.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            // §6 deploy + verify
            t(
                "upload_to_aem",
                "Upload and install the built package on the configured AEM instance.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "fetch_aem_form_html",
                "Fetch the rendered Adaptive Form HTML from AEM (after upload) for verification.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "fetch_aem_dor_pdf",
                "Fetch the Document-of-Record PDF from AEM and view its first page.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            // §7 references
            t(
                "list_reference_forms",
                "List the profile's reference forms (hand-built, known-good worked examples). \
                 Consult references BEFORE building: they show the expected JCR structure, \
                 dictionary setup and DoR conventions for this profile's forms.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "search_references",
                "Semantic search for precedent forms by MEANING, not by name. The query must be a \
                 natural-language DESCRIPTION of the input you are building — the form's (or the \
                 current section's) purpose, the kinds of fields it contains and how they are \
                 grouped — NOT a form name or a single keyword. References are matched by embedding \
                 this description against each reference's stored description (a literal substring \
                 fallback over descriptions + package XML is folded in). Run this first (before \
                 building), section by section; each hit carries a ref_id to pass to \
                 get_reference_package / read_reference_file. Optional top_k caps hits per signal \
                 (default 3).",
                serde_json::json!({"query": {"type":"string"}, "top_k": {"type":"integer"}}),
                serde_json::json!(["query"]),
            ),
            t(
                "grep_references",
                "Literal/regex substring search over reference descriptions + AEM package XML — the \
                 grep counterpart to search_references. Use it to find a specific string (a field \
                 name, label, or AEM resource type) verbatim; use search_references when looking \
                 for a form that resembles your input by meaning.",
                serde_json::json!({"query": {"type":"string"}, "regex": {"type":"boolean"}}),
                serde_json::json!(["query"]),
            ),
            t(
                "read_reference_file",
                "Read a reference's description ('description') or a package file by path (get the \
                 path from get_reference_package). Use it to study how a known-good form was built \
                 and mirror its structure.",
                serde_json::json!({"ref_id": {"type":"string"}, "path": {"type":"string"}, "offset": {"type":"integer"}, "limit": {"type":"integer"}}),
                serde_json::json!(["ref_id", "path"]),
            ),
            t(
                "get_reference_package",
                "List the package files (known-good output) of a reference by its ref_id (from \
                 list_reference_forms / search_references), then read individual files with \
                 read_reference_file.",
                serde_json::json!({"ref_id": {"type":"string"}}),
                serde_json::json!(["ref_id"]),
            ),
            t(
                "list_reference_docs",
                "List the profile's reference documentation (.md/.txt).",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "read_reference_doc",
                "Read a reference documentation doc by id.",
                serde_json::json!({"doc_id": {"type":"string"}, "offset": {"type":"integer"}, "limit": {"type":"integer"}}),
                serde_json::json!(["doc_id"]),
            ),
            t(
                "grep_reference_docs",
                "Regex/substring search over reference documentation.",
                serde_json::json!({"query": {"type":"string"}, "regex": {"type":"boolean"}}),
                serde_json::json!(["query"]),
            ),
            // §8 control
            t(
                "get_schema",
                "Return the JSON schema for the 'structured' or 'aem' tree.",
                serde_json::json!({"kind": {"type":"string","enum":["structured","aem"]}}),
                serde_json::json!(["kind"]),
            ),
            t(
                "get_profile_info",
                "Profile/AEM config: form_code, languages, JCR paths, binding flags.",
                serde_json::json!({}),
                serde_json::json!([]),
            ),
            t(
                "finish",
                "Terminal step — call this once, last, after the package is built, validated and reviewed (review_output) — and uploaded if an AEM connection is configured — to persist the structured + AEM trees + package as the result and end the run.",
                serde_json::json!({"summary": {"type":"string"}}),
                serde_json::json!([]),
            ),
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
                let label = input["state_label"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
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
                                crate::image_encode::encode_rgba_to_jpeg(&i, 82)
                                    .map_err(|e| e.to_string())
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
                let label = input["state_label"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                match self.extractor(input) {
                    Ok(ex) => match ex.find(&label) {
                        Some(rec) => {
                            let env = rec.state.structured(rec.context.clone());
                            ToolReply::Text(
                                serde_json::to_string_pretty(&env.content).unwrap_or_default(),
                            )
                        }
                        None => ToolReply::Error(format!("Unknown state_label: {label:?}")),
                    },
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_merged_structured" => match self.extractor(input) {
                Ok(ex) => {
                    ToolReply::Text(serde_json::to_string_pretty(&ex.merged).unwrap_or_default())
                }
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
                        // A structured edit resets the AEM tree + everything downstream.
                        self.aem = None;
                        self.aem_xml = None;
                        self.package = None;
                        self.snapshot_structured("AI: set structured");
                        ToolReply::Text(format!(
                            "OK ({} top-level node(s)). AEM tree, content XML and package reset — re-run convert_structured_to_aem.",
                            self.structured.len()
                        ))
                    }
                    Err(e) => ToolReply::Error(format!("Invalid StructuredNode JSON: {e}")),
                }
            }

            // §3 conversion
            "convert_structured_to_aem" => {
                if self.structured.is_empty() {
                    return ToolReply::Error(
                        "Structured tree is empty; set_structured first.".into(),
                    );
                }
                let cfg = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                let aem = blueprint::convert_to_aem(&self.structured, &cfg);
                self.aem = Some(aem);
                // New AEM tree invalidates the content XML + package.
                self.aem_xml = None;
                self.package = None;
                self.snapshot_aem("AI: convert from structured");
                ToolReply::Text(
                    "OK — AEM tree generated from structured (content XML + package invalidated)."
                        .into(),
                )
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
                        // Updating the AEM tree invalidates the content XML + package.
                        self.aem_xml = None;
                        self.package = None;
                        self.snapshot_aem("AI: set AEM tree");
                        ToolReply::Text("OK (content XML + package invalidated).".into())
                    }
                    Err(e) => ToolReply::Error(format!("Invalid AemNode JSON: {e}")),
                }
            }
            "get_aem_content_xml" => match self.ensure_aem_xml() {
                Ok(xml) => ToolReply::Text(xml),
                Err(e) => ToolReply::Error(e),
            },
            "get_aem_xml_outline" => {
                let xml = match self.ensure_aem_xml() {
                    Ok(xml) => xml,
                    Err(e) => return ToolReply::Error(e),
                };
                match blueprint::outline_aem_xml(&xml) {
                    Ok(outline) => ToolReply::Text(outline),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "get_aem_xml_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                if path.is_empty() {
                    return ToolReply::Error("`path` must not be empty.".into());
                }
                let xml = match self.ensure_aem_xml() {
                    Ok(xml) => xml,
                    Err(e) => return ToolReply::Error(e),
                };
                match blueprint::read_aem_xml_node(&xml, &path) {
                    Ok(node) => ToolReply::Text(node),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "set_aem_xml_attribute" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let attr = input["attribute"].as_str().unwrap_or_default().to_string();
                let value = input["value"].as_str().unwrap_or_default().to_string();
                if path.is_empty() || attr.is_empty() {
                    return ToolReply::Error("`path` and `attribute` must not be empty.".into());
                }
                let label = format!("AI: set @{attr} on {path}");
                self.apply_aem_xml_edit(&label, |xml| {
                    blueprint::set_aem_xml_attribute(xml, &path, &attr, &value)
                })
            }
            "remove_aem_xml_attribute" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let attr = input["attribute"].as_str().unwrap_or_default().to_string();
                if path.is_empty() || attr.is_empty() {
                    return ToolReply::Error("`path` and `attribute` must not be empty.".into());
                }
                let label = format!("AI: remove @{attr} on {path}");
                self.apply_aem_xml_edit(&label, |xml| {
                    blueprint::remove_aem_xml_attribute(xml, &path, &attr)
                })
            }
            "remove_aem_xml_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                if path.is_empty() {
                    return ToolReply::Error("`path` must not be empty.".into());
                }
                let label = format!("AI: remove node {path}");
                self.apply_aem_xml_edit(&label, |xml| blueprint::remove_aem_xml_node(xml, &path))
            }
            "replace_aem_xml_node" => {
                let path = input["path"].as_str().unwrap_or_default().to_string();
                let frag = input["xml"].as_str().unwrap_or_default().to_string();
                if path.is_empty() || frag.is_empty() {
                    return ToolReply::Error("`path` and `xml` must not be empty.".into());
                }
                let label = format!("AI: replace node {path}");
                self.apply_aem_xml_edit(&label, |xml| {
                    blueprint::replace_aem_xml_node(xml, &path, &frag)
                })
            }
            "insert_aem_xml_node" => {
                let parent = input["parent_path"].as_str().unwrap_or_default().to_string();
                let frag = input["xml"].as_str().unwrap_or_default().to_string();
                if parent.is_empty() || frag.is_empty() {
                    return ToolReply::Error("`parent_path` and `xml` must not be empty.".into());
                }
                let position = match parse_insert_pos(&input["position"]) {
                    Ok(p) => p,
                    Err(e) => return ToolReply::Error(e),
                };
                let label = format!("AI: insert node into {parent}");
                self.apply_aem_xml_edit(&label, |xml| {
                    blueprint::insert_aem_xml_node(xml, &parent, &frag, position)
                })
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
                let translations = blueprint::aem_translations_from_content(
                    &self.structured,
                    &cfg.master_language,
                );
                let (pkg, note) = match self.aem_xml.clone() {
                    Some(xml) => (
                        blueprint::to_aem_package_from_node_with_xml(&aem, &cfg, translations, xml),
                        " (using edited content XML)",
                    ),
                    None => (
                        blueprint::to_aem_package_from_node_with_translations(
                            &aem,
                            &cfg,
                            translations,
                        ),
                        "",
                    ),
                };
                let size = pkg.len();
                self.package = Some(pkg);
                ToolReply::Text(format!("Built package ({size} bytes){note}."))
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
            "validate_aem_package" => {
                let Some(pkg) = self.package.clone() else {
                    return ToolReply::Error(
                        "No package built yet; call build_aem_package.".into(),
                    );
                };
                match validate_package_bytes(&pkg) {
                    Ok(msg) => ToolReply::Text(msg),
                    Err(e) => ToolReply::Error(e),
                }
            }
            "review_output" => {
                let Some(aem) = self.aem.clone() else {
                    return ToolReply::Error(
                        "No AEM tree yet; call convert_structured_to_aem first.".into(),
                    );
                };
                let merged = match self.extractor(&serde_json::Value::Null) {
                    Ok(ex) => ex.merged.clone(),
                    Err(e) => return ToolReply::Error(e),
                };
                let master = self
                    .config()
                    .map(|c| c.master_language)
                    .unwrap_or_else(|_| "en".into());
                let report = blueprint::review_output(&merged, &aem, &master);
                ToolReply::Text(serde_json::to_string_pretty(&report).unwrap_or_default())
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
                    return ToolReply::Error(
                        "No package built yet; call build_aem_package.".into(),
                    );
                };
                let cfg = match self.config() {
                    Ok(c) => c,
                    Err(e) => return ToolReply::Error(e),
                };
                match crate::aem_client::upload_and_install_package(&conn, pkg, &cfg.form_code)
                    .await
                {
                    Ok(()) => {
                        self.aem_uploaded = true;
                        self.aem_form_path = Some(form_jcr_path(&cfg));
                        ToolReply::Text("Uploaded and installed on AEM.".into())
                    }
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
                let query = input["query"].as_str().unwrap_or_default().to_string();
                if query.trim().is_empty() {
                    return ToolReply::Error(
                        "search_references requires a non-empty query — pass a description of the \
                         input form/section, not an empty string."
                            .into(),
                    );
                }
                let top_k = input["top_k"].as_u64().unwrap_or(3).max(1) as usize;
                let matcher = match self.matcher() {
                    Ok(m) => m,
                    Err(e) => return ToolReply::Error(e),
                };
                let hits: Vec<_> =
                    crate::references::search_references(&profile, &query, matcher, top_k)
                        .into_iter()
                        .map(|h| serde_json::json!({"ref_id": h.ref_id, "label": h.label, "where": h.location, "matched": h.matched, "score": h.score, "snippet": h.snippet}))
                        .collect();
                ToolReply::Text(serde_json::to_string_pretty(&hits).unwrap_or_default())
            }
            "grep_references" => {
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
                Some("aem") => ToolReply::Text(
                    serde_json::to_string_pretty(&blueprint::aem_schema()).unwrap_or_default(),
                ),
                _ => ToolReply::Text(
                    serde_json::to_string_pretty(&blueprint::structured_schema())
                        .unwrap_or_default(),
                ),
            },
            "get_profile_info" => match self.config() {
                Ok(c) => ToolReply::Text(format!(
                    "form_code: {}\nlanguages: {:?}\nmaster_language: {}\nform_path: {}\nform_dir: {}\nbind_to_xsd: {}\nuse_fragments: {}",
                    c.form_code,
                    c.languages,
                    c.master_language,
                    c.form_path,
                    c.form_dir,
                    c.bind_to_xsd,
                    c.use_fragments
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

/// Parse the `position` argument of `insert_aem_xml_node` into an [`InsertPos`].
/// Accepts the strings `"first"` / `"last"`, or an object
/// `{"before": "<child>"}` / `{"after": "<child>"}`.
fn parse_insert_pos(value: &serde_json::Value) -> Result<blueprint::InsertPos, String> {
    use blueprint::InsertPos;
    if let Some(s) = value.as_str() {
        return match s {
            "first" => Ok(InsertPos::First),
            "last" => Ok(InsertPos::Last),
            other => Err(format!(
                "invalid position '{other}'; use \"first\", \"last\", {{\"before\":...}} or {{\"after\":...}}"
            )),
        };
    }
    if let Some(s) = value.get("before").and_then(|v| v.as_str()) {
        return Ok(InsertPos::Before(s.to_string()));
    }
    if let Some(s) = value.get("after").and_then(|v| v.as_str()) {
        return Ok(InsertPos::After(s.to_string()));
    }
    Err("`position` must be \"first\", \"last\", {\"before\":\"<child>\"} or {\"after\":\"<child>\"}".into())
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
        regex_lite::Regex::new(query)
            .map(|re| re.is_match(line))
            .unwrap_or(false)
    } else {
        line.to_lowercase().contains(&query.to_lowercase())
    }
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
    let mut bp =
        blueprint::Blueprint::from_pdf_bytes(pdf).map_err(|e| format!("PDF parse: {e}"))?;
    let states = bp.states().map_err(|e| format!("states: {e}"))?;
    let state = states.iter().next().ok_or("no state in DoR PDF")?;
    let img = state
        .render_plain(RENDER_SCALE)
        .map_err(|e| format!("render: {e}"))?;
    crate::image_encode::encode_rgba_to_jpeg(&img, 82).map_err(|e| format!("encode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_key_defaults_to_current() {
        assert_eq!(
            ConversionAgent::source_key(&serde_json::json!({})),
            "current"
        );
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
    fn config_reflects_languages_in_seeded_structured_tree() {
        use blueprint::{InlineText, ParagraphNode, StructuredNode, TranslatedText};

        let mut agent = ConversionAgent::new(
            Some("ubs".into()),
            Vec::new(),
            None,
            "test-config-languages".into(),
        );
        // The ubs profile templates reference a couple of xfa vars; supply the
        // minimal context so load_aem_config succeeds without a real PDF.
        let mut vars = HashMap::new();
        vars.insert("formrange_code".to_string(), "TESTFORM".to_string());
        vars.insert("formrange_entity".to_string(), "TEST".to_string());
        agent.context = blueprint::Context::new("en".to_string(), vars);

        // With no content the config falls back to the profile default.
        let before = agent.config().expect("config loads for ubs profile");
        assert_eq!(before.languages, vec!["en".to_string()]);

        // Seed a bilingual (de + en) working tree.
        let mut content = TranslatedText::empty();
        content.insert("en", InlineText::plain("Hello"));
        content.insert("de", InlineText::plain("Hallo"));
        agent.seed_structured(vec![StructuredNode::Paragraph(ParagraphNode {
            content,
            som_path: None,
            source_name: None,
        })]);

        // config() must now reflect the languages present in the tree so
        // get_profile_info and the package builder treat the form as
        // multilingual instead of collapsing it to the en-only default.
        let after = agent.config().expect("config loads");
        assert!(after.languages.contains(&"en".to_string()));
        assert!(
            after.languages.contains(&"de".to_string()),
            "config.languages must include every language in the seeded tree, got {:?}",
            after.languages
        );
    }

    #[test]
    fn form_path_trims_slashes() {
        assert_eq!(
            join_form_path("/ubs/all/", "/AF_FORM/"),
            "/content/forms/af/ubs/all/AF_FORM"
        );
        assert_eq!(
            join_form_path("ubs", "AF_FORM"),
            "/content/forms/af/ubs/AF_FORM"
        );
    }
}
