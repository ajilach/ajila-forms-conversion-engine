//! Radio button grouper module.
//!
//! Groups radio buttons that are on the same line (horizontally or vertically)
//! into RadioButtonGroup groups. Grouping stops if another element is in between.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::{Bounds, Hint};
use crate::xfa::scripting::SomPath;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::{HashMap, HashSet};

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

    /// Get the exclGroup SOM path for a radio button, if it has one.
    fn get_excl_group_path(&self, doc: &Document, rb_idx: usize) -> Option<SomPath> {
        let group = doc.get_group(rb_idx)?;
        if let GroupKind::RadioButton { field, .. } = &group.kind {
            // 'field' is an index into the children vec
            let field_group_idx = *group.children.get(*field)?;
            // Use collect_nodes to get all nodes in this group subtree
            let nodes = doc.collect_nodes(field_group_idx);
            for node in &nodes {
                for hint in &node.hints {
                    if let Hint::ExclGroupSomPath(path) = hint {
                        return Some(path.clone());
                    }
                }
            }
        }
        None
    }

    /// Get the bounds of just the *field* (radio-button circle) part of a RadioButton group,
    /// ignoring the label.  Used for alignment checks where the whole-group bounds —
    /// which include the potentially wide label text — would give misleading results.
    fn get_field_bounds(&self, doc: &Document, rb_idx: usize) -> Option<Bounds> {
        let group = doc.get_group(rb_idx)?;
        if let GroupKind::RadioButton { field, .. } = &group.kind {
            let field_group_idx = *group.children.get(*field)?;
            return doc.get_bounds(field_group_idx);
        }
        None
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

    /// Check if there are any non-inset elements between two vertically aligned radio buttons.
    /// Inset elements (further to the right than the radio buttons) are allowed between them.
    /// This also considers that elements between radio buttons might be:
    /// - Other radio buttons (which we skip)
    /// - Content that is visually inset relative to its own radio button
    fn has_non_inset_elements_between(
        &self,
        doc: &Document,
        bounds1: &Bounds,
        bounds2: &Bounds,
        radio_button_indices: &HashSet<usize>,
        inset_threshold: Decimal,
    ) -> bool {
        // Create bounding box that encompasses both radio buttons
        let region = bounds1.union(bounds2);

        // The minimum left edge of the radio buttons - content must be to the right of this
        let rb_left = bounds1.left().min(bounds2.left());

        // Also consider the right edge of radio button fields - content that starts
        // after the field (to the right of the checkbox itself) is likely a label, not blocking content
        let rb_right = bounds1.right().max(bounds2.right());

        // Build a set of all child indices of radio buttons (labels, etc.)
        let mut radio_button_children: HashSet<usize> = HashSet::new();
        for &rb_idx in radio_button_indices {
            if let Some(group) = doc.get_group(rb_idx) {
                for &child_idx in &group.children {
                    radio_button_children.insert(child_idx);
                }
            }
        }

        // Check all root groups
        for root_idx in doc.roots() {
            // Skip if it's one of the radio buttons we're checking
            if radio_button_indices.contains(&root_idx) {
                continue;
            }

            // Skip if it's a child of one of the radio buttons (e.g., a label)
            if radio_button_children.contains(&root_idx) {
                continue;
            }

            // Get bounds of this element
            let Some(bounds) = doc.get_bounds(root_idx) else {
                continue;
            };

            // Check if this element overlaps with the region between the two radio buttons
            if region.overlaps(&bounds) {
                // Check if the element is inset (to the right of the radio buttons)
                // An element is considered inset if:
                // 1. Its left edge is to the right of the radio buttons' left edge by the threshold, OR
                // 2. Its left edge is past the radio buttons' right edge (it's a label-like element)
                let is_inset =
                    bounds.left() >= rb_left + inset_threshold || bounds.left() >= rb_right;

                if !is_inset {
                    // Check if this element is itself a RadioButton or RadioButtonContent
                    // (which would have its own inset content below it)
                    let is_radio_related = doc
                        .get_group(root_idx)
                        .map(|g| matches!(g.kind, GroupKind::RadioButton { .. }))
                        .unwrap_or(false);

                    if !is_radio_related {
                        return true; // Found a non-inset, non-radio element in between
                    }
                }
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
            // For vertical grouping, we allow a larger gap if the content between is inset
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

                    // Check if vertically aligned
                    if self.are_vertically_aligned(&last_bounds, &candidate_bounds) {
                        let distance = self.vertical_distance(&last_bounds, &candidate_bounds);
                        let inset_threshold = Decimal::from_str("10.0").unwrap();

                        // Check if there's only inset content between (no blocking elements)
                        let has_blocking_elements = self.has_non_inset_elements_between(
                            doc,
                            &last_bounds,
                            &candidate_bounds,
                            &radio_button_set,
                            inset_threshold,
                        );

                        // If there are no blocking elements, allow a much larger gap
                        // (to account for inset content below radio buttons)
                        let max_gap = if has_blocking_elements {
                            self.max_vertical_gap
                        } else {
                            // Allow up to 500pt gap when only inset content is between
                            Decimal::from_str("500.0").unwrap()
                        };

                        if distance <= max_gap && !has_blocking_elements {
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

    /// Return the last segment of an ExclGroupSomPath (the group name itself, without the
    /// subform/chapter prefix).  E.g. `"Form.Page1.Ch2.RB_Group_Foo"` → `"RB_Group_Foo"`.
    fn excl_group_name(path: &SomPath) -> &str {
        path.as_str().rsplit('.').next().unwrap_or(path.as_str())
    }

    /// Group radio buttons by their exclGroup SOM path.
    /// All radio buttons in the same exclGroup are grouped together regardless of position.
    fn group_by_excl_group(&self, doc: &mut Document, radio_buttons: &[usize]) {
        // Build a map of exclGroup path to radio button indices
        let mut excl_groups: HashMap<SomPath, Vec<usize>> = HashMap::new();

        for &rb_idx in radio_buttons {
            if let Some(path) = self.get_excl_group_path(doc, rb_idx) {
                excl_groups.entry(path).or_default().push(rb_idx);
            }
        }

        // Sort by key (SOM path) to ensure deterministic group ordering —
        // HashMap iteration order is non-deterministic.
        let mut sorted_groups: Vec<_> = excl_groups.into_iter().collect();
        sorted_groups.sort_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()));

        // Create RadioButtonGroup for each exclGroup with more than one radio button
        for (_path, group) in sorted_groups {
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

    /// Fallback grouping: group radio buttons that share the same ExclGroup *name*
    /// (last segment of their ExclGroupSomPath) and are spatially aligned.
    ///
    /// This handles XFA forms where the same logical radio group is split across
    /// separate subforms/chapters (each with its own full SOM path but the same
    /// group name).  We require spatial alignment as a safety guard against
    /// accidentally merging unrelated groups from different form pages.
    fn group_by_excl_group_name(&self, doc: &mut Document, radio_buttons: &[usize]) {
        // Build a map of group name → radio button indices
        let mut name_groups: HashMap<String, Vec<usize>> = HashMap::new();

        for &rb_idx in radio_buttons {
            if let Some(path) = self.get_excl_group_path(doc, rb_idx) {
                let name = Self::excl_group_name(&path).to_string();
                name_groups.entry(name).or_default().push(rb_idx);
            }
        }

        // Sort deterministically
        let mut sorted: Vec<_> = name_groups.into_iter().collect();
        sorted.sort_by(|(a, _), (b, _)| a.cmp(b));

        for (_name, candidates) in sorted {
            if candidates.len() < 2 {
                continue;
            }

            // Among the candidates, find pairs that are spatially aligned.
            // We use a simple greedy union-find: any two buttons that are
            // vertically or horizontally aligned (within tolerance) go into the
            // same group.
            let mut assigned: HashSet<usize> = HashSet::new();
            let mut groups: Vec<Vec<usize>> = Vec::new();

            for &rb_idx in &candidates {
                if assigned.contains(&rb_idx) {
                    continue;
                }
                // Use the field (circle) bounds for alignment, not the full group bounds
                // (which include the label and would skew horizontal-center comparisons).
                let Some(rb_field_bounds) = self.get_field_bounds(doc, rb_idx) else {
                    continue;
                };

                let mut group = vec![rb_idx];
                assigned.insert(rb_idx);

                for &other_idx in &candidates {
                    if assigned.contains(&other_idx) {
                        continue;
                    }
                    let Some(other_field_bounds) = self.get_field_bounds(doc, other_idx) else {
                        continue;
                    };

                    if self.are_vertically_aligned(&rb_field_bounds, &other_field_bounds)
                        || self.are_horizontally_aligned(&rb_field_bounds, &other_field_bounds)
                    {
                        group.push(other_idx);
                        assigned.insert(other_idx);
                    }
                }

                if group.len() > 1 {
                    groups.push(group);
                }
            }

            for group in groups {
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

        // First, group by exclGroup (XFA semantic grouping)
        self.group_by_excl_group(doc, &radio_buttons);

        // Then, group any remaining ungrouped radio buttons by alignment
        let roots = doc.roots();
        let remaining_radio_buttons: Vec<usize> = roots
            .iter()
            .filter(|&&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::RadioButton { .. })
                )
            })
            .copied()
            .collect();

        if !remaining_radio_buttons.is_empty() {
            self.group_aligned_radio_buttons(doc, &remaining_radio_buttons);
        }

        // Finally, group any still-ungrouped radio buttons by ExclGroup NAME (last path segment).
        // This handles forms where the same logical radio group spans multiple XFA subforms/chapters,
        // giving each button a different full SOM path but the same group name.
        let roots = doc.roots();
        let still_remaining: Vec<usize> = roots
            .iter()
            .filter(|&&idx| {
                matches!(
                    doc.get_group(idx).map(|g| &g.kind),
                    Some(GroupKind::RadioButton { .. })
                )
            })
            .copied()
            .collect();

        if !still_remaining.is_empty() {
            self.group_by_excl_group_name(doc, &still_remaining);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::modules::{FieldGrouper, RadioButtonDetector, TextBlockGrouper};
    use crate::document::{Document, GroupKind};
    use crate::flattened::{Flattened, FlattenedNode, Page};
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
