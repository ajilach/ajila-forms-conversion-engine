//! Editor state management.
//!
//! This module provides the state types and operations for the structured
//! document editor.

use blueprint::StructuredNode;
use std::collections::HashSet;

/// A path to a node in the document tree.
///
/// Each element in the vector represents an index into the children array
/// at that level of the tree. For example, `[0, 2, 1]` means:
/// - Root content[0]
/// - If that's a Group, its children[2]
/// - If that's a Group, its children[1]
pub type NodePath = Vec<usize>;

/// The current selection state in the editor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectionState {
    /// Set of selected node paths.
    pub selected: HashSet<NodePath>,
    /// The node currently being edited (text editing mode).
    pub editing: Option<NodePath>,
    /// The list item currently being edited (path to list node, item index).
    pub editing_list_item: Option<(NodePath, usize)>,
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
        self.editing_list_item = None;
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
        self.editing_list_item = None;
    }

    /// Start editing a list item.
    pub fn start_editing_list_item(&mut self, path: NodePath, index: usize) {
        self.editing = None;
        self.editing_list_item = Some((path, index));
    }

    /// Check if we're editing a specific list item.
    pub fn is_editing_list_item(&self, path: &NodePath, index: usize) -> bool {
        self.editing_list_item.as_ref() == Some(&(path.clone(), index))
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
    /// Start editing a list item.
    StartEditingListItem(NodePath, usize),
    /// Update text content of a node.
    UpdateText { path: NodePath, content: String, language: Option<String> },
    /// Update list item content.
    UpdateListItem { path: NodePath, item_index: usize, content: String, language: Option<String> },
    /// Stop editing.
    StopEditing,
    /// Add a new node.
    AddNode { parent: NodePath, index: usize, node_type: NewNodeType },
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
pub fn get_node_at_path<'a>(content: &'a [StructuredNode], path: &NodePath) -> Option<&'a StructuredNode> {
    if path.is_empty() {
        return None;
    }

    let mut current = content.get(path[0])?;

    for &idx in &path[1..] {
        current = match current {
            StructuredNode::Group(g) => g.children.get(idx)?,
            StructuredNode::Table(_) => {
                // For tables, we need to navigate through rows/cells
                // This is a simplified version - tables have a more complex structure
                return None;
            }
            StructuredNode::GridLayout(g) => g.elements.get(idx).map(|e| &e.node)?,
            StructuredNode::Repeatable(r) => {
                if idx == 0 {
                    r.item.as_ref()
                } else {
                    return None;
                }
            }
            StructuredNode::Conditional(c) => {
                if idx == 0 {
                    c.content.as_ref()
                } else {
                    return None;
                }
            }
            _ => return None,
        };
    }

    Some(current)
}

/// Get a mutable reference to a node at a given path.
pub fn get_node_at_path_mut<'a>(content: &'a mut [StructuredNode], path: &NodePath) -> Option<&'a mut StructuredNode> {
    if path.is_empty() {
        return None;
    }

    let first_idx = path[0];
    if first_idx >= content.len() {
        return None;
    }

    let mut current = &mut content[first_idx];

    for &idx in &path[1..] {
        current = match current {
            StructuredNode::Group(g) => g.children.get_mut(idx)?,
            StructuredNode::GridLayout(g) => g.elements.get_mut(idx).map(|e| &mut e.node)?,
            StructuredNode::Repeatable(r) => {
                if idx == 0 {
                    r.item.as_mut()
                } else {
                    return None;
                }
            }
            StructuredNode::Conditional(c) => {
                if idx == 0 {
                    c.content.as_mut()
                } else {
                    return None;
                }
            }
            _ => return None,
        };
    }

    Some(current)
}

/// Delete nodes at the given paths from the document.
///
/// Paths are processed in reverse order (deepest first, highest index first)
/// to avoid invalidating indices.
pub fn delete_nodes(content: &mut Vec<StructuredNode>, paths: &HashSet<NodePath>) {
    // Sort paths: deepest first, then by index descending
    let mut sorted_paths: Vec<_> = paths.iter().cloned().collect();
    sorted_paths.sort_by(|a, b| {
        // First by depth (longer paths first)
        let depth_cmp = b.len().cmp(&a.len());
        if depth_cmp != std::cmp::Ordering::Equal {
            return depth_cmp;
        }
        // Then by last index descending
        b.last().cmp(&a.last())
    });

    for path in sorted_paths {
        if path.is_empty() {
            continue;
        }

        if path.len() == 1 {
            // Root level deletion
            let idx = path[0];
            if idx < content.len() {
                content.remove(idx);
            }
        } else {
            // Nested deletion - get parent and remove child
            let parent_path = &path[..path.len() - 1];
            let child_idx = path[path.len() - 1];

            if let Some(parent) = get_node_at_path_mut(content, &parent_path.to_vec()) {
                match parent {
                    StructuredNode::Group(g) => {
                        if child_idx < g.children.len() {
                            g.children.remove(child_idx);
                        }
                    }
                    StructuredNode::GridLayout(g) => {
                        if child_idx < g.elements.len() {
                            g.elements.remove(child_idx);
                        }
                    }
                    StructuredNode::Repeatable(r) => {
                        // Repeatable has only one item template (at index 0)
                        // Replace with Empty instead of removing
                        if child_idx == 0 {
                            r.item = Box::new(StructuredNode::Empty);
                        }
                    }
                    StructuredNode::Conditional(c) => {
                        // Conditional has only one content (at index 0)
                        // Replace with Empty instead of removing
                        if child_idx == 0 {
                            c.content = Box::new(StructuredNode::Empty);
                        }
                    }
                    _ => {}
                }
            }
        }
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
pub fn node_children(node: &StructuredNode) -> Option<&[StructuredNode]> {
    match node {
        StructuredNode::Group(g) => Some(&g.children),
        _ => None,
    }
}
