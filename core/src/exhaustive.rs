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

#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;
use regex_lite::Regex;

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
use crate::xfa::scripting::events::EventActivity;
use crate::xfa::scripting::registry::ScriptRegistry;
use crate::xfa::scripting::{SomPath, SomResolver, XfaForm};
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
// Exploration context — bundles all shared / precomputed state
// ============================================================================

/// Shared context for exhaustive exploration.
///
/// Groups all the precomputed maps, shared caches, and thread-safe state
/// that was previously passed as 8–10 individual function parameters.
struct ExplorationContext {
    /// Static ordering of selectable fields (radio/checkbox/dropdown with scripts)
    global_field_order: Vec<SelectableField>,

    /// Precomputed excl-group lookup: field SOM path → parent exclGroup SOM path.
    /// Avoids repeated `O(tree-depth)` walks during exploration.
    excl_group_map: HashMap<SomPath, Option<SomPath>>,

    /// Inverse index: exclGroup SOM path → indices into `global_field_order`.
    /// Used to find all radio buttons in a group without filtering + re-walking.
    radio_group_indices: HashMap<SomPath, Vec<usize>>,

    /// Shared script registry (Arc) — reused across all `from_post_init_with_registry` calls.
    script_registry: Arc<ScriptRegistry>,

    /// Cached post-init XFA nodes from the initial form.
    post_init_nodes: Arc<Vec<XfaNode>>,

    /// Cached initial computed field values.
    init_values: Arc<HashMap<SomPath, String>>,

    /// Thread-safe set of already-collected state keys (for dedup).
    rendered_states: Arc<Mutex<HashSet<Vec<Option<FieldAction>>>>>,

    /// Thread-safe vec of collected form states (output of Pass 1).
    collected_states: Arc<Mutex<Vec<CollectedState>>>,

    /// Cross-path deduplication: track seen (layout, field_index) pairs.
    seen_layouts: SeenLayouts,
}

// ============================================================================
// Independent field partitioning — static script analysis + union-find
// ============================================================================

/// Simple union-find (disjoint-set) data structure for partitioning fields.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]); // path compression
        }
        self.parent[x]
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        // union by rank
        match self.rank[ra].cmp(&self.rank[rb]) {
            std::cmp::Ordering::Less => self.parent[ra] = rb,
            std::cmp::Ordering::Greater => self.parent[rb] = ra,
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
            }
        }
    }

    /// Extract connected components as groups of original indices.
    fn groups(&mut self, n: usize) -> Vec<Vec<usize>> {
        let mut map: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..n {
            map.entry(self.find(i)).or_default().push(i);
        }
        // Sort groups by their smallest index to ensure deterministic ordering
        let mut groups: Vec<Vec<usize>> = map.into_values().collect();
        groups.sort_by_key(|g| g[0]);
        groups
    }
}

/// Categorised field reference: whether the script accesses `.presence`
/// (which affects container visibility and all descendants) or just
/// `.rawValue`/`.value` (which only affects the specific field).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RefKind {
    /// Script reads/writes `.presence` — toggling visibility of a container
    /// affects all interactive fields inside it.
    Presence,
    /// Script reads/writes `.rawValue` or `.value` — only the specific field
    /// is affected.
    Value,
    /// Script calls a method on the identifier (e.g., `soLocal.change()`) or
    /// references via `resolveNode` — conservatively treated like `Value`.
    Other,
}

/// A reference extracted from a script source, with its kind.
#[derive(Debug, Clone)]
struct FieldRef {
    name: String,
    kind: RefKind,
}

/// Extract field name references from a JavaScript script source,
/// categorised by reference kind (presence vs value vs other).
///
/// Uses regex to find patterns like:
/// - `Name.presence` → [`RefKind::Presence`]
/// - `Name.rawValue` / `Name.value` → [`RefKind::Value`]
/// - `Name.method(` → [`RefKind::Other`]
/// - `resolveNode("path")` → [`RefKind::Other`]
///
/// Script object names (like `soLocalLabelDefinition`) are NOT filtered —
/// they are returned so that the caller can resolve them transitively.
fn extract_field_references(source: &str) -> Vec<FieldRef> {
    let re_presence = Regex::new(r"(\b[A-Za-z_]\w*)\.presence").expect("valid regex");

    let re_value = Regex::new(r"(\b[A-Za-z_]\w*)\.(?:rawValue|value)").expect("valid regex");

    let re_resolve =
        Regex::new(r#"resolveNode\([\s]*["']([^"']+)["'][\s]*\)"#).expect("valid regex");

    let re_method = Regex::new(r"(\b[A-Za-z_]\w*)\.\w+\(").expect("valid regex");

    let mut refs = Vec::new();
    let mut seen: HashSet<(String, RefKind)> = HashSet::new();

    // Non-field names to ignore — only true JS globals, not script objects
    let ignore: HashSet<&str> = [
        "this",
        "xfa",
        "event",
        "app",
        "console",
        "Math",
        "String",
        "Number",
        "Date",
        "parseInt",
        "parseFloat",
        "JSON",
    ]
    .into_iter()
    .collect();

    let mut add_ref = |name: String, kind: RefKind, seen: &mut HashSet<(String, RefKind)>| {
        if !ignore.contains(name.as_str()) && seen.insert((name.clone(), kind.clone())) {
            refs.push(FieldRef { name, kind });
        }
    };

    for cap in re_presence.captures_iter(source) {
        add_ref(cap[1].to_string(), RefKind::Presence, &mut seen);
    }

    for cap in re_value.captures_iter(source) {
        add_ref(cap[1].to_string(), RefKind::Value, &mut seen);
    }

    for cap in re_resolve.captures_iter(source) {
        let path = &cap[1];
        let clean = path
            .trim_start_matches("xfa.form..")
            .trim_start_matches("xfa.form.");
        for part in clean.split('.') {
            let name = part.split('[').next().unwrap_or(part);
            if !name.is_empty() {
                add_ref(name.to_string(), RefKind::Other, &mut seen);
            }
        }
    }

    for cap in re_method.captures_iter(source) {
        add_ref(cap[1].to_string(), RefKind::Other, &mut seen);
    }

    refs
}

/// Collect script object sources from `<variables><script>` nodes in the XFA tree.
///
/// Returns a map of script object name → full source code. These are named
/// script objects defined in `<subform><variables><script name="...">` that
/// expose methods callable from event scripts (e.g. `soLocalLabelDefinition.change()`).
fn collect_script_object_sources(nodes: &[XfaNode]) -> HashMap<String, String> {
    let mut result: HashMap<String, String> = HashMap::new();

    fn walk(nodes: &[XfaNode], result: &mut HashMap<String, String>) {
        for node in nodes {
            if let XfaNodeKind::Element { tag_name, .. } = &node.kind {
                if tag_name == "variables" {
                    for child in &node.children {
                        if let XfaNodeKind::Element {
                            tag_name: child_tag,
                            text_content,
                            ..
                        } = &child.kind
                        {
                            if child_tag == "script" {
                                if let Some(name) = &child.name {
                                    // Collect source from text_content and child text nodes
                                    if let Some(content) = text_content {
                                        result.entry(name.clone()).or_default().push_str(content);
                                    }
                                    for script_child in &child.children {
                                        match &script_child.kind {
                                            XfaNodeKind::Element {
                                                text_content: Some(content),
                                                ..
                                            } => {
                                                result
                                                    .entry(name.clone())
                                                    .or_default()
                                                    .push_str(content);
                                            }
                                            XfaNodeKind::Text { content } => {
                                                result
                                                    .entry(name.clone())
                                                    .or_default()
                                                    .push_str(content);
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            walk(&node.children, result);
        }
    }

    walk(nodes, &mut result);
    result
}

/// Extract field references transitively through script objects.
///
/// First extracts direct references from the source. Then, for any reference
/// that matches a known script object name, recursively analyses that script
/// object's source code to find additional field references.
fn extract_field_references_transitive(
    source: &str,
    script_objects: &HashMap<String, String>,
) -> Vec<FieldRef> {
    let mut all_refs = extract_field_references(source);
    let mut seen_names: HashSet<(String, RefKind)> = all_refs
        .iter()
        .map(|r| (r.name.clone(), r.kind.clone()))
        .collect();
    let mut visited_objects: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = all_refs
        .iter()
        .filter(|r| script_objects.contains_key(r.name.as_str()))
        .map(|r| r.name.clone())
        .collect();

    while let Some(obj_name) = queue.pop() {
        if !visited_objects.insert(obj_name.clone()) {
            continue;
        }
        if let Some(obj_source) = script_objects.get(&obj_name) {
            let obj_refs = extract_field_references(obj_source);
            for r in obj_refs {
                let key = (r.name.clone(), r.kind.clone());
                if seen_names.insert(key) {
                    if script_objects.contains_key(r.name.as_str()) {
                        queue.push(r.name.clone());
                    }
                    all_refs.push(r);
                }
            }
        }
    }

    all_refs
}

/// Collect all SOM paths of descendant interactive fields under a container.
///
/// "Interactive" here means the path appears in `field_paths` (the set of
/// all selectable field paths from the global field order).
fn descendant_interactive_fields(
    container_path: &SomPath,
    field_paths: &HashSet<SomPath>,
) -> Vec<SomPath> {
    let prefix = format!("{}.", container_path.as_str());
    field_paths
        .iter()
        .filter(|p| p.as_str().starts_with(&prefix))
        .cloned()
        .collect()
}

/// Build independent field partitions from the global field order.
///
/// Analyzes the script source text of every interactive field's change/click
/// scripts to build a dependency graph. Fields whose scripts reference the
/// same containers or fields are unioned into the same partition. Fields in
/// different partitions are guaranteed to be independent and can be explored
/// separately.
///
/// Script object calls (e.g. `soLocalLabelDefinition.change()`) are resolved
/// transitively: if a field's script calls a script object, all field
/// references inside that script object's source are treated as dependencies
/// of the calling field.
///
/// Returns groups of indices into `global_field_order`. Each group is an
/// independent partition that can be explored in isolation.
fn partition_fields(
    global_field_order: &[SelectableField],
    excl_group_map: &HashMap<SomPath, Option<SomPath>>,
    radio_group_indices: &HashMap<SomPath, Vec<usize>>,
    script_registry: &ScriptRegistry,
    som_resolver: &SomResolver,
    script_objects: &HashMap<String, String>,
) -> Vec<Vec<usize>> {
    let n = global_field_order.len();
    if n == 0 {
        return Vec::new();
    }

    let mut uf = UnionFind::new(n);

    // Build a set of all field paths for quick membership tests
    let field_paths: HashSet<SomPath> = global_field_order.iter().map(|f| f.path.clone()).collect();

    // Build path→index lookup
    let path_to_idx: HashMap<&SomPath, usize> = global_field_order
        .iter()
        .enumerate()
        .map(|(i, f)| (&f.path, i))
        .collect();

    // Union all radio buttons in the same exclGroup
    for indices in radio_group_indices.values() {
        if indices.len() > 1 {
            let first = indices[0];
            for &idx in &indices[1..] {
                uf.union(first, idx);
            }
        }
    }

    // For each selectable field, analyze its change/click scripts with
    // transitive script-object resolution
    for (idx, field) in global_field_order.iter().enumerate() {
        // Determine the script owner: for radios it's the exclGroup, for others it's self
        let script_owner = if field.is_radio() {
            excl_group_map
                .get(&field.path)
                .and_then(|eg| eg.as_ref())
                .cloned()
                .unwrap_or_else(|| field.path.clone())
        } else {
            field.path.clone()
        };

        // Analyze Change and Click scripts for this owner
        for activity in &[EventActivity::Change, EventActivity::Click] {
            let scripts = script_registry.get_event_scripts(&script_owner, activity);
            for registered_script in scripts {
                // Use transitive extraction to follow script object calls
                let refs = extract_field_references_transitive(
                    &registered_script.script.source,
                    script_objects,
                );

                for field_ref in &refs {
                    // Skip script object names themselves — they are not XFA fields
                    if script_objects.contains_key(field_ref.name.as_str()) {
                        continue;
                    }

                    // Use context-aware scoped resolution instead of global
                    // name lookup. This ensures that `RB_Group` in section A's
                    // script resolves to section A's exclGroup, not section B's.
                    let target_path =
                        match som_resolver.resolve_unqualified(&field_ref.name, &script_owner) {
                            Some(p) => p,
                            None => continue,
                        };

                    // Case 1: target is directly a selectable field
                    if let Some(&target_idx) = path_to_idx.get(&target_path) {
                        uf.union(idx, target_idx);
                    }

                    // Case 2: target is a container whose presence is changed —
                    // only then do we union with ALL descendant interactive
                    // fields (hiding/showing a container affects all children).
                    // For .rawValue/.value refs to containers, the container
                    // itself is not a selectable field, so no union is needed.
                    if field_ref.kind == RefKind::Presence {
                        let descendants = descendant_interactive_fields(&target_path, &field_paths);
                        for desc in &descendants {
                            if let Some(&desc_idx) = path_to_idx.get(desc) {
                                uf.union(idx, desc_idx);
                            }
                        }
                    }

                    // Case 3: target is an exclGroup that appears in radio_group_indices
                    if let Some(rg_indices) = radio_group_indices.get(&target_path) {
                        for &rg_idx in rg_indices {
                            uf.union(idx, rg_idx);
                        }
                    }
                }
            }
        }
    }

    uf.groups(n)
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
    // OPTIMIZATION: Cache post-init nodes and computed values from the already-
    // initialised form. Branches use `XfaForm::from_post_init` which skips the
    // expensive `ScriptExecutor::execute()` phase (saves one Boa JS context
    // creation + all init-script phases per branch).
    let post_init_nodes = Arc::new(form.xfa_nodes().to_vec());
    let init_values = Arc::new(form.current_field_values());

    // OPTIMIZATION (Step 2): Share the script registry across all branches via Arc.
    // This avoids two full tree walks per `from_post_init` call.
    let script_registry = form.script_registry_arc();

    // Establish the global field ordering from the initial form state
    // Only includes fields with interactive scripts (change, click, calculate)
    let global_field_order = get_all_selectable_fields_ordered(form);

    // OPTIMIZATION (Step 1): Precompute excl-group lookups for every selectable field.
    // `find_excl_group_for_field` does an O(tree-depth) walk each time — here we pay
    // that cost once per field instead of O(N × options) times during exploration.
    let mut excl_group_map: HashMap<SomPath, Option<SomPath>> = HashMap::new();
    let mut radio_group_indices: HashMap<SomPath, Vec<usize>> = HashMap::new();

    for (idx, field) in global_field_order.iter().enumerate() {
        let excl = form.find_excl_group_for_field(field.path.as_str());
        if let Some(ref eg) = excl {
            radio_group_indices.entry(eg.clone()).or_default().push(idx);
        }
        excl_group_map.insert(field.path.clone(), excl);
    }

    // ── Independent field partitioning ──────────────────────────────────
    // Statically analyse script sources to partition fields into independent
    // groups. Fields whose scripts never reference each other (directly or
    // transitively) can be explored separately: we explore each group in
    // isolation and then combine the per-group results via cross-product,
    // reducing exponential blowup from ∏kᵢ to Σkᵢ + combination.
    let som_resolver = SomResolver::from_nodes(&post_init_nodes);
    let script_objects = collect_script_object_sources(&post_init_nodes);
    let groups = partition_fields(
        &global_field_order,
        &excl_group_map,
        &radio_group_indices,
        &script_registry,
        &som_resolver,
        &script_objects,
    );

    log::info!(
        "[exhaustive] {} selectable fields → {} independent group(s): {}",
        global_field_order.len(),
        groups.len(),
        groups
            .iter()
            .map(|g| g.len().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // If a single group (or zero/one fields), fall through to the original
    // algorithm — no partitioning benefit.
    if groups.len() <= 1 {
        return collect_states_single_group(
            form,
            global_field_order,
            excl_group_map,
            radio_group_indices,
            script_registry,
            post_init_nodes,
            init_values,
        );
    }

    // ── Per-group exploration ───────────────────────────────────────────
    // Explore each independent group in isolation. Each group gets its own
    // sub-field-order (keeping the original indices so field_actions align).
    let mut per_group_results: Vec<Vec<CollectedState>> = Vec::with_capacity(groups.len());

    for group_indices in &groups {
        // Build a sub-field-order containing only this group's fields.
        // We keep the same global_field_order length for ExplorationState but
        // only visit fields in this group — all others are pre-marked as Skipped.
        let group_set: HashSet<usize> = group_indices.iter().copied().collect();

        // Build group-local radio_group_indices (only include radio groups
        // whose members are entirely within this partition)
        let group_radio_indices: HashMap<SomPath, Vec<usize>> = radio_group_indices
            .iter()
            .filter(|(_, indices)| indices.iter().all(|i| group_set.contains(i)))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let collected_states = Arc::new(Mutex::new(Vec::<CollectedState>::new()));
        let rendered_states = Arc::new(Mutex::new(HashSet::<Vec<Option<FieldAction>>>::new()));
        let seen_layouts: SeenLayouts = Arc::new(Mutex::new(HashMap::new()));

        let ctx = ExplorationContext {
            global_field_order: global_field_order.clone(),
            excl_group_map: excl_group_map.clone(),
            radio_group_indices: group_radio_indices,
            script_registry: script_registry.clone(),
            post_init_nodes: post_init_nodes.clone(),
            init_values: init_values.clone(),
            rendered_states,
            collected_states: collected_states.clone(),
            seen_layouts,
        };

        // Pre-mark all fields NOT in this group as Skipped so the linear
        // explorer only visits this group's fields.
        let mut initial_state = ExplorationState::new(global_field_order.len());
        for idx in 0..global_field_order.len() {
            if !group_set.contains(&idx) {
                initial_state.field_actions[idx] = Some(FieldAction::Skipped);
            }
        }

        // Reset form to initial state for this group exploration
        form.reset_for_branch(post_init_nodes.as_ref().clone(), &init_values)
            .map_err(crate::Error::FormCreation)?;

        collect_states_linear(form, initial_state, &ctx)?;

        let states = Arc::try_unwrap(collected_states)
            .map(|mutex| mutex.into_inner().unwrap())
            .unwrap_or_else(|arc| arc.lock().unwrap().clone());

        per_group_results.push(states);
    }

    // ── Cross-product combination ──────────────────────────────────────
    // Combine per-group results: for each combination of per-group states,
    // apply all selections to a fresh form via reset_for_branch, capture
    // the final flattened layout, and dedup by FlattenedKey.
    let combined = combine_group_results(
        form,
        &per_group_results,
        &post_init_nodes,
        &init_values,
        &script_registry,
    )?;

    Ok(combined)
}

/// Original single-group exploration (no partitioning).
fn collect_states_single_group(
    form: &mut XfaForm,
    global_field_order: Vec<SelectableField>,
    excl_group_map: HashMap<SomPath, Option<SomPath>>,
    radio_group_indices: HashMap<SomPath, Vec<usize>>,
    script_registry: Arc<ScriptRegistry>,
    post_init_nodes: Arc<Vec<XfaNode>>,
    init_values: Arc<HashMap<SomPath, String>>,
) -> Result<Vec<CollectedState>, crate::Error> {
    let collected_states = Arc::new(Mutex::new(Vec::<CollectedState>::new()));
    let rendered_states = Arc::new(Mutex::new(HashSet::<Vec<Option<FieldAction>>>::new()));
    let seen_layouts: SeenLayouts = Arc::new(Mutex::new(HashMap::new()));

    let ctx = ExplorationContext {
        global_field_order,
        excl_group_map,
        radio_group_indices,
        script_registry,
        post_init_nodes,
        init_values,
        rendered_states,
        collected_states: collected_states.clone(),
        seen_layouts,
    };

    let initial_state = ExplorationState::new(ctx.global_field_order.len());
    collect_states_linear(form, initial_state, &ctx)?;

    let states = Arc::try_unwrap(collected_states)
        .map(|mutex| mutex.into_inner().unwrap())
        .unwrap_or_else(|arc| arc.lock().unwrap().clone());

    Ok(states)
}

/// Combine per-group exploration results via cross-product.
///
/// For each combination of per-group states (one state per group), apply all
/// selections from every group to a single form, refresh it, and capture the
/// combined flattened layout. Dedup by FlattenedKey to avoid emitting
/// duplicate states.
fn combine_group_results(
    form: &mut XfaForm,
    per_group_results: &[Vec<CollectedState>],
    post_init_nodes: &Arc<Vec<XfaNode>>,
    init_values: &Arc<HashMap<SomPath, String>>,
    _script_registry: &Arc<ScriptRegistry>,
) -> Result<Vec<CollectedState>, crate::Error> {
    // Compute cross-product indices
    let group_sizes: Vec<usize> = per_group_results.iter().map(|g| g.len().max(1)).collect();
    let total_combos: usize = group_sizes.iter().product();

    let mut seen_keys: HashSet<Vec<FlattenedKey>> = HashSet::new();
    let mut combined: Vec<CollectedState> = Vec::new();

    for combo_idx in 0..total_combos {
        // Decode combo_idx into per-group state indices
        let mut remaining = combo_idx;
        let mut group_state_indices: Vec<usize> = Vec::with_capacity(per_group_results.len());
        for &size in group_sizes.iter().rev() {
            group_state_indices.push(remaining % size);
            remaining /= size;
        }
        group_state_indices.reverse();

        // Merge selections and field_actions from all groups
        let mut merged_selections: Vec<Selection> = Vec::new();
        let mut merged_field_actions: Vec<Option<FieldAction>> = Vec::new();

        for (group_idx, &state_idx) in group_state_indices.iter().enumerate() {
            if per_group_results[group_idx].is_empty() {
                continue;
            }
            let group_state = &per_group_results[group_idx][state_idx];
            merged_selections.extend(group_state.selections.iter().cloned());

            // On first group, initialise merged_field_actions from its actions
            if merged_field_actions.is_empty() {
                merged_field_actions = group_state.field_actions.clone();
            } else {
                // Overlay this group's non-None actions onto the merged vector
                for (i, action) in group_state.field_actions.iter().enumerate() {
                    if action.is_some() {
                        merged_field_actions[i] = action.clone();
                    }
                }
            }
        }

        // Apply merged selections to a fresh form
        form.reset_for_branch(post_init_nodes.as_ref().clone(), init_values)
            .map_err(crate::Error::FormCreation)?;

        // Replay all selections on the form
        for sel in &merged_selections {
            match sel.kind {
                SelectionKind::Radio => {
                    let _ = form.select_radio_button(sel.som_path.as_str());
                }
                SelectionKind::Checkbox => {
                    let _ = form.set_value_as_user(sel.som_path.as_str(), sel.primary_value());
                }
                SelectionKind::Dropdown => {
                    let _ = form.set_value_as_user(sel.som_path.as_str(), sel.primary_value());
                }
            }
        }

        form.refresh().map_err(crate::Error::FormCreation)?;

        // Dedup by flattened key
        let flattened_key = form.flattened_mut().flattened_key().clone();
        if seen_keys.contains(&flattened_key) {
            continue;
        }
        seen_keys.insert(flattened_key);

        let label = generate_label(&merged_selections);
        let flattened = form.flattened().clone();

        combined.push(CollectedState {
            flattened,
            selections: merged_selections,
            label,
            field_actions: merged_field_actions,
        });
    }

    Ok(combined)
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
    ctx: &ExplorationContext,
) -> Result<(), crate::Error> {
    // Check if this is a complete state (all fields processed)
    if exploration_state.is_complete() {
        let state_key = exploration_state.state_key();

        // Skip if we've already collected this exact state (thread-safe check)
        {
            let mut states = ctx.rendered_states.lock().unwrap();
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
        ctx.collected_states.lock().unwrap().push(CollectedState {
            flattened,
            selections: exploration_state.selections.clone(),
            label,
            field_actions: exploration_state.field_actions.clone(),
        });

        return Ok(());
    }

    // Process the next field
    let field_index = exploration_state.next_field_index;
    let field = &ctx.global_field_order[field_index];

    // If this field was pre-marked as Skipped (e.g. during per-partition
    // exploration where only a subset of fields is active), advance
    // immediately without evaluating can_select or entering explore_*.
    if exploration_state.field_actions[field_index] == Some(FieldAction::Skipped) {
        let mut new_state = exploration_state;
        new_state.next_field_index = field_index + 1;
        return collect_states_linear(form, new_state, ctx);
    }

    // Check if field can be selected
    let can_select = can_select_field(form, field, &exploration_state.selections, ctx);

    // If field cannot be selected, automatically skip it and continue
    if !can_select {
        let mut new_state = exploration_state.clone();
        new_state.field_actions[field_index] = Some(FieldAction::Skipped);
        new_state.next_field_index = field_index + 1;

        return collect_states_linear(form, new_state, ctx);
    }

    match &field.kind {
        SelectableFieldKind::Radio => {
            explore_radio(form, exploration_state, field_index, field, ctx)?;
        }
        SelectableFieldKind::Checkbox => {
            explore_checkbox(form, exploration_state, field_index, field, ctx)?;
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
                return collect_states_linear(form, new_state, ctx);
            }
            explore_dropdown(form, exploration_state, field_index, field, &options, ctx)?;
        }
    }

    Ok(())
}

// ============================================================================
// Branch preparation (sequential, engine-reusing)
// ============================================================================

/// A prepared branch: the result of applying a mutation and refreshing the form,
/// before any further recursive exploration.
///
/// Stores the post-refresh XFA node snapshot and field values so the form can
/// be cheaply reset to this state via `reset_for_branch`.
struct PreparedBranch {
    /// Structural key of the flattened layout after refresh — used for deduplication.
    /// Captures position, dimensions, text content, and field names/labels while
    /// excluding field values and checked state.
    flattened_key: Vec<FlattenedKey>,
    /// Post-mutation XFA node snapshot — used to reset the form for recursion.
    snapshot_nodes: Vec<XfaNode>,
    /// Post-mutation field values — used together with snapshot_nodes.
    snapshot_values: HashMap<SomPath, String>,
    /// The exploration state at this point (selections, field_actions, etc.)
    state: ExplorationState,
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
///
/// OPTIMIZATION: Reuses the passed `form` via `reset_for_branch` instead of
/// creating a new `XfaForm::from_post_init` for each representative. This
/// avoids creating a new Boa JS engine per representative, saving ~25-40ms
/// each time.
fn explore_with_dedup(
    form: &mut XfaForm,
    branches: Vec<PreparedBranch>,
    ctx: &ExplorationContext,
) -> Result<(), crate::Error> {
    // Group branches by identical flattened layout.
    let groups = group_branches_by_flattened_state(branches);

    for mut group in groups {
        let mut representative = group.remove(0);

        // Record the values from duplicate branches on the representative's
        // last selection so the merger can later emit one conditional per value.
        if !group.is_empty() {
            if let Some(rep_sel) = representative.state.selections.last_mut() {
                for dup in &group {
                    if let Some(dup_sel) = dup.state.selections.last() {
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

        let composite_key = (
            representative.flattened_key.clone(),
            representative.state.next_field_index,
        );

        {
            let mut seen = ctx.seen_layouts.lock().unwrap();
            if seen.contains_key(&composite_key) {
                continue;
            }
            seen.insert(composite_key, representative.state.selections.clone());
        }

        let depth_snapshot_nodes = Arc::new(representative.snapshot_nodes);
        let depth_snapshot_values = Arc::new(representative.snapshot_values);

        form.reset_for_branch(
            depth_snapshot_nodes.as_ref().clone(),
            &depth_snapshot_values,
        )
        .map_err(crate::Error::FormCreation)?;

        // Build a temporary context with the depth-specific snapshots
        let depth_ctx = ExplorationContext {
            global_field_order: ctx.global_field_order.clone(),
            excl_group_map: ctx.excl_group_map.clone(),
            radio_group_indices: ctx.radio_group_indices.clone(),
            script_registry: ctx.script_registry.clone(),
            post_init_nodes: depth_snapshot_nodes,
            init_values: depth_snapshot_values,
            rendered_states: ctx.rendered_states.clone(),
            collected_states: ctx.collected_states.clone(),
            seen_layouts: ctx.seen_layouts.clone(),
        };

        collect_states_linear(form, representative.state, &depth_ctx)?;
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
    // Collect into a Vec sorted by the minimum original index in each group
    // to ensure deterministic processing order (HashMap iteration is random).
    let mut sorted_entries: Vec<(usize, Vec<usize>)> = key_to_group
        .into_values()
        .map(|indices| {
            let min_idx = *indices.iter().min().unwrap();
            (min_idx, indices)
        })
        .collect();
    sorted_entries.sort_by_key(|(min_idx, _)| *min_idx);

    for (_, indices) in sorted_entries {
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
///
/// Phase A: prepares one branch per radio option in parallel using rayon.
/// Each rayon task creates its own `XfaForm` via `from_post_init`.
/// Phase B: dedup + recurse via `explore_with_dedup` (reuses the passed form).
fn explore_radio(
    form: &mut XfaForm,
    exploration_state: ExplorationState,
    _field_index: usize,
    field: &SelectableField,
    ctx: &ExplorationContext,
) -> Result<(), crate::Error> {
    // OPTIMIZATION (Step 1): Use precomputed excl-group map instead of walking the tree
    if let Some(Some(excl_group_path)) = ctx.excl_group_map.get(&field.path) {
        // OPTIMIZATION (Step 1): Use precomputed radio-group indices
        let group_field_indices = ctx
            .radio_group_indices
            .get(excl_group_path)
            .cloned()
            .unwrap_or_default();

        let group_fields: Vec<SelectableField> = group_field_indices
            .iter()
            .map(|&idx| ctx.global_field_order[idx].clone())
            .collect();

        // Phase A: prepare branches in parallel using rayon thread pool
        // OPTIMIZATION (Step 2): Share the script registry via Arc
        #[cfg(not(target_arch = "wasm32"))]
        let iter = group_fields.par_iter().enumerate();
        #[cfg(target_arch = "wasm32")]
        let iter = group_fields.iter().enumerate();
        let branches: Result<Vec<PreparedBranch>, crate::Error> = iter
            .map(|(option_index, radio_field)| {
                let nodes = ctx.post_init_nodes.as_ref().clone();
                let mut new_form = XfaForm::from_post_init_with_registry(
                    nodes,
                    &ctx.init_values,
                    ctx.script_registry.clone(),
                )
                .map_err(crate::Error::FormCreation)?;

                let _ = new_form.select_radio_button(radio_field.path.as_str());

                let mut state = exploration_state.clone();
                // OPTIMIZATION (Step 1): Use precomputed excl-group map
                let group_path = ctx.excl_group_map.get(&radio_field.path).cloned().flatten();
                state.selections.push(Selection::new_with_index(
                    radio_field.path.clone(),
                    group_path.clone(),
                    radio_field.path.name().to_string(),
                    SelectionKind::Radio,
                    option_index,
                ));

                // Mark all fields in this radio group as processed
                // OPTIMIZATION (Step 1): Use precomputed radio-group indices
                let sel_group_id = group_path.as_ref().map(FieldId::from_som_path);
                for &idx in &group_field_indices {
                    let f = &ctx.global_field_order[idx];
                    let f_group = ctx.excl_group_map.get(&f.path).and_then(|g| g.as_ref());
                    if f.is_radio() && f_group.map(FieldId::from_som_path) == sel_group_id {
                        state.field_actions[idx] = if f.path == radio_field.path {
                            Some(FieldAction::Selected(radio_field.path.name().to_string()))
                        } else {
                            Some(FieldAction::Skipped)
                        };
                        state.next_field_index = idx + 1;
                    }
                }

                new_form.refresh().map_err(crate::Error::FormCreation)?;

                // OPTIMIZATION (Step 6): Use cached flattened key
                let flattened_key = new_form.flattened_mut().flattened_key().clone();
                let snapshot_nodes = new_form.xfa_nodes().to_vec();
                let snapshot_values = new_form.current_field_values();

                Ok(PreparedBranch {
                    flattened_key,
                    snapshot_nodes,
                    snapshot_values,
                    state,
                })
            })
            .collect();

        // Phase B: group by identical XFA state and recurse
        explore_with_dedup(form, branches?, ctx)?;
    }

    Ok(())
}

/// Explore both checked and unchecked states of a checkbox.
///
/// Phase A: prepares two branches (checked/unchecked) in parallel using rayon.
/// Phase B: dedup + recurse via `explore_with_dedup` (reuses the passed form).
fn explore_checkbox(
    form: &mut XfaForm,
    exploration_state: ExplorationState,
    field_index: usize,
    field: &SelectableField,
    ctx: &ExplorationContext,
) -> Result<(), crate::Error> {
    // Per XFA 3.3 §17 pp.758-759: use <items> on/off values when available.
    // The items list can have up to 3 values: on (1st), off (2nd), neutral (3rd).
    // Fall back to "1"/"0" when no <items> are defined (common Acrobat convention).
    let (on_val, off_val) = form
        .resolve(field.path.as_str())
        .map(|node| node.xfa_node().extract_item_values())
        .unwrap_or((None, None));
    let on_value = on_val.unwrap_or_else(|| "1".to_string());
    let off_value = off_val.unwrap_or_else(|| "0".to_string());
    let checkbox_values: Vec<(&str, &str)> =
        vec![(&on_value, "checked"), (&off_value, "unchecked")];

    // Phase A: prepare branches in parallel using rayon thread pool
    // OPTIMIZATION (Step 2): Share the script registry via Arc
    #[cfg(not(target_arch = "wasm32"))]
    let iter = checkbox_values.par_iter().enumerate();
    #[cfg(target_arch = "wasm32")]
    let iter = checkbox_values.iter().enumerate();
    let branches: Result<Vec<PreparedBranch>, crate::Error> = iter
        .map(|(option_index, (raw_value, label))| {
            let nodes = ctx.post_init_nodes.as_ref().clone();
            let mut new_form = XfaForm::from_post_init_with_registry(
                nodes,
                &ctx.init_values,
                ctx.script_registry.clone(),
            )
            .map_err(crate::Error::FormCreation)?;

            let _ = new_form.set_value_as_user(field.path.as_str(), raw_value);

            let mut state = exploration_state.clone();
            state.selections.push(Selection::standalone_with_index(
                field.path.clone(),
                label.to_string(),
                SelectionKind::Checkbox,
                option_index,
            ));
            state.field_actions[field_index] = Some(FieldAction::Selected(label.to_string()));
            state.next_field_index = field_index + 1;

            new_form.refresh().map_err(crate::Error::FormCreation)?;

            // OPTIMIZATION (Step 6): Use cached flattened key
            let flattened_key = new_form.flattened_mut().flattened_key().clone();
            let snapshot_nodes = new_form.xfa_nodes().to_vec();
            let snapshot_values = new_form.current_field_values();

            Ok(PreparedBranch {
                flattened_key,
                snapshot_nodes,
                snapshot_values,
                state,
            })
        })
        .collect();

    // Phase B: group by identical XFA state and recurse
    explore_with_dedup(form, branches?, ctx)?;

    Ok(())
}

/// Explore all options of a dropdown field.
///
/// Phase A: prepares one branch per option in parallel using rayon.
/// Phase B: dedup + recurse via `explore_with_dedup` (reuses the passed form).
fn explore_dropdown(
    form: &mut XfaForm,
    exploration_state: ExplorationState,
    field_index: usize,
    field: &SelectableField,
    options: &[(String, String)],
    ctx: &ExplorationContext,
) -> Result<(), crate::Error> {
    // Phase A: prepare branches in parallel using rayon thread pool
    // OPTIMIZATION (Step 2): Share the script registry via Arc
    #[cfg(not(target_arch = "wasm32"))]
    let iter = options.par_iter().enumerate();
    #[cfg(target_arch = "wasm32")]
    let iter = options.iter().enumerate();
    let branches: Result<Vec<PreparedBranch>, crate::Error> = iter
        .map(|(option_index, (display_value, save_value))| {
            let nodes = ctx.post_init_nodes.as_ref().clone();
            let mut new_form = XfaForm::from_post_init_with_registry(
                nodes,
                &ctx.init_values,
                ctx.script_registry.clone(),
            )
            .map_err(crate::Error::FormCreation)?;

            let _ = new_form.set_value_as_user(field.path.as_str(), save_value);

            let mut state = exploration_state.clone();
            state.selections.push(Selection::standalone_with_index(
                field.path.clone(),
                display_value.clone(),
                SelectionKind::Dropdown,
                option_index,
            ));
            state.field_actions[field_index] = Some(FieldAction::Selected(save_value.clone()));
            state.next_field_index = field_index + 1;

            new_form.refresh().map_err(crate::Error::FormCreation)?;

            // OPTIMIZATION (Step 6): Use cached flattened key
            let flattened_key = new_form.flattened_mut().flattened_key().clone();
            let snapshot_nodes = new_form.xfa_nodes().to_vec();
            let snapshot_values = new_form.current_field_values();

            Ok(PreparedBranch {
                flattened_key,
                snapshot_nodes,
                snapshot_values,
                state,
            })
        })
        .collect();

    // Phase B: group by identical XFA state and recurse
    explore_with_dedup(form, branches?, ctx)?;

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
    ctx: &ExplorationContext,
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
    // OPTIMIZATION (Step 1): Use precomputed excl-group map
    if field.is_radio() {
        if let Some(Some(excl_group)) = ctx.excl_group_map.get(&field.path) {
            let excl_group_id = FieldId::from_som_path(excl_group);
            let group_already_has_selection = current_selections
                .iter()
                .any(|sel| sel.group_path.as_ref() == Some(&excl_group_id));
            if group_already_has_selection {
                return false;
            }
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
        // Per XFA 3.3 §17: skip fields whose access (or parent exclGroup's access)
        // prevents user interaction.
        // - "protected": no events generated at all.
        // - "readOnly": no direct user changes allowed.
        // - "nonInteractive": behaves as rendering to paper.
        // Only "open" (the default) allows full user interaction.
        if let Some(resolved) = form.resolve(field.path.as_str()) {
            let access = resolved
                .xfa_node()
                .attributes
                .get("access")
                .map(|s| s.as_str());
            if matches!(access, Some("protected" | "readOnly" | "nonInteractive")) {
                return false;
            }
        }

        // For radio/checkbox in exclGroup, also check the parent exclGroup's access
        if field.is_radio() || matches!(field.kind, SelectableFieldKind::Checkbox) {
            if let Some(excl_group_path) = form.find_excl_group_for_field(field.path.as_str()) {
                if let Some(eg) = form.resolve(excl_group_path.as_str()) {
                    let eg_access = eg.xfa_node().attributes.get("access").map(|s| s.as_str());
                    if matches!(eg_access, Some("protected" | "readOnly" | "nonInteractive")) {
                        return false;
                    }
                }
            }
        }

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
