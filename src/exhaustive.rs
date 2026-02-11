//! Exhaustive form state exploration module.
//!
//! This module provides functionality to recursively discover and render
//! all possible form states by clicking through radio buttons and checkboxes.
//!
//! # Linear Field Exploration
//!
//! Fields are explored in a linear, globally-defined order. For each field:
//! - Radio buttons: Explore each option in the group (one branch per option)
//! - Checkboxes: Always select them
//! - Hidden/unavailable fields: Automatically skip and continue
//!
//! Only "complete" states (all fields processed) are collected.
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
use std::sync::{Arc, Mutex};
use std::thread;

use crate::flattened::Flattened;
use crate::structured::Selection;
use crate::xfa::scripting::{SomPath, XfaForm};
use crate::xfa::{XfaNode, XfaNodeKind};

/// A selectable field (radio button or checkbox) with its SOM path and shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

/// Action taken for a field during exploration
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FieldAction {
    /// Field was selected
    Selected,
    /// Field was skipped (not visible or already selected in radio group)
    Skipped,
}

/// Exploration state tracking which fields have been processed
#[derive(Debug, Clone)]
struct ExplorationState {
    /// Index of the next field to process in the global field order
    next_field_index: usize,
    /// Actions taken for each field (indexed by global field order)
    field_actions: Vec<Option<FieldAction>>,
    /// Current selections (for applying to the form)
    selections: Vec<Selection>,
}

impl ExplorationState {
    fn new(num_fields: usize) -> Self {
        Self {
            next_field_index: 0,
            field_actions: vec![None; num_fields],
            selections: Vec::new(),
        }
    }

    /// Check if all fields have been processed (complete state)
    fn is_complete(&self) -> bool {
        self.next_field_index >= self.field_actions.len()
    }

    /// Get a unique key for this exploration state based on actions taken
    fn state_key(&self) -> Vec<Option<FieldAction>> {
        self.field_actions.clone()
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
#[derive(Clone)]
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
/// combinations in a linear field order, producing a `Vec<CollectedState>`
/// with one entry per complete state.
///
/// Fields are explored in a globally-defined static order. Each exploration
/// path processes fields sequentially, either selecting or skipping them.
/// Only complete states (where all fields have been processed) are collected.
///
/// `xfa_bytes` must be the raw XFA XML bytes so that the explorer can
/// cheaply recreate a fresh `XfaForm` for each branch.
pub fn collect_states(
    form: &mut XfaForm,
    xfa_bytes: &[u8],
) -> Result<Vec<CollectedState>, crate::Error> {
    // Use Arc<Mutex<>> for thread-safe access to shared state
    let collected_states = Arc::new(Mutex::new(Vec::<CollectedState>::new()));
    let rendered_states = Arc::new(Mutex::new(HashSet::<Vec<Option<FieldAction>>>::new()));

    // OPTIMIZATION: Parse XFA once and cache for cloning (much faster than re-parsing)
    let base_nodes = Arc::new(XfaNode::parse(xfa_bytes).map_err(crate::Error::XfaParse)?);

    // Establish the global field ordering from the initial form state
    let global_field_order = get_all_checkbutton_fields_ordered(form.xfa_nodes());

    let initial_state = ExplorationState::new(global_field_order.len());

    collect_states_linear(
        form,
        initial_state,
        &global_field_order,
        rendered_states.clone(),
        collected_states.clone(),
        base_nodes.clone(),
    )?;

    // Extract the final collected states
    let states = Arc::try_unwrap(collected_states)
        .map(|mutex| mutex.into_inner().unwrap())
        .unwrap_or_else(|arc| arc.lock().unwrap().clone());

    Ok(states)
}

/// Pass 1 implementation: recursively collect all form states using linear field exploration.
///
/// Fields are processed in a globally-defined order. At each step:
/// - If the field can be selected, spawn a thread to explore that branch
/// - If the field cannot be selected, automatically skip it and continue
///
/// Only complete states (all fields processed) are collected.
fn collect_states_linear(
    form: &mut XfaForm,
    exploration_state: ExplorationState,
    global_field_order: &[SelectableField],
    rendered_states: Arc<Mutex<HashSet<Vec<Option<FieldAction>>>>>,
    collected_states: Arc<Mutex<Vec<CollectedState>>>,
    base_nodes: Arc<Vec<XfaNode>>,
) -> Result<(), crate::Error> {
    // Check if this is a complete state (all fields processed)
    if exploration_state.is_complete() {
        let state_key = exploration_state.state_key();

        // Skip if we've already collected this exact state (thread-safe check)
        {
            let mut states = rendered_states.lock().unwrap();
            if states.contains(&state_key) {
                return Ok(());
            }
            // Mark this state as collected
            states.insert(state_key);
        }

        // Generate a human-readable label based on selections
        let label = if exploration_state.selections.is_empty() {
            "default".to_string()
        } else {
            exploration_state
                .selections
                .iter()
                .map(|sel| sel.field_path.name().to_string())
                .collect::<Vec<_>>()
                .join("_")
        };

        // Get flattened data for this state (but don't analyze yet)
        let flattened = form.flattened().clone();

        // Store the collected state (thread-safe)
        collected_states.lock().unwrap().push(CollectedState {
            flattened,
            selections: exploration_state.selections.clone(),
            label,
        });

        return Ok(());
    }

    // Process the next field
    let field_index = exploration_state.next_field_index;
    let field = &global_field_order[field_index];

    // Check if field can be selected
    let can_select = can_select_field(form, field, &exploration_state.selections);

    // If field cannot be selected, automatically skip it and continue
    if !can_select {
        let mut new_state = exploration_state.clone();
        new_state.field_actions[field_index] = Some(FieldAction::Skipped);
        new_state.next_field_index = field_index + 1;

        return collect_states_linear(
            form,
            new_state,
            global_field_order,
            rendered_states,
            collected_states,
            base_nodes,
        );
    }

    // For radio buttons, explore each option in the group
    if field.is_radio() {
        if let Some(excl_group_path) = form.find_excl_group_for_field(field.path.as_str()) {
            // Find all radio buttons in this group
            let group_fields: Vec<SelectableField> = global_field_order
                .iter()
                .filter(|f| {
                    f.is_radio()
                        && form.find_excl_group_for_field(f.path.as_str()).as_ref()
                            == Some(&excl_group_path)
                })
                .cloned()
                .collect();

            let mut handles = Vec::new();

            // Try selecting each radio button in the group
            for radio_field in group_fields {
                let base_nodes = base_nodes.clone();
                let rendered_states = rendered_states.clone();
                let collected_states = collected_states.clone();
                let mut new_state = exploration_state.clone();
                let global_field_order = global_field_order.to_vec();

                let handle = thread::spawn(move || -> Result<(), crate::Error> {
                    let nodes_reset = base_nodes.as_ref().clone();
                    let mut new_form =
                        XfaForm::new(nodes_reset).map_err(crate::Error::FormCreation)?;

                    // Apply all current selections
                    for sel in &new_state.selections {
                        let _ = new_form.select_radio_button(sel.field_path.as_str());
                    }

                    // Select the radio button
                    let _ = new_form.select_radio_button(radio_field.path.as_str());

                    let group_path = new_form.find_excl_group_for_field(radio_field.path.as_str());
                    new_state
                        .selections
                        .push(Selection::new(radio_field.path.clone(), group_path));

                    // Mark all fields in this radio group as processed
                    for (idx, field) in global_field_order.iter().enumerate() {
                        if field.is_radio() {
                            if let Some(fg) =
                                new_form.find_excl_group_for_field(field.path.as_str())
                            {
                                if Some(&fg)
                                    == new_state
                                        .selections
                                        .last()
                                        .and_then(|s| s.group_path.as_ref())
                                {
                                    new_state.field_actions[idx] = if field.path == radio_field.path
                                    {
                                        Some(FieldAction::Selected)
                                    } else {
                                        Some(FieldAction::Skipped)
                                    };
                                    new_state.next_field_index = idx + 1;
                                }
                            }
                        }
                    }

                    new_form.refresh().map_err(crate::Error::FormCreation)?;

                    collect_states_linear(
                        &mut new_form,
                        new_state,
                        &global_field_order,
                        rendered_states,
                        collected_states,
                        base_nodes,
                    )
                });

                handles.push(handle);
            }

            // Wait for all threads
            for handle in handles {
                handle.join().unwrap()?;
            }

            return Ok(());
        }
    }

    // For checkboxes, just select it and continue
    let base_nodes = base_nodes.clone();
    let rendered_states = rendered_states.clone();
    let collected_states = collected_states.clone();
    let mut new_state = exploration_state.clone();
    let field = field.clone();
    let global_field_order = global_field_order.to_vec();

    let handle = thread::spawn(move || -> Result<(), crate::Error> {
        let nodes_reset = base_nodes.as_ref().clone();
        let mut new_form = XfaForm::new(nodes_reset).map_err(crate::Error::FormCreation)?;

        // Apply all current selections
        for sel in &new_state.selections {
            let _ = new_form.select_radio_button(sel.field_path.as_str());
        }

        // Select the checkbox
        if let Some(mut node) = new_form.resolve_mut(field.path.as_str()) {
            node.set_raw_value("1");
        }

        new_state
            .selections
            .push(Selection::new(field.path.clone(), None));
        new_state.field_actions[field_index] = Some(FieldAction::Selected);
        new_state.next_field_index = field_index + 1;

        new_form.refresh().map_err(crate::Error::FormCreation)?;

        collect_states_linear(
            &mut new_form,
            new_state,
            &global_field_order,
            rendered_states,
            collected_states,
            base_nodes,
        )
    });

    handle.join().unwrap()?;

    Ok(())
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Check if a field can be selected given the current state
fn can_select_field(
    form: &XfaForm,
    field: &SelectableField,
    current_selections: &[Selection],
) -> bool {
    // Check if field is visible
    if !form.is_path_visible(field.path.as_str()) {
        return false;
    }

    // Check if already selected
    if current_selections
        .iter()
        .any(|s| s.field_path == field.path)
    {
        return false;
    }

    // For radio buttons, check if a sibling from the same group is selected
    if field.is_radio() {
        if let Some(excl_group) = form.find_excl_group_for_field(field.path.as_str()) {
            let group_already_has_selection = current_selections.iter().any(|sel| {
                form.find_excl_group_for_field(sel.field_path.as_str())
                    .map(|g| g == excl_group)
                    .unwrap_or(false)
            });
            if group_already_has_selection {
                return false;
            }
        }
    }

    true
}

/// Get ALL checkButton/checkbox fields in globally-defined order.
/// This establishes the static ordering used throughout the exploration.
fn get_all_checkbutton_fields_ordered(nodes: &[XfaNode]) -> Vec<SelectableField> {
    let mut results = Vec::new();
    search_checkbuttons(nodes, "", &mut results);
    // Sort by SOM path to ensure consistent global ordering
    results.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    results
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
