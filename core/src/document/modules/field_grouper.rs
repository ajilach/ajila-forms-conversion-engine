//! Field grouper module.
//!
//! Wraps each flattened field node in its own FieldGroup.
//! This provides a one-to-one mapping from field nodes to FieldGroup groups.
//! Only interactive (access="open") fields are marked as Fields.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};

/// Wraps each interactive field node in its own FieldGroup.
///
/// Creates a one-to-one mapping: each flattened field node that is interactive
/// (access="open") gets its own FieldGroup. Non-interactive fields (readOnly,
/// protected, nonInteractive) are skipped since they are not user-editable.
pub struct FieldGrouper;

impl Default for FieldGrouper {
    fn default() -> Self {
        Self::new()
    }
}

impl FieldGrouper {
    pub fn new() -> Self {
        FieldGrouper
    }

    /// Check if a field node is interactive based on its access level.
    fn is_interactive(doc: &Document, leaf_idx: usize) -> bool {
        doc.get_node(leaf_idx)
            .map(|node| node.is_interactive())
            .unwrap_or(true)
    }
}

impl AnalysisModule for FieldGrouper {
    fn name(&self) -> &'static str {
        "FieldGrouper"
    }

    fn process(&self, doc: &mut Document) {
        // Get all unclaimed field leaves
        let field_leaves = doc.unclaimed_field_leaves();

        // Wrap each interactive field leaf in its own FieldGroup
        for leaf_idx in field_leaves {
            // Skip non-interactive fields - they are not user-editable
            if !Self::is_interactive(doc, leaf_idx) {
                continue;
            }

            doc.merge(
                vec![leaf_idx],
                GroupKind::Field,
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }
}
