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

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Minimum horizontal gap (in points) between columns to trigger detection.
const MIN_GAP_PT: &str = "10.0";

/// Minimum number of elements required in each column band (overall).
const MIN_ELEMENTS_PER_COLUMN: usize = 5;

/// Maximum element width relative to content width to be considered
/// a column element (elements wider than this are full-width).
const MAX_ELEMENT_WIDTH_RATIO: &str = "0.6";

/// Minimum element width relative to content width to be considered
/// a column element (elements narrower than this are excluded as
/// page furniture like footers, page numbers, etc.).
const MIN_ELEMENT_WIDTH_RATIO: &str = "0.15";

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
    /// Public accessor for tests
    #[cfg(test)]
    pub fn get_element_bounds_static(doc: &Document, group_idx: usize) -> Option<Bounds> {
        Self::get_element_bounds(doc, group_idx)
    }

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
    /// Returns detected sections if a multi-column layout is found.
    fn detect_column_sections(doc: &Document, roots: &[usize]) -> Option<Vec<ColumnSection>> {
        let max_width_ratio = Decimal::from_str(MAX_ELEMENT_WIDTH_RATIO).unwrap();
        let min_width_ratio = Decimal::from_str(MIN_ELEMENT_WIDTH_RATIO).unwrap();

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
        let mut column_candidates: Vec<(usize, Bounds)> = Vec::new();
        let mut narrow_items: Vec<(usize, Bounds)> = Vec::new();

        for (idx, bounds) in &bounded {
            if bounds.width > content_width * max_width_ratio {
                // Full-width elements are excluded from column detection
            } else if bounds.width >= content_width * min_width_ratio {
                column_candidates.push((*idx, *bounds));
            } else {
                // Narrow elements (section numbers, overlays) — collect for later injection
                narrow_items.push((*idx, *bounds));
            }
        }

        if column_candidates.len() < MIN_ELEMENTS_PER_COLUMN * 2 {
            return None;
        }

        // Find horizontal split
        let split_x = Self::find_horizontal_split(&column_candidates)?;

        // Verify the gap is clean: no column candidate spans across it
        for (_, bounds) in &column_candidates {
            let cx = bounds.center_x();
            let half_gap_region = (split_x - Decimal::from_str("5.0").unwrap())
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

        // Left-edge alignment check: real text columns have all elements
        // aligned to the same left margin. Form grids have elements scattered
        // at varying x positions.
        let alignment_tolerance = Decimal::from_str("5.0").unwrap();
        if !Self::has_consistent_alignment(&column_candidates, split_x, alignment_tolerance) {
            return None;
        }

        // Build sections split at page boundaries.
        // Within each page section, we output left items first, then right items.
        // This produces correct reading order: left col page 1, right col page 1,
        // left col page 2, right col page 2, etc.
        let mut all_ys: Vec<Decimal> = left_elements
            .iter()
            .chain(right_elements.iter())
            .map(|(_, y)| *y)
            .collect();
        all_ys.sort();
        all_ys.dedup();

        let col_min_y = all_ys.first().copied()?;
        let col_max_y = all_ys.last().copied()?;

        // Use pre-computed page breaks from the flattener (content subform boundaries).
        // Filter to breaks within the column content range.
        let mut break_ys: Vec<Decimal> = doc
            .source
            .page
            .page_breaks
            .iter()
            .copied()
            .filter(|&y| y > col_min_y && y < col_max_y)
            .collect();

        // If no subform boundary breaks fall within the column content, fall back
        // to page-height arithmetic (the original behavior).
        if break_ys.is_empty() {
            let page_height = doc.source.page.height;
            if page_height > Decimal::ZERO {
                let mut page_y = page_height;
                while page_y < col_max_y {
                    if page_y > col_min_y {
                        break_ys.push(page_y);
                    }
                    page_y += page_height;
                }
            }
        }

        // Build section bounds from page breaks
        let mut section_bounds: Vec<(Decimal, Decimal)> = Vec::new();
        let mut prev_y = col_min_y;
        for br in &break_ys {
            section_bounds.push((prev_y, *br));
            prev_y = *br;
        }
        section_bounds.push((prev_y, col_max_y + Decimal::ONE));

        // For each section range, collect left/right elements
        let mut sections: Vec<ColumnSection> = Vec::new();

        // Vertical slack (points) when aligning the two columns' starting edges.
        let band_tolerance = Decimal::from_str("6.0").unwrap();

        for (start_y, end_y) in &section_bounds {
            let left_in_section: Vec<(usize, Decimal)> = left_elements
                .iter()
                .filter(|(_, y)| y >= start_y && y < end_y)
                .copied()
                .collect();

            let right_in_section: Vec<(usize, Decimal)> = right_elements
                .iter()
                .filter(|(_, y)| y >= start_y && y < end_y)
                .copied()
                .collect();

            // A genuine two-column band has both columns starting at roughly the
            // same height. Content sitting above the point where the *second*
            // column begins is single-column intro material (e.g. a sub-title
            // field or an address block) that merely happens to be left-aligned
            // and narrow. Sweeping it into the column section would drag it — and
            // everything below it — ahead of intervening single-column content in
            // reading order. Trim that overhang by aligning both columns to the
            // lower of their two starting edges.
            //
            // The band is anchored on *document* content only. Master-page
            // furniture (a centred watermark, etc.) is decorative page chrome
            // whose position is meaningless for reading order; letting it anchor
            // the band would defeat the trim. It is otherwise kept (it may be the
            // element that lets a sparse column clear the detection threshold) and
            // simply swept along if it falls inside the band.
            let real_top = |items: &[(usize, Decimal)]| {
                items
                    .iter()
                    .filter(|(idx, _)| doc.master_page_region(*idx).is_none())
                    .map(|(_, y)| *y)
                    .min()
            };
            let band_top = match (real_top(&left_in_section), real_top(&right_in_section)) {
                (Some(left_min), Some(right_min)) => left_min.max(right_min) - band_tolerance,
                // If a column has no document content here, fall back to no trimming.
                _ => Decimal::MIN,
            };

            let mut section_left: Vec<usize> = left_in_section
                .iter()
                .filter(|(_, y)| *y >= band_top)
                .map(|(idx, _)| *idx)
                .collect();

            let mut section_right: Vec<usize> = right_in_section
                .iter()
                .filter(|(_, y)| *y >= band_top)
                .map(|(idx, _)| *idx)
                .collect();

            // Inject narrow items (section numbers, etc.) into the correct column
            // side, restricted to the same aligned band.
            for (idx, bounds) in &narrow_items {
                let y = bounds.top();
                if y < *start_y || y >= *end_y || y < band_top {
                    continue;
                }
                if bounds.center_x() < split_x {
                    section_left.push(*idx);
                } else {
                    section_right.push(*idx);
                }
            }

            // Only create a section if both columns have content
            if !section_left.is_empty() && !section_right.is_empty() {
                sections.push(ColumnSection {
                    left: section_left,
                    right: section_right,
                });
            }
        }

        if sections.is_empty() {
            return None;
        }

        Some(sections)
    }

    /// Check that both column bands have consistent element widths.
    /// Returns true if in each band, the most common width accounts for
    /// at least 50% of elements (indicating a uniform column container
    /// rather than a form grid with varying field widths).
    /// Also checks that left and right columns have similar dominant widths
    /// (real columns are similar size, unlike form labels + fields).
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

        let min_ratio = Decimal::from_str("0.5").unwrap();
        if Self::dominant_value_ratio(&left_widths, tolerance) < min_ratio
            || Self::dominant_value_ratio(&right_widths, tolerance) < min_ratio
        {
            return false;
        }

        // The dominant widths of left and right columns must be similar.
        // Real multi-column layouts have roughly equal column widths.
        // A narrow label column (108pt) next to a wide content area (240pt) is a form, not columns.
        let left_dominant = Self::find_dominant_value(&left_widths, tolerance);
        let right_dominant = Self::find_dominant_value(&right_widths, tolerance);
        if let (Some(lw), Some(rw)) = (left_dominant, right_dominant) {
            let narrow = lw.min(rw);
            let wide = lw.max(rw);
            // Columns should be within 2:1 ratio of each other
            if wide > Decimal::ZERO && narrow * Decimal::TWO < wide {
                return false;
            }
        }

        true
    }

    /// Check that both column bands have consistent left-edge alignment.
    /// Real text columns share a common left margin; form grids have elements
    /// scattered at varying x positions. Returns true if at least 70% of
    /// elements in each band share a common left edge.
    fn has_consistent_alignment(
        candidates: &[(usize, Bounds)],
        split_x: Decimal,
        tolerance: Decimal,
    ) -> bool {
        let left_xs: Vec<Decimal> = candidates
            .iter()
            .filter(|(_, b)| b.center_x() < split_x)
            .map(|(_, b)| b.x)
            .collect();

        let right_xs: Vec<Decimal> = candidates
            .iter()
            .filter(|(_, b)| b.center_x() >= split_x)
            .map(|(_, b)| b.x)
            .collect();

        let threshold = Decimal::from_str("0.7").unwrap();
        Self::dominant_value_ratio(&left_xs, tolerance) >= threshold
            && Self::dominant_value_ratio(&right_xs, tolerance) >= threshold
    }

    /// Compute what fraction of values match the most common value (±tolerance).
    fn dominant_value_ratio(values: &[Decimal], tolerance: Decimal) -> Decimal {
        if values.is_empty() {
            return Decimal::ZERO;
        }

        let mut best_count = 0usize;
        for v in values {
            let count = values
                .iter()
                .filter(|other| (*other - v).abs() <= tolerance)
                .count();
            if count > best_count {
                best_count = count;
            }
        }

        Decimal::from(best_count as u64) / Decimal::from(values.len() as u64)
    }

    /// Find the most common value in a set (±tolerance). Returns the value with
    /// the highest count of nearby matches.
    fn find_dominant_value(values: &[Decimal], tolerance: Decimal) -> Option<Decimal> {
        if values.is_empty() {
            return None;
        }

        let mut best_count = 0usize;
        let mut best_value = values[0];
        for v in values {
            let count = values
                .iter()
                .filter(|other| (*other - v).abs() <= tolerance)
                .count();
            if count > best_count {
                best_count = count;
                best_value = *v;
            }
        }

        Some(best_value)
    }
}

impl AnalysisModule for ColumnLayoutDetector {
    fn name(&self) -> &'static str {
        "ColumnLayoutDetector"
    }

    fn process(&self, doc: &mut Document) {
        let roots = doc.roots();

        let Some(sections) = Self::detect_column_sections(doc, &roots) else {
            return;
        };

        // Process each section: merge all items into a single ColumnSection group
        // with left-column items first (sorted by y), then right-column items (sorted by y).
        // The ColumnSection tells the structured converter to preserve this order
        // without re-sorting by position.
        for section in sections.iter() {
            let mut left_sorted: Vec<(usize, Decimal)> = section
                .left
                .iter()
                .filter_map(|&idx| Self::get_element_bounds(doc, idx).map(|b| (idx, b.y)))
                .collect();
            left_sorted.sort_by_key(|(_, y)| *y);

            let mut right_sorted: Vec<(usize, Decimal)> = section
                .right
                .iter()
                .filter_map(|&idx| Self::get_element_bounds(doc, idx).map(|b| (idx, b.y)))
                .collect();
            right_sorted.sort_by_key(|(_, y)| *y);

            let mut ordered_indices: Vec<usize> = Vec::new();
            ordered_indices.extend(left_sorted.into_iter().map(|(idx, _)| idx));
            ordered_indices.extend(right_sorted.into_iter().map(|(idx, _)| idx));

            doc.merge(
                ordered_indices,
                GroupKind::ColumnSection,
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
            page: Page::new(num(595.0), num(842.0)),
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
        // Should have 1 ColumnSection containing all column items in order
        assert_eq!(
            roots.len(),
            1,
            "Expected 1 ColumnSection group, got {}",
            roots.len()
        );

        let group = doc.get_group(roots[0]).unwrap();
        assert!(matches!(group.kind, GroupKind::ColumnSection));
        // 9 left + 9 right = 18 children
        assert_eq!(group.children.len(), 18);
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
    fn test_narrow_elements_not_treated_as_columns() {
        // Narrow elements (like number overlays) should be captured into the
        // ColumnSection alongside their column — not left as stray roots or
        // marked as NoPrint.
        let nodes = vec![
            // Narrow elements (≤ min_width_ratio of content width)
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

        // No NoPrint — column detector doesn't discard elements
        assert_eq!(
            noprint_count, 0,
            "Column detector should not mark anything as NoPrint"
        );
        // 1 ColumnSection group created (containing all column items directly)
        let column_section_count = roots
            .iter()
            .filter(|&&idx| matches!(doc.get_group(idx).unwrap().kind, GroupKind::ColumnSection))
            .count();
        assert_eq!(column_section_count, 1, "Expected 1 ColumnSection group");
        // Narrow elements are captured into the ColumnSection (not left as stray leaves)
        let leaf_count = roots
            .iter()
            .filter(|&&idx| matches!(doc.get_group(idx).unwrap().kind, GroupKind::Leaf { .. }))
            .count();
        assert_eq!(
            leaf_count, 0,
            "Narrow elements should be captured into ColumnSection, not left as stray leaves"
        );
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
        let column_section_count = roots
            .iter()
            .filter(|&&idx| matches!(doc.get_group(idx).unwrap().kind, GroupKind::ColumnSection))
            .count();
        // Full-width element remains as root leaf, column elements grouped into 1 ColumnSection
        assert_eq!(column_section_count, 1, "Expected 1 ColumnSection group");
        assert_eq!(
            roots.len(),
            2,
            "Expected 2 roots: 1 full-width + 1 ColumnSection"
        );
    }

    #[test]
    fn test_multiple_column_sections() {
        // Two separate column sections on different pages.
        // Page height is 842pt, so section 2 at y=900+ is on page 2.
        let nodes = vec![
            // Section 1 (page 1): y=10 to y=170
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
            // Section 2 (page 2): y=900 to y=1060
            text_node(0.0, 900.0, 200.0, 14.0, "S2 Left text 1 content"),
            text_node(0.0, 930.0, 200.0, 14.0, "S2 Left text 2 content"),
            text_node(0.0, 960.0, 200.0, 14.0, "S2 Left text 3 content"),
            text_node(0.0, 990.0, 200.0, 14.0, "S2 Left text 4 content"),
            text_node(0.0, 1020.0, 200.0, 14.0, "S2 Left text 5 content"),
            text_node(0.0, 1060.0, 200.0, 14.0, "S2 Left text 6 content"),
            text_node(260.0, 900.0, 200.0, 14.0, "S2 Right text 1 content"),
            text_node(260.0, 930.0, 200.0, 14.0, "S2 Right text 2 content"),
            text_node(260.0, 960.0, 200.0, 14.0, "S2 Right text 3 content"),
            text_node(260.0, 990.0, 200.0, 14.0, "S2 Right text 4 content"),
            text_node(260.0, 1020.0, 200.0, 14.0, "S2 Right text 5 content"),
            text_node(260.0, 1060.0, 200.0, 14.0, "S2 Right text 6 content"),
        ];

        let flattened = make_flattened(nodes);
        let mut doc = Document::from_flattened(&flattened);

        ColumnLayoutDetector::new().process(&mut doc);

        let roots = doc.roots();
        let column_section_count = roots
            .iter()
            .filter(|&&idx| matches!(doc.get_group(idx).unwrap().kind, GroupKind::ColumnSection))
            .count();
        // 2 pages = 2 ColumnSection groups
        assert_eq!(
            column_section_count, 2,
            "Expected 2 ColumnSection groups (one per page)"
        );
    }

    #[test]
    fn test_columns_with_different_y_start_same_section() {
        // Columns that start at different y positions — both on page 1,
        // so they should end up in a single ColumnSection.
        let nodes = vec![
            text_node(0.0, 50.0, 200.0, 14.0, "Left column text 1 here"),
            text_node(0.0, 80.0, 200.0, 14.0, "Left column text 2 here"),
            text_node(0.0, 110.0, 200.0, 14.0, "Left column text 3 here"),
            text_node(0.0, 140.0, 200.0, 14.0, "Left column text 4 here"),
            text_node(0.0, 170.0, 200.0, 14.0, "Left column text 5 here"),
            text_node(0.0, 210.0, 200.0, 14.0, "Left column text 6 here"),
            // Right column starts 30pt lower
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
        let column_section_count = roots
            .iter()
            .filter(|&&idx| matches!(doc.get_group(idx).unwrap().kind, GroupKind::ColumnSection))
            .count();
        assert_eq!(
            column_section_count, 1,
            "Expected 1 ColumnSection (all on same page)"
        );
    }
}
