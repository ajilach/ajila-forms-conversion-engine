//! Script Executor - Handles XFA script execution as a separate concern from flattening.
//!
//! This module extracts script execution logic from the Flattened module to maintain
//! a clean separation of concerns:
//! - `ScriptExecutor` handles all side effects (script execution, presence changes)
//! - `Flattened` remains a pure transformation from XFA tree to absolute positions
//!
//! # Architecture
//!
//! ```text
//! XFA Nodes (immutable) ──► ScriptExecutor ──► ScriptExecutionResult
//!                                │                    │
//!                                │                    ├─ computed_values
//!                                │                    └─ presence_changes
//!                                ▼
//!                          XFA Nodes (cloned + modified)
//!                                │
//!                                ▼
//!                           Flattened (pure)
//! ```

use crate::xfa::scripting::{
    EventActivity, EventRef, ListenScope, Presence, ScriptContentType, SomPath, XfaScript,
    XfaScriptEngine, parse_events_from_node, wrap_script_object,
};
use crate::xfa::{XfaNode, XfaNodeKind};
use std::collections::HashMap;

/// A pair of child name and child ID.
type ChildNameIdPair = (String, String);

/// Represents an event with its associated metadata:
/// - name: The node name
/// - full_path: The full SOM path
/// - children: List of child (name, id) pairs
/// - script: The XFA script to execute
/// - presence: The node's presence value
type EventWithChildren = (String, String, Vec<ChildNameIdPair>, XfaScript, Presence);

/// Result of script execution containing computed values and presence changes.
#[derive(Debug, Clone, Default)]
pub struct ScriptExecutionResult {
    /// Computed field values from script execution (field name/path -> value)
    pub computed_values: HashMap<SomPath, String>,
    /// Presence changes to apply to nodes (name, optional id, presence)
    pub presence_changes: Vec<(String, Option<String>, Presence)>,
}

/// Executes XFA scripts and collects their results without mutating the input tree.
pub struct ScriptExecutor;

impl ScriptExecutor {
    /// Execute all form-ready scripts and return the results.
    ///
    /// This function does NOT mutate the input nodes. Instead, it returns
    /// a `ScriptExecutionResult` containing:
    /// - `computed_values`: Field values computed by scripts
    /// - `presence_changes`: Presence changes that should be applied to nodes
    ///
    /// The caller is responsible for applying presence changes to a cloned tree.
    ///
    /// # Arguments
    /// * `xfa_nodes` - The XFA node tree (read-only)
    ///
    /// # Returns
    /// `ScriptExecutionResult` on success, or prints a warning and returns default on failure.
    pub fn execute(xfa_nodes: &[XfaNode]) -> ScriptExecutionResult {
        match Self::execute_internal(xfa_nodes) {
            Ok(result) => result,
            Err(e) => {
                eprintln!(
                    "Warning: Script execution failed: {}. Continuing without script results.",
                    e
                );
                ScriptExecutionResult::default()
            }
        }
    }

    /// Internal implementation that can return errors.
    fn execute_internal(xfa_nodes: &[XfaNode]) -> Result<ScriptExecutionResult, String> {
        let mut computed_values = HashMap::new();
        let mut presence_changes: Vec<(String, Option<String>, Presence)> = Vec::new();
        let mut engine = XfaScriptEngine::new();

        // Extract and register translation objects from the XFA
        // (includes Footer_Line_txtlanguage, Footer_Line_txtformid, etc.)
        Self::extract_and_register_translations(xfa_nodes, &mut engine);

        // Build the XFA SOM hierarchy for unqualified references
        Self::build_and_register_xfa_som_hierarchy(xfa_nodes, &mut engine);

        // Build parent-child map for setting up `this.childField` access
        let parent_child_map = Self::build_parent_child_map_with_ids(xfa_nodes);

        // Find all events recursively, starting from root
        let mut subform_counters: HashMap<String, usize> = HashMap::new();
        let mut all_events = Vec::new();
        Self::find_all_events_with_child_ids(
            xfa_nodes,
            &mut all_events,
            &parent_child_map,
            &mut subform_counters,
            None, // Start with no parent path
        );

        // Warn about FormCalc scripts that cannot be executed.
        // Only JavaScript is supported; FormCalc scripts are silently skipped.
        let formcalc_count = all_events
            .iter()
            .filter(|(_, _, _, script, _)| script.content_type == ScriptContentType::FormCalc)
            .count();
        if formcalc_count > 0 {
            eprintln!(
                "Warning: {} FormCalc script(s) found but not executed \
                 (only JavaScript is supported). Results may be incomplete.",
                formcalc_count
            );
        }

        // Register all event scripts in the _xfa_event_scripts_ registry
        // so that execEvent() can find them at runtime.
        // Per XFA 3.3 §10 pp.407-409 Rule 3.
        for (_, full_path, _, script, _) in &all_events {
            if script.content_type == ScriptContentType::JavaScript {
                engine.register_event_script(
                    full_path,
                    script.activity.activity_name(),
                    &script.source,
                );
            }
        }

        // Phase 0: Execute calculate scripts with convergence loop
        // Per XFA 3.3 §10 p.407: "All value calculations are done, then all
        // property calculations, then all validations, and then all initialize
        // events are fired. Calculations are repeated if the values on which
        // they depend change."
        //
        // Per XFA 3.3 §10 p.380: "In cascading calculations the processing
        // application re-activates calculate objects as the values upon which
        // they depend change."
        //
        // Per XFA 3.3 §10 p.380 on circular references: "It is recommended
        // that the processing application provide some means of identifying
        // and terminating the execution of seemingly infinite loops."
        let max_calc_iterations: usize = std::env::var("XFA_MAX_CALC_ITERATIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(25);

        // Collect calculate events for the convergence loop
        let calc_events: Vec<&EventWithChildren> = all_events
            .iter()
            .filter(|(_, _, _, script, _)| {
                script.content_type == ScriptContentType::JavaScript
                    && script.activity == EventActivity::Calculate
            })
            .collect();

        for iteration in 0..max_calc_iterations {
            let mut any_changed = false;

            for (field_name, full_path, child_fields, script, _presence) in &calc_events {
                engine.set_current_field_with_children(full_path, field_name, "", child_fields);
                engine.update_event_context(&EventActivity::Calculate, full_path);

                let old_value = computed_values.get(&SomPath::new(full_path.clone())).cloned();

                if let Ok(Some(value)) = engine.execute_script(script) {
                    // Per XFA 3.3 §10 p.380: the calculate result replaces the
                    // container's value.
                    if old_value.as_ref() != Some(&value) {
                        any_changed = true;
                    }
                    computed_values.insert(SomPath::new(field_name.clone()), value.clone());
                    computed_values.insert(SomPath::new(full_path.clone()), value);
                }

                // Collect values set on fields during calculation (side effects)
                let calc_som_values = engine.get_all_som_field_values();
                for (calc_field_name, calc_value) in calc_som_values {
                    let key = SomPath::new(calc_field_name);
                    if computed_values.get(&key) != Some(&calc_value) {
                        any_changed = true;
                    }
                    computed_values.insert(key, calc_value);
                }
            }

            if !any_changed {
                break;
            }

            if iteration == max_calc_iterations - 1 {
                eprintln!(
                    "Warning: Calculation convergence not reached after {} iterations \
                     (possible circular dependency). Results may be incorrect.",
                    max_calc_iterations
                );
            }
        }

        // Phase 0b: Execute validate scripts
        // Per XFA 3.3 §10 p.407: validations run after calculations, before
        // initialize events. Validate scripts are executed but their results
        // are informational (validation state), not value-replacing.
        // However, validate scripts may set rawValue as a side effect.
        for (field_name, full_path, child_fields, script, _presence) in &all_events {
            if script.content_type == ScriptContentType::JavaScript
                && script.activity == EventActivity::Validate
            {
                engine.set_current_field_with_children(full_path, field_name, "", child_fields);
                engine.update_event_context(&EventActivity::Validate, full_path);
                let _ = engine.execute_script(script);

                // Collect any side-effect values set during validation
                let validate_som_values = engine.get_all_som_field_values();
                for (vf_name, vf_value) in validate_som_values {
                    computed_values.insert(SomPath::new(vf_name), vf_value);
                }
            }
        }

        // Phase 1: Execute initialize events
        // Per XFA 3.3 §10 p.407 Rule 1: "The property is inspected at the
        // moment that the calculation, validation, or event would be triggered."
        // We maintain a running presence map so that if an earlier init script
        // sets a sibling's presence to inactive, subsequent init events for
        // that sibling are suppressed within the same phase.
        let mut init_dynamic_presence: HashMap<String, Presence> = HashMap::new();
        for (field_name, full_path, child_fields, script, static_presence) in &all_events {
            if script.content_type == ScriptContentType::JavaScript
                && script.activity == EventActivity::Initialize
                && !Self::is_effectively_inactive(
                    full_path,
                    *static_presence,
                    &init_dynamic_presence,
                )
            {
                engine.set_current_field_with_children(full_path, field_name, "", child_fields);
                engine.update_event_context(&EventActivity::Initialize, full_path);

                let result = engine.execute_script(script);
                if let Err(ref e) = result
                    && std::env::var("XFA_DEBUG").is_ok()
                {
                    eprintln!("[INIT ERR] field={field_name} path={full_path}: {e}");
                }
                let _ = result;

                // Collect presence values set on the current field
                if let Some(presence) = engine.get_current_field_presence() {
                    presence_changes.push((field_name.clone(), None, presence));
                }

                // Collect presence values set on child fields
                for (child_name, child_id) in child_fields {
                    if let Some((id, presence)) = engine.get_child_field_presence(child_name) {
                        let storage_id = if !id.is_empty() {
                            Some(id)
                        } else if !child_id.is_empty() {
                            Some(child_id.clone())
                        } else {
                            None
                        };
                        presence_changes.push((child_name.clone(), storage_id, presence));
                    }
                }

                // Collect values from initialize scripts
                // Empty strings are valid per XFA spec (cleared fields, deselected exclGroups)
                let init_som_values = engine.get_all_som_field_values();
                for (init_field_name, init_value) in init_som_values {
                    computed_values.insert(SomPath::new(init_field_name), init_value);
                }

                // Update running presence map for intra-phase suppression.
                // Per XFA 3.3 §10 Rule 1: presence is checked at the moment
                // each event would fire, so changes from earlier init scripts
                // must suppress later ones within the same phase.
                // Use full SOM paths as keys to avoid collisions between
                // fields with the same leaf name in different subforms.
                if let Some(presence) = engine.get_current_field_presence() {
                    init_dynamic_presence.insert(full_path.clone(), presence);
                }
                for (child_name, _child_id) in child_fields {
                    if let Some((_id, presence)) = engine.get_child_field_presence(child_name) {
                        // child_name is already a leaf; prefix with parent path
                        let child_full = format!("{}.{}", full_path, child_name);
                        init_dynamic_presence.insert(child_full, presence);
                    }
                }
                let som_pres = engine.get_all_som_presence_changes();
                for (som_path, pres_str) in &som_pres {
                    let presence = pres_str.parse().unwrap_or_default();
                    // SOM paths from the engine are already full paths
                    init_dynamic_presence.insert(som_path.clone(), presence);
                }
            }
        }

        // Build dynamic presence map from Phase 1 presence changes.
        // Per XFA 3.3 §10 p.407 Rule 1, presence is inspected at the moment
        // the event would be triggered.
        // Also collect SOM-level presence changes (e.g. `fieldB.presence = "inactive"`)
        // for cross-phase suppression only — these are NOT added to the output
        // presence_changes since they were not previously tracked there.
        let mut dynamic_presence_overrides: HashMap<String, Presence> = HashMap::new();
        let som_presence_changes = engine.get_all_som_presence_changes();
        for (som_path, presence_str) in &som_presence_changes {
            let presence = presence_str.parse().unwrap_or_default();
            // Use full SOM path as key to avoid leaf-name collisions
            dynamic_presence_overrides.insert(som_path.clone(), presence);
            engine.update_initial_presence(&SomPath::new(som_path), presence_str);
        }
        let mut presence_map_after_phase1 =
            Self::build_full_path_presence_map(&all_events, &presence_changes);
        presence_map_after_phase1.extend(dynamic_presence_overrides.clone());

        // Phase 2: Execute form-ready JavaScript events
        for (field_name, full_path, child_fields, script, static_presence) in &all_events {
            if script.content_type == ScriptContentType::JavaScript
                && script.activity == EventActivity::Ready
                && script.event_ref == EventRef::Form
                && !field_name.is_empty()
                && !Self::is_effectively_inactive(
                    full_path,
                    *static_presence,
                    &presence_map_after_phase1,
                )
            {
                engine.set_current_field_with_children(full_path, field_name, "", child_fields);
                engine.update_event_context(&EventActivity::Ready, full_path);

                if let Ok(Some(value)) = engine.execute_script(script) {
                    computed_values.insert(SomPath::new(field_name.clone()), value);
                }

                // Collect values set on child fields
                // Empty strings are valid per XFA spec (cleared fields, deselected exclGroups)
                for (child_name, child_id) in child_fields {
                    if let Some((id, child_value)) = engine.get_child_field_value(child_name) {
                        let storage_key = if !id.is_empty() { id } else { child_id.clone() };

                        if !storage_key.is_empty() {
                            computed_values
                                .insert(SomPath::new(storage_key.clone()), child_value.clone());
                        }
                        computed_values.insert(SomPath::new(child_name.clone()), child_value);
                    }
                }
            }
        }

        // Build dynamic presence map from Phase 1+2 presence changes.
        // Also collect SOM-level presence changes from Phase 2 for cross-phase suppression.
        let som_presence_changes_2 = engine.get_all_som_presence_changes();
        for (som_path, presence_str) in &som_presence_changes_2 {
            let presence = presence_str.parse().unwrap_or_default();
            // Use full SOM path as key to avoid leaf-name collisions
            dynamic_presence_overrides.insert(som_path.clone(), presence);
            engine.update_initial_presence(&SomPath::new(som_path), presence_str);
        }
        let mut presence_map_after_phase2 =
            Self::build_full_path_presence_map(&all_events, &presence_changes);
        presence_map_after_phase2.extend(dynamic_presence_overrides);

        // Phase 3: Execute layout-ready JavaScript events
        for (field_name, full_path, child_fields, script, static_presence) in &all_events {
            if script.content_type == ScriptContentType::JavaScript
                && script.activity == EventActivity::Ready
                && script.event_ref == EventRef::Layout
                && !field_name.is_empty()
                && !Self::is_effectively_inactive(
                    full_path,
                    *static_presence,
                    &presence_map_after_phase2,
                )
            {
                engine.set_current_field_with_children(full_path, field_name, "", child_fields);
                engine.update_event_context(&EventActivity::Ready, full_path);

                if let Ok(Some(value)) = engine.execute_script(script) {
                    computed_values.insert(SomPath::new(field_name.clone()), value);
                }

                // Collect values set on child fields
                // Empty strings are valid per XFA spec (cleared fields, deselected exclGroups)
                for (child_name, child_id) in child_fields {
                    if let Some((id, child_value)) = engine.get_child_field_value(child_name) {
                        let storage_key = if !id.is_empty() { id } else { child_id.clone() };

                        if !storage_key.is_empty() {
                            computed_values
                                .insert(SomPath::new(storage_key.clone()), child_value.clone());
                        }
                        computed_values.insert(SomPath::new(child_name.clone()), child_value);
                    }
                }
            }
        }

        // Phase 4: Execute docReady JavaScript events
        // Per XFA 3.3 §10 p.408: docReady fires after all form-level events
        // (initialize, form:ready, layout:ready) have completed.
        for (field_name, full_path, child_fields, script, static_presence) in &all_events {
            if script.content_type == ScriptContentType::JavaScript
                && script.activity == EventActivity::DocReady
                && !field_name.is_empty()
                && !Self::is_effectively_inactive(
                    full_path,
                    *static_presence,
                    &presence_map_after_phase2,
                )
            {
                engine.set_current_field_with_children(full_path, field_name, "", child_fields);
                engine.update_event_context(&EventActivity::DocReady, full_path);

                if let Ok(Some(value)) = engine.execute_script(script) {
                    computed_values.insert(SomPath::new(field_name.clone()), value);
                }

                // Collect values set on child fields
                for (child_name, child_id) in child_fields {
                    if let Some((id, child_value)) = engine.get_child_field_value(child_name) {
                        let storage_key = if !id.is_empty() { id } else { child_id.clone() };

                        if !storage_key.is_empty() {
                            computed_values
                                .insert(SomPath::new(storage_key.clone()), child_value.clone());
                        }
                        computed_values.insert(SomPath::new(child_name.clone()), child_value);
                    }
                }
            }
        }

        // Phase 5: Collect all values from SOM hierarchy
        // Empty strings are valid per XFA spec (cleared fields, deselected exclGroups)
        let som_values = engine.get_all_som_field_values();
        for (field_name, value) in som_values {
            computed_values
                .entry(SomPath::new(field_name.clone()))
                .or_insert(value);
        }

        Ok(ScriptExecutionResult {
            computed_values,
            presence_changes,
        })
    }

    /// Apply presence changes to a mutable XFA node tree.
    ///
    /// This should be called on a cloned tree to preserve the original.
    pub fn apply_presence_changes(
        nodes: &mut [XfaNode],
        changes: &[(String, Option<String>, Presence)],
    ) {
        for (name, id, presence) in changes {
            // Try to find by ID first (more specific)
            if let Some(id_val) = id
                && Self::apply_presence_by_id(nodes, id_val, *presence)
            {
                continue;
            }
            // Fall back to finding by name
            Self::apply_presence_by_name(nodes, name, *presence);
        }
    }

    /// Recursively find a node by ID and set its presence
    fn apply_presence_by_id(nodes: &mut [XfaNode], id: &str, presence: Presence) -> bool {
        for node in nodes {
            if node.attributes.get("id").map(|s| s.as_str()) == Some(id) {
                node.set_presence(presence);
                return true;
            }
            if Self::apply_presence_by_id(&mut node.children, id, presence) {
                return true;
            }
        }
        false
    }

    /// Recursively find ALL nodes by name and set their presence
    fn apply_presence_by_name(nodes: &mut [XfaNode], name: &str, presence: Presence) -> bool {
        let mut found = false;
        for node in nodes {
            if node.name.as_deref() == Some(name) {
                node.set_presence(presence);
                found = true;
            }
            if Self::apply_presence_by_name(&mut node.children, name, presence) {
                found = true;
            }
        }
        found
    }

    // ========================================================================
    // Helper functions moved from Flattened
    // ========================================================================

    /// Build a parent-child map that tracks both child names AND their unique IDs.
    fn build_parent_child_map_with_ids(
        xfa_nodes: &[XfaNode],
    ) -> HashMap<String, Vec<(String, String)>> {
        let mut parent_child_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut subform_counters: HashMap<String, usize> = HashMap::new();

        fn collect_children_with_ids(
            nodes: &[XfaNode],
            parent_key: Option<&str>,
            map: &mut HashMap<String, Vec<(String, String)>>,
            counters: &mut HashMap<String, usize>,
        ) {
            for node in nodes {
                let node_name = node.name.clone().unwrap_or_default();
                let node_id = node.attributes.get("id").cloned().unwrap_or_default();

                if let Some(parent) = parent_key {
                    let is_field = matches!(node.kind, XfaNodeKind::Field)
                        || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "field");

                    if is_field && !node_name.is_empty() {
                        map.entry(parent.to_string())
                            .or_default()
                            .push((node_name.clone(), node_id.clone()));
                    }
                }

                let is_subform = matches!(node.kind, XfaNodeKind::Subform)
                    || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                let is_exclgroup = matches!(node.kind, XfaNodeKind::ExclGroup);

                if (is_subform || is_exclgroup) && !node_name.is_empty() {
                    let key = if !node_id.is_empty() {
                        format!("{}#{}", node_name, node_id)
                    } else {
                        let count = counters.entry(node_name.clone()).or_insert(0);
                        let key = format!("{}[{}]", node_name, *count);
                        *count += 1;
                        key
                    };
                    collect_children_with_ids(&node.children, Some(&key), map, counters);
                } else if !is_subform && !is_exclgroup {
                    collect_children_with_ids(&node.children, parent_key, map, counters);
                }
            }
        }

        collect_children_with_ids(
            xfa_nodes,
            None,
            &mut parent_child_map,
            &mut subform_counters,
        );
        parent_child_map
    }

    /// Check if a node is effectively inactive, considering both static
    /// presence and dynamic overrides set by earlier script phases.
    /// Per XFA 3.3 §10 p.407 Rule 1.
    fn is_effectively_inactive(
        full_path: &str,
        static_presence: Presence,
        dynamic_overrides: &HashMap<String, Presence>,
    ) -> bool {
        if static_presence == Presence::Inactive {
            return true;
        }
        // Look up by full SOM path to avoid collisions between fields
        // with the same leaf name in different subforms.
        if let Some(&p) = dynamic_overrides.get(full_path) {
            return p == Presence::Inactive;
        }
        false
    }

    /// Build a lookup keyed by full SOM path from presence_changes.
    /// Uses `all_events` to resolve leaf names → full paths.
    fn build_full_path_presence_map(
        all_events: &[EventWithChildren],
        changes: &[(String, Option<String>, Presence)],
    ) -> HashMap<String, Presence> {
        // Build a leaf→full_path lookup from all_events.
        // Note: if multiple events share a leaf, the last full_path wins;
        // that's fine because name collisions are the problem we're solving
        // and full SOM paths are unique.
        let mut leaf_to_full: HashMap<&str, &str> = HashMap::new();
        for (name, full_path, children, _, _) in all_events {
            if !name.is_empty() {
                leaf_to_full.insert(name.as_str(), full_path.as_str());
            }
            for (child_name, _) in children {
                // Children paths are relative to the parent's full path
                leaf_to_full
                    .entry(child_name.as_str())
                    .or_insert(full_path.as_str());
            }
        }

        let mut map = HashMap::new();
        for (name, _id, presence) in changes {
            // If the name looks like a full path already (contains '.'), use it as-is.
            // Otherwise resolve via the leaf→full mapping.
            let key = if name.contains('.') {
                name.clone()
            } else if let Some(&full) = leaf_to_full.get(name.as_str()) {
                // Build child full path: parent_full_path.child_name
                if full.ends_with(name.as_str()) {
                    full.to_string()
                } else {
                    format!("{}.{}", full, name)
                }
            } else {
                name.clone()
            };
            map.insert(key, *presence);
        }
        map
    }

    /// Find all events with child IDs and full SOM paths
    fn find_all_events_with_child_ids(
        nodes: &[XfaNode],
        events: &mut Vec<EventWithChildren>,
        parent_child_map: &HashMap<String, Vec<ChildNameIdPair>>,
        subform_counters: &mut HashMap<String, usize>,
        parent_path: Option<&str>,
    ) {
        for node in nodes {
            // XFA 3.3 §10 p.407 Rule 1: When a container has presence=inactive,
            // it does not generate any of its normal calculations, validations,
            // or events. Skip this node and all its children.
            if node.get_presence() == Presence::Inactive {
                continue;
            }
            let name = node.name.clone().unwrap_or_default();
            let node_id = node.attributes.get("id").cloned().unwrap_or_default();

            let is_subform = matches!(node.kind, XfaNodeKind::Subform)
                || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
            let is_exclgroup = matches!(node.kind, XfaNodeKind::ExclGroup);

            // Build the full SOM path for this node
            let full_path = if !name.is_empty() {
                match parent_path {
                    Some(p) => format!("{}.{}", p, name),
                    None => name.clone(),
                }
            } else {
                parent_path.unwrap_or("").to_string()
            };

            let key = if !node_id.is_empty() {
                format!("{}#{}", name, node_id)
            } else if (is_subform || is_exclgroup) && !name.is_empty() {
                let count = subform_counters.entry(name.clone()).or_insert(0);
                let key = format!("{}[{}]", name, *count);
                *count += 1;
                key
            } else {
                name.clone()
            };

            let children = parent_child_map.get(&key).cloned().unwrap_or_default();

            let node_events = parse_events_from_node(&node.children);
            for event in node_events {
                // Include both the name (for display) and full_path (for SOM lookup)
                events.push((
                    name.clone(),
                    full_path.clone(),
                    children.clone(),
                    event,
                    node.get_presence(),
                ));
            }

            // Recurse with updated parent path
            let next_parent = if !name.is_empty() && (is_subform || is_exclgroup) {
                Some(full_path.as_str())
            } else {
                parent_path
            };

            Self::find_all_events_with_child_ids(
                &node.children,
                events,
                parent_child_map,
                subform_counters,
                next_parent,
            );
        }
    }

    /// Build and register the XFA SOM hierarchy in the scripting engine.
    fn build_and_register_xfa_som_hierarchy(xfa_nodes: &[XfaNode], engine: &mut XfaScriptEngine) {
        fn register_nodes_recursive(
            nodes: &[XfaNode],
            parent_path: Option<&str>,
            engine: &mut XfaScriptEngine,
            parent_is_exclgroup: bool,
        ) {
            for node in nodes {
                let node_name = node.name.clone().unwrap_or_default();

                if node_name.is_empty() {
                    register_nodes_recursive(
                        &node.children,
                        parent_path,
                        engine,
                        parent_is_exclgroup,
                    );
                    continue;
                }

                let is_subform = matches!(node.kind, XfaNodeKind::Subform)
                    || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                let is_field = matches!(node.kind, XfaNodeKind::Field)
                    || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "field");
                let is_exclgroup = matches!(node.kind, XfaNodeKind::ExclGroup);
                let is_draw = matches!(node.kind, XfaNodeKind::Draw)
                    || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "draw");

                if !is_subform && !is_field && !is_exclgroup && !is_draw {
                    register_nodes_recursive(&node.children, parent_path, engine, false);
                    continue;
                }

                let full_path = match parent_path {
                    Some(p) => format!("{}.{}", p, node_name),
                    None => node_name.clone(),
                };

                let value = node.attributes.get("rawValue").cloned().unwrap_or_default();

                // Extract item values from <items> for exclGroup children
                // (XFA 3.3 §4 pp.195-197, §17 pp.758-759).
                let (item_key, off_value) = if parent_is_exclgroup {
                    node.extract_item_values()
                } else {
                    (None, None)
                };

                engine.register_xfa_node(
                    &node_name,
                    &full_path,
                    parent_path,
                    is_field,
                    &value,
                    parent_is_exclgroup,
                    item_key.as_deref(),
                    off_value.as_deref(),
                    node.presence.as_str(),
                );

                if is_subform || is_exclgroup {
                    register_nodes_recursive(
                        &node.children,
                        Some(&full_path),
                        engine,
                        is_exclgroup,
                    );
                }
            }
        }

        // Find the root subform container (e.g., "UBSForms")
        if let Some(root) = Self::find_root_subform(xfa_nodes) {
            // Register the root subform first
            let root_name = root.name.clone().unwrap_or_default();
            if !root_name.is_empty() {
                engine.register_xfa_node(
                    &root_name, &root_name, None, false, "", false, None, None, "visible",
                );
            }

            // Per XFA 3.3 §3: SOM paths include the root subform.
            // All children of the root are registered under the root's name
            // (e.g. "UBSForms_66816.Page", not just "Page").
            // Unqualified name resolution uses the _xfa_fields_ registry
            // and resolveNode scoping, not path stripping.
            register_nodes_recursive(&root.children, Some(&root_name), engine, false);
        }
    }

    /// Find the root content subform
    fn find_root_subform(xfa_nodes: &[XfaNode]) -> Option<&XfaNode> {
        for node in xfa_nodes {
            let is_subform = matches!(node.kind, XfaNodeKind::Subform)
                || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
            let is_page_set = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "pageSet");

            if is_page_set {
                continue;
            }

            if is_subform && node.name.is_some() {
                return Some(node);
            }

            if let Some(found) = Self::find_root_subform(&node.children) {
                return Some(found);
            }
        }
        None
    }

    /// Extract and register translations from <variables> elements.
    /// This handles both <text> variables (simple values) and <script> variables (code objects).
    fn extract_and_register_translations(xfa_nodes: &[XfaNode], engine: &mut XfaScriptEngine) {
        let mut variable_scripts: Vec<(String, String)> = Vec::new();
        let mut text_vars: Vec<(String, String)> = Vec::new();
        Self::collect_variable_items(xfa_nodes, &mut variable_scripts, &mut text_vars);

        // Register <text> variables as fields (e.g., Footer_Line_txtlanguage, Footer_Line_txtformid)
        for (name, value) in &text_vars {
            engine.register_field(name, name, value);
        }

        // Register <script> variables as JavaScript objects
        // Per XFA 3.3 §10 pp. 376-378: named script objects expose all
        // top-level variables and functions as properties/methods.
        for (name, content) in &variable_scripts {
            let wrapped = wrap_script_object(name, content, false);

            let _ = engine.execute_variable_script(&wrapped);
        }
    }

    /// Recursively collect both <script> and <text> content from <variables> elements
    fn collect_variable_items(
        nodes: &[XfaNode],
        scripts: &mut Vec<(String, String)>,
        text_vars: &mut Vec<(String, String)>,
    ) {
        for node in nodes {
            if let XfaNodeKind::Element { tag_name, .. } = &node.kind
                && tag_name == "variables"
            {
                for child in &node.children {
                    if let XfaNodeKind::Element {
                        tag_name: child_tag,
                        text_content,
                        ..
                    } = &child.kind
                        && let Some(name) =
                            child.name.as_ref().or_else(|| child.attributes.get("name"))
                    {
                        if child_tag == "script" {
                            // Handle <script> - may have content directly or in children
                            if let Some(content) = text_content
                                && !content.is_empty()
                            {
                                scripts.push((name.clone(), content.clone()));
                            }
                            // Also check for content in child nodes
                            for script_child in &child.children {
                                if let XfaNodeKind::Text { content } = &script_child.kind
                                    && !content.is_empty()
                                {
                                    scripts.push((name.clone(), content.clone()));
                                }
                            }
                        } else if child_tag == "text" {
                            // Handle <text> variables - these are simple string values
                            let value = text_content.clone().unwrap_or_default();
                            text_vars.push((name.clone(), value));
                        }
                    }
                }
            }

            Self::collect_variable_items(&node.children, scripts, text_vars);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xfa::{XfaNode, XfaNodeKind};
    use std::collections::HashMap;

    /// Build a minimal field node with an initialize event script.
    fn make_field_with_init_script(name: &str, script_source: &str, presence: &str) -> XfaNode {
        let mut attrs = HashMap::new();
        attrs.insert("name".to_string(), name.to_string());
        if !presence.is_empty() {
            attrs.insert("presence".to_string(), presence.to_string());
        }

        let mut field = XfaNode::new(XfaNodeKind::Field, attrs);

        // Build <event activity="initialize"><script contentType="application/x-javascript">...</script></event>
        let script_node = XfaNode::new(
            XfaNodeKind::Element {
                tag_name: "script".to_string(),
                text_content: Some(script_source.to_string()),
            },
            {
                let mut a = HashMap::new();
                a.insert(
                    "contentType".to_string(),
                    "application/x-javascript".to_string(),
                );
                a
            },
        );

        let mut event_node = XfaNode::new(
            XfaNodeKind::Element {
                tag_name: "event".to_string(),
                text_content: None,
            },
            {
                let mut a = HashMap::new();
                a.insert("activity".to_string(), "initialize".to_string());
                a
            },
        );
        event_node.children.push(script_node);
        field.children.push(event_node);
        field
    }

    /// Build a field with a form-ready event script.
    fn make_field_with_ready_script(name: &str, script_source: &str, presence: &str) -> XfaNode {
        let mut attrs = HashMap::new();
        attrs.insert("name".to_string(), name.to_string());
        if !presence.is_empty() {
            attrs.insert("presence".to_string(), presence.to_string());
        }

        let mut field = XfaNode::new(XfaNodeKind::Field, attrs);

        let script_node = XfaNode::new(
            XfaNodeKind::Element {
                tag_name: "script".to_string(),
                text_content: Some(script_source.to_string()),
            },
            {
                let mut a = HashMap::new();
                a.insert(
                    "contentType".to_string(),
                    "application/x-javascript".to_string(),
                );
                a
            },
        );

        let mut event_node = XfaNode::new(
            XfaNodeKind::Element {
                tag_name: "event".to_string(),
                text_content: None,
            },
            {
                let mut a = HashMap::new();
                a.insert("activity".to_string(), "ready".to_string());
                a.insert("ref".to_string(), "$form".to_string());
                a
            },
        );
        event_node.children.push(script_node);
        field.children.push(event_node);
        field
    }

    /// Wrap fields in a minimal template > subform structure.
    fn wrap_in_template(fields: Vec<XfaNode>) -> Vec<XfaNode> {
        let mut subform = XfaNode::new(XfaNodeKind::Subform, {
            let mut a = HashMap::new();
            a.insert("name".to_string(), "Root".to_string());
            a
        });
        subform.children = fields;

        let mut template = XfaNode::new(XfaNodeKind::Template, HashMap::new());
        template.children.push(subform);
        vec![template]
    }

    // =========================================================================
    // Test: inactive presence suppresses script execution (static)
    // =========================================================================

    #[test]
    fn test_inactive_presence_suppresses_initialize_event() {
        // An active field whose initialize script sets a value
        let active_field =
            make_field_with_init_script("activeField", r#"this.rawValue = "hello";"#, "");
        // An inactive field whose initialize script would set a value
        let inactive_field = make_field_with_init_script(
            "inactiveField",
            r#"this.rawValue = "should_not_appear";"#,
            "inactive",
        );

        let nodes = wrap_in_template(vec![active_field, inactive_field]);
        let result = ScriptExecutor::execute(&nodes);

        // Active field's script should have executed
        let active_found = result.computed_values.values().any(|v| v == "hello");
        assert!(
            active_found,
            "Active field's initialize script should have executed"
        );

        // Inactive field's script should NOT have executed
        let inactive_found = result
            .computed_values
            .values()
            .any(|v| v == "should_not_appear");
        assert!(
            !inactive_found,
            "Inactive field's initialize script must NOT execute per XFA 3.3 §10 Rule 1"
        );
    }

    #[test]
    fn test_inactive_presence_suppresses_form_ready_event() {
        // An active field with a form-ready script
        let active_field =
            make_field_with_ready_script("activeReady", r#"this.rawValue = "ready_value";"#, "");
        // An inactive field with a form-ready script
        let inactive_field = make_field_with_ready_script(
            "inactiveReady",
            r#"this.rawValue = "inactive_ready";"#,
            "inactive",
        );

        let nodes = wrap_in_template(vec![active_field, inactive_field]);
        let result = ScriptExecutor::execute(&nodes);

        let active_found = result.computed_values.values().any(|v| v == "ready_value");
        assert!(
            active_found,
            "Active field's form-ready script should have executed"
        );

        let inactive_found = result
            .computed_values
            .values()
            .any(|v| v == "inactive_ready");
        assert!(
            !inactive_found,
            "Inactive field's form-ready script must NOT execute per XFA 3.3 §10 Rule 1"
        );
    }

    // =========================================================================
    // Test: dynamic presence change across phases
    // =========================================================================

    #[test]
    fn test_dynamic_inactive_suppresses_later_phases() {
        // fieldA's initialize script sets fieldB's presence to inactive
        // using `fieldB.presence = "inactive"` via SOM global access.
        // fieldB has a form-ready script that should be suppressed because
        // fieldA's initialize script set it inactive before Phase 2 runs.
        let field_a =
            make_field_with_init_script("fieldA", r#"Root.fieldB.presence = "inactive";"#, "");
        let field_b = make_field_with_ready_script(
            "fieldB",
            r#"this.rawValue = "should_be_suppressed";"#,
            "",
        );

        // Wrap in a subform so fieldB is a sibling/child of the same parent
        let mut subform = XfaNode::new(XfaNodeKind::Subform, {
            let mut a = HashMap::new();
            a.insert("name".to_string(), "Root".to_string());
            a
        });
        subform.children = vec![field_a, field_b];

        let mut template = XfaNode::new(XfaNodeKind::Template, HashMap::new());
        template.children.push(subform);
        let nodes = vec![template];

        let result = ScriptExecutor::execute(&nodes);

        // fieldB's form-ready script should NOT have executed
        // because fieldA's initialize script set fieldB's presence to inactive
        // via the SOM hierarchy, and our cross-phase presence tracking suppresses it.
        let suppressed = result
            .computed_values
            .values()
            .any(|v| v == "should_be_suppressed");
        assert!(
            !suppressed,
            "fieldB's form-ready script must be suppressed after dynamic presence change to inactive"
        );
    }

    /// Build a field with a calculate event script.
    fn make_field_with_calculate_script(
        name: &str,
        script_source: &str,
        presence: &str,
    ) -> XfaNode {
        let mut attrs = HashMap::new();
        attrs.insert("name".to_string(), name.to_string());
        if !presence.is_empty() {
            attrs.insert("presence".to_string(), presence.to_string());
        }

        let mut field = XfaNode::new(XfaNodeKind::Field, attrs);

        let script_node = XfaNode::new(
            XfaNodeKind::Element {
                tag_name: "script".to_string(),
                text_content: Some(script_source.to_string()),
            },
            {
                let mut a = HashMap::new();
                a.insert(
                    "contentType".to_string(),
                    "application/x-javascript".to_string(),
                );
                a
            },
        );

        let mut event_node = XfaNode::new(
            XfaNodeKind::Element {
                tag_name: "event".to_string(),
                text_content: None,
            },
            {
                let mut a = HashMap::new();
                a.insert("activity".to_string(), "calculate".to_string());
                a
            },
        );
        event_node.children.push(script_node);
        field.children.push(event_node);
        field
    }

    /// Build a field with a validate event script.
    fn make_field_with_validate_script(name: &str, script_source: &str) -> XfaNode {
        let mut attrs = HashMap::new();
        attrs.insert("name".to_string(), name.to_string());

        let mut field = XfaNode::new(XfaNodeKind::Field, attrs);

        let script_node = XfaNode::new(
            XfaNodeKind::Element {
                tag_name: "script".to_string(),
                text_content: Some(script_source.to_string()),
            },
            {
                let mut a = HashMap::new();
                a.insert(
                    "contentType".to_string(),
                    "application/x-javascript".to_string(),
                );
                a
            },
        );

        let mut event_node = XfaNode::new(
            XfaNodeKind::Element {
                tag_name: "event".to_string(),
                text_content: None,
            },
            {
                let mut a = HashMap::new();
                a.insert("activity".to_string(), "validate".to_string());
                a
            },
        );
        event_node.children.push(script_node);
        field.children.push(event_node);
        field
    }

    // =========================================================================
    // Test: children of inactive container are also suppressed
    // =========================================================================

    #[test]
    fn test_inactive_container_suppresses_children() {
        // An inactive subform containing a field with a script
        let child_field =
            make_field_with_init_script("childField", r#"this.rawValue = "child_value";"#, "");

        let mut inactive_subform = XfaNode::new(XfaNodeKind::Subform, {
            let mut a = HashMap::new();
            a.insert("name".to_string(), "InactiveGroup".to_string());
            a.insert("presence".to_string(), "inactive".to_string());
            a
        });
        inactive_subform.children.push(child_field);

        let nodes = wrap_in_template(vec![inactive_subform]);
        let result = ScriptExecutor::execute(&nodes);

        let child_found = result.computed_values.values().any(|v| v == "child_value");
        assert!(
            !child_found,
            "Children of inactive containers must NOT have events executed per XFA 3.3 §10 Rule 1"
        );
    }

    // =========================================================================
    // Test: calculate events run before initialize events
    // =========================================================================

    #[test]
    fn test_calculate_runs_before_initialize() {
        // Per XFA 3.3 §10 p.407: "All value calculations are done, then all
        // property calculations, then all validations, and then all initialize
        // events are fired."
        //
        // fieldA has a calculate script that sets its value.
        // fieldB has an initialize script that reads fieldA's value.
        // If calculate runs before initialize, fieldB should see fieldA's
        // calculated value.
        let field_a =
            make_field_with_calculate_script("fieldA", r#"this.rawValue = "calculated";"#, "");
        let field_b =
            make_field_with_init_script("fieldB", r#"this.rawValue = Root.fieldA.rawValue;"#, "");

        let nodes = wrap_in_template(vec![field_a, field_b]);
        let result = ScriptExecutor::execute(&nodes);

        let field_b_value = result.computed_values.get(&SomPath::new("fieldB")).cloned();
        assert_eq!(
            field_b_value,
            Some("calculated".to_string()),
            "Initialize script should see calculate results (calculate runs first per XFA spec)"
        );
    }

    #[test]
    fn test_calculate_result_replaces_field_value() {
        // Per XFA 3.3 §10 p.380: "field: Replaces the value of the container
        // object" — the calculate script return value becomes the field value.
        let field = make_field_with_calculate_script("taxField", r#"this.rawValue = "42.50";"#, "");

        let nodes = wrap_in_template(vec![field]);
        let result = ScriptExecutor::execute(&nodes);

        let tax_value = result
            .computed_values
            .get(&SomPath::new("taxField"))
            .cloned();
        assert_eq!(
            tax_value,
            Some("42.50".to_string()),
            "Calculate script result should replace field value"
        );
    }

    // =========================================================================
    // Test: validate scripts run after calculations, before initialize
    // =========================================================================

    #[test]
    fn test_validate_runs_after_calculate_before_initialize() {
        // Per XFA 3.3 §10 p.407: calculate → validate → initialize.
        // Validate scripts can read calculated values. They don't replace
        // field values, but they can set side-effect state.
        // Here we verify that a validate script can read a value that
        // was set by a calculate script, confirming correct ordering.
        let field_a =
            make_field_with_calculate_script("calcField", r#"this.rawValue = "100";"#, "");
        // Validate script reads calcField's value and marks a validation
        // result on a different field.
        let field_b = make_field_with_validate_script(
            "validField",
            r#"
                var v = Root.calcField.rawValue;
                if (v === "100") {
                    this.rawValue = "valid_" + v;
                }
            "#,
        );

        let nodes = wrap_in_template(vec![field_a, field_b]);
        let result = ScriptExecutor::execute(&nodes);

        // calcField should have its calculated value
        let calc_value = result
            .computed_values
            .get(&SomPath::new("calcField"))
            .cloned();
        assert_eq!(
            calc_value,
            Some("100".to_string()),
            "Calculate script should have set calcField value"
        );

        // validField should have been set by the validate script that read calcField
        let valid_value = result
            .computed_values
            .get(&SomPath::new("validField"))
            .cloned();
        assert_eq!(
            valid_value,
            Some("valid_100".to_string()),
            "Validate script should see calculated values (validate runs after calculate)"
        );
    }

    // =========================================================================
    // Test: intra-phase presence suppression within initialize
    // =========================================================================

    #[test]
    fn test_intra_phase_init_presence_suppression() {
        // Per XFA 3.3 §10 Rule 1: "The property is inspected at the moment
        // that the calculation, validation, or event would be triggered."
        //
        // fieldA's initialize script sets fieldB to inactive.
        // fieldB's initialize script should NOT execute because fieldA's
        // script already set it inactive, and presence is checked at the
        // moment fieldB's event would fire (within the same init phase).
        let field_a =
            make_field_with_init_script("fieldA", r#"Root.fieldB.presence = "inactive";"#, "");
        let field_b =
            make_field_with_init_script("fieldB", r#"this.rawValue = "should_not_run";"#, "");

        let nodes = wrap_in_template(vec![field_a, field_b]);
        let result = ScriptExecutor::execute(&nodes);

        let suppressed = result
            .computed_values
            .values()
            .any(|v| v == "should_not_run");
        assert!(
            !suppressed,
            "fieldB's init script must be suppressed by intra-phase presence change from fieldA"
        );
    }
}
