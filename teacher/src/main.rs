//! Teacher CLI – run the blueprint pipeline then invoke smart-edit and print
//! the suggested changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use blueprint::{PipelineConfig, PipelineEvent, PipelineStep, StructuredNode};
use clap::Parser;
use image::ImageEncoder;
use serde::{Deserialize, Serialize};

/// A single proposed change returned by the smart-edit LLM call.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ChangeItem {
    id: usize,
    description: String,
}

/// The structured result returned by a smart-edit call.
#[derive(Clone, Debug)]
struct SmartEditResult {
    nodes: Vec<StructuredNode>,
    changes: Vec<ChangeItem>,
}

/// Teacher – run the blueprint pipeline and smart-edit, then output suggested changes.
#[derive(Parser, Debug)]
#[command(name = "teacher")]
#[command(
    about = "Run the blueprint pipeline and smart-edit on a PDF, then print suggested changes"
)]
struct Args {
    /// Path(s) to the PDF document(s).
    #[arg(value_name = "DOCUMENT", required = true)]
    documents: Vec<PathBuf>,

    /// Scale factor for rendering (default: 1.5)
    #[arg(short, long, default_value = "1.5")]
    scale: f32,

    /// Name of an embedded profile.
    #[arg(long)]
    profile: Option<String>,

    /// Output format for the results.
    #[arg(long, value_enum, default_value = "text")]
    format: OutputFormat,

    /// OpenAI API key. Falls back to the OPENAI_API_KEY environment variable if not set.
    #[arg(long, env = "OPENAI_API_KEY")]
    api_key: String,

    /// OpenAI model to use (default: gpt-4o).
    #[arg(long, default_value = "gpt-4o")]
    model: String,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OutputFormat {
    /// Human-readable text summary.
    Text,
    /// Full JSON with nodes and changes.
    Json,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args = Args::parse();

    for doc_path in &args.documents {
        if !doc_path.exists() {
            eprintln!("Error: document not found: {}", doc_path.display());
            std::process::exit(1);
        }
    }

    // ── Read files ───────────────────────────────────────────────────────
    let files: Vec<(String, Vec<u8>)> = args
        .documents
        .iter()
        .map(|p| {
            let name = doc_stem(p).to_string();
            let bytes = std::fs::read(p)?;
            Ok((name, bytes))
        })
        .collect::<Result<_, std::io::Error>>()?;

    // ── Load profile fonts ───────────────────────────────────────────────
    if let Some(ref profile_name) = args.profile {
        blueprint::load_profile_fonts(profile_name)?;
    }

    // ── Run the pipeline ─────────────────────────────────────────────────
    let config = PipelineConfig {
        scale: args.scale,
        render_plain: true, // needed for smart-edit image context
        render_annotated: false,
        render_labelled: false,
    };

    let mut plain_images: HashMap<String, String> = HashMap::new();

    let output = blueprint::run_pipeline(&files, &config, |event| match event {
        PipelineEvent::StepChanged(step) => {
            let msg = match step {
                PipelineStep::Parsing => "Parsing PDF(s)...",
                PipelineStep::ExhaustiveSearching => "Discovering form states...",
                PipelineStep::Flattening => "Rendering plain images...",
                PipelineStep::Structuring => "Structuring form content...",
                PipelineStep::Merging => "Merging outputs...",
                PipelineStep::Complete => "Pipeline complete.",
            };
            eprintln!("{}", msg);
        }
        PipelineEvent::PlainRender { label, image } => {
            let mut png_bytes = Vec::new();
            if let Ok(()) = encode_rgba_to_png(&image, &mut png_bytes) {
                let b64 = base64::prelude::BASE64_STANDARD.encode(&png_bytes);
                plain_images.insert(label, b64);
            }
        }
        PipelineEvent::Warning(msg) => eprintln!("Warning: {}", msg),
        _ => {}
    })?;

    let content = &output.merged.content;
    eprintln!(
        "Pipeline produced {} root nodes, {} page images.",
        content.len(),
        plain_images.len()
    );

    // ── Run smart edit ───────────────────────────────────────────────────
    eprintln!("Running smart edit via OpenAI API...");
    let result = run_smart_edit(content, &[], &plain_images, &args.api_key, &args.model).await?;

    // ── Output results ───────────────────────────────────────────────────
    match args.format {
        OutputFormat::Text => {
            if result.changes.is_empty() {
                println!("No changes suggested.");
            } else {
                println!("Suggested changes:");
                for change in &result.changes {
                    println!("  [{}] {}", change.id, change.description);
                }
            }
        }
        OutputFormat::Json => {
            let out = serde_json::json!({
                "nodes": result.nodes,
                "changes": result.changes,
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
    }

    Ok(())
}

// ─── Multi-turn conversation history ────────────────────────────────────────

type ChatHistory = Vec<serde_json::Value>;

// ─── Smart edit logic ────────────────────────────────────────────────────────

fn serialize_selected_nodes(
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

async fn run_smart_edit(
    content: &[StructuredNode],
    selected_indices: &[usize],
    plain_images: &HashMap<String, String>,
    api_key: &str,
    model: &str,
) -> Result<SmartEditResult, String> {
    let json_context = serialize_selected_nodes(content, selected_indices)?;
    let images: Vec<(String, String)> = plain_images
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let prompt = build_smart_edit_prompt(selected_indices, plain_images);
    let user_text = build_initial_user_text(&prompt, &json_context);

    let mut history: ChatHistory = Vec::new();
    let raw = openai_chat_turn(&mut history, &user_text, &images, api_key, model).await?;
    let mut result = parse_with_repair(&raw, &mut history, api_key, model).await?;
    ensure_change_list(
        content,
        selected_indices,
        &mut history,
        &mut result,
        api_key,
        model,
    )
    .await;
    Ok(result)
}

async fn parse_with_repair(
    raw: &str,
    history: &mut ChatHistory,
    api_key: &str,
    model: &str,
) -> Result<SmartEditResult, String> {
    match parse_smart_edit_response(raw) {
        Ok(result) => Ok(result),
        Err(original_error) => {
            // The bad response is already in history; include the parse error
            // so the model knows what to fix.
            let repair_prompt = format!(
                "Your previous response was not parseable by the consumer. \
                 Parse error: {original_error}\n\n\
                 Re-emit the SAME answer in the exact required format.\n\
                 Return ONLY one valid JSON object with exactly two keys:\n\
                 - \"nodes\": array of StructuredNode JSON\n\
                 - \"changes\": array of {{\"id\": int, \"description\": string}}\n\
                 Do not add explanations, markdown, or code fences."
            );

            if let Ok(repaired_raw) =
                openai_chat_turn(history, &repair_prompt, &[], api_key, model).await
                && let Ok(parsed) = parse_smart_edit_response(&repaired_raw)
            {
                return Ok(parsed);
            }

            Err(original_error)
        }
    }
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
        return;
    }

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

    if let Ok(raw) = openai_chat_turn(history, &followup_prompt, &[], api_key, model).await
        && let Ok(changes) = parse_change_list(&raw)
        && !changes.is_empty()
    {
        result.changes = changes;
    }
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
         - Field: {{ name: UUID, label: InlineText|null, input_type: FieldType, value: InputValue|null, placeholder: TranslatableString|null }}\n\
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

// ─── OpenAI API integration ──────────────────────────────────────────────────

fn build_initial_user_text(prompt: &str, json_context: &str) -> String {
    format!(
        "{prompt}\n\n\
         The structured JSON representation of the selected form nodes is included below. \
         The attached PNG images show the rendered form pages for visual reference.\n\n\
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

async fn openai_chat_turn(
    history: &mut ChatHistory,
    user_text: &str,
    images: &[(String, String)],
    api_key: &str,
    model: &str,
) -> Result<String, String> {
    use async_openai::{Client, config::OpenAIConfig};

    if api_key.is_empty() {
        return Err(
            "OpenAI API key is not set. Pass --api-key or set the OPENAI_API_KEY environment variable.".to_string(),
        );
    }

    let mut content: Vec<serde_json::Value> =
        vec![serde_json::json!({"type": "text", "text": user_text})];

    for (_label, b64) in images {
        content.push(serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:image/png;base64,{b64}"),
                "detail": "high"
            }
        }));
    }

    history.push(serde_json::json!({"role": "user", "content": content}));

    let config = OpenAIConfig::new().with_api_key(api_key);
    let client = Client::with_config(config);

    let request = serde_json::json!({
        "model": model,
        "messages": history,
        "response_format": { "type": "json_object" },
    });

    let response: serde_json::Value = client
        .chat()
        .create_byot(request)
        .await
        .map_err(|e| format!("OpenAI API error: {e}"))?;

    let response_text = response["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| format!("Unexpected OpenAI response structure: {response}"))?
        .to_string();

    history.push(serde_json::json!({"role": "assistant", "content": response_text}));

    Ok(response_text)
}

// ─── Response parsing ────────────────────────────────────────────────────────

fn parse_smart_edit_response(response: &str) -> Result<SmartEditResult, String> {
    let trimmed = response.trim();
    let mut candidates: Vec<&str> = vec![trimmed];
    candidates.extend(extract_fenced_blocks(trimmed));

    for candidate in &candidates {
        if let Some(result) = try_parse_result_object(candidate) {
            return Ok(result);
        }
    }

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
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    let obj = value.as_object()?;
    let nodes_val = obj.get("nodes")?;
    let nodes: Vec<StructuredNode> = serde_json::from_value(nodes_val.clone()).ok()?;
    let changes: Vec<ChangeItem> = obj
        .get("changes")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Some(SmartEditResult { nodes, changes })
}

fn parse_change_list(response: &str) -> Result<Vec<ChangeItem>, String> {
    let trimmed = response.trim();

    if let Ok(items) = serde_json::from_str::<Vec<ChangeItem>>(trimmed) {
        return Ok(items);
    }

    for block in extract_fenced_blocks(trimmed) {
        if let Ok(items) = serde_json::from_str::<Vec<ChangeItem>>(block) {
            return Ok(items);
        }
    }

    for candidate in extract_json_array_candidates(trimmed) {
        if let Ok(items) = serde_json::from_str::<Vec<ChangeItem>>(candidate) {
            return Ok(items);
        }
    }

    Err("Could not parse change list".to_string())
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

fn compute_changed_indices(
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

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn doc_stem(path: &Path) -> &str {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document")
}

fn encode_rgba_to_png(img: &blueprint::RgbaImage, output: &mut Vec<u8>) -> Result<(), String> {
    use image::ExtendedColorType;
    use image::codecs::png::PngEncoder;

    let (width, height) = img.dimensions();
    let encoder = PngEncoder::new(output);

    encoder
        .write_image(img.as_raw(), width, height, ExtendedColorType::Rgba8)
        .map_err(|e| format!("PNG encoding error: {}", e))
}
