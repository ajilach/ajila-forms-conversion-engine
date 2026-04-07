//! Editor state management.
//!
//! This module provides the state types and operations for the structured
//! document editor.

use blueprint::{InlineText, StructuredNode};
use std::collections::HashSet;

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
    /// Start editing a node's text.
    StartEditing(NodePath),
    /// Update text content of a node (works for paragraphs, headings, field labels, list items).
    UpdateText { path: NodePath, content: String, language: Option<String> },
    /// Stop editing.
    StopEditing,
    /// Start editing a node's metadata.
    StartEditingMetadata(NodePath),
    /// Update node metadata.
    UpdateMetadata { path: NodePath, metadata: NodeMetadata },
    /// Add a new node.
    AddNode { parent: NodePath, index: usize, node_type: NewNodeType },
    /// Convert selected node(s) to a different type.
    ConvertSelected(ConvertTarget),
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
}

/// Simplified field input type for the editor UI.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldInputKind {
    Text,
    Number,
    Date,
    Email,
    Tel,
    Checkbox,
    Dropdown,
    Radio,
}

/// Types of nodes that can be added.
#[derive(Clone, Debug)]
pub enum NewNodeType {
    Paragraph,
    Heading(u8),
    List,
    Group,
}

/// Get a node at a given path.
///
/// Note: This only works for paths that resolve to StructuredNode references.
/// ListItem paths cannot be resolved this way (list items are InlineText, not StructuredNode).
pub fn get_node_at_path<'a>(content: &'a [StructuredNode], path: &NodePath) -> Option<&'a StructuredNode> {
    if path.is_empty() {
        return None;
    }

    let first_segment = &path[0];
    let PathSegment::Child(first_idx) = first_segment else {
        return None; // Path must start with Child
    };

    let mut current = content.get(*first_idx)?;

    for segment in &path[1..] {
        current = match (current, segment) {
            // Child navigation
            (StructuredNode::Group(g), PathSegment::Child(idx)) => g.children.get(*idx)?,
            (StructuredNode::GridLayout(g), PathSegment::Child(idx)) => g.elements.get(*idx).map(|e| &e.node)?,
            (StructuredNode::Repeatable(r), PathSegment::Child(0)) => r.item.as_ref(),
            (StructuredNode::Conditional(c), PathSegment::Child(0)) => c.content.as_ref(),

            // Table navigation
            (StructuredNode::Table(t), PathSegment::TableRow(row_idx)) => {
                // TableRow is not a StructuredNode itself, but we continue navigation
                // This case returns None - use get_table_cell_at_path for cells
                let _ = t.rows.get(*row_idx)?;
                return None; // Can't return TableRow as StructuredNode
            }
            (StructuredNode::Table(t), PathSegment::TableHeader) => {
                let _ = t.header.as_ref()?;
                return None; // Can't return TableHeader as StructuredNode
            }

            // ListItem is InlineText, not StructuredNode
            (StructuredNode::List(_), PathSegment::ListItem(_)) => return None,

            _ => return None,
        };
    }

    Some(current)
}

/// Get a table cell at a given path.
///
/// For paths ending with TableCell, this returns the cell's StructuredNode.
pub fn get_table_cell_at_path<'a>(content: &'a [StructuredNode], path: &NodePath) -> Option<&'a StructuredNode> {
    if path.len() < 3 {
        return None;
    }

    // Navigate to the table first
    let table_path: NodePath = path.iter().take_while(|s| matches!(s, PathSegment::Child(_))).cloned().collect();
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
/// Note: This only works for paths that resolve to StructuredNode references.
pub fn get_node_at_path_mut<'a>(content: &'a mut [StructuredNode], path: &NodePath) -> Option<&'a mut StructuredNode> {
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

    let mut current = &mut content[*first_idx];

    for segment in &path[1..] {
        current = match (current, segment) {
            // Child navigation
            (StructuredNode::Group(g), PathSegment::Child(idx)) => g.children.get_mut(*idx)?,
            (StructuredNode::GridLayout(g), PathSegment::Child(idx)) => g.elements.get_mut(*idx).map(|e| &mut e.node)?,
            (StructuredNode::Repeatable(r), PathSegment::Child(0)) => r.item.as_mut(),
            (StructuredNode::Conditional(c), PathSegment::Child(0)) => c.content.as_mut(),

            // For ListItem, we can't return the item as StructuredNode
            (StructuredNode::List(_), PathSegment::ListItem(_)) => return None,

            _ => return None,
        };
    }

    Some(current)
}

/// Get a mutable reference to a table cell at a given path.
pub fn get_table_cell_at_path_mut<'a>(content: &'a mut [StructuredNode], path: &NodePath) -> Option<&'a mut StructuredNode> {
    if path.len() < 3 {
        return None;
    }

    // Navigate to the table first
    let table_path: NodePath = path.iter().take_while(|s| matches!(s, PathSegment::Child(_))).cloned().collect();

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
            if let PathSegment::Child(idx) = last_segment {
                if *idx < content.len() {
                    content.remove(*idx);
                }
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
                            StructuredNode::Group(g) => {
                                if *child_idx < g.children.len() {
                                    g.children.remove(*child_idx);
                                }
                            }
                            StructuredNode::GridLayout(g) => {
                                if *child_idx < g.elements.len() {
                                    g.elements.remove(*child_idx);
                                }
                            }
                            StructuredNode::Repeatable(r) => {
                                if *child_idx == 0 {
                                    *r.item = StructuredNode::Empty;
                                }
                            }
                            StructuredNode::Conditional(c) => {
                                if *child_idx == 0 {
                                    *c.content = StructuredNode::Empty;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                PathSegment::ListItem(item_idx) => {
                    // Deletion of a list item
                    if let Some(parent) = get_node_at_path_mut(content, &parent_path) {
                        if let StructuredNode::List(l) = parent {
                            if *item_idx < l.items.len() {
                                l.items.remove(*item_idx);
                            }
                        }
                    }
                }
                PathSegment::TableRow(row_idx) => {
                    // Deletion of a table row
                    if let Some(parent) = get_node_at_path_mut(content, &parent_path) {
                        if let StructuredNode::Table(t) = parent {
                            if *row_idx < t.rows.len() {
                                t.rows.remove(*row_idx);
                            }
                        }
                    }
                }
                PathSegment::TableHeader => {
                    // Deletion of table header
                    if let Some(parent) = get_node_at_path_mut(content, &parent_path) {
                        if let StructuredNode::Table(t) = parent {
                            t.header = None;
                        }
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

/// Check if the selected nodes can be merged.
pub fn can_merge_selected(content: &[StructuredNode], paths: &HashSet<NodePath>) -> Result<(), blueprint::ElementMergeError> {
    if paths.len() < 2 {
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
            format!("H{}: {}", h.level.as_u8(), if text.len() > 50 { format!("{}...", preview) } else { preview })
        }
        StructuredNode::Paragraph(p) => {
            let text = p.content.as_plain_text();
            let preview: String = text.chars().take(50).collect();
            if text.len() > 50 { format!("{}...", preview) } else { preview }
        }
        StructuredNode::Field(f) => {
            let label = f.label.as_ref().map(|l| l.as_plain_text()).unwrap_or_default();
            format!("Field: {}", if label.is_empty() { f.name.to_string() } else { label })
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

/// Determine available conversion targets for the current selection.
///
/// Returns a list of possible conversions based on what's selected:
/// - Single paragraph -> Heading
/// - Single heading -> Paragraph
/// - Multiple paragraphs -> List
/// - Single list -> Multiple paragraphs (converted internally to keep as one action)
pub fn available_conversions(content: &[StructuredNode], paths: &HashSet<NodePath>) -> Vec<ConvertTarget> {
    if paths.is_empty() {
        return vec![];
    }

    // Only support root-level conversions for now (single Child segment)
    if !paths.iter().all(|p| p.len() == 1 && matches!(p.first(), Some(PathSegment::Child(_)))) {
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
        let all_text_like = nodes.iter().all(|n| {
            matches!(n, StructuredNode::Paragraph(_) | StructuredNode::Heading(_))
        });

        if all_text_like {
            // Multiple paragraphs/headings can become a list
            targets.push(ConvertTarget::List);
        }
    }

    targets
}

/// Check if a path refers to a pseudo-node (ListItem, TableRow, TableHeader, TableCell).
pub fn is_pseudo_node_path(path: &NodePath) -> bool {
    path.last().map_or(false, |seg| {
        matches!(
            seg,
            PathSegment::ListItem(_) | PathSegment::TableRow(_) | PathSegment::TableHeader | PathSegment::TableCell(_)
        )
    })
}

/// Check if a path refers to a list item.
pub fn is_list_item_path(path: &NodePath) -> bool {
    path.last().map_or(false, |seg| matches!(seg, PathSegment::ListItem(_)))
}

/// Check if a path refers to a table row.
pub fn is_table_row_path(path: &NodePath) -> bool {
    path.last().map_or(false, |seg| matches!(seg, PathSegment::TableRow(_)))
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
pub fn move_list_item_up(content: &mut Vec<StructuredNode>, path: &NodePath) -> Option<NodePath> {
    let (parent_path, item_idx) = get_list_item_info(path)?;
    if item_idx == 0 {
        return None; // Can't move first item up
    }

    let parent = get_node_at_path_mut(content, &parent_path)?;
    if let StructuredNode::List(l) = parent {
        if item_idx < l.items.len() {
            l.items.swap(item_idx, item_idx - 1);
            let mut new_path = parent_path;
            new_path.push(PathSegment::ListItem(item_idx - 1));
            return Some(new_path);
        }
    }
    None
}

/// Move a list item down within its list.
/// Returns the new path if the move was successful.
pub fn move_list_item_down(content: &mut Vec<StructuredNode>, path: &NodePath) -> Option<NodePath> {
    let (parent_path, item_idx) = get_list_item_info(path)?;

    let parent = get_node_at_path_mut(content, &parent_path)?;
    if let StructuredNode::List(l) = parent {
        if item_idx + 1 < l.items.len() {
            l.items.swap(item_idx, item_idx + 1);
            let mut new_path = parent_path;
            new_path.push(PathSegment::ListItem(item_idx + 1));
            return Some(new_path);
        }
    }
    None
}

/// Move a table row up within its table.
/// Returns the new path if the move was successful.
pub fn move_table_row_up(content: &mut Vec<StructuredNode>, path: &NodePath) -> Option<NodePath> {
    let (parent_path, row_idx) = get_table_row_info(path)?;
    if row_idx == 0 {
        return None; // Can't move first row up
    }

    let parent = get_node_at_path_mut(content, &parent_path)?;
    if let StructuredNode::Table(t) = parent {
        if row_idx < t.rows.len() {
            t.rows.swap(row_idx, row_idx - 1);
            let mut new_path = parent_path;
            new_path.push(PathSegment::TableRow(row_idx - 1));
            return Some(new_path);
        }
    }
    None
}

/// Move a table row down within its table.
/// Returns the new path if the move was successful.
pub fn move_table_row_down(content: &mut Vec<StructuredNode>, path: &NodePath) -> Option<NodePath> {
    let (parent_path, row_idx) = get_table_row_info(path)?;

    let parent = get_node_at_path_mut(content, &parent_path)?;
    if let StructuredNode::Table(t) = parent {
        if row_idx + 1 < t.rows.len() {
            t.rows.swap(row_idx, row_idx + 1);
            let mut new_path = parent_path;
            new_path.push(PathSegment::TableRow(row_idx + 1));
            return Some(new_path);
        }
    }
    None
}

/// Get the list item text at a path for editing.
pub fn get_list_item_text<'a>(content: &'a [StructuredNode], path: &NodePath) -> Option<&'a InlineText> {
    let (parent_path, item_idx) = get_list_item_info(path)?;
    let parent = get_node_at_path(content, &parent_path)?;
    if let StructuredNode::List(l) = parent {
        l.items.get(item_idx)
    } else {
        None
    }
}

/// Get mutable list item text at a path for editing.
pub fn get_list_item_text_mut<'a>(content: &'a mut [StructuredNode], path: &NodePath) -> Option<&'a mut InlineText> {
    let (parent_path, item_idx) = get_list_item_info(path)?;
    let parent = get_node_at_path_mut(content, &parent_path)?;
    if let StructuredNode::List(l) = parent {
        l.items.get_mut(item_idx)
    } else {
        None
    }
}
