//! Master page detector module.
//!
//! Detects header and footer groups based on the MasterPage hint.
//! Master page content is content that appears on the page background (outside
//! the contentArea) in XFA forms - typically headers, footers, and background elements.
//!
//! This module examines MasterPage hints on leaf nodes and creates Header/Footer
//! groups for contiguous master page content.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::{Hint, MasterPageRegion};

/// Detects header and footer groups based on MasterPage hints.
///
/// Creates GroupKind::Header and GroupKind::Footer groups for leaf nodes
/// that have MasterPage hints with Header or Footer regions respectively.
pub struct MasterPageDetector;

impl Default for MasterPageDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl MasterPageDetector {
    pub fn new() -> Self {
        MasterPageDetector
    }

    /// Get the MasterPage hint region from a leaf group's node.
    fn get_master_page_region(&self, doc: &Document, group_idx: usize) -> Option<MasterPageRegion> {
        let group = doc.groups.get(group_idx)?;
        if let GroupKind::Leaf { node_index } = &group.kind
            && let Some(node) = doc.get_node(*node_index) {
                for hint in &node.hints {
                    if let Hint::MasterPage { region } = hint {
                        return Some(*region);
                    }
                }
            }
        None
    }

    /// Find all leaf groups with a specific MasterPage region.
    fn find_groups_by_region(&self, doc: &Document, target_region: MasterPageRegion) -> Vec<usize> {
        let mut groups = Vec::new();

        for (idx, group) in doc.groups.iter().enumerate() {
            // Only process leaf groups
            if let GroupKind::Leaf { .. } = &group.kind
                && let Some(region) = self.get_master_page_region(doc, idx)
                    && region == target_region {
                        groups.push(idx);
                    }
        }

        groups
    }
}

impl AnalysisModule for MasterPageDetector {
    fn process(&self, doc: &mut Document) {
        // Find all header region groups
        let header_groups = self.find_groups_by_region(doc, MasterPageRegion::Header);

        // Find all footer region groups
        let footer_groups = self.find_groups_by_region(doc, MasterPageRegion::Footer);

        // Create a Header group containing all header leaf groups
        if !header_groups.is_empty() {
            doc.merge(
                header_groups,
                GroupKind::Header,
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }

        // Create a Footer group containing all footer leaf groups
        if !footer_groups.is_empty() {
            doc.merge(
                footer_groups,
                GroupKind::Footer,
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }

        // Note: Background region groups are not explicitly grouped.
        // They remain as leaf groups or get picked up by other analysis modules.
    }

    fn name(&self) -> &'static str {
        "MasterPageDetector"
    }
}
