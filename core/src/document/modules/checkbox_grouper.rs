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
use crate::flattened::FlattenedNodeKind;
use rust_decimal::prelude::*;
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
    /// Maximum vertical gap for sibling checkboxes (with blocking content between)
    pub max_sibling_gap: Decimal,
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
            max_sibling_gap: Decimal::from_str("300.0").unwrap(),
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

    /// Get the SOM path of a checkbox's field.
    fn get_checkbox_som_path(
        &self,
        doc: &Document,
        cb_idx: usize,
    ) -> Option<crate::xfa::scripting::SomPath> {
        let group = doc.get_group(cb_idx)?;
        if let GroupKind::Checkbox { field, .. } = &group.kind {
            let field_group_idx = *group.children.get(*field)?;
            return doc.som_path(field_group_idx);
        }
        None
    }

    /// Get the label text of a checkbox.
    fn get_checkbox_label(&self, doc: &Document, cb_idx: usize) -> Option<String> {
        let group = doc.get_group(cb_idx)?;
        if let GroupKind::Checkbox { label, .. } = &group.kind {
            let label_group_idx = *group.children.get(*label)?;
            let text = doc.get_text_content(label_group_idx);
            if text.is_empty() { None } else { Some(text) }
        } else {
            None
        }
    }

    /// Returns true if a group or any descendant is radio-related conditional content.
    fn is_radio_related_group_or_descendant(&self, doc: &Document, group_idx: usize) -> bool {
        let mut stack = vec![group_idx];
        let mut visited = HashSet::new();

        while let Some(idx) = stack.pop() {
            if !visited.insert(idx) {
                continue;
            }

            let Some(group) = doc.get_group(idx) else {
                continue;
            };

            if matches!(
                group.kind,
                GroupKind::RadioButtonGroup
                    | GroupKind::RadioButton { .. }
                    | GroupKind::ExclGroup { .. }
            ) {
                return true;
            }

            stack.extend(group.children.iter().copied());
        }

        false
    }

    /// Check if two checkboxes are in the same form section based on SOM paths.
    ///
    /// Two checkboxes are "siblings" if their SOM paths have the same length and
    /// differ in exactly one segment (excluding the leaf). This indicates they are
    /// parallel instances of the same checkbox template within a section, with
    /// conditional content between them.
    fn are_section_siblings(&self, doc: &Document, cb1_idx: usize, cb2_idx: usize) -> bool {
        let Some(path1) = self.get_checkbox_som_path(doc, cb1_idx) else {
            return false;
        };
        let Some(path2) = self.get_checkbox_som_path(doc, cb2_idx) else {
            return false;
        };

        let segments1: Vec<&str> = path1.as_str().split('.').collect();
        let segments2: Vec<&str> = path2.as_str().split('.').collect();

        // Must be same length and differ
        if segments1.len() != segments2.len() || path1.as_str() == path2.as_str() {
            return false;
        }

        // The leaf (field name) must be identical — true conditional siblings
        // share the same field template name (e.g., "CB_Checkbox") placed in
        // different container sections.
        if segments1.last() != segments2.last() {
            return false;
        }

        // Count differing segments (excluding the leaf field name)
        let differing_count = segments1
            .iter()
            .zip(segments2.iter())
            .take(segments1.len().saturating_sub(1)) // exclude leaf
            .filter(|(a, b)| a != b)
            .count();

        // Exactly one differing non-leaf segment = sibling checkboxes
        differing_count == 1
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
    ///
    /// Zero-width elements and other checkbox groups are also skipped.
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

        // Collect overlapping roots once so we can do a two-pass classification.
        let mut overlapping: Vec<usize> = Vec::new();
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
            if bounds.width.is_zero() {
                continue;
            }
            if region.overlaps(&bounds) {
                overlapping.push(root_idx);
            }
        }

        // Detect whether the gap already contains evidence of conditional
        // content for the upper checkbox.
        let has_inset_conditional_content = overlapping.iter().any(|&root_idx| {
            let Some(bounds) = doc.get_bounds(root_idx) else {
                return false;
            };
            let Some(group) = doc.get_group(root_idx) else {
                return false;
            };
            let is_radio_related = self.is_radio_related_group_or_descendant(doc, root_idx);
            if is_radio_related {
                return true;
            }

            let is_inset = bounds.left() >= cb_left + inset_threshold || bounds.left() >= cb_right;
            if !is_inset {
                return false;
            }
            matches!(
                group.kind,
                GroupKind::RadioButtonGroup
                    | GroupKind::RadioButton { .. }
                    | GroupKind::ExclGroup { .. }
                    | GroupKind::Field
                    | GroupKind::LabeledField { .. }
                    | GroupKind::InlineField { .. }
                    | GroupKind::InlineDateField { .. }
                    | GroupKind::DateField { .. }
            )
        });

        for &root_idx in &overlapping {
            let bounds = doc.get_bounds(root_idx).unwrap();
            let group_kind = doc.get_group(root_idx).map(|g| &g.kind);

            let is_checkbox_related = group_kind
                .map(|k| matches!(k, GroupKind::Checkbox { .. } | GroupKind::CheckboxGroup))
                .unwrap_or(false);

            let is_radio_related = self.is_radio_related_group_or_descendant(doc, root_idx);

            if is_checkbox_related || is_radio_related {
                continue;
            }

            let is_inset = bounds.left() >= cb_left + inset_threshold || bounds.left() >= cb_right;

            if is_inset {
                continue;
            }

            // Decorative draw elements (empty-content Text/Draw nodes such as
            // background rectangles, highlight boxes, or separator rules) carry
            // no real content and must not block grouping. The visual boxes
            // drawn behind checkbox option rows are a common example.
            let nodes = doc.collect_nodes(root_idx);
            let is_decorative_empty_draw = !nodes.is_empty()
                && nodes.iter().all(|node| {
                    matches!(
                        &node.kind,
                        FlattenedNodeKind::Text { content, .. } if content.trim().is_empty()
                    )
                });
            if is_decorative_empty_draw {
                continue;
            }

            // If the gap clearly contains conditional content for the upper
            // checkbox (an inset field/radio group), treat full-width text
            // drawings inside the gap as labels for that conditional content
            // rather than as blocking elements. Such labels often span the
            // full container width visually even though their text margin is
            // inset.
            if has_inset_conditional_content {
                let is_text_only = group_kind
                    .map(|k| {
                        matches!(
                            k,
                            GroupKind::TextBlock
                                | GroupKind::Paragraph
                                | GroupKind::Heading { .. }
                                | GroupKind::Leaf { .. }
                        )
                    })
                    .unwrap_or(false);
                if is_text_only {
                    continue;
                }
            }

            return true;
        }

        false
    }

    /// Detects whether the gap between two vertically aligned checkboxes
    /// contains inset conditional content (e.g. a nested radio group or
    /// other field whose left edge is inset relative to the checkbox column).
    /// Used to allow a larger vertical gap when grouping checkboxes whose
    /// own conditional content sits between them.
    fn has_inset_content_between(
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
            if bounds.width.is_zero() {
                continue;
            }
            if !region.overlaps(&bounds) {
                continue;
            }
            if let Some(group) = doc.get_group(root_idx) {
                if self.is_radio_related_group_or_descendant(doc, root_idx) {
                    return true;
                }

                let is_inset =
                    bounds.left() >= cb_left + inset_threshold || bounds.left() >= cb_right;
                if !is_inset {
                    continue;
                }

                if matches!(
                    group.kind,
                    GroupKind::RadioButtonGroup
                        | GroupKind::RadioButton { .. }
                        | GroupKind::ExclGroup { .. }
                        | GroupKind::Field
                        | GroupKind::LabeledField { .. }
                        | GroupKind::InlineField { .. }
                        | GroupKind::InlineDateField { .. }
                        | GroupKind::DateField { .. }
                ) {
                    return true;
                }
            }
        }

        false
    }

    /// Returns true if a radio-related group (or descendant) overlaps the
    /// region between two checkboxes, regardless of inset position.
    fn has_radio_related_between(
        &self,
        doc: &Document,
        bounds1: &Bounds,
        bounds2: &Bounds,
        checkbox_indices: &HashSet<usize>,
    ) -> bool {
        let region = bounds1.union(bounds2);

        let mut checkbox_children: HashSet<usize> = HashSet::new();
        for &cb_idx in checkbox_indices {
            if let Some(group) = doc.get_group(cb_idx) {
                for &child_idx in &group.children {
                    checkbox_children.insert(child_idx);
                }
            }
        }

        for root_idx in doc.roots() {
            if checkbox_indices.contains(&root_idx) || checkbox_children.contains(&root_idx) {
                continue;
            }
            let Some(bounds) = doc.get_bounds(root_idx) else {
                continue;
            };
            if !region.overlaps(&bounds) {
                continue;
            }
            if self.is_radio_related_group_or_descendant(doc, root_idx) {
                return true;
            }
        }

        false
    }

    /// Group spatially aligned checkboxes together.
    ///
    /// A seed checkbox is first extended horizontally and then vertically to
    /// build the row/column "spine" of a group. Afterwards a horizontal-fill
    /// pass extends *every* member rightward along its own row, so that
    /// multi-column ("grid") layouts are fully captured even when the columns
    /// are ragged (the second-column checkbox's X depends on the first
    /// column's label width and therefore does not line up vertically). Because
    /// the fill only ever adds same-row neighbours, it cannot bridge separate
    /// groups that are stacked vertically.
    fn group_aligned_checkboxes(&self, doc: &mut Document, checkboxes: &[usize]) {
        if checkboxes.is_empty() {
            return;
        }

        let inset_threshold = Decimal::from_str("10.0").unwrap();
        let mut grouped: HashSet<usize> = HashSet::new();
        let checkbox_set: HashSet<usize> = checkboxes.iter().copied().collect();

        // Whether `candidate` sits in the same row as `member` and is close
        // enough horizontally (with nothing in between) to be grouped.
        let horizontally_adjacent = |doc: &Document,
                                     member_idx: usize,
                                     candidate_field_bounds: &Bounds,
                                     candidate_group_bounds: &Bounds|
         -> bool {
            let Some(member_field_bounds) = self.get_field_bounds(doc, member_idx) else {
                return false;
            };
            if !member_field_bounds
                .is_horizontally_aligned(candidate_field_bounds, self.alignment_tolerance)
            {
                return false;
            }
            let member_group_bounds = doc.get_bounds(member_idx).unwrap_or(member_field_bounds);
            let distance = member_group_bounds
                .horizontal_gap_to(candidate_group_bounds)
                .unwrap_or(Decimal::MAX);
            distance <= self.max_horizontal_gap
                && !self.has_elements_between(
                    doc,
                    &member_field_bounds,
                    candidate_field_bounds,
                    &checkbox_set,
                )
        };

        for &seed_idx in checkboxes {
            if grouped.contains(&seed_idx) {
                continue;
            }

            let Some(cb_field_bounds) = self.get_field_bounds(doc, seed_idx) else {
                continue;
            };
            let cb_group_bounds = doc.get_bounds(seed_idx).unwrap_or(cb_field_bounds);

            let mut group: Vec<usize> = vec![seed_idx];
            grouped.insert(seed_idx);

            // 1. Extend horizontally along the seed's row.
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
                    let candidate_group_bounds = doc
                        .get_bounds(candidate_idx)
                        .unwrap_or(candidate_field_bounds);

                    if last_field_bounds
                        .is_horizontally_aligned(&candidate_field_bounds, self.alignment_tolerance)
                    {
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

            // 2. Extend vertically along the seed's column.
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
                        let has_inset_content = self.has_inset_content_between(
                            doc,
                            &last_field_bounds,
                            &candidate_field_bounds,
                            &checkbox_set,
                            inset_threshold,
                        );
                        let has_radio_conditional_between = self.has_radio_related_between(
                            doc,
                            &last_field_bounds,
                            &candidate_field_bounds,
                            &checkbox_set,
                        );

                        let should_group = if !has_blocking {
                            distance <= self.max_vertical_gap
                                || (has_inset_content && distance <= self.max_sibling_gap)
                        } else {
                            distance <= self.max_sibling_gap
                                && (self.are_section_siblings(
                                    doc,
                                    *group.last().unwrap(),
                                    candidate_idx,
                                ) || has_radio_conditional_between)
                        };

                        if should_group {
                            group.push(candidate_idx);
                            grouped.insert(candidate_idx);
                            last_field_bounds = candidate_field_bounds;
                            found_vertical = true;
                            break;
                        }
                    }
                }
            }

            // 3. Horizontal-fill pass: extend every member rightward along its
            // own row. This captures the second (and further) columns of a
            // multi-column bucket even when the columns are ragged and do not
            // line up vertically. Iterates to a fixpoint so each newly added
            // cell can itself anchor further horizontal neighbours. Only
            // same-row neighbours are added, so vertically-stacked groups are
            // never merged.
            let mut changed = true;
            while changed {
                changed = false;
                'candidate: for &candidate_idx in checkboxes {
                    if grouped.contains(&candidate_idx) {
                        continue;
                    }
                    let Some(candidate_field_bounds) = self.get_field_bounds(doc, candidate_idx)
                    else {
                        continue;
                    };
                    let candidate_group_bounds = doc
                        .get_bounds(candidate_idx)
                        .unwrap_or(candidate_field_bounds);

                    for &member_idx in &group {
                        if horizontally_adjacent(
                            doc,
                            member_idx,
                            &candidate_field_bounds,
                            &candidate_group_bounds,
                        ) {
                            group.push(candidate_idx);
                            grouped.insert(candidate_idx);
                            changed = true;
                            continue 'candidate;
                        }
                    }
                }
            }

            if group.len() > 1 {
                // Sort members into reading order (row by row, left to right) so
                // grid layouts produce options in the expected sequence. Mirrors
                // `compare_bounds_reading_order`: quantize Y into 4.0pt bands,
                // then order by X.
                let band = Decimal::new(40, 1);
                group.sort_by(|&a, &b| {
                    let ba = self.get_field_bounds(doc, a);
                    let bb = self.get_field_bounds(doc, b);
                    match (ba, bb) {
                        (Some(ba), Some(bb)) => {
                            let qa = (ba.y / band).round() * band;
                            let qb = (bb.y / band).round() * band;
                            qa.cmp(&qb).then_with(|| ba.x.cmp(&bb.x))
                        }
                        _ => std::cmp::Ordering::Equal,
                    }
                });

                // Don't group checkboxes that all have the same label text.
                // Identical labels (e.g., multiple "Ja" checkboxes) indicate
                // independent yes/no fields for different questions in a
                // tabular layout, not options within a single group.
                let labels: Vec<Option<String>> = group
                    .iter()
                    .map(|&idx| self.get_checkbox_label(doc, idx))
                    .collect();

                let all_same_label =
                    labels.iter().all(|l| l.is_some()) && labels.windows(2).all(|w| w[0] == w[1]);

                if !all_same_label {
                    doc.merge_inferred(group, GroupKind::CheckboxGroup, self.name());
                }
            }
        }
    }
}

impl AnalysisModule for CheckboxGrouper {
    fn name(&self) -> &'static str {
        "CheckboxGrouper"
    }

    fn process(&self, doc: &mut Document) {
        // Use all checkbox groups, not only roots. Conditional container
        // wrappers can claim one checkbox while sibling options stay unclaimed;
        // restricting to roots would then miss valid grouping candidates.
        let checkboxes = doc.find_groups(|k| matches!(k, GroupKind::Checkbox { .. }));

        if checkboxes.is_empty() {
            return;
        }

        self.group_aligned_checkboxes(doc, &checkboxes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::modules::{
        CheckboxDetector, FieldGrouper, RadioButtonDetector, RadioButtonGrouper, TextBlockGrouper,
    };
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

    fn new_radio_node(name: &str, x: f64, y: f64) -> FlattenedNode {
        let mut node = FlattenedNode::new_field(
            name.to_string(),
            "".to_string(),
            "".to_string(),
            num(x),
            num(y),
            num(12.0),
            num(12.0),
        );
        node.add_hint(Hint::WidgetType(WidgetKind::Radio));
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

    #[test]
    fn test_checkbox_grouping_across_conditional_radio_content() {
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
                    num(80.0),
                    num(12.0),
                ),
                new_checkbox_node("cb2", 50.0, 122.0),
                FlattenedNode::new_text(
                    "Option B".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(66.0),
                    num(122.0),
                    num(80.0),
                    num(12.0),
                ),
                // Full-width explanatory line inside the conditional block.
                FlattenedNode::new_text(
                    "Conditional details".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(40.0),
                    num(142.0),
                    num(220.0),
                    num(12.0),
                ),
                // Nested radio controls in the conditional section.
                new_radio_node("rb1", 50.0, 158.0),
                FlattenedNode::new_text(
                    "Single".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(66.0),
                    num(158.0),
                    num(50.0),
                    num(12.0),
                ),
                new_radio_node("rb2", 50.0, 176.0),
                FlattenedNode::new_text(
                    "Joint".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(66.0),
                    num(176.0),
                    num(50.0),
                    num(12.0),
                ),
                new_checkbox_node("cb3", 50.0, 198.0),
                FlattenedNode::new_text(
                    "Option C".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(66.0),
                    num(198.0),
                    num(80.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);

        FieldGrouper::new().process(&mut doc);
        TextBlockGrouper::new().process(&mut doc);
        RadioButtonDetector::new().process(&mut doc);
        CheckboxDetector::new().process(&mut doc);
        RadioButtonGrouper::new().process(&mut doc);
        CheckboxGrouper::new().process(&mut doc);

        let checkbox_groups = doc.find_groups(|k| matches!(k, GroupKind::CheckboxGroup));
        assert_eq!(checkbox_groups.len(), 1);
        let group = doc.get_group(checkbox_groups[0]).unwrap();
        assert_eq!(group.children.len(), 3);
    }
}
