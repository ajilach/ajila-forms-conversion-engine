//! Editor state management.
//!
//! This module provides the state types and operations for the structured
//! document editor.

use blueprint::structured::{FieldCondition, GridLayoutElement, NameValue};
use blueprint::{FieldId, ListItem, ListNode, StructuredNode, TableNode, TranslatedText};
use std::collections::{BTreeSet, HashMap, HashSet};

/// A segment of a path to a node in the document tree.
///
/// This enables addressing not just tree children, but also pseudo-nodes
/// like list items and table rows/cells.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PathSegment {
    /// Index into children array (Group, GridLayout, Repeatable, Conditional, or root content).
    Child(usize),
    /// Index into ListNode.items.
    ListItem(usize),
    /// Index into TableNode.rows.
    TableRow(usize),
    /// Addresses the optional TableNode.header.
    TableHeader,
    /// Index into row's cells (follows TableRow or TableHeader).
    TableCell(usize),
}

impl PathSegment {
    /// Get the index if this is a Child segment.
    pub fn as_child_index(&self) -> Option<usize> {
        match self {
            PathSegment::Child(idx) => Some(*idx),
            _ => None,
        }
    }
}

/// A path to a node in the document tree.
///
/// Each element represents navigation into the tree structure.
/// Examples:
/// - `[Child(0), Child(2)]` - root[0]'s third child (if Group)
/// - `[Child(1), ListItem(3)]` - fourth item in second list
/// - `[Child(0), TableRow(2), TableCell(1)]` - second cell in third row of first table
pub type NodePath = Vec<PathSegment>;

/// The current selection state in the editor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectionState {
    /// Set of selected node paths.
    pub selected: HashSet<NodePath>,
    /// The node currently being edited (text editing mode).
    pub editing: Option<NodePath>,
    /// The node whose metadata is being edited.
    pub editing_metadata: Option<NodePath>,
}

impl SelectionState {
    /// Create a new empty selection state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle selection of a node.
    pub fn toggle(&mut self, path: NodePath) {
        if self.selected.contains(&path) {
            self.selected.remove(&path);
        } else {
            self.selected.insert(path);
        }
    }

    /// Select a single node, clearing previous selection.
    pub fn select_single(&mut self, path: NodePath) {
        self.selected.clear();
        self.selected.insert(path);
    }

    /// Clear all selections.
    pub fn clear(&mut self) {
        self.selected.clear();
        self.editing = None;
        self.editing_metadata = None;
    }

    /// Check if a node is selected.
    pub fn is_selected(&self, path: &NodePath) -> bool {
        self.selected.contains(path)
    }

    /// Get the number of selected nodes.
    pub fn count(&self) -> usize {
        self.selected.len()
    }

    /// Start editing a node's text.
    pub fn start_editing(&mut self, path: NodePath) {
        self.editing = Some(path);
    }

    /// Stop editing.
    pub fn stop_editing(&mut self) {
        self.editing = None;
        self.editing_metadata = None;
    }

    /// Start editing a node's metadata.
    pub fn start_editing_metadata(&mut self, path: NodePath) {
        self.editing = None;
        self.editing_metadata = Some(path);
    }

    /// Check if we're editing a specific node's metadata.
    pub fn is_editing_metadata(&self, path: &NodePath) -> bool {
        self.editing_metadata.as_ref() == Some(path)
    }

    /// Check if we're currently editing a specific node.
    pub fn is_editing(&self, path: &NodePath) -> bool {
        self.editing.as_ref() == Some(path)
    }
}

/// Operations that can be performed on the document.
#[derive(Clone, Debug)]
pub enum EditorAction {
    /// Select or deselect a node.
    ToggleSelection(NodePath),
    /// Select a single node.
    SelectSingle(NodePath),
    /// Clear all selections.
    ClearSelection,
    /// Delete selected nodes.
    DeleteSelected,
    /// Merge selected nodes.
    MergeSelected,
    /// Move selected node up.
    MoveUp,
    /// Move selected node down.
    MoveDown,
    /// Move selected node into the adjacent container above (indent).
    Indent,
    /// Move selected node out of its container to the parent level (outdent).
    Outdent,
    /// Start editing a node's text.
    StartEditing(NodePath),
    /// Update text content of a node (works for paragraphs, headings, field labels, list items).
    UpdateText {
        path: NodePath,
        content: String,
        language: Option<String>,
    },
    /// Stop editing.
    StopEditing,
    /// Start editing a node's metadata.
    StartEditingMetadata(NodePath),
    /// Update node metadata.
    UpdateMetadata {
        path: NodePath,
        metadata: NodeMetadata,
    },
    /// Add a new node.
    AddNode {
        parent: NodePath,
        index: usize,
        node_type: NewNodeType,
    },
    /// Duplicate selected nodes (insert copies below).
    DuplicateSelected,
    /// Convert selected node(s) to a different type.
    ConvertSelected(ConvertTarget),
    /// Open the smart edit modal (AI-assisted editing via gh copilot).
    SmartEdit,
    /// Select all root-level nodes.
    SelectAll,
}

/// Produce a human-readable label for a content-mutating action, for the edit
/// history. Returns `None` for actions that do not change document content
/// (selection changes, edit-mode toggles, and the async Smart Edit trigger).
pub fn describe_action(action: &EditorAction) -> Option<String> {
    let label = match action {
        EditorAction::DeleteSelected => "Deleted node(s)",
        EditorAction::MergeSelected => "Merged nodes",
        EditorAction::MoveUp => "Moved up",
        EditorAction::MoveDown => "Moved down",
        EditorAction::Indent => "Indented",
        EditorAction::Outdent => "Outdented",
        EditorAction::DuplicateSelected => "Duplicated node(s)",
        EditorAction::UpdateText { .. } => "Edited text",
        EditorAction::UpdateMetadata { .. } => "Updated properties",
        EditorAction::AddNode { .. } => "Added node",
        EditorAction::ConvertSelected(_) => "Converted node(s)",
        // Non-mutating or deferred actions.
        EditorAction::ToggleSelection(_)
        | EditorAction::SelectSingle(_)
        | EditorAction::ClearSelection
        | EditorAction::StartEditing(_)
        | EditorAction::StopEditing
        | EditorAction::StartEditingMetadata(_)
        | EditorAction::SelectAll
        | EditorAction::SmartEdit => return None,
    };
    Some(label.to_string())
}

/// Target type for conversion operations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConvertTarget {
    /// Convert to paragraph (single element).
    Paragraph,
    /// Explode to multiple paragraphs (e.g., list -> paragraphs).
    Paragraphs,
    /// Convert to heading (with level).
    Heading(u8),
    /// Convert multiple items to list.
    List,
    /// Convert to field (text becomes label).
    Field,
}

/// Editable metadata for a node.
#[derive(Clone, Debug)]
pub enum NodeMetadata {
    /// Heading level (1-6).
    HeadingLevel(u8),
    /// Repeatable min/max occurrences.
    Repeatable { min: u32, max: Option<u32> },
    /// Grid layout columns.
    GridColumns(usize),
    /// Grid element span.
    #[allow(dead_code)]
    GridElementSpan(usize),
    /// Field input type.
    FieldInputType(FieldInputKind),
    /// Field options (for Radio/Dropdown fields).
    FieldOptions(Vec<NameValue>),
    /// Field required flag.
    FieldRequired(bool),
}

/// Simplified field input type for the editor UI.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldInputKind {
    Text,
    Textarea,
    Number,
    Date,
    Email,
    Tel,
    Checkbox,
    Dropdown,
    Radio,
    CheckboxGroup,
}

/// Types of nodes that can be added.
#[derive(Clone, Debug, PartialEq)]
pub enum NewNodeType {
    Paragraph,
    Heading(u8),
    List,
    Group,
    Repeatable,
    Field,
    ListItem,
    TableRow,
    TableCell,
}

/// Get a node at a given path.
///
/// Handles regular child navigation, as well as paths through table rows/cells.
/// ListItem paths are not resolved by this function (use `get_list_at_path` for list traversal).
pub fn get_node_at_path<'a>(
    content: &'a [StructuredNode],
    path: &NodePath,
) -> Option<&'a StructuredNode> {
    if path.is_empty() {
        return None;
    }

    let first_segment = &path[0];
    let PathSegment::Child(first_idx) = first_segment else {
        return None; // Path must start with Child
    };

    let mut current = content.get(*first_idx)?;
    let mut i = 1;

    while i < path.len() {
        let segment = &path[i];
        current = match (current, segment) {
            // Child navigation
            (StructuredNode::Group(g), PathSegment::Child(idx)) => g.children.get(*idx)?,
            (StructuredNode::GridLayout(g), PathSegment::Child(idx)) => {
                g.elements.get(*idx).map(|e| &e.node)?
            }
            (StructuredNode::Repeatable(r), PathSegment::Child(0)) => r.item.as_ref(),
            (StructuredNode::Conditional(c), PathSegment::Child(0)) => c.content.as_ref(),

            // Table cell navigation: TableRow/TableHeader followed by TableCell
            (StructuredNode::Table(t), PathSegment::TableRow(row_idx)) => {
                // Next segment must be TableCell
                if i + 1 >= path.len() {
                    return None; // TableRow is not a StructuredNode
                }
                let PathSegment::TableCell(cell_idx) = &path[i + 1] else {
                    return None;
                };
                i += 1; // Skip the TableCell segment in the next iteration
                t.rows.get(*row_idx)?.cells.get(*cell_idx)?
            }
            (StructuredNode::Table(t), PathSegment::TableHeader) => {
                // Next segment must be TableCell
                if i + 1 >= path.len() {
                    return None; // TableHeader is not a StructuredNode
                }
                let PathSegment::TableCell(cell_idx) = &path[i + 1] else {
                    return None;
                };
                i += 1; // Skip the TableCell segment in the next iteration
                t.header.as_ref()?.cells.get(*cell_idx)?
            }

            // ListItem is InlineText, not StructuredNode
            (StructuredNode::List(_), PathSegment::ListItem(_)) => return None,

            _ => return None,
        };
        i += 1;
    }

    Some(current)
}

/// Get a table cell at a given path.
///
/// For paths ending with TableCell, this returns the cell's StructuredNode.
#[allow(dead_code)]
pub fn get_table_cell_at_path<'a>(
    content: &'a [StructuredNode],
    path: &NodePath,
) -> Option<&'a StructuredNode> {
    if path.len() < 3 {
        return None;
    }

    // Navigate to the table first
    let table_path: NodePath = path
        .iter()
        .take_while(|s| matches!(s, PathSegment::Child(_)))
        .cloned()
        .collect();
    let table = get_node_at_path(content, &table_path)?;
    let StructuredNode::Table(t) = table else {
        return None;
    };

    // Get the row/header and cell segments
    let remaining: Vec<_> = path.iter().skip(table_path.len()).collect();
    if remaining.len() != 2 {
        return None;
    }

    match (remaining[0], remaining[1]) {
        (PathSegment::TableRow(row_idx), PathSegment::TableCell(cell_idx)) => {
            t.rows.get(*row_idx)?.cells.get(*cell_idx)
        }
        (PathSegment::TableHeader, PathSegment::TableCell(cell_idx)) => {
            t.header.as_ref()?.cells.get(*cell_idx)
        }
        _ => None,
    }
}

/// Get a mutable reference to a node at a given path.
///
/// Handles regular child navigation, as well as paths through table rows/cells.
/// ListItem paths are not resolved by this function (use `get_list_at_path_mut` for list traversal).
pub fn get_node_at_path_mut<'a>(
    content: &'a mut [StructuredNode],
    path: &NodePath,
) -> Option<&'a mut StructuredNode> {
    if path.is_empty() {
        return None;
    }

    let first_segment = &path[0];
    let PathSegment::Child(first_idx) = first_segment else {
        return None;
    };

    if *first_idx >= content.len() {
        return None;
    }

    // Check if this path goes through a table cell
    // Look for TableRow/TableHeader followed by TableCell pattern
    let table_cell_idx = path
        .iter()
        .position(|s| matches!(s, PathSegment::TableRow(_) | PathSegment::TableHeader));

    if let Some(table_segment_idx) = table_cell_idx {
        // Path goes through a table - split into: path_to_table, row/header, cell, rest
        if table_segment_idx + 1 >= path.len() {
            return None; // No cell segment after row/header
        }
        let PathSegment::TableCell(cell_idx) = &path[table_segment_idx + 1] else {
            return None;
        };

        // Navigate to the table
        let mut current = &mut content[*first_idx];
        for segment in &path[1..table_segment_idx] {
            current = match (current, segment) {
                (StructuredNode::Group(g), PathSegment::Child(idx)) => g.children.get_mut(*idx)?,
                (StructuredNode::GridLayout(g), PathSegment::Child(idx)) => {
                    g.elements.get_mut(*idx).map(|e| &mut e.node)?
                }
                _ => return None,
            };
        }

        // Get the cell from the table
        let StructuredNode::Table(t) = current else {
            return None;
        };

        let cell = match &path[table_segment_idx] {
            PathSegment::TableRow(row_idx) => t.rows.get_mut(*row_idx)?.cells.get_mut(*cell_idx)?,
            PathSegment::TableHeader => t.header.as_mut()?.cells.get_mut(*cell_idx)?,
            _ => return None,
        };

        // If there are more segments after the cell, continue navigation
        let remaining = &path[table_segment_idx + 2..];
        if remaining.is_empty() {
            return Some(cell);
        }

        // Continue navigation from the cell
        let mut current = cell;
        for segment in remaining {
            current = match (current, segment) {
                (StructuredNode::Group(g), PathSegment::Child(idx)) => g.children.get_mut(*idx)?,
                (StructuredNode::GridLayout(g), PathSegment::Child(idx)) => {
                    g.elements.get_mut(*idx).map(|e| &mut e.node)?
                }
                (StructuredNode::Repeatable(r), PathSegment::Child(0)) => r.item.as_mut(),
                (StructuredNode::Conditional(c), PathSegment::Child(0)) => c.content.as_mut(),
                (StructuredNode::List(_), PathSegment::ListItem(_)) => return None,
                _ => return None,
            };
        }
        Some(current)
    } else {
        // Regular path without table traversal
        let mut current = &mut content[*first_idx];

        for segment in &path[1..] {
            current = match (current, segment) {
                (StructuredNode::Group(g), PathSegment::Child(idx)) => g.children.get_mut(*idx)?,
                (StructuredNode::GridLayout(g), PathSegment::Child(idx)) => {
                    g.elements.get_mut(*idx).map(|e| &mut e.node)?
                }
                (StructuredNode::Repeatable(r), PathSegment::Child(0)) => r.item.as_mut(),
                (StructuredNode::Conditional(c), PathSegment::Child(0)) => c.content.as_mut(),
                (StructuredNode::List(_), PathSegment::ListItem(_)) => return None,
                _ => return None,
            };
        }

        Some(current)
    }
}

/// Get a list at a given path.
///
/// The path must start at a root/node child that resolves to a `StructuredNode::List`.
/// Additional `ListItem(i)` segments descend into `items[i].sublist` recursively.
pub fn get_list_at_path<'a>(
    content: &'a [StructuredNode],
    path: &NodePath,
) -> Option<&'a ListNode> {
    if path.is_empty() {
        return None;
    }

    let first_list_item_idx = path
        .iter()
        .position(|segment| matches!(segment, PathSegment::ListItem(_)));

    let (list_node_path, list_path_suffix) = if let Some(i) = first_list_item_idx {
        (&path[..i], &path[i..])
    } else {
        (&path[..], &path[path.len()..])
    };

    let current = get_node_at_path(content, &list_node_path.to_vec())?;

    let mut list = match current {
        StructuredNode::List(list) => list,
        _ => return None,
    };

    for segment in list_path_suffix {
        let PathSegment::ListItem(item_idx) = segment else {
            return None;
        };
        let item = list.items.get(*item_idx)?;
        list = item.sublist.as_deref()?;
    }

    Some(list)
}

/// Get a mutable list at a given path.
///
/// The path must start at a root/node child that resolves to a `StructuredNode::List`.
/// Additional `ListItem(i)` segments descend into `items[i].sublist` recursively.
pub fn get_list_at_path_mut<'a>(
    content: &'a mut [StructuredNode],
    path: &NodePath,
) -> Option<&'a mut ListNode> {
    if path.is_empty() {
        return None;
    }

    let first_list_item_idx = path
        .iter()
        .position(|segment| matches!(segment, PathSegment::ListItem(_)));

    let (list_node_path, list_path_suffix) = if let Some(i) = first_list_item_idx {
        (&path[..i], &path[i..])
    } else {
        (&path[..], &path[path.len()..])
    };

    let node = get_node_at_path_mut(content, &list_node_path.to_vec())?;
    let mut list = match node {
        StructuredNode::List(list) => list,
        _ => return None,
    };

    for segment in list_path_suffix {
        let PathSegment::ListItem(item_idx) = segment else {
            return None;
        };
        let item = list.items.get_mut(*item_idx)?;
        list = item.sublist.as_deref_mut()?;
    }

    Some(list)
}

/// Get a mutable reference to a table cell at a given path.
#[allow(dead_code)]
pub fn get_table_cell_at_path_mut<'a>(
    content: &'a mut [StructuredNode],
    path: &NodePath,
) -> Option<&'a mut StructuredNode> {
    if path.len() < 3 {
        return None;
    }

    // Navigate to the table first
    let table_path: NodePath = path
        .iter()
        .take_while(|s| matches!(s, PathSegment::Child(_)))
        .cloned()
        .collect();

    // Must have at least one Child segment to navigate to the table
    if table_path.is_empty() {
        return None;
    }

    let PathSegment::Child(first_idx) = &table_path[0] else {
        return None;
    };

    if *first_idx >= content.len() {
        return None;
    }

    // Navigate to table
    let mut current = &mut content[*first_idx];
    for segment in &table_path[1..] {
        let PathSegment::Child(idx) = segment else {
            return None;
        };
        current = match current {
            StructuredNode::Group(g) => g.children.get_mut(*idx)?,
            StructuredNode::GridLayout(g) => g.elements.get_mut(*idx).map(|e| &mut e.node)?,
            _ => return None,
        };
    }

    let StructuredNode::Table(t) = current else {
        return None;
    };

    // Get the cell
    let remaining: Vec<_> = path.iter().skip(table_path.len()).cloned().collect();
    if remaining.len() != 2 {
        return None;
    }

    match (&remaining[0], &remaining[1]) {
        (PathSegment::TableRow(row_idx), PathSegment::TableCell(cell_idx)) => {
            t.rows.get_mut(*row_idx)?.cells.get_mut(*cell_idx)
        }
        (PathSegment::TableHeader, PathSegment::TableCell(cell_idx)) => {
            t.header.as_mut()?.cells.get_mut(*cell_idx)
        }
        _ => None,
    }
}

/// Delete nodes at the given paths from the document.
///
/// Paths are processed in reverse order (deepest first, highest index first)
/// to avoid invalidating indices. Handles both regular nodes and pseudo-nodes
/// (list items, table rows/header).
pub fn delete_nodes(content: &mut Vec<StructuredNode>, paths: &HashSet<NodePath>) {
    // Sort paths: deepest first, then by last segment's index descending
    let mut sorted_paths: Vec<_> = paths.iter().cloned().collect();
    sorted_paths.sort_by(|a, b| {
        // First by depth (longer paths first)
        let depth_cmp = b.len().cmp(&a.len());
        if depth_cmp != std::cmp::Ordering::Equal {
            return depth_cmp;
        }
        // Then by last segment index descending
        let a_idx = segment_index(a.last());
        let b_idx = segment_index(b.last());
        b_idx.cmp(&a_idx)
    });

    for path in sorted_paths {
        if path.is_empty() {
            continue;
        }

        let last_segment = path.last().unwrap();

        if path.len() == 1 {
            // Root level deletion
            if let PathSegment::Child(idx) = last_segment
                && *idx < content.len()
            {
                content.remove(*idx);
            }
        } else {
            // Get parent path (all but last segment)
            let parent_path: NodePath = path[..path.len() - 1].to_vec();

            // Handle different last segment types
            match last_segment {
                PathSegment::Child(child_idx) => {
                    // Deletion of a child node
                    if let Some(parent) = get_node_at_path_mut(content, &parent_path) {
                        match parent {
                            StructuredNode::Group(g) if *child_idx < g.children.len() => {
                                g.children.remove(*child_idx);
                            }
                            StructuredNode::GridLayout(g) if *child_idx < g.elements.len() => {
                                g.elements.remove(*child_idx);
                            }
                            StructuredNode::Repeatable(r) if *child_idx == 0 => {
                                *r.item = StructuredNode::Empty;
                            }
                            StructuredNode::Conditional(c) if *child_idx == 0 => {
                                *c.content = StructuredNode::Empty;
                            }
                            _ => {}
                        }
                    }
                }
                PathSegment::ListItem(item_idx) => {
                    // Deletion of a list item
                    if let Some(list) = get_list_at_path_mut(content, &parent_path)
                        && *item_idx < list.items.len()
                    {
                        list.items.remove(*item_idx);
                    }
                }
                PathSegment::TableRow(row_idx) => {
                    // Deletion of a table row
                    if let Some(StructuredNode::Table(t)) =
                        get_node_at_path_mut(content, &parent_path)
                        && *row_idx < t.rows.len()
                    {
                        t.rows.remove(*row_idx);
                    }
                }
                PathSegment::TableHeader => {
                    // Deletion of table header
                    if let Some(StructuredNode::Table(t)) =
                        get_node_at_path_mut(content, &parent_path)
                    {
                        t.header = None;
                    }
                }
                PathSegment::TableCell(_) => {
                    // We don't support deleting individual cells (would break table structure)
                    // Could potentially clear cell content instead
                }
            }
        }
    }
}

/// Helper to get an index from a PathSegment for sorting purposes.
fn segment_index(segment: Option<&PathSegment>) -> usize {
    match segment {
        Some(PathSegment::Child(idx)) => *idx,
        Some(PathSegment::ListItem(idx)) => *idx,
        Some(PathSegment::TableRow(idx)) => *idx,
        Some(PathSegment::TableCell(idx)) => *idx,
        Some(PathSegment::TableHeader) => 0,
        None => 0,
    }
}

/// Assign fresh [`FieldId`]s to every field within a (duplicated) subtree so the
/// copy does not collide with the original. Conditional references to fields that
/// live inside the same subtree are remapped to the new ids, so internal
/// dependencies stay intact.
fn reassign_field_ids(node: &mut StructuredNode) {
    let mut remap: HashMap<FieldId, FieldId> = HashMap::new();
    replace_field_ids(node, &mut remap);
    if !remap.is_empty() {
        remap_field_conditions(node, &remap);
    }
}

/// First pass: give every [`FieldNode`] a new random id, recording old -> new.
fn replace_field_ids(node: &mut StructuredNode, remap: &mut HashMap<FieldId, FieldId>) {
    match node {
        StructuredNode::Field(f) => {
            let new_id = FieldId::random();
            remap.insert(f.name.clone(), new_id.clone());
            f.name = new_id;
        }
        StructuredNode::Group(g) => {
            for child in &mut g.children {
                replace_field_ids(child, remap);
            }
        }
        StructuredNode::GridLayout(g) => {
            for el in &mut g.elements {
                replace_field_ids(&mut el.node, remap);
            }
        }
        StructuredNode::Repeatable(r) => replace_field_ids(&mut r.item, remap),
        StructuredNode::Conditional(c) => replace_field_ids(&mut c.content, remap),
        StructuredNode::Table(t) => {
            if let Some(header) = &mut t.header {
                for cell in &mut header.cells {
                    replace_field_ids(cell, remap);
                }
            }
            for row in &mut t.rows {
                for cell in &mut row.cells {
                    replace_field_ids(cell, remap);
                }
            }
        }
        _ => {}
    }
}

/// Second pass: rewrite conditional references to fields that were re-keyed.
fn remap_field_conditions(node: &mut StructuredNode, remap: &HashMap<FieldId, FieldId>) {
    match node {
        StructuredNode::Conditional(c) => {
            if let Some(new_id) = remap.get(&c.condition.field_name) {
                c.condition = FieldCondition {
                    field_name: new_id.clone(),
                    value: c.condition.value.clone(),
                };
            }
            remap_field_conditions(&mut c.content, remap);
        }
        StructuredNode::Group(g) => {
            for child in &mut g.children {
                remap_field_conditions(child, remap);
            }
        }
        StructuredNode::GridLayout(g) => {
            for el in &mut g.elements {
                remap_field_conditions(&mut el.node, remap);
            }
        }
        StructuredNode::Repeatable(r) => remap_field_conditions(&mut r.item, remap),
        StructuredNode::Table(t) => {
            if let Some(header) = &mut t.header {
                for cell in &mut header.cells {
                    remap_field_conditions(cell, remap);
                }
            }
            for row in &mut t.rows {
                for cell in &mut row.cells {
                    remap_field_conditions(cell, remap);
                }
            }
        }
        _ => {}
    }
}

/// Duplicate selected nodes, inserting all copies after the last selected element.
///
/// Groups sibling paths by their parent container, clones them in order, and inserts
/// the entire batch after the highest-indexed selected sibling within that container.
pub fn duplicate_nodes(content: &mut Vec<StructuredNode>, paths: &HashSet<NodePath>) {
    // Group paths by (parent_path, segment_type) so siblings are handled together.
    // Within each group, clones are inserted as a batch after the last selected element.
    use std::collections::BTreeMap;

    // Categorize paths by parent
    #[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
    enum SegmentKind {
        Child,
        ListItem,
        TableRow,
    }

    // Group: (parent_path, kind) -> sorted list of indices
    let mut groups: BTreeMap<(NodePath, SegmentKind), Vec<usize>> = BTreeMap::new();

    for path in paths {
        if path.is_empty() {
            continue;
        }
        let last_segment = path.last().unwrap();
        let parent_path: NodePath = path[..path.len() - 1].to_vec();

        let (kind, idx) = match last_segment {
            PathSegment::Child(idx) => (SegmentKind::Child, *idx),
            PathSegment::ListItem(idx) => (SegmentKind::ListItem, *idx),
            PathSegment::TableRow(idx) => (SegmentKind::TableRow, *idx),
            PathSegment::TableHeader | PathSegment::TableCell(_) => continue,
        };

        groups.entry((parent_path, kind)).or_default().push(idx);
    }

    // Process groups from deepest/highest-index first so insertions don't invalidate
    // other groups' indices.
    let mut group_list: Vec<_> = groups.into_iter().collect();
    group_list.sort_by(|a, b| {
        let depth_cmp = b.0.0.len().cmp(&a.0.0.len());
        if depth_cmp != std::cmp::Ordering::Equal {
            return depth_cmp;
        }
        // For same depth, process higher parent indices first
        b.0.0.cmp(&a.0.0)
    });

    for ((parent_path, kind), mut indices) in group_list {
        indices.sort();
        let max_idx = *indices.last().unwrap();

        if parent_path.is_empty() && kind == SegmentKind::Child {
            // Root level duplication
            let mut clones: Vec<_> = indices
                .iter()
                .filter_map(|&idx| content.get(idx).cloned())
                .collect();
            for clone in &mut clones {
                reassign_field_ids(clone);
            }
            // Insert all clones after the last selected element
            let insert_pos = (max_idx + 1).min(content.len());
            for (i, clone) in clones.into_iter().enumerate() {
                content.insert(insert_pos + i, clone);
            }
        } else {
            match kind {
                SegmentKind::Child => {
                    if let Some(parent) = get_node_at_path_mut(content, &parent_path) {
                        match parent {
                            StructuredNode::Group(g) => {
                                let mut clones: Vec<_> = indices
                                    .iter()
                                    .filter_map(|&idx| g.children.get(idx).cloned())
                                    .collect();
                                for clone in &mut clones {
                                    reassign_field_ids(clone);
                                }
                                let insert_pos = (max_idx + 1).min(g.children.len());
                                for (i, clone) in clones.into_iter().enumerate() {
                                    g.children.insert(insert_pos + i, clone);
                                }
                            }
                            StructuredNode::GridLayout(g) => {
                                let mut clones: Vec<_> = indices
                                    .iter()
                                    .filter_map(|&idx| g.elements.get(idx).cloned())
                                    .collect();
                                for clone in &mut clones {
                                    reassign_field_ids(&mut clone.node);
                                }
                                let insert_pos = (max_idx + 1).min(g.elements.len());
                                for (i, clone) in clones.into_iter().enumerate() {
                                    g.elements.insert(insert_pos + i, clone);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                SegmentKind::ListItem => {
                    if let Some(list) = get_list_at_path_mut(content, &parent_path) {
                        let clones: Vec<_> = indices
                            .iter()
                            .filter_map(|&idx| list.items.get(idx).cloned())
                            .collect();
                        let insert_pos = (max_idx + 1).min(list.items.len());
                        for (i, clone) in clones.into_iter().enumerate() {
                            list.items.insert(insert_pos + i, clone);
                        }
                    }
                }
                SegmentKind::TableRow => {
                    if let Some(StructuredNode::Table(t)) =
                        get_node_at_path_mut(content, &parent_path)
                    {
                        let mut clones: Vec<_> = indices
                            .iter()
                            .filter_map(|&idx| t.rows.get(idx).cloned())
                            .collect();
                        for row in &mut clones {
                            for cell in &mut row.cells {
                                reassign_field_ids(cell);
                            }
                        }
                        let insert_pos = (max_idx + 1).min(t.rows.len());
                        for (i, clone) in clones.into_iter().enumerate() {
                            t.rows.insert(insert_pos + i, clone);
                        }
                    }
                }
            }
        }
    }
}

/// Get the shared parent path if all paths are siblings (same parent container)
/// and all end with a `Child` segment.
///
/// Returns `None` if:
/// - `paths` is empty
/// - any path is empty or doesn't end with a `Child` segment
/// - paths don't all share the same parent prefix
pub fn get_shared_parent_path(paths: &[NodePath]) -> Option<NodePath> {
    if paths.is_empty() {
        return None;
    }

    // All paths must be non-empty and end with a Child segment
    if !paths
        .iter()
        .all(|p| !p.is_empty() && matches!(p.last(), Some(PathSegment::Child(_))))
    {
        return None;
    }

    // All paths must share the same parent (all but last segment)
    let first_parent = &paths[0][..paths[0].len() - 1];
    if paths.iter().all(|p| p[..p.len() - 1] == *first_parent) {
        Some(first_parent.to_vec())
    } else {
        None
    }
}

/// Check if the selected nodes can be merged.
pub fn can_merge_selected(
    content: &[StructuredNode],
    paths: &HashSet<NodePath>,
) -> Result<(), blueprint::ElementMergeError> {
    if paths.len() < 2 {
        return Err(blueprint::ElementMergeError::NotEnoughNodes);
    }

    // All selected nodes must be siblings (share the same parent container)
    let sorted_paths: Vec<NodePath> = paths.iter().cloned().collect();
    if get_shared_parent_path(&sorted_paths).is_none() {
        return Err(blueprint::ElementMergeError::NotEnoughNodes);
    }

    // Get all selected nodes
    let nodes: Vec<_> = paths
        .iter()
        .filter_map(|p| get_node_at_path(content, p))
        .collect();

    if nodes.len() < 2 {
        return Err(blueprint::ElementMergeError::NotEnoughNodes);
    }

    blueprint::can_merge_all(&nodes)
}

/// Get a summary text for a node (for display in the tree).
pub fn node_summary(node: &StructuredNode) -> String {
    match node {
        StructuredNode::Heading(h) => {
            let text = h.content.as_plain_text();
            let preview: String = text.chars().take(50).collect();
            format!(
                "H{}: {}",
                h.level.as_u8(),
                if text.len() > 50 {
                    format!("{}...", preview)
                } else {
                    preview
                }
            )
        }
        StructuredNode::Paragraph(p) => {
            let text = p.content.as_plain_text();
            let preview: String = text.chars().take(50).collect();
            if text.len() > 50 {
                format!("{}...", preview)
            } else {
                preview
            }
        }
        StructuredNode::Field(f) => {
            let label = f
                .label
                .as_ref()
                .map(|l| l.as_plain_text())
                .unwrap_or_default();
            format!(
                "Field: {}",
                if label.is_empty() {
                    f.name.to_string()
                } else {
                    label
                }
            )
        }
        StructuredNode::List(l) => {
            format!("List ({} items)", l.items.len())
        }
        StructuredNode::Table(t) => {
            format!("Table ({} rows)", t.rows.len())
        }
        StructuredNode::Group(g) => {
            format!("Group ({} children)", g.children.len())
        }
        StructuredNode::Repeatable(_) => "Repeatable".to_string(),
        StructuredNode::Conditional(_) => "Conditional".to_string(),
        StructuredNode::Image(_) => "Image".to_string(),
        StructuredNode::GridLayout(g) => {
            format!("Grid ({} cols, {} elements)", g.columns, g.elements.len())
        }
        StructuredNode::Empty => "Empty".to_string(),
        StructuredNode::Footnote(n) => {
            let preview: String = n.content.as_plain_text().chars().take(50).collect();
            format!("Footnote: {}", preview)
        }
    }
}

/// Get the type name of a node for display.
pub fn node_type_name(node: &StructuredNode) -> &'static str {
    match node {
        StructuredNode::Heading(_) => "Heading",
        StructuredNode::Paragraph(_) => "Paragraph",
        StructuredNode::Field(_) => "Field",
        StructuredNode::List(_) => "List",
        StructuredNode::Table(_) => "Table",
        StructuredNode::Group(_) => "Group",
        StructuredNode::Repeatable(_) => "Repeatable",
        StructuredNode::Conditional(_) => "Conditional",
        StructuredNode::Image(_) => "Image",
        StructuredNode::GridLayout(_) => "GridLayout",
        StructuredNode::Empty => "Empty",
        StructuredNode::Footnote(_) => "Footnote",
    }
}

/// Check if a node can have children.
pub fn node_has_children(node: &StructuredNode) -> bool {
    matches!(
        node,
        StructuredNode::Group(_)
            | StructuredNode::Table(_)
            | StructuredNode::GridLayout(_)
            | StructuredNode::Repeatable(_)
            | StructuredNode::Conditional(_)
            | StructuredNode::List(_)
    )
}

/// Get the children of a node if it has any.
#[allow(dead_code)]
pub fn node_children(node: &StructuredNode) -> Option<&[StructuredNode]> {
    match node {
        StructuredNode::Group(g) => Some(&g.children),
        _ => None,
    }
}

/// Collect all selectable paths for the given content.
///
/// This includes each root node path and may include descendants depending on node type.
pub fn collect_selectable_paths(content: &[StructuredNode]) -> HashSet<NodePath> {
    let mut paths = HashSet::new();
    for (i, node) in content.iter().enumerate() {
        let mut path = vec![PathSegment::Child(i)];
        collect_selectable_paths_from_node(node, &mut path, &mut paths);
    }
    paths
}

fn collect_selectable_paths_from_node(
    node: &StructuredNode,
    path: &mut NodePath,
    out: &mut HashSet<NodePath>,
) {
    out.insert(path.clone());

    match node {
        StructuredNode::Group(g) => {
            for (i, child) in g.children.iter().enumerate() {
                path.push(PathSegment::Child(i));
                collect_selectable_paths_from_node(child, path, out);
                path.pop();
            }
        }
        StructuredNode::GridLayout(g) => {
            for (i, element) in g.elements.iter().enumerate() {
                path.push(PathSegment::Child(i));
                collect_selectable_paths_from_node(&element.node, path, out);
                path.pop();
            }
        }
        StructuredNode::Repeatable(r) => {
            path.push(PathSegment::Child(0));
            collect_selectable_paths_from_node(&r.item, path, out);
            path.pop();
        }
        StructuredNode::Conditional(c) => {
            path.push(PathSegment::Child(0));
            collect_selectable_paths_from_node(&c.content, path, out);
            path.pop();
        }
        StructuredNode::List(l) => {
            collect_selectable_paths_from_list(l, path, out);
        }
        StructuredNode::Table(t) => {
            if let Some(header) = &t.header {
                path.push(PathSegment::TableHeader);
                out.insert(path.clone());

                for (ci, cell) in header.cells.iter().enumerate() {
                    path.push(PathSegment::TableCell(ci));
                    path.push(PathSegment::Child(0));
                    collect_selectable_paths_from_node(cell, path, out);
                    path.pop();
                    path.pop();
                }

                path.pop();
            }

            for (ri, row) in t.rows.iter().enumerate() {
                path.push(PathSegment::TableRow(ri));
                out.insert(path.clone());

                for (ci, cell) in row.cells.iter().enumerate() {
                    path.push(PathSegment::TableCell(ci));
                    path.push(PathSegment::Child(0));
                    collect_selectable_paths_from_node(cell, path, out);
                    path.pop();
                    path.pop();
                }

                path.pop();
            }
        }
        _ => {}
    }
}

fn collect_selectable_paths_from_list(
    list: &ListNode,
    path: &mut NodePath,
    out: &mut HashSet<NodePath>,
) {
    for (i, item) in list.items.iter().enumerate() {
        path.push(PathSegment::ListItem(i));
        out.insert(path.clone());

        if let Some(sublist) = &item.sublist {
            collect_selectable_paths_from_list(sublist, path, out);
        }

        path.pop();
    }
}

/// Determine available conversion targets for the current selection.
///
/// Returns a list of possible conversions based on what's selected:
/// - Single paragraph -> Heading
/// - Single heading -> Paragraph
/// - Multiple paragraphs -> List
/// - Single list -> Multiple paragraphs (converted internally to keep as one action)
pub fn available_conversions(
    content: &[StructuredNode],
    paths: &HashSet<NodePath>,
) -> Vec<ConvertTarget> {
    if paths.is_empty() {
        return vec![];
    }

    // All selected paths must be siblings (same parent, all ending with a Child segment)
    let sorted_paths: Vec<NodePath> = paths.iter().cloned().collect();
    if get_shared_parent_path(&sorted_paths).is_none() {
        return vec![];
    }

    // Get all selected nodes
    let nodes: Vec<&StructuredNode> = paths
        .iter()
        .filter_map(|p| get_node_at_path(content, p))
        .collect();

    if nodes.is_empty() {
        return vec![];
    }

    let mut targets = vec![];

    // Single node conversions
    if nodes.len() == 1 {
        match nodes[0] {
            StructuredNode::Paragraph(_) => {
                // Paragraph can become heading or field
                targets.push(ConvertTarget::Heading(2));
                targets.push(ConvertTarget::Field);
            }
            StructuredNode::Heading(_) => {
                // Heading can become paragraph or field
                targets.push(ConvertTarget::Paragraph);
                targets.push(ConvertTarget::Field);
            }
            StructuredNode::List(_) => {
                // List can become paragraphs (explode list items)
                targets.push(ConvertTarget::Paragraphs);
            }
            StructuredNode::Field(_) => {
                // Field can become paragraph or heading (label becomes content)
                targets.push(ConvertTarget::Paragraph);
                targets.push(ConvertTarget::Heading(2));
            }
            _ => {}
        }
    }

    // Multiple node conversions
    if nodes.len() >= 2 {
        // Check if all are paragraphs or headings (text-like content)
        let all_text_like = nodes
            .iter()
            .all(|n| matches!(n, StructuredNode::Paragraph(_) | StructuredNode::Heading(_)));

        if all_text_like {
            // Multiple paragraphs/headings can become a list
            targets.push(ConvertTarget::List);
        }
    }

    targets
}

/// Check if a path refers to a pseudo-node (ListItem, TableRow, TableHeader, TableCell).
#[allow(dead_code)]
pub fn is_pseudo_node_path(path: &NodePath) -> bool {
    path.last().is_some_and(|seg| {
        matches!(
            seg,
            PathSegment::ListItem(_)
                | PathSegment::TableRow(_)
                | PathSegment::TableHeader
                | PathSegment::TableCell(_)
        )
    })
}

/// Check if a path refers to a list item.
pub fn is_list_item_path(path: &NodePath) -> bool {
    path.last()
        .is_some_and(|seg| matches!(seg, PathSegment::ListItem(_)))
}

/// Check if a path refers to a table row.
pub fn is_table_row_path(path: &NodePath) -> bool {
    path.last()
        .is_some_and(|seg| matches!(seg, PathSegment::TableRow(_)))
}

/// Check if a path refers to a table cell.
pub fn is_table_cell_path(path: &NodePath) -> bool {
    path.last()
        .is_some_and(|seg| matches!(seg, PathSegment::TableCell(_)))
}

/// Get the parent path and item index for a list item path.
pub fn get_list_item_info(path: &NodePath) -> Option<(NodePath, usize)> {
    if let Some(PathSegment::ListItem(idx)) = path.last() {
        let parent_path: NodePath = path[..path.len() - 1].to_vec();
        Some((parent_path, *idx))
    } else {
        None
    }
}

/// Get the parent path and row index for a table row path.
pub fn get_table_row_info(path: &NodePath) -> Option<(NodePath, usize)> {
    if let Some(PathSegment::TableRow(idx)) = path.last() {
        let parent_path: NodePath = path[..path.len() - 1].to_vec();
        Some((parent_path, *idx))
    } else {
        None
    }
}

/// Move a list item up within its list.
/// Returns the new path if the move was successful.
pub fn move_list_item_up(content: &mut [StructuredNode], path: &NodePath) -> Option<NodePath> {
    let (parent_path, item_idx) = get_list_item_info(path)?;
    if item_idx == 0 {
        return None; // Can't move first item up
    }

    let list = get_list_at_path_mut(content, &parent_path)?;
    if item_idx < list.items.len() {
        list.items.swap(item_idx, item_idx - 1);
        let mut new_path = parent_path;
        new_path.push(PathSegment::ListItem(item_idx - 1));
        return Some(new_path);
    }
    None
}

/// Move a list item down within its list.
/// Returns the new path if the move was successful.
pub fn move_list_item_down(content: &mut [StructuredNode], path: &NodePath) -> Option<NodePath> {
    let (parent_path, item_idx) = get_list_item_info(path)?;

    let list = get_list_at_path_mut(content, &parent_path)?;
    if item_idx + 1 < list.items.len() {
        list.items.swap(item_idx, item_idx + 1);
        let mut new_path = parent_path;
        new_path.push(PathSegment::ListItem(item_idx + 1));
        return Some(new_path);
    }
    None
}

/// Move a table row up within its table.
/// Returns the new path if the move was successful.
pub fn move_table_row_up(content: &mut [StructuredNode], path: &NodePath) -> Option<NodePath> {
    let (parent_path, row_idx) = get_table_row_info(path)?;
    if row_idx == 0 {
        return None; // Can't move first row up
    }

    let parent = get_node_at_path_mut(content, &parent_path)?;
    if let StructuredNode::Table(t) = parent
        && row_idx < t.rows.len()
    {
        t.rows.swap(row_idx, row_idx - 1);
        let mut new_path = parent_path;
        new_path.push(PathSegment::TableRow(row_idx - 1));
        return Some(new_path);
    }
    None
}

/// Move a table row down within its table.
/// Returns the new path if the move was successful.
pub fn move_table_row_down(content: &mut [StructuredNode], path: &NodePath) -> Option<NodePath> {
    let (parent_path, row_idx) = get_table_row_info(path)?;

    let parent = get_node_at_path_mut(content, &parent_path)?;
    if let StructuredNode::Table(t) = parent
        && row_idx + 1 < t.rows.len()
    {
        t.rows.swap(row_idx, row_idx + 1);
        let mut new_path = parent_path;
        new_path.push(PathSegment::TableRow(row_idx + 1));
        return Some(new_path);
    }
    None
}

/// Get the list item text at a path for editing.
#[allow(dead_code)]
pub fn get_list_item_text<'a>(
    content: &'a [StructuredNode],
    path: &NodePath,
) -> Option<&'a TranslatedText> {
    let (parent_path, item_idx) = get_list_item_info(path)?;
    let list = get_list_at_path(content, &parent_path)?;
    list.items.get(item_idx).map(|item| &item.content)
}

/// Get mutable list item text at a path for editing.
pub fn get_list_item_text_mut<'a>(
    content: &'a mut [StructuredNode],
    path: &NodePath,
) -> Option<&'a mut TranslatedText> {
    let (parent_path, item_idx) = get_list_item_info(path)?;
    let list = get_list_at_path_mut(content, &parent_path)?;
    list.items.get_mut(item_idx).map(|item| &mut item.content)
}

/// Check if a path addresses a child inside a container (Group or GridLayout).
/// Returns true if the path has more than one Child segment and doesn't end with
/// a pseudo-node segment (ListItem, TableRow, TableHeader, TableCell).
pub fn is_container_child_path(path: &NodePath) -> bool {
    if path.len() < 2 {
        return false;
    }
    // Must end with a Child segment
    matches!(path.last(), Some(PathSegment::Child(_)))
}

/// Get the parent path and child index for a container child path.
pub fn get_container_child_info(path: &NodePath) -> Option<(NodePath, usize)> {
    if let Some(PathSegment::Child(idx)) = path.last()
        && path.len() >= 2
    {
        let parent_path: NodePath = path[..path.len() - 1].to_vec();
        return Some((parent_path, *idx));
    }
    None
}

/// Get the number of children in a container node.
pub fn get_container_children_count(
    content: &[StructuredNode],
    parent_path: &NodePath,
) -> Option<usize> {
    let parent = get_node_at_path(content, parent_path)?;
    match parent {
        StructuredNode::Group(g) => Some(g.children.len()),
        StructuredNode::GridLayout(g) => Some(g.elements.len()),
        _ => None,
    }
}

/// Move a child up within its container (Group or GridLayout).
/// Returns the new path if the move was successful.
pub fn move_container_child_up(
    content: &mut [StructuredNode],
    path: &NodePath,
) -> Option<NodePath> {
    let (parent_path, child_idx) = get_container_child_info(path)?;
    if child_idx == 0 {
        return None; // Can't move first child up
    }

    let parent = get_node_at_path_mut(content, &parent_path)?;
    match parent {
        StructuredNode::Group(g) if child_idx < g.children.len() => {
            g.children.swap(child_idx, child_idx - 1);
            let mut new_path = parent_path;
            new_path.push(PathSegment::Child(child_idx - 1));
            return Some(new_path);
        }
        StructuredNode::GridLayout(g) if child_idx < g.elements.len() => {
            g.elements.swap(child_idx, child_idx - 1);
            let mut new_path = parent_path;
            new_path.push(PathSegment::Child(child_idx - 1));
            return Some(new_path);
        }
        _ => {}
    }
    None
}

/// Move a child down within its container (Group or GridLayout).
/// Returns the new path if the move was successful.
pub fn move_container_child_down(
    content: &mut [StructuredNode],
    path: &NodePath,
) -> Option<NodePath> {
    let (parent_path, child_idx) = get_container_child_info(path)?;

    let parent = get_node_at_path_mut(content, &parent_path)?;
    match parent {
        StructuredNode::Group(g) if child_idx + 1 < g.children.len() => {
            g.children.swap(child_idx, child_idx + 1);
            let mut new_path = parent_path;
            new_path.push(PathSegment::Child(child_idx + 1));
            return Some(new_path);
        }
        StructuredNode::GridLayout(g) if child_idx + 1 < g.elements.len() => {
            g.elements.swap(child_idx, child_idx + 1);
            let mut new_path = parent_path;
            new_path.push(PathSegment::Child(child_idx + 1));
            return Some(new_path);
        }
        _ => {}
    }
    None
}

/// Check if a node can be indented (moved into the sibling container above it).
///
/// Returns true if the node's previous sibling is a container (Group or GridLayout).
pub fn can_indent(content: &[StructuredNode], path: &NodePath) -> bool {
    if path.is_empty() {
        return false;
    }

    // Get the sibling list and the node's index within it
    if path.len() == 1 {
        // Root-level node
        if let Some(PathSegment::Child(idx)) = path.first() {
            if *idx == 0 {
                return false;
            }
            return is_children_container(&content[idx - 1]);
        }
    } else if is_container_child_path(path)
        && let Some((parent_path, child_idx)) = get_container_child_info(path)
    {
        if child_idx == 0 {
            return false;
        }
        if let Some(parent) = get_node_at_path(content, &parent_path) {
            let prev_sibling = match parent {
                StructuredNode::Group(g) => g.children.get(child_idx - 1),
                StructuredNode::GridLayout(g) => g.elements.get(child_idx - 1).map(|e| &e.node),
                _ => None,
            };
            if let Some(sibling) = prev_sibling {
                return is_children_container(sibling);
            }
        }
    }
    false
}

/// Check if a node can be outdented (moved out of its container to the parent level).
///
/// Returns true if the node is inside a container (Group or GridLayout) that itself
/// has a parent (not at root level inside the container that is root-level).
pub fn can_outdent(_content: &[StructuredNode], path: &NodePath) -> bool {
    // Must be at least 2 segments: parent container + child index
    if path.len() < 2 {
        return false;
    }
    is_container_child_path(path)
}

/// Indent a node: remove it from its current position and append it to the
/// previous sibling container (Group or GridLayout).
/// Returns the new path if successful.
pub fn indent_node(content: &mut [StructuredNode], path: &NodePath) -> Option<NodePath> {
    if path.len() == 1 {
        // Root-level node
        let PathSegment::Child(idx) = path[0] else {
            return None;
        };
        if idx == 0 || idx >= content.len() {
            return None;
        }
        if !is_children_container(&content[idx - 1]) {
            return None;
        }

        // Root-level indent is handled directly in editor.rs since we need
        // Vec access (slices don't support remove/insert).
        return None;
    }

    // Container child: remove from current parent and append to previous sibling container
    let (parent_path, child_idx) = get_container_child_info(path)?;
    if child_idx == 0 {
        return None;
    }

    let parent = get_node_at_path_mut(content, &parent_path)?;
    match parent {
        StructuredNode::Group(g) => {
            if child_idx >= g.children.len() {
                return None;
            }
            if !is_children_container(&g.children[child_idx - 1]) {
                return None;
            }
            let node = g.children.remove(child_idx);
            // Append to previous sibling
            let extra_segments = append_to_container(&mut g.children[child_idx - 1], node)?;
            let mut new_path = parent_path;
            new_path.push(PathSegment::Child(child_idx - 1));
            new_path.extend(extra_segments);
            Some(new_path)
        }
        StructuredNode::GridLayout(g) => {
            if child_idx >= g.elements.len() {
                return None;
            }
            if !is_children_container(&g.elements[child_idx - 1].node) {
                return None;
            }
            let element = g.elements.remove(child_idx);
            let node = element.node;
            let extra_segments = append_to_container(&mut g.elements[child_idx - 1].node, node)?;
            let mut new_path = parent_path;
            new_path.push(PathSegment::Child(child_idx - 1));
            new_path.extend(extra_segments);
            Some(new_path)
        }
        _ => None,
    }
}

/// Outdent a node: remove it from its container and insert it after the container
/// in the grandparent's children list.
/// Returns the new path if successful.
pub fn outdent_node(content: &mut [StructuredNode], path: &NodePath) -> Option<NodePath> {
    if path.len() < 2 || !is_container_child_path(path) {
        return None;
    }

    let (parent_path, child_idx) = get_container_child_info(path)?;

    // If parent is at root level (parent_path.len() == 1), we need special handling
    // since the grandparent is the root Vec (handled in editor.rs).
    if parent_path.len() == 1 {
        return None; // Handled specially in editor.rs
    }

    // Otherwise, remove from parent and insert into grandparent after the parent
    let (grandparent_path, parent_idx_in_grandparent) = get_container_child_info(&parent_path)?;

    // Check if the grandparent is a Repeatable. If so, the node should be placed
    // after the Repeatable in the Repeatable's parent (one more level up).
    let grandparent_node = get_node_at_path(content, &grandparent_path)?;
    if matches!(grandparent_node, StructuredNode::Repeatable(_)) {
        // Grandparent is a Repeatable — outdent goes past it.
        // If the Repeatable is at root level, editor.rs handles it.
        if grandparent_path.len() == 1 {
            return None; // Handled specially in editor.rs
        }
        // Nested Repeatable: insert after the Repeatable in the great-grandparent
        let (great_grandparent_path, rep_idx_in_ggp) = get_container_child_info(&grandparent_path)?;

        // Remove from inner container
        let parent = get_node_at_path_mut(content, &parent_path)?;
        let node = match parent {
            StructuredNode::Group(g) => {
                if child_idx >= g.children.len() {
                    return None;
                }
                g.children.remove(child_idx)
            }
            StructuredNode::GridLayout(g) => {
                if child_idx >= g.elements.len() {
                    return None;
                }
                g.elements.remove(child_idx).node
            }
            _ => return None,
        };

        // Insert after the Repeatable in the great-grandparent
        let ggp = get_node_at_path_mut(content, &great_grandparent_path)?;
        let insert_idx = rep_idx_in_ggp + 1;
        match ggp {
            StructuredNode::Group(g) => {
                g.children.insert(insert_idx, node);
                let mut new_path = great_grandparent_path;
                new_path.push(PathSegment::Child(insert_idx));
                Some(new_path)
            }
            StructuredNode::GridLayout(g) => {
                g.elements
                    .insert(insert_idx, GridLayoutElement { span: 1, node });
                let mut new_path = great_grandparent_path;
                new_path.push(PathSegment::Child(insert_idx));
                Some(new_path)
            }
            _ => None,
        }
    } else {
        // Normal case: grandparent is Group/GridLayout
        // First, remove the node from the parent container
        let parent = get_node_at_path_mut(content, &parent_path)?;
        let node = match parent {
            StructuredNode::Group(g) => {
                if child_idx >= g.children.len() {
                    return None;
                }
                g.children.remove(child_idx)
            }
            StructuredNode::GridLayout(g) => {
                if child_idx >= g.elements.len() {
                    return None;
                }
                g.elements.remove(child_idx).node
            }
            _ => return None,
        };

        // Insert into grandparent after the parent's position
        let grandparent = get_node_at_path_mut(content, &grandparent_path)?;
        let insert_idx = parent_idx_in_grandparent + 1;
        match grandparent {
            StructuredNode::Group(g) => {
                g.children.insert(insert_idx, node);
                let mut new_path = grandparent_path;
                new_path.push(PathSegment::Child(insert_idx));
                Some(new_path)
            }
            StructuredNode::GridLayout(g) => {
                g.elements
                    .insert(insert_idx, GridLayoutElement { span: 1, node });
                let mut new_path = grandparent_path;
                new_path.push(PathSegment::Child(insert_idx));
                Some(new_path)
            }
            _ => None,
        }
    }
}

/// Append a node to a container (Group, GridLayout, or Repeatable wrapping one of those).
/// Returns the path segments to reach the new node relative to the container.
fn append_to_container(
    container: &mut StructuredNode,
    node: StructuredNode,
) -> Option<Vec<PathSegment>> {
    match container {
        StructuredNode::Group(g) => {
            g.children.push(node);
            Some(vec![PathSegment::Child(g.children.len() - 1)])
        }
        StructuredNode::GridLayout(g) => {
            g.elements.push(GridLayoutElement { span: 1, node });
            Some(vec![PathSegment::Child(g.elements.len() - 1)])
        }
        StructuredNode::Repeatable(r) => {
            // Indent into the Repeatable's item (which must be a Group/GridLayout)
            let inner_segments = append_to_container(r.item.as_mut(), node)?;
            let mut segments = vec![PathSegment::Child(0)];
            segments.extend(inner_segments);
            Some(segments)
        }
        _ => None,
    }
}

/// Check if a node is a container that can have arbitrary children.
fn is_children_container(node: &StructuredNode) -> bool {
    match node {
        StructuredNode::Group(_) | StructuredNode::GridLayout(_) => true,
        StructuredNode::Repeatable(r) => is_children_container(&r.item),
        _ => false,
    }
}

/// An option for the Add dropdown, carrying a label and the action payload.
#[derive(Clone, Debug, PartialEq)]
pub struct AddOption {
    pub label: &'static str,
    pub parent: NodePath,
    pub index: usize,
    pub node_type: NewNodeType,
}

/// Compute the list of add options based on the current selection.
///
/// When a single element is selected, the standard options (Paragraph, Heading,
/// List, Group) insert below it. When a list item, table row, or table cell is
/// selected, context-specific options are prepended.
pub fn compute_add_options(
    content: &[StructuredNode],
    selection: &SelectionState,
) -> Vec<AddOption> {
    let mut options = Vec::new();

    // Determine insertion target from selection
    let (parent, index) = if selection.selected.len() == 1 {
        let path = selection.selected.iter().next().unwrap();
        insertion_target_from_path(content, path)
    } else {
        // No selection or multiple selection: insert at root index 0
        (vec![], 0)
    };

    // Context-specific options
    if selection.selected.len() == 1 {
        let path = selection.selected.iter().next().unwrap();

        if is_list_item_path(path) {
            if let Some(PathSegment::ListItem(idx)) = path.last() {
                let list_parent: NodePath = path[..path.len() - 1].to_vec();
                options.push(AddOption {
                    label: "List Item",
                    parent: list_parent,
                    index: idx + 1,
                    node_type: NewNodeType::ListItem,
                });
            }
        } else if is_table_row_path(path) {
            if let Some(PathSegment::TableRow(idx)) = path.last() {
                let table_parent: NodePath = path[..path.len() - 1].to_vec();
                options.push(AddOption {
                    label: "Table Row",
                    parent: table_parent,
                    index: idx + 1,
                    node_type: NewNodeType::TableRow,
                });
            }
        } else if is_table_cell_path(path) {
            let row_parent: NodePath = path[..path.len() - 1].to_vec();
            if let Some(PathSegment::TableCell(idx)) = path.last() {
                options.push(AddOption {
                    label: "Table Cell",
                    parent: row_parent,
                    index: idx + 1,
                    node_type: NewNodeType::TableCell,
                });
            }
        }
    }

    // Standard options (always available) with computed insertion target
    for (label, node_type) in [
        ("Paragraph", NewNodeType::Paragraph),
        ("Heading", NewNodeType::Heading(2)),
        ("List", NewNodeType::List),
        ("Group", NewNodeType::Group),
        ("Repeatable", NewNodeType::Repeatable),
        ("Field", NewNodeType::Field),
    ] {
        options.push(AddOption {
            label,
            parent: parent.clone(),
            index,
            node_type,
        });
    }

    options
}

/// For a selected path, compute the parent path and insertion index such that
/// a new element is inserted directly below the selected element.
fn insertion_target_from_path(content: &[StructuredNode], path: &NodePath) -> (NodePath, usize) {
    // Root-level node: insert at root after this element
    if path.len() == 1
        && let Some(PathSegment::Child(idx)) = path.first()
    {
        return (vec![], idx + 1);
    }

    // Container child (Group/GridLayout): insert after this child in the same container
    if is_container_child_path(path)
        && let Some((parent_path, child_idx)) = get_container_child_info(path)
    {
        return (parent_path, child_idx + 1);
    }

    // List item selected: standard nodes go after the parent list at sibling level
    if is_list_item_path(path) {
        return sibling_insert_after(content, &path[..path.len() - 1]);
    }

    // Table row selected: standard nodes go after the parent table at sibling level
    if is_table_row_path(path) {
        return sibling_insert_after(content, &path[..path.len() - 1]);
    }

    // Table cell selected: standard nodes go after the parent table at sibling level
    if is_table_cell_path(path) {
        // Go up past TableCell and TableRow/TableHeader segments to reach the table
        let table_path: NodePath = path
            .iter()
            .take_while(|s| matches!(s, PathSegment::Child(_)))
            .cloned()
            .collect();
        return sibling_insert_after(content, &table_path);
    }

    // Fallback: root index 0
    (vec![], 0)
}

/// Given a node path, compute insertion target as "after this node" at its parent level.
fn sibling_insert_after(
    _content: &[StructuredNode],
    node_path: &[PathSegment],
) -> (NodePath, usize) {
    if node_path.is_empty() {
        return (vec![], 0);
    }

    // Find the nearest concrete node segment so nested pseudo-paths (e.g., list items)
    // can still insert after their owning node.
    for i in (0..node_path.len()).rev() {
        if let PathSegment::Child(idx) = node_path[i] {
            let parent: NodePath = node_path[..i].to_vec();
            return (parent, idx + 1);
        }
    }

    // Fallback: root index 0
    (vec![], 0)
}

/// Get the number of columns in a table (from header or first row).
pub fn get_table_column_count(table: &TableNode) -> usize {
    if let Some(header) = &table.header {
        return header.cells.len();
    }
    if let Some(first_row) = table.rows.first() {
        return first_row.cells.len();
    }
    1 // default to 1 column
}

// ── Search ────────────────────────────────────────────────────────────────────

/// Check whether a `TranslatedText` contains the given query string (case-insensitive) in any language.
pub fn translated_text_contains(text: &TranslatedText, query: &str) -> bool {
    text.iter()
        .any(|(_, inline)| inline.as_plain_text().to_lowercase().contains(query))
}

/// Check whether a list item's content contains the query string (case-insensitive) in any language.
fn list_item_contains(item: &ListItem, query: &str) -> bool {
    translated_text_contains(&item.content, query)
}

/// Search for all selectable node paths whose text content matches the query.
///
/// Searches across all languages. Returns paths in document order.
pub fn search_nodes(content: &[StructuredNode], query: &str) -> Vec<NodePath> {
    if query.is_empty() {
        return vec![];
    }
    let q = query.to_lowercase();
    let mut results = Vec::new();
    search_nodes_recursive(content, &mut vec![], &q, &mut results);
    results
}

fn search_nodes_recursive(
    nodes: &[StructuredNode],
    path: &mut NodePath,
    query: &str,
    results: &mut Vec<NodePath>,
) {
    for (i, node) in nodes.iter().enumerate() {
        path.push(PathSegment::Child(i));

        let matched = match node {
            StructuredNode::Paragraph(p) => translated_text_contains(&p.content, query),
            StructuredNode::Heading(h) => translated_text_contains(&h.content, query),
            StructuredNode::Field(f) => f
                .label
                .as_ref()
                .is_some_and(|l| translated_text_contains(l, query)),
            StructuredNode::Footnote(n) => translated_text_contains(&n.content, query),
            _ => false,
        };

        if matched {
            results.push(path.clone());
        }

        // Recurse into children
        match node {
            StructuredNode::Group(g) => {
                search_nodes_recursive(&g.children, path, query, results);
            }
            StructuredNode::GridLayout(g) => {
                let element_nodes: Vec<StructuredNode> =
                    g.elements.iter().map(|e| e.node.clone()).collect();
                search_nodes_recursive(&element_nodes, path, query, results);
            }
            StructuredNode::Repeatable(r) => {
                search_nodes_recursive(std::slice::from_ref(&r.item), path, query, results);
            }
            StructuredNode::Conditional(c) => {
                search_nodes_recursive(std::slice::from_ref(&c.content), path, query, results);
            }
            StructuredNode::List(l) => {
                search_list_items_recursive(l, path, query, results);
            }
            StructuredNode::Table(t) => {
                if let Some(header) = &t.header {
                    path.push(PathSegment::TableHeader);
                    for (ci, cell) in header.cells.iter().enumerate() {
                        path.push(PathSegment::TableCell(ci));
                        search_nodes_recursive(std::slice::from_ref(cell), path, query, results);
                        path.pop();
                    }
                    path.pop();
                }
                for (ri, row) in t.rows.iter().enumerate() {
                    path.push(PathSegment::TableRow(ri));
                    for (ci, cell) in row.cells.iter().enumerate() {
                        path.push(PathSegment::TableCell(ci));
                        search_nodes_recursive(std::slice::from_ref(cell), path, query, results);
                        path.pop();
                    }
                    path.pop();
                }
            }
            _ => {}
        }

        path.pop();
    }
}

fn search_list_items_recursive(
    list: &ListNode,
    path: &mut NodePath,
    query: &str,
    results: &mut Vec<NodePath>,
) {
    for (i, item) in list.items.iter().enumerate() {
        path.push(PathSegment::ListItem(i));
        if list_item_contains(item, query) {
            results.push(path.clone());
        }
        if let Some(sublist) = &item.sublist {
            search_list_items_recursive(sublist, path, query, results);
        }
        path.pop();
    }
}

// ── Missing Translation Highlighting ─────────────────────────────────────────

/// Check whether `text` has a missing translation for any of the given document languages.
///
/// A translation is considered missing when:
/// - A document language key exists in the `TranslatedText` but its `InlineText` is empty, or
/// - The text has non-empty content for at least one document language but no entry at all for
///   another document language (implicit absence).
///
/// Texts that only use the "default" key (single-language documents) are never flagged.
fn translated_text_has_missing(text: &TranslatedText, document_languages: &[String]) -> bool {
    // Explicit missing: a language key exists but the InlineText is empty
    if !text.missing_translation_languages().is_empty() {
        return true;
    }

    // Implicit missing: some doc language has content but another has no entry
    if document_languages.len() <= 1 {
        return false;
    }

    let mut text_langs = BTreeSet::new();
    text.collect_languages(&mut text_langs);

    // If the only key is "default", this is a mono-language text — not a translation issue
    if text_langs.len() == 1 && text_langs.contains("default") {
        return false;
    }

    let any_has_content = document_languages
        .iter()
        .any(|l| text.get(l).is_some_and(|t| !t.is_empty()));

    if !any_has_content {
        return false;
    }

    // At least one doc language has content; flag if any other lacks it
    document_languages
        .iter()
        .any(|l| text.get(l).is_none_or(|t| t.is_empty()))
}

/// Check whether a node has missing translations for any of the given document languages.
///
/// Only inspects the node's own text fields (not children); container nodes are never flagged.
pub fn node_has_missing_translations(node: &StructuredNode, document_languages: &[String]) -> bool {
    if document_languages.len() <= 1 {
        return false;
    }
    match node {
        StructuredNode::Paragraph(p) => translated_text_has_missing(&p.content, document_languages),
        StructuredNode::Heading(h) => translated_text_has_missing(&h.content, document_languages),
        StructuredNode::Field(f) => f
            .label
            .as_ref()
            .is_some_and(|l| translated_text_has_missing(l, document_languages)),
        StructuredNode::Footnote(n) => translated_text_has_missing(&n.content, document_languages),
        _ => false,
    }
}

/// Check whether a list item has missing translations for any of the given document languages.
pub fn list_item_has_missing_translations(item: &ListItem, document_languages: &[String]) -> bool {
    if document_languages.len() <= 1 {
        return false;
    }
    translated_text_has_missing(&item.content, document_languages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blueprint::{ListNode, ParagraphNode};
    use std::collections::HashSet;

    fn make_nested_list_content() -> Vec<StructuredNode> {
        vec![StructuredNode::List(ListNode {
            list_style: blueprint::document::ListStyleType::Disc,
            items: vec![
                ListItem {
                    content: TranslatedText::plain("Top 1"),
                    sublist: Some(Box::new(ListNode {
                        list_style: blueprint::document::ListStyleType::Disc,
                        items: vec![
                            ListItem::simple(TranslatedText::plain("Sub 1")),
                            ListItem::simple(TranslatedText::plain("Sub 2")),
                        ],
                    })),
                },
                ListItem::simple(TranslatedText::plain("Top 2")),
            ],
        })]
    }

    #[test]
    fn get_list_at_path_resolves_nested_sublist() {
        let content = make_nested_list_content();
        let path = vec![PathSegment::Child(0), PathSegment::ListItem(0)];
        let sublist = get_list_at_path(&content, &path).expect("sublist should resolve");

        assert_eq!(sublist.items.len(), 2);
        assert_eq!(sublist.items[0].as_plain_text(), "Sub 1");
    }

    #[test]
    fn get_list_item_text_mut_updates_nested_item() {
        let mut content = make_nested_list_content();
        let nested_item_path = vec![
            PathSegment::Child(0),
            PathSegment::ListItem(0),
            PathSegment::ListItem(1),
        ];

        let nested_text = get_list_item_text_mut(&mut content, &nested_item_path)
            .expect("nested list item text should be mutable");
        *nested_text = TranslatedText::plain("Sub 2 updated");

        let updated = get_list_item_text(&content, &nested_item_path)
            .expect("nested list item text should resolve");
        assert_eq!(updated.as_plain_text(), "Sub 2 updated");
    }

    #[test]
    fn move_list_item_up_moves_nested_item_within_sublist() {
        let mut content = make_nested_list_content();
        let path = vec![
            PathSegment::Child(0),
            PathSegment::ListItem(0),
            PathSegment::ListItem(1),
        ];

        let new_path = move_list_item_up(&mut content, &path).expect("move should succeed");
        assert_eq!(
            new_path,
            vec![
                PathSegment::Child(0),
                PathSegment::ListItem(0),
                PathSegment::ListItem(0)
            ]
        );

        let moved = get_list_item_text(&content, &new_path).expect("moved item should resolve");
        assert_eq!(moved.as_plain_text(), "Sub 2");
    }

    #[test]
    fn delete_nodes_deletes_nested_list_item() {
        let mut content = make_nested_list_content();
        let path = vec![
            PathSegment::Child(0),
            PathSegment::ListItem(0),
            PathSegment::ListItem(1),
        ];

        let mut paths = HashSet::new();
        paths.insert(path);
        delete_nodes(&mut content, &paths);

        let sublist_path = vec![PathSegment::Child(0), PathSegment::ListItem(0)];
        let sublist = get_list_at_path(&content, &sublist_path).expect("sublist should resolve");
        assert_eq!(sublist.items.len(), 1);
        assert_eq!(sublist.items[0].as_plain_text(), "Sub 1");
    }

    #[test]
    fn compute_add_options_for_nested_list_item_keeps_nested_parent() {
        let content = make_nested_list_content();
        let nested_path = vec![
            PathSegment::Child(0),
            PathSegment::ListItem(0),
            PathSegment::ListItem(1),
        ];

        let mut selection = SelectionState::new();
        selection.select_single(nested_path);

        let options = compute_add_options(&content, &selection);
        let list_item_option = options
            .iter()
            .find(|o| o.node_type == NewNodeType::ListItem)
            .expect("list item option should exist");

        assert_eq!(
            list_item_option.parent,
            vec![PathSegment::Child(0), PathSegment::ListItem(0)]
        );
        assert_eq!(list_item_option.index, 2);
    }

    #[test]
    fn get_node_at_path_still_resolves_regular_nodes() {
        let content = vec![StructuredNode::Group(blueprint::GroupNode {
            children: vec![StructuredNode::Paragraph(ParagraphNode {
                content: TranslatedText::plain("Hello"),
                som_path: None,
                source_name: None,
            })],
        })];

        let path = vec![PathSegment::Child(0), PathSegment::Child(0)];
        let node = get_node_at_path(&content, &path).expect("regular child path should resolve");

        assert!(matches!(node, StructuredNode::Paragraph(_)));
    }

    #[test]
    fn collect_selectable_paths_includes_descendants() {
        let content = vec![StructuredNode::Group(blueprint::GroupNode {
            children: vec![StructuredNode::List(ListNode {
                list_style: blueprint::document::ListStyleType::Disc,
                items: vec![ListItem::simple(TranslatedText::plain("Nested item"))],
            })],
        })];

        let paths = collect_selectable_paths(&content);

        assert!(paths.contains(&vec![PathSegment::Child(0)]));
        assert!(paths.contains(&vec![PathSegment::Child(0), PathSegment::Child(0)]));
        assert!(paths.contains(&vec![
            PathSegment::Child(0),
            PathSegment::Child(0),
            PathSegment::ListItem(0)
        ]));
    }

    fn make_field(id: &str, label: &str) -> StructuredNode {
        use blueprint::{FieldNode, FieldType};
        StructuredNode::Field(FieldNode {
            name: FieldId::from(id),
            som_path: None,
            label: Some(TranslatedText::plain(label)),
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
            required: false,
        })
    }

    fn field_id_of(node: &StructuredNode) -> FieldId {
        match node {
            StructuredNode::Field(f) => f.name.clone(),
            _ => panic!("expected a field node"),
        }
    }

    #[test]
    fn duplicate_nodes_assigns_unique_field_ids() {
        let mut content = vec![make_field("form.field1", "Label")];

        let mut paths = HashSet::new();
        paths.insert(vec![PathSegment::Child(0)]);
        duplicate_nodes(&mut content, &paths);

        assert_eq!(content.len(), 2);
        let original_id = field_id_of(&content[0]);
        let copy_id = field_id_of(&content[1]);
        assert_eq!(
            original_id,
            FieldId::from("form.field1"),
            "original field id must be unchanged"
        );
        assert_ne!(
            original_id, copy_id,
            "duplicated field must receive a fresh unique id"
        );
        // The label (and its translations) should still be copied verbatim.
        match &content[1] {
            StructuredNode::Field(f) => {
                assert_eq!(
                    f.label.as_ref().map(|l| l.as_plain_text()),
                    Some("Label".to_string())
                );
            }
            _ => panic!("expected a field node"),
        }
    }

    #[test]
    fn duplicate_nodes_remaps_conditional_references_within_subtree() {
        use blueprint::structured::FieldCondition;
        use blueprint::{ConditionalNode, GroupNode, InputValue};

        // A group containing a field plus a conditional that references that field.
        let group = StructuredNode::Group(GroupNode {
            children: vec![
                make_field("form.trigger", "Trigger"),
                StructuredNode::Conditional(ConditionalNode {
                    condition: FieldCondition {
                        field_name: FieldId::from("form.trigger"),
                        value: InputValue::Bool(true),
                    },
                    content: Box::new(make_field("form.dependent", "Dependent")),
                }),
            ],
        });
        let mut content = vec![group];

        let mut paths = HashSet::new();
        paths.insert(vec![PathSegment::Child(0)]);
        duplicate_nodes(&mut content, &paths);

        assert_eq!(content.len(), 2);

        let copy_children = match &content[1] {
            StructuredNode::Group(g) => &g.children,
            _ => panic!("expected a group node"),
        };
        let copied_trigger_id = field_id_of(&copy_children[0]);
        let copied_condition_id = match &copy_children[1] {
            StructuredNode::Conditional(c) => c.condition.field_name.clone(),
            _ => panic!("expected a conditional node"),
        };

        // The conditional in the copy must point at the copy's trigger field,
        // not the original one.
        assert_ne!(copied_trigger_id, FieldId::from("form.trigger"));
        assert_eq!(
            copied_trigger_id, copied_condition_id,
            "conditional reference must be remapped to the duplicated field"
        );
    }
}
