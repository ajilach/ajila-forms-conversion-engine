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

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::RenderMode;
use crate::flattened::Flattened;
use crate::modules::{GlobalContext, GlobalFontStats, MergeInput, merge_structured_trees};
use crate::scripting::{SomPath, XfaForm};
use crate::structured::{InputValue, StructuredNode};
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
    /// Locale identifier (e.g., "DE", "EN")
    pub locale: &'a str,
    /// Render modes to use (can be multiple: plain, labelled, annotated)
    pub render_modes: Vec<RenderMode>,
    /// Whether to output structured JSON for each state
    pub structured: bool,
    /// Whether to merge all structured outputs into a single JSON with conditionals
    pub merge: bool,
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
    /// The selections that led to this state
    selections: Vec<SomPath>,
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
        if config.merge {
            println!("  Merge mode: enabled");
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
    let global_font_stats = GlobalFontStats::from_flattened_iter(flattened_refs.iter().copied());

    if !config.quiet {
        println!(
            "    Body size: {:.1}pt, {} text samples from {} states",
            global_font_stats.body_size,
            global_font_stats.sample_count,
            collected_states.len()
        );
    }

    // ========================================================================
    // Pass 2: Run analysis pipeline with global context and generate outputs
    // ========================================================================
    if !config.quiet {
        println!("\n  Pass 2: Analyzing and generating outputs...");
    }

    let global_ctx = GlobalContext::with_font_stats(&flattened_refs, global_font_stats);
    let mut merge_inputs: Vec<MergeInput> = Vec::new();
    let mut images_rendered = 0;

    for state in &collected_states {
        images_rendered +=
            process_state_with_context(state, &global_ctx, &mut merge_inputs, config)?;
    }

    // If merge mode is enabled, merge all collected structured outputs
    if config.merge && !merge_inputs.is_empty() {
        if !config.quiet {
            println!("\nMerging {} structured outputs...", merge_inputs.len());
        }

        let merged = merge_structured_trees(merge_inputs);
        let json = serde_json::to_string_pretty(&merged)
            .map_err(|e| format!("Failed to serialize merged structured form: {}", e))?;

        let json_path = std::path::PathBuf::from(format!("{}_merged.json", config.doc_name));
        std::fs::write(&json_path, json)
            .map_err(|e| format!("Failed to write merged JSON file: {}", e))?;

        if !config.quiet {
            println!("  ✓ Merged output: {}", json_path.display());
        }

        // Generate HTML from merged output if requested
        if config.html {
            use crate::modules::{HtmlConfig, generate_html};

            if !config.quiet {
                println!("\nGenerating HTML form from merged output...");
            }

            let html_config = HtmlConfig {
                form_id: config.doc_name.to_string(),
                include_styles: true,
                include_scripts: true,
            };

            // Wrap merged node in a slice for generate_html
            let nodes = vec![merged];
            let html = generate_html(&nodes, &html_config);

            let html_path = std::path::PathBuf::from(format!("{}_merged.html", config.doc_name));
            std::fs::write(&html_path, html)
                .map_err(|e| format!("Failed to write merged HTML file: {}", e))?;

            if !config.quiet {
                println!("  ✓ HTML form: {}", html_path.display());
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

/// Pass 1: Recursively collect all form states and their flattened data.
/// This does NOT run the analysis pipeline yet.
fn collect_all_states(
    form: &mut XfaForm,
    current_selections: Vec<SomPath>,
    rendered_states: &mut HashSet<Vec<SomPath>>,
    collected_states: &mut Vec<CollectedState>,
    config: &ExhaustiveConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    // Get the canonical state representation
    let state = get_current_state(&current_selections);

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
            .map(|path| path.name().to_string())
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
        if current_selections.iter().any(|p| p == &field.path) {
            continue;
        }

        // For radio buttons, skip if a sibling from the same group is selected
        if field.is_radio() {
            if let Some(excl_group) = form.find_excl_group_for_field(field.path.as_str()) {
                let group_already_has_selection = current_selections.iter().any(|sel_path| {
                    form.find_excl_group_for_field(sel_path.as_str())
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
        for sel_path in &current_selections {
            let _ = new_form.select_radio_button(sel_path.as_str());
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

        new_selections.push(field.path.clone());
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
fn process_state_with_context(
    state: &CollectedState,
    global_ctx: &GlobalContext,
    merge_inputs: &mut Vec<MergeInput>,
    config: &ExhaustiveConfig,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut images_rendered = 0;

    // Create Document and run analysis pipeline with global context
    let mut doc = crate::document::Document::from_flattened(&state.flattened);
    crate::modules::run_analysis_pipeline_with_context(&mut doc, global_ctx);

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

    // Output structured JSON if requested (or collect for merging)
    if config.structured || config.merge {
        let structured_nodes = crate::modules::convert_to_structured(&doc);

        // Write individual JSON file if structured output is requested
        if config.structured {
            let json = serde_json::to_string_pretty(&structured_nodes)
                .map_err(|e| format!("Failed to serialize structured form: {}", e))?;

            let json_path = std::path::PathBuf::from(format!(
                "{}_{}.json",
                config.doc_name, state.state_suffix
            ));
            std::fs::write(&json_path, json)
                .map_err(|e| format!("Failed to write JSON file: {}", e))?;
            outputs.push(json_path.display().to_string());
        }

        // Collect for merging if merge mode is enabled
        if config.merge {
            // Build the complete state_values map from all selections
            let mut state_values = HashMap::new();

            if !state.selections.is_empty() {
                // Build a mapping from (grouped_field_name, internal_name) -> option_label
                // by traversing the structured output
                let radio_mapping = build_radio_option_mapping(&structured_nodes);

                // We need to get the values for all selections
                // Since we don't have the form anymore, we need to recreate it temporarily
                let xfa_data = crate::extract_xfa_from_pdf(config.pdf_path)?.unwrap();
                let nodes = XfaNode::parse(&xfa_data)?;
                let mut temp_form = XfaForm::new(nodes)
                    .map_err(|e| format!("Failed to recreate XfaForm: {}", e))?;

                // Apply selections to get the correct values
                for sel_path in &state.selections {
                    let _ = temp_form.select_radio_button(sel_path.as_str());
                    temp_form.refresh()?;
                }

                // Now get the value for each selection and build state_values
                for sel_path in &state.selections {
                    // Use the grouped field name (e.g., "RB_1_RB_2_RB_3") if it's a radio button group
                    let field_name = get_field_name_for_path(&temp_form, sel_path);
                    let internal_name = sel_path.name().to_string();

                    // Look up the option label from the mapping, fall back to internal name
                    let value = if let Some(label) =
                        radio_mapping.get(&(field_name.clone(), internal_name.clone()))
                    {
                        InputValue::Radio(label.clone())
                    } else {
                        get_selection_value(&temp_form, sel_path)
                    };

                    state_values.insert(field_name, value);
                }
            }

            let last_path = state
                .selections
                .last()
                .cloned()
                .unwrap_or_else(|| SomPath::new("__default__"));

            merge_inputs.push(MergeInput {
                tree: StructuredNode::Group(crate::structured::GroupNode {
                    children: structured_nodes,
                }),
                state_values,
                last_path,
            });
        }
    }

    if !config.quiet {
        println!(
            "    ✓ Generated: {} (selections: {:?})",
            outputs.join(", "),
            state
                .selections
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
        );
    }

    Ok(images_rendered)
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

/// Get the InputValue for a selected field.
/// For radio buttons, this returns the radio button's name as the value
/// (this distinguishes which option was selected within the group).
/// For checkboxes, this returns Checkbox(true) since selecting means checking.
fn get_selection_value(form: &XfaForm, path: &SomPath) -> InputValue {
    // Check if this is a radio button or checkbox
    let is_radio = find_all_checkbutton_fields(form.xfa_nodes())
        .iter()
        .any(|f| &f.path == path && f.is_radio());

    if is_radio {
        // Use the button's name as the value (distinguishes which option in the group)
        InputValue::Radio(path.name().to_string())
    } else {
        // Checkbox - when selected, it's checked (true)
        InputValue::Checkbox(true)
    }
}

/// Get the logical field name for a selection path.
/// For radio buttons in a group, this returns the combined name (e.g., "RB_1_RB_2_RB_3").
/// For checkboxes, this returns the checkbox's own name.
fn get_field_name_for_path(form: &XfaForm, path: &SomPath) -> String {
    // Find all checkbutton fields to identify radio groups
    let all_fields = find_all_checkbutton_fields(form.xfa_nodes());

    // Check if this is a radio button
    let is_radio = all_fields.iter().any(|f| &f.path == path && f.is_radio());

    if is_radio {
        // Find the parent subform that contains this radio button's group
        // Radio buttons in the same group share a parent subform
        let parent_path = path.parent();

        // Find all radio buttons with the same parent (siblings in the same group)
        let siblings: Vec<_> = all_fields
            .iter()
            .filter(|f| f.is_radio() && f.path.parent() == parent_path)
            .collect();

        if siblings.len() > 1 {
            // Multiple radio buttons = a group. Combine their names.
            let mut names: Vec<_> = siblings.iter().map(|f| f.path.name().to_string()).collect();
            names.sort();
            names.dedup();
            return names.join("_");
        }
    }

    // Single field or checkbox: use its own name
    path.name().to_string()
}

/// Build a mapping from (grouped_field_name, internal_name) -> option_label
/// by traversing the structured nodes and extracting radio field information.
fn build_radio_option_mapping(nodes: &[StructuredNode]) -> HashMap<(String, String), String> {
    use crate::structured::FieldType;

    let mut mapping = HashMap::new();

    fn traverse(node: &StructuredNode, mapping: &mut HashMap<(String, String), String>) {
        match node {
            StructuredNode::Field(field) => {
                if let FieldType::Radio {
                    options,
                    option_names,
                } = &field.input_type
                {
                    if let Some(names) = option_names {
                        // Both options and names should have the same length
                        for (internal_name, label) in names.iter().zip(options.iter()) {
                            mapping
                                .insert((field.name.clone(), internal_name.clone()), label.clone());
                        }
                    }
                }
            }
            StructuredNode::Group(group) => {
                for child in &group.children {
                    traverse(child, mapping);
                }
            }
            StructuredNode::Repeatable(rep) => {
                traverse(&rep.item, mapping);
            }
            StructuredNode::Conditional(cond) => {
                traverse(&cond.content, mapping);
            }
            StructuredNode::Table(table) => {
                if let Some(header) = &table.header {
                    for cell in &header.cells {
                        traverse(cell, mapping);
                    }
                }
                for row in &table.rows {
                    for cell in &row.cells {
                        traverse(cell, mapping);
                    }
                }
            }
            _ => {}
        }
    }

    for node in nodes {
        traverse(node, &mut mapping);
    }

    mapping
}
