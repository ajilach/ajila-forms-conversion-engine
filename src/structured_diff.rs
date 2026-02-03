//! Structural diff algorithm for StructuredNode trees.
//!
//! Compares two trees to detect structurally added and removed nodes.
//! Structural identity includes all type metadata, text content, and condition values,
//! but excludes field input values.

use crate::structured::{
    ConditionalNode, FieldNode, FieldType, GroupNode, HeadingNode, ImageNode, InputValue,
    ParagraphNode, RepeatableNode, StructuredNode, TableHeader, TableNode, TableRow,
};

/// Result of diffing two StructuredNode trees
#[derive(Debug, Clone, Default)]
pub struct DiffResult {
    /// Nodes that were added (present in new tree, absent in old)
    pub added: Vec<StructuredNode>,
    /// Nodes that were removed (present in old tree, absent in new)
    pub removed: Vec<StructuredNode>,
}

/// A single field value change between two structurally equivalent trees
#[derive(Debug, Clone)]
pub struct FieldValueChange {
    /// The unique name of the field
    pub field_name: String,
    /// The old value (from the first tree)
    pub old_value: Option<InputValue>,
    /// The new value (from the second tree)
    pub new_value: Option<InputValue>,
}

/// Result of comparing field values between two structurally equivalent trees
#[derive(Debug, Clone, Default)]
pub struct ValueDiffResult {
    /// List of fields whose values differ
    pub changes: Vec<FieldValueChange>,
}

impl ValueDiffResult {
    /// Create an empty value diff result
    pub fn empty() -> Self {
        Self::default()
    }

    /// Check if there are no value differences
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Merge another ValueDiffResult into this one
    pub fn merge(&mut self, other: ValueDiffResult) {
        self.changes.extend(other.changes);
    }
}

impl DiffResult {
    /// Create an empty diff result
    pub fn empty() -> Self {
        Self::default()
    }

    /// Create a result with a single added node
    pub fn added(node: StructuredNode) -> Self {
        Self {
            added: vec![node],
            removed: vec![],
        }
    }

    /// Create a result with a single removed node
    pub fn removed(node: StructuredNode) -> Self {
        Self {
            added: vec![],
            removed: vec![node],
        }
    }

    /// Merge another DiffResult into this one
    pub fn merge(&mut self, other: DiffResult) {
        self.added.extend(other.added);
        self.removed.extend(other.removed);
    }

    /// Check if there are no differences
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

// ============================================================================
// Structural equality implementations
// ============================================================================

/// Structural equality for InputValue (used in FieldCondition comparison)
pub fn input_value_structural_eq(a: &InputValue, b: &InputValue) -> bool {
    match (a, b) {
        (InputValue::Text(a), InputValue::Text(b)) => a == b,
        (InputValue::Number(a), InputValue::Number(b)) => a == b,
        (InputValue::Date(a), InputValue::Date(b)) => a == b,
        (InputValue::Email(a), InputValue::Email(b)) => a == b,
        (InputValue::Tel(a), InputValue::Tel(b)) => a == b,
        (InputValue::Checkbox(a), InputValue::Checkbox(b)) => a == b,
        (InputValue::Radio(a), InputValue::Radio(b)) => a == b,
        (InputValue::Select(a), InputValue::Select(b)) => a == b,
        _ => false,
    }
}

/// Structural equality for FieldType (compares variant and all constraint fields)
pub fn field_type_structural_eq(a: &FieldType, b: &FieldType) -> bool {
    match (a, b) {
        (
            FieldType::Text {
                regex: regex_a,
                max_length: max_a,
                min_length: min_a,
            },
            FieldType::Text {
                regex: regex_b,
                max_length: max_b,
                min_length: min_b,
            },
        ) => regex_a == regex_b && max_a == max_b && min_a == min_b,

        (
            FieldType::Number {
                min: min_a,
                max: max_a,
                step: step_a,
            },
            FieldType::Number {
                min: min_b,
                max: max_b,
                step: step_b,
            },
        ) => min_a == min_b && max_a == max_b && step_a == step_b,

        (FieldType::Date, FieldType::Date) => true,
        (FieldType::Email, FieldType::Email) => true,
        (FieldType::Tel, FieldType::Tel) => true,
        (FieldType::Checkbox, FieldType::Checkbox) => true,

        (FieldType::Radio { options: opts_a, .. }, FieldType::Radio { options: opts_b, .. }) => {
            opts_a == opts_b
        }

        (FieldType::Select { options: opts_a }, FieldType::Select { options: opts_b }) => {
            opts_a == opts_b
        }

        _ => false,
    }
}

/// Structural equality for FieldNode (compares name and input_type, ignores value)
pub fn field_node_structural_eq(a: &FieldNode, b: &FieldNode) -> bool {
    a.name == b.name && field_type_structural_eq(&a.input_type, &b.input_type)
}

/// Structural equality for HeadingNode (compares level and text content)
pub fn heading_node_structural_eq(a: &HeadingNode, b: &HeadingNode) -> bool {
    a.level.as_u8() == b.level.as_u8() && a.content.as_plain_text() == b.content.as_plain_text()
}

/// Structural equality for ParagraphNode (compares text content)
pub fn paragraph_node_structural_eq(a: &ParagraphNode, b: &ParagraphNode) -> bool {
    a.content.as_plain_text() == b.content.as_plain_text()
}

/// Structural equality for ImageNode (compares alt_text)
pub fn image_node_structural_eq(a: &ImageNode, b: &ImageNode) -> bool {
    a.alt_text == b.alt_text
}

/// Structural equality for RepeatableNode (compares occurrence constraints only, not item)
/// Item comparison is handled by the diff algorithm recursively
pub fn repeatable_node_structural_eq(a: &RepeatableNode, b: &RepeatableNode) -> bool {
    a.min_occurrences == b.min_occurrences && a.max_occurrences == b.max_occurrences
}

/// Structural equality for ConditionalNode (compares condition only, not content)
/// Content comparison is handled by the diff algorithm recursively
pub fn conditional_node_structural_eq(a: &ConditionalNode, b: &ConditionalNode) -> bool {
    a.condition.field_name == b.condition.field_name
        && input_value_structural_eq(&a.condition.value, &b.condition.value)
}

/// Structural equality for GroupNode
/// Groups are containers - they match if they're both groups (children handled by diff)
pub fn group_node_structural_eq(_a: &GroupNode, _b: &GroupNode) -> bool {
    // Groups match as containers; children are compared by the diff algorithm
    true
}

/// Structural equality for TableHeader (headers match if same cell count)
/// Cell contents are compared by the diff algorithm
pub fn table_header_structural_eq(a: &TableHeader, b: &TableHeader) -> bool {
    a.cells.len() == b.cells.len()
}

/// Structural equality for TableRow (rows match if same cell count)
/// Cell contents are compared by the diff algorithm
pub fn table_row_structural_eq(a: &TableRow, b: &TableRow) -> bool {
    a.cells.len() == b.cells.len()
}

/// Structural equality for TableNode (compares structure: caption, header presence, row count)
/// Cell contents are compared by the diff algorithm
pub fn table_node_structural_eq(a: &TableNode, b: &TableNode) -> bool {
    // Compare caption
    let caption_eq = match (&a.caption, &b.caption) {
        (Some(ca), Some(cb)) => ca.as_plain_text() == cb.as_plain_text(),
        (None, None) => true,
        _ => false,
    };
    if !caption_eq {
        return false;
    }

    // Compare header presence and cell count
    let header_eq = match (&a.header, &b.header) {
        (Some(ha), Some(hb)) => table_header_structural_eq(ha, hb),
        (None, None) => true,
        _ => false,
    };
    if !header_eq {
        return false;
    }

    // Compare row count
    if a.rows.len() != b.rows.len() {
        return false;
    }

    // Compare each row's cell count
    a.rows
        .iter()
        .zip(b.rows.iter())
        .all(|(ra, rb)| table_row_structural_eq(ra, rb))
}

/// Structural equality for StructuredNode (the main entry point for comparison)
pub fn structured_node_structural_eq(a: &StructuredNode, b: &StructuredNode) -> bool {
    match (a, b) {
        (StructuredNode::Field(a), StructuredNode::Field(b)) => field_node_structural_eq(a, b),
        (StructuredNode::Heading(a), StructuredNode::Heading(b)) => {
            heading_node_structural_eq(a, b)
        }
        (StructuredNode::Paragraph(a), StructuredNode::Paragraph(b)) => {
            paragraph_node_structural_eq(a, b)
        }
        (StructuredNode::Image(a), StructuredNode::Image(b)) => image_node_structural_eq(a, b),
        (StructuredNode::Table(a), StructuredNode::Table(b)) => table_node_structural_eq(a, b),
        (StructuredNode::Repeatable(a), StructuredNode::Repeatable(b)) => {
            repeatable_node_structural_eq(a, b)
        }
        (StructuredNode::Group(a), StructuredNode::Group(b)) => group_node_structural_eq(a, b),
        (StructuredNode::Conditional(a), StructuredNode::Conditional(b)) => {
            conditional_node_structural_eq(a, b)
        }
        (StructuredNode::Empty, StructuredNode::Empty) => true,
        _ => false,
    }
}

// ============================================================================
// LCS-based child diffing
// ============================================================================

/// Compute the Longest Common Subsequence table for two slices using structural equality
fn lcs_table(old: &[StructuredNode], new: &[StructuredNode]) -> Vec<Vec<usize>> {
    let m = old.len();
    let n = new.len();
    let mut table = vec![vec![0; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if structured_node_structural_eq(&old[i - 1], &new[j - 1]) {
                table[i][j] = table[i - 1][j - 1] + 1;
            } else {
                table[i][j] = table[i - 1][j].max(table[i][j - 1]);
            }
        }
    }

    table
}

/// Diff two child lists using LCS algorithm
/// Returns added/removed nodes and recursively diffs matched pairs
fn diff_children(old: &[StructuredNode], new: &[StructuredNode]) -> DiffResult {
    if old.is_empty() && new.is_empty() {
        return DiffResult::empty();
    }

    if old.is_empty() {
        return DiffResult {
            added: new.to_vec(),
            removed: vec![],
        };
    }

    if new.is_empty() {
        return DiffResult {
            added: vec![],
            removed: old.to_vec(),
        };
    }

    let table = lcs_table(old, new);
    let mut result = DiffResult::empty();

    // Backtrack through LCS table to find matches and differences
    let mut i = old.len();
    let mut j = new.len();
    let mut matched_old: Vec<bool> = vec![false; old.len()];
    let mut matched_new: Vec<bool> = vec![false; new.len()];
    let mut match_pairs: Vec<(usize, usize)> = Vec::new();

    while i > 0 && j > 0 {
        if structured_node_structural_eq(&old[i - 1], &new[j - 1]) {
            matched_old[i - 1] = true;
            matched_new[j - 1] = true;
            match_pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if table[i - 1][j] >= table[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }

    // Collect removed nodes (unmatched in old)
    for (idx, node) in old.iter().enumerate() {
        if !matched_old[idx] {
            result.removed.push(node.clone());
        }
    }

    // Collect added nodes (unmatched in new)
    for (idx, node) in new.iter().enumerate() {
        if !matched_new[idx] {
            result.added.push(node.clone());
        }
    }

    // Recurse into matched pairs to find nested differences
    for (old_idx, new_idx) in match_pairs {
        let nested = diff_node_children(&old[old_idx], &new[new_idx]);
        result.merge(nested);
    }

    result
}

/// Recursively diff the children of two structurally equal nodes
fn diff_node_children(old: &StructuredNode, new: &StructuredNode) -> DiffResult {
    match (old, new) {
        (StructuredNode::Group(old_g), StructuredNode::Group(new_g)) => {
            diff_children(&old_g.children, &new_g.children)
        }

        (StructuredNode::Table(old_t), StructuredNode::Table(new_t)) => {
            let mut result = DiffResult::empty();

            // Diff header cells if both have headers
            if let (Some(old_h), Some(new_h)) = (&old_t.header, &new_t.header) {
                result.merge(diff_children(&old_h.cells, &new_h.cells));
            }

            // Diff each row's cells
            for (old_row, new_row) in old_t.rows.iter().zip(new_t.rows.iter()) {
                result.merge(diff_children(&old_row.cells, &new_row.cells));
            }

            result
        }

        (StructuredNode::Repeatable(old_r), StructuredNode::Repeatable(new_r)) => {
            diff_node(&old_r.item, &new_r.item)
        }

        (StructuredNode::Conditional(old_c), StructuredNode::Conditional(new_c)) => {
            diff_node(&old_c.content, &new_c.content)
        }

        // Leaf nodes or mismatched types have no children to diff
        _ => DiffResult::empty(),
    }
}

// ============================================================================
// Main diff entry points
// ============================================================================

/// Diff two StructuredNode trees
///
/// If the nodes are not structurally equal at the root, returns the old as removed
/// and the new as added. Otherwise, recursively diffs children.
pub fn diff_node(old: &StructuredNode, new: &StructuredNode) -> DiffResult {
    if !structured_node_structural_eq(old, new) {
        return DiffResult {
            added: vec![new.clone()],
            removed: vec![old.clone()],
        };
    }

    // Nodes are structurally equal, recurse into children
    diff_node_children(old, new)
}

/// Main entry point: diff two StructuredNode trees
///
/// Returns a DiffResult containing all added and removed nodes.
pub fn diff_trees(old: &StructuredNode, new: &StructuredNode) -> DiffResult {
    diff_node(old, new)
}

// ============================================================================
// Value diff implementation
// ============================================================================

/// Compare values of two InputValue instances
fn values_equal(a: &Option<InputValue>, b: &Option<InputValue>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(va), Some(vb)) => input_value_structural_eq(va, vb),
        _ => false,
    }
}

/// Collect value changes from two child lists using LCS matching
/// Only compares values of fields that exist in both trees (structurally matched)
fn value_diff_children(old: &[StructuredNode], new: &[StructuredNode]) -> ValueDiffResult {
    if old.is_empty() || new.is_empty() {
        return ValueDiffResult::empty();
    }

    let table = lcs_table(old, new);
    let mut result = ValueDiffResult::empty();

    // Backtrack through LCS table to find matches
    let mut i = old.len();
    let mut j = new.len();
    let mut match_pairs: Vec<(usize, usize)> = Vec::new();

    while i > 0 && j > 0 {
        if structured_node_structural_eq(&old[i - 1], &new[j - 1]) {
            match_pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if table[i - 1][j] >= table[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }

    // Recurse into matched pairs to find value differences
    for (old_idx, new_idx) in match_pairs {
        let nested = value_diff_node(&old[old_idx], &new[new_idx]);
        result.merge(nested);
    }

    result
}

/// Recursively collect value changes from two structurally equal nodes
fn value_diff_node(old: &StructuredNode, new: &StructuredNode) -> ValueDiffResult {
    // Only process structurally equivalent nodes
    if !structured_node_structural_eq(old, new) {
        return ValueDiffResult::empty();
    }

    match (old, new) {
        (StructuredNode::Field(old_f), StructuredNode::Field(new_f)) => {
            let mut result = ValueDiffResult::empty();
            if !values_equal(&old_f.value, &new_f.value) {
                result.changes.push(FieldValueChange {
                    field_name: old_f.name.clone(),
                    old_value: old_f.value.clone(),
                    new_value: new_f.value.clone(),
                });
            }
            result
        }

        (StructuredNode::Group(old_g), StructuredNode::Group(new_g)) => {
            value_diff_children(&old_g.children, &new_g.children)
        }

        (StructuredNode::Table(old_t), StructuredNode::Table(new_t)) => {
            let mut result = ValueDiffResult::empty();

            // Diff header cells if both have headers
            if let (Some(old_h), Some(new_h)) = (&old_t.header, &new_t.header) {
                result.merge(value_diff_children(&old_h.cells, &new_h.cells));
            }

            // Diff each row's cells
            for (old_row, new_row) in old_t.rows.iter().zip(new_t.rows.iter()) {
                result.merge(value_diff_children(&old_row.cells, &new_row.cells));
            }

            result
        }

        (StructuredNode::Repeatable(old_r), StructuredNode::Repeatable(new_r)) => {
            value_diff_node(&old_r.item, &new_r.item)
        }

        (StructuredNode::Conditional(old_c), StructuredNode::Conditional(new_c)) => {
            value_diff_node(&old_c.content, &new_c.content)
        }

        // Leaf nodes without values (Heading, Paragraph, Image, Empty)
        _ => ValueDiffResult::empty(),
    }
}

/// Main entry point: compare field values between two StructuredNode trees
///
/// Returns a ValueDiffResult containing all fields whose values differ.
/// Only compares fields that exist in both trees (structurally matched).
/// Fields that only exist in one tree are ignored.
pub fn structured_value_diff(old: &StructuredNode, new: &StructuredNode) -> ValueDiffResult {
    value_diff_node(old, new)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::{HeadingLevel, InlineText};

    fn make_heading(level: u8, text: &str) -> StructuredNode {
        StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::from_u8(level),
            content: InlineText::plain(text),
        })
    }

    fn make_paragraph(text: &str) -> StructuredNode {
        StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain(text),
        })
    }

    fn make_field(name: &str) -> StructuredNode {
        StructuredNode::Field(FieldNode {
            name: name.to_string(),
            label: None,
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
        })
    }

    fn make_group(children: Vec<StructuredNode>) -> StructuredNode {
        StructuredNode::Group(GroupNode { children })
    }

    #[test]
    fn test_identical_trees() {
        let tree = make_group(vec![
            make_heading(1, "Title"),
            make_paragraph("Some text"),
            make_field("name"),
        ]);

        let result = diff_trees(&tree, &tree);
        assert!(result.is_empty());
    }

    #[test]
    fn test_added_node() {
        let old = make_group(vec![make_heading(1, "Title"), make_field("name")]);

        let new = make_group(vec![
            make_heading(1, "Title"),
            make_paragraph("New paragraph"),
            make_field("name"),
        ]);

        let result = diff_trees(&old, &new);
        assert_eq!(result.added.len(), 1);
        assert_eq!(result.removed.len(), 0);

        if let StructuredNode::Paragraph(p) = &result.added[0] {
            assert_eq!(p.content.as_plain_text(), "New paragraph");
        } else {
            panic!("Expected Paragraph node");
        }
    }

    #[test]
    fn test_removed_node() {
        let old = make_group(vec![
            make_heading(1, "Title"),
            make_paragraph("Old paragraph"),
            make_field("name"),
        ]);

        let new = make_group(vec![make_heading(1, "Title"), make_field("name")]);

        let result = diff_trees(&old, &new);
        assert_eq!(result.added.len(), 0);
        assert_eq!(result.removed.len(), 1);

        if let StructuredNode::Paragraph(p) = &result.removed[0] {
            assert_eq!(p.content.as_plain_text(), "Old paragraph");
        } else {
            panic!("Expected Paragraph node");
        }
    }

    #[test]
    fn test_field_value_ignored() {
        let old = StructuredNode::Field(FieldNode {
            name: "email".to_string(),
            label: None,
            input_type: FieldType::Email,
            value: Some(InputValue::Email("old@test.com".to_string())),
            placeholder: None,
        });

        let new = StructuredNode::Field(FieldNode {
            name: "email".to_string(),
            label: None,
            input_type: FieldType::Email,
            value: Some(InputValue::Email("new@test.com".to_string())),
            placeholder: None,
        });

        let result = diff_trees(&old, &new);
        assert!(result.is_empty(), "Field value changes should be ignored");
    }

    #[test]
    fn test_field_type_change_detected() {
        let old = StructuredNode::Field(FieldNode {
            name: "count".to_string(),
            label: None,
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
        });

        let new = StructuredNode::Field(FieldNode {
            name: "count".to_string(),
            label: None,
            input_type: FieldType::Number {
                min: None,
                max: None,
                step: None,
            },
            value: None,
            placeholder: None,
        });

        let result = diff_trees(&old, &new);
        assert_eq!(result.added.len(), 1);
        assert_eq!(result.removed.len(), 1);
    }

    #[test]
    fn test_radio_options_change_detected() {
        let old = StructuredNode::Field(FieldNode {
            name: "choice".to_string(),
            label: None,
            input_type: FieldType::Radio {
                options: vec!["A".to_string(), "B".to_string()],
                option_names: None,
            },
            value: None,
            placeholder: None,
        });

        let new = StructuredNode::Field(FieldNode {
            name: "choice".to_string(),
            label: None,
            input_type: FieldType::Radio {
                options: vec!["A".to_string(), "B".to_string(), "C".to_string()],
                option_names: None,
            },
            value: None,
            placeholder: None,
        });

        let result = diff_trees(&old, &new);
        assert_eq!(result.added.len(), 1);
        assert_eq!(result.removed.len(), 1);
    }

    #[test]
    fn test_nested_diff() {
        let old = make_group(vec![make_group(vec![
            make_heading(1, "Inner"),
            make_field("a"),
        ])]);

        let new = make_group(vec![make_group(vec![
            make_heading(1, "Inner"),
            make_field("a"),
            make_field("b"),
        ])]);

        let result = diff_trees(&old, &new);
        assert_eq!(result.added.len(), 1);
        assert_eq!(result.removed.len(), 0);

        if let StructuredNode::Field(f) = &result.added[0] {
            assert_eq!(f.name, "b");
        } else {
            panic!("Expected Field node");
        }
    }

    // ========================================================================
    // Value diff tests
    // ========================================================================

    fn make_field_with_value(name: &str, value: Option<InputValue>) -> StructuredNode {
        StructuredNode::Field(FieldNode {
            name: name.to_string(),
            label: None,
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value,
            placeholder: None,
        })
    }

    #[test]
    fn test_value_diff_identical_values() {
        let old = make_field_with_value("name", Some(InputValue::Text("John".to_string())));
        let new = make_field_with_value("name", Some(InputValue::Text("John".to_string())));

        let result = structured_value_diff(&old, &new);
        assert!(result.is_empty());
    }

    #[test]
    fn test_value_diff_changed_value() {
        let old = make_field_with_value("name", Some(InputValue::Text("John".to_string())));
        let new = make_field_with_value("name", Some(InputValue::Text("Jane".to_string())));

        let result = structured_value_diff(&old, &new);
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].field_name, "name");
        assert_eq!(
            result.changes[0].old_value,
            Some(InputValue::Text("John".to_string()))
        );
        assert_eq!(
            result.changes[0].new_value,
            Some(InputValue::Text("Jane".to_string()))
        );
    }

    #[test]
    fn test_value_diff_none_to_some() {
        let old = make_field_with_value("name", None);
        let new = make_field_with_value("name", Some(InputValue::Text("John".to_string())));

        let result = structured_value_diff(&old, &new);
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].field_name, "name");
        assert_eq!(result.changes[0].old_value, None);
        assert_eq!(
            result.changes[0].new_value,
            Some(InputValue::Text("John".to_string()))
        );
    }

    #[test]
    fn test_value_diff_some_to_none() {
        let old = make_field_with_value("name", Some(InputValue::Text("John".to_string())));
        let new = make_field_with_value("name", None);

        let result = structured_value_diff(&old, &new);
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].field_name, "name");
        assert_eq!(
            result.changes[0].old_value,
            Some(InputValue::Text("John".to_string()))
        );
        assert_eq!(result.changes[0].new_value, None);
    }

    #[test]
    fn test_value_diff_nested_in_group() {
        let old = make_group(vec![
            make_field_with_value("a", Some(InputValue::Text("old_a".to_string()))),
            make_field_with_value("b", Some(InputValue::Text("same".to_string()))),
        ]);
        let new = make_group(vec![
            make_field_with_value("a", Some(InputValue::Text("new_a".to_string()))),
            make_field_with_value("b", Some(InputValue::Text("same".to_string()))),
        ]);

        let result = structured_value_diff(&old, &new);
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].field_name, "a");
    }

    #[test]
    fn test_value_diff_ignores_added_field() {
        let old = make_group(vec![make_field_with_value(
            "a",
            Some(InputValue::Text("value".to_string())),
        )]);
        let new = make_group(vec![
            make_field_with_value("a", Some(InputValue::Text("value".to_string()))),
            make_field_with_value("b", Some(InputValue::Text("new_field".to_string()))),
        ]);

        let result = structured_value_diff(&old, &new);
        assert!(result.is_empty(), "Added fields should be ignored");
    }

    #[test]
    fn test_value_diff_ignores_removed_field() {
        let old = make_group(vec![
            make_field_with_value("a", Some(InputValue::Text("value".to_string()))),
            make_field_with_value("b", Some(InputValue::Text("removed_field".to_string()))),
        ]);
        let new = make_group(vec![make_field_with_value(
            "a",
            Some(InputValue::Text("value".to_string())),
        )]);

        let result = structured_value_diff(&old, &new);
        assert!(result.is_empty(), "Removed fields should be ignored");
    }

    #[test]
    fn test_value_diff_structurally_different_roots() {
        let old = make_heading(1, "Title");
        let new = make_paragraph("Different type");

        let result = structured_value_diff(&old, &new);
        assert!(
            result.is_empty(),
            "Structurally different trees should return empty"
        );
    }

    #[test]
    fn test_value_diff_multiple_changes() {
        let old = make_group(vec![
            make_field_with_value("a", Some(InputValue::Text("old_a".to_string()))),
            make_field_with_value("b", Some(InputValue::Text("old_b".to_string()))),
            make_field_with_value("c", Some(InputValue::Text("same".to_string()))),
        ]);
        let new = make_group(vec![
            make_field_with_value("a", Some(InputValue::Text("new_a".to_string()))),
            make_field_with_value("b", Some(InputValue::Text("new_b".to_string()))),
            make_field_with_value("c", Some(InputValue::Text("same".to_string()))),
        ]);

        let result = structured_value_diff(&old, &new);
        assert_eq!(result.changes.len(), 2);

        let field_names: Vec<&str> = result
            .changes
            .iter()
            .map(|c| c.field_name.as_str())
            .collect();
        assert!(field_names.contains(&"a"));
        assert!(field_names.contains(&"b"));
    }
}
