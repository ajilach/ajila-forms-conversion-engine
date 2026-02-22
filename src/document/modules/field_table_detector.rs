//! Field table detector module.
//!
//! Detects rows of fields that have bold header text aligned above them.
//! When all fields in a row have matching bold headers, the detector
//! creates a GridLayout containing LabeledFields.
//!
//! This module:
//! 1. Finds rows of unclaimed Field groups that are horizontally aligned
//! 2. Searches for bold TextBlock headers positioned directly ABOVE the row
//! 3. Requires ALL fields to have a matching header (no partial matches)
//! 4. Creates LabeledFields pairing each header with its field
//! 5. Wraps each row in a GridLayout with dynamic column count

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

type Num = Decimal;

/// Detects field tables: rows of fields with bold headers aligned above.
///
/// A field table is detected when:
/// 1. Multiple fields are aligned on the same horizontal line (a row)
/// 2. Bold text blocks exist directly above the row
/// 3. Each field has exactly one header that overlaps horizontally
/// 4. ALL fields in the row have matching headers
///
/// Output: Each row becomes a GridLayout containing LabeledFields
/// (header as label, field as field).
pub struct FieldTableDetector {
    /// Tolerance for considering y-coordinates as the same row (in points)
    pub row_tolerance: Num,
    /// Maximum vertical gap between header bottom and field top (in points)
    pub header_gap_threshold: Num,
    /// Tolerance for horizontal overlap detection (in points)
    pub horizontal_tolerance: Num,
    /// Minimum number of fields required to form a table row
    pub min_fields_per_row: usize,
}

impl Default for FieldTableDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl FieldTableDetector {
    pub fn new() -> Self {
        FieldTableDetector {
            row_tolerance: Decimal::from_str("5.0").unwrap(),
            header_gap_threshold: Decimal::from_str("30.0").unwrap(),
            horizontal_tolerance: Decimal::from_str("5.0").unwrap(),
            min_fields_per_row: 2,
        }
    }

    /// Configure the row tolerance.
    pub fn with_row_tolerance(mut self, tolerance: Num) -> Self {
        self.row_tolerance = tolerance;
        self
    }

    /// Configure the header gap threshold.
    pub fn with_header_gap_threshold(mut self, threshold: Num) -> Self {
        self.header_gap_threshold = threshold;
        self
    }

    /// Configure the horizontal tolerance.
    pub fn with_horizontal_tolerance(mut self, tolerance: Num) -> Self {
        self.horizontal_tolerance = tolerance;
        self
    }

    /// Configure minimum fields per row.
    pub fn with_min_fields_per_row(mut self, min: usize) -> Self {
        self.min_fields_per_row = min;
        self
    }

    /// Check if a group contains bold text.
    fn is_bold_text(&self, doc: &Document, group_idx: usize) -> bool {
        doc.collect_nodes(group_idx).iter().any(|n| n.is_bold())
    }

    /// Find all unclaimed TextBlock groups that could be headers.
    fn find_candidate_headers(&self, doc: &Document) -> Vec<usize> {
        let roots = doc.roots();
        roots
            .into_iter()
            .filter(|&idx| {
                doc.is_text_block(idx) && !doc.is_heading(idx) && self.is_bold_text(doc, idx)
            })
            .collect()
    }

    /// Group fields into rows based on y-coordinate alignment.
    fn group_fields_into_rows(
        &self,
        doc: &Document,
        fields: &[usize],
    ) -> Vec<Vec<(usize, Bounds)>> {
        // Collect fields with their bounds
        let mut bounded_fields: Vec<(usize, Bounds)> = fields
            .iter()
            .filter_map(|&idx| doc.get_bounds(idx).map(|b| (idx, b)))
            .collect();

        if bounded_fields.is_empty() {
            return Vec::new();
        }

        // Sort by y-coordinate (top to bottom)
        bounded_fields.sort_by(|a, b| a.1.y.cmp(&b.1.y));

        // Group into rows
        let mut rows: Vec<Vec<(usize, Bounds)>> = Vec::new();

        for (idx, bounds) in bounded_fields {
            // Try to find an existing row with similar y-coordinate
            let mut found_row = false;
            for row in &mut rows {
                let row_y = row[0].1.y;
                if (bounds.y - row_y).abs() <= self.row_tolerance {
                    row.push((idx, bounds));
                    found_row = true;
                    break;
                }
            }

            if !found_row {
                rows.push(vec![(idx, bounds)]);
            }
        }

        // Sort each row by x-coordinate (left to right)
        for row in &mut rows {
            row.sort_by(|a, b| a.1.x.cmp(&b.1.x));
        }

        rows
    }

    /// Try to match all fields in a row with headers above.
    /// Returns None if any field does not have a matching header.
    /// Returns Some(vec of (header_idx, field_idx) pairs) if all match.
    fn match_row_with_headers(
        &self,
        _doc: &Document,
        row: &[(usize, Bounds)],
        candidate_headers: &[(usize, Bounds)],
    ) -> Option<Vec<(usize, usize)>> {
        let mut matches: Vec<(usize, usize)> = Vec::new();
        let mut used_headers: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for (field_idx, field_bounds) in row {
            // Find a header for this field that has not been used yet
            let mut best_match: Option<(usize, Num)> = None;

            for &(header_idx, ref header_bounds) in candidate_headers {
                if used_headers.contains(&header_idx) {
                    continue;
                }

                let gap = header_bounds.vertical_gap_to(field_bounds);
                let Some(gap) = gap else {
                    continue;
                };

                if gap > self.header_gap_threshold {
                    continue;
                }

                if !header_bounds.overlaps_horizontally(field_bounds, self.horizontal_tolerance) {
                    continue;
                }

                if best_match.is_none_or(|(_, best_gap)| gap < best_gap) {
                    best_match = Some((header_idx, gap));
                }
            }

            let Some((header_idx, _)) = best_match else {
                // This field has no matching header - fail the entire row
                return None;
            };

            used_headers.insert(header_idx);
            matches.push((header_idx, *field_idx));
        }

        Some(matches)
    }

    /// Process the document to detect field tables.
    fn detect_field_tables(&self, doc: &mut Document) {
        // Step 1: Find unclaimed fields
        let unclaimed_fields = doc.unclaimed_fields_outside_repeatables();
        if unclaimed_fields.len() < self.min_fields_per_row {
            return;
        }

        // Step 2: Find candidate headers (bold text blocks)
        let candidate_headers: Vec<(usize, Bounds)> = self
            .find_candidate_headers(doc)
            .into_iter()
            .filter_map(|idx| doc.get_bounds(idx).map(|b| (idx, b)))
            .collect();

        if candidate_headers.is_empty() {
            return;
        }

        // Step 3: Group fields into rows
        let rows = self.group_fields_into_rows(doc, &unclaimed_fields);

        // Step 4: For each row, try to match with headers
        for row in rows {
            if row.len() < self.min_fields_per_row {
                continue;
            }

            // Try to match all fields in this row with headers
            let Some(matches) = self.match_row_with_headers(doc, &row, &candidate_headers) else {
                // Not all fields have headers - skip this row
                continue;
            };

            // Step 5: Create LabeledFields for each header-field pair
            let mut labeled_field_indices: Vec<usize> = Vec::new();

            for (header_idx, field_idx) in &matches {
                let labeled_field = doc.create_labeled_field(*header_idx, *field_idx, self.name());
                labeled_field_indices.push(labeled_field);
            }

            // Step 6: Create GridLayout containing the LabeledFields
            let num_columns = labeled_field_indices.len();
            let spans = vec![1; num_columns];

            doc.merge(
                labeled_field_indices,
                GroupKind::GridLayout {
                    columns: num_columns,
                    spans,
                },
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }
}

impl AnalysisModule for FieldTableDetector {
    fn name(&self) -> &'static str {
        "FieldTableDetector"
    }

    fn process(&self, doc: &mut Document) {
        self.detect_field_tables(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_configuration() {
        let detector = FieldTableDetector::new();
        assert_eq!(detector.min_fields_per_row, 2);
        assert_eq!(detector.row_tolerance, Decimal::from_str("5.0").unwrap());
    }

    #[test]
    fn test_builder_configuration() {
        let detector = FieldTableDetector::new()
            .with_min_fields_per_row(3)
            .with_row_tolerance(Decimal::from_str("10.0").unwrap())
            .with_header_gap_threshold(Decimal::from_str("50.0").unwrap());

        assert_eq!(detector.min_fields_per_row, 3);
        assert_eq!(detector.row_tolerance, Decimal::from_str("10.0").unwrap());
        assert_eq!(
            detector.header_gap_threshold,
            Decimal::from_str("50.0").unwrap()
        );
    }
}
