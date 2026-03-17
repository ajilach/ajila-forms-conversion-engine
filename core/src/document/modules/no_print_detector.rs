//! No-print detector module.
//!
//! Detects and claims elements that have the NoPrint hint (relevant="-print").
//! These elements are screen-only interactive elements like add/remove buttons
//! that should not appear in the structured output.
//!
//! This module MUST run first in the analysis pipeline so that NoPrint elements
//! are claimed before other modules can process them.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::Hint;

/// Detects and claims elements with the NoPrint hint.
///
/// Creates GroupKind::NoPrint groups for leaf nodes that have the NoPrint hint.
/// By claiming these elements early, they won't be processed by other modules
/// like LabelAttacher or FieldGrouper.
pub struct NoPrintDetector;

impl Default for NoPrintDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl NoPrintDetector {
    pub fn new() -> Self {
        NoPrintDetector
    }

    /// Check if a leaf group has the NoPrint hint.
    fn has_no_print_hint(&self, doc: &Document, group_idx: usize) -> bool {
        let group = doc.groups.get(group_idx);
        if let Some(group) = group
            && let GroupKind::Leaf { node_index } = &group.kind
            && let Some(node) = doc.get_node(*node_index)
        {
            return node.hints.iter().any(|h| matches!(h, Hint::NoPrint));
        }
        false
    }

    /// Find all leaf groups with the NoPrint hint.
    fn find_no_print_groups(&self, doc: &Document) -> Vec<usize> {
        let mut groups = Vec::new();

        for (idx, group) in doc.groups.iter().enumerate() {
            // Only process leaf groups
            if let GroupKind::Leaf { .. } = &group.kind
                && self.has_no_print_hint(doc, idx)
            {
                groups.push(idx);
            }
        }

        groups
    }
}

impl AnalysisModule for NoPrintDetector {
    fn process(&self, doc: &mut Document) {
        // Find all NoPrint groups
        let no_print_groups = self.find_no_print_groups(doc);

        // Create individual NoPrint wrapper groups for each element
        // This claims them so other modules won't process them
        for group_idx in no_print_groups {
            doc.merge(
                vec![group_idx],
                GroupKind::NoPrint,
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }

    fn name(&self) -> &'static str {
        "NoPrintDetector"
    }
}
