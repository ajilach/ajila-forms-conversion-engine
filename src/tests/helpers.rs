use crate::structured::{
    ConditionalNode, FieldNode, FieldType, HeadingNode, InlineNode, ListNode, StructuredNode,
    FieldId,
};

/// Recursively walk a tree of `StructuredNode`s, calling `callback` on every
/// node encountered (depth-first, pre-order).
///
/// The walker handles all node variants that can contain children:
/// `Group`, `Conditional`, `Repeatable`, `GridLayout`, and `Table`.
///
/// # Examples
///
/// Collecting all `FieldNode`s:
/// ```ignore
/// let mut fields = Vec::new();
/// walk_structured_nodes(&nodes, &mut |node| {
///     if let StructuredNode::Field(f) = node {
///         fields.push(f.clone());
///     }
/// });
/// ```
pub fn walk_structured_nodes(nodes: &[StructuredNode], callback: &mut impl FnMut(&StructuredNode)) {
    for node in nodes {
        callback(node);
        match node {
            StructuredNode::Group(group) => {
                walk_structured_nodes(&group.children, callback);
            }
            StructuredNode::Conditional(cond) => {
                walk_structured_nodes(std::slice::from_ref(cond.content.as_ref()), callback);
            }
            StructuredNode::Repeatable(rep) => {
                walk_structured_nodes(std::slice::from_ref(rep.item.as_ref()), callback);
            }
            StructuredNode::GridLayout(grid) => {
                for element in &grid.elements {
                    walk_structured_nodes(std::slice::from_ref(&element.node), callback);
                }
            }
            StructuredNode::Table(table) => {
                if let Some(header) = &table.header {
                    walk_structured_nodes(&header.cells, callback);
                }
                for row in &table.rows {
                    walk_structured_nodes(&row.cells, callback);
                }
            }
            _ => {}
        }
    }
}

// ============================================================================
// Typed collector helpers built on top of `walk_structured_nodes`
// ============================================================================

/// Collect all `FieldNode`s from the tree.
pub fn collect_fields(nodes: &[StructuredNode]) -> Vec<FieldNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Field(f) = node {
            out.push(f.clone());
        }
    });
    out
}

/// Collect all `FieldNode`s whose `input_type` is `Radio`.
pub fn collect_radio_fields(nodes: &[StructuredNode]) -> Vec<FieldNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Field(f) = node {
            if matches!(f.input_type, FieldType::Radio { .. }) {
                out.push(f.clone());
            }
        }
    });
    out
}

/// Collect field labels (as plain text) from all `FieldNode`s in the tree.
pub fn collect_field_labels(nodes: &[StructuredNode]) -> Vec<String> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Field(f) = node {
            if let Some(label) = &f.label {
                out.push(label.as_plain_text());
            }
        }
    });
    out
}

/// Collect field names (SOM path strings) from all `FieldNode`s in the tree.
pub fn collect_field_names(nodes: &[StructuredNode]) -> Vec<String> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Field(f) = node {
            out.push(f.som_path_str().to_string());
        }
    });
    out
}

/// Collect all `HeadingNode`s as `(level, text)` pairs.
pub fn collect_headings(nodes: &[StructuredNode]) -> Vec<(u8, String)> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Heading(h) = node {
            out.push((h.level.as_u8(), h.content.as_plain_text()));
        }
    });
    out
}

/// Collect all `ConditionalNode`s from the tree.
pub fn collect_conditionals(nodes: &[StructuredNode]) -> Vec<ConditionalNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Conditional(c) = node {
            out.push(c.clone());
        }
    });
    out
}

/// Count the number of `Conditional` nodes in the tree.
pub fn count_conditionals(nodes: &[StructuredNode]) -> usize {
    let mut count = 0;
    walk_structured_nodes(nodes, &mut |node| {
        if matches!(node, StructuredNode::Conditional(_)) {
            count += 1;
        }
    });
    count
}

/// Collect all `ListNode`s from the tree.
pub fn collect_lists(nodes: &[StructuredNode]) -> Vec<ListNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::List(l) = node {
            out.push(l.clone());
        }
    });
    out
}

/// Collect all `InlineNode`s from `Paragraph` nodes in the tree.
pub fn collect_inline_nodes(nodes: &[StructuredNode]) -> Vec<InlineNode> {
    let mut out = Vec::new();
    walk_structured_nodes(nodes, &mut |node| {
        if let StructuredNode::Paragraph(p) = node {
            out.extend(p.content.0.clone());
        }
    });
    out
}

/// Find the `FieldId` of the first field whose SOM path ends with `suffix`.
pub fn find_field_id_by_suffix(nodes: &[StructuredNode], suffix: &str) -> Option<FieldId> {
    let fields = collect_fields(nodes);
    fields
        .iter()
        .find(|f| f.som_path_str().ends_with(suffix))
        .map(|f| f.name.clone())
}
