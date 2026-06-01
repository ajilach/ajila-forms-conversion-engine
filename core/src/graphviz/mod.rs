//! GraphViz Decision-Flow Output
//!
//! Generates a DOT graph representing the user flow through an interactive XFA
//! document. Decision points (radio buttons, checkboxes, dropdowns) become
//! diamond-shaped nodes, edges carry the selected value, and leaf nodes show
//! the rendered form image for each terminal state.

use std::collections::HashMap;
use std::fmt::Write;

use crate::structured::{FieldId, FieldNode, FieldType, InputValue, StructuredNode};
use crate::xfa::scripting::SomPath;

// ============================================================================
// Public types
// ============================================================================

/// A single explored form state with the selections that led to it.
#[derive(Debug, Clone)]
pub struct GraphState {
    /// The selections made to reach this state (ordered by depth).
    pub selections: Vec<GraphSelection>,
    /// Human-readable label for this state (e.g. `"default"`, `"RB_1_CB_2"`).
    pub label: String,
}

/// A simplified selection used for graph construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GraphSelection {
    /// The FieldId that identifies the decision point (group for radio buttons).
    pub condition_id: FieldId,
    /// The SOM path of the group (or field) — used as fallback label.
    pub som_path: SomPath,
    /// All equivalent values for this selection (e.g. `["RB_1", "RB_2", "RB_3"]`
    /// when they produce identical output). The first is the primary/representative.
    pub values: Vec<String>,
    /// The kind of selection.
    pub kind: GraphSelectionKind,
}

impl GraphSelection {
    /// The primary (first) value — used for trie insertion and grouping.
    pub fn primary_value(&self) -> &str {
        &self.values[0]
    }
}

/// Kind of selection for display purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphSelectionKind {
    Radio,
    Checkbox,
    Dropdown,
}

/// Mapping from FieldId → human-readable label and option value → option name.
#[derive(Debug, Clone, Default)]
pub struct FieldLabelMap {
    /// FieldId → field label (plain text).
    pub labels: HashMap<FieldId, String>,
    /// FieldId → (option value → option display name).
    pub option_names: HashMap<FieldId, HashMap<String, String>>,
}

impl FieldLabelMap {
    /// Merge another `FieldLabelMap` into this one, adding any entries that
    /// are not already present. This is used to aggregate labels from all
    /// explored states (some fields are only visible in certain states).
    pub fn merge_from(&mut self, other: &FieldLabelMap) {
        for (id, label) in &other.labels {
            self.labels
                .entry(id.clone())
                .or_insert_with(|| label.clone());
        }
        for (id, opts) in &other.option_names {
            let entry = self.option_names.entry(id.clone()).or_default();
            for (value, name) in opts {
                entry.entry(value.clone()).or_insert_with(|| name.clone());
            }
        }
    }
}

// ============================================================================
// Trie for building the decision tree
// ============================================================================

/// A trie node representing either a decision point or a leaf state.
#[derive(Debug)]
struct TrieNode {
    /// FieldId for this decision (None for the root before any decisions).
    condition_id: Option<FieldId>,
    /// SOM path fallback label for this decision.
    som_path: Option<SomPath>,
    /// Edges: (all equivalent values, child trie node).
    /// Multiple values map to the same child when they produce identical output.
    children: Vec<(Vec<String>, TrieNode)>,
    /// Leaf states that terminate at this node (no further decisions).
    leaves: Vec<GraphState>,
    /// Kind of selection at this decision point.
    kind: Option<GraphSelectionKind>,
}

impl TrieNode {
    fn root() -> Self {
        Self {
            condition_id: None,
            som_path: None,
            children: Vec::new(),
            leaves: Vec::new(),
            kind: None,
        }
    }

    /// Insert a state into the trie following its selection path.
    fn insert(&mut self, state: GraphState, depth: usize) {
        if depth >= state.selections.len() {
            // No more decisions — this is a leaf.
            self.leaves.push(state);
            return;
        }

        let sel = &state.selections[depth];
        let primary = sel.primary_value().to_string();
        let all_values = sel.values.clone();

        // Find existing child whose primary value matches.
        let child_idx = self
            .children
            .iter()
            .position(|(vals, _)| vals[0] == primary);

        if let Some(idx) = child_idx {
            // Merge any new equivalent values into the existing edge.
            for v in &all_values {
                if !self.children[idx].0.contains(v) {
                    self.children[idx].0.push(v.clone());
                }
            }
            self.children[idx].1.insert(state, depth + 1);
        } else {
            let mut child = TrieNode {
                condition_id: None,
                som_path: None,
                children: Vec::new(),
                leaves: Vec::new(),
                kind: None,
            };
            // Set the decision info on *this* node if not yet set.
            if self.condition_id.is_none() || self.condition_id.as_ref() == Some(&sel.condition_id)
            {
                self.condition_id = Some(sel.condition_id.clone());
                self.som_path = Some(sel.som_path.clone());
                self.kind = Some(sel.kind);
            }

            child.insert(state, depth + 1);
            self.children.push((all_values, child));
        }
    }
}

// ============================================================================
// DOT generation
// ============================================================================

/// Generate a DOT graph from the explored form states.
///
/// `states` — all explored form states with their selection paths and image paths.
/// `field_labels` — mapping from FieldId to human-readable labels and option names.
///
/// Returns a string containing valid DOT language that can be rendered with
/// `dot -Tpng output.dot -o output.png` (or `-Tsvg`).
pub fn generate_dot(states: &[GraphState], field_labels: &FieldLabelMap) -> String {
    let mut trie = TrieNode::root();
    for state in states {
        trie.insert(state.clone(), 0);
    }

    let mut dot = String::new();
    writeln!(dot, "digraph FormFlow {{").unwrap();
    writeln!(dot, "    rankdir=TB;").unwrap();
    writeln!(dot, "    bgcolor=\"#f8f9fa\";").unwrap();
    writeln!(dot, "    node [fontname=\"Helvetica\", fontsize=11];").unwrap();
    writeln!(dot, "    edge [fontname=\"Helvetica\", fontsize=10];").unwrap();
    writeln!(dot).unwrap();

    let mut counter = 0usize;
    emit_trie_node(&trie, &mut dot, &mut counter, field_labels, None);

    writeln!(dot, "}}").unwrap();
    dot
}

/// Recursively emit DOT nodes and edges for a trie node.
/// Returns the DOT node id assigned to this trie node.
fn emit_trie_node(
    node: &TrieNode,
    dot: &mut String,
    counter: &mut usize,
    field_labels: &FieldLabelMap,
    parent_edge: Option<(&str, &str)>, // (parent_id, edge_label)
) -> String {
    let my_id = format!("n{}", *counter);
    *counter += 1;

    if !node.children.is_empty() {
        // This is a decision node.
        let label = decision_label(node, field_labels);
        writeln!(
            dot,
            "    {} [shape=diamond, style=filled, fillcolor=\"#fff3cd\", label={}, width=2.5, height=1];",
            my_id,
            dot_quote(&label)
        )
        .unwrap();

        // Connect from parent if applicable.
        if let Some((parent_id, edge_label)) = parent_edge {
            writeln!(
                dot,
                "    {} -> {} [label={}];",
                parent_id,
                my_id,
                dot_quote(edge_label)
            )
            .unwrap();
        }

        // Emit children: one edge per value, all values of a group point to the same child.
        for (values, child) in &node.children {
            let child_id = emit_trie_node(child, dot, counter, field_labels, None);
            for value in values {
                let edge_label = option_display_name(node, value, field_labels);
                writeln!(
                    dot,
                    "    {} -> {} [label={}];",
                    my_id,
                    child_id,
                    dot_quote(&edge_label)
                )
                .unwrap();
            }
        }

        // Emit any direct leaves of this decision node (states that terminate here).
        for leaf in &node.leaves {
            emit_leaf(leaf, dot, counter, &my_id, None, field_labels);
        }
    } else {
        // This is a pure leaf collection node (no further decisions).
        // If there's exactly one leaf, emit it directly.
        // If there are multiple, emit each.
        if node.leaves.len() == 1 {
            let leaf = &node.leaves[0];
            emit_leaf_node(leaf, dot, &my_id);
            if let Some((parent_id, edge_label)) = parent_edge {
                writeln!(
                    dot,
                    "    {} -> {} [label={}];",
                    parent_id,
                    my_id,
                    dot_quote(edge_label)
                )
                .unwrap();
            }
        } else if node.leaves.is_empty() {
            // Empty node — shouldn't normally happen, but handle gracefully.
            writeln!(
                dot,
                "    {} [shape=box, label=\"(empty)\", style=dashed];",
                my_id
            )
            .unwrap();
            if let Some((parent_id, edge_label)) = parent_edge {
                writeln!(
                    dot,
                    "    {} -> {} [label={}];",
                    parent_id,
                    my_id,
                    dot_quote(edge_label)
                )
                .unwrap();
            }
        } else {
            // Multiple leaves with no further decisions — create an intermediate
            // invisible node so the parent edge connects once.
            writeln!(dot, "    {} [shape=point, width=0.1];", my_id).unwrap();
            if let Some((parent_id, edge_label)) = parent_edge {
                writeln!(
                    dot,
                    "    {} -> {} [label={}];",
                    parent_id,
                    my_id,
                    dot_quote(edge_label)
                )
                .unwrap();
            }
            for leaf in &node.leaves {
                emit_leaf(leaf, dot, counter, &my_id, None, field_labels);
            }
        }
    }

    my_id
}

/// Emit a single leaf (rendered form state) node.
fn emit_leaf(
    leaf: &GraphState,
    dot: &mut String,
    counter: &mut usize,
    parent_id: &str,
    edge_label: Option<&str>,
    _field_labels: &FieldLabelMap,
) {
    let leaf_id = format!("n{}", *counter);
    *counter += 1;

    emit_leaf_node(leaf, dot, &leaf_id);

    if let Some(lbl) = edge_label {
        writeln!(
            dot,
            "    {} -> {} [label={}];",
            parent_id,
            leaf_id,
            dot_quote(lbl)
        )
        .unwrap();
    } else {
        writeln!(dot, "    {} -> {};", parent_id, leaf_id).unwrap();
    }
}

/// Emit the DOT node definition for a leaf state.
fn emit_leaf_node(leaf: &GraphState, dot: &mut String, node_id: &str) {
    let label = dot_escape(&leaf.label);
    writeln!(
        dot,
        "    {} [shape=box, style=\"filled,rounded\", fillcolor=\"#d4edda\", \
         label=<\
         <TABLE BORDER=\"0\" CELLBORDER=\"0\" CELLSPACING=\"4\">\
         <TR><TD><B>{}</B></TD></TR>\
         </TABLE>>];",
        node_id, label
    )
    .unwrap();
}

// ============================================================================
// Label resolution helpers
// ============================================================================

/// Build a human-readable label for a decision node.
fn decision_label(node: &TrieNode, field_labels: &FieldLabelMap) -> String {
    if let Some(ref cid) = node.condition_id {
        // 1. Use the field's own label if available.
        if let Some(lbl) = field_labels.labels.get(cid) {
            if !lbl.is_empty() {
                return lbl.clone();
            }
        }

        // 2. If no label but option names exist, list them as the decision description.
        if let Some(opts) = field_labels.option_names.get(cid) {
            let mut names: Vec<&str> = opts.values().map(|s| s.as_str()).collect();
            if !names.is_empty() {
                names.sort();
                return names.join(" / ");
            }
        }
    }
    // 3. Fallback: last segment of the SOM path.
    if let Some(ref som) = node.som_path {
        return som.name().to_string();
    }
    "?".to_string()
}

/// Get a human-readable name for an option value at a decision node.
fn option_display_name(node: &TrieNode, value: &str, field_labels: &FieldLabelMap) -> String {
    // For checkboxes, show "checked" / "unchecked".
    if node.kind == Some(GraphSelectionKind::Checkbox) {
        return if value == "checked" {
            "checked ☑".to_string()
        } else {
            "unchecked ☐".to_string()
        };
    }

    // Try to find the option display name from the field label map.
    if let Some(ref cid) = node.condition_id {
        if let Some(options) = field_labels.option_names.get(cid) {
            if let Some(name) = options.get(value) {
                if !name.is_empty() {
                    return name.clone();
                }
            }
        }
    }

    // Fallback: raw value.
    value.to_string()
}

// ============================================================================
// Building the FieldLabelMap from structured nodes
// ============================================================================

/// Walk a set of structured nodes and build a [`FieldLabelMap`] that maps
/// `FieldId`s to their human-readable labels and option value→name mappings.
///
/// This is typically called on the structured output of any single state
/// (all states share the same field definitions for decision-level fields).
pub fn build_field_label_map(nodes: &[StructuredNode]) -> FieldLabelMap {
    let mut map = FieldLabelMap::default();
    collect_field_labels(nodes, &mut map);
    map
}

fn collect_field_labels(nodes: &[StructuredNode], map: &mut FieldLabelMap) {
    for node in nodes {
        match node {
            StructuredNode::Field(field) => {
                register_field(field, map);
            }
            StructuredNode::Group(g) => {
                collect_field_labels(&g.children, map);
            }
            StructuredNode::Conditional(c) => {
                collect_field_labels(std::slice::from_ref(c.content.as_ref()), map);
            }
            StructuredNode::Table(t) => {
                if let Some(ref header) = t.header {
                    collect_field_labels_from_cells(&header.cells, map);
                }
                for row in &t.rows {
                    collect_field_labels_from_cells(&row.cells, map);
                }
            }
            StructuredNode::Repeatable(r) => {
                collect_field_labels(std::slice::from_ref(r.item.as_ref()), map);
            }
            StructuredNode::GridLayout(gl) => {
                let nodes: Vec<&StructuredNode> = gl.elements.iter().map(|e| &e.node).collect();
                for n in nodes {
                    collect_field_labels(std::slice::from_ref(n), map);
                }
            }
            _ => {}
        }
    }
}

fn collect_field_labels_from_cells(cells: &[StructuredNode], map: &mut FieldLabelMap) {
    collect_field_labels(cells, map);
}

fn register_field(field: &FieldNode, map: &mut FieldLabelMap) {
    // Extract label text.
    let label_text = field
        .label
        .as_ref()
        .map(|l| l.as_plain_text())
        .unwrap_or_default();

    if !label_text.is_empty() {
        map.labels.insert(field.name.clone(), label_text);
    }

    // Extract option names for Radio and Select fields.
    let options = match &field.input_type {
        FieldType::Radio { options } => Some(options),
        FieldType::Select { options } => Some(options),
        _ => None,
    };

    if let Some(opts) = options {
        let mut value_names: HashMap<String, String> = HashMap::new();
        for opt in opts {
            let value_str = match &opt.value {
                InputValue::Text(t) => t.clone(),
                InputValue::Bool(b) => {
                    if *b {
                        "checked".to_string()
                    } else {
                        "unchecked".to_string()
                    }
                }
                InputValue::Number(n) => n.to_string(),
            };
            let name_str = opt.name.as_str().to_string();
            if !name_str.is_empty() {
                value_names.insert(value_str, name_str);
            }
        }
        if !value_names.is_empty() {
            map.option_names.insert(field.name.clone(), value_names);
        }
    }
}

// ============================================================================
// DOT string helpers
// ============================================================================

/// Escape a string for use inside DOT HTML-like labels.
fn dot_escape(s: &str) -> String {
    // & must be escaped first to avoid double-escaping &lt; etc.
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Wrap a string in DOT double quotes with proper escaping.
fn dot_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

// ============================================================================
// Conversion helpers (from library Selection → GraphSelection)
// ============================================================================

/// Convert a library [`crate::Selection`] into a [`GraphSelection`].
impl From<&crate::structured::Selection> for GraphSelection {
    fn from(sel: &crate::structured::Selection) -> Self {
        let condition_id = sel.condition_path().clone();
        let som_path = sel
            .group_som_path
            .clone()
            .unwrap_or_else(|| sel.som_path.clone());
        let kind = match sel.kind {
            crate::structured::SelectionKind::Radio => GraphSelectionKind::Radio,
            crate::structured::SelectionKind::Checkbox => GraphSelectionKind::Checkbox,
            crate::structured::SelectionKind::Dropdown => GraphSelectionKind::Dropdown,
        };
        GraphSelection {
            condition_id,
            som_path,
            values: sel.values.clone(),
            kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::{
        FieldId, FieldNode, FieldType, GroupNode, InputValue, NameValue, StructuredNode,
        TranslatableString, TranslatedText,
    };
    use crate::xfa::scripting::SomPath;

    /// Helper: build a radio field node with given SOM path, label, and options.
    fn radio_field(som: &str, label: &str, options: &[(&str, &str)]) -> StructuredNode {
        let opts = options
            .iter()
            .map(|(name, value)| NameValue {
                name: TranslatableString::Plain(name.to_string()),
                value: InputValue::Text(value.to_string()),
            })
            .collect();
        StructuredNode::Field(FieldNode {
            name: FieldId::from_som_path(&SomPath::new(som)),
            som_path: Some(SomPath::new(som)),
            label: Some(TranslatedText::plain(label)),
            input_type: FieldType::Radio { options: opts },
            value: None,
            placeholder: None,
            required: false,
        })
    }

    /// Helper: build a checkbox field node.
    fn checkbox_field(som: &str, label: &str) -> StructuredNode {
        StructuredNode::Field(FieldNode {
            name: FieldId::from_som_path(&SomPath::new(som)),
            som_path: Some(SomPath::new(som)),
            label: Some(TranslatedText::plain(label)),
            input_type: FieldType::Bool,
            value: None,
            placeholder: None,
            required: false,
        })
    }

    #[test]
    fn build_field_label_map_extracts_radio_labels() {
        let nodes = vec![radio_field(
            "Form.Page.RadioGroup",
            "Choose option",
            &[("New", "RB_1"), ("Change", "RB_2"), ("Delete", "RB_3")],
        )];

        let map = build_field_label_map(&nodes);
        let fid = FieldId::from_som_path(&SomPath::new("Form.Page.RadioGroup"));

        assert_eq!(map.labels.get(&fid).unwrap(), "Choose option");
        let opts = map.option_names.get(&fid).unwrap();
        assert_eq!(opts.get("RB_1").unwrap(), "New");
        assert_eq!(opts.get("RB_2").unwrap(), "Change");
        assert_eq!(opts.get("RB_3").unwrap(), "Delete");
    }

    #[test]
    fn build_field_label_map_extracts_from_groups() {
        let nodes = vec![StructuredNode::Group(GroupNode {
            children: vec![
                radio_field("Form.Radio", "Top Radio", &[("A", "v1"), ("B", "v2")]),
                checkbox_field("Form.Check", "My Checkbox"),
            ],
        })];

        let map = build_field_label_map(&nodes);
        assert_eq!(
            map.labels
                .get(&FieldId::from_som_path(&SomPath::new("Form.Radio")))
                .unwrap(),
            "Top Radio"
        );
        assert_eq!(
            map.labels
                .get(&FieldId::from_som_path(&SomPath::new("Form.Check")))
                .unwrap(),
            "My Checkbox"
        );
    }

    #[test]
    fn generate_dot_produces_valid_structure() {
        let states = vec![
            GraphState {
                selections: vec![GraphSelection {
                    condition_id: FieldId::from_som_path(&SomPath::new("Form.Radio")),
                    som_path: SomPath::new("Form.Radio"),
                    values: vec!["RB_1".to_string()],
                    kind: GraphSelectionKind::Radio,
                }],
                label: "RB_1".to_string(),
            },
            GraphState {
                selections: vec![GraphSelection {
                    condition_id: FieldId::from_som_path(&SomPath::new("Form.Radio")),
                    som_path: SomPath::new("Form.Radio"),
                    values: vec!["RB_2".to_string()],
                    kind: GraphSelectionKind::Radio,
                }],
                label: "RB_2".to_string(),
            },
        ];

        let mut field_labels = FieldLabelMap::default();
        field_labels.labels.insert(
            FieldId::from_som_path(&SomPath::new("Form.Radio")),
            "Action Type".to_string(),
        );
        let mut opts = HashMap::new();
        opts.insert("RB_1".to_string(), "New".to_string());
        opts.insert("RB_2".to_string(), "Change".to_string());
        field_labels
            .option_names
            .insert(FieldId::from_som_path(&SomPath::new("Form.Radio")), opts);

        let dot = generate_dot(&states, &field_labels);

        assert!(dot.contains("digraph FormFlow"));
        assert!(dot.contains("shape=diamond"));
        assert!(dot.contains("Action Type"));
        assert!(dot.contains("New"));
        assert!(dot.contains("Change"));
        assert!(dot.contains("RB_1"));
        assert!(dot.contains("RB_2"));
        assert!(dot.ends_with("}\n"));
    }

    #[test]
    fn generate_dot_nested_decisions() {
        // Simulate: Radio(RB_1, RB_2) where RB_2 has a nested Checkbox(checked, unchecked).
        let states = vec![
            GraphState {
                selections: vec![GraphSelection {
                    condition_id: FieldId::from_som_path(&SomPath::new("Form.Radio")),
                    som_path: SomPath::new("Form.Radio"),
                    values: vec!["RB_1".to_string()],
                    kind: GraphSelectionKind::Radio,
                }],
                label: "RB_1".to_string(),
            },
            GraphState {
                selections: vec![
                    GraphSelection {
                        condition_id: FieldId::from_som_path(&SomPath::new("Form.Radio")),
                        som_path: SomPath::new("Form.Radio"),
                        values: vec!["RB_2".to_string()],
                        kind: GraphSelectionKind::Radio,
                    },
                    GraphSelection {
                        condition_id: FieldId::from_som_path(&SomPath::new("Form.Check")),
                        som_path: SomPath::new("Form.Check"),
                        values: vec!["checked".to_string()],
                        kind: GraphSelectionKind::Checkbox,
                    },
                ],
                label: "RB_2_checked".to_string(),
            },
            GraphState {
                selections: vec![
                    GraphSelection {
                        condition_id: FieldId::from_som_path(&SomPath::new("Form.Radio")),
                        som_path: SomPath::new("Form.Radio"),
                        values: vec!["RB_2".to_string()],
                        kind: GraphSelectionKind::Radio,
                    },
                    GraphSelection {
                        condition_id: FieldId::from_som_path(&SomPath::new("Form.Check")),
                        som_path: SomPath::new("Form.Check"),
                        values: vec!["unchecked".to_string()],
                        kind: GraphSelectionKind::Checkbox,
                    },
                ],
                label: "RB_2_unchecked".to_string(),
            },
        ];

        let field_labels = FieldLabelMap::default();
        let dot = generate_dot(&states, &field_labels);

        // Should have two diamond decision nodes.
        let diamond_count = dot.matches("shape=diamond").count();
        assert_eq!(
            diamond_count, 2,
            "expected 2 decision diamonds, got {}",
            diamond_count
        );

        // Should have three leaf nodes.
        assert!(dot.contains("RB_1"));
        assert!(dot.contains("RB_2_checked"));
        assert!(dot.contains("RB_2_unchecked"));

        // Checkbox edges should show check/uncheck symbols.
        assert!(dot.contains("checked"));
        assert!(dot.contains("unchecked"));
    }

    #[test]
    fn generate_dot_single_state_no_decisions() {
        let states = vec![GraphState {
            selections: vec![],
            label: "default".to_string(),
        }];
        let field_labels = FieldLabelMap::default();
        let dot = generate_dot(&states, &field_labels);

        assert!(dot.contains("digraph FormFlow"));
        assert!(dot.contains("default"));
        // No decision nodes.
        assert!(!dot.contains("shape=diamond"));
    }

    #[test]
    fn dot_escape_handles_special_chars() {
        assert_eq!(dot_escape("a\"b"), "a&quot;b");
        assert_eq!(dot_escape("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(dot_escape("a&b"), "a&amp;b");
        assert_eq!(dot_escape("a&<b>"), "a&amp;&lt;b&gt;");
    }

    #[test]
    fn graph_selection_from_library_selection() {
        let sel = crate::structured::Selection::new(
            SomPath::new("Form.Page.Field"),
            Some(SomPath::new("Form.Page.Group")),
            "RB_1".to_string(),
            crate::structured::SelectionKind::Radio,
        );
        let gs: GraphSelection = (&sel).into();
        assert_eq!(
            gs.condition_id,
            FieldId::from_som_path(&SomPath::new("Form.Page.Group"))
        );
        assert_eq!(gs.som_path, SomPath::new("Form.Page.Group"));
        assert_eq!(gs.values, vec!["RB_1".to_string()]);
        assert_eq!(gs.kind, GraphSelectionKind::Radio);
    }

    #[test]
    fn generate_dot_dedup_multiple_values_same_state() {
        // RB_1 and RB_2 produce identical output → same GraphState with values: ["RB_1", "RB_2"].
        // RB_3 is different.
        let states = vec![
            GraphState {
                selections: vec![GraphSelection {
                    condition_id: FieldId::from_som_path(&SomPath::new("Form.Radio")),
                    som_path: SomPath::new("Form.Radio"),
                    values: vec!["RB_1".to_string(), "RB_2".to_string()],
                    kind: GraphSelectionKind::Radio,
                }],
                label: "RB_1".to_string(),
            },
            GraphState {
                selections: vec![GraphSelection {
                    condition_id: FieldId::from_som_path(&SomPath::new("Form.Radio")),
                    som_path: SomPath::new("Form.Radio"),
                    values: vec!["RB_3".to_string()],
                    kind: GraphSelectionKind::Radio,
                }],
                label: "RB_3".to_string(),
            },
        ];

        let mut field_labels = FieldLabelMap::default();
        let mut opts = HashMap::new();
        opts.insert("RB_1".to_string(), "Alpha".to_string());
        opts.insert("RB_2".to_string(), "Beta".to_string());
        opts.insert("RB_3".to_string(), "Gamma".to_string());
        field_labels
            .option_names
            .insert(FieldId::from_som_path(&SomPath::new("Form.Radio")), opts);

        let dot = generate_dot(&states, &field_labels);

        // One decision diamond.
        assert_eq!(dot.matches("shape=diamond").count(), 1);

        // Two leaf nodes (one shared for RB_1+RB_2, one for RB_3).
        assert!(dot.contains("RB_1"));
        assert!(dot.contains("RB_3"));

        // Three edges from the diamond (Alpha, Beta, Gamma).
        assert!(dot.contains("Alpha"));
        assert!(dot.contains("Beta"));
        assert!(dot.contains("Gamma"));

        // Alpha and Beta point to the same node.
        // Count arrows: should be 3 total from the diamond.
        let arrow_count = dot.matches(" -> ").count();
        assert_eq!(arrow_count, 3, "expected 3 edges, got {}", arrow_count);
    }
}
