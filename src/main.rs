#[allow(unused_imports)]
use blueprint::xfa::{XfaNode, XfaNodeKind};
#[allow(unused_imports)]
use blueprint::{
    Blueprint, Context, Document, DocumentEnvelope, FieldLabelMap, Flattened, FlattenedNodeKind,
    GraphSelection, GraphState, HtmlConfig, MergeInput, RecursiveMerger, Selection, StructuredNode,
    build_field_label_map, document, flattened, structured, xfa,
};
use clap::{Parser, ValueEnum};
use log::{error, info};
use std::path::{Path, PathBuf};

/// CLI render mode, wrapping the library's [`blueprint::RenderMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RenderMode {
    Plain,
    Labelled,
    Annotated,
}

impl From<RenderMode> for blueprint::RenderMode {
    fn from(mode: RenderMode) -> Self {
        match mode {
            RenderMode::Plain => blueprint::RenderMode::Plain,
            RenderMode::Labelled => blueprint::RenderMode::Labelled,
            RenderMode::Annotated => blueprint::RenderMode::Annotated,
        }
    }
}

/// Check if PDF contains XFA and extract it
pub fn extract_xfa_from_pdf<P: AsRef<Path>>(
    path: P,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    blueprint::extract_xfa_from_pdf(path).map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })
}

/// Blueprint - XFA PDF document processor
#[derive(Parser, Debug)]
#[command(name = "blueprint")]
#[command(about = "Process and analyze XFA PDF documents", long_about = None)]
struct Args {
    /// Path(s) to the PDF document(s). Multiple files of the same document in
    /// different languages can be passed for multilingual output.
    #[arg(value_name = "DOCUMENT", required = true)]
    documents: Vec<PathBuf>,

    /// Render mode(s) for output images. Can be specified multiple times.
    /// Modes: plain, labelled, annotated
    #[arg(long = "render", value_enum)]
    render_modes: Vec<RenderMode>,

    /// Scale factor for rendering (default: 1.5)
    #[arg(short, long, default_value = "1.5")]
    scale: f32,

    /// Enable specific analysis modules (can be specified multiple times)
    /// Example: --module ubs --module custom
    #[arg(long = "module")]
    modules: Vec<String>,

    /// Export the structured form as JSON
    #[arg(long)]
    structured: bool,

    /// Export the form as a standalone HTML file with embedded CSS and JavaScript
    #[arg(long)]
    html: bool,

    /// Export the form as AEM Adaptive Forms JCR content XML.
    /// Pass a TOML profile file to configure customer-specific output settings.
    /// If no profile is given, legacy UBS defaults are used.
    #[arg(long)]
    aem: Option<PathBuf>,

    /// Export a GraphViz DOT file showing the interactive decision flow
    #[arg(long)]
    graphviz: bool,

    /// Dump the raw XFA XML content to a file and exit
    #[arg(long)]
    dump_xfa: bool,
}

/// Render a form state to disk using the specified render mode.
fn render_state(
    state: &blueprint::FormState,
    doc_name: &str,
    scale: f32,
    mode: RenderMode,
) -> Result<(), Box<dyn std::error::Error>> {
    let suffix = match mode {
        RenderMode::Plain => "plain",
        RenderMode::Labelled => "labelled",
        RenderMode::Annotated => "annotated",
    };

    let output_path =
        std::path::PathBuf::from(format!("{}_{}.{}.png", doc_name, state.label, suffix));

    let img = match mode {
        RenderMode::Plain => state.render_plain(scale)?,
        RenderMode::Labelled => state.render_labelled(scale)?,
        RenderMode::Annotated => state.render_annotated(scale)?,
    };

    img.save(&output_path)
        .map_err(|e| format!("Failed to save image: {}", e))?;

    Ok(())
}

/// Helper to print verbose analysis summary
#[allow(dead_code)]
fn print_analysis_summary(doc: &Document) {
    let text_blocks = doc.find_groups(|k| matches!(k, document::GroupKind::TextBlock));
    info!("Text blocks created: {}", text_blocks.len());

    let field_groups = doc.find_groups(|k| matches!(k, document::GroupKind::Field));
    info!("Field groups created: {}", field_groups.len());

    let date_fields = doc.find_groups(|k| matches!(k, document::GroupKind::DateField { .. }));
    info!("Date fields detected: {}", date_fields.len());

    let radio_buttons = doc.find_groups(|k| matches!(k, document::GroupKind::RadioButton { .. }));
    info!("Radio buttons detected: {}", radio_buttons.len());

    let checkboxes = doc.find_groups(|k| matches!(k, document::GroupKind::Checkbox { .. }));
    info!("Checkboxes detected: {}", checkboxes.len());

    let radio_button_groups =
        doc.find_groups(|k| matches!(k, document::GroupKind::RadioButtonGroup));
    info!("Radio button groups created: {}", radio_button_groups.len());

    let headings = doc.headings();
    info!("Headings detected: {}", headings.len());

    let labeled_fields = doc.labeled_fields();
    info!("Labeled fields found: {}", labeled_fields.len());

    // Print radio button summary
    if !radio_buttons.is_empty() {
        info!("Radio Buttons:");
        for (i, &rb_idx) in radio_buttons.iter().enumerate() {
            if let Some(group) = doc.get_group(rb_idx)
                && let document::GroupKind::RadioButton { field, label } = group.kind
            {
                // Get the field name
                let field_name = group
                    .children
                    .get(field)
                    .and_then(|&field_idx| {
                        let nodes = doc.collect_nodes(field_idx);
                        nodes.first().and_then(|n| {
                            if let FlattenedNodeKind::Field { name, .. } = &n.kind {
                                Some(name.clone())
                            } else {
                                None
                            }
                        })
                    })
                    .unwrap_or_else(|| "unknown".to_string());

                // Get the label text
                let label_text = group
                    .children
                    .get(label)
                    .map(|&label_idx| doc.get_text_content(label_idx))
                    .unwrap_or_else(String::new);

                let preview: String = label_text.chars().take(50).collect();
                let suffix = if label_text.chars().count() > 50 {
                    "..."
                } else {
                    ""
                };
                info!("  {}: [{}] {}{}", i + 1, field_name, preview, suffix);
            }
        }
    }

    // Print heading summary
    if !headings.is_empty() {
        info!("Headings:");
        for &h_idx in &headings {
            if let Some(group) = doc.get_group(h_idx)
                && let document::GroupKind::Heading { level } = group.kind
            {
                let text = doc.get_text_content(h_idx);
                let preview: String = text.chars().take(60).collect();
                let suffix = if text.chars().count() > 60 { "..." } else { "" };
                info!("  H{}: {}{}", level, preview, suffix);
            }
        }
    }

    // Print labeled field summary
    if !labeled_fields.is_empty() {
        info!("Labeled Fields (sample):");
        for (i, &lf_idx) in labeled_fields.iter().take(10).enumerate() {
            let label_text = doc.get_label_text(lf_idx).unwrap_or_default();
            let field_name = doc.get_field_name(lf_idx).unwrap_or_default();
            let preview: String = label_text.chars().take(40).collect();
            let suffix = if label_text.chars().count() > 40 {
                "..."
            } else {
                ""
            };
            info!("  {}: '{}{}' -> {}", i + 1, preview, suffix, field_name);
        }
        if labeled_fields.len() > 10 {
            info!("  ... and {} more", labeled_fields.len() - 10);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args = Args::parse();

    // Check if all documents exist
    for doc_path in &args.documents {
        if !doc_path.exists() {
            error!("Document not found: {}", doc_path.display());
            std::process::exit(1);
        }
    }

    // =========================================================================
    // Process each document (potentially different languages of the same form)
    // =========================================================================
    let mut all_envelopes: Vec<DocumentEnvelope> = Vec::new();
    let mut base_doc_name: Option<String> = None;

    for doc_path in &args.documents {
        info!("Processing document: {}", doc_path.display());

        let doc_name = doc_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");

        if base_doc_name.is_none() {
            base_doc_name = Some(doc_name.to_string());
        }

        // =====================================================================
        // PIPELINE STAGE 1: Extract XFA (optional dump) and build Blueprint
        // =====================================================================
        if args.dump_xfa {
            match extract_xfa_from_pdf(doc_path)? {
                Some(xfa_data) => {
                    let xml_path = std::path::PathBuf::from(format!("{}.xml", doc_name));
                    std::fs::write(&xml_path, &xfa_data)
                        .map_err(|e| format!("Failed to write XFA XML: {}", e))?;
                    info!("XFA dumped to {}", xml_path.display());
                }
                None => {
                    info!(
                        "{} is not an XFA PDF (no XFA data to dump)",
                        doc_path.display()
                    );
                }
            }
            continue;
        }

        let mut bp = Blueprint::from_pdf(doc_path)?;

        if bp.is_xfa() {
            info!("XFA data extracted");
            info!("XFA structure parsed");
        } else {
            info!("AcroForm PDF parsed (non-XFA)");
        }

        // =====================================================================
        // PIPELINE STAGE 2: Extract language + build context
        // =====================================================================
        let language = bp.language().to_string();
        info!("Detected language: {}", language);

        let context = bp.context();

        if !args.modules.is_empty() {
            info!("Enabled modules: {:?}", args.modules);
        }

        // =====================================================================
        // PIPELINE STAGE 3: Run exhaustive exploration (no I/O inside lib)
        // =====================================================================
        let need_structured = args.structured
            || args.html
            || args.aem.is_some()
            || args.graphviz
            || args.documents.len() > 1;
        let generate_html = if args.documents.len() > 1 {
            false
        } else {
            args.html
        };

        if bp.is_xfa() {
            info!("Exhaustive mode: recursively discovering all form states...");
        } else {
            info!("AcroForm mode: extracting text and form fields...");
        }
        if !args.render_modes.is_empty() {
            info!("  Render modes: {:?}", args.render_modes);
        }
        if args.structured {
            info!("  Structured JSON: enabled");
        }
        if args.html {
            info!("  HTML output: enabled");
        }
        if args.aem.is_some() {
            info!("  AEM package output: enabled");
        }
        if args.graphviz {
            info!("  GraphViz decision-flow: enabled");
        }

        info!("  Pass 1: Collecting all form states...");

        let form_states = bp.states()?;

        info!("    Found {} unique states", form_states.len());
        info!("  Pass 2: Analyzing and generating outputs...");

        let mut structured_outputs: Vec<(Vec<Selection>, Vec<StructuredNode>)> = Vec::new();
        let mut graph_states: Vec<GraphState> = Vec::new();
        let mut graph_field_labels: Option<FieldLabelMap> = None;
        let mut state_error: Option<Box<dyn std::error::Error>> = None;

        for state in form_states.iter() {
            if state_error.is_some() {
                break;
            }

            let mut outputs: Vec<String> = Vec::new();

            for mode in &args.render_modes {
                if let Err(err) = render_state(&state, doc_name, args.scale, *mode) {
                    state_error = Some(err);
                    break;
                }

                let suffix = match mode {
                    RenderMode::Plain => "plain",
                    RenderMode::Labelled => "labelled",
                    RenderMode::Annotated => "annotated",
                };
                outputs.push(format!("{}_{}.{}.png", doc_name, state.label, suffix));
            }

            // For graphviz, collect graph states from each form state.
            if args.graphviz {
                let graph_sels: Vec<GraphSelection> =
                    state.selections.iter().map(GraphSelection::from).collect();
                graph_states.push(GraphState {
                    selections: graph_sels,
                    label: state.label.clone(),
                });
            }

            if need_structured {
                let envelope = state.structured(context.clone());

                if args.structured {
                    let json = match serde_json::to_string_pretty(&envelope) {
                        Ok(val) => val,
                        Err(err) => {
                            state_error = Some(Box::new(err));
                            break;
                        }
                    };

                    let json_path =
                        std::path::PathBuf::from(format!("{}_{}.json", doc_name, state.label));
                    if let Err(err) = std::fs::write(&json_path, json) {
                        state_error = Some(Box::new(err));
                        break;
                    }
                    outputs.push(json_path.display().to_string());
                }

                // Aggregate field labels from every state so that fields only
                // visible in certain states (e.g. nested radio buttons) are
                // captured.
                if args.graphviz {
                    let state_labels = build_field_label_map(&envelope.content);
                    match graph_field_labels {
                        Some(ref mut existing) => existing.merge_from(&state_labels),
                        None => graph_field_labels = Some(state_labels),
                    }
                }

                structured_outputs.push((state.selections.clone(), envelope.content));
            }

            if !outputs.is_empty() {
                info!(
                    "    Generated: {} (selections: {:?})",
                    outputs.join(", "),
                    state
                        .selections
                        .iter()
                        .map(|sel| sel.som_path.as_str())
                        .collect::<Vec<_>>()
                );
            }
        }

        if let Some(err) = state_error {
            return Err(err);
        }

        // Merge structured outputs for this document
        if need_structured && !structured_outputs.is_empty() {
            let merge_inputs: Vec<MergeInput> = structured_outputs
                .into_iter()
                .map(|(selections, nodes)| MergeInput::new(selections, nodes))
                .collect();

            let merger = RecursiveMerger::new(merge_inputs);
            let merged = merger.merge();

            let merged_envelope = DocumentEnvelope {
                context: context.clone(),
                content: merged,
            };

            if args.structured {
                let json = serde_json::to_string_pretty(&merged_envelope)
                    .map_err(|e| format!("Failed to serialize merged structured form: {}", e))?;

                let json_path = std::path::PathBuf::from(format!("{}_merged.json", doc_name));
                std::fs::write(&json_path, json)
                    .map_err(|e| format!("Failed to write merged JSON file: {}", e))?;

                info!("    Merged output: {}", json_path.display());
            }

            if generate_html {
                let html_config = HtmlConfig::default();
                let html_output = blueprint::to_html(&merged_envelope.content, &html_config);

                let html_path = std::path::PathBuf::from(format!("{}_merged.html", doc_name));
                std::fs::write(&html_path, html_output)
                    .map_err(|e| format!("Failed to write merged HTML file: {}", e))?;

                info!("    Merged HTML: {}", html_path.display());
            }

            if args.aem.is_some() && args.documents.len() <= 1 {
                if bp.is_xfa() {
                    let aem_config =
                        load_aem_config(args.aem.as_deref(), &merged_envelope.context)?;
                    let aem_output =
                        blueprint::to_aem_package(&merged_envelope.content, &aem_config);

                    let aem_path = std::path::PathBuf::from(format!("{}_merged.zip", doc_name));
                    std::fs::write(&aem_path, aem_output)
                        .map_err(|e| format!("Failed to write AEM package: {}", e))?;

                    info!("    Merged AEM package: {}", aem_path.display());
                } else {
                    info!("    AEM export skipped (requires XFA variables)");
                }
            }

            if args.documents.len() > 1 {
                all_envelopes.push(merged_envelope);
            }
        }

        // Generate GraphViz decision-flow DOT file
        if args.graphviz && !graph_states.is_empty() {
            let field_labels = graph_field_labels.unwrap_or_default();
            let dot_output = blueprint::generate_dot(&graph_states, &field_labels);

            let dot_path = std::path::PathBuf::from(format!("{}_flow.dot", doc_name));
            std::fs::write(&dot_path, dot_output)
                .map_err(|e| format!("Failed to write GraphViz DOT file: {}", e))?;

            info!("    Decision flow: {}", dot_path.display());
        }
    }

    // =========================================================================
    // PIPELINE STAGE 4: Merge translations (if multiple documents)
    // =========================================================================
    if args.documents.len() > 1 && !all_envelopes.is_empty() {
        let doc_name = base_doc_name.as_deref().unwrap_or("document");
        // Strip language suffix from base doc name for the merged output
        let merged_name = strip_language_suffix(doc_name);

        info!("Merging {} language variants...", all_envelopes.len());

        let merged_envelope = blueprint::merge_translations(all_envelopes)
            .map_err(|e| format!("Translation merge failed: {}", e))?;

        // Write merged multilingual JSON
        if args.structured {
            let json = serde_json::to_string_pretty(&merged_envelope)
                .map_err(|e| format!("Failed to serialize multilingual form: {}", e))?;

            let json_path = std::path::PathBuf::from(format!("{}_multilingual.json", merged_name));
            std::fs::write(&json_path, json)
                .map_err(|e| format!("Failed to write multilingual JSON: {}", e))?;

            info!("Multilingual JSON: {}", json_path.display());
        }

        // Generate merged multilingual HTML
        if args.html {
            let html_config = HtmlConfig::default();
            let html_output = blueprint::to_html(&merged_envelope.content, &html_config);

            let html_path = std::path::PathBuf::from(format!("{}_multilingual.html", merged_name));
            std::fs::write(&html_path, html_output)
                .map_err(|e| format!("Failed to write multilingual HTML: {}", e))?;

            info!("Multilingual HTML: {}", html_path.display());
        }

        // Generate merged multilingual AEM package
        if args.aem.is_some() {
            let aem_config = load_aem_config(args.aem.as_deref(), &merged_envelope.context)?;
            let aem_output = blueprint::to_aem_package(&merged_envelope.content, &aem_config);

            let aem_path = std::path::PathBuf::from(format!("{}_multilingual.zip", merged_name));
            std::fs::write(&aem_path, aem_output)
                .map_err(|e| format!("Failed to write multilingual AEM package: {}", e))?;

            info!("Multilingual AEM package: {}", aem_path.display());
        }
    }

    Ok(())
}

/// Strip language suffixes like "_DE", "_EN", etc. from a document name
/// to get a base name for merged output.
fn strip_language_suffix(name: &str) -> &str {
    let suffixes = [
        "_DE", "_EN", "_FR", "_IT", "_ES", "_de", "_en", "_fr", "_it", "_es",
    ];
    for suffix in &suffixes {
        if let Some(stripped) = name.strip_suffix(suffix) {
            return stripped;
        }
    }
    name
}

/// Load AEM config: if a profile path is given, read and parse the TOML
/// to create an `AemConfig` via `from_profile()`. Otherwise fall back to
/// the legacy `AemConfig::new()` constructor.
fn load_aem_config(
    profile_path: Option<&Path>,
    ctx: &blueprint::Context,
) -> Result<blueprint::AemConfig, Box<dyn std::error::Error>> {
    match profile_path {
        Some(path) => {
            let toml_str = std::fs::read_to_string(path)
                .map_err(|e| format!("Failed to read AEM profile '{}': {}", path.display(), e))?;
            let profile: blueprint::AemProfile = toml::from_str(&toml_str)
                .map_err(|e| format!("Failed to parse AEM profile '{}': {}", path.display(), e))?;
            let config = blueprint::AemConfig::from_profile(&profile, ctx)?;
            Ok(config)
        }
        None => {
            let config = blueprint::AemConfig::new(ctx)?;
            Ok(config)
        }
    }
}
