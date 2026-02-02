//! Radio button grouper module.
//!
//! Groups radio buttons that are on the same line (horizontally or vertically)
//! into RadioButtonGroup groups. Grouping stops if another element is in between.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::HashSet;

/// Groups adjacent radio buttons on the same line into RadioButtonGroup.
///
/// Radio buttons are grouped if they:
/// 1. Are aligned horizontally (same Y coordinate) or vertically (same X coordinate)
/// 2. Are close to each other
/// 3. Have no other elements in between
pub struct RadioButtonGrouper {
    /// Maximum horizontal gap between radio buttons to group them
    pub max_horizontal_gap: Decimal,
    /// Maximum vertical gap between radio buttons to group them
    pub max_vertical_gap: Decimal,
    /// Tolerance for considering elements on the same line
    pub alignment_tolerance: Decimal,
}

impl Default for RadioButtonGrouper {
    fn default() -> Self {
        Self::new()
    }
}

impl RadioButtonGrouper {
    pub fn new() -> Self {
        RadioButtonGrouper {
            max_horizontal_gap: Decimal::from_str("50.0").unwrap(),
            max_vertical_gap: Decimal::from_str("30.0").unwrap(),
            alignment_tolerance: Decimal::from_str("10.0").unwrap(),
        }
    }

    /// Check if two radio buttons are horizontally aligned.
    fn are_horizontally_aligned(&self, bounds1: &Bounds, bounds2: &Bounds) -> bool {
        bounds1.is_horizontally_aligned(bounds2, self.alignment_tolerance)
    }

    /// Check if two radio buttons are vertically aligned.
    fn are_vertically_aligned(&self, bounds1: &Bounds, bounds2: &Bounds) -> bool {
        bounds1.is_vertically_aligned(bounds2, self.alignment_tolerance)
    }

    /// Calculate horizontal distance between two radio buttons.
    fn horizontal_distance(&self, bounds1: &Bounds, bounds2: &Bounds) -> Decimal {
        bounds1.horizontal_gap_to(bounds2).unwrap_or(Decimal::MAX)
    }

    /// Calculate vertical distance between two radio buttons.
    fn vertical_distance(&self, bounds1: &Bounds, bounds2: &Bounds) -> Decimal {
        bounds1.vertical_gap_to(bounds2).unwrap_or(Decimal::MAX)
    }

    /// Check if there are any elements between two radio buttons.
    fn has_elements_between(
        &self,
        doc: &Document,
        bounds1: &Bounds,
        bounds2: &Bounds,
        radio_button_indices: &HashSet<usize>,
    ) -> bool {
        // Create bounding box that encompasses both radio buttons
        let region = bounds1.union(bounds2);

        // Check all root groups
        for root_idx in doc.roots() {
            // Skip if it's one of the radio buttons we're checking
            if radio_button_indices.contains(&root_idx) {
                continue;
            }

            // Get bounds of this element
            let Some(bounds) = doc.get_bounds(root_idx) else {
                continue;
            };

            // Check if this element overlaps with the region between the two radio buttons
            if region.overlaps(&bounds) {
                return true; // Found an element in between
            }
        }

        false
    }

    /// Group radio buttons that are on the same line.
    fn group_aligned_radio_buttons(&self, doc: &mut Document, radio_buttons: &[usize]) {
        if radio_buttons.is_empty() {
            return;
        }

        let mut grouped: HashSet<usize> = HashSet::new();
        let radio_button_set: HashSet<usize> = radio_buttons.iter().copied().collect();

        for &rb_idx in radio_buttons {
            if grouped.contains(&rb_idx) {
                continue;
            }

            let Some(rb_bounds) = doc.get_bounds(rb_idx) else {
                continue;
            };

            let mut group: Vec<usize> = vec![rb_idx];
            grouped.insert(rb_idx);

            // Try to find adjacent radio buttons in horizontal direction
            let mut found_horizontal = true;
            let mut last_bounds = rb_bounds;

            while found_horizontal {
                found_horizontal = false;

                for &candidate_idx in radio_buttons {
                    if grouped.contains(&candidate_idx) {
                        continue;
                    }

                    let Some(candidate_bounds) = doc.get_bounds(candidate_idx) else {
                        continue;
                    };

                    // Check if horizontally aligned and close
                    if self.are_horizontally_aligned(&last_bounds, &candidate_bounds) {
                        let distance = self.horizontal_distance(&last_bounds, &candidate_bounds);

                        if distance <= self.max_horizontal_gap
                            && !self.has_elements_between(
                                doc,
                                &last_bounds,
                                &candidate_bounds,
                                &radio_button_set,
                            )
                        {
                            group.push(candidate_idx);
                            grouped.insert(candidate_idx);
                            last_bounds = candidate_bounds;
                            found_horizontal = true;
                            break;
                        }
                    }
                }
            }

            // Try to find adjacent radio buttons in vertical direction
            let mut found_vertical = true;
            last_bounds = rb_bounds;

            while found_vertical {
                found_vertical = false;

                for &candidate_idx in radio_buttons {
                    if grouped.contains(&candidate_idx) {
                        continue;
                    }

                    let Some(candidate_bounds) = doc.get_bounds(candidate_idx) else {
                        continue;
                    };

                    // Check if vertically aligned and close
                    if self.are_vertically_aligned(&last_bounds, &candidate_bounds) {
                        let distance = self.vertical_distance(&last_bounds, &candidate_bounds);

                        if distance <= self.max_vertical_gap
                            && !self.has_elements_between(
                                doc,
                                &last_bounds,
                                &candidate_bounds,
                                &radio_button_set,
                            )
                        {
                            group.push(candidate_idx);
                            grouped.insert(candidate_idx);
                            last_bounds = candidate_bounds;
                            found_vertical = true;
                            break;
                        }
                    }
                }
            }

            // Create a RadioButtonGroup if we have more than one radio button
            if group.len() > 1 {
                doc.merge(
                    group,
                    GroupKind::RadioButtonGroup,
                    GroupSource::Inferred {
                        module: self.name().to_string(),
                    },
                );
            }
        }
    }
}

impl AnalysisModule for RadioButtonGrouper {
    fn name(&self) -> &'static str {
        "RadioButtonGrouper"
    }

    fn process(&self, doc: &mut Document) {
        // Get all root RadioButton groups
        let roots = doc.roots();
        let radio_buttons: Vec<usize> = roots
            .iter()
            .filter(|&&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::RadioButton { .. })
                )
            })
            .copied()
            .collect();

        if radio_buttons.is_empty() {
            return;
        }

        // Group aligned radio buttons
        self.group_aligned_radio_buttons(doc, &radio_buttons);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, GroupKind};
    use crate::flattened::{Flattened, FlattenedNode, FlattenedNodeKind, Page};
    use crate::modules::{FieldGrouper, RadioButtonDetector, TextBlockGrouper};
    use crate::xfa::num;

    #[test]
    fn test_horizontal_radio_button_grouping() {
        // Create a flattened document with 3 horizontally aligned radio buttons
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Radio button 1
                FlattenedNode::new_field(
                    "radio1".to_string(),
                    "".to_string(),
                    "".to_string(),
                    num(50.0),
                    num(100.0),
                    num(12.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "Option 1".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(65.0),
                    num(100.0),
                    num(50.0),
                    num(12.0),
                ),
                // Radio button 2 (30 points away horizontally, same Y)
                FlattenedNode::new_field(
                    "radio2".to_string(),
                    "".to_string(),
                    "".to_string(),
                    num(145.0),
                    num(100.0),
                    num(12.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "Option 2".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(160.0),
                    num(100.0),
                    num(50.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);

        // Process with required modules
        FieldGrouper::new().process(&mut doc);
        TextBlockGrouper::new().process(&mut doc);
        RadioButtonDetector::new().process(&mut doc);
        RadioButtonGrouper::new().process(&mut doc);

        // Should have created a RadioButtonGroup
        let radio_button_groups = doc.find_groups(|k| matches!(k, GroupKind::RadioButtonGroup));
        assert_eq!(radio_button_groups.len(), 1);

        // The group should contain 2 RadioButton children
        let group = doc.get_group(radio_button_groups[0]).unwrap();
        assert_eq!(group.children.len(), 2);
    }
}
