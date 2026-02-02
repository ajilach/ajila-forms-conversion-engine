//! Exhaustive form state exploration module.
//!
//! This module provides functionality to recursively discover and render
//! all possible form states by clicking through radio buttons and checkboxes.

use std::collections::HashSet;
use std::path::Path;

use crate::RenderMode;
use crate::flattened::Flattened;
use crate::scripting::{SomPath, XfaForm};
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
    /// Whether to suppress verbose output
    pub quiet: bool,
}

/// Result of exhaustive exploration
pub struct ExhaustiveResult {
    /// Total number of unique states rendered
    pub states_rendered: usize,
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

    let mut rendered_states: HashSet<Vec<SomPath>> = HashSet::new();

    let states_rendered = explore_states(
        form,
        Vec::new(), // No selections initially
        &mut rendered_states,
        config,
    )?;

    if !config.quiet {
        println!(
            "\n✓ Exhaustive rendering complete ({} unique states)",
            states_rendered
        );
    }

    Ok(ExhaustiveResult { states_rendered })
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

/// Convert current selection state to a canonical representation.
fn get_current_state(selections: &[SomPath]) -> Vec<SomPath> {
    let mut state: Vec<_> = selections.iter().cloned().collect();
    state.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    state
}

/// Recursive function to explore all states.
/// Selections are SomPath values for the selected fields.
fn explore_states(
    form: &mut XfaForm,
    current_selections: Vec<SomPath>,
    rendered_states: &mut HashSet<Vec<SomPath>>,
    config: &ExhaustiveConfig,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut images_rendered = 0;

    // Get the canonical state representation
    let state = get_current_state(&current_selections);

    // Skip if we've already rendered this state
    if rendered_states.contains(&state) {
        return Ok(0);
    }

    // Mark this state as rendered
    rendered_states.insert(state.clone());

    // Generate a filename based on the current selections (use short names for readability)
    let state_suffix = if current_selections.is_empty() {
        "default".to_string()
    } else {
        current_selections
            .iter()
            .map(|path| path.name().to_string())
            .collect::<Vec<_>>()
            .join("_")
    };

    // Create Document and run analysis pipeline once for this state
    let flattened = form.flattened();
    let mut doc = crate::document::Document::from_flattened(flattened);
    crate::modules::run_analysis_pipeline(&mut doc);

    // Output all requested formats from the analyzed document
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
            config.doc_name, state_suffix, suffix
        ));

        match mode {
            RenderMode::Plain => {
                flattened
                    .render_to_image_buffer_plain(config.scale)?
                    .save(&output_path)
                    .map_err(|e| format!("Failed to save image: {}", e))?;
            }
            RenderMode::Labelled => {
                doc.render_to_image(&output_path, config.scale)?;
            }
            RenderMode::Annotated => {
                flattened.render_to_image(&output_path, config.scale)?;
            }
        }
        outputs.push(output_path.display().to_string());
        images_rendered += 1;
    }

    // Output structured JSON if requested
    if config.structured {
        let structured_nodes = crate::modules::convert_to_structured(&doc);
        let json = serde_json::to_string_pretty(&structured_nodes)
            .map_err(|e| format!("Failed to serialize structured form: {}", e))?;

        let json_path =
            std::path::PathBuf::from(format!("{}_{}.json", config.doc_name, state_suffix));
        std::fs::write(&json_path, json)
            .map_err(|e| format!("Failed to write JSON file: {}", e))?;
        outputs.push(json_path.display().to_string());
    }

    if !config.quiet {
        println!(
            "  ✓ Generated: {} (selections: {:?})",
            outputs.join(", "),
            current_selections
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
        );
    }

    // Get visible selectable fields in this state (with SOM paths)
    let visible_fields = get_visible_selectable_fields(form);

    // For each visible field that is NOT already selected, try selecting it
    for field in &visible_fields {
        // Skip if this field (by SOM path) is already in our current selections
        if current_selections.iter().any(|p| p == &field.path) {
            continue;
        }

        // For radio buttons, also skip if a sibling from the same group is selected
        // (we only select one radio button per group)
        if field.is_radio() {
            // Check if any sibling in the same exclGroup is already selected
            // Use the SOM path for proper path-based exclGroup lookup
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

        // Apply all current selections to the new form using FULL SOM paths
        // This ensures we select the correct field when there are duplicates (e.g., RB_1 in different sections)
        let mut new_selections = current_selections.clone();
        for sel_path in &current_selections {
            let _ = new_form.select_radio_button(sel_path.as_str());
            new_form.refresh()?;
        }

        // Now select the new field using its SOM path
        if field.is_radio() {
            let _ = new_form.select_radio_button(field.path.as_str());
        } else {
            // For checkboxes, use the SOM path for resolution
            if let Some(mut node) = new_form.resolve_mut(field.path.as_str()) {
                node.set_raw_value("1");
            }
        }

        // Add this field to our selections
        new_selections.push(field.path.clone());

        // Refresh the form to apply changes
        new_form.refresh()?;

        // Recursively explore from this new state
        images_rendered += explore_states(&mut new_form, new_selections, rendered_states, config)?;
    }

    Ok(images_rendered)
}
