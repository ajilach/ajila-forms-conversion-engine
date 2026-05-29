//! Checkbox grouper module.
//!
//! Groups checkboxes that are spatially aligned (horizontally or vertically)
//! into `CheckboxGroup` groups. Grouping stops if an unrelated element is
//! in between.
//!
//! Unlike radio buttons, checkboxes do not share an `exclGroup` XFA concept.
//! Grouping is based purely on spatial proximity and alignment.

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use crate::flattened::Bounds;
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use std::collections::HashSet;

/// Groups adjacent checkboxes into `CheckboxGroup`.
///
/// Checkboxes are grouped if they:
/// 1. Are aligned horizontally (same Y coordinate) or vertically (same X coordinate)
/// 2. Are close to each other (within `max_horizontal_gap` / `max_vertical_gap`)
/// 3. Have no unrelated elements in between
pub struct CheckboxGrouper {
    /// Maximum horizontal gap between checkboxes to group them
    pub max_horizontal_gap: Decimal,
    /// Maximum vertical gap between checkboxes to group them
    pub max_vertical_gap: Decimal,
    /// Tolerance for considering elements on the same line
    pub alignment_tolerance: Decimal,
}

impl Default for CheckboxGrouper {
    fn default() -> Self {
        Self::new()
    }
}

impl CheckboxGrouper {
    pub fn new() -> Self {
        CheckboxGrouper {
            max_horizontal_gap: Decimal::from_str("50.0").unwrap(),
            max_vertical_gap: Decimal::from_str("30.0").unwrap(),
            alignment_tolerance: Decimal::from_str("10.0").unwrap(),
        }
    }

    /// Get the bounds of just the *field* (checkbox box) part of a Checkbox group,
    /// ignoring the label. Used for alignment checks where the full group bounds —
    /// which include the potentially wide label text — would give misleading results.
    fn get_field_bounds(&self, doc: &Document, cb_idx: usize) -> Option<Bounds> {
        let group = doc.get_group(cb_idx)?;
        if let GroupKind::Checkbox { field, .. } = &group.kind {
            let field_group_idx = *group.children.get(*field)?;
            return doc.get_bounds(field_group_idx);
        }
        None
    }

    /// Check if there are any elements between two checkboxes.
    fn has_elements_between(
        &self,
        doc: &Document,
        bounds1: &Bounds,
        bounds2: &Bounds,
        checkbox_indices: &HashSet<usize>,
    ) -> bool {
        let region = bounds1.union(bounds2);

        for root_idx in doc.roots() {
            if checkbox_indices.contains(&root_idx) {
                continue;
            }

            let Some(bounds) = doc.get_bounds(root_idx) else {
                continue;
            };

            if region.overlaps(&bounds) {
                return true;
            }
        }

        false
    }

    /// Check if there are non-inset elements between two vertically aligned checkboxes.
    ///
    /// Elements that are inset to the right of the checkboxes (e.g. conditional content
    /// below a checkbox) are ignored, analogous to the radio button grouper.
    fn has_non_inset_elements_between(
        &self,
        doc: &Document,
        bounds1: &Bounds,
        bounds2: &Bounds,
        checkbox_indices: &HashSet<usize>,
        inset_threshold: Decimal,
    ) -> bool {
        let region = bounds1.union(bounds2);
        let cb_left = bounds1.left().min(bounds2.left());
        let cb_right = bounds1.right().max(bounds2.right());

        // Collect all child indices of the checkboxes so we can skip them
        let mut checkbox_children: HashSet<usize> = HashSet::new();
        for &cb_idx in checkbox_indices {
            if let Some(group) = doc.get_group(cb_idx) {
                for &child_idx in &group.children {
                    checkbox_children.insert(child_idx);
                }
            }
        }

        for root_idx in doc.roots() {
            if checkbox_indices.contains(&root_idx) {
                continue;
            }
            if checkbox_children.contains(&root_idx) {
                continue;
            }

            let Some(bounds) = doc.get_bounds(root_idx) else {
                continue;
            };

            if region.overlaps(&bounds) {
                let is_inset =
                    bounds.left() >= cb_left + inset_threshold || bounds.left() >= cb_right;

                if !is_inset {
                    let is_checkbox_related = doc
                        .get_group(root_idx)
                        .map(|g| {
                            matches!(
                                g.kind,
                                GroupKind::Checkbox { .. } | GroupKind::CheckboxGroup
                            )
                        })
                        .unwrap_or(false);

                    if !is_checkbox_related {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Group spatially aligned checkboxes together.
    fn group_aligned_checkboxes(&self, doc: &mut Document, checkboxes: &[usize]) {
        if checkboxes.is_empty() {
            return;
        }

        let mut grouped: HashSet<usize> = HashSet::new();
        let checkbox_set: HashSet<usize> = checkboxes.iter().copied().collect();

        for &cb_idx in checkboxes {
            if grouped.contains(&cb_idx) {
                continue;
            }

            let Some(cb_field_bounds) = self.get_field_bounds(doc, cb_idx) else {
                continue;
            };
            let cb_group_bounds = doc.get_bounds(cb_idx).unwrap_or(cb_field_bounds);

            let mut group: Vec<usize> = vec![cb_idx];
            grouped.insert(cb_idx);

            // Try horizontal direction
            let mut found_horizontal = true;
            let mut last_field_bounds = cb_field_bounds;
            let mut last_group_bounds = cb_group_bounds;

            while found_horizontal {
                found_horizontal = false;

                for &candidate_idx in checkboxes {
                    if grouped.contains(&candidate_idx) {
                        continue;
                    }

                    let Some(candidate_field_bounds) = self.get_field_bounds(doc, candidate_idx)
                    else {
                        continue;
                    };

                    if last_field_bounds
                        .is_horizontally_aligned(&candidate_field_bounds, self.alignment_tolerance)
                    {
                        let candidate_group_bounds = doc
                            .get_bounds(candidate_idx)
                            .unwrap_or(candidate_field_bounds);
                        let distance = last_group_bounds
                            .horizontal_gap_to(&candidate_group_bounds)
                            .unwrap_or(Decimal::MAX);

                        if distance <= self.max_horizontal_gap
                            && !self.has_elements_between(
                                doc,
                                &last_field_bounds,
                                &candidate_field_bounds,
                                &checkbox_set,
                            )
                        {
                            group.push(candidate_idx);
                            grouped.insert(candidate_idx);
                            last_field_bounds = candidate_field_bounds;
                            last_group_bounds = candidate_group_bounds;
                            found_horizontal = true;
                            break;
                        }
                    }
                }
            }

            // Try vertical direction
            let inset_threshold = Decimal::from_str("10.0").unwrap();
            let mut found_vertical = true;
            let mut last_field_bounds = cb_field_bounds;

            while found_vertical {
                found_vertical = false;

                for &candidate_idx in checkboxes {
                    if grouped.contains(&candidate_idx) {
                        continue;
                    }

                    let Some(candidate_field_bounds) = self.get_field_bounds(doc, candidate_idx)
                    else {
                        continue;
                    };

                    if last_field_bounds
                        .is_vertically_aligned(&candidate_field_bounds, self.alignment_tolerance)
                    {
                        let distance = last_field_bounds
                            .vertical_gap_to(&candidate_field_bounds)
                            .unwrap_or(Decimal::MAX);

                        let has_blocking = self.has_non_inset_elements_between(
                            doc,
                            &last_field_bounds,
                            &candidate_field_bounds,
                            &checkbox_set,
                            inset_threshold,
                        );

                        let max_gap = if has_blocking {
                            self.max_vertical_gap
                        } else {
                            Decimal::from_str("500.0").unwrap()
                        };

                        if distance <= max_gap && !has_blocking {
                            group.push(candidate_idx);
                            grouped.insert(candidate_idx);
                            last_field_bounds = candidate_field_bounds;
                            found_vertical = true;
                            break;
                        }
                    }
                }
            }

            if group.len() > 1 {
                doc.merge_inferred(group, GroupKind::CheckboxGroup, self.name());
            }
        }
    }
}

impl AnalysisModule for CheckboxGrouper {
    fn name(&self) -> &'static str {
        "CheckboxGrouper"
    }

    fn process(&self, doc: &mut Document) {
        let roots = doc.roots();
        let checkboxes: Vec<usize> = roots
            .iter()
            .filter(|&&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::Checkbox { .. })
                )
            })
            .copied()
            .collect();

        if checkboxes.is_empty() {
            return;
        }

        self.group_aligned_checkboxes(doc, &checkboxes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::modules::{CheckboxDetector, FieldGrouper, TextBlockGrouper};
    use crate::document::{Document, GroupKind};
    use crate::flattened::{Flattened, FlattenedNode, Hint, Page, WidgetKind};
    use crate::xfa::num;

    fn new_checkbox_node(name: &str, x: f64, y: f64) -> FlattenedNode {
        let mut node = FlattenedNode::new_field(
            name.to_string(),
            "".to_string(),
            "".to_string(),
            num(x),
            num(y),
            num(12.0),
            num(12.0),
        );
        node.add_hint(Hint::WidgetType(WidgetKind::Checkbox));
        node
    }

    #[test]
    fn test_vertical_checkbox_grouping() {
        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                new_checkbox_node("cb1", 50.0, 100.0),
                FlattenedNode::new_text(
                    "Option A".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(66.0),
                    num(100.0),
                    num(60.0),
                    num(12.0),
                ),
                new_checkbox_node("cb2", 50.0, 120.0),
                FlattenedNode::new_text(
                    "Option B".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(66.0),
                    num(120.0),
                    num(60.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);

        FieldGrouper::new().process(&mut doc);
        TextBlockGrouper::new().process(&mut doc);
        CheckboxDetector::new().process(&mut doc);
        CheckboxGrouper::new().process(&mut doc);

        let checkbox_groups = doc.find_groups(|k| matches!(k, GroupKind::CheckboxGroup));
        assert_eq!(checkbox_groups.len(), 1);
        let group = doc.get_group(checkbox_groups[0]).unwrap();
        assert_eq!(group.children.len(), 2);
    }

    #[test]
    fn test_horizontal_checkbox_grouping() {
        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                new_checkbox_node("cb1", 50.0, 100.0),
                FlattenedNode::new_text(
                    "Yes".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(65.0),
                    num(100.0),
                    num(30.0),
                    num(12.0),
                ),
                // cb2 placed 20pt after cb1's label ends (~82pt) so gap ≈ 18pt ≤ 50pt
                new_checkbox_node("cb2", 100.0, 100.0),
                FlattenedNode::new_text(
                    "No".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(115.0),
                    num(100.0),
                    num(30.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);

        FieldGrouper::new().process(&mut doc);
        TextBlockGrouper::new().process(&mut doc);
        CheckboxDetector::new().process(&mut doc);
        CheckboxGrouper::new().process(&mut doc);

        let checkbox_groups = doc.find_groups(|k| matches!(k, GroupKind::CheckboxGroup));
        assert_eq!(checkbox_groups.len(), 1);
        let group = doc.get_group(checkbox_groups[0]).unwrap();
        assert_eq!(group.children.len(), 2);
    }
}
