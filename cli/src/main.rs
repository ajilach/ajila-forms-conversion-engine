use blueprint::{
    FieldLabelMap, GraphSelection, GraphState, HtmlConfig, HtmlCustomStyles, PipelineConfig,
    PipelineEvent, PipelineStep, build_field_label_map, generate_dot, run_pipeline,
};
use clap::{Parser, ValueEnum};
use log::info;
use std::path::{Path, PathBuf};

/// CLI render mode, wrapping the library's [`blueprint::RenderMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RenderMode {
    Plain,
    Labelled,
    Annotated,
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
    #[arg(long)]
    aem: bool,

    /// Path to a profile directory containing per-output configs.
    /// Expected layout: {profile}/aem/config.toml, {profile}/html/config.toml.
    #[arg(long)]
    profile: Option<PathBuf>,

    /// Export a GraphViz DOT file showing the interactive decision flow
    #[arg(long)]
    graphviz: bool,

    /// Dump the raw XFA XML content to a file and exit
    #[arg(long)]
    dump_xfa: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args = Args::parse();

    // Validate that all paths exist up-front.
    for doc_path in &args.documents {
        if !doc_path.exists() {
            eprintln!("Error: document not found: {}", doc_path.display());
            std::process::exit(1);
        }
    }

    // ─── --dump-xfa: extract raw XML and exit ────────────────────────────────
    if args.dump_xfa {
        for doc_path in &args.documents {
            let doc_name = doc_stem(doc_path);
            match blueprint::extract_xfa_from_pdf(doc_path)? {
                Some(xfa_data) => {
                    let xml_path = PathBuf::from(format!("{}.xml", doc_name));
                    std::fs::write(&xml_path, &xfa_data)
                        .map_err(|e| format!("Failed to write XFA XML: {}", e))?;
                    info!("XFA dumped to {}", xml_path.display());
                }
                None => info!(
                    "{} is not an XFA PDF (no XFA data to dump)",
                    doc_path.display()
                ),
            }
        }
        return Ok(());
    }

    if !args.modules.is_empty() {
        info!("Enabled modules: {:?}", args.modules);
    }

    // ─── Read all files into memory ──────────────────────────────────────────
    let files: Vec<(String, Vec<u8>)> = args
        .documents
        .iter()
        .map(|p| {
            let name = doc_stem(p).to_string();
            let bytes = std::fs::read(p)?;
            Ok((name, bytes))
        })
        .collect::<Result<_, std::io::Error>>()?;

    // Derive a base name for output files from the first document.
    let base_name = files.first().map(|(n, _)| n.as_str()).unwrap_or("document");

    // ─── Build pipeline config from CLI args ─────────────────────────────────
    let config = PipelineConfig {
        scale: args.scale,
        render_plain: args.render_modes.contains(&RenderMode::Plain),
        render_annotated: args.render_modes.contains(&RenderMode::Annotated),
        render_labelled: args.render_modes.contains(&RenderMode::Labelled),
    };

    // ─── Run the pipeline ────────────────────────────────────────────────────
    // The callback fires for every step change, states-found notification, and
    // individual render completion.  Renders are saved to disk immediately.
    let output = run_pipeline(&files, &config, |event| match event {
        PipelineEvent::StepChanged(step) => {
            let msg = match step {
                PipelineStep::Parsing => "Parsing PDF(s)...",
                PipelineStep::ExhaustiveSearching => "Discovering form states...",
                PipelineStep::Flattening => "Rendering plain/annotated images...",
                PipelineStep::Structuring => "Structuring form content...",
                PipelineStep::Merging => "Merging outputs...",
                PipelineStep::Complete => "Done.",
            };
            info!("{}", msg);
        }

        PipelineEvent::StatesFound { file, count } => {
            info!("  {}: {} unique states found", file, count);
        }

        PipelineEvent::PlainRender { label, image } => {
            let path = PathBuf::from(format!("{}_{}.plain.png", base_name, label));
            match image.save(&path) {
                Ok(()) => info!("  Saved: {}", path.display()),
                Err(e) => eprintln!("Warning: failed to save {}: {}", path.display(), e),
            }
        }

        PipelineEvent::AnnotatedRender { label, image } => {
            let path = PathBuf::from(format!("{}_{}.annotated.png", base_name, label));
            match image.save(&path) {
                Ok(()) => info!("  Saved: {}", path.display()),
                Err(e) => eprintln!("Warning: failed to save {}: {}", path.display(), e),
            }
        }

        PipelineEvent::LabelledRender { label, image } => {
            let path = PathBuf::from(format!("{}_{}.labelled.png", base_name, label));
            match image.save(&path) {
                Ok(()) => info!("  Saved: {}", path.display()),
                Err(e) => eprintln!("Warning: failed to save {}: {}", path.display(), e),
            }
        }

        PipelineEvent::Warning(msg) => eprintln!("Warning: {}", msg),
    })?;

    // ─── Post-pipeline: structured / HTML / AEM / GraphViz output ────────────
    let is_multilingual = args.documents.len() > 1;
    let merged_name = strip_language_suffix(base_name).to_string();
    let suffix = if is_multilingual { "multilingual" } else { "merged" };

    // Structured JSON
    if args.structured {
        let json = serde_json::to_string_pretty(&output.merged)
            .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
        let json_path = PathBuf::from(format!("{}_{}.json", merged_name, suffix));
        std::fs::write(&json_path, json)
            .map_err(|e| format!("Failed to write JSON: {}", e))?;
        info!("Structured JSON: {}", json_path.display());
    }

    // HTML
    if args.html {
        let custom_styles = load_html_config(args.profile.as_deref())?;
        let html_config = HtmlConfig {
            custom_styles,
            ..HtmlConfig::default()
        };
        let html = blueprint::to_html(&output.merged.content, &html_config);
        let html_path = PathBuf::from(format!("{}_{}.html", merged_name, suffix));
        std::fs::write(&html_path, html)
            .map_err(|e| format!("Failed to write HTML: {}", e))?;
        info!("HTML: {}", html_path.display());
    }

    // AEM package
    if args.aem {
        match load_aem_config(args.profile.as_deref(), &output.merged.context) {
            Ok(aem_config) => {
                let aem_zip = blueprint::to_aem_package(&output.merged.content, &aem_config);
                let aem_path = PathBuf::from(format!("{}_{}.zip", merged_name, suffix));
                std::fs::write(&aem_path, aem_zip)
                    .map_err(|e| format!("Failed to write AEM package: {}", e))?;
                info!("AEM package: {}", aem_path.display());
            }
            Err(e) => info!("AEM export skipped: {}", e),
        }
    }

    // GraphViz decision-flow DOT file
    if args.graphviz {
        let graph_states: Vec<GraphState> = output
            .state_labels
            .iter()
            .map(|(label, selections)| GraphState {
                selections: selections.iter().map(GraphSelection::from).collect(),
                label: label.clone(),
            })
            .collect();
        let field_labels: FieldLabelMap = build_field_label_map(&output.merged.content);
        let dot = generate_dot(&graph_states, &field_labels);
        let dot_path = PathBuf::from(format!("{}_flow.dot", merged_name));
        std::fs::write(&dot_path, dot)
            .map_err(|e| format!("Failed to write GraphViz DOT file: {}", e))?;
        info!("GraphViz DOT: {}", dot_path.display());
    }

    Ok(())
}

// ─── Filename helpers ─────────────────────────────────────────────────────────

/// Extract the file stem of a path as `&str`, falling back to `"document"`.
fn doc_stem(path: &Path) -> &str {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document")
}

/// Strip language suffixes like `_DE`, `_EN`, etc. from a document name
/// to get a base name suitable for merged output files.
fn strip_language_suffix(name: &str) -> &str {
    const SUFFIXES: &[&str] = &[
        "_DE", "_EN", "_FR", "_IT", "_ES", "_de", "_en", "_fr", "_it", "_es",
    ];
    for suffix in SUFFIXES {
        if let Some(stripped) = name.strip_suffix(suffix) {
            return stripped;
        }
    }
    name
}

// ─── Profile loading ──────────────────────────────────────────────────────────

/// Load AEM config from the `aem/` subdirectory of a profile.
///
/// The directory must contain a `config.toml` file and may contain `*.xml`
/// template files. Each `.xml` file's stem (e.g. `root` from `root.xml`)
/// becomes a key in `component_templates`.
fn load_aem_config(
    profile_path: Option<&Path>,
    ctx: &blueprint::Context,
) -> Result<blueprint::AemConfig, Box<dyn std::error::Error>> {
    let base = profile_path.ok_or("No profile directory specified (use --profile <dir>)")?;

    let dir = base.join("aem");
    if !dir.is_dir() {
        return Err(format!(
            "AEM profile directory '{}' not found inside profile '{}'",
            dir.display(),
            base.display()
        )
        .into());
    }

    // Read config.toml
    let config_path = dir.join("config.toml");
    let toml_str = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config.toml in '{}': {}", dir.display(), e))?;
    let profile: blueprint::AemProfile = toml::from_str(&toml_str)
        .map_err(|e| format!("Failed to parse config.toml in '{}': {}", dir.display(), e))?;

    // Scan for *.xml template files
    let mut templates = std::collections::HashMap::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| {
        format!(
            "Failed to read profile directory '{}': {}",
            dir.display(),
            e
        )
    })? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("xml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read template '{}': {}", path.display(), e))?;
                templates.insert(stem.to_string(), content);
            }
        }
    }

    let config = blueprint::AemConfig::from_profile(&profile, templates, ctx)?;
    Ok(config)
}

/// Load HTML custom styles from the `html/` subdirectory of a profile.
///
/// Returns `None` if no profile is specified or the `html/` subdirectory
/// does not exist within the profile.
fn load_html_config(
    profile_path: Option<&Path>,
) -> Result<Option<HtmlCustomStyles>, Box<dyn std::error::Error>> {
    let base = match profile_path {
        Some(p) => p,
        None => return Ok(None),
    };

    let dir = base.join("html");
    if !dir.is_dir() {
        return Ok(None);
    }

    // Read config.toml
    let config_path = dir.join("config.toml");
    if !config_path.exists() {
        return Ok(None);
    }

    let toml_str = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config.toml in '{}': {}", dir.display(), e))?;
    let profile: blueprint::HtmlProfile = toml::from_str(&toml_str)
        .map_err(|e| format!("Failed to parse config.toml in '{}': {}", dir.display(), e))?;

    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;

    // Resolve stylesheet
    let stylesheet_css = match &profile.stylesheet {
        Some(path) => {
            let full = dir.join(path);
            let css = std::fs::read_to_string(&full)
                .map_err(|e| format!("Failed to read stylesheet '{}': {}", full.display(), e))?;
            Some(css)
        }
        None => None,
    };

    // Resolve logo
    let logo_data_uri = match &profile.logo {
        Some(path) => {
            let full = dir.join(path);
            let bytes = std::fs::read(&full)
                .map_err(|e| format!("Failed to read logo '{}': {}", full.display(), e))?;
            let mime = mime_from_extension(&full);
            let encoded = b64.encode(&bytes);
            Some(format!("data:{};base64,{}", mime, encoded))
        }
        None => None,
    };

    // Resolve fonts
    let mut font_faces = Vec::new();
    for font_profile in &profile.fonts {
        use blueprint::ResolvedFontVariant;

        let mut variants = Vec::new();

        let variant_specs: &[(
            &Option<std::path::PathBuf>,
            &str, // weight
            &str, // style
        )] = &[
            (&font_profile.regular, "normal", "normal"),
            (&font_profile.bold, "bold", "normal"),
            (&font_profile.italic, "normal", "italic"),
            (&font_profile.bold_italic, "bold", "italic"),
        ];

        for (opt_path, weight, style) in variant_specs {
            if let Some(path) = opt_path {
                let full = dir.join(path);
                let bytes = std::fs::read(&full)
                    .map_err(|e| format!("Failed to read font '{}': {}", full.display(), e))?;
                let encoded = b64.encode(&bytes);
                variants.push(ResolvedFontVariant {
                    weight: weight.to_string(),
                    style: style.to_string(),
                    data_uri: format!("data:font/ttf;base64,{}", encoded),
                });
            }
        }

        font_faces.push(blueprint::ResolvedFontFamily {
            family: font_profile.family.clone(),
            variants,
        });
    }

    Ok(Some(HtmlCustomStyles {
        stylesheet_css,
        logo_data_uri,
        font_faces,
    }))
}

/// Guess MIME type from file extension.
fn mime_from_extension(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}
