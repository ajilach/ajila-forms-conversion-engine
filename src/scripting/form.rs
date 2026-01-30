//! XFA Form Interface - High-level API for interacting with XFA forms
//!
//! This module provides the main `XfaForm` struct and node reference types
//! for working with XFA forms at a high level.

use super::dependency::DependencyTracker;
use super::engine::XfaScriptEngine;
use super::events::{parse_events_from_node, EventActivity, EventRef, RunAt, ScriptContentType, XfaScript};
use super::registry::{RegisteredScript, ScriptRegistry, ScriptType};
use super::som::{walk_som_path_mut, SomPath, SomResolver};
use super::state::Presence;

use crate::flattened::{Flattened, FlattenedNode, FlattenedNodeKind};
use crate::xfa::{Num, XfaNode, XfaNodeKind};

use std::collections::HashMap;

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
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                if tag_name == "event" {
                    for script_child in &child.children {
                        if let XfaNodeKind::Element {
                            tag_name: script_tag,
                            text_content,
                            ..
                        } = &script_child.kind
                        {
                            if script_tag == "script" {
                                if let Some(content) = text_content {
                                    if content.contains("addItem") {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Check if this field has a choiceList UI element
    fn has_choice_list(&self) -> bool {
        for child in &self.xfa_node.children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                if tag_name == "ui" {
                    for ui_child in &child.children {
                        if let XfaNodeKind::Element {
                            tag_name: ui_tag, ..
                        } = &ui_child.kind
                        {
                            if ui_tag == "choiceList" {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
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
        for child in &self.xfa_node.children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                if tag_name == "ui" {
                    for ui_child in &child.children {
                        if let XfaNodeKind::Element {
                            tag_name: ui_tag, ..
                        } = &ui_child.kind
                        {
                            if ui_tag == "button" {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
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
        for child in &self.xfa_node.children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                if tag_name == "ui" {
                    for ui_child in &child.children {
                        if let XfaNodeKind::Element {
                            tag_name: ui_tag, ..
                        } = &ui_child.kind
                        {
                            if ui_tag == "checkButton" {
                                return Some(ui_child);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Get the dropdown options (display values and save values)
    pub fn dropdown_options(&self) -> Vec<(String, String)> {
        let mut display_items: Vec<String> = Vec::new();
        let mut save_items: Vec<String> = Vec::new();

        for child in &self.xfa_node.children {
            if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                if tag_name == "items" {
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
        }

        if save_items.is_empty() {
            save_items = display_items.clone();
        }

        display_items
            .into_iter()
            .zip(save_items.into_iter())
            .collect()
    }

    /// Get just the display values for dropdown options
    pub fn dropdown_display_values(&self) -> Vec<String> {
        self.dropdown_options().into_iter().map(|(d, _)| d).collect()
    }

    /// Get just the save values for dropdown options
    pub fn dropdown_save_values(&self) -> Vec<String> {
        self.dropdown_options().into_iter().map(|(_, s)| s).collect()
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
                } => {
                    match tag_name.as_str() {
                        "text" | "integer" | "decimal" | "float" | "boolean" | "date"
                        | "dateTime" | "time" => {
                            if let Some(content) = text_content {
                                values.push(content.clone());
                            } else {
                                values.push(String::new());
                            }
                        }
                        _ => {}
                    }
                }
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
                    if let XfaNodeKind::Text { content } = &text_child.kind {
                        if !content.is_empty() {
                            return Some(content.clone());
                        }
                    }
                    if let XfaNodeKind::Element {
                        text_content: Some(content),
                        ..
                    } = &text_child.kind
                    {
                        if !content.is_empty() {
                            return Some(content.clone());
                        }
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
    /// Reference to the form's computed values cache
    computed_values: &'a mut HashMap<SomPath, String>,
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
    pub fn raw_value(&self) -> Option<String> {
        if let Some(value) = self.computed_values.get(&self.som_path) {
            return Some(value.clone());
        }
        if let Some(name) = &self.xfa_node.name {
            let name_path = SomPath::new(name);
            if let Some(value) = self.computed_values.get(&name_path) {
                return Some(value.clone());
            }
        }

        if let Some(raw) = self.xfa_node.attributes.get("rawValue") {
            return Some(raw.clone());
        }

        XfaNodeRef::extract_value_from_xfa_node(self.xfa_node)
    }

    /// Set the raw value of this node
    pub fn set_raw_value(&mut self, value: &str) {
        self.computed_values
            .insert(self.som_path.clone(), value.to_string());
        if let Some(name) = &self.xfa_node.name {
            self.computed_values
                .insert(SomPath::new(name), value.to_string());
        }

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

    fn set_node_value(node: &mut XfaNode, value: &str) {
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
    /// Language code for translations (e.g., "DE", "EN")
    language: String,
    /// Form identifier
    form_id: String,
    /// SOM resolver for node lookups
    som_resolver: SomResolver,
    /// Cached mapping of field names to their flattened node indices
    field_index_cache: HashMap<String, usize>,
    /// Cached computed values from scripts
    computed_values: HashMap<SomPath, String>,
    /// Registry of all scripts in the form, categorized by type
    script_registry: ScriptRegistry,
    /// Dependency tracker for cascading calculations
    dependency_tracker: DependencyTracker,
    /// Dirty flag - set when changes require refresh
    dirty: bool,
    /// Persistent script engine
    script_engine: XfaScriptEngine,
}

impl XfaForm {
    /// Create a new XFA form from parsed nodes
    pub fn new(mut nodes: Vec<XfaNode>, language: &str, form_id: &str) -> Result<Self, String> {
        let script_registry = Self::build_script_registry(&nodes);
        let dependency_tracker = DependencyTracker::new();

        // Execute scripts using ScriptExecutor
        let script_result =
            crate::script_executor::ScriptExecutor::execute(&nodes, language, form_id);

        // Apply presence changes to the nodes
        crate::script_executor::ScriptExecutor::apply_presence_changes(
            &mut nodes,
            &script_result.presence_changes,
        );

        let computed_values = script_result.computed_values;

        // Flatten with the computed values
        let flattened = Flattened::from_xfa(&nodes, &computed_values)?;

        let som_resolver = SomResolver::from_nodes(&nodes);
        let field_index_cache = Self::build_field_index_cache(&flattened);

        // Create persistent script engine
        let mut script_engine = XfaScriptEngine::new();

        // Initialize engine with form context
        script_engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", language);
        script_engine.register_field("Footer_Line_txtformid", "Footer_Line_txtformid", form_id);
        Self::extract_and_register_translations(&nodes, &mut script_engine);
        Self::build_som_hierarchy_with_values(&nodes, &computed_values, &mut script_engine);

        Ok(XfaForm {
            nodes,
            flattened,
            language: language.to_string(),
            form_id: form_id.to_string(),
            som_resolver,
            field_index_cache,
            computed_values,
            script_registry,
            dependency_tracker,
            dirty: false,
            script_engine,
        })
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
            computed_values: &mut self.computed_values,
        })
    }

    /// Execute an event activity on a node
    pub fn execute_event(
        &mut self,
        som_expression: &str,
        activity: EventActivity,
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

        self.sync_computed_values_to_engine();

        let current_value = self
            .computed_values
            .get(&resolved_path)
            .cloned()
            .unwrap_or_default();

        self.script_engine
            .set_current_field(&resolved_path, &node_name, &current_value);

        let mut changed_fields = Vec::new();
        for script in &scripts {
            let result = self.script_engine.execute_script(script);
            if let Ok(Some(value)) = result {
                if !value.is_empty() {
                    changed_fields.push(resolved_path.clone());
                    self.computed_values
                        .insert(resolved_path.clone(), value.clone());
                    self.computed_values.insert(SomPath::new(&node_name), value);
                }
            }
        }

        let mut presence_changed = if let Some(presence) =
            self.script_engine.get_current_field_presence()
        {
            Self::apply_presence_by_path(&mut self.nodes, &resolved_path, presence);
            true
        } else {
            false
        };

        let som_presence_changes = self.script_engine.get_all_som_presence_changes();
        for (som_path, presence_str) in som_presence_changes {
            let presence = Presence::from_str(&presence_str);
            Self::apply_presence_by_path(&mut self.nodes, &som_path, presence);
            presence_changed = true;
        }

        let som_values = self.script_engine.get_all_som_field_values();
        for (field_path, value) in som_values {
            let field_som_path = SomPath::new(&field_path);
            if !value.is_empty() && self.computed_values.get(&field_som_path) != Some(&value) {
                changed_fields.push(field_som_path.clone());
                self.computed_values.insert(field_som_path, value);
            }
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
        self.execute_event(som_expression, EventActivity::Click)
    }

    /// Convenience method to execute a change event
    pub fn change(&mut self, som_expression: &str) -> Result<EventResult, String> {
        self.execute_event(som_expression, EventActivity::Change)
    }

    /// Convenience method to execute an initialize event
    pub fn initialize(&mut self, som_expression: &str) -> Result<EventResult, String> {
        self.execute_event(som_expression, EventActivity::Initialize)
    }

    /// Convenience method to execute an enter event
    pub fn enter(&mut self, som_expression: &str) -> Result<EventResult, String> {
        self.execute_event(som_expression, EventActivity::Enter)
    }

    /// Convenience method to execute an exit event
    pub fn exit(&mut self, som_expression: &str) -> Result<EventResult, String> {
        self.execute_event(som_expression, EventActivity::Exit)
    }

    /// Re-flatten the form to reflect any changes
    pub fn refresh(&mut self) -> Result<(), String> {
        self.flattened = Flattened::reflatten(&self.nodes, &self.computed_values)?;
        self.som_resolver = SomResolver::from_nodes(&self.nodes);
        self.field_index_cache = Self::build_field_index_cache(&self.flattened);
        self.dirty = false;
        Ok(())
    }

    /// Execute change event scripts on the parent exclGroup when a radio button is selected.
    pub fn trigger_change_on_excl_group(
        &mut self,
        field_path: &str,
    ) -> Result<EventResult, String> {
        let excl_group_path = self.find_parent_excl_group_by_path(field_path);

        if let Some(ref excl_path) = excl_group_path {
            let result = self.execute_event(excl_path, EventActivity::Change)?;
            self.cascade_calculations(excl_path)?;
            Ok(result)
        } else {
            self.execute_event(field_path, EventActivity::Change)
        }
    }

    /// Select a radio button in an exclusion group.
    pub fn select_radio_button(&mut self, radio_button_path: &str) -> Result<EventResult, String> {
        let resolved_path = self
            .som_resolver
            .resolve_node(radio_button_path, None)
            .ok_or_else(|| format!("Could not resolve radio button: {}", radio_button_path))?;

        let button_name = resolved_path.name();

        let button_value = button_name
            .strip_prefix("RB_")
            .or_else(|| button_name.rsplit('_').next())
            .unwrap_or(button_name);

        self.computed_values
            .insert(resolved_path.clone(), "1".to_string());

        if let Some(node) = Self::find_xfa_node_by_path_mut(&mut self.nodes, &resolved_path) {
            node.attributes
                .insert("rawValue".to_string(), "1".to_string());
        }

        let excl_group_path = self.find_parent_excl_group_by_path(&resolved_path);

        if let Some(ref excl_path) = excl_group_path {
            self.computed_values
                .insert(excl_path.clone(), button_value.to_string());

            if let Some(excl_node) = Self::find_xfa_node_by_path_mut(&mut self.nodes, excl_path) {
                excl_node
                    .attributes
                    .insert("rawValue".to_string(), button_value.to_string());
            }

            self.script_engine
                .update_field_value(excl_path, button_value);
            self.script_engine.update_field_value(&resolved_path, "1");

            self.dirty = true;

            self.trigger_change_on_excl_group(&resolved_path)
        } else {
            self.dirty = true;
            Ok(EventResult::default())
        }
    }

    /// Run calculate scripts for all fields that depend on the changed field.
    pub fn cascade_calculations(&mut self, changed_field: &SomPath) -> Result<(), String> {
        let dependents = self.dependency_tracker.get_dependents_cascade(changed_field);

        if dependents.is_empty() {
            return Ok(());
        }

        self.sync_computed_values_to_engine();

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

                if let Ok(Some(value)) = self.script_engine.execute_script(&registered_script.script)
                {
                    if !value.is_empty() {
                        self.computed_values
                            .insert(dependent_path.clone(), value.clone());
                        self.computed_values
                            .insert(SomPath::new(&registered_script.owner_name), value);
                    }
                }
            }
        }

        let som_values = self.script_engine.get_all_som_field_values();
        for (field_path, value) in som_values {
            let field_som_path = SomPath::new(&field_path);
            if !value.is_empty() && self.computed_values.get(&field_som_path) != Some(&value) {
                self.computed_values.insert(field_som_path, value);
            }
        }

        self.dirty = true;
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
                let is_excl_group = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");

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
                } else if node_name.is_none() {
                    if let Some(found) = walk_path_for_excl_group(
                        &node.children,
                        parts,
                        idx,
                        excl_group_for_children,
                        current_path,
                    ) {
                        return Some(found);
                    }
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
                let is_excl_group = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");

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

    /// Check if the form has uncommitted changes that require refresh
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Get a computed value by field name or path
    pub fn get_computed_value(&self, name: &str) -> Option<&String> {
        self.computed_values.get(name)
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

    /// Get all field names in the form
    pub fn field_names(&self) -> Vec<String> {
        self.field_index_cache.keys().cloned().collect()
    }

    /// Get access to the underlying XFA nodes (read-only)
    pub fn xfa_nodes(&self) -> &[XfaNode] {
        &self.nodes
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

                    let is_field = matches!(node.kind, XfaNodeKind::Field)
                        || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "field");
                    let is_subform = matches!(node.kind, XfaNodeKind::Subform)
                        || matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "subform");
                    let is_excl_group = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");

                    if is_field && !name.is_empty() {
                        if let Some(p) = parent {
                            map.entry(p.to_string())
                                .or_default()
                                .push((name.clone(), id.clone()));
                        }
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
                } else if node.name.is_none() {
                    if let Some(found) = walk_path(&node.children, parts, idx) {
                        return Some(found);
                    }
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
                } else if node.name.is_none() {
                    if let Some(found) = walk_path(&mut node.children, parts, idx) {
                        return Some(found);
                    }
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
                } else if node.name.is_none() {
                    if walk_and_apply(&mut node.children, parts, idx, presence) {
                        return true;
                    }
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

    fn sync_computed_values_to_engine(&mut self) {
        for (path, value) in &self.computed_values {
            self.script_engine.update_field_value(path, value);
        }
    }

    fn extract_and_register_translations(nodes: &[XfaNode], engine: &mut XfaScriptEngine) {
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
                                if let Some(name) = &child.name {
                                    if child_tag == "script" {
                                        if let Some(content) = text_content {
                                            if !content.is_empty() {
                                                scripts.push((name.clone(), content.clone()));
                                            }
                                        }
                                        for script_child in child.children.iter() {
                                            if let XfaNodeKind::Element {
                                                text_content: Some(content),
                                                ..
                                            } = &script_child.kind
                                            {
                                                scripts.push((name.clone(), content.clone()));
                                            }
                                            if let XfaNodeKind::Text { content } =
                                                &script_child.kind
                                            {
                                                if !content.is_empty() {
                                                    scripts.push((name.clone(), content.clone()));
                                                }
                                            }
                                        }
                                    } else if child_tag == "text" {
                                        let value = text_content.clone().unwrap_or_default();
                                        text_vars.push((name.clone(), value));
                                    }
                                }
                            }
                        }
                    }
                }
                collect_variable_items(&node.children, scripts, text_vars);
            }
        }

        let mut scripts = Vec::new();
        let mut text_vars = Vec::new();
        collect_variable_items(nodes, &mut scripts, &mut text_vars);

        for (name, value) in &text_vars {
            engine.register_field(name, name, value);
        }

        for (name, content) in &scripts {
            let script_src = format!(
                r#"globalThis.{name} = (function() {{ 
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
                }})();"#,
                name = name,
                content = content
            );

            let _ = engine.execute_script(&XfaScript {
                source: script_src,
                content_type: ScriptContentType::JavaScript,
                activity: EventActivity::Initialize,
                event_ref: EventRef::Form,
                name: Some(name.clone()),
                run_at: RunAt::Client,
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
            if let Some(name) = &node.name {
                if let Some(value) = computed_values.get(name.as_str()) {
                    return value.clone();
                }
            }
            if let Some(raw) = node.attributes.get("rawValue") {
                return raw.clone();
            }
            for child in &node.children {
                if matches!(child.kind, XfaNodeKind::Value) {
                    for text_child in &child.children {
                        if let XfaNodeKind::Text { content } = &text_child.kind {
                            if !content.is_empty() {
                                return content.clone();
                            }
                        }
                        if let XfaNodeKind::Element {
                            text_content: Some(content),
                            ..
                        } = &text_child.kind
                        {
                            if !content.is_empty() {
                                return content.clone();
                            }
                        }
                    }
                }
            }
            String::new()
        }

        fn register_fields(
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

                let is_excl_group = matches!(&node.kind, XfaNodeKind::Element { tag_name, .. } if tag_name == "exclGroup");

                if matches!(node.kind, XfaNodeKind::Field | XfaNodeKind::Subform) || is_excl_group {
                    if let Some(name) = &node.name {
                        let value = get_node_value(node, &node_path, computed_values);
                        let initial_presence = node.get_presence().as_str();
                        engine.register_field_with_presence(&node_path, name, &value, initial_presence);
                    }
                }

                register_fields(&node.children, &node_path, computed_values, engine);
            }
        }

        register_fields(nodes, "", computed_values, engine);
    }
}
