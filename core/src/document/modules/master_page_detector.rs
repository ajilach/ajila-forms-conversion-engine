//! Master page detector module.
//!
//! Detects header, footer, and background groups based on the MasterPage hint.
//! Master page content is content that appears on the page background (outside
//! the contentArea) in XFA forms, or is repeated across multiple pages in
//! AcroForm PDFs — typically headers, footers, or background decorations.
//!
//! For multi-page merged documents, this module creates **separate** Header/Footer/Background
//! groups per page by clustering spatially adjacent master-page nodes. Nodes on
//! different pages are far apart vertically (separated by at least a page height),
//! so a simple gap-based clustering splits them correctly.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::MasterPageRegion;

/// Detects header, footer, and background groups based on MasterPage hints.
///
/// Creates one `GroupKind::Header`, one `GroupKind::Footer`, and one `GroupKind::Background`
/// group **per page** by clustering master-page leaf nodes that are spatially adjacent.
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
        doc.master_page_region(group_idx)
    }

    /// Find all leaf groups with a specific MasterPage region.
    fn find_groups_by_region(&self, doc: &Document, target_region: MasterPageRegion) -> Vec<usize> {
        let mut groups = Vec::new();

        for (idx, group) in doc.groups.iter().enumerate() {
            // Only process leaf groups
            if let GroupKind::Leaf { .. } = &group.kind
                && let Some(region) = self.get_master_page_region(doc, idx)
                && region == target_region
            {
                groups.push(idx);
            }
        }

        groups
    }

    /// Split a list of group indices into clusters of spatially adjacent groups.
    ///
    /// Groups are sorted by their Y coordinate, then split whenever the vertical
    /// gap between consecutive groups exceeds `max_gap`. This separates nodes
    /// from different pages in a merged multi-page document.
    fn cluster_by_vertical_proximity(
        &self,
        doc: &Document,
        group_indices: Vec<usize>,
        max_gap: rust_decimal::Decimal,
    ) -> Vec<Vec<usize>> {
        if group_indices.is_empty() {
            return Vec::new();
        }

        // Pair each index with its Y coordinate
        let mut with_y: Vec<(usize, rust_decimal::Decimal)> = group_indices
            .into_iter()
            .filter_map(|idx| doc.get_bounds(idx).map(|b| (idx, b.y)))
            .collect();

        // Sort by Y
        with_y.sort_by(|a, b| a.1.cmp(&b.1));

        // Split into clusters at large gaps
        let mut clusters: Vec<Vec<usize>> = Vec::new();
        let mut current_cluster: Vec<usize> = vec![with_y[0].0];
        let mut prev_bottom = {
            let b = doc.get_bounds(with_y[0].0).unwrap();
            b.y + b.height
        };

        for &(idx, y) in &with_y[1..] {
            if y - prev_bottom > max_gap {
                clusters.push(std::mem::take(&mut current_cluster));
            }
            current_cluster.push(idx);
            let bottom = doc.get_bounds(idx).map(|b| b.y + b.height).unwrap_or(y);
            if bottom > prev_bottom {
                prev_bottom = bottom;
            }
        }
        if !current_cluster.is_empty() {
            clusters.push(current_cluster);
        }

        clusters
    }
}

impl AnalysisModule for MasterPageDetector {
    fn process(&self, doc: &mut Document) {
        // A gap larger than this means nodes are on different pages.
        // Typical page height is ~842pt (A4); a gap of 200pt is well below
        // that but large enough to never occur within a single header/footer
        // region (which is typically 30-80pt tall).
        let max_gap = rust_decimal::Decimal::from(200);

        // Find all header region groups and cluster per page
        let header_groups = self.find_groups_by_region(doc, MasterPageRegion::Header);
        let header_clusters = self.cluster_by_vertical_proximity(doc, header_groups, max_gap);

        for cluster in header_clusters {
            if !cluster.is_empty() {
                doc.merge(
                    cluster,
                    GroupKind::Header,
                    GroupSource::Inferred {
                        module: self.name().to_string(),
                    },
                );
            }
        }

        // Find all footer region groups and cluster per page
        let footer_groups = self.find_groups_by_region(doc, MasterPageRegion::Footer);
        let footer_clusters = self.cluster_by_vertical_proximity(doc, footer_groups, max_gap);

        for cluster in footer_clusters {
            if !cluster.is_empty() {
                doc.merge(
                    cluster,
                    GroupKind::Footer,
                    GroupSource::Inferred {
                        module: self.name().to_string(),
                    },
                );
            }
        }

        // Find all background region groups and cluster per page
        let bg_groups = self.find_groups_by_region(doc, MasterPageRegion::Background);
        let bg_clusters = self.cluster_by_vertical_proximity(doc, bg_groups, max_gap);

        for cluster in bg_clusters {
            if !cluster.is_empty() {
                doc.merge(
                    cluster,
                    GroupKind::Background,
                    GroupSource::Inferred {
                        module: self.name().to_string(),
                    },
                );
            }
        }
    }

    fn name(&self) -> &'static str {
        "MasterPageDetector"
    }
}
