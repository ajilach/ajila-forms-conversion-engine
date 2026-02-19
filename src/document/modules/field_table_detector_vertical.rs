//! Vertical field table detector module.
//!
//! Detects columns of fields that have bold label text aligned to their left.
//! When all fields in a column have matching bold labels, the detector
//! creates a GridLayout containing LabeledFields.
//!
//! This module:
//! 1. Finds columns of unclaimed Field groups that are vertically aligned (same x)
//! 2. Searches for bold TextBlock labels positioned directly to the LEFT of the fields
//! 3. Requires ALL fields to have a matching label (no partial matches)
//! 4. Requires all labels to be horizontally aligned with each other (same x)
//! 5. Requires all fields to be horizontally aligned with each other (same x)
//! 6. Creates LabeledFields pairing each label with its field
//! 7. Wraps each column in a 1-column GridLayout

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

type Num = Decimal;

/// Detects vertical field tables: columns of fields with bold labels to the left.
///
/// A vertical field table is detected when:
/// 1. Multiple fields are aligned on the same vertical line (a column)
/// 2. Bold text blocks exist directly to the left of the fields
/// 3. Each field has exactly one label on the same line to its left
/// 4. ALL fields in the column have matching labels
/// 5. All matched labels share the same x-position (horizontally aligned)
///
/// Output: Each column becomes a 1-column GridLayout containing LabeledFields
/// (label text as label, field as field).
pub struct FieldTableDetectorVertical {
    /// Tolerance for considering x-coordinates as the same column (in points)
    pub column_tolerance: Num,
    /// Maximum horizontal gap between label right edge and field left edge (in points)
    pub label_gap_threshold: Num,
    /// Tolerance for same-line detection (in points)
    pub vertical_tolerance: Num,
    /// Tolerance for checking that all labels share the same x-position (in points)
    pub label_alignment_tolerance: Num,
    /// Minimum number of fields required to form a table column
    pub min_fields_per_column: usize,
}

impl Default for FieldTableDetectorVertical {
    fn default() -> Self {
        Self::new()
    }
}

impl FieldTableDetectorVertical {
    pub fn new() -> Self {
        FieldTableDetectorVertical {
            column_tolerance: Decimal::from_str("5.0").unwrap(),
            label_gap_threshold: Decimal::from_str("120.0").unwrap(),
            vertical_tolerance: Decimal::from_str("5.0").unwrap(),
            label_alignment_tolerance: Decimal::from_str("5.0").unwrap(),
            min_fields_per_column: 2,
        }
    }

    /// Configure the column tolerance.
    pub fn with_column_tolerance(mut self, tolerance: Num) -> Self {
        self.column_tolerance = tolerance;
        self
    }

    /// Configure the label gap threshold.
    pub fn with_label_gap_threshold(mut self, threshold: Num) -> Self {
        self.label_gap_threshold = threshold;
        self
    }

    /// Configure the vertical tolerance.
    pub fn with_vertical_tolerance(mut self, tolerance: Num) -> Self {
        self.vertical_tolerance = tolerance;
        self
    }

    /// Configure the label alignment tolerance.
    pub fn with_label_alignment_tolerance(mut self, tolerance: Num) -> Self {
        self.label_alignment_tolerance = tolerance;
        self
    }

    /// Configure minimum fields per column.
    pub fn with_min_fields_per_column(mut self, min: usize) -> Self {
        self.min_fields_per_column = min;
        self
    }

    /// Check if a group is inside a repeatable section (has Occurrence hint).
    fn is_inside_repeatable(&self, doc: &Document, group_idx: usize) -> bool {
        let nodes = doc.collect_nodes(group_idx);
        nodes.iter().any(|node| {
            node.hints
                .iter()
                .any(|hint| matches!(hint, crate::flattened::Hint::Occurrence { .. }))
        })
    }

    /// Find all unclaimed Field groups (not yet part of LabeledField, etc.)
    /// Excludes fields that are inside repeatable sections.
    fn find_unclaimed_fields(&self, doc: &Document) -> Vec<usize> {
        let roots = doc.roots();
        roots
            .into_iter()
            .filter(|&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::Field) | Some(GroupKind::DateField { .. })
                ) && !self.is_inside_repeatable(doc, idx)
            })
            .collect()
    }

    /// Find all unclaimed TextBlock groups that could be labels (not headings).
    fn find_candidate_labels(&self, doc: &Document) -> Vec<usize> {
        let roots = doc.roots();
        roots
            .into_iter()
            .filter(|&idx| doc.is_text_block(idx) && !doc.is_heading(idx))
            .collect()
    }

    /// Group fields into columns based on x-coordinate alignment.
    fn group_fields_into_columns(
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

        // Sort by x-coordinate (left to right)
        bounded_fields.sort_by(|a, b| a.1.x.cmp(&b.1.x));

        // Group into columns
        let mut columns: Vec<Vec<(usize, Bounds)>> = Vec::new();

        for (idx, bounds) in bounded_fields {
            // Try to find an existing column with similar x-coordinate
            let mut found_column = false;
            for column in &mut columns {
                let col_x = column[0].1.x;
                if (bounds.x - col_x).abs() <= self.column_tolerance {
                    column.push((idx, bounds));
                    found_column = true;
                    break;
                }
            }

            if !found_column {
                columns.push(vec![(idx, bounds)]);
            }
        }

        // Sort each column by y-coordinate (top to bottom)
        for column in &mut columns {
            column.sort_by(|a, b| a.1.y.cmp(&b.1.y));
        }

        columns
    }

    /// Try to match all fields in a column with labels to their left.
    /// Returns None if any field does not have a matching label,
    /// or if labels are not horizontally aligned with each other.
    /// Returns Some(vec of (label_idx, field_idx) pairs) if all match.
    fn match_column_with_labels(
        &self,
        _doc: &Document,
        column: &[(usize, Bounds)],
        candidate_labels: &[(usize, Bounds)],
    ) -> Option<Vec<(usize, usize)>> {
        let mut matches: Vec<(usize, usize)> = Vec::new();
        let mut matched_label_bounds: Vec<&Bounds> = Vec::new();
        let mut used_labels: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for (field_idx, field_bounds) in column {
            // Find the best label to the left of this field
            let mut best_match: Option<(usize, Num, &Bounds)> = None;

            for &(label_idx, ref label_bounds) in candidate_labels {
                if used_labels.contains(&label_idx) {
                    continue;
                }

                // Label must be to the left of the field and on the same line
                let gap = label_bounds.horizontal_gap_to(field_bounds);
                let Some(gap) = gap else {
                    continue;
                };

                if gap > self.label_gap_threshold {
                    continue;
                }

                if !label_bounds.is_on_same_line(field_bounds, self.vertical_tolerance) {
                    continue;
                }

                if best_match.is_none_or(|(_, best_gap, _)| gap < best_gap) {
                    best_match = Some((label_idx, gap, label_bounds));
                }
            }

            let Some((label_idx, _, label_bounds)) = best_match else {
                // This field has no matching label - fail the entire column
                return None;
            };

            used_labels.insert(label_idx);
            matched_label_bounds.push(label_bounds);
            matches.push((label_idx, *field_idx));
        }

        // Verify that all matched labels are horizontally aligned (same x-position)
        if matched_label_bounds.len() >= 2 {
            let first_x = matched_label_bounds[0].x;
            for label_bounds in &matched_label_bounds[1..] {
                if (label_bounds.x - first_x).abs() > self.label_alignment_tolerance {
                    return None;
                }
            }
        }

        Some(matches)
    }

    /// Process the document to detect vertical field tables.
    fn detect_field_tables_vertical(&self, doc: &mut Document) {
        // Step 1: Find unclaimed fields
        let unclaimed_fields = self.find_unclaimed_fields(doc);
        if unclaimed_fields.len() < self.min_fields_per_column {
            return;
        }

        // Step 2: Find candidate labels (bold text blocks)
        let candidate_labels: Vec<(usize, Bounds)> = self
            .find_candidate_labels(doc)
            .into_iter()
            .filter_map(|idx| doc.get_bounds(idx).map(|b| (idx, b)))
            .collect();

        if candidate_labels.is_empty() {
            return;
        }

        // Step 3: Group fields into columns
        let columns = self.group_fields_into_columns(doc, &unclaimed_fields);

        // Step 4: For each column, try to match with labels
        for column in columns {
            if column.len() < self.min_fields_per_column {
                continue;
            }

            // Try to match all fields in this column with labels to the left
            let Some(matches) =
                self.match_column_with_labels(doc, &column, &candidate_labels)
            else {
                continue;
            };

            // Step 5: Create LabeledFields for each label-field pair
            let mut labeled_field_indices: Vec<usize> = Vec::new();

            for (label_idx, field_idx) in &matches {
                let labeled_field = doc.create_labeled_field(*label_idx, *field_idx, self.name());
                labeled_field_indices.push(labeled_field);
            }

            // Step 6: Create a 1-column GridLayout containing the LabeledFields
            let num_rows = labeled_field_indices.len();
            let spans = vec![1; num_rows];

            doc.merge(
                labeled_field_indices,
                GroupKind::GridLayout {
                    columns: 1,
                    spans,
                },
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
        }
    }
}

impl AnalysisModule for FieldTableDetectorVertical {
    fn name(&self) -> &'static str {
        "FieldTableDetectorVertical"
    }

    fn process(&self, doc: &mut Document) {
        self.detect_field_tables_vertical(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_configuration() {
        let detector = FieldTableDetectorVertical::new();
        assert_eq!(detector.min_fields_per_column, 2);
        assert_eq!(
            detector.column_tolerance,
            Decimal::from_str("5.0").unwrap()
        );
        assert_eq!(
            detector.label_gap_threshold,
            Decimal::from_str("120.0").unwrap()
        );
        assert_eq!(
            detector.vertical_tolerance,
            Decimal::from_str("5.0").unwrap()
        );
        assert_eq!(
            detector.label_alignment_tolerance,
            Decimal::from_str("5.0").unwrap()
        );
    }

    #[test]
    fn test_builder_configuration() {
        let detector = FieldTableDetectorVertical::new()
            .with_min_fields_per_column(3)
            .with_column_tolerance(Decimal::from_str("10.0").unwrap())
            .with_label_gap_threshold(Decimal::from_str("50.0").unwrap())
            .with_vertical_tolerance(Decimal::from_str("8.0").unwrap())
            .with_label_alignment_tolerance(Decimal::from_str("3.0").unwrap());

        assert_eq!(detector.min_fields_per_column, 3);
        assert_eq!(
            detector.column_tolerance,
            Decimal::from_str("10.0").unwrap()
        );
        assert_eq!(
            detector.label_gap_threshold,
            Decimal::from_str("50.0").unwrap()
        );
        assert_eq!(
            detector.vertical_tolerance,
            Decimal::from_str("8.0").unwrap()
        );
        assert_eq!(
            detector.label_alignment_tolerance,
            Decimal::from_str("3.0").unwrap()
        );
    }
}
