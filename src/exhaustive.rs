//! Exhaustive form state exploration module.
//!
//! This module provides functionality to recursively discover and render
//! all possible form states by clicking through radio buttons and checkboxes.
//!
//! # Two-Pass Architecture
//!
//! When running in exhaustive mode, the module uses a two-pass approach:
//! 1. **Collection Pass**: Explore all form states and collect flattened data
//! 2. **Analysis Pass**: Compute global statistics from all states, then run
//!    analysis pipeline on each state using the global context
//!
//! This ensures consistent heading detection and other statistics-based
//! analysis across all form states.
use std::collections::HashSet;
use std::path::Path;

use crate::RenderMode;
use crate::document::modules::{GlobalContext, GlobalFontStats};
use crate::flattened::Flattened;
use crate::structured::Selection;
use crate::xfa::scripting::{SomPath, XfaForm};
use crate::xfa::{XfaNode, XfaNodeKind};

/// A selectable field (radio button or checkbox) with its SOM path and shape.
#[derive(Debug, Clone)]
struct SelectableField {
    /// The SOM path uniquely identifying this field
    path: SomPath,
    /// The shape of the checkButton: "round" for radio buttons, "square" for checkboxes
    shape: String,
}

impl SelectableField {
    fn new(path: SomPath, shape: String) -> Self {
        Self { path, shape }
    }

    /// Returns true if this is a radio button (round shape)
    fn is_radio(&self) -> bool {
        self.shape == "round"
    }

    /// Get the field name (last component of the SOM path)
    fn name(&self) -> &str {
        self.path.name()
    }
}

/// Get a canonical state representation by sorting the selections.
/// This ensures that the same set of selections always produces the same state key,
/// regardless of the order in which the selections were made.
fn get_current_state(selections: &[SomPath]) -> Vec<SomPath> {
    let mut sorted = selections.to_vec();
    sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    sorted
}

/// Configuration for exhaustive rendering
pub struct ExhaustiveConfig<'a> {
    /// Name of the document (used for output filenames)
    pub doc_name: &'a str,
    /// Scale factor for rendering
    pub scale: f32,
    /// Path to the source PDF file
    pub pdf_path: &'a Path,
    /// Render modes to use (can be multiple: plain, labelled, annotated)
    pub render_modes: Vec<RenderMode>,
    /// Whether to output structured JSON for each state
    pub structured: bool,
    /// Whether to generate HTML output
    pub html: bool,
    /// Whether to suppress verbose output
    pub quiet: bool,
}

/// Result of exhaustive exploration
pub struct ExhaustiveResult {
    /// Total number of unique states rendered
    pub states_rendered: usize,
}

/// Collected state data from the first pass
struct CollectedState {
    /// The flattened data for this state
    flattened: Flattened,
    /// The selections that led to this state (with group path info)
    selections: Vec<Selection>,
    /// State suffix for file naming
    state_suffix: String,
}

/// Run exhaustive form state exploration.
///
/// This function recursively explores all possible form states by:
/// 1. Starting from the default state
/// 2. Finding all visible radio buttons and checkboxes
/// 3. For each unselected control, creating a new state with it selected
/// 4. Recursively exploring from each new state
///
/// Each unique state is rendered to a PNG file.
pub fn run_exhaustive(
    form: &mut XfaForm,
    config: &ExhaustiveConfig,
) -> Result<ExhaustiveResult, Box<dyn std::error::Error>> {
    if !config.quiet {
        println!("\nExhaustive mode: recursively discovering all form states...");
        if !config.render_modes.is_empty() {
            println!("  Render modes: {:?}", config.render_modes);
        }
        if config.structured {
            println!("  Structured JSON: enabled");
        }
    }

    // ========================================================================
    // Pass 1: Collect all form states and their flattened data
    // ========================================================================
    if !config.quiet {
        println!("\n  Pass 1: Collecting all form states...");
    }

    let mut rendered_states: HashSet<Vec<SomPath>> = HashSet::new();
    let mut collected_states: Vec<CollectedState> = Vec::new();

    collect_all_states(
        form,
        Vec::new(),
        &mut rendered_states,
        &mut collected_states,
        config,
    )?;

    if !config.quiet {
        println!("    Found {} unique states", collected_states.len());
    }

    // ========================================================================
    // Compute global font statistics from all collected flattened data
    // ========================================================================
    if !config.quiet {
        println!("\n  Computing global font statistics...");
    }

    let flattened_refs: Vec<&Flattened> = collected_states.iter().map(|s| &s.flattened).collect();
    let mut global_font_stats =
        GlobalFontStats::from_flattened_iter(flattened_refs.iter().copied());
    // Second pass: compute border statistics for consistent heading level detection
    global_font_stats.compute_border_stats(flattened_refs.iter().copied());

    if !config.quiet {
        println!(
            "    Body size: {:.1}pt, {} text samples from {} states",
            global_font_stats.body_size,
            global_font_stats.sample_count,
            collected_states.len()
        );
        if global_font_stats.border_stats.total_count > 0 {
            println!(
                "    Border stats: {}/{} headings have borders (should_use_borders: {})",
                global_font_stats.border_stats.underlined_count,
                global_font_stats.border_stats.total_count,
                global_font_stats.border_stats.should_use_borders()
            );
        }
    }

    // ========================================================================
    // Pass 2: Run analysis pipeline with global context and generate outputs
    // ========================================================================
    if !config.quiet {
        println!("\n  Pass 2: Analyzing and generating outputs...");
    }

    let global_ctx = GlobalContext::with_font_stats(&flattened_refs, global_font_stats);
    let mut images_rendered = 0;

    // Collect all structured outputs for merging
    let mut structured_outputs: Vec<(Vec<Selection>, Vec<crate::structured::StructuredNode>)> =
        Vec::new();

    for state in &collected_states {
        let (rendered, structured_nodes) = process_state_with_context(state, &global_ctx, config)?;
        images_rendered += rendered;

        if config.structured {
            if let Some(nodes) = structured_nodes {
                structured_outputs.push((state.selections.clone(), nodes));
            }
        }
    }

    // Merge all structured outputs and write the merged JSON
    if config.structured && !structured_outputs.is_empty() {
        if !config.quiet {
            println!(
                "\n  Merging {} structured outputs...",
                structured_outputs.len()
            );
        }

        let merge_inputs: Vec<crate::structured::MergeInput> = structured_outputs
            .into_iter()
            .map(|(selections, nodes)| crate::structured::MergeInput::new(selections, nodes))
            .collect();

        let merger = crate::structured::RecursiveMerger::new(merge_inputs);
        let merged = merger.merge();

        // Write merged JSON
        let json = serde_json::to_string_pretty(&merged)
            .map_err(|e| format!("Failed to serialize merged structured form: {}", e))?;

        let json_path = std::path::PathBuf::from(format!("{}_merged.json", config.doc_name));
        std::fs::write(&json_path, json)
            .map_err(|e| format!("Failed to write merged JSON file: {}", e))?;

        if !config.quiet {
            println!("    ✓ Merged output: {}", json_path.display());
        }

        // Generate merged HTML if requested
        if config.html {
            let html_config = crate::html::HtmlConfig::default();
            let html = crate::html::generate_html(&merged, &html_config);

            let html_path = std::path::PathBuf::from(format!("{}_merged.html", config.doc_name));
            std::fs::write(&html_path, html)
                .map_err(|e| format!("Failed to write merged HTML file: {}", e))?;

            if !config.quiet {
                println!("    ✓ Merged HTML: {}", html_path.display());
            }
        }
    }

    if !config.quiet {
        println!(
            "\n✓ Exhaustive rendering complete ({} unique states)",
            collected_states.len()
        );
    }

    Ok(ExhaustiveResult {
        states_rendered: collected_states.len(),
    })
}

/// Run exhaustive form state exploration and return merged structured nodes directly.
///
/// This is a test helper function that performs exhaustive exploration and returns
/// the merged StructuredNode tree without writing to files.
pub fn run_exhaustive_to_merged(
    pdf_path: &str,
) -> Result<Vec<crate::structured::StructuredNode>, Box<dyn std::error::Error>> {
    use crate::xfa::XfaNode;

    // Extract and parse XFA from PDF
    let xfa_data = crate::extract_xfa_from_pdf(pdf_path)?
        .ok_or_else(|| format!("No XFA data in PDF: {}", pdf_path))?;
    let nodes = XfaNode::parse(&xfa_data)?;
    let mut form = XfaForm::new(nodes)?;

    // ========================================================================
    // Pass 1: Collect all form states and their flattened data
    // ========================================================================
    let mut rendered_states: HashSet<Vec<SomPath>> = HashSet::new();
    let mut collected_states: Vec<CollectedState> = Vec::new();

    // Use a dummy config for state collection
    let config = ExhaustiveConfig {
        pdf_path: Path::new(pdf_path),
        doc_name: "_test",
        scale: 1.0,
        render_modes: vec![],
        structured: true,
        quiet: true,
        html: false,
    };

    collect_all_states(
        &mut form,
        Vec::new(),
        &mut rendered_states,
        &mut collected_states,
        &config,
    )?;

    // ========================================================================
    // Compute global font statistics from all collected flattened data
    // ========================================================================
    let flattened_refs: Vec<&Flattened> = collected_states.iter().map(|s| &s.flattened).collect();
    let mut global_font_stats =
        GlobalFontStats::from_flattened_iter(flattened_refs.iter().copied());
    global_font_stats.compute_border_stats(flattened_refs.iter().copied());

    // ========================================================================
    // Pass 2: Run analysis pipeline with global context
    // ========================================================================
    let global_ctx = GlobalContext::with_font_stats(&flattened_refs, global_font_stats);

    // Collect all structured outputs for merging
    let mut structured_outputs: Vec<(Vec<Selection>, Vec<crate::structured::StructuredNode>)> =
        Vec::new();

    for state in &collected_states {
        let (_, structured_nodes) = process_state_with_context(state, &global_ctx, &config)?;

        if let Some(nodes) = structured_nodes {
            structured_outputs.push((state.selections.clone(), nodes));
        }
    }

    // Merge all structured outputs
    if structured_outputs.is_empty() {
        return Ok(Vec::new());
    }

    let merge_inputs: Vec<crate::structured::MergeInput> = structured_outputs
        .into_iter()
        .map(|(selections, nodes)| crate::structured::MergeInput::new(selections, nodes))
        .collect();

    let merger = crate::structured::RecursiveMerger::new(merge_inputs);
    Ok(merger.merge())
}

/// Pass 1: Recursively collect all form states and their flattened data.
/// This does NOT run the analysis pipeline yet.
fn collect_all_states(
    form: &mut XfaForm,
    current_selections: Vec<Selection>,
    rendered_states: &mut HashSet<Vec<SomPath>>,
    collected_states: &mut Vec<CollectedState>,
    config: &ExhaustiveConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // Get the canonical state representation (using field paths for deduplication)
    let field_paths: Vec<SomPath> = current_selections
        .iter()
        .map(|s| s.field_path.clone())
        .collect();
    let state = get_current_state(&field_paths);

    // Skip if we've already collected this state
    if rendered_states.contains(&state) {
        return Ok(());
    }

    // Mark this state as collected
    rendered_states.insert(state.clone());

    // Generate a filename suffix based on selections
    let state_suffix = if current_selections.is_empty() {
        "default".to_string()
    } else {
        current_selections
            .iter()
            .map(|sel| sel.field_path.name().to_string())
            .collect::<Vec<_>>()
            .join("_")
    };

    // Get flattened data for this state (but don't analyze yet)
    let flattened = form.flattened().clone();

    // Store the collected state
    collected_states.push(CollectedState {
        flattened,
        selections: current_selections.clone(),
        state_suffix,
    });

    // Get visible selectable fields in this state
    let visible_fields = get_visible_selectable_fields(form);

    // Recursively explore other states
    for field in &visible_fields {
        // Skip if already selected
        if current_selections
            .iter()
            .any(|s| s.field_path == field.path)
        {
            continue;
        }

        // For radio buttons, skip if a sibling from the same group is selected
        if field.is_radio() {
            if let Some(excl_group) = form.find_excl_group_for_field(field.path.as_str()) {
                let group_already_has_selection = current_selections.iter().any(|sel| {
                    form.find_excl_group_for_field(sel.field_path.as_str())
                        .map(|g| g == excl_group)
                        .unwrap_or(false)
                });
                if group_already_has_selection {
                    continue;
                }
            }
        }

        // Create a fresh form from the PDF
        let xfa_data_reset = crate::extract_xfa_from_pdf(config.pdf_path)?.unwrap();
        let nodes_reset = XfaNode::parse(&xfa_data_reset)?;
        let mut new_form =
            XfaForm::new(nodes_reset).map_err(|e| format!("Failed to recreate XfaForm: {}", e))?;

        // Apply all current selections
        let mut new_selections = current_selections.clone();
        for sel in &current_selections {
            let _ = new_form.select_radio_button(sel.field_path.as_str());
            new_form.refresh()?;
        }

        // Select the new field
        if field.is_radio() {
            let _ = new_form.select_radio_button(field.path.as_str());
        } else {
            if let Some(mut node) = new_form.resolve_mut(field.path.as_str()) {
                node.set_raw_value("1");
            }
        }

        // Create the selection with group path info
        // For radio buttons, look up the exclGroup path from the form
        let group_path = if field.is_radio() {
            new_form.find_excl_group_for_field(field.path.as_str())
        } else {
            None
        };
        new_selections.push(Selection::new(field.path.clone(), group_path));
        new_form.refresh()?;

        // Recursively collect from this new state
        collect_all_states(
            &mut new_form,
            new_selections,
            rendered_states,
            collected_states,
            config,
        )?;
    }

    Ok(())
}

/// Pass 2: Process a collected state using the global context.
/// Runs analysis pipeline with global statistics and generates outputs.
/// Returns (images_rendered, optional_structured_nodes).
fn process_state_with_context(
    state: &CollectedState,
    global_ctx: &GlobalContext,
    config: &ExhaustiveConfig,
) -> Result<(usize, Option<Vec<crate::structured::StructuredNode>>), Box<dyn std::error::Error>> {
    let mut images_rendered = 0;

    // Create Document and run analysis pipeline with global context
    let mut doc = crate::document::Document::from_flattened(&state.flattened);
    crate::document::modules::run_analysis_pipeline_with_context(&mut doc, global_ctx);

    let mut outputs = Vec::new();

    // Render PNGs for each requested render mode
    for mode in &config.render_modes {
        let suffix = match mode {
            RenderMode::Plain => "plain",
            RenderMode::Labelled => "labelled",
            RenderMode::Annotated => "annotated",
        };
        let output_path = std::path::PathBuf::from(format!(
            "{}_{}.{}.png",
            config.doc_name, state.state_suffix, suffix
        ));

        match mode {
            RenderMode::Plain => {
                state
                    .flattened
                    .render_to_image_buffer_plain(config.scale)?
                    .save(&output_path)
                    .map_err(|e| format!("Failed to save image: {}", e))?;
            }
            RenderMode::Labelled => {
                doc.render_to_image(&output_path, config.scale)?;
            }
            RenderMode::Annotated => {
                state
                    .flattened
                    .render_to_image(&output_path, config.scale)?;
            }
        }
        outputs.push(output_path.display().to_string());
        images_rendered += 1;
    }

    // Output structured JSON if requested - always write intermediate per-state files
    let structured_nodes = if config.structured {
        let nodes = crate::structured::convert(&doc);

        // Write individual JSON files for each state (intermediate representations)
        let json = serde_json::to_string_pretty(&nodes)
            .map_err(|e| format!("Failed to serialize structured form: {}", e))?;

        let json_path =
            std::path::PathBuf::from(format!("{}_{}.json", config.doc_name, state.state_suffix));
        std::fs::write(&json_path, json)
            .map_err(|e| format!("Failed to write JSON file: {}", e))?;
        outputs.push(json_path.display().to_string());

        Some(nodes)
    } else {
        None
    };

    if !config.quiet {
        println!(
            "    ✓ Generated: {} (selections: {:?})",
            outputs.join(", "),
            state
                .selections
                .iter()
                .map(|sel| sel.field_path.as_str())
                .collect::<Vec<_>>()
        );
    }

    Ok((images_rendered, structured_nodes))
}

/// Find ALL checkButton/checkbox fields in the XFA tree (including hidden sections).
/// Returns SelectableField entries where the SomPath uniquely identifies each field.
fn find_all_checkbutton_fields(nodes: &[XfaNode]) -> Vec<SelectableField> {
    let mut results = Vec::new();
    search_checkbuttons(nodes, "", &mut results);
    results
}

fn search_checkbuttons(nodes: &[XfaNode], current_path: &str, results: &mut Vec<SelectableField>) {
    for node in nodes {
        // Build the SOM path for this node
        let node_path = if let Some(name) = &node.name {
            if current_path.is_empty() {
                name.clone()
            } else {
                format!("{}.{}", current_path, name)
            }
        } else {
            current_path.to_string()
        };

        // Check if this is a Field node
        if matches!(&node.kind, XfaNodeKind::Field) {
            // Check if this field has a checkButton UI element
            let shape = node.children.iter().find_map(|c| {
                if let XfaNodeKind::Element { tag_name: t, .. } = &c.kind {
                    if t == "ui" {
                        return c.children.iter().find_map(|ui_c| {
                            if let XfaNodeKind::Element { tag_name: t2, .. } = &ui_c.kind {
                                if t2 == "checkButton" {
                                    return Some(
                                        ui_c.attributes
                                            .get("shape")
                                            .cloned()
                                            .unwrap_or_else(|| "square".to_string()),
                                    );
                                }
                            }
                            None
                        });
                    }
                }
                None
            });

            if let Some(shape) = shape {
                let name = node.name.clone().unwrap_or_default();
                if !name.is_empty() {
                    // Convert shape to our standard format
                    let shape_normalized = if shape == "round" { "round" } else { "square" };
                    results.push(SelectableField::new(
                        SomPath::new(node_path.clone()),
                        shape_normalized.to_string(),
                    ));
                }
            }
        }
        // Recurse into children
        search_checkbuttons(&node.children, &node_path, results);
    }
}

/// Get the list of all VISIBLE radio buttons/checkboxes in a given form state.
fn get_visible_selectable_fields(form: &XfaForm) -> Vec<SelectableField> {
    // Get ALL checkButton fields with their SOM paths
    let all_fields = find_all_checkbutton_fields(form.xfa_nodes());

    // Filter to only those that are currently visible (checking XFA tree)
    all_fields
        .into_iter()
        .filter(|field| form.is_path_visible(field.path.as_str()))
        .collect()
}
