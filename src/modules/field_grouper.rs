//! Field grouper module.
//!
//! Wraps each flattened field node in its own FieldGroup.
//! This provides a one-to-one mapping from field nodes to FieldGroup groups.

use crate::document::{Document, GroupKind, GroupSource};
use super::AnalysisModule;

/// Wraps each field node in its own FieldGroup.
///
/// Creates a one-to-one mapping: each flattened field node gets its own FieldGroup.
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
}

impl AnalysisModule for FieldGrouper {
    fn name(&self) -> &'static str {
        "FieldGrouper"
    }
    
    fn process(&self, doc: &mut Document) {
        // Get all unclaimed field leaves
        let field_leaves = doc.unclaimed_field_leaves();
        
        // Wrap each field leaf in its own FieldGroup
        for leaf_idx in field_leaves {
            doc.merge(
                vec![leaf_idx],
                GroupKind::Field,
                GroupSource::Inferred { module: self.name().to_string() },
            );
        }
    }
}
