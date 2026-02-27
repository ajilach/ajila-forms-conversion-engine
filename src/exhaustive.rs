//! Exhaustive form state exploration module.
//!
//! This module provides functionality to recursively discover and render
//! all possible form states by clicking through radio buttons, checkboxes,
//! and dropdowns.
//!
//! # Linear Field Exploration
//!
//! Fields are explored in a linear, globally-defined order. For each field:
//! - Radio buttons: Explore each option in the group (one branch per option)
//! - Checkboxes: Explore both checked and unchecked states
//! - Dropdowns: Explore each option (one branch per option)
//! - Hidden/unavailable fields: Automatically skip and continue
//!
//! Only "complete" states (all fields processed) are collected.
//! The full cartesian product of all field values is explored, but only
//! states where all visible fields have been processed are kept.
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
use std::sync::{Arc, Mutex};
use std::thread;

use crate::flattened::{Flattened, FlattenedKey};

/// Global registry of seen (layout, field_index) pairs for cross-path deduplication.
///
/// Maps a composite key (flattened layout + next field index) to the selections
/// that first reached that state. When another path reaches the same state at
/// the same exploration depth, we skip redundant exploration.
///
/// The field index is included in the key to avoid incorrectly deduplicating
/// states at different exploration depths that happen to have the same flattened
/// layout (common in forms where selections don't change visibility).
type SeenLayouts = Arc<Mutex<HashMap<(Vec<FlattenedKey>, usize), Vec<Selection>>>>;
use crate::structured::{FieldId, Selection, SelectionKind};
use crate::xfa::scripting::{SomPath, XfaForm};
use crate::xfa::{XfaNode, XfaNodeKind};

/// The kind of selectable field found in the XFA tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum SelectableFieldKind {
    /// Radio button (checkButton with shape="round", inside an exclGroup)
    Radio,
    /// Checkbox (checkButton with shape="square")
    Checkbox,
    /// Dropdown (choiceList). Options are resolved dynamically from the live form
    /// at exploration time, since they may come from merged data or scripts.
    Dropdown,
}

/// A selectable field (radio button, checkbox, or dropdown) with its SOM path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SelectableField {
    /// The SOM path uniquely identifying this field
    path: SomPath,
    /// The kind of selectable field
    kind: SelectableFieldKind,
}

impl SelectableField {
    fn new(path: SomPath, kind: SelectableFieldKind) -> Self {
        Self { path, kind }
    }

    /// Returns true if this is a radio button
    fn is_radio(&self) -> bool {
        matches!(self.kind, SelectableFieldKind::Radio)
    }
}

/// Action taken for a field during exploration
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FieldAction {
    /// Field was selected with a specific value (e.g., "1"/"0" for checkbox, save value for dropdown)
    Selected(String),
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
#[allow(dead_code)]
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
    /// Complete field actions for dedup bookkeeping (not used outside exhaustive)
    field_actions: Vec<Option<FieldAction>>,
}

impl CollectedState {
    /// Create a new `CollectedState` without field actions.
    ///
    /// Used by non-XFA (AcroForm) PDFs where no exhaustive exploration occurs.
    pub fn new_simple(flattened: Flattened, selections: Vec<Selection>, label: String) -> Self {
        CollectedState {
            flattened,
            selections,
            label,
            field_actions: Vec::new(),
        }
    }
}

// ============================================================================
// Pure library API — no I/O, no printing
// ============================================================================

/// Collect all reachable form states from the given XFA form.
///
/// This is the library-facing entry point. It performs **Pass 1** of the
/// two-pass architecture: recursively explores radio button / checkbox /
/// dropdown combinations in a linear field order, producing a `Vec<CollectedState>`
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
    _xfa_bytes: &[u8],
) -> Result<Vec<CollectedState>, crate::Error> {
    // Use Arc<Mutex<>> for thread-safe access to shared state
    let collected_states = Arc::new(Mutex::new(Vec::<CollectedState>::new()));
    let rendered_states = Arc::new(Mutex::new(HashSet::<Vec<Option<FieldAction>>>::new()));

    // Cross-path deduplication: track seen flattened layouts globally
    let seen_layouts: SeenLayouts = Arc::new(Mutex::new(HashMap::new()));

    // OPTIMIZATION: Cache post-init nodes and computed values from the already-
    // initialised form. Branches use `XfaForm::from_post_init` which skips the
    // expensive `ScriptExecutor::execute()` phase (saves one Boa JS context
    // creation + all init-script phases per branch).
    let post_init_nodes = Arc::new(form.xfa_nodes().to_vec());
    let init_values = Arc::new(form.current_field_values());

    // Establish the global field ordering from the initial form state
    // Only includes fields with interactive scripts (change, click, calculate)
    let global_field_order = get_all_selectable_fields_ordered(form);

    let initial_state = ExplorationState::new(global_field_order.len());

    collect_states_linear(
        form,
        initial_state,
        &global_field_order,
        rendered_states.clone(),
        collected_states.clone(),
        post_init_nodes.clone(),
        init_values.clone(),
        seen_layouts.clone(),
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
    post_init_nodes: Arc<Vec<XfaNode>>,
    init_values: Arc<HashMap<SomPath, String>>,
    seen_layouts: SeenLayouts,
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
        let label = generate_label(&exploration_state.selections);

        // Get flattened data for this state (but don't analyze yet)
        let flattened = form.flattened().clone();

        // Store the collected state (thread-safe)
        collected_states.lock().unwrap().push(CollectedState {
            flattened,
            selections: exploration_state.selections.clone(),
            label,
            field_actions: exploration_state.field_actions.clone(),
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
            post_init_nodes,
            init_values,
            seen_layouts,
        );
    }

    match &field.kind {
        SelectableFieldKind::Radio => {
            explore_radio(
                form,
                exploration_state,
                field_index,
                field,
                global_field_order,
                rendered_states,
                collected_states,
                post_init_nodes,
                init_values,
                seen_layouts,
            )?;
        }
        SelectableFieldKind::Checkbox => {
            explore_checkbox(
                exploration_state,
                field_index,
                field,
                global_field_order,
                rendered_states,
                collected_states,
                post_init_nodes,
                init_values,
                seen_layouts,
            )?;
        }
        SelectableFieldKind::Dropdown => {
            // Resolve dropdown options from the live form (they may come from
            // merged data or scripts, so they aren't available at discovery time)
            let options = form
                .resolve(field.path.as_str())
                .map(|node| node.dropdown_options())
                .unwrap_or_default();
            if options.is_empty() {
                // No options available — skip this dropdown
                let mut new_state = exploration_state.clone();
                new_state.field_actions[field_index] = Some(FieldAction::Skipped);
                new_state.next_field_index = field_index + 1;
                return collect_states_linear(
                    form,
                    new_state,
                    global_field_order,
                    rendered_states,
                    collected_states,
                    post_init_nodes,
                    init_values,
                    seen_layouts,
                );
            }
            explore_dropdown(
                exploration_state,
                field_index,
                field,
                &options,
                global_field_order,
                rendered_states,
                collected_states,
                post_init_nodes,
                init_values,
                seen_layouts,
            )?;
        }
    }

    Ok(())
}

// ============================================================================
// Branch preparation (Phase A)
// ============================================================================

/// A prepared branch: the result of applying a mutation and refreshing the form,
/// before any further recursive exploration.
///
/// Stores the post-refresh XFA node snapshot and field values (which are `Send`)
/// instead of the full `XfaForm` (which contains a Boa JS engine and is not
/// `Send`). The `XfaForm` is recreated from the snapshot via `from_post_init`
/// for the representative branch during Phase B — no selection replay needed.
struct PreparedBranch {
    /// Structural key of the flattened layout after refresh — used for deduplication.
    /// Captures position, dimensions, text content, and field names/labels while
    /// excluding field values and checked state.
    flattened_key: Vec<FlattenedKey>,
    /// Post-mutation XFA node snapshot — used to recreate the form without replay.
    snapshot_nodes: Vec<XfaNode>,
    /// Post-mutation field values — used together with snapshot_nodes.
    snapshot_values: HashMap<SomPath, String>,
    /// The exploration state at this point (selections, field_actions, etc.)
    state: ExplorationState,
}

/// Spawn a thread that creates a fresh form from a snapshot (which already
/// has all prior selections applied), applies a branch-specific mutation
/// via `setup`, refreshes the form, and returns the prepared branch including
/// a new snapshot for downstream recursion.
///
/// The `snapshot_nodes` and `snapshot_values` represent the form state at the
/// current exploration depth — they already include all prior selections, so
/// no replay is needed. Only the new branch mutation is applied.
fn spawn_branch(
    snapshot_nodes: Arc<Vec<XfaNode>>,
    snapshot_values: Arc<HashMap<SomPath, String>>,
    state: ExplorationState,
    setup: impl FnOnce(&mut XfaForm, &mut ExplorationState) + Send + 'static,
) -> thread::JoinHandle<Result<PreparedBranch, crate::Error>> {
    thread::spawn(move || -> Result<PreparedBranch, crate::Error> {
        // OPTIMIZATION: Use from_post_init to skip ScriptExecutor::execute().
        // The snapshot nodes already have presence changes, form-DOM merges,
        // AND all prior selections applied — saving one Boa JS context creation,
        // all init-script phases, and selection replay per branch.
        let nodes = snapshot_nodes.as_ref().clone();
        let mut new_form =
            XfaForm::from_post_init(nodes, &snapshot_values).map_err(crate::Error::FormCreation)?;
        let mut state = state;

        // Apply branch-specific mutation (no replay needed — snapshot is current)
        setup(&mut new_form, &mut state);

        new_form.refresh().map_err(crate::Error::FormCreation)?;

        // Build a structural key from the flattened layout for deduplication.
        // This captures JS-driven label/presence changes that only appear
        // after reflattening, while ignoring field values and checked state.
        let flattened_key = FlattenedKey::from_flattened(new_form.flattened());

        // Capture the post-mutation snapshot so the representative can create
        // a form via from_post_init without replaying any selections.
        let snapshot_nodes = new_form.xfa_nodes().to_vec();
        let snapshot_values = new_form.current_field_values();

        Ok(PreparedBranch {
            flattened_key,
            snapshot_nodes,
            snapshot_values,
            state,
        })
    })
}

/// Given a set of prepared branches, group them by identical XFA state
/// (using hash + PartialEq), then recurse only once per unique state.
/// The collected states from the representative are cloned for each duplicate
/// with the duplicate's own selection/field-actions patched in.
///
/// Cross-path deduplication: When branches from different fields converge to
/// the same intermediate flattened layout, their subtrees would be identical.
/// We track seen layouts to avoid redundant exploration, but only when the
/// layout key is distinct enough (includes selection depth/field info).
fn explore_with_dedup(
    branches: Vec<PreparedBranch>,
    global_field_order: &[SelectableField],
    rendered_states: Arc<Mutex<HashSet<Vec<Option<FieldAction>>>>>,
    collected_states: Arc<Mutex<Vec<CollectedState>>>,
    seen_layouts: SeenLayouts,
) -> Result<(), crate::Error> {
    // Group branches by identical flattened layout.
    // Each group is a vec of branches whose flattened output is structurally
    // identical (same positions, text content, field names/labels).
    let groups = group_branches_by_flattened_state(branches);

    for mut group in groups {
        // Only recurse the representative of each group.
        // Duplicates within the same group have identical flattened output,
        // so they would produce the same visual result — skip them entirely.
        let mut representative = group.remove(0);

        // Record the values from duplicate branches on the representative's
        // last selection so the merger can later emit one conditional per value.
        if !group.is_empty() {
            if let Some(rep_sel) = representative.state.selections.last_mut() {
                for dup in &group {
                    if let Some(dup_sel) = dup.state.selections.last() {
                        // Only merge values for the same selection depth/field
                        if dup_sel.field_path == rep_sel.field_path
                            || dup_sel.group_path == rep_sel.group_path
                        {
                            for v in &dup_sel.values {
                                rep_sel.add_value(v.clone());
                            }
                        }
                    }
                }
            }
        }

        // Cross-path deduplication: Build a composite key that includes both
        // the flattened layout AND the current exploration progress (field index).
        // This avoids the bug where forms with no conditional visibility would
        // skip all exploration after the first branch because all intermediate
        // states have the same flattened layout.
        let composite_key = (
            representative.flattened_key.clone(),
            representative.state.next_field_index,
        );

        {
            let mut seen = seen_layouts.lock().unwrap();
            if seen.contains_key(&composite_key) {
                // This exact (layout, field_index) combination was already explored.
                // The subtree would be identical, so skip redundant exploration.
                continue;
            }
            // Register this composite key as seen
            seen.insert(composite_key, representative.state.selections.clone());
        }

        // Create form directly from the branch's post-mutation snapshot —
        // no selection replay needed, no extra refresh.
        let depth_snapshot_nodes = Arc::new(representative.snapshot_nodes);
        let depth_snapshot_values = Arc::new(representative.snapshot_values);

        let mut form = XfaForm::from_post_init(
            depth_snapshot_nodes.as_ref().clone(),
            &depth_snapshot_values,
        )
        .map_err(crate::Error::FormCreation)?;

        collect_states_linear(
            &mut form,
            representative.state,
            global_field_order,
            rendered_states.clone(),
            collected_states.clone(),
            depth_snapshot_nodes,
            depth_snapshot_values,
            seen_layouts.clone(),
        )?;
    }

    Ok(())
}

/// Group prepared branches by identical flattened layout.
///
/// Uses `FlattenedKey` (which derives `Eq + Hash`) for grouping.
/// Branches whose flattened output has the same structure (positions,
/// text content, field names/labels) but differ only in field values
/// are placed in the same group.
fn group_branches_by_flattened_state(branches: Vec<PreparedBranch>) -> Vec<Vec<PreparedBranch>> {
    use std::collections::HashMap;

    let mut key_to_group: HashMap<Vec<FlattenedKey>, Vec<usize>> = HashMap::new();
    for (i, branch) in branches.iter().enumerate() {
        key_to_group
            .entry(branch.flattened_key.clone())
            .or_default()
            .push(i);
    }

    // Convert index groups → branch groups (consuming the vec)
    let mut branches: Vec<Option<PreparedBranch>> = branches.into_iter().map(Some).collect();
    let mut groups: Vec<Vec<PreparedBranch>> = Vec::new();
    for (_key, indices) in key_to_group {
        let mut group = Vec::with_capacity(indices.len());
        for i in indices {
            if let Some(b) = branches[i].take() {
                group.push(b);
            }
        }
        groups.push(group);
    }

    groups
}

/// Patch the selections of a cloned collected state: replace the
/// representative's branching selection(s) with the duplicate's.
///
/// Generate a human-readable label from a list of selections.
fn generate_label(selections: &[Selection]) -> String {
    if selections.is_empty() {
        "default".to_string()
    } else {
        selections
            .iter()
            .map(|sel| match sel.kind {
                SelectionKind::Radio => sel.som_path.name().to_string(),
                SelectionKind::Checkbox => {
                    format!("{}_{}", sel.som_path.name(), sel.primary_value())
                }
                SelectionKind::Dropdown => {
                    format!("{}_{}", sel.som_path.name(), sel.primary_value())
                }
            })
            .collect::<Vec<_>>()
            .join("_")
    }
}

/// Explore all options of a radio button group.
/// Phase A: prepares one branch per radio option. Phase B: dedup + recurse.
fn explore_radio(
    form: &mut XfaForm,
    exploration_state: ExplorationState,
    _field_index: usize,
    field: &SelectableField,
    global_field_order: &[SelectableField],
    rendered_states: Arc<Mutex<HashSet<Vec<Option<FieldAction>>>>>,
    collected_states: Arc<Mutex<Vec<CollectedState>>>,
    post_init_nodes: Arc<Vec<XfaNode>>,
    init_values: Arc<HashMap<SomPath, String>>,
    seen_layouts: SeenLayouts,
) -> Result<(), crate::Error> {
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

        // Phase A: spawn branch-preparation threads
        let mut handles = Vec::new();
        for radio_field in group_fields {
            let global_order_for_closure = global_field_order.to_vec();

            let handle = spawn_branch(
                post_init_nodes.clone(),
                init_values.clone(),
                exploration_state.clone(),
                move |new_form, state| {
                    // Select the radio button
                    let _ = new_form.select_radio_button(radio_field.path.as_str());

                    let group_path = new_form.find_excl_group_for_field(radio_field.path.as_str());
                    state.selections.push(Selection::new(
                        radio_field.path.clone(),
                        group_path.clone(),
                        radio_field.path.name().to_string(),
                        SelectionKind::Radio,
                    ));

                    // Mark all fields in this radio group as processed
                    for (idx, f) in global_order_for_closure.iter().enumerate() {
                        if f.is_radio()
                            && let Some(fg) = new_form.find_excl_group_for_field(f.path.as_str())
                            && state.selections.last().and_then(|s| s.group_path.as_ref())
                                == Some(&FieldId::from_som_path(&fg))
                        {
                            state.field_actions[idx] = if f.path == radio_field.path {
                                Some(FieldAction::Selected(radio_field.path.name().to_string()))
                            } else {
                                Some(FieldAction::Skipped)
                            };
                            state.next_field_index = idx + 1;
                        }
                    }
                },
            );

            handles.push(handle);
        }

        // Collect prepared branches
        let mut branches = Vec::new();
        for handle in handles {
            branches.push(handle.join().unwrap()?);
        }

        // Phase B: group by identical XFA state and recurse
        explore_with_dedup(
            branches,
            global_field_order,
            rendered_states,
            collected_states,
            seen_layouts,
        )?;
    }

    Ok(())
}

/// Explore both checked and unchecked states of a checkbox.
/// Phase A: prepares two branches (checked/unchecked). Phase B: dedup + recurse.
fn explore_checkbox(
    exploration_state: ExplorationState,
    field_index: usize,
    field: &SelectableField,
    global_field_order: &[SelectableField],
    rendered_states: Arc<Mutex<HashSet<Vec<Option<FieldAction>>>>>,
    collected_states: Arc<Mutex<Vec<CollectedState>>>,
    post_init_nodes: Arc<Vec<XfaNode>>,
    init_values: Arc<HashMap<SomPath, String>>,
    seen_layouts: SeenLayouts,
) -> Result<(), crate::Error> {
    let checkbox_values = [("1", "checked"), ("0", "unchecked")];

    // Phase A: spawn branch-preparation threads
    let mut handles = Vec::new();
    for (raw_value, label) in checkbox_values {
        let field = field.clone();
        let raw_value = raw_value.to_string();
        let label = label.to_string();

        let handle = spawn_branch(
            post_init_nodes.clone(),
            init_values.clone(),
            exploration_state.clone(),
            move |new_form, state| {
                // Set the checkbox value and fire change event
                let _ = new_form.set_value_as_user(field.path.as_str(), &raw_value);

                state.selections.push(Selection::standalone(
                    field.path.clone(),
                    label.clone(),
                    SelectionKind::Checkbox,
                ));
                state.field_actions[field_index] = Some(FieldAction::Selected(label));
                state.next_field_index = field_index + 1;
            },
        );

        handles.push(handle);
    }

    // Collect prepared branches
    let mut branches = Vec::new();
    for handle in handles {
        branches.push(handle.join().unwrap()?);
    }

    // Phase B: group by identical XFA state and recurse
    explore_with_dedup(
        branches,
        global_field_order,
        rendered_states,
        collected_states,
        seen_layouts,
    )?;

    Ok(())
}

/// Explore all options of a dropdown field.
/// Phase A: prepares one branch per option. Phase B: dedup + recurse.
fn explore_dropdown(
    exploration_state: ExplorationState,
    field_index: usize,
    field: &SelectableField,
    options: &[(String, String)],
    global_field_order: &[SelectableField],
    rendered_states: Arc<Mutex<HashSet<Vec<Option<FieldAction>>>>>,
    collected_states: Arc<Mutex<Vec<CollectedState>>>,
    post_init_nodes: Arc<Vec<XfaNode>>,
    init_values: Arc<HashMap<SomPath, String>>,
    seen_layouts: SeenLayouts,
) -> Result<(), crate::Error> {
    // Phase A: spawn branch-preparation threads
    let mut handles = Vec::new();
    for (display_value, save_value) in options {
        let field = field.clone();
        let save_value = save_value.clone();
        let display_value = display_value.clone();

        let handle = spawn_branch(
            post_init_nodes.clone(),
            init_values.clone(),
            exploration_state.clone(),
            move |new_form, state| {
                // Set the dropdown value and fire change event
                let _ = new_form.set_value_as_user(field.path.as_str(), &save_value);

                state.selections.push(Selection::standalone(
                    field.path.clone(),
                    display_value.clone(),
                    SelectionKind::Dropdown,
                ));
                state.field_actions[field_index] = Some(FieldAction::Selected(save_value));
                state.next_field_index = field_index + 1;
            },
        );

        handles.push(handle);
    }

    // Collect prepared branches
    let mut branches = Vec::new();
    for handle in handles {
        branches.push(handle.join().unwrap()?);
    }

    // Phase B: group by identical XFA state and recurse
    explore_with_dedup(
        branches,
        global_field_order,
        rendered_states,
        collected_states,
        seen_layouts,
    )?;

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
        .any(|s| s.field_path == FieldId::from_som_path(&field.path))
    {
        return false;
    }

    // For radio buttons, check if a sibling from the same group is selected
    if field.is_radio()
        && let Some(excl_group) = form.find_excl_group_for_field(field.path.as_str())
    {
        let excl_group_id = FieldId::from_som_path(&excl_group);
        let group_already_has_selection = current_selections
            .iter()
            .any(|sel| sel.group_path.as_ref() == Some(&excl_group_id));
        if group_already_has_selection {
            return false;
        }
    }

    true
}

/// Get selectable fields (radio buttons, checkboxes, dropdowns) that have interactive scripts.
/// Only fields with change, click, or calculate scripts on themselves or their parent exclGroup
/// (for radios) are included. This establishes the static ordering used throughout the exploration.
fn get_all_selectable_fields_ordered(form: &XfaForm) -> Vec<SelectableField> {
    let mut results = Vec::new();
    search_selectable_fields(form.xfa_nodes(), "", &mut results);

    // Filter to only include fields with interactive scripts
    let registry = form.script_registry();
    results.retain(|field| {
        // Check if the field itself has interactive scripts
        if registry.has_interactive_scripts(&field.path) {
            return true;
        }

        // For radio buttons, also check the parent exclGroup
        if field.is_radio() {
            if let Some(excl_group_path) = form.find_excl_group_for_field(field.path.as_str()) {
                if registry.has_interactive_scripts(&excl_group_path) {
                    return true;
                }
            }
        }

        false
    });

    // Sort by SOM path to ensure consistent global ordering
    results.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    results
}

/// Search for all selectable fields in the XFA tree: checkButtons (radio/checkbox) and choiceLists (dropdown).
fn search_selectable_fields(
    nodes: &[XfaNode],
    current_path: &str,
    results: &mut Vec<SelectableField>,
) {
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
            let name = node.name.clone().unwrap_or_default();
            if !name.is_empty() {
                // Look for <ui> child and check for checkButton or choiceList
                let field_kind = node.children.iter().find_map(|c| {
                    if let XfaNodeKind::Element { tag_name: t, .. } = &c.kind
                        && t == "ui"
                    {
                        return c.children.iter().find_map(|ui_c| {
                            if let XfaNodeKind::Element { tag_name: t2, .. } = &ui_c.kind {
                                match t2.as_str() {
                                    "checkButton" => {
                                        let shape = ui_c
                                            .attributes
                                            .get("shape")
                                            .cloned()
                                            .unwrap_or_else(|| "square".to_string());
                                        if shape == "round" {
                                            Some(SelectableFieldKind::Radio)
                                        } else {
                                            Some(SelectableFieldKind::Checkbox)
                                        }
                                    }
                                    "choiceList" => Some(SelectableFieldKind::Dropdown),
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        });
                    }
                    None
                });

                if let Some(kind) = field_kind {
                    results.push(SelectableField::new(SomPath::new(node_path.clone()), kind));
                }
            }
        }

        // Recurse into children
        search_selectable_fields(&node.children, &node_path, results);
    }
}
