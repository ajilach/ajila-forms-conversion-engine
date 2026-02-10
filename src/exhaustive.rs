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

// ============================================================================
// Public data types used by both the library facade and the CLI
// ============================================================================

/// Collected state data from the first pass (public for use by lib.rs facade).
pub struct CollectedState {
    /// The flattened data for this state
    pub flattened: Flattened,
    /// The selections that led to this state (with group path info)
    pub selections: Vec<Selection>,
    /// State suffix / human-readable label for this state
    pub label: String,
}

// ============================================================================
// Pure library API — no I/O, no printing
// ============================================================================

/// Collect all reachable form states from the given XFA form.
///
/// This is the library-facing entry point. It performs **Pass 1** of the
/// two-pass architecture: recursively explores radio button / checkbox
/// combinations, producing a `Vec<CollectedState>` with one entry per
/// unique state.
///
/// `xfa_bytes` must be the raw XFA XML bytes so that the explorer can
/// cheaply recreate a fresh `XfaForm` for each branch.
pub fn collect_states(
    form: &mut XfaForm,
    xfa_bytes: &[u8],
) -> Result<Vec<CollectedState>, crate::Error> {
    let mut rendered_states: HashSet<Vec<SomPath>> = HashSet::new();
    let mut collected_states: Vec<CollectedState> = Vec::new();

    // OPTIMIZATION: Parse XFA once and cache for cloning (much faster than re-parsing)
    let base_nodes = XfaNode::parse(xfa_bytes).map_err(crate::Error::XfaParse)?;

    collect_all_states_cached(
        form,
        Vec::new(),
        &mut rendered_states,
        &mut collected_states,
        &base_nodes,
    )?;

    Ok(collected_states)
}

/// Pass 1 implementation: recursively collect all form states using cached
/// parsed nodes (no XML re-parsing needed).
fn collect_all_states_cached(
    form: &mut XfaForm,
    current_selections: Vec<Selection>,
    rendered_states: &mut HashSet<Vec<SomPath>>,
    collected_states: &mut Vec<CollectedState>,
    base_nodes: &[XfaNode],
) -> Result<(), crate::Error> {
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

    // Generate a human-readable label based on selections
    let label = if current_selections.is_empty() {
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
        label,
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

        // OPTIMIZATION: Clone nodes instead of re-parsing XML (10-100x faster)
        let nodes_reset = base_nodes.to_vec();
        let mut new_form = XfaForm::new(nodes_reset).map_err(crate::Error::FormCreation)?;

        // OPTIMIZATION: Apply all selections in batch, then refresh once
        let mut new_selections = current_selections.clone();

        // Apply all current selections without refreshing
        for sel in &current_selections {
            let _ = new_form.select_radio_button(sel.field_path.as_str());
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
        let group_path = if field.is_radio() {
            new_form.find_excl_group_for_field(field.path.as_str())
        } else {
            None
        };
        new_selections.push(Selection::new(field.path.clone(), group_path));

        // OPTIMIZATION: Single refresh at the end instead of after each selection
        new_form.refresh().map_err(crate::Error::FormCreation)?;

        // Recursively collect from this new state
        collect_all_states_cached(
            &mut new_form,
            new_selections,
            rendered_states,
            collected_states,
            base_nodes,
        )?;
    }

    Ok(())
}

// ============================================================================
// Shared helpers
// ============================================================================

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
