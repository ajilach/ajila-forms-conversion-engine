//! Multi-column detector module.
//!
//! Detects spatially separated columns of non-interactive content (TextBlock,
//! Heading, List, Paragraph) and groups them into `MultiColumn` groups.
//!
//! Only non-interactive elements (those that contain no field nodes) are
//! eligible.  Elements that are too wide to belong to a single column (wider
//! than `max_element_width_fraction` of the page width) are also excluded so
//! that full-width headings and decorative text draws do not interfere.
//!
//! # Detection algorithm
//!
//! 1. Collect eligible root groups with their spatial bounds.
//! 2. Cluster groups into x-bands by sweeping from left to right: a new
//!    cluster starts whenever the gap between the current element's left edge
//!    and the right extent of the current cluster exceeds `min_column_gap`.
//! 3. Discard clusters with fewer than `min_items_per_column` elements.
//! 4. Build multi-column sets by grouping adjacent clusters whose y-ranges
//!    overlap (they run alongside each other on the page).
//! 5. For each such set with ≥ 2 columns, create a `MultiColumn` group whose
//!    children are stored in column-major order (column 0 first, then
//!    column 1, …).  Within each column the elements are sorted by y.

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Minimum physical gap (points) between the right edge of one cluster and
/// the left edge of the next for them to be considered separate columns.
const MIN_COLUMN_GAP: f64 = 15.0;

/// Minimum number of elements that a cluster must contain to be treated as
/// an independent column.
const MIN_ITEMS_PER_COLUMN: usize = 2;

/// Elements wider than this fraction of the page width are excluded from
/// column detection (they are full-width content such as headings that span
/// all columns).
const MAX_ELEMENT_WIDTH_FRACTION: f64 = 0.6;

/// Detects multi-column layouts of non-interactive content.
pub struct MultiColumnDetector {
    /// Minimum physical gap (points) between columns.
    pub min_column_gap: Decimal,
    /// Minimum number of elements required in each column.
    pub min_items_per_column: usize,
    /// Elements wider than this fraction of the page width are excluded.
    pub max_element_width_fraction: Decimal,
}

impl Default for MultiColumnDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiColumnDetector {
    pub fn new() -> Self {
        MultiColumnDetector {
            min_column_gap: Decimal::from_f64(MIN_COLUMN_GAP)
                .unwrap_or(Decimal::from(15)),
            min_items_per_column: MIN_ITEMS_PER_COLUMN,
            max_element_width_fraction: Decimal::from_f64(MAX_ELEMENT_WIDTH_FRACTION)
                .unwrap_or(Decimal::from_str("0.6").unwrap()),
        }
    }

    // -------------------------------------------------------------------------
    // Eligibility
    // -------------------------------------------------------------------------

    /// Return `true` if `group_idx` may participate in a multi-column layout.
    ///
    /// Eligible groups are non-interactive content kinds (TextBlock, Heading,
    /// List, Paragraph) that contain no interactive field nodes.
    fn is_eligible(&self, doc: &Document, group_idx: usize) -> bool {
        let Some(group) = doc.get_group(group_idx) else {
            return false;
        };
        let is_content_kind = matches!(
            &group.kind,
            GroupKind::TextBlock
                | GroupKind::Heading { .. }
                | GroupKind::List { .. }
                | GroupKind::Paragraph
        );
        is_content_kind && !doc.contains_field(group_idx)
    }

    // -------------------------------------------------------------------------
    // Spatial clustering helpers
    // -------------------------------------------------------------------------

    /// Cluster candidate groups into x-bands using a left-to-right sweep.
    ///
    /// Groups are sorted by their left edge (`x`).  A new cluster is started
    /// whenever the gap between the current element's `x` and the right extent
    /// accumulated so far in the current cluster exceeds `min_column_gap`.
    ///
    /// Returns clusters ordered left-to-right, each containing the group
    /// indices and bounds of its members.
    fn cluster_by_x_extent<'a>(
        &self,
        candidates: &'a [(usize, Bounds)],
    ) -> Vec<Vec<(usize, &'a Bounds)>> {
        if candidates.is_empty() {
            return vec![];
        }

        // Sort by left edge
        let mut sorted: Vec<(usize, &Bounds)> =
            candidates.iter().map(|(idx, b)| (*idx, b)).collect();
        sorted.sort_by(|a, b| a.1.x.cmp(&b.1.x));

        let mut clusters: Vec<Vec<(usize, &Bounds)>> = Vec::new();
        let mut current: Vec<(usize, &Bounds)> = Vec::new();
        // Track the maximum right extent of the current cluster
        let mut cluster_right = sorted[0].1.right();

        for &(idx, bounds) in &sorted {
            let gap = bounds.x - cluster_right;
            if !current.is_empty() && gap > self.min_column_gap {
                // Gap large enough → start a new cluster
                clusters.push(current);
                current = Vec::new();
                cluster_right = bounds.right();
            } else {
                // Extend the right extent of the current cluster
                cluster_right = cluster_right.max(bounds.right());
            }
            current.push((idx, bounds));
        }
        if !current.is_empty() {
            clusters.push(current);
        }

        clusters
    }

    /// Return `true` if two clusters share any vertical extent.
    fn clusters_overlap_y(cluster_a: &[(usize, &Bounds)], cluster_b: &[(usize, &Bounds)]) -> bool {
        let min_y_a = cluster_a.iter().map(|(_, b)| b.y).min().unwrap_or(Decimal::MAX);
        let max_y_a = cluster_a
            .iter()
            .map(|(_, b)| b.bottom())
            .max()
            .unwrap_or(Decimal::MIN);
        let min_y_b = cluster_b.iter().map(|(_, b)| b.y).min().unwrap_or(Decimal::MAX);
        let max_y_b = cluster_b
            .iter()
            .map(|(_, b)| b.bottom())
            .max()
            .unwrap_or(Decimal::MIN);
        min_y_a <= max_y_b && min_y_b <= max_y_a
    }
}

impl AnalysisModule for MultiColumnDetector {
    fn name(&self) -> &'static str {
        "MultiColumnDetector"
    }

    fn process(&self, doc: &mut Document) {
        let page_width = doc.source.page.width;
        let max_width = page_width * self.max_element_width_fraction;

        // ── Step 1: collect eligible root groups ─────────────────────────────
        let candidates: Vec<(usize, Bounds)> = doc
            .roots()
            .into_iter()
            .filter(|&idx| self.is_eligible(doc, idx))
            .filter_map(|idx| doc.get_bounds(idx).map(|b| (idx, b)))
            // Exclude full-width elements that span most of the page
            .filter(|(_, b)| b.width <= max_width)
            .collect();

        if candidates.len() < self.min_items_per_column * 2 {
            return;
        }

        // ── Step 2: cluster into x-bands ──────────────────────────────────────
        let clusters = self.cluster_by_x_extent(&candidates);

        if clusters.len() < 2 {
            return;
        }

        // ── Step 3: keep only clusters with enough items ──────────────────────
        let valid_clusters: Vec<&Vec<(usize, &Bounds)>> = clusters
            .iter()
            .filter(|c| c.len() >= self.min_items_per_column)
            .collect();

        if valid_clusters.len() < 2 {
            return;
        }

        // ── Step 4: group adjacent valid clusters by y-range overlap ──────────
        let mut multi_column_groups: Vec<Vec<&Vec<(usize, &Bounds)>>> = Vec::new();
        let mut current_group: Vec<&Vec<(usize, &Bounds)>> = vec![valid_clusters[0]];

        for i in 1..valid_clusters.len() {
            let prev = valid_clusters[i - 1];
            let curr = valid_clusters[i];
            if Self::clusters_overlap_y(prev, curr) {
                current_group.push(curr);
            } else {
                if current_group.len() >= 2 {
                    multi_column_groups.push(std::mem::take(&mut current_group));
                } else {
                    current_group.clear();
                }
                current_group.push(curr);
            }
        }
        if current_group.len() >= 2 {
            multi_column_groups.push(current_group);
        }

        if multi_column_groups.is_empty() {
            return;
        }

        // ── Step 5: create MultiColumn groups ────────────────────────────────
        for column_group in multi_column_groups {
            let num_columns = column_group.len();
            let mut column_sizes: Vec<usize> = Vec::with_capacity(num_columns);
            let mut all_children: Vec<usize> = Vec::new();

            for cluster in &column_group {
                // Sort each column's elements by y (top-to-bottom reading order)
                let mut col_items: Vec<(usize, &Bounds)> = cluster.to_vec();
                col_items.sort_by(|a, b| a.1.y.cmp(&b.1.y));

                column_sizes.push(col_items.len());
                all_children.extend(col_items.iter().map(|(idx, _)| *idx));
            }

            doc.merge_inferred(
                all_children,
                GroupKind::MultiColumn {
                    num_columns,
                    column_sizes,
                },
                self.name(),
            );
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::modules::{HeadingDetector, TextBlockGrouper, TextBlockMerger};
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;

    fn make_text(content: &str, x: f64, y: f64, w: f64, h: f64) -> FlattenedNode {
        FlattenedNode::new_text(
            content.to_string(),
            num(10.0),
            "Helvetica".to_string(),
            num(x),
            num(y),
            num(w),
            num(h),
        )
    }

    /// Build a 2-column document: 3 text nodes in left column, 3 in right column.
    ///
    /// ```text
    /// Left col (x=50–200)    Right col (x=300–450)
    /// "Left A"  y=100        "Right A" y=105
    /// "Left B"  y=120        "Right B" y=125
    /// "Left C"  y=140        "Right C" y=145
    /// ```
    fn two_column_flattened() -> Flattened {
        Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                make_text("Left A", 50.0, 100.0, 150.0, 14.0),
                make_text("Right A", 300.0, 105.0, 150.0, 14.0),
                make_text("Left B", 50.0, 120.0, 150.0, 14.0),
                make_text("Right B", 300.0, 125.0, 150.0, 14.0),
                make_text("Left C", 50.0, 140.0, 150.0, 14.0),
                make_text("Right C", 300.0, 145.0, 150.0, 14.0),
            ],
        )
    }

    #[test]
    fn test_two_column_layout_detected() {
        let flattened = two_column_flattened();
        let mut doc = Document::from_flattened(&flattened);

        TextBlockGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);
        MultiColumnDetector::new().process(&mut doc);

        let mc_groups: Vec<usize> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::MultiColumn { .. })
                )
            })
            .collect();

        assert_eq!(mc_groups.len(), 1, "Should detect exactly one MultiColumn group");

        let mc_idx = mc_groups[0];
        if let Some(GroupKind::MultiColumn {
            num_columns,
            column_sizes,
        }) = doc.get_group(mc_idx).map(|g| &g.kind)
        {
            assert_eq!(*num_columns, 2, "Should have 2 columns");
            assert_eq!(column_sizes.len(), 2, "Should have sizes for 2 columns");
            assert_eq!(column_sizes[0] + column_sizes[1], doc.get_group(mc_idx).unwrap().children.len());
        } else {
            panic!("Expected MultiColumn group");
        }
    }

    #[test]
    fn test_single_column_not_detected() {
        // All text in the same x range → no multi-column
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                make_text("Line 1", 50.0, 100.0, 200.0, 14.0),
                make_text("Line 2", 50.0, 120.0, 200.0, 14.0),
                make_text("Line 3", 50.0, 140.0, 200.0, 14.0),
                make_text("Line 4", 50.0, 160.0, 200.0, 14.0),
            ],
        );
        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);
        MultiColumnDetector::new().process(&mut doc);

        let mc_groups: Vec<usize> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::MultiColumn { .. })
                )
            })
            .collect();

        assert!(mc_groups.is_empty(), "Single column layout should not be detected");
    }

    #[test]
    fn test_columns_without_y_overlap_not_merged() {
        // Two groups of text with the same x separation but at completely
        // different vertical positions (no y overlap) — not a multi-column layout.
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Left area, top of page
                make_text("Top L1", 50.0, 50.0, 150.0, 14.0),
                make_text("Top L2", 50.0, 70.0, 150.0, 14.0),
                // Right area, bottom of page (completely different y range)
                make_text("Bot R1", 300.0, 600.0, 150.0, 14.0),
                make_text("Bot R2", 300.0, 620.0, 150.0, 14.0),
            ],
        );
        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);
        MultiColumnDetector::new().process(&mut doc);

        let mc_groups: Vec<usize> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::MultiColumn { .. })
                )
            })
            .collect();

        assert!(
            mc_groups.is_empty(),
            "Vertically separated groups with no y-overlap should not be merged"
        );
    }

    #[test]
    fn test_full_width_element_excluded_from_column_detection() {
        // A heading that spans almost the full page width must not prevent
        // the two narrow columns below it from being detected.
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Full-width heading at the top (width = 495pt ≈ 83% of page)
                make_text("Full Width Heading", 50.0, 50.0, 495.0, 14.0),
                // Two columns below
                make_text("Left A", 50.0, 100.0, 150.0, 14.0),
                make_text("Right A", 300.0, 105.0, 150.0, 14.0),
                make_text("Left B", 50.0, 120.0, 150.0, 14.0),
                make_text("Right B", 300.0, 125.0, 150.0, 14.0),
                make_text("Left C", 50.0, 140.0, 150.0, 14.0),
                make_text("Right C", 300.0, 145.0, 150.0, 14.0),
            ],
        );
        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);
        MultiColumnDetector::new().process(&mut doc);

        let mc_groups: Vec<usize> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::MultiColumn { .. })
                )
            })
            .collect();

        assert_eq!(
            mc_groups.len(),
            1,
            "Should detect the two narrow columns despite the full-width heading"
        );
    }

    #[test]
    fn test_column_children_in_column_major_y_order() {
        // Verify that children in the detected MultiColumn group are stored
        // in column-major order and each column is sorted by y.
        let flattened = two_column_flattened();
        let mut doc = Document::from_flattened(&flattened);

        TextBlockGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);
        MultiColumnDetector::new().process(&mut doc);

        let mc_idx = doc
            .roots()
            .into_iter()
            .find(|&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::MultiColumn { .. })
                )
            })
            .expect("MultiColumn group should exist");

        let (num_columns, column_sizes) =
            if let Some(GroupKind::MultiColumn {
                num_columns,
                column_sizes,
            }) = doc.get_group(mc_idx).map(|g| &g.kind)
            {
                (*num_columns, column_sizes.clone())
            } else {
                panic!("Expected MultiColumn");
            };

        assert_eq!(num_columns, 2);
        let children = doc.get_group(mc_idx).unwrap().children.clone();

        // Check that the first column's text comes before the second column's text
        // Column 0: children[0..column_sizes[0]]
        // Column 1: children[column_sizes[0]..]
        let col0_size = column_sizes[0];
        for i in 0..col0_size {
            let text = doc.get_text_content(children[i]);
            assert!(
                text.starts_with("Left"),
                "Column 0 child {i} should be left-column text, got: {text}"
            );
        }
        for i in col0_size..children.len() {
            let text = doc.get_text_content(children[i]);
            assert!(
                text.starts_with("Right"),
                "Column 1 child {i} should be right-column text, got: {text}"
            );
        }
    }

    #[test]
    fn test_interactive_content_not_eligible() {
        use crate::document::modules::{FieldGrouper, RadioButtonDetector};
        use crate::flattened::FlattenedNode;

        // A mix of text and a field: the field should prevent the column from
        // being eligible, but a column of pure text alongside it is still fine.
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Left column: 3 pure text nodes
                make_text("Left A", 50.0, 100.0, 150.0, 14.0),
                make_text("Left B", 50.0, 120.0, 150.0, 14.0),
                make_text("Left C", 50.0, 140.0, 150.0, 14.0),
                // Right column: only a single text node (< min_items)
                make_text("Right A", 300.0, 100.0, 150.0, 14.0),
                // Interactive field in right area
                FlattenedNode::new_field(
                    "MyField".to_string(),
                    "".to_string(),
                    "My Field".to_string(),
                    num(300.0),
                    num(125.0),
                    num(150.0),
                    num(20.0),
                ),
            ],
        );
        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);
        MultiColumnDetector::new().process(&mut doc);

        let mc_groups: Vec<usize> = doc
            .roots()
            .into_iter()
            .filter(|&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::MultiColumn { .. })
                )
            })
            .collect();

        // The right side doesn't have enough non-interactive items to form a column
        assert!(
            mc_groups.is_empty(),
            "Should not detect multi-column when right side has too few non-interactive elements"
        );
    }
}
