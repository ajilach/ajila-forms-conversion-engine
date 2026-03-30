//! Label attacher module.
//!
//! Associates text labels with their corresponding fields to create
//! LabeledField groups.
//!
//! Uses statistical analysis to determine the dominant label position
//! (above, below, or left of fields) based on the document layout.

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::HashMap;

/// The position of a label relative to its field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LabelPosition {
    Above,
    Below,
    Left,
}

/// Attaches labels to fields based on spatial relationships.
///
/// Uses statistical analysis to determine the dominant label position:
/// 1. Analyzes all text-field spatial relationships
/// 2. Determines which position (above, below, left) is most common
/// 3. Uses that position to match labels to fields
pub struct LabelAttacher {
    /// Maximum vertical distance for label above/below field
    pub vertical_threshold: Decimal,
    /// Maximum horizontal distance for label to left of field
    pub horizontal_threshold: Decimal,
    /// Vertical tolerance for "same line" detection
    pub line_tolerance: Decimal,
}

impl Default for LabelAttacher {
    fn default() -> Self {
        Self::new()
    }
}

impl LabelAttacher {
    pub fn new() -> Self {
        LabelAttacher {
            vertical_threshold: Decimal::from_str("30.0").unwrap(),
            horizontal_threshold: Decimal::from_str("150.0").unwrap(),
            line_tolerance: Decimal::from_str("8.0").unwrap(),
        }
    }

    /// Configure the vertical threshold.
    pub fn with_vertical_threshold(mut self, threshold: Decimal) -> Self {
        self.vertical_threshold = threshold;
        self
    }

    /// Configure the horizontal threshold.
    pub fn with_horizontal_threshold(mut self, threshold: Decimal) -> Self {
        self.horizontal_threshold = threshold;
        self
    }

    /// Check if text is above the field and return the gap distance.
    fn check_above(&self, text_bounds: &Bounds, field_bounds: &Bounds) -> Option<Decimal> {
        text_bounds.is_above_within(field_bounds, self.vertical_threshold, self.line_tolerance)
    }

    /// Check if text is below the field and return the gap distance.
    fn check_below(&self, text_bounds: &Bounds, field_bounds: &Bounds) -> Option<Decimal> {
        text_bounds.is_below_within(field_bounds, self.vertical_threshold, self.line_tolerance)
    }

    /// Check if text is to the left of the field and return the gap distance.
    fn check_left(&self, text_bounds: &Bounds, field_bounds: &Bounds) -> Option<Decimal> {
        text_bounds.is_left_within(field_bounds, self.horizontal_threshold, self.line_tolerance)
    }

    /// Analyze all text-field relationships and determine the dominant label position.
    ///
    /// For each field, finds the closest text in each direction (above, below, left).
    /// Then votes for the position that most frequently has the closest text.
    fn analyze_label_positions(
        &self,
        doc: &Document,
        text_groups: &[usize],
        field_groups: &[usize],
    ) -> Option<LabelPosition> {
        // For each field, find the closest text in each direction
        // Then determine which direction most often has the closest match
        let mut position_votes: HashMap<LabelPosition, usize> = HashMap::new();

        for &field_idx in field_groups {
            let Some(field_bounds) = doc.get_bounds(field_idx) else {
                continue;
            };

            // Find the closest text in each direction for this field
            let mut best_above: Option<Decimal> = None;
            let mut best_below: Option<Decimal> = None;
            let mut best_left: Option<Decimal> = None;

            for &text_idx in text_groups {
                let Some(text_bounds) = doc.get_bounds(text_idx) else {
                    continue;
                };

                if let Some(gap) = self.check_above(&text_bounds, &field_bounds)
                    && best_above.is_none_or(|b| gap < b)
                {
                    best_above = Some(gap);
                }
                if let Some(gap) = self.check_below(&text_bounds, &field_bounds)
                    && best_below.is_none_or(|b| gap < b)
                {
                    best_below = Some(gap);
                }
                if let Some(gap) = self.check_left(&text_bounds, &field_bounds)
                    && best_left.is_none_or(|b| gap < b)
                {
                    best_left = Some(gap);
                }
            }

            // Vote for the direction with the smallest gap (closest text)
            let candidates: Vec<(LabelPosition, Decimal)> = [
                best_above.map(|g| (LabelPosition::Above, g)),
                best_below.map(|g| (LabelPosition::Below, g)),
                best_left.map(|g| (LabelPosition::Left, g)),
            ]
            .into_iter()
            .flatten()
            .collect();

            if let Some((winner, _gap)) = candidates.into_iter().min_by_key(|(_, g)| *g) {
                *position_votes.entry(winner).or_insert(0) += 1;
            }
        }

        // Find the dominant position (most votes).
        // Break ties by variant order for deterministic results.
        position_votes
            .into_iter()
            .max_by(|(pos1, count1), (pos2, count2)| {
                count1.cmp(count2).then_with(|| pos1.cmp(pos2))
            })
            .map(|(pos, _)| pos)
    }

    /// Find the best label for a field at the given position.
    fn find_label_at_position(
        &self,
        doc: &Document,
        field_idx: usize,
        text_candidates: &[usize],
        position: LabelPosition,
    ) -> Option<(usize, Decimal)> {
        let field_bounds = doc.get_bounds(field_idx)?;

        let mut best: Option<(usize, Decimal)> = None;

        for &text_idx in text_candidates {
            let Some(text_bounds) = doc.get_bounds(text_idx) else {
                continue;
            };

            let gap = match position {
                LabelPosition::Above => self.check_above(&text_bounds, &field_bounds),
                LabelPosition::Below => self.check_below(&text_bounds, &field_bounds),
                LabelPosition::Left => self.check_left(&text_bounds, &field_bounds),
            };

            if let Some(g) = gap
                && best.map(|(_, best_gap)| g < best_gap).unwrap_or(true)
            {
                best = Some((text_idx, g));
            }
        }

        best
    }
}

impl AnalysisModule for LabelAttacher {
    fn name(&self) -> &'static str {
        "LabelAttacher"
    }

    fn process(&self, doc: &mut Document) {
        // Get all root groups
        let roots = doc.roots();

        // Find TextBlock groups that are NOT headings (headings shouldn't be used as labels)
        // Only plain text blocks should be used as field labels
        // Also filter out text blocks that have no actual text content
        let text_groups: Vec<usize> = roots
            .iter()
            .filter(|&&idx| {
                doc.is_text_block(idx)
                    && !doc.is_heading(idx)
                    && !doc.get_text_content(idx).trim().is_empty()
            })
            .copied()
            .collect();

        if text_groups.is_empty() {
            return;
        }

        let field_groups: Vec<usize> = roots
            .iter()
            .filter(|&&idx| doc.is_field(idx))
            .copied()
            .collect();

        // Steps 1-3: attach labels to regular fields
        if !field_groups.is_empty() {
            if let Some(dominant_position) =
                self.analyze_label_positions(doc, &text_groups, &field_groups)
            {
                self.attach_labels_to_fields(doc, &text_groups, &field_groups, dominant_position);
            }
        }

        // Step 4: attach labels to radio/excl groups
        self.attach_labels_to_radio_groups(doc);
    }
}

impl LabelAttacher {
    /// Attach labels to regular fields using the dominant position.
    fn attach_labels_to_fields(
        &self,
        doc: &mut Document,
        text_groups: &[usize],
        field_groups: &[usize],
        dominant_position: LabelPosition,
    ) {
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        let mut used_labels: std::collections::HashSet<usize> = std::collections::HashSet::new();

        // Sort fields by position for consistent processing
        let mut sorted_fields = field_groups.to_vec();
        sorted_fields.sort_by(|&a, &b| {
            let bounds_a = doc.get_bounds(a);
            let bounds_b = doc.get_bounds(b);
            match (bounds_a, bounds_b) {
                (Some(a), Some(b)) => a.y.cmp(&b.y).then_with(|| a.x.cmp(&b.x)),
                _ => std::cmp::Ordering::Equal,
            }
        });

        // Define fallback order for each dominant position
        let fallback_directions = match dominant_position {
            LabelPosition::Above => vec![LabelPosition::Left, LabelPosition::Below],
            LabelPosition::Below => vec![LabelPosition::Above, LabelPosition::Left],
            LabelPosition::Left => vec![LabelPosition::Above, LabelPosition::Below],
        };

        for field_idx in sorted_fields {
            // Filter out already-used labels
            let available_labels: Vec<_> = text_groups
                .iter()
                .filter(|idx| !used_labels.contains(idx))
                .copied()
                .collect();

            // Try dominant direction first
            let mut matched =
                self.find_label_at_position(doc, field_idx, &available_labels, dominant_position);

            // If no match in dominant direction, try fallback directions
            if matched.is_none() {
                for &fallback_dir in &fallback_directions {
                    if let Some(result) =
                        self.find_label_at_position(doc, field_idx, &available_labels, fallback_dir)
                    {
                        matched = Some(result);
                        break;
                    }
                }
            }

            if let Some((label_idx, _gap)) = matched {
                pairs.push((label_idx, field_idx));
                used_labels.insert(label_idx);
            }
        }

        // Create LabeledField groups
        for (label_idx, field_idx) in pairs {
            doc.merge_inferred(
                vec![label_idx, field_idx],
                GroupKind::LabeledField { label: 0, field: 1 },
                self.name(),
            );
        }
    }

    /// Attach labels to radio button / exclusion groups.
    ///
    /// Runs after regular field label attachment. Uses remaining unclaimed
    /// text blocks and tries Above, Left, Below in order.
    fn attach_labels_to_radio_groups(&self, doc: &mut Document) {
        let roots_after = doc.roots();

        let remaining_text_groups: Vec<usize> = roots_after
            .iter()
            .filter(|&&idx| {
                doc.is_text_block(idx)
                    && !doc.is_heading(idx)
                    && !doc.get_text_content(idx).trim().is_empty()
            })
            .copied()
            .collect();

        let radio_groups: Vec<usize> = roots_after
            .iter()
            .filter(|&&idx| doc.is_radio_or_excl_group(idx))
            .copied()
            .collect();

        if remaining_text_groups.is_empty() || radio_groups.is_empty() {
            return;
        }

        let mut radio_pairs: Vec<(usize, usize)> = Vec::new();
        let mut used_radio_labels: std::collections::HashSet<usize> =
            std::collections::HashSet::new();

        let directions = [
            LabelPosition::Above,
            LabelPosition::Left,
            LabelPosition::Below,
        ];

        for &radio_idx in &radio_groups {
            let available: Vec<_> = remaining_text_groups
                .iter()
                .filter(|idx| !used_radio_labels.contains(idx))
                .copied()
                .collect();

            let mut matched = None;
            for &dir in &directions {
                if let Some(result) = self.find_label_at_position(doc, radio_idx, &available, dir) {
                    matched = Some(result);
                    break;
                }
            }

            if let Some((label_idx, _gap)) = matched {
                radio_pairs.push((label_idx, radio_idx));
                used_radio_labels.insert(label_idx);
            }
        }

        for (label_idx, radio_idx) in radio_pairs {
            doc.merge_inferred(
                vec![label_idx, radio_idx],
                GroupKind::LabeledField { label: 0, field: 1 },
                self.name(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::modules::{AnalysisModule, FieldGrouper, TextBlockGrouper};
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;

    #[test]
    fn test_label_above_field() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Labels above fields (this pattern should be detected as dominant)
                FlattenedNode::new_text(
                    "First Name:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(60.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_FirstName".to_string(),
                    "".to_string(),
                    "First Name".to_string(),
                    num(10.0),
                    num(115.0),
                    num(150.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    "Last Name:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(150.0),
                    num(60.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_LastName".to_string(),
                    "".to_string(),
                    "Last Name".to_string(),
                    num(10.0),
                    num(165.0),
                    num(150.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        LabelAttacher::new().process(&mut doc);

        // Should have created LabeledFields
        let labeled = doc.labeled_fields();
        assert_eq!(labeled.len(), 2);
    }

    #[test]
    fn test_label_left_of_field() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Labels to left of fields (this pattern should be detected as dominant)
                FlattenedNode::new_text(
                    "Name:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(35.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_Name".to_string(),
                    "".to_string(),
                    "Name".to_string(),
                    num(60.0),
                    num(98.0),
                    num(150.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    "Email:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(130.0),
                    num(35.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "TF_Email".to_string(),
                    "".to_string(),
                    "Email".to_string(),
                    num(60.0),
                    num(128.0),
                    num(150.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        LabelAttacher::new().process(&mut doc);

        // Should have created LabeledFields
        let labeled = doc.labeled_fields();
        assert_eq!(labeled.len(), 2);

        // Check that the right labels are attached
        assert_eq!(doc.get_label_text(labeled[0]), Some("Name:".to_string()));
        assert_eq!(doc.get_label_text(labeled[1]), Some("Email:".to_string()));
    }

    #[test]
    fn test_statistical_analysis_chooses_dominant_position() {
        // Mix of positions, but "above" should dominate (3 above vs 1 left)
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Three labels above
                FlattenedNode::new_text(
                    "A:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(50.0),
                    num(20.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "F_A".to_string(),
                    "".to_string(),
                    "A".to_string(),
                    num(10.0),
                    num(65.0),
                    num(100.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    "B:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(20.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "F_B".to_string(),
                    "".to_string(),
                    "B".to_string(),
                    num(10.0),
                    num(115.0),
                    num(100.0),
                    num(20.0),
                ),
                FlattenedNode::new_text(
                    "C:".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(150.0),
                    num(20.0),
                    num(12.0),
                ),
                FlattenedNode::new_field(
                    "F_C".to_string(),
                    "".to_string(),
                    "C".to_string(),
                    num(10.0),
                    num(165.0),
                    num(100.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        LabelAttacher::new().process(&mut doc);

        // All 3 should be labeled
        let labeled = doc.labeled_fields();
        assert_eq!(labeled.len(), 3);
    }

    #[test]
    fn test_no_label_for_distant_field() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Text at top
                FlattenedNode::new_text(
                    "Title".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(10.0),
                    num(30.0),
                    num(12.0),
                ),
                // Field far below (too far to be associated)
                FlattenedNode::new_field(
                    "SomeField".to_string(),
                    "".to_string(),
                    "Some Field".to_string(),
                    num(10.0),
                    num(500.0),
                    num(150.0),
                    num(20.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        FieldGrouper::new().process(&mut doc);
        LabelAttacher::new().process(&mut doc);

        // Should NOT create a LabeledField (too far apart)
        let labeled = doc.labeled_fields();
        assert_eq!(labeled.len(), 0);
    }
}
