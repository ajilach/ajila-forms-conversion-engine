//! Column layout detector module.
//!
//! Detects multi-column layouts (e.g., two side-by-side text columns) by
//! analyzing the horizontal distribution of root groups. When a clear
//! horizontal gap separates elements into two bands, the detector wraps each
//! column's elements into a separate composite group so that downstream
//! modules and the structured converter process them sequentially (left
//! column first, then right column) rather than interleaving by y-position.
//!
//! Additionally, narrow overlay elements (width ≤ 10mm) that sit alongside
//! wider column content are discarded (claimed as NoPrint) since they
//! typically contain redundant numbering.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Minimum horizontal gap (in points) between columns to trigger detection.
const MIN_GAP_PT: &str = "10.0";

/// Minimum number of elements required in each column band.
const MIN_ELEMENTS_PER_COLUMN: usize = 3;

/// Minimum vertical extent (in points) that a column's elements must span.
const MIN_VERTICAL_EXTENT_PT: &str = "50.0";

/// Maximum element width relative to content width to be considered
/// a single-column element (elements wider than this are skipped).
const MAX_ELEMENT_WIDTH_RATIO: &str = "0.6";

/// Maximum width (in points) for an element to be considered a narrow overlay.
/// Approximately 10mm ≈ 28.35pt.
const NARROW_OVERLAY_WIDTH_PT: &str = "28.35";

/// Detects multi-column layouts and reorders elements so that left-column
/// content precedes right-column content in the output.
pub struct ColumnLayoutDetector;

impl Default for ColumnLayoutDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ColumnLayoutDetector {
    pub fn new() -> Self {
        ColumnLayoutDetector
    }

    /// Detect and return column assignments for the given root groups.
    ///
    /// Returns `Some((left_indices, right_indices, overlay_indices))` if a
    /// multi-column layout is detected, `None` otherwise.
    fn detect_columns(
        doc: &Document,
        roots: &[usize],
    ) -> Option<(Vec<usize>, Vec<usize>, Vec<usize>)> {
        let min_gap = Decimal::from_str(MIN_GAP_PT).unwrap();
        let min_vertical_extent = Decimal::from_str(MIN_VERTICAL_EXTENT_PT).unwrap();
        let max_width_ratio = Decimal::from_str(MAX_ELEMENT_WIDTH_RATIO).unwrap();
        let narrow_threshold = Decimal::from_str(NARROW_OVERLAY_WIDTH_PT).unwrap();

        // Collect roots with valid bounds
        let bounded: Vec<(usize, Bounds)> = roots
            .iter()
            .filter_map(|&idx| doc.get_bounds(idx).map(|b| (idx, b)))
            .collect();

        if bounded.len() < MIN_ELEMENTS_PER_COLUMN * 2 {
            return None;
        }

        // Compute content extent
        let min_left = bounded.iter().map(|(_, b)| b.left()).min()?;
        let max_right = bounded.iter().map(|(_, b)| b.right()).max()?;
        let content_width = max_right - min_left;

        if content_width <= Decimal::ZERO {
            return None;
        }

        // Separate narrow overlays from regular elements
        let mut overlays: Vec<usize> = Vec::new();
        let mut regular: Vec<(usize, Bounds)> = Vec::new();

        for (idx, bounds) in &bounded {
            if bounds.width <= narrow_threshold {
                overlays.push(*idx);
            } else {
                regular.push((*idx, bounds.clone()));
            }
        }

        // Filter out elements that span too much of the content width
        let column_candidates: Vec<(usize, Bounds)> = regular
            .iter()
            .filter(|(_, b)| b.width <= content_width * max_width_ratio)
            .cloned()
            .collect();

        if column_candidates.len() < MIN_ELEMENTS_PER_COLUMN * 2 {
            return None;
        }

        // Sort candidates by x-center to find the gap
        let mut by_center: Vec<(usize, Bounds, Decimal)> = column_candidates
            .iter()
            .map(|(idx, b)| (*idx, b.clone(), b.center_x()))
            .collect();
        by_center.sort_by_key(|(_, _, cx)| *cx);

        // Find the largest gap between consecutive x-centers
        let mut best_gap = Decimal::ZERO;
        let mut best_gap_idx = 0;

        for i in 1..by_center.len() {
            let gap = by_center[i].2 - by_center[i - 1].2;
            if gap > best_gap {
                best_gap = gap;
                best_gap_idx = i;
            }
        }

        if best_gap < min_gap {
            return None;
        }

        // Split at the gap
        let split_x = (by_center[best_gap_idx - 1].2 + by_center[best_gap_idx].2) / Decimal::TWO;

        // Classify elements into left and right bands
        let mut left: Vec<usize> = Vec::new();
        let mut right: Vec<usize> = Vec::new();

        for (idx, _bounds, cx) in &by_center {
            if *cx < split_x {
                left.push(*idx);
            } else {
                right.push(*idx);
            }
        }

        // Check minimum element count in each band
        if left.len() < MIN_ELEMENTS_PER_COLUMN || right.len() < MIN_ELEMENTS_PER_COLUMN {
            return None;
        }

        // Check vertical extent of each band
        let left_extent = Self::vertical_extent(doc, &left);
        let right_extent = Self::vertical_extent(doc, &right);

        if left_extent < min_vertical_extent || right_extent < min_vertical_extent {
            return None;
        }

        // Require that both columns start at approximately the same y-coordinate.
        // The structured converter sorts groups by y-then-x (with a 2pt threshold).
        // If one column starts significantly higher, the converter might sort
        // it before the other regardless of x-position.
        let left_min_y = left
            .iter()
            .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.top()))
            .min();
        let right_min_y = right
            .iter()
            .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.top()))
            .min();
        if let (Some(ly), Some(ry)) = (left_min_y, right_min_y) {
            // Allow up to ~5mm (≈14pt) difference in starting y
            let max_y_diff = Decimal::from_str("14.0").unwrap();
            if (ly - ry).abs() > max_y_diff {
                return None;
            }
        }

        // Verify that the gap is clean: no element's bounding box spans across
        // the gap midpoint (left edge < split_x < right edge with significant overlap)
        let gap_left = by_center[best_gap_idx - 1].2;
        let gap_right = by_center[best_gap_idx].2;
        for (_, bounds) in &bounded {
            if bounds.left() < gap_left && bounds.right() > gap_right {
                // This element spans the gap — likely not a true column layout
                // Only flag if it's not a narrow overlay
                if bounds.width > narrow_threshold {
                    return None;
                }
            }
        }

        // Also classify regular elements that were filtered out (too wide) — don't include them
        // They'll remain as separate roots and sort naturally

        // Only include narrow overlays that actually overlap with column content
        let relevant_overlays: Vec<usize> = overlays
            .into_iter()
            .filter(|&idx| {
                if let Some(b) = doc.get_bounds(idx) {
                    // Only discard if it overlaps vertically with column content
                    let min_y = by_center.iter().map(|(_, b, _)| b.top()).min();
                    let max_y = by_center.iter().map(|(_, b, _)| b.bottom()).max();
                    if let (Some(min), Some(max)) = (min_y, max_y) {
                        b.top() < max && b.bottom() > min
                    } else {
                        false
                    }
                } else {
                    false
                }
            })
            .collect();

        Some((left, right, relevant_overlays))
    }

    /// Compute the vertical extent (max_bottom - min_top) of a set of groups.
    fn vertical_extent(doc: &Document, indices: &[usize]) -> Decimal {
        let bounds: Vec<Bounds> = indices
            .iter()
            .filter_map(|&idx| doc.get_bounds(idx))
            .collect();

        if bounds.is_empty() {
            return Decimal::ZERO;
        }

        let min_top = bounds.iter().map(|b| b.top()).min().unwrap();
        let max_bottom = bounds.iter().map(|b| b.bottom()).max().unwrap();

        max_bottom - min_top
    }
}

impl AnalysisModule for ColumnLayoutDetector {
    fn name(&self) -> &'static str {
        "ColumnLayoutDetector"
    }

    fn process(&self, doc: &mut Document) {
        let roots = doc.roots();

        let Some((left, right, overlays)) = Self::detect_columns(doc, &roots) else {
            return;
        };

        // Claim narrow overlays as NoPrint (discard them)
        for idx in overlays {
            doc.merge(
                vec![idx],
                GroupKind::NoPrint,
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }

        // Sort left column by y (top-to-bottom)
        let mut left_sorted: Vec<(usize, Decimal)> = left
            .iter()
            .filter_map(|&idx| doc.get_bounds(idx).map(|b| (idx, b.y)))
            .collect();
        left_sorted.sort_by_key(|(_, y)| *y);
        let left_indices: Vec<usize> = left_sorted.into_iter().map(|(idx, _)| idx).collect();

        // Sort right column by y (top-to-bottom)
        let mut right_sorted: Vec<(usize, Decimal)> = right
            .iter()
            .filter_map(|&idx| doc.get_bounds(idx).map(|b| (idx, b.y)))
            .collect();
        right_sorted.sort_by_key(|(_, y)| *y);
        let right_indices: Vec<usize> = right_sorted.into_iter().map(|(idx, _)| idx).collect();

        // Wrap each column into a composite group
        if !left_indices.is_empty() {
            doc.merge(
                left_indices,
                GroupKind::Unknown,
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }

        if !right_indices.is_empty() {
            doc.merge(
                right_indices,
                GroupKind::Unknown,
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::flattened::{Flattened, FlattenedKind, FlattenedNode, FlattenedNodeKind, Page};
    use crate::xfa::num;

    /// Helper to create a text node at a given position.
    fn text_node(x: f64, y: f64, w: f64, h: f64, content: &str) -> FlattenedNode {
        FlattenedNode {
            kind: FlattenedNodeKind::Text {
                content: content.to_string(),
                font_size: num(10.0),
                font_name: "Arial".to_string(),
                source_name: None,
            },
            x: num(x),
            y: num(y),
            width: num(w),
            height: num(h),
            rotate: 0,
            style: Default::default(),
            hints: vec![],
            no_wrap: false,
        }
    }

    /// Create a Flattened with given nodes.
    fn make_flattened(nodes: Vec<FlattenedNode>) -> Flattened {
        let children: Vec<FlattenedKind> = nodes.into_iter().map(FlattenedKind::Node).collect();
        Flattened {
            page: Page {
                width: num(595.0),
                height: num(842.0),
            },
            children,
            language: "de".to_string(),
            cached_key: None,
        }
    }

    #[test]
    fn test_detects_two_column_layout() {
        // Left column: x=0..85mm (0..240pt), center ~120pt
        // Right column: x=90mm..175mm (255..496pt), center ~375pt
        // Gap between 240pt and 255pt
        let nodes = vec![
            // Left column elements
            text_node(0.0, 10.0, 230.0, 14.0, "Left heading"),
            text_node(0.0, 30.0, 230.0, 14.0, "Left paragraph 1"),
            text_node(0.0, 50.0, 230.0, 14.0, "Left paragraph 2"),
            text_node(0.0, 70.0, 230.0, 14.0, "Left paragraph 3"),
            // Right column elements
            text_node(255.0, 10.0, 230.0, 14.0, "Right heading"),
            text_node(255.0, 30.0, 230.0, 14.0, "Right paragraph 1"),
            text_node(255.0, 50.0, 230.0, 14.0, "Right paragraph 2"),
            text_node(255.0, 70.0, 230.0, 14.0, "Right paragraph 3"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        ColumnLayoutDetector::new().process(&mut doc);

        // Should have created two composite groups
        let roots = doc.roots();
        assert_eq!(roots.len(), 2, "Expected 2 column groups, got {}", roots.len());

        // Left column group should come first (lower x)
        let left_group = doc.get_group(roots[0]).unwrap();
        let right_group = doc.get_group(roots[1]).unwrap();

        // Verify they're composite groups
        assert!(matches!(left_group.kind, GroupKind::Unknown));
        assert!(matches!(right_group.kind, GroupKind::Unknown));

        // Verify left group has 4 children and right group has 4 children
        assert_eq!(left_group.children.len(), 4);
        assert_eq!(right_group.children.len(), 4);

        // Verify left group bounds are to the left of right group bounds
        let left_bounds = doc.get_bounds(roots[0]).unwrap();
        let right_bounds = doc.get_bounds(roots[1]).unwrap();
        assert!(left_bounds.center_x() < right_bounds.center_x());
    }

    #[test]
    fn test_no_columns_in_single_column_layout() {
        // All elements in a single vertical flow
        let nodes = vec![
            text_node(20.0, 10.0, 400.0, 14.0, "Heading"),
            text_node(20.0, 30.0, 400.0, 14.0, "Paragraph 1"),
            text_node(20.0, 50.0, 400.0, 14.0, "Paragraph 2"),
            text_node(20.0, 70.0, 400.0, 14.0, "Paragraph 3"),
            text_node(20.0, 90.0, 400.0, 14.0, "Paragraph 4"),
            text_node(20.0, 110.0, 400.0, 14.0, "Paragraph 5"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        let roots_before = doc.roots().len();
        ColumnLayoutDetector::new().process(&mut doc);
        let roots_after = doc.roots().len();

        // Should not have created any composite groups
        assert_eq!(roots_before, roots_after);
    }

    #[test]
    fn test_narrow_overlays_discarded() {
        // Two columns with narrow overlay elements
        let nodes = vec![
            // Narrow overlays (≤10mm ≈ 28.35pt)
            text_node(0.0, 10.0, 25.0, 80.0, "1. 2. 3. 4."),
            text_node(255.0, 10.0, 25.0, 80.0, "5. 6. 7."),
            // Left column elements
            text_node(30.0, 10.0, 200.0, 14.0, "Left heading"),
            text_node(30.0, 30.0, 200.0, 14.0, "Left para 1"),
            text_node(30.0, 50.0, 200.0, 14.0, "Left para 2"),
            text_node(30.0, 70.0, 200.0, 14.0, "Left para 3"),
            // Right column elements
            text_node(280.0, 10.0, 200.0, 14.0, "Right heading"),
            text_node(280.0, 30.0, 200.0, 14.0, "Right para 1"),
            text_node(280.0, 50.0, 200.0, 14.0, "Right para 2"),
            text_node(280.0, 70.0, 200.0, 14.0, "Right para 3"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        ColumnLayoutDetector::new().process(&mut doc);

        let roots = doc.roots();
        // Should have: 2 column groups + 2 NoPrint groups for overlays
        let noprint_count = roots
            .iter()
            .filter(|&&idx| matches!(doc.get_group(idx).unwrap().kind, GroupKind::NoPrint))
            .count();
        let unknown_count = roots
            .iter()
            .filter(|&&idx| matches!(doc.get_group(idx).unwrap().kind, GroupKind::Unknown))
            .count();

        assert_eq!(noprint_count, 2, "Expected 2 NoPrint groups for overlays");
        assert_eq!(unknown_count, 2, "Expected 2 Unknown groups for columns");
    }

    #[test]
    fn test_not_enough_elements_per_column() {
        // Only 1 element on the right — should not detect columns
        let nodes = vec![
            text_node(0.0, 10.0, 200.0, 14.0, "Left 1"),
            text_node(0.0, 30.0, 200.0, 14.0, "Left 2"),
            text_node(0.0, 50.0, 200.0, 14.0, "Left 3"),
            text_node(0.0, 70.0, 200.0, 14.0, "Left 4"),
            text_node(300.0, 10.0, 200.0, 14.0, "Right 1"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        let roots_before = doc.roots().len();
        ColumnLayoutDetector::new().process(&mut doc);
        let roots_after = doc.roots().len();

        assert_eq!(roots_before, roots_after);
    }

    #[test]
    fn test_insufficient_vertical_extent() {
        // Elements on both sides but all on the same line (no vertical extent)
        let nodes = vec![
            text_node(0.0, 10.0, 100.0, 14.0, "Left 1"),
            text_node(0.0, 12.0, 100.0, 14.0, "Left 2"),
            text_node(0.0, 14.0, 100.0, 14.0, "Left 3"),
            text_node(300.0, 10.0, 100.0, 14.0, "Right 1"),
            text_node(300.0, 12.0, 100.0, 14.0, "Right 2"),
            text_node(300.0, 14.0, 100.0, 14.0, "Right 3"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        let roots_before = doc.roots().len();
        ColumnLayoutDetector::new().process(&mut doc);
        let roots_after = doc.roots().len();

        assert_eq!(roots_before, roots_after);
    }

    #[test]
    fn test_rejects_columns_with_different_start_y() {
        // Two bands separated horizontally, but right column starts much higher.
        // Should NOT detect columns since the y-difference exceeds the threshold.
        let nodes = vec![
            // Left column starts at y=50
            text_node(0.0, 50.0, 200.0, 14.0, "Left 1"),
            text_node(0.0, 70.0, 200.0, 14.0, "Left 2"),
            text_node(0.0, 90.0, 200.0, 14.0, "Left 3"),
            text_node(0.0, 110.0, 200.0, 14.0, "Left 4"),
            // Right column starts at y=10 (40pt difference > 14pt threshold)
            text_node(300.0, 10.0, 200.0, 14.0, "Right 1"),
            text_node(300.0, 30.0, 200.0, 14.0, "Right 2"),
            text_node(300.0, 50.0, 200.0, 14.0, "Right 3"),
            text_node(300.0, 70.0, 200.0, 14.0, "Right 4"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        let roots_before = doc.roots().len();
        ColumnLayoutDetector::new().process(&mut doc);
        let roots_after = doc.roots().len();

        assert_eq!(roots_before, roots_after);
    }
}
