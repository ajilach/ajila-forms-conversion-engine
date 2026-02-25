//! Grid template detector module.
//!
//! Detects grid layouts by analyzing spatial arrangement of fields.
//! Groups elements that are aligned in rows and columns into grid structures.
//!
//! The detector works by:
//! 1. Finding unclaimed groups that could be grid elements
//! 2. Analyzing their x/y coordinates to detect row/column alignment
//! 3. Verifying consistent spacing patterns
//! 4. Creating GridLayout groups with appropriate column counts and spans

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::HashSet;

/// Detects grid layouts by identifying elements aligned in rows and columns.
///
/// Grid layouts are characterized by:
/// 1. Multiple elements aligned on consistent x-coordinates (columns)
/// 2. Multiple elements aligned on consistent y-coordinates (rows)
/// 3. At least 2x2 grid (2 rows, 2 columns)
/// 4. Elements arranged in reading order (top to bottom, left to right)
pub struct GridTemplateDetector {
    /// Tolerance for considering two coordinates as aligned (in points)
    pub alignment_tolerance: Decimal,
    /// Minimum number of rows required for a grid
    pub min_rows: usize,
    /// Minimum number of columns required for a grid
    pub min_columns: usize,
    /// Tolerance for "same line" vertical alignment
    pub line_tolerance: Decimal,
}

impl Default for GridTemplateDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl GridTemplateDetector {
    pub fn new() -> Self {
        GridTemplateDetector {
            alignment_tolerance: Decimal::from_str("5.0").unwrap(), // 5 points tolerance
            min_rows: 2,
            min_columns: 2,
            line_tolerance: Decimal::from_str("2.0").unwrap(), // 2 points for same line
        }
    }

    /// Configure the alignment tolerance.
    pub fn with_alignment_tolerance(mut self, tolerance: Decimal) -> Self {
        self.alignment_tolerance = tolerance;
        self
    }

    /// Configure the minimum grid size.
    pub fn with_min_size(mut self, rows: usize, columns: usize) -> Self {
        self.min_rows = rows;
        self.min_columns = columns;
        self
    }

    /// Group coordinates that are within tolerance of each other.
    pub fn cluster_coordinates(&self, coords: &[Decimal]) -> Vec<Vec<usize>> {
        let mut clusters: Vec<Vec<usize>> = Vec::new();
        let mut sorted_indices: Vec<usize> = (0..coords.len()).collect();
        sorted_indices.sort_by(|&a, &b| coords[a].cmp(&coords[b]));

        for idx in sorted_indices {
            let coord = coords[idx];
            let mut found_cluster = false;

            // Try to add to an existing cluster
            for cluster in &mut clusters {
                let cluster_coord = coords[cluster[0]];
                if (coord - cluster_coord).abs() <= self.alignment_tolerance {
                    cluster.push(idx);
                    found_cluster = true;
                    break;
                }
            }

            // Create a new cluster if not found
            if !found_cluster {
                clusters.push(vec![idx]);
            }
        }

        clusters
    }

    /// Compute proportional column spans for elements based on their widths.
    ///
    /// Uses a 12-column grid. If all widths are roughly equal (max/min ratio < 1.2),
    /// returns `None` to signal that equal spacing should be used.
    /// Otherwise, distributes 12 columns proportionally using the largest-remainder method.
    fn compute_spans(widths: &[Decimal]) -> Option<(usize, Vec<usize>)> {
        if widths.len() < 2 {
            return None;
        }

        let min_w = widths.iter().copied().min().unwrap();
        let max_w = widths.iter().copied().max().unwrap();

        // If widths are roughly equal, keep default equal spacing
        if min_w > Decimal::ZERO && max_w * Decimal::from(10) < min_w * Decimal::from(12) {
            return None; // max/min < 1.2
        }

        let total_width: Decimal = widths.iter().copied().sum();
        if total_width <= Decimal::ZERO {
            return None;
        }

        let grid_cols = Decimal::from(12);

        // Compute fractional spans and integer floors
        let fractional: Vec<Decimal> = widths
            .iter()
            .map(|w| (*w * grid_cols) / total_width)
            .collect();
        let mut spans: Vec<usize> = fractional
            .iter()
            .map(|f| f.floor().to_usize().unwrap_or(1).max(1))
            .collect();

        // Distribute remainder using largest-remainder method
        let assigned: usize = spans.iter().sum();
        let mut remainder = 12usize.saturating_sub(assigned);

        if remainder > 0 {
            // Sort indices by fractional part (descending) to assign leftover columns
            let mut indices: Vec<usize> = (0..widths.len()).collect();
            indices.sort_by(|&a, &b| {
                let frac_a = fractional[a] - Decimal::from(spans[a] as u64);
                let frac_b = fractional[b] - Decimal::from(spans[b] as u64);
                frac_b.cmp(&frac_a)
            });
            for &i in &indices {
                if remainder == 0 {
                    break;
                }
                spans[i] += 1;
                remainder -= 1;
            }
        }

        Some((12, spans))
    }

    /// Detect grid patterns in a set of groups (single row only).
    fn detect_grid(&self, doc: &Document, group_indices: &[usize]) -> Option<GridCandidate> {
        // For single-row grids, we need at least min_columns elements
        if group_indices.len() < self.min_columns {
            return None;
        }

        // Collect bounds for all groups
        let mut bounded_groups = Vec::new();
        for &idx in group_indices {
            if let Some(bounds) = doc.get_bounds(idx) {
                bounded_groups.push((idx, bounds));
            }
        }

        if bounded_groups.len() < self.min_columns {
            return None;
        }

        // Check that all groups are on the same horizontal line (within tolerance)
        let first_y = bounded_groups[0].1.y;
        for (_, bounds) in &bounded_groups {
            if (bounds.y - first_y).abs() > self.alignment_tolerance {
                return None; // Not all on same line
            }
        }

        // Sort by x coordinate (left to right)
        bounded_groups.sort_by(|a, b| a.1.x.cmp(&b.1.x));

        // Derive proportional colspans from field widths
        let widths: Vec<Decimal> = bounded_groups.iter().map(|(_, b)| b.width).collect();

        let (columns, spans) = match Self::compute_spans(&widths) {
            Some((cols, spans)) => (cols, spans),
            None => {
                // Equal widths — use span 1 for all, columns = element count
                let n = bounded_groups.len();
                (n, vec![1; n])
            }
        };

        // Build the grid elements in order (single row)
        let elements: Vec<GridElement> = bounded_groups
            .into_iter()
            .zip(spans)
            .map(|((group_idx, _), span)| GridElement { group_idx, span })
            .collect();

        Some(GridCandidate { columns, elements })
    }

    /// Find all potential grid groups among unclaimed groups.
    fn find_grid_candidates(&self, doc: &Document) -> Vec<GridCandidate> {
        // Get all unclaimed groups that could be grid elements (fields, labeled fields, etc.)
        let all_roots = doc.roots();
        let unclaimed: Vec<usize> = all_roots
            .into_iter()
            .filter(|&idx| {
                // Include any group with bounds, but skip headers/footers/headings/grids
                if let Some(group) = doc.get_group(idx) {
                    // Accept fields and labeled fields as grid candidates
                    matches!(
                        group.kind,
                        GroupKind::Field
                            | GroupKind::LabeledField { .. }
                            | GroupKind::RadioButton { .. }
                            | GroupKind::DateField { .. }
                    )
                } else {
                    false
                }
            })
            .collect();

        // Skip if we don't have enough groups
        if unclaimed.len() < self.min_rows * self.min_columns {
            return Vec::new();
        }

        // Group nearby elements together as potential grid candidates
        let mut candidates = Vec::new();
        let mut remaining: HashSet<usize> = unclaimed.iter().copied().collect();

        while !remaining.is_empty() {
            // Take the first remaining element
            let &start_idx = remaining.iter().next().unwrap();
            remaining.remove(&start_idx);

            let start_bounds = match doc.get_bounds(start_idx) {
                Some(b) => b,
                None => continue,
            };

            // Find all elements on the same horizontal line (within alignment tolerance)
            let mut row_group = vec![start_idx];
            let remaining_vec: Vec<usize> = remaining.iter().copied().collect();

            for &other_idx in &remaining_vec {
                if let Some(other_bounds) = doc.get_bounds(other_idx) {
                    // Check if on same horizontal line
                    if (start_bounds.y - other_bounds.y).abs() <= self.alignment_tolerance {
                        row_group.push(other_idx);
                        remaining.remove(&other_idx);
                    }
                }
            }

            // Try to detect a grid pattern in this row
            if let Some(candidate) = self.detect_grid(doc, &row_group) {
                candidates.push(candidate);
            }
        }

        candidates
    }
}

impl AnalysisModule for GridTemplateDetector {
    fn name(&self) -> &'static str {
        "GridTemplateDetector"
    }

    fn process(&self, doc: &mut Document) {
        let candidates = self.find_grid_candidates(doc);

        for candidate in candidates {
            // Create the GridLayout group
            let group_indices: Vec<usize> =
                candidate.elements.iter().map(|e| e.group_idx).collect();
            let spans: Vec<usize> = candidate.elements.iter().map(|e| e.span).collect();

            doc.merge(
                group_indices,
                GroupKind::GridLayout {
                    columns: candidate.columns,
                    spans,
                },
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }
}

/// A candidate grid layout detected in the document.
struct GridCandidate {
    /// Number of columns in the grid
    columns: usize,
    /// Elements in row-major order
    elements: Vec<GridElement>,
}

/// An element within a grid layout.
struct GridElement {
    /// Index of the group in the document
    group_idx: usize,
    /// Column span for this element (default 1)
    span: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(val: &str) -> Decimal {
        Decimal::from_str(val).unwrap()
    }

    #[test]
    fn equal_widths_returns_none() {
        // Three fields of identical width → equal spacing
        let widths = vec![d("100"), d("100"), d("100")];
        assert!(GridTemplateDetector::compute_spans(&widths).is_none());
    }

    #[test]
    fn nearly_equal_widths_returns_none() {
        // max/min = 119/100 = 1.19 < 1.2 → equal spacing
        let widths = vec![d("100"), d("110"), d("119")];
        assert!(GridTemplateDetector::compute_spans(&widths).is_none());
    }

    #[test]
    fn ratio_1_2() {
        // 100 + 200 = 300. Fractions: 4.0, 8.0 → spans [4, 8]
        let widths = vec![d("100"), d("200")];
        let (cols, spans) = GridTemplateDetector::compute_spans(&widths).unwrap();
        assert_eq!(cols, 12);
        assert_eq!(spans, vec![4, 8]);
    }

    #[test]
    fn ratio_1_1_2() {
        // 100 + 100 + 200 = 400. Fractions: 3.0, 3.0, 6.0 → spans [3, 3, 6]
        let widths = vec![d("100"), d("100"), d("200")];
        let (cols, spans) = GridTemplateDetector::compute_spans(&widths).unwrap();
        assert_eq!(cols, 12);
        assert_eq!(spans, vec![3, 3, 6]);
    }

    #[test]
    fn ratio_1_3() {
        // 50 + 150 = 200. Fractions: 3.0, 9.0 → spans [3, 9]
        let widths = vec![d("50"), d("150")];
        let (cols, spans) = GridTemplateDetector::compute_spans(&widths).unwrap();
        assert_eq!(cols, 12);
        assert_eq!(spans, vec![3, 9]);
    }

    #[test]
    fn remainder_distributed_by_largest_fraction() {
        // Widths: 100, 100, 130 = 330 total
        // Fractions: 3.636, 3.636, 4.727
        // Floors: 3, 3, 4 = 10, remainder = 2
        // Fractional parts: 0.636, 0.636, 0.727 → assign +1 to idx 2, then idx 0 (or 1)
        let widths = vec![d("100"), d("100"), d("130")];
        let (cols, spans) = GridTemplateDetector::compute_spans(&widths).unwrap();
        assert_eq!(cols, 12);
        assert_eq!(spans.iter().sum::<usize>(), 12);
        // The wider field should get the largest span
        assert!(spans[2] >= spans[0]);
        assert!(spans[2] >= spans[1]);
    }

    #[test]
    fn four_fields_varying() {
        // 100, 100, 100, 300 = 600 total
        // Fractions: 2.0, 2.0, 2.0, 6.0 → spans [2, 2, 2, 6]
        let widths = vec![d("100"), d("100"), d("100"), d("300")];
        let (cols, spans) = GridTemplateDetector::compute_spans(&widths).unwrap();
        assert_eq!(cols, 12);
        assert_eq!(spans, vec![2, 2, 2, 6]);
    }

    #[test]
    fn single_element_returns_none() {
        let widths = vec![d("100")];
        assert!(GridTemplateDetector::compute_spans(&widths).is_none());
    }

    #[test]
    fn minimum_span_is_one() {
        // Very skewed: 10, 590 = 600 total
        // Fractions: 0.2, 11.8 → floor gives 0, 11
        // But min is clamped to 1, so: [1, 11]
        let widths = vec![d("10"), d("590")];
        let (cols, spans) = GridTemplateDetector::compute_spans(&widths).unwrap();
        assert_eq!(cols, 12);
        assert_eq!(spans, vec![1, 11]);
        assert!(spans.iter().all(|&s| s >= 1));
    }
}
