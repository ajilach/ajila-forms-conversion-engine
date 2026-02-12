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
    EventActivity, EventRef, Presence, ScriptContentType, SomPath, XfaScriptEngine,
    parse_events_from_node,
};
use crate::xfa::{XfaNode, XfaNodeKind};
use std::collections::HashMap;

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

        // Phase 1: Execute initialize events
        for (field_name, full_path, child_fields, script) in &all_events {
            if script.content_type == ScriptContentType::JavaScript
                && script.activity == EventActivity::Initialize
            {
                engine.set_current_field_with_children(full_path, field_name, "", child_fields);

                let result = engine.execute_script(script);
                if let Err(ref e) = result {
                    if std::env::var("XFA_DEBUG").is_ok() {
                        eprintln!("[INIT ERR] field={field_name} path={full_path}: {e}");
                    }
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
                let init_som_values = engine.get_all_som_field_values();
                for (init_field_name, init_value) in init_som_values {
                    if !init_value.is_empty() {
                        computed_values.insert(SomPath::new(init_field_name), init_value);
                    }
                }
            }
        }

        // Phase 2: Execute form-ready JavaScript events
        for (field_name, full_path, child_fields, script) in &all_events {
            if script.content_type == ScriptContentType::JavaScript
                && script.activity == EventActivity::Ready
                && script.event_ref == EventRef::Form
                && !field_name.is_empty()
            {
                engine.set_current_field_with_children(full_path, field_name, "", child_fields);

                if let Ok(Some(value)) = engine.execute_script(script) {
                    computed_values.insert(SomPath::new(field_name.clone()), value);
                }

                // Collect values set on child fields
                for (child_name, child_id) in child_fields {
                    if let Some((id, child_value)) = engine.get_child_field_value(child_name) {
                        if !child_value.is_empty() {
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
        }

        // Phase 3: Execute layout-ready JavaScript events
        for (field_name, full_path, child_fields, script) in &all_events {
            if script.content_type == ScriptContentType::JavaScript
                && script.activity == EventActivity::Ready
                && script.event_ref == EventRef::Layout
                && !field_name.is_empty()
            {
                engine.set_current_field_with_children(full_path, field_name, "", child_fields);

                if let Ok(Some(value)) = engine.execute_script(script) {
                    computed_values.insert(SomPath::new(field_name.clone()), value);
                }

                // Collect values set on child fields
                for (child_name, child_id) in child_fields {
                    if let Some((id, child_value)) = engine.get_child_field_value(child_name) {
                        if !child_value.is_empty() {
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
        }

        // Phase 4: Collect all values from SOM hierarchy
        let som_values = engine.get_all_som_field_values();
        for (field_name, value) in som_values {
            if !value.is_empty() {
                computed_values
                    .entry(SomPath::new(field_name.clone()))
                    .or_insert(value);
            }
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
            if let Some(id_val) = id {
                if Self::apply_presence_by_id(nodes, id_val, *presence) {
                    continue;
                }
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
                let is_exclgroup = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");

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

    /// Find all events with child IDs and full SOM paths
    fn find_all_events_with_child_ids(
        nodes: &[XfaNode],
        events: &mut Vec<(
            String,
            String,
            Vec<(String, String)>,
            crate::xfa::scripting::XfaScript,
        )>,
        parent_child_map: &HashMap<String, Vec<(String, String)>>,
        subform_counters: &mut HashMap<String, usize>,
        parent_path: Option<&str>,
    ) {
        for node in nodes {
            let name = node.name.clone().unwrap_or_default();
            let node_id = node.attributes.get("id").cloned().unwrap_or_default();

            let is_subform = matches!(node.kind, XfaNodeKind::Subform)
                || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
            let is_exclgroup = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");

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
                events.push((name.clone(), full_path.clone(), children.clone(), event));
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
                    register_nodes_recursive(&node.children, parent_path, engine, parent_is_exclgroup);
                    continue;
                }

                let is_subform = matches!(node.kind, XfaNodeKind::Subform)
                    || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                let is_field = matches!(node.kind, XfaNodeKind::Field)
                    || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "field");
                let is_exclgroup = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");
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

                engine.register_xfa_node(&node_name, &full_path, parent_path, is_field, &value, parent_is_exclgroup);

                if is_subform || is_exclgroup {
                    register_nodes_recursive(&node.children, Some(&full_path), engine, is_exclgroup);
                }
            }
        }

        // Find the root subform container (e.g., "UBSForms")
        if let Some(root) = Self::find_root_subform(xfa_nodes) {
            // Register the root subform first
            let root_name = root.name.clone().unwrap_or_default();
            if !root_name.is_empty() {
                engine.register_xfa_node(&root_name, &root_name, None, false, "", false);
            }

            // Register immediate children of root in the SOM hierarchy.
            // Subforms like "Page" are registered as top-level globals so scripts
            // can access "Page.FormTitle..." without needing "UBSForms.Page.FormTitle...".
            for child in &root.children {
                let child_name = child.name.clone().unwrap_or_default();
                if child_name.is_empty() {
                    continue;
                }

                let is_subform = matches!(child.kind, XfaNodeKind::Subform)
                    || matches!(&child.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                let is_page_set = matches!(child.kind, XfaNodeKind::PageSet)
                    || matches!(&child.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "pageSet");
                let is_variables = matches!(&child.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "variables");
                let is_proto = matches!(&child.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "proto");
                let is_field = matches!(child.kind, XfaNodeKind::Field)
                    || matches!(&child.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "field");
                let is_exclgroup = matches!(&child.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");
                let is_draw = matches!(child.kind, XfaNodeKind::Draw)
                    || matches!(&child.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "draw");

                // Skip variables, proto elements
                if is_variables || is_proto {
                    continue;
                }

                // Register page chrome fields (headers, footers) under root so
                // scripts can resolve them via the SOM hierarchy.
                if is_page_set {
                    register_nodes_recursive(&child.children, Some(&root_name), engine, false);
                    continue;
                }

                if is_subform {
                    // Register this child subform (e.g., "Page") as a global
                    engine.register_xfa_node(&child_name, &child_name, None, false, "", false);
                    // Recurse with child_name as parent
                    register_nodes_recursive(&child.children, Some(&child_name), engine, false);
                } else if is_field || is_exclgroup || is_draw {
                    // Non-subform root children (fields, exclGroups, draws)
                    let full_path = format!("{}.{}", root_name, child_name);
                    let value = child
                        .attributes
                        .get("rawValue")
                        .cloned()
                        .unwrap_or_default();
                    engine.register_xfa_node(
                        &child_name,
                        &full_path,
                        Some(&root_name),
                        is_field,
                        &value,
                        false,
                    );
                    if is_exclgroup {
                        register_nodes_recursive(&child.children, Some(&full_path), engine, true);
                    }
                }
            }
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
        for (name, content) in &variable_scripts {
            let wrapped = format!(
                r#"
                var {name} = (function() {{
                    {content}
                    
                    var _obj = {{}};
                    if (typeof setupVariables === 'function') {{
                        _obj.setupVariables = function() {{ setupVariables(); }};
                    }}
                    if (typeof change === 'function') {{
                        _obj.change = function() {{ change(); }};
                    }}
                    if (typeof calculate === 'function') {{
                        _obj.calculate = function() {{ calculate(); }};
                    }}
                    if (typeof validate === 'function') {{
                        _obj.validate = function() {{ return validate(); }};
                    }}
                    return _obj;
                }})();
                "#,
                name = name,
                content = content
            );

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
            if let XfaNodeKind::Element { tag_name, .. } = &node.kind {
                if tag_name == "variables" {
                    for child in &node.children {
                        if let XfaNodeKind::Element {
                            tag_name: child_tag,
                            text_content,
                            ..
                        } = &child.kind
                        {
                            if let Some(name) =
                                child.name.as_ref().or_else(|| child.attributes.get("name"))
                            {
                                if child_tag == "script" {
                                    // Handle <script> - may have content directly or in children
                                    if let Some(content) = text_content {
                                        if !content.is_empty() {
                                            scripts.push((name.clone(), content.clone()));
                                        }
                                    }
                                    // Also check for content in child nodes
                                    for script_child in &child.children {
                                        if let XfaNodeKind::Text { content } = &script_child.kind {
                                            if !content.is_empty() {
                                                scripts.push((name.clone(), content.clone()));
                                            }
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
                }
            }

            Self::collect_variable_items(&node.children, scripts, text_vars);
        }
    }
}
