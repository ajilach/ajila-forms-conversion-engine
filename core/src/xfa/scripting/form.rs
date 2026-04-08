//! XFA Form Interface - High-level API for interacting with XFA forms
//!
//! This module provides the main `XfaForm` struct and node reference types
//! for working with XFA forms at a high level.

use super::dependency::DependencyTracker;
use super::engine::XfaScriptEngine;
use super::events::{
    EventActivity, EventRef, ListenScope, RunAt, ScriptContentType, XfaScript,
    parse_events_from_node,
};
use super::registry::{RegisteredScript, ScriptRegistry, ScriptType};
use super::som::{SomPath, SomResolver};
use super::state::Presence;

use crate::flattened::{Flattened, FlattenedNode, FlattenedNodeKind};
use crate::xfa::{Num, XfaNode, XfaNodeKind};

use std::collections::HashMap;
use std::sync::Arc;

/// Position and size of a node in the flattened layout
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeBounds {
    pub x: Num,
    pub y: Num,
    pub width: Num,
    pub height: Num,
}

/// Result of executing an event
#[derive(Debug, Default)]
pub struct EventResult {
    /// Whether any values changed
    pub values_changed: bool,
    /// Whether presence changed on any node
    pub presence_changed: bool,
    /// SOM paths of fields whose values changed
    pub changed_fields: Vec<SomPath>,
}

/// A reference to a resolved node in the XFA form (immutable)
pub struct XfaNodeRef<'a> {
    /// The XFA node
    xfa_node: &'a XfaNode,
    /// The flattened node (if visible in layout)
    flattened_node: Option<&'a FlattenedNode>,
    /// The SOM path used to resolve this node
    som_path: SomPath,
    /// Whether the node AND all its ancestors have visible presence
    ancestors_visible: bool,
}

impl<'a> XfaNodeRef<'a> {
    /// Get the presence of this node
    pub fn presence(&self) -> Presence {
        self.xfa_node.get_presence()
    }

    /// Get the bounds (position and size) from the flattened layout
    pub fn bounds(&self) -> Option<NodeBounds> {
        self.flattened_node.map(|n| NodeBounds {
            x: n.x,
            y: n.y,
            width: n.width,
            height: n.height,
        })
    }

    /// Get the position (x, y) from the flattened layout
    pub fn position(&self) -> Option<(Num, Num)> {
        self.flattened_node.map(|n| (n.x, n.y))
    }

    /// Get the size (width, height) from the flattened layout
    pub fn size(&self) -> Option<(Num, Num)> {
        self.flattened_node.map(|n| (n.width, n.height))
    }

    /// Get the raw value of this node
    pub fn raw_value(&self) -> Option<String> {
        // First try flattened node
        if let Some(flat) = self.flattened_node {
            match &flat.kind {
                FlattenedNodeKind::Field { value, .. } if !value.is_empty() => {
                    return Some(value.clone());
                }
                FlattenedNodeKind::Text { content, .. } if !content.is_empty() => {
                    return Some(content.clone());
                }
                _ => {}
            }
        }

        // Fall back to XFA node attributes or value child
        if let Some(raw) = self.xfa_node.attributes.get("rawValue") {
            return Some(raw.clone());
        }

        // Try text_content for <text> variable elements
        if let XfaNodeKind::Element {
            text_content: Some(content),
            ..
        } = &self.xfa_node.kind
            && !content.is_empty()
        {
            return Some(content.clone());
        }

        Self::extract_value_from_xfa_node(self.xfa_node)
    }

    /// Get the name of this node
    pub fn name(&self) -> Option<&str> {
        self.xfa_node.name.as_deref()
    }

    /// Get the SOM path used to resolve this node
    pub fn som_path(&self) -> &SomPath {
        &self.som_path
    }

    /// Check if this node is visible based on its presence and ancestor presence
    pub fn is_visible(&self) -> bool {
        let own_presence = self.xfa_node.get_presence();
        if own_presence.should_skip_layout() {
            return false;
        }
        self.ancestors_visible
    }

    /// Get the XFA node kind
    pub fn kind(&self) -> &XfaNodeKind {
        &self.xfa_node.kind
    }

    /// Get a reference to the underlying XFA node (for debugging/advanced usage)
    pub fn xfa_node(&self) -> &XfaNode {
        self.xfa_node
    }

    /// Find a child element inside this node's `<ui>` element by tag name.
    ///
    /// XFA fields store their widget type as a child of the `<ui>` element
    /// (e.g. `<ui><checkButton .../></ui>`).  This helper traverses
    /// `children → ui → children` and returns the first match.
    fn find_ui_child(&self, target_tag: &str) -> Option<&XfaNode> {
        for child in &self.xfa_node.children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind
                && tag_name == "ui"
            {
                for ui_child in &child.children {
                    if let XfaNodeKind::Element {
                        tag_name: ui_tag, ..
                    } = &ui_child.kind
                        && ui_tag == target_tag
                    {
                        return Some(ui_child);
                    }
                }
            }
        }
        None
    }

    /// Check if this is a dropdown/choicelist field
    pub fn is_dropdown(&self) -> bool {
        self.has_choice_list()
    }

    /// Check if this dropdown's items are populated via JavaScript
    pub fn is_script_populated_dropdown(&self) -> bool {
        if !self.is_dropdown() {
            return false;
        }

        for child in &self.xfa_node.children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind
                && tag_name == "event"
            {
                for script_child in &child.children {
                    if let XfaNodeKind::Element {
                        tag_name: script_tag,
                        text_content,
                        ..
                    } = &script_child.kind
                        && script_tag == "script"
                        && let Some(content) = text_content
                        && content.contains("addItem")
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if this field has a choiceList UI element
    fn has_choice_list(&self) -> bool {
        self.find_ui_child("choiceList").is_some()
    }

    /// Check if this is a radio button field
    pub fn is_radio_button(&self) -> bool {
        self.get_check_button_shape() == Some("round".to_string())
    }

    /// Check if this is a checkbox field
    pub fn is_checkbox(&self) -> bool {
        if let Some(shape) = self.get_check_button_shape() {
            shape == "square"
        } else {
            self.has_check_button()
        }
    }

    /// Check if this is a button field
    pub fn is_button(&self) -> bool {
        self.has_button_ui()
    }

    fn has_button_ui(&self) -> bool {
        self.find_ui_child("button").is_some()
    }

    /// Check if this field has a checkButton UI element
    pub fn has_check_button(&self) -> bool {
        self.find_check_button().is_some()
    }

    fn get_check_button_shape(&self) -> Option<String> {
        self.find_check_button()
            .and_then(|cb| cb.attributes.get("shape").cloned())
    }

    fn find_check_button(&self) -> Option<&XfaNode> {
        self.find_ui_child("checkButton")
    }

    /// Get the dropdown options (display values and save values)
    pub fn dropdown_options(&self) -> Vec<(String, String)> {
        let mut display_items: Vec<String> = Vec::new();
        let mut save_items: Vec<String> = Vec::new();

        for child in &self.xfa_node.children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind
                && tag_name == "items"
            {
                let is_save = child
                    .attributes
                    .get("save")
                    .map(|s| s == "1")
                    .unwrap_or(false);
                let items = Self::extract_items_values(child);

                if is_save {
                    save_items = items;
                } else if display_items.is_empty() {
                    display_items = items;
                } else if save_items.is_empty() {
                    save_items = items;
                }
            }
        }

        // Per XFA spec: when only one <items> element exists, display = save
        // regardless of which attribute was set.
        if display_items.is_empty() {
            display_items = save_items.clone();
        }
        if save_items.is_empty() {
            save_items = display_items.clone();
        }

        display_items.into_iter().zip(save_items).collect()
    }

    /// Get just the display values for dropdown options
    pub fn dropdown_display_values(&self) -> Vec<String> {
        self.dropdown_options()
            .into_iter()
            .map(|(d, _)| d)
            .collect()
    }

    /// Get just the save values for dropdown options
    pub fn dropdown_save_values(&self) -> Vec<String> {
        self.dropdown_options()
            .into_iter()
            .map(|(_, s)| s)
            .collect()
    }

    /// Get the number of dropdown options
    pub fn dropdown_option_count(&self) -> usize {
        self.dropdown_options().len()
    }

    /// Get the currently selected dropdown index (0-based)
    pub fn selected_dropdown_index(&self) -> Option<usize> {
        let current_value = self.raw_value()?;
        let save_values = self.dropdown_save_values();
        save_values.iter().position(|v| v == &current_value)
    }

    /// Get the currently selected dropdown display text
    pub fn selected_dropdown_text(&self) -> Option<String> {
        let idx = self.selected_dropdown_index()?;
        self.dropdown_display_values().get(idx).cloned()
    }

    fn extract_items_values(items_node: &XfaNode) -> Vec<String> {
        let mut values = Vec::new();
        for child in &items_node.children {
            match &child.kind {
                XfaNodeKind::Element {
                    tag_name,
                    text_content,
                } => match tag_name.as_str() {
                    "text" | "integer" | "decimal" | "float" | "boolean" | "date" | "dateTime"
                    | "time" => {
                        if let Some(content) = text_content {
                            values.push(content.clone());
                        } else {
                            values.push(String::new());
                        }
                    }
                    _ => {}
                },
                XfaNodeKind::Text { content } => {
                    values.push(content.clone());
                }
                _ => {}
            }
        }
        values
    }

    fn extract_value_from_xfa_node(node: &XfaNode) -> Option<String> {
        for child in &node.children {
            if matches!(child.kind, XfaNodeKind::Value) {
                for text_child in &child.children {
                    if let XfaNodeKind::Text { content } = &text_child.kind
                        && !content.is_empty()
                    {
                        return Some(content.clone());
                    }
                    if let XfaNodeKind::Element {
                        text_content: Some(content),
                        ..
                    } = &text_child.kind
                        && !content.is_empty()
                    {
                        return Some(content.clone());
                    }
                }
            }
        }
        None
    }
}

/// A mutable reference to a resolved node in the XFA form
pub struct XfaNodeRefMut<'a> {
    /// The XFA node (mutable)
    xfa_node: &'a mut XfaNode,
    /// The SOM path used to resolve this node
    som_path: SomPath,
    /// Reference to the persistent script engine (source of truth for field values)
    script_engine: &'a mut XfaScriptEngine,
}

impl<'a> XfaNodeRefMut<'a> {
    /// Get the presence of this node
    pub fn presence(&self) -> Presence {
        self.xfa_node.get_presence()
    }

    /// Set the presence of this node
    pub fn set_presence(&mut self, presence: Presence) {
        self.xfa_node.set_presence(presence);
    }

    /// Get the raw value of this node
    pub fn raw_value(&mut self) -> Option<String> {
        // Read from the engine (single source of truth)
        if let Some(value) = self.script_engine.get_field_value(&self.som_path) {
            return Some(value);
        }
        // Fallback to node attributes
        if let Some(raw) = self.xfa_node.attributes.get("rawValue") {
            return Some(raw.clone());
        }
        XfaNodeRef::extract_value_from_xfa_node(self.xfa_node)
    }

    /// Set the raw value of this node
    pub fn set_raw_value(&mut self, value: &str) {
        // Write to the engine (single source of truth)
        self.script_engine.update_field_value(&self.som_path, value);
        // Also update the XFA node
        Self::set_node_value(self.xfa_node, value);
    }

    /// Get the name of this node
    pub fn name(&self) -> Option<&str> {
        self.xfa_node.name.as_deref()
    }

    /// Get the SOM path used to resolve this node
    pub fn som_path(&self) -> &SomPath {
        &self.som_path
    }

    pub(crate) fn set_node_value(node: &mut XfaNode, value: &str) {
        for child in &mut node.children {
            if matches!(child.kind, XfaNodeKind::Value) {
                for text_child in &mut child.children {
                    if let XfaNodeKind::Text { content } = &mut text_child.kind {
                        *content = value.to_string();
                        return;
                    }
                    if let XfaNodeKind::Element { text_content, .. } = &mut text_child.kind {
                        *text_content = Some(value.to_string());
                        return;
                    }
                }
            }
        }
        node.attributes
            .insert("rawValue".to_string(), value.to_string());
    }
}

/// High-level interface for interacting with an XFA form
pub struct XfaForm {
    /// The XFA node tree
    nodes: Vec<XfaNode>,
    /// The current flattened layout
    flattened: Flattened,
    /// SOM resolver for node lookups
    som_resolver: SomResolver,
    /// Cached mapping of field names to their flattened node indices
    field_index_cache: HashMap<String, usize>,
    /// Registry of all scripts in the form, categorized by type
    script_registry: Arc<ScriptRegistry>,
    /// Dependency tracker for cascading calculations
    dependency_tracker: DependencyTracker,
    /// Dirty flag - set when changes require refresh
    dirty: bool,
    /// Persistent script engine — single source of truth for field values
    script_engine: XfaScriptEngine,
}

impl XfaForm {
    /// Create a new XFA form from parsed nodes
    pub fn new(mut nodes: Vec<XfaNode>) -> Result<Self, String> {
        let script_registry = Arc::new(Self::build_script_registry(&nodes));
        let dependency_tracker = DependencyTracker::new();

        // Execute scripts using ScriptExecutor
        let script_result = crate::xfa::script_executor::ScriptExecutor::execute(&nodes);

        // Apply presence changes to the nodes
        crate::xfa::script_executor::ScriptExecutor::apply_presence_changes(
            &mut nodes,
            &script_result.presence_changes,
        );

        // Merge items from Form DOM packet into Template DOM fields.
        // The Form DOM preserves runtime state (e.g. script-populated dropdown items).
        Flattened::merge_form_items_into_template(&mut nodes);

        // Merge presence values from Form DOM into Template DOM.
        // The Form DOM preserves visibility state set by scripts (e.g. hiding
        // a section based on dropdown selection).
        // Pass the script presence changes so we skip paths already handled
        // by script execution (which produces authoritative runtime state).
        Flattened::merge_form_presence_into_template(&mut nodes, &script_result.presence_changes);

        let init_values = script_result.computed_values;

        // Flatten with the init-time computed values
        let flattened = Flattened::from_xfa(&nodes, &init_values)?;

        let som_resolver = SomResolver::from_nodes(&nodes);
        let field_index_cache = Self::build_field_index_cache(&flattened);

        // Create persistent script engine — disable auto exclGroup sync since
        // interactive events use select_radio_button for explicit propagation
        let mut script_engine = XfaScriptEngine::new();

        // Register fields/subforms FIRST so that script objects can reference
        // them at initialization time (e.g., `var b = UBSForms.Page;`).
        Self::build_som_hierarchy_with_values(&nodes, &init_values, &mut script_engine);
        // Initialize engine with form context - variables like Footer_Line_txtlanguage
        // and Footer_Line_txtformid are extracted from XFA <variables><text> elements
        Self::extract_and_register_translations(&nodes, &mut script_engine);

        Ok(XfaForm {
            nodes,
            flattened,
            som_resolver,
            field_index_cache,
            script_registry,
            dependency_tracker,
            dirty: false,
            script_engine,
        })
    }

    /// Create an XFA form from nodes that have already been through initial
    /// script execution, presence changes, and form-DOM merges.
    ///
    /// This is significantly faster than [`new`] because it skips the
    /// `ScriptExecutor::execute()` phase (which creates a throwaway Boa JS
    /// engine and runs all init-time scripts). A persistent `XfaScriptEngine`
    /// is still created for interactive events.
    ///
    /// Use this when you have cached post-init nodes and their computed values
    /// from a previous `XfaForm::new()` call.
    pub fn from_post_init(
        nodes: Vec<XfaNode>,
        init_values: &HashMap<SomPath, String>,
    ) -> Result<Self, String> {
        let script_registry = Arc::new(Self::build_script_registry(&nodes));
        Self::from_post_init_inner(nodes, init_values, script_registry)
    }

    /// Like [`from_post_init`], but accepts a pre-built `ScriptRegistry` shared
    /// via `Arc`. This avoids the two full tree walks that
    /// `build_script_registry` performs — a significant saving when many
    /// branches are created from the same base nodes (e.g. exhaustive
    /// exploration).
    pub fn from_post_init_with_registry(
        nodes: Vec<XfaNode>,
        init_values: &HashMap<SomPath, String>,
        script_registry: Arc<ScriptRegistry>,
    ) -> Result<Self, String> {
        Self::from_post_init_inner(nodes, init_values, script_registry)
    }

    fn from_post_init_inner(
        nodes: Vec<XfaNode>,
        init_values: &HashMap<SomPath, String>,
        script_registry: Arc<ScriptRegistry>,
    ) -> Result<Self, String> {
        let dependency_tracker = DependencyTracker::new();

        let flattened = Flattened::from_xfa(&nodes, init_values)?;
        let som_resolver = SomResolver::from_nodes(&nodes);
        let field_index_cache = Self::build_field_index_cache(&flattened);

        let mut script_engine = XfaScriptEngine::new();
        // Register fields/subforms FIRST so that script objects can reference
        // them at initialization time (e.g., `var b = UBSForms.Page;`).
        Self::build_som_hierarchy_with_values(&nodes, init_values, &mut script_engine);
        Self::extract_and_register_translations(&nodes, &mut script_engine);

        Ok(XfaForm {
            nodes,
            flattened,
            som_resolver,
            field_index_cache,
            script_registry,
            dependency_tracker,
            dirty: false,
            script_engine,
        })
    }

    /// Reset the form to a snapshot state, reusing the existing Boa JS engine.
    ///
    /// This is significantly cheaper than [`from_post_init`] because it avoids:
    /// - Creating a new Boa JavaScript context (`Context::default()`)
    /// - Re-running `setup_environment()` (XFA objects, globals, JS helpers)
    /// - Rebuilding the SOM hierarchy with JS object creation per field
    /// - Re-extracting translations
    ///
    /// Instead, it reuses the existing JS objects and only updates their
    /// `rawValue` and `presence` properties to match the snapshot.
    pub fn reset_for_branch(
        &mut self,
        nodes: Vec<XfaNode>,
        snapshot_values: &HashMap<SomPath, String>,
    ) -> Result<(), String> {
        self.nodes = nodes;
        self.som_resolver = SomResolver::from_nodes(&self.nodes);

        // Build a presence map from the restored nodes
        let presence_map = Self::build_presence_map(&self.nodes);

        // Reset all field values and presence in the JS engine (reuse JS objects)
        self.script_engine
            .reset_field_states(snapshot_values, &presence_map);

        // Reflatten and rebuild caches
        self.flattened = Flattened::from_xfa(&self.nodes, snapshot_values)?;
        self.field_index_cache = Self::build_field_index_cache(&self.flattened);
        self.dirty = false;

        Ok(())
    }

    /// Build a map of SOM path → presence string from the XFA node tree.
    fn build_presence_map(nodes: &[XfaNode]) -> HashMap<SomPath, String> {
        let mut map = HashMap::new();
        Self::collect_presence_recursive(nodes, "", &mut map);
        map
    }

    fn collect_presence_recursive(
        nodes: &[XfaNode],
        path: &str,
        map: &mut HashMap<SomPath, String>,
    ) {
        for node in nodes {
            let node_path = match &node.name {
                Some(name) if path.is_empty() => name.clone(),
                Some(name) => format!("{}.{}", path, name),
                None => path.to_string(),
            };

            if node.name.is_some() {
                let presence = node.get_presence().as_str().to_string();
                map.insert(SomPath::new(&node_path), presence);
            }

            Self::collect_presence_recursive(&node.children, &node_path, map);
        }
    }

    /// Resolve a node by SOM expression (immutable)
    pub fn resolve(&self, som_expression: &str) -> Option<XfaNodeRef<'_>> {
        let resolved_path = self.som_resolver.resolve_node(som_expression, None)?;

        let xfa_node = Self::find_xfa_node_by_path(&self.nodes, &resolved_path)?;

        let node_name = xfa_node.name.as_ref()?;
        let flattened_node = self
            .field_index_cache
            .get(node_name)
            .and_then(|&idx| self.flattened.iter_nodes().nth(idx));

        let ancestors_visible = self.check_ancestors_visible(node_name);

        Some(XfaNodeRef {
            xfa_node,
            flattened_node,
            som_path: resolved_path,
            ancestors_visible,
        })
    }

    /// Resolve a node by SOM expression (mutable)
    pub fn resolve_mut(&mut self, som_expression: &str) -> Option<XfaNodeRefMut<'_>> {
        let resolved_path = self.som_resolver.resolve_node(som_expression, None)?;

        let xfa_node = Self::find_xfa_node_by_path_mut(&mut self.nodes, &resolved_path)?;

        Some(XfaNodeRefMut {
            xfa_node,
            som_path: resolved_path,
            script_engine: &mut self.script_engine,
        })
    }

    /// Evaluate a JavaScript expression directly in the engine (for debugging/testing).
    #[cfg(test)]
    pub fn eval_js(&mut self, source: &str) -> Result<Option<String>, String> {
        use super::events::{EventRef, ListenScope, RunAt, ScriptContentType, XfaScript};
        self.script_engine.execute_script(&XfaScript {
            source: source.to_string(),
            content_type: ScriptContentType::JavaScript,
            activity: EventActivity::Calculate,
            event_ref: EventRef::Current,
            name: None,
            run_at: RunAt::Client,
            listen: ListenScope::default(),
        })
    }

    /// Execute an event activity on a node.
    ///
    /// For change events, `prev_value` should carry the field's value before
    /// the change so that `xfa.event.prevText` / `newText` are correct.
    pub fn execute_event(
        &mut self,
        som_expression: &str,
        activity: EventActivity,
        prev_value: Option<&str>,
    ) -> Result<EventResult, String> {
        let resolved_path = self
            .som_resolver
            .resolve_node(som_expression, None)
            .ok_or_else(|| format!("Could not resolve SOM expression: {}", som_expression))?;

        let node_name = Self::find_xfa_node_by_path(&self.nodes, &resolved_path)
            .and_then(|n| n.name.clone())
            .ok_or_else(|| format!("Node has no name: {}", resolved_path))?;

        let scripts = self.find_node_scripts(&resolved_path, &activity);

        if scripts.is_empty() {
            return Ok(EventResult::default());
        }

        // Snapshot field values before script execution for change detection
        let pre_values = self.script_engine.get_all_som_field_values();

        let current_value = self
            .script_engine
            .get_field_value(&resolved_path)
            .unwrap_or_default();

        self.script_engine
            .set_current_field(&resolved_path, &node_name, &current_value);

        // Set up $event context before script execution (XFA 3.3 §10 pp.398-404)
        self.script_engine
            .update_event_context(&activity, &resolved_path, prev_value);

        let mut changed_fields = Vec::new();
        for script in &scripts {
            let result = self.script_engine.execute_script(script);
            if let Ok(Some(value)) = result {
                // Empty strings are valid per XFA spec (e.g. rawValue = "" clears a field)
                changed_fields.push(resolved_path.clone());
                // Update engine with the script return value
                self.script_engine
                    .update_field_value(&resolved_path, &value);
            }
        }

        let mut presence_changed =
            if let Some(presence) = self.script_engine.get_current_field_presence() {
                Self::apply_presence_by_path(&mut self.nodes, &resolved_path, presence);
                true
            } else {
                false
            };

        let som_presence_changes = self.script_engine.get_all_som_presence_changes();
        for (som_path, presence_str) in &som_presence_changes {
            let presence = presence_str.parse().unwrap_or_default();
            Self::apply_presence_by_path(&mut self.nodes, som_path, presence);
            presence_changed = true;
        }
        // Update initial_presence baseline so subsequent events detect reverts correctly
        for (som_path, presence_str) in &som_presence_changes {
            self.script_engine
                .update_initial_presence(&SomPath::new(som_path), presence_str);
        }

        // Detect side-effect value changes by comparing with pre-execution snapshot.
        // Per XFA 3.3 §10: when a script sets rawValue on another field, that
        // assignment updates the Form DOM.  We must propagate these side-effect
        // changes back into the XFA node tree so that subsequent reflattening
        // picks them up (mirroring the write-back already done for presence).
        let post_values = self.script_engine.get_all_som_field_values();
        for (field_name, new_value) in &post_values {
            let field_som_path = SomPath::new(field_name);
            if pre_values.get(field_name) != Some(new_value) {
                changed_fields.push(field_som_path.clone());
                // Write the updated value back to the XFA node tree
                if let Some(node) =
                    Self::find_xfa_node_by_path_mut(&mut self.nodes, &field_som_path)
                {
                    XfaNodeRefMut::set_node_value(node, new_value);
                }
            }
        }

        // Event propagation: walk up to ancestor containers, firing any
        // handlers with listen="refAndDescendents" that match this activity.
        // Per XFA 3.3 §10 p.387: "events can now propagate upward to
        // enclosing containers."
        let mut ancestor = resolved_path.parent();
        while let Some(ancestor_path) = ancestor {
            let propagating_scripts = self.find_propagating_scripts(&ancestor_path, &activity);
            if !propagating_scripts.is_empty() {
                let ancestor_name = ancestor_path.name().to_string();
                let ancestor_value = self
                    .script_engine
                    .get_field_value(&ancestor_path)
                    .unwrap_or_default();
                self.script_engine.set_current_field(
                    &ancestor_path,
                    &ancestor_name,
                    &ancestor_value,
                );
                // Keep $event.target pointing at the ORIGINAL target
                self.script_engine
                    .update_event_context(&activity, &resolved_path, prev_value);

                for script in &propagating_scripts {
                    let result = self.script_engine.execute_script(script);
                    if let Ok(Some(value)) = result {
                        changed_fields.push(ancestor_path.clone());
                        self.script_engine
                            .update_field_value(&ancestor_path, &value);
                    }
                }
            }
            ancestor = ancestor_path.parent();
        }

        let values_changed = !changed_fields.is_empty();

        if values_changed || presence_changed {
            self.dirty = true;
        }

        Ok(EventResult {
            values_changed,
            presence_changed,
            changed_fields,
        })
    }

    /// Convenience method to execute a click event
    pub fn click(&mut self, som_expression: &str) -> Result<EventResult, String> {
        self.execute_event(som_expression, EventActivity::Click, None)
    }

    /// Convenience method to execute a change event
    pub fn change(&mut self, som_expression: &str) -> Result<EventResult, String> {
        self.execute_event(som_expression, EventActivity::Change, None)
    }

    /// Convenience method to execute an initialize event
    pub fn initialize(&mut self, som_expression: &str) -> Result<EventResult, String> {
        self.execute_event(som_expression, EventActivity::Initialize, None)
    }

    /// Convenience method to execute an enter event
    pub fn enter(&mut self, som_expression: &str) -> Result<EventResult, String> {
        self.execute_event(som_expression, EventActivity::Enter, None)
    }

    /// Convenience method to execute an exit event
    pub fn exit(&mut self, som_expression: &str) -> Result<EventResult, String> {
        self.execute_event(som_expression, EventActivity::Exit, None)
    }

    /// Re-flatten the form to reflect any changes
    pub fn refresh(&mut self) -> Result<(), String> {
        // Duplicate XFA nodes for dynamic subform instances (setInstances(N))
        // before reflattening so each instance produces its own flattened output.
        self.apply_dynamic_instances();

        // Sync presence changes from JS engine back to XFA nodes so that
        // reflattening picks up visibility toggled by scripts.
        let presence_changes = self.script_engine.get_all_som_presence_changes();
        for (som_path, presence_str) in &presence_changes {
            let presence: crate::xfa::Presence = presence_str.parse().unwrap();
            Self::apply_presence_by_path(&mut self.nodes, som_path, presence);
        }

        let values = self.script_engine.get_all_field_values_for_flattening();
        self.flattened = Flattened::reflatten(&self.nodes, &values)?;
        self.som_resolver = SomResolver::from_nodes(&self.nodes);
        self.field_index_cache = Self::build_field_index_cache(&self.flattened);
        self.dirty = false;
        Ok(())
    }

    /// Apply dynamic subform instances to the XFA node tree.
    ///
    /// When scripts call `instanceManager.setInstances(N)`, the JS engine creates
    /// N clones of a subform with independent state.  This method:
    ///
    /// 1. Resolves xfa:embed references on the original subform with instance 0 values
    /// 2. For instances 1..N, finds embed-containing draws within the subform tree
    ///    and creates additional draw nodes with per-instance resolved text
    ///
    /// This avoids the complexities of full subform cloning (SOM path conflicts,
    /// overlapping content merging) by only duplicating the visible draw elements.
    fn apply_dynamic_instances(&mut self) {
        let instances = self.script_engine.get_dynamic_instances();

        for (subform_path, count, instance_values) in instances {
            if count <= 1 || instance_values.len() < count {
                continue;
            }

            let Some(subform) = Self::find_xfa_node_by_path_mut(&mut self.nodes, &subform_path)
            else {
                continue;
            };

            // Deduplicate: skip if we already processed this subform
            let already_processed = subform.children.iter().any(|c| {
                c.name
                    .as_deref()
                    .map(|n| n.contains("_inst"))
                    .unwrap_or(false)
            });
            if already_processed {
                continue;
            }

            let id_map = Self::build_subform_id_map(subform);

            // Pass 1: Create copies for instances 1..N from the UNMODIFIED draws
            // (before resolving instance 0 values on the original).
            for i in 1..count {
                Self::insert_instance_draws(subform, &id_map, &instance_values[i], i);
            }

            // Pass 2: Resolve embeds on the ORIGINAL with instance 0 values.
            // This must happen AFTER creating copies so they start from the
            // unmodified template with intact xfa:embed references.
            Self::write_instance_values(subform, &instance_values[0]);
            Self::resolve_embeds_in_subtree(subform, &id_map, &instance_values[0]);
        }
    }

    /// Insert additional draw nodes into a subform for a specific instance.
    ///
    /// Walks the subform tree to find draws (and fields used as labels) and creates
    /// copies with per-instance resolved text.  Each copy gets a unique name suffix
    /// and a vertical offset so it doesn't overlap with the original.
    fn insert_instance_draws(
        node: &mut XfaNode,
        id_map: &HashMap<String, String>,
        values: &HashMap<String, String>,
        instance_idx: usize,
    ) {
        // Collect draw/field nodes to duplicate (we can't modify children while iterating)
        let mut inserts: Vec<(usize, XfaNode)> = Vec::new();

        for (idx, child) in node.children.iter().enumerate() {
            let is_draw = matches!(child.kind, XfaNodeKind::Draw);
            let is_field = matches!(child.kind, XfaNodeKind::Field);

            if is_draw || is_field {
                // Check if this node (or its descendants) has xfa:embed refs
                // For instance 0 those are already resolved — but for clones,
                // we start from the resolved-for-instance-0 text.
                // Instead, check if there's a name that matches the per-instance
                // values (meaning the script sets different values per instance).
                if let Some(name) = &child.name {
                    // Check if any value key references this node
                    let has_instance_specific = values
                        .keys()
                        .any(|k| k == name || k.starts_with(&format!("{}.", name)));

                    if has_instance_specific || Self::node_has_embeds(child) {
                        let mut clone = child.clone();
                        // Give unique name
                        if let Some(ref mut n) = clone.name {
                            *n = format!("{}_inst{}", n, instance_idx);
                        }
                        // Offset y position
                        let h = child.h.unwrap_or_else(|| crate::xfa::num(20.0));
                        let original_y = child.y.unwrap_or(rust_decimal::Decimal::ZERO);
                        clone.y =
                            Some(original_y + h * rust_decimal::Decimal::from(instance_idx as u32));
                        // Write instance values and resolve any remaining embeds
                        Self::write_instance_values(&mut clone, values);
                        Self::resolve_embeds_in_subtree(&mut clone, id_map, values);
                        inserts.push((idx, clone));
                    }
                }
            }
        }

        // Insert clones right after their originals (process in reverse to keep indices valid)
        for (idx, clone) in inserts.into_iter().rev() {
            node.children.insert(idx + 1, clone);
        }

        // Recurse into child subforms (which may also contain draws)
        for child in &mut node.children {
            if matches!(child.kind, XfaNodeKind::Subform)
                && !child
                    .name
                    .as_deref()
                    .map(|n| n.contains("_inst"))
                    .unwrap_or(false)
            {
                Self::insert_instance_draws(child, id_map, values, instance_idx);
            }
        }
    }

    /// Check if a node or any of its descendants has xfa:embed attributes
    fn node_has_embeds(node: &XfaNode) -> bool {
        if node.attributes.contains_key("xfa:embed") {
            return true;
        }
        node.children.iter().any(Self::node_has_embeds)
    }

    /// Build a mapping from element ID to relative field path within a subform.
    ///
    /// Mirrors the SOM path construction rules: only subform and exclGroup names
    /// extend the parent path prefix.  This produces keys compatible with
    /// `collect_instance_values` in the script engine.
    fn build_subform_id_map(node: &XfaNode) -> HashMap<String, String> {
        fn collect(node: &XfaNode, prefix: &str, map: &mut HashMap<String, String>) {
            for child in &node.children {
                let name = child.name.as_deref().unwrap_or("");
                let is_container =
                    matches!(child.kind, XfaNodeKind::Subform | XfaNodeKind::ExclGroup);

                let child_path = if !name.is_empty() {
                    if prefix.is_empty() {
                        name.to_string()
                    } else {
                        format!("{}.{}", prefix, name)
                    }
                } else {
                    prefix.to_string()
                };

                if let Some(id) = child.attributes.get("id") {
                    if !child_path.is_empty() {
                        map.insert(id.clone(), child_path.clone());
                    }
                }

                // Only container nodes extend the parent path for their children
                let next_prefix = if !name.is_empty() && is_container {
                    child_path.as_str()
                } else {
                    prefix
                };
                collect(child, next_prefix, map);
            }
        }

        let mut map = HashMap::new();
        collect(node, "", &mut map);
        map
    }

    /// Write per-instance field values into an XFA subform node tree.
    fn write_instance_values(node: &mut XfaNode, values: &HashMap<String, String>) {
        fn walk(node: &mut XfaNode, values: &HashMap<String, String>, prefix: &str) {
            for child in &mut node.children {
                let name = child.name.clone().unwrap_or_default();
                let is_container =
                    matches!(child.kind, XfaNodeKind::Subform | XfaNodeKind::ExclGroup);

                let child_path = if !name.is_empty() {
                    if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{}.{}", prefix, name)
                    }
                } else {
                    prefix.to_string()
                };

                // Set value on field nodes that have a matching entry
                if matches!(child.kind, XfaNodeKind::Field) {
                    if let Some(value) = values.get(&child_path) {
                        XfaNodeRefMut::set_node_value(child, value);
                    }
                }

                let next_prefix = if !name.is_empty() && is_container {
                    child_path.as_str()
                } else {
                    prefix
                };
                walk(child, values, next_prefix);
            }
        }
        walk(node, values, "");
    }

    /// Resolve xfa:embed references on draws/fields in a subform by computing
    /// the full text string from all embedded values and replacing the value
    /// subtree with plain text.
    ///
    /// This avoids whitespace issues from modifying individual embed spans.
    fn resolve_embeds_in_subtree(
        node: &mut XfaNode,
        id_map: &HashMap<String, String>,
        values: &HashMap<String, String>,
    ) {
        // If this node is a draw/field with embeds, resolve it directly
        let is_draw_or_field = node.kind.is_draw() || node.kind.is_field();
        if is_draw_or_field && Self::node_has_embeds(node) {
            if let Some(text) = Self::compute_resolved_text(node, id_map, values) {
                Self::set_draw_value_text(node, &text);
            }
            return; // No need to recurse further into draw/field internals
        }

        // Process all children: resolve embeds on draws/fields, recurse into subforms
        for child in &mut node.children {
            Self::resolve_embeds_in_subtree(child, id_map, values);
        }
    }

    /// Compute the full resolved text from a draw/field node's value subtree.
    /// Walks body > p > span children, resolving xfa:embed references and
    /// preserving inter-span spacing.
    fn compute_resolved_text(
        node: &XfaNode,
        id_map: &HashMap<String, String>,
        values: &HashMap<String, String>,
    ) -> Option<String> {
        // Find the value > exData > body > p structure
        fn find_paragraph(node: &XfaNode) -> Option<&XfaNode> {
            for child in &node.children {
                match &child.kind {
                    XfaNodeKind::Value => return find_paragraph(child),
                    XfaNodeKind::Element { tag_name, .. }
                        if matches!(tag_name.as_str(), "value" | "exData" | "body") =>
                    {
                        return find_paragraph(child);
                    }
                    XfaNodeKind::Element { tag_name, .. } if tag_name == "p" => {
                        return Some(child);
                    }
                    _ => {}
                }
            }
            None
        }

        let p = find_paragraph(node)?;
        let mut parts = Vec::new();
        for child in &p.children {
            match &child.kind {
                XfaNodeKind::Element {
                    tag_name,
                    text_content,
                } if tag_name == "span" => {
                    if let Some(embed_ref) = child.attributes.get("xfa:embed") {
                        let id = embed_ref.strip_prefix('#').unwrap_or(embed_ref);
                        if let Some(rel_path) = id_map.get(id) {
                            parts.push(values.get(rel_path).cloned().unwrap_or_default());
                        }
                    } else {
                        // Non-embed span: use text_content or child text; treat
                        // empty spans between text-producing siblings as space.
                        let text = text_content
                            .as_deref()
                            .or_else(|| {
                                child.children.iter().find_map(|c| {
                                    if let XfaNodeKind::Text { content } = &c.kind {
                                        Some(content.as_str())
                                    } else {
                                        None
                                    }
                                })
                            })
                            .unwrap_or("");
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            parts.push(trimmed.to_string());
                        } else if !parts.is_empty() {
                            // Empty span between other content → space
                            parts.push(" ".to_string());
                        }
                    }
                }
                XfaNodeKind::Text { content } => {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                }
                _ => {}
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(""))
        }
    }

    /// Replace a draw/field node's value subtree with a plain text value.
    fn set_draw_value_text(node: &mut XfaNode, text: &str) {
        // Find or create the value child
        if let Some(value_child) = node.children.iter_mut().find(|c| {
            matches!(c.kind, XfaNodeKind::Value)
                || matches!(&c.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "value")
        }) {
            // Replace entire value subtree with a single text node
            value_child.children.clear();
            value_child.children.push(XfaNode {
                kind: XfaNodeKind::Text {
                    content: text.to_string(),
                },
                name: None,
                children: Vec::new(),
                attributes: HashMap::new(),
                x: None,
                y: None,
                w: None,
                h: None,
                min_w: None,
                min_h: None,
                max_w: None,
                max_h: None,
                layout: None,
                rotate: 0,
                margin_top: None,
                margin_bottom: None,
                margin_left: None,
                margin_right: None,
                border: None,
                font: None,
                para: None,
                presence: crate::xfa::Presence::Visible,
            });
        }
    }

    /// Return all SOM presence changes detected by the script engine.
    pub fn get_presence_changes(&mut self) -> HashMap<String, String> {
        self.script_engine.get_all_som_presence_changes()
    }

    /// Execute change event scripts on the parent exclGroup when a radio button is selected.
    pub fn trigger_change_on_excl_group(
        &mut self,
        field_path: &str,
        prev_value: Option<&str>,
    ) -> Result<EventResult, String> {
        let excl_group_path = self.find_parent_excl_group_by_path(field_path);

        if let Some(ref excl_path) = excl_group_path {
            let result = self.execute_event(excl_path, EventActivity::Change, prev_value)?;
            self.cascade_calculations(excl_path)?;
            self.run_ancestor_calculate_scripts(excl_path)?;
            Ok(result)
        } else {
            let resolved_path = self
                .som_resolver
                .resolve_node(field_path, None)
                .unwrap_or_else(|| SomPath::from(field_path));
            let result = self.execute_event(field_path, EventActivity::Change, prev_value)?;
            self.cascade_calculations(&resolved_path)?;
            self.run_ancestor_calculate_scripts(&resolved_path)?;
            Ok(result)
        }
    }

    /// Set a field value as if the user interacted with it.
    ///
    /// Unlike the low-level `set_raw_value` (which is a pure setter), this method
    /// also fires the `change` event and cascades dependent calculations — matching
    /// the XFA 3.3 spec requirement that change events fire when a user "makes a
    /// selection from a choice list or drop-down menu, checks or unchecks a checkbox".
    ///
    /// Use this for any user-simulated interaction (exhaustive exploration, replay).
    /// For programmatic / calculated value changes, use `set_raw_value` directly.
    pub fn set_value_as_user(
        &mut self,
        field_path: &str,
        value: &str,
    ) -> Result<EventResult, String> {
        let resolved_path = self
            .som_resolver
            .resolve_node(field_path, None)
            .ok_or_else(|| format!("Could not resolve field: {}", field_path))?;

        // Capture the previous value BEFORE updating so that
        // xfa.event.prevText is correct in change event scripts.
        let prev_value = self
            .script_engine
            .get_field_value(&resolved_path)
            .unwrap_or_default();

        // Update engine (source of truth) and XFA node
        self.script_engine.update_field_value(&resolved_path, value);
        if let Some(node) = Self::find_xfa_node_by_path_mut(&mut self.nodes, &resolved_path) {
            XfaNodeRefMut::set_node_value(node, value);
        }

        self.dirty = true;

        // Fire change event and cascade calculations
        self.trigger_change_on_excl_group(&resolved_path, Some(&prev_value))
    }

    /// Select a radio button in an exclusion group.
    pub fn select_radio_button(&mut self, radio_button_path: &str) -> Result<EventResult, String> {
        let resolved_path = self
            .som_resolver
            .resolve_node(radio_button_path, None)
            .ok_or_else(|| format!("Could not resolve radio button: {}", radio_button_path))?;

        // Determine the value this radio button contributes to its exclGroup.
        // Per XFA 3.3 §4: the on-value comes from the <items> element.
        // Fall back to deriving from the field name only if no <items> exist.
        let button_value = Self::find_xfa_node_by_path(&self.nodes, &resolved_path)
            .and_then(|node| node.extract_item_values().0)
            .unwrap_or_else(|| {
                let button_name = resolved_path.name();
                button_name
                    .strip_prefix("RB_")
                    .or_else(|| button_name.rsplit('_').next())
                    .unwrap_or(button_name)
                    .to_string()
            });

        // Capture the previous value before updating for change event context.
        let _prev_value = self
            .script_engine
            .get_field_value(&resolved_path)
            .unwrap_or_default();

        // Update engine (source of truth) and XFA node
        self.script_engine.update_field_value(&resolved_path, "1");

        if let Some(node) = Self::find_xfa_node_by_path_mut(&mut self.nodes, &resolved_path) {
            node.attributes
                .insert("rawValue".to_string(), "1".to_string());
        }

        let excl_group_path = self.find_parent_excl_group_by_path(&resolved_path);

        if let Some(ref excl_path) = excl_group_path {
            let excl_prev = self
                .script_engine
                .get_field_value(excl_path)
                .unwrap_or_default();

            self.script_engine
                .update_field_value(excl_path, &button_value);

            if let Some(excl_node) = Self::find_xfa_node_by_path_mut(&mut self.nodes, excl_path) {
                excl_node
                    .attributes
                    .insert("rawValue".to_string(), button_value.to_string());
            }

            self.dirty = true;

            self.trigger_change_on_excl_group(&resolved_path, Some(&excl_prev))
        } else {
            self.dirty = true;
            Ok(EventResult::default())
        }
    }

    /// Run calculate scripts for all fields that depend on the changed field.
    pub fn cascade_calculations(&mut self, changed_field: &SomPath) -> Result<(), String> {
        let dependents = self
            .dependency_tracker
            .get_dependents_cascade(changed_field);

        if dependents.is_empty() {
            return Ok(());
        }

        for dependent_path in dependents {
            let scripts = self
                .script_registry
                .get_event_scripts(&dependent_path, &EventActivity::Calculate);

            for registered_script in scripts {
                self.script_engine.set_current_field(
                    &registered_script.owner_path,
                    &registered_script.owner_name,
                    "",
                );

                if let Ok(Some(value)) =
                    self.script_engine.execute_script(&registered_script.script)
                    && !value.is_empty()
                {
                    // Update engine directly (source of truth)
                    self.script_engine
                        .update_field_value(&dependent_path, &value);
                }
            }
        }

        self.dirty = true;
        Ok(())
    }

    /// Run calculate scripts on ancestor subforms of a changed field.
    ///
    /// Per XFA 3.3: calculate scripts on container nodes (subforms) fire when
    /// any descendant field value changes. Walk upward from the changed field
    /// and execute calculate scripts registered on each ancestor.
    fn run_ancestor_calculate_scripts(&mut self, field_path: &SomPath) -> Result<(), String> {
        let path_str = field_path.as_str().to_string();
        let mut pos = path_str.len();
        while let Some(dot) = path_str[..pos].rfind('.') {
            let ancestor = &path_str[..dot];
            let ancestor_path = SomPath::new(ancestor);

            let has_calc = self
                .script_registry
                .get_event_scripts(&ancestor_path, &EventActivity::Calculate)
                .len()
                > 0;

            if has_calc {
                // Use execute_event to correctly handle presence changes,
                // value side-effects, and script object calls.
                let _ = self.execute_event(ancestor, EventActivity::Calculate, None);
            }

            pos = dot;
        }

        Ok(())
    }

    /// Find the parent exclGroup for a given field (public API)
    pub fn find_excl_group_for_field(&self, field_name_or_path: &str) -> Option<SomPath> {
        if field_name_or_path.contains('.') {
            self.find_parent_excl_group_by_path(field_name_or_path)
        } else {
            self.find_parent_excl_group(field_name_or_path)
        }
    }

    /// Find the parent exclGroup for a given SOM path
    fn find_parent_excl_group_by_path(&self, som_path: &str) -> Option<SomPath> {
        let parts: Vec<&str> = som_path.split('.').collect();
        if parts.is_empty() {
            return None;
        }

        fn walk_path_for_excl_group(
            nodes: &[XfaNode],
            parts: &[&str],
            idx: usize,
            current_excl_group_path: Option<String>,
            current_path: &str,
        ) -> Option<String> {
            if idx >= parts.len() {
                return current_excl_group_path;
            }

            let target_name = parts[idx];

            for node in nodes {
                let node_name = node.name.as_deref();
                let is_excl_group = node.kind.is_exclgroup();

                let node_path = if let Some(name) = node_name {
                    if current_path.is_empty() {
                        Some(name.to_string())
                    } else {
                        Some(format!("{}.{}", current_path, name))
                    }
                } else {
                    None
                };

                let excl_group_for_children = if is_excl_group && node_path.is_some() {
                    node_path.clone()
                } else {
                    current_excl_group_path.clone()
                };

                if node_name == Some(target_name) {
                    if idx == parts.len() - 1 {
                        return current_excl_group_path;
                    }
                    let next_path = node_path.unwrap_or_else(|| current_path.to_string());
                    return walk_path_for_excl_group(
                        &node.children,
                        parts,
                        idx + 1,
                        excl_group_for_children,
                        &next_path,
                    );
                } else if node_name.is_none()
                    && let Some(found) = walk_path_for_excl_group(
                        &node.children,
                        parts,
                        idx,
                        excl_group_for_children,
                        current_path,
                    )
                {
                    return Some(found);
                }
            }

            None
        }

        walk_path_for_excl_group(&self.nodes, &parts, 0, None, "").map(SomPath::new)
    }

    fn find_parent_excl_group(&self, field_path: &str) -> Option<SomPath> {
        fn find_excl_group_parent(
            nodes: &[XfaNode],
            target_name: &str,
            current_excl_group: Option<&str>,
        ) -> Option<String> {
            for node in nodes {
                let is_excl_group = node.kind.is_exclgroup();

                let excl_group_for_children = if is_excl_group {
                    node.name.as_deref()
                } else {
                    current_excl_group
                };

                if node.name.as_deref() == Some(target_name) {
                    return current_excl_group.map(|s| s.to_string());
                }

                if let Some(found) =
                    find_excl_group_parent(&node.children, target_name, excl_group_for_children)
                {
                    return Some(found);
                }
            }
            None
        }

        let target_name = field_path.rsplit('.').next().unwrap_or(field_path);
        find_excl_group_parent(&self.nodes, target_name, None).map(SomPath::new)
    }

    fn check_ancestors_visible(&self, target_name: &str) -> bool {
        fn check_path(nodes: &[XfaNode], target: &str, parent_hidden: bool) -> Option<bool> {
            for node in nodes {
                let node_presence = node.get_presence();
                let is_hidden = node_presence.should_skip_layout() || parent_hidden;

                if node.name.as_deref() == Some(target) {
                    return Some(!is_hidden);
                }

                if let Some(result) = check_path(&node.children, target, is_hidden) {
                    return Some(result);
                }
            }
            None
        }

        check_path(&self.nodes, target_name, false).unwrap_or(true)
    }

    /// Check if a node at the given SOM path is visible.
    pub fn is_path_visible(&self, som_path: &str) -> bool {
        let parts: Vec<&str> = som_path.split('.').collect();
        if parts.is_empty() {
            return true;
        }

        fn walk_path(nodes: &[XfaNode], parts: &[&str], idx: usize) -> bool {
            if idx >= parts.len() {
                return true;
            }

            let target_name = parts[idx];

            for node in nodes {
                // Skip PageSet nodes and the XDP "form" data tree — they
                // contain duplicates of template subform names with different
                // (or default) presence values that would produce false
                // positives.
                if matches!(&node.kind, XfaNodeKind::PageSet) {
                    continue;
                }
                if let XfaNodeKind::Element { tag_name, .. } = &node.kind {
                    if tag_name == "form" {
                        continue;
                    }
                }

                let node_presence = node.get_presence();
                let is_hidden = node_presence.should_skip_layout();

                if node.name.as_deref() == Some(target_name) {
                    if is_hidden {
                        return false;
                    }
                    return walk_path(&node.children, parts, idx + 1);
                }

                if !is_hidden && walk_path(&node.children, parts, idx) {
                    return true;
                }
            }

            false
        }

        walk_path(&self.nodes, &parts, 0)
    }

    /// Register a dependency between fields.
    pub fn add_dependency(&mut self, dependent_field: &str, source_field: &str) {
        self.dependency_tracker
            .add_dependency(&SomPath::new(dependent_field), &SomPath::new(source_field));
    }

    /// Get the script registry (read-only)
    pub fn script_registry(&self) -> &ScriptRegistry {
        &self.script_registry
    }

    /// Get a shared reference to the script registry `Arc`.
    pub fn script_registry_arc(&self) -> Arc<ScriptRegistry> {
        Arc::clone(&self.script_registry)
    }

    /// Check if the form has uncommitted changes that require refresh
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Get a computed value by field name or path
    pub fn get_computed_value(&mut self, name: &str) -> Option<String> {
        self.script_engine.get_field_value(&SomPath::new(name))
    }

    /// Get the page dimensions
    pub fn page_size(&self) -> (Num, Num) {
        (self.flattened.page.width, self.flattened.page.height)
    }

    /// Get an iterator over all flattened nodes (read-only)
    pub fn flattened_nodes(&self) -> impl Iterator<Item = &FlattenedNode> {
        self.flattened.iter_nodes()
    }

    /// Get the underlying Flattened struct
    pub fn flattened(&self) -> &Flattened {
        &self.flattened
    }

    /// Get the underlying Flattened struct (mutable)
    pub fn flattened_mut(&mut self) -> &mut Flattened {
        &mut self.flattened
    }

    /// Get all field names in the form
    pub fn field_names(&self) -> Vec<String> {
        self.field_index_cache.keys().cloned().collect()
    }

    /// Get access to the underlying XFA nodes (read-only)
    pub fn xfa_nodes(&self) -> &[XfaNode] {
        &self.nodes
    }

    /// Get the current field values from the persistent script engine.
    ///
    /// Returns the same map used by [`refresh`] for flattening. Useful for
    /// snapshotting the form state after init or after applying selections so
    /// that a subsequent [`from_post_init`] call can skip script execution.
    pub fn current_field_values(&mut self) -> HashMap<SomPath, String> {
        self.script_engine.get_all_field_values_for_flattening()
    }

    // ========================================================================
    // Private helper methods
    // ========================================================================

    fn build_script_registry(nodes: &[XfaNode]) -> ScriptRegistry {
        let mut registry = ScriptRegistry::new();

        fn build_parent_child_map(nodes: &[XfaNode]) -> HashMap<String, Vec<(String, String)>> {
            let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();

            fn collect(
                nodes: &[XfaNode],
                parent: Option<&str>,
                map: &mut HashMap<String, Vec<(String, String)>>,
            ) {
                for node in nodes {
                    let name = node.name.clone().unwrap_or_default();
                    let id = node.attributes.get("id").cloned().unwrap_or_default();

                    let is_field = node.kind.is_field();
                    let is_subform = node.kind.is_subform();
                    let is_excl_group = node.kind.is_exclgroup();

                    if is_field
                        && !name.is_empty()
                        && let Some(p) = parent
                    {
                        map.entry(p.to_string())
                            .or_default()
                            .push((name.clone(), id.clone()));
                    }

                    let next_parent = if (is_subform || is_excl_group) && !name.is_empty() {
                        Some(name.as_str())
                    } else {
                        parent
                    };

                    collect(&node.children, next_parent, map);
                }
            }

            collect(nodes, None, &mut map);
            map
        }

        let parent_child_map = build_parent_child_map(nodes);

        fn collect_scripts(
            nodes: &[XfaNode],
            parent_path: &str,
            registry: &mut ScriptRegistry,
            parent_child_map: &HashMap<String, Vec<(String, String)>>,
        ) {
            for node in nodes {
                let name = node.name.clone().unwrap_or_default();
                let node_path = if parent_path.is_empty() {
                    name.clone()
                } else if !name.is_empty() {
                    format!("{}.{}", parent_path, name)
                } else {
                    parent_path.to_string()
                };

                let child_fields = parent_child_map.get(&name).cloned().unwrap_or_default();

                let scripts = parse_events_from_node(&node.children);
                for script in scripts {
                    let script_type = ScriptType::from_activity(&script.activity);

                    registry.register(RegisteredScript {
                        script,
                        owner_path: SomPath::new(node_path.clone()),
                        owner_name: name.clone(),
                        child_fields: child_fields.clone(),
                        script_type,
                    });
                }

                collect_scripts(&node.children, &node_path, registry, parent_child_map);
            }
        }

        collect_scripts(nodes, "", &mut registry, &parent_child_map);
        registry
    }

    fn build_field_index_cache(flattened: &Flattened) -> HashMap<String, usize> {
        let mut cache = HashMap::new();
        for (idx, node) in flattened.iter_nodes().enumerate() {
            match &node.kind {
                FlattenedNodeKind::Field { name, .. } => {
                    cache.insert(name.clone(), idx);
                }
                FlattenedNodeKind::Text {
                    source_name: Some(name),
                    ..
                } => {
                    cache.insert(name.clone(), idx);
                }
                _ => {}
            }
        }
        cache
    }

    fn find_xfa_node_by_path<'a>(nodes: &'a [XfaNode], path: &str) -> Option<&'a XfaNode> {
        let parts: Vec<&str> = path.split('.').collect();
        if parts.is_empty() {
            return None;
        }

        fn walk_path<'a>(nodes: &'a [XfaNode], parts: &[&str], idx: usize) -> Option<&'a XfaNode> {
            if idx >= parts.len() {
                return None;
            }

            let target_name = parts[idx];

            for node in nodes {
                if node.name.as_deref() == Some(target_name) {
                    if idx == parts.len() - 1 {
                        return Some(node);
                    }
                    return walk_path(&node.children, parts, idx + 1);
                } else if node.name.is_none()
                    && let Some(found) = walk_path(&node.children, parts, idx)
                {
                    return Some(found);
                }
            }

            None
        }

        walk_path(nodes, &parts, 0)
    }

    fn find_xfa_node_by_path_mut<'a>(
        nodes: &'a mut [XfaNode],
        path: &str,
    ) -> Option<&'a mut XfaNode> {
        let parts: Vec<&str> = path.split('.').collect();
        if parts.is_empty() {
            return None;
        }

        fn walk_path<'a>(
            nodes: &'a mut [XfaNode],
            parts: &[&str],
            idx: usize,
        ) -> Option<&'a mut XfaNode> {
            if idx >= parts.len() {
                return None;
            }

            let target_name = parts[idx];

            for node in nodes.iter_mut() {
                if node.name.as_deref() == Some(target_name) {
                    if idx == parts.len() - 1 {
                        return Some(node);
                    }
                    return walk_path(&mut node.children, parts, idx + 1);
                } else if node.name.is_none()
                    && let Some(found) = walk_path(&mut node.children, parts, idx)
                {
                    return Some(found);
                }
            }

            None
        }

        walk_path(nodes, &parts, 0)
    }

    fn apply_presence_by_path(nodes: &mut [XfaNode], som_path: &str, presence: Presence) {
        let parts: Vec<&str> = som_path.split('.').collect();
        if parts.is_empty() {
            return;
        }

        fn walk_and_apply(
            nodes: &mut [XfaNode],
            parts: &[&str],
            idx: usize,
            presence: Presence,
        ) -> bool {
            if idx >= parts.len() {
                return false;
            }

            let target_name = parts[idx];

            for node in nodes.iter_mut() {
                if node.name.as_deref() == Some(target_name) {
                    if idx == parts.len() - 1 {
                        node.set_presence(presence);
                        return true;
                    } else if walk_and_apply(&mut node.children, parts, idx + 1, presence) {
                        return true;
                    }
                } else if node.name.is_none()
                    && walk_and_apply(&mut node.children, parts, idx, presence)
                {
                    return true;
                }
            }

            false
        }

        walk_and_apply(nodes, &parts, 0, presence);
    }

    fn find_node_scripts(&self, path: &str, activity: &EventActivity) -> Vec<XfaScript> {
        if let Some(node) = Self::find_xfa_node_by_path(&self.nodes, path) {
            parse_events_from_node(&node.children)
                .into_iter()
                .filter(|script| &script.activity == activity)
                .collect()
        } else {
            vec![]
        }
    }

    /// Find scripts on an ancestor node that have `listen="refAndDescendents"`
    /// matching the given activity.
    /// Per XFA 3.3 §10 p.387: these scripts fire when a descendant triggers
    /// the same activity.
    fn find_propagating_scripts(
        &self,
        ancestor_path: &SomPath,
        activity: &EventActivity,
    ) -> Vec<XfaScript> {
        if let Some(node) = Self::find_xfa_node_by_path(&self.nodes, ancestor_path) {
            parse_events_from_node(&node.children)
                .into_iter()
                .filter(|script| {
                    &script.activity == activity && script.listen == ListenScope::RefAndDescendents
                })
                .collect()
        } else {
            vec![]
        }
    }

    fn extract_and_register_translations(nodes: &[XfaNode], engine: &mut XfaScriptEngine) {
        // Register <text> variables as fields using the shared helper
        let text_vars = crate::xfa::collect_text_variables(nodes);
        for (name, value) in &text_vars {
            engine.register_field(name, name, value);
        }

        // Collect and register <script> variables using the shared helper
        let scripts = crate::xfa::collect_variable_scripts(nodes);

        for (name, content) in &scripts {
            // Per XFA 3.3 §10 pp. 376-378: named script objects expose all
            // top-level variables and functions as properties/methods.
            let script_src = super::script_object::wrap_script_object(name, content, true);

            let _ = engine.execute_script(&XfaScript {
                source: script_src,
                content_type: ScriptContentType::JavaScript,
                activity: EventActivity::Initialize,
                event_ref: EventRef::Form,
                name: Some(name.clone()),
                run_at: RunAt::Client,
                listen: ListenScope::default(),
            });
        }
    }

    fn build_som_hierarchy_with_values(
        nodes: &[XfaNode],
        computed_values: &HashMap<SomPath, String>,
        engine: &mut XfaScriptEngine,
    ) {
        fn get_node_value(
            node: &XfaNode,
            path: &str,
            computed_values: &HashMap<SomPath, String>,
        ) -> String {
            if let Some(value) = computed_values.get(path) {
                return value.clone();
            }
            if let Some(name) = &node.name
                && let Some(value) = computed_values.get(name.as_str())
            {
                return value.clone();
            }
            if let Some(raw) = node.attributes.get("rawValue") {
                return raw.clone();
            }
            for child in &node.children {
                if matches!(child.kind, XfaNodeKind::Value) {
                    for text_child in &child.children {
                        if let XfaNodeKind::Text { content } = &text_child.kind
                            && !content.is_empty()
                        {
                            return content.clone();
                        }
                        if let XfaNodeKind::Element {
                            text_content: Some(content),
                            ..
                        } = &text_child.kind
                            && !content.is_empty()
                        {
                            return content.clone();
                        }
                    }
                }
            }
            String::new()
        }

        /// First pass: register Template DOM nodes in the SOM hierarchy.
        /// Skips `Element { tag_name: "form" }` subtrees — those are handled
        /// by the second pass below.
        fn register_fields(
            nodes: &[XfaNode],
            path: &str,
            computed_values: &HashMap<SomPath, String>,
            engine: &mut XfaScriptEngine,
            parent_is_exclgroup: bool,
        ) {
            for node in nodes {
                // Skip the Form DOM packet entirely — it is processed in a
                // dedicated second pass that only updates existing entries.
                if matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "form")
                {
                    continue;
                }

                let node_path = match &node.name {
                    Some(name) if path.is_empty() => name.clone(),
                    Some(name) => format!("{}.{}", path, name),
                    None => path.to_string(),
                };

                let is_excl_group = node.kind.is_exclgroup();

                let is_draw = node.kind.is_draw();

                let is_field = node.kind.is_field();
                let is_subform = node.kind.is_subform();

                if (is_field || is_subform || is_excl_group || is_draw)
                    && let Some(name) = &node.name
                {
                    let value = get_node_value(node, &node_path, computed_values);
                    let initial_presence = node.get_presence().as_str();

                    if parent_is_exclgroup {
                        // Extract item values from <items> for exclGroup
                        // parent→child propagation (XFA 3.3 §4 pp.195-197,
                        // §17 pp.758-759).
                        let (item_key, off_value) = node.extract_item_values();

                        // Use register_xfa_node with structural exclGroup info
                        // so _exclGroupParent linkage is set up correctly.
                        engine.register_xfa_node(
                            name,
                            &node_path,
                            if path.is_empty() { None } else { Some(path) },
                            is_field,
                            &value,
                            true,
                            item_key.as_deref(),
                            off_value.as_deref(),
                            initial_presence,
                        );
                    } else {
                        engine.register_field_with_presence(
                            &node_path,
                            name,
                            &value,
                            initial_presence,
                            is_subform,
                        );
                    }
                }

                register_fields(
                    &node.children,
                    &node_path,
                    computed_values,
                    engine,
                    is_excl_group,
                );
            }
        }

        /// Second pass: walk `Element { tag_name: "form" }` subtrees and
        /// update initial_presence + form_state values on entries that were
        /// already registered by the template pass.  Does NOT create new JS
        /// objects or touch the SOM hierarchy.
        ///
        /// Per XFA 3.3 §3: the `<form>` packet is a saved snapshot of the
        /// Form DOM.  On reload the Form DOM is rebuilt from the Template DOM
        /// and then the saved content is applied as updates.
        fn update_from_form_dom(
            nodes: &[XfaNode],
            path: &str,
            computed_values: &HashMap<SomPath, String>,
            engine: &mut XfaScriptEngine,
        ) {
            for node in nodes {
                let node_path = match &node.name {
                    Some(name) if path.is_empty() => name.clone(),
                    Some(name) => format!("{}.{}", path, name),
                    None => path.to_string(),
                };

                let is_registrable = matches!(
                    node.kind,
                    XfaNodeKind::Field
                        | XfaNodeKind::Subform
                        | XfaNodeKind::ExclGroup
                        | XfaNodeKind::Draw
                ) || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "draw");

                if is_registrable && node.name.is_some() {
                    let value = get_node_value(node, &node_path, computed_values);
                    let presence = node.get_presence().as_str();
                    let som_path = SomPath::new(&node_path);
                    engine.update_field_presence_baseline(&som_path, &value, presence);
                }

                // Recurse into children of the form DOM subtree.
                update_from_form_dom(&node.children, &node_path, computed_values, engine);
            }
        }

        // Pass 1: Register Template DOM nodes (skip <form> subtrees).
        register_fields(nodes, "", computed_values, engine, false);

        // Pass 2: Apply Form DOM state to already-registered entries.
        for node in nodes {
            if matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "form") {
                update_from_form_dom(&node.children, "", computed_values, engine);
            }
        }
    }
}
