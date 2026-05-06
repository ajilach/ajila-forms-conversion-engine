//! Column layout detector module.
//!
//! Detects multi-column layouts (e.g., two side-by-side text columns) by
//! analyzing the horizontal distribution of root groups. When a clear
//! horizontal gap separates elements into two bands, the detector identifies
//! vertical sections where both columns have content and wraps each column's
//! elements into a separate composite group. This ensures downstream modules
//! and the structured converter process them sequentially (left column first,
//! then right column) rather than interleaving by y-position.
//!
//! The detector handles:
//! - Full-width elements (headings, signatures) that are excluded from columns
//! - Multiple column sections separated by page breaks or full-width elements
//! - Narrow overlay elements (width ≤ 10mm) that are discarded as NoPrint

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Minimum horizontal gap (in points) between columns to trigger detection.
const MIN_GAP_PT: &str = "10.0";

/// Minimum number of elements required in each column band per section.
const MIN_ELEMENTS_PER_COLUMN: usize = 5;

/// Minimum vertical extent (in points) that a section's columns must span.
const MIN_VERTICAL_EXTENT_PT: &str = "150.0";

/// Maximum element width relative to content width to be considered
/// a column element (elements wider than this are full-width).
const MAX_ELEMENT_WIDTH_RATIO: &str = "0.6";

/// Minimum element width relative to content width to be considered
/// a column element (elements narrower than this are excluded as
/// page furniture like footers, page numbers, etc.).
const MIN_ELEMENT_WIDTH_RATIO: &str = "0.15";

/// Maximum width (in points) for an element to be considered a narrow overlay.
/// Approximately 10mm ≈ 28.35pt.
const NARROW_OVERLAY_WIDTH_PT: &str = "28.35";

/// Minimum vertical gap (in points) between consecutive column elements
/// to consider it a section break. This should be larger than normal
/// paragraph spacing but catch page breaks.
const SECTION_GAP_THRESHOLD_PT: &str = "85.0";

/// A detected column section containing left and right element indices.
struct ColumnSection {
    left: Vec<usize>,
    right: Vec<usize>,
}

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

    /// Get the element-level bounds (container bounds) for a group,
    /// using the node's specified width/height rather than text content bounds.
    /// This is important for column detection because element positioning
    /// reflects the form layout, not text content length.
    fn get_element_bounds(doc: &Document, group_idx: usize) -> Option<Bounds> {
        let node_indices = doc.collect_node_indices(group_idx);
        if node_indices.is_empty() {
            return None;
        }

        let mut min_x = Decimal::MAX;
        let mut min_y = Decimal::MAX;
        let mut max_x = Decimal::MIN;
        let mut max_y = Decimal::MIN;

        for node_idx in node_indices {
            if let Some(node) = doc.source.iter_nodes().nth(node_idx) {
                let b = node.bounds();
                min_x = min_x.min(b.x);
                min_y = min_y.min(b.y);
                max_x = max_x.max(b.x + b.width);
                max_y = max_y.max(b.y + b.height);
            }
        }

        if min_x == Decimal::MAX {
            return None;
        }

        Some(Bounds::new(min_x, min_y, max_x - min_x, max_y - min_y))
    }

    /// Find the horizontal split point among column candidates.
    /// Returns the split x-coordinate if a clear gap is found.
    fn find_horizontal_split(candidates: &[(usize, Bounds)]) -> Option<Decimal> {
        let min_gap = Decimal::from_str(MIN_GAP_PT).unwrap();

        let mut by_center: Vec<(usize, Decimal)> = candidates
            .iter()
            .map(|(idx, b)| (*idx, b.center_x()))
            .collect();
        by_center.sort_by_key(|(_, cx)| *cx);

        let mut best_gap = Decimal::ZERO;
        let mut best_gap_idx = 0;

        for i in 1..by_center.len() {
            let gap = by_center[i].1 - by_center[i - 1].1;
            if gap > best_gap {
                best_gap = gap;
                best_gap_idx = i;
            }
        }

        if best_gap < min_gap || best_gap_idx == 0 {
            return None;
        }

        Some((by_center[best_gap_idx - 1].1 + by_center[best_gap_idx].1) / Decimal::TWO)
    }

    /// Detect column sections among the given root groups.
    /// Returns detected sections and overlay indices to discard.
    fn detect_column_sections(
        doc: &Document,
        roots: &[usize],
    ) -> Option<(Vec<ColumnSection>, Vec<usize>)> {
        let max_width_ratio = Decimal::from_str(MAX_ELEMENT_WIDTH_RATIO).unwrap();
        let min_width_ratio = Decimal::from_str(MIN_ELEMENT_WIDTH_RATIO).unwrap();
        let narrow_threshold = Decimal::from_str(NARROW_OVERLAY_WIDTH_PT).unwrap();
        let section_gap = Decimal::from_str(SECTION_GAP_THRESHOLD_PT).unwrap();
        let min_vertical_extent = Decimal::from_str(MIN_VERTICAL_EXTENT_PT).unwrap();

        // Collect roots with valid bounds (element-level bounds for layout detection)
        let bounded: Vec<(usize, Bounds)> = roots
            .iter()
            .filter_map(|&idx| Self::get_element_bounds(doc, idx).map(|b| (idx, b)))
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

        // Classify elements into categories
        let mut overlays: Vec<usize> = Vec::new();
        let mut full_width: Vec<(usize, Bounds)> = Vec::new();
        let mut column_candidates: Vec<(usize, Bounds)> = Vec::new();

        for (idx, bounds) in &bounded {
            if bounds.width <= narrow_threshold {
                overlays.push(*idx);
            } else if bounds.width > content_width * max_width_ratio {
                full_width.push((*idx, bounds.clone()));
            } else if bounds.width >= content_width * min_width_ratio {
                column_candidates.push((*idx, bounds.clone()));
            }
            // Elements between narrow and min_width_ratio are ignored
            // (page furniture like footers, timestamps, page numbers)
        }

        if column_candidates.len() < MIN_ELEMENTS_PER_COLUMN * 2 {
            return None;
        }

        // Find horizontal split
        let split_x = match Self::find_horizontal_split(&column_candidates) {
            Some(x) => x,
            None => return None,
        };

        // Verify the gap is clean: no column candidate spans across it
        for (_, bounds) in &column_candidates {
            let cx = bounds.center_x();
            let half_gap_region =
                (split_x - Decimal::from_str("5.0").unwrap())
                    ..=(split_x + Decimal::from_str("5.0").unwrap());
            if half_gap_region.contains(&cx) {
                // Element sits right on the split — not a clean column layout
                return None;
            }
        }

        // Partition into left and right
        let mut left_elements: Vec<(usize, Decimal)> = Vec::new();
        let mut right_elements: Vec<(usize, Decimal)> = Vec::new();

        for (idx, bounds) in &column_candidates {
            let y = bounds.top();
            if bounds.center_x() < split_x {
                left_elements.push((*idx, y));
            } else {
                right_elements.push((*idx, y));
            }
        }

        // Need minimum elements in each band overall
        if left_elements.len() < MIN_ELEMENTS_PER_COLUMN
            || right_elements.len() < MIN_ELEMENTS_PER_COLUMN
        {
            return None;
        }

        // Width consistency check: true column layouts have elements with
        // consistent widths (same column container), while form grids have
        // varying field widths. Require that the dominant width accounts for
        // at least 50% of elements in each band.
        let width_tolerance = Decimal::from_str("5.0").unwrap();
        if !Self::has_consistent_widths(&column_candidates, split_x, width_tolerance) {
            return None;
        }

        // Find section boundaries by looking at merged y-positions of all
        // column candidates. A section break occurs where there's a large
        // vertical gap with no column elements, or where a full-width element
        // interrupts.
        let mut all_ys: Vec<Decimal> = left_elements
            .iter()
            .chain(right_elements.iter())
            .map(|(_, y)| *y)
            .collect();
        all_ys.sort();
        all_ys.dedup();

        // Collect section break y-values (gaps in merged column elements)
        let mut break_ys: Vec<Decimal> = Vec::new();
        for i in 1..all_ys.len() {
            if all_ys[i] - all_ys[i - 1] > section_gap {
                // Midpoint of the gap
                break_ys.push((all_ys[i - 1] + all_ys[i]) / Decimal::TWO);
            }
        }

        // Also break at full-width elements that fall within the column range
        let col_min_y = all_ys.first().copied()?;
        let col_max_y = all_ys.last().copied()?;
        for (_, bounds) in &full_width {
            let fw_y = bounds.top();
            if fw_y > col_min_y && fw_y < col_max_y {
                break_ys.push(fw_y);
            }
        }

        break_ys.sort();
        break_ys.dedup();

        // Build sections from breaks
        let mut section_bounds: Vec<(Decimal, Decimal)> = Vec::new();
        let mut prev_y = col_min_y;
        for br in &break_ys {
            section_bounds.push((prev_y, *br));
            prev_y = *br;
        }
        section_bounds.push((prev_y, col_max_y + Decimal::ONE));

        // For each section range, collect left/right elements
        let mut sections: Vec<ColumnSection> = Vec::new();

        for (start_y, end_y) in &section_bounds {
            let section_left: Vec<usize> = left_elements
                .iter()
                .filter(|(_, y)| y >= start_y && y < end_y)
                .map(|(idx, _)| *idx)
                .collect();

            let section_right: Vec<usize> = right_elements
                .iter()
                .filter(|(_, y)| y >= start_y && y < end_y)
                .map(|(idx, _)| *idx)
                .collect();

            // Only create a section if both columns have enough elements
            if section_left.len() >= MIN_ELEMENTS_PER_COLUMN
                && section_right.len() >= MIN_ELEMENTS_PER_COLUMN
            {
                // Check vertical extent
                let left_extent = Self::vertical_extent(doc, &section_left);
                let right_extent = Self::vertical_extent(doc, &section_right);

                if left_extent >= min_vertical_extent && right_extent >= min_vertical_extent {
                    sections.push(ColumnSection {
                        left: section_left,
                        right: section_right,
                    });
                }
            }
        }

        if sections.is_empty() {
            return None;
        }

        // Collect overlays that are true column-number overlays:
        // They must be narrow, overlap vertically with a section, AND
        // share the same x-position as a column's left edge (indicating
        // they're numbering overlays for that column, not regular content).
        // Additionally, they must span a significant portion of the section height.
        let relevant_overlays: Vec<usize> = overlays
            .into_iter()
            .filter(|&idx| {
                let Some(b) = Self::get_element_bounds(doc, idx) else {
                    return false;
                };

                sections.iter().any(|section| {
                    // Get section vertical extent
                    let section_min_y = section
                        .left
                        .iter()
                        .chain(section.right.iter())
                        .filter_map(|&i| Self::get_element_bounds(doc, i).map(|b| b.top()))
                        .min();
                    let section_max_y = section
                        .left
                        .iter()
                        .chain(section.right.iter())
                        .filter_map(|&i| Self::get_element_bounds(doc, i).map(|b| b.bottom()))
                        .max();

                    let (Some(min_y), Some(max_y)) = (section_min_y, section_max_y) else {
                        return false;
                    };

                    // Must overlap vertically with the section
                    if b.top() >= max_y || b.bottom() <= min_y {
                        return false;
                    }

                    // Must span at least 30% of the section height
                    // (true number overlays span most/all of the column)
                    let section_height = max_y - min_y;
                    let overlay_height = b.height;
                    if section_height <= Decimal::ZERO
                        || overlay_height < section_height * Decimal::from_str("0.3").unwrap()
                    {
                        return false;
                    }

                    // Must align with the left edge of either column
                    let left_x = section
                        .left
                        .iter()
                        .filter_map(|&i| Self::get_element_bounds(doc, i).map(|b| b.left()))
                        .min();
                    let right_x = section
                        .right
                        .iter()
                        .filter_map(|&i| Self::get_element_bounds(doc, i).map(|b| b.left()))
                        .min();

                    let x_tolerance = Decimal::from_str("5.0").unwrap();
                    let aligned = match (left_x, right_x) {
                        (Some(lx), Some(rx)) => {
                            (b.left() - lx).abs() <= x_tolerance
                                || (b.left() - rx).abs() <= x_tolerance
                        }
                        (Some(lx), None) => (b.left() - lx).abs() <= x_tolerance,
                        (None, Some(rx)) => (b.left() - rx).abs() <= x_tolerance,
                        (None, None) => false,
                    };

                    aligned
                })
            })
            .collect();

        Some((sections, relevant_overlays))
    }

    /// Compute the vertical extent (max_bottom - min_top) of a set of groups.
    fn vertical_extent(doc: &Document, indices: &[usize]) -> Decimal {
        let bounds: Vec<Bounds> = indices
            .iter()
            .filter_map(|&idx| Self::get_element_bounds(doc, idx))
            .collect();

        if bounds.is_empty() {
            return Decimal::ZERO;
        }

        let min_top = bounds.iter().map(|b| b.top()).min().unwrap();
        let max_bottom = bounds.iter().map(|b| b.bottom()).max().unwrap();

        max_bottom - min_top
    }

    /// Check that both column bands have consistent element widths.
    /// Returns true if in each band, the most common width accounts for
    /// at least 50% of elements (indicating a uniform column container
    /// rather than a form grid with varying field widths).
    fn has_consistent_widths(
        candidates: &[(usize, Bounds)],
        split_x: Decimal,
        tolerance: Decimal,
    ) -> bool {
        let left_widths: Vec<Decimal> = candidates
            .iter()
            .filter(|(_, b)| b.center_x() < split_x)
            .map(|(_, b)| b.width)
            .collect();

        let right_widths: Vec<Decimal> = candidates
            .iter()
            .filter(|(_, b)| b.center_x() >= split_x)
            .map(|(_, b)| b.width)
            .collect();

        Self::dominant_width_ratio(&left_widths, tolerance) >= Decimal::from_str("0.5").unwrap()
            && Self::dominant_width_ratio(&right_widths, tolerance)
                >= Decimal::from_str("0.5").unwrap()
    }

    /// Compute what fraction of widths match the most common width (±tolerance).
    fn dominant_width_ratio(widths: &[Decimal], tolerance: Decimal) -> Decimal {
        if widths.is_empty() {
            return Decimal::ZERO;
        }

        let mut best_count = 0usize;
        for w in widths {
            let count = widths
                .iter()
                .filter(|other| (*other - w).abs() <= tolerance)
                .count();
            if count > best_count {
                best_count = count;
            }
        }

        Decimal::from(best_count as u64) / Decimal::from(widths.len() as u64)
    }
}

impl AnalysisModule for ColumnLayoutDetector {
    fn name(&self) -> &'static str {
        "ColumnLayoutDetector"
    }

    fn process(&self, doc: &mut Document) {
        let roots = doc.roots();

        let Some((sections, overlays)) = Self::detect_column_sections(doc, &roots) else {
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

        // Process each section: wrap left and right elements into groups
        for section in sections {
            // Sort left column by y (top-to-bottom)
            let mut left_sorted: Vec<(usize, Decimal)> = section
                .left
                .iter()
                .filter_map(|&idx| Self::get_element_bounds(doc, idx).map(|b| (idx, b.y)))
                .collect();
            left_sorted.sort_by_key(|(_, y)| *y);
            let left_indices: Vec<usize> = left_sorted.into_iter().map(|(idx, _)| idx).collect();

            // Sort right column by y (top-to-bottom)
            let mut right_sorted: Vec<(usize, Decimal)> = section
                .right
                .iter()
                .filter_map(|&idx| Self::get_element_bounds(doc, idx).map(|b| (idx, b.y)))
                .collect();
            right_sorted.sort_by_key(|(_, y)| *y);
            let right_indices: Vec<usize> =
                right_sorted.into_iter().map(|(idx, _)| idx).collect();

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
        let nodes = vec![
            // Left column elements (need 5+ per column, span > 150pt)
            text_node(0.0, 10.0, 230.0, 14.0, "Left heading"),
            text_node(0.0, 30.0, 230.0, 14.0, "Left paragraph 1"),
            text_node(0.0, 50.0, 230.0, 14.0, "Left paragraph 2"),
            text_node(0.0, 70.0, 230.0, 14.0, "Left paragraph 3"),
            text_node(0.0, 90.0, 230.0, 14.0, "Left paragraph 4"),
            text_node(0.0, 110.0, 230.0, 14.0, "Left paragraph 5"),
            text_node(0.0, 130.0, 230.0, 14.0, "Left paragraph 6"),
            text_node(0.0, 150.0, 230.0, 14.0, "Left paragraph 7"),
            text_node(0.0, 170.0, 230.0, 14.0, "Left paragraph 8"),
            // Right column elements
            text_node(255.0, 10.0, 230.0, 14.0, "Right heading"),
            text_node(255.0, 30.0, 230.0, 14.0, "Right paragraph 1"),
            text_node(255.0, 50.0, 230.0, 14.0, "Right paragraph 2"),
            text_node(255.0, 70.0, 230.0, 14.0, "Right paragraph 3"),
            text_node(255.0, 90.0, 230.0, 14.0, "Right paragraph 4"),
            text_node(255.0, 110.0, 230.0, 14.0, "Right paragraph 5"),
            text_node(255.0, 130.0, 230.0, 14.0, "Right paragraph 6"),
            text_node(255.0, 150.0, 230.0, 14.0, "Right paragraph 7"),
            text_node(255.0, 170.0, 230.0, 14.0, "Right paragraph 8"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        ColumnLayoutDetector::new().process(&mut doc);

        let roots = doc.roots();
        assert_eq!(roots.len(), 2, "Expected 2 column groups, got {}", roots.len());

        let left_bounds = doc.get_bounds(roots[0]).unwrap();
        let right_bounds = doc.get_bounds(roots[1]).unwrap();
        assert!(left_bounds.center_x() < right_bounds.center_x());
    }

    #[test]
    fn test_no_columns_in_single_column_layout() {
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

        assert_eq!(roots_before, roots_after);
    }

    #[test]
    fn test_narrow_overlays_discarded() {
        let nodes = vec![
            // Narrow overlays (≤10mm ≈ 28.35pt) aligned with column left edges
            text_node(30.0, 10.0, 25.0, 170.0, "1. 2. 3. 4."),
            text_node(280.0, 10.0, 25.0, 170.0, "5. 6. 7."),
            // Left column elements (5+ for min threshold, consistent width)
            text_node(30.0, 10.0, 200.0, 14.0, "Left heading line one"),
            text_node(30.0, 40.0, 200.0, 14.0, "Left para 1 content"),
            text_node(30.0, 70.0, 200.0, 14.0, "Left para 2 content"),
            text_node(30.0, 100.0, 200.0, 14.0, "Left para 3 content"),
            text_node(30.0, 130.0, 200.0, 14.0, "Left para 4 content"),
            text_node(30.0, 160.0, 200.0, 14.0, "Left para 5 content"),
            // Right column elements
            text_node(280.0, 10.0, 200.0, 14.0, "Right heading content"),
            text_node(280.0, 40.0, 200.0, 14.0, "Right para 1 content"),
            text_node(280.0, 70.0, 200.0, 14.0, "Right para 2 content"),
            text_node(280.0, 100.0, 200.0, 14.0, "Right para 3 content"),
            text_node(280.0, 130.0, 200.0, 14.0, "Right para 4 content"),
            text_node(280.0, 160.0, 200.0, 14.0, "Right para 5 content"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        ColumnLayoutDetector::new().process(&mut doc);

        let roots = doc.roots();
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
    fn test_full_width_elements_excluded_from_columns() {
        // Full-width heading at top, then two columns below
        let nodes = vec![
            // Full-width heading (should remain independent)
            text_node(0.0, 10.0, 480.0, 20.0, "Full-width heading"),
            // Left column (starts after heading, 5+ elements, span > 150pt)
            text_node(0.0, 50.0, 220.0, 14.0, "Left column text 1"),
            text_node(0.0, 80.0, 220.0, 14.0, "Left column text 2"),
            text_node(0.0, 110.0, 220.0, 14.0, "Left column text 3"),
            text_node(0.0, 140.0, 220.0, 14.0, "Left column text 4"),
            text_node(0.0, 170.0, 220.0, 14.0, "Left column text 5"),
            text_node(0.0, 200.0, 220.0, 14.0, "Left column text 6"),
            // Right column (starts after heading)
            text_node(260.0, 50.0, 220.0, 14.0, "Right column text 1"),
            text_node(260.0, 80.0, 220.0, 14.0, "Right column text 2"),
            text_node(260.0, 110.0, 220.0, 14.0, "Right column text 3"),
            text_node(260.0, 140.0, 220.0, 14.0, "Right column text 4"),
            text_node(260.0, 170.0, 220.0, 14.0, "Right column text 5"),
            text_node(260.0, 200.0, 220.0, 14.0, "Right column text 6"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        ColumnLayoutDetector::new().process(&mut doc);

        let roots = doc.roots();
        let unknown_count = roots
            .iter()
            .filter(|&&idx| matches!(doc.get_group(idx).unwrap().kind, GroupKind::Unknown))
            .count();
        assert_eq!(unknown_count, 2, "Expected 2 column groups");
        assert_eq!(roots.len(), 3, "Expected 3 roots: 1 full-width + 2 columns");
    }

    #[test]
    fn test_multiple_column_sections() {
        // Two separate column sections separated by a large vertical gap (>85pt)
        let nodes = vec![
            // Section 1: y=10 to y=170 (span=160 > 150pt)
            text_node(0.0, 10.0, 200.0, 14.0, "S1 Left text 1 content"),
            text_node(0.0, 40.0, 200.0, 14.0, "S1 Left text 2 content"),
            text_node(0.0, 70.0, 200.0, 14.0, "S1 Left text 3 content"),
            text_node(0.0, 100.0, 200.0, 14.0, "S1 Left text 4 content"),
            text_node(0.0, 130.0, 200.0, 14.0, "S1 Left text 5 content"),
            text_node(0.0, 170.0, 200.0, 14.0, "S1 Left text 6 content"),
            text_node(260.0, 10.0, 200.0, 14.0, "S1 Right text 1 content"),
            text_node(260.0, 40.0, 200.0, 14.0, "S1 Right text 2 content"),
            text_node(260.0, 70.0, 200.0, 14.0, "S1 Right text 3 content"),
            text_node(260.0, 100.0, 200.0, 14.0, "S1 Right text 4 content"),
            text_node(260.0, 130.0, 200.0, 14.0, "S1 Right text 5 content"),
            text_node(260.0, 170.0, 200.0, 14.0, "S1 Right text 6 content"),
            // Section 2: y=300 to y=460 (gap of 130pt from section 1)
            text_node(0.0, 300.0, 200.0, 14.0, "S2 Left text 1 content"),
            text_node(0.0, 330.0, 200.0, 14.0, "S2 Left text 2 content"),
            text_node(0.0, 360.0, 200.0, 14.0, "S2 Left text 3 content"),
            text_node(0.0, 390.0, 200.0, 14.0, "S2 Left text 4 content"),
            text_node(0.0, 420.0, 200.0, 14.0, "S2 Left text 5 content"),
            text_node(0.0, 460.0, 200.0, 14.0, "S2 Left text 6 content"),
            text_node(260.0, 300.0, 200.0, 14.0, "S2 Right text 1 content"),
            text_node(260.0, 330.0, 200.0, 14.0, "S2 Right text 2 content"),
            text_node(260.0, 360.0, 200.0, 14.0, "S2 Right text 3 content"),
            text_node(260.0, 390.0, 200.0, 14.0, "S2 Right text 4 content"),
            text_node(260.0, 420.0, 200.0, 14.0, "S2 Right text 5 content"),
            text_node(260.0, 460.0, 200.0, 14.0, "S2 Right text 6 content"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        ColumnLayoutDetector::new().process(&mut doc);

        let roots = doc.roots();
        let unknown_count = roots
            .iter()
            .filter(|&&idx| matches!(doc.get_group(idx).unwrap().kind, GroupKind::Unknown))
            .count();
        // 2 sections × 2 columns = 4 column groups
        assert_eq!(unknown_count, 4, "Expected 4 column groups (2 sections × 2)");
    }

    #[test]
    fn test_columns_with_different_y_start_same_section() {
        // Columns that start at different y but are within the same section
        // (gap < 85pt threshold) — should still detect as one section
        let nodes = vec![
            text_node(0.0, 50.0, 200.0, 14.0, "Left column text 1 here"),
            text_node(0.0, 80.0, 200.0, 14.0, "Left column text 2 here"),
            text_node(0.0, 110.0, 200.0, 14.0, "Left column text 3 here"),
            text_node(0.0, 140.0, 200.0, 14.0, "Left column text 4 here"),
            text_node(0.0, 170.0, 200.0, 14.0, "Left column text 5 here"),
            text_node(0.0, 210.0, 200.0, 14.0, "Left column text 6 here"),
            // Right column starts 30pt lower (within threshold)
            text_node(300.0, 80.0, 200.0, 14.0, "Right column text 1 here"),
            text_node(300.0, 110.0, 200.0, 14.0, "Right column text 2 here"),
            text_node(300.0, 140.0, 200.0, 14.0, "Right column text 3 here"),
            text_node(300.0, 170.0, 200.0, 14.0, "Right column text 4 here"),
            text_node(300.0, 200.0, 200.0, 14.0, "Right column text 5 here"),
            text_node(300.0, 230.0, 200.0, 14.0, "Right column text 6 here"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        ColumnLayoutDetector::new().process(&mut doc);

        let roots = doc.roots();
        let unknown_count = roots
            .iter()
            .filter(|&&idx| matches!(doc.get_group(idx).unwrap().kind, GroupKind::Unknown))
            .count();
        assert_eq!(unknown_count, 2, "Expected 2 column groups");
    }
}
