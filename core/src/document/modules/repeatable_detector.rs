//! Repeatable section detector module.
//!
//! Detects repeatable sections (dynamic arrays/tables) based on XFA occurrence hints.
//! These are form sections that can have multiple instances at runtime, such as:
//! - Line items in an invoice
//! - Multiple addresses
//! - Repeating data entry rows
//!
//! Per XFA 3.3 spec (Chapter 9, "The Occur Element"):
//! - `min`: minimum number of occurrences required
//! - `max`: maximum number permitted (-1 = unlimited)
//! - A section is "repeatable" if max > 1 or max is unlimited

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use crate::flattened::Bounds;
use rust_decimal::Decimal;
use std::collections::HashSet;

type Num = Decimal;

/// Information about a detected repeatable section.
#[derive(Debug, Clone)]
pub struct RepeatableSection {
    /// Group indices that belong to this repeatable section
    pub member_groups: Vec<usize>,
    /// Minimum occurrences required
    pub min_occurrences: u32,
    /// Maximum occurrences allowed (None = unlimited)
    pub max_occurrences: Option<u32>,
    /// Bounding box of all members combined
    pub bounds: Option<Bounds>,
    /// Node indices that belong to this repeatable section (from FlattenedKind::Group).
    /// Used to determine which Document groups should be included based on structural
    /// containment rather than spatial containment.
    pub node_indices: Vec<usize>,
    /// Whether this section is user-repeatable (has add/remove buttons).
    /// Sections without buttons are script-managed (e.g., signature blocks)
    /// and should be preserved as separate groups but not as repeatables.
    pub is_user_repeatable: bool,
}

/// Detects repeatable sections by examining Hint::Occurrence on flattened nodes.
///
/// Repeatable sections are identified by:
/// 1. Having Hint::Occurrence attached to their flattened nodes
/// 2. The max occurrence being > 1 or unlimited (-1)
///
/// This module groups together adjacent nodes that share the same occurrence hint,
/// creating a RepeatableSection group that can be used by downstream analysis.
pub struct RepeatableDetector {
    /// Whether to group spatially adjacent repeatable items together
    pub group_adjacent: bool,
    /// Maximum vertical gap to consider items as part of same repeatable section
    pub max_vertical_gap: Decimal,
}

impl Default for RepeatableDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl RepeatableDetector {
    pub fn new() -> Self {
        RepeatableDetector {
            group_adjacent: true,
            max_vertical_gap: Decimal::from_str_exact("20.0").unwrap(),
        }
    }

    /// Configure whether to group spatially adjacent repeatable items.
    pub fn with_group_adjacent(mut self, group: bool) -> Self {
        self.group_adjacent = group;
        self
    }

    /// Configure maximum vertical gap for grouping.
    pub fn with_max_vertical_gap(mut self, gap: Decimal) -> Self {
        self.max_vertical_gap = gap;
        self
    }

    /// Extract occurrence hint from the flattened structure.
    /// Searches for Occurrence hints on the specific leaf node for the given group.
    fn get_occurrence_hint(&self, doc: &Document, group_idx: usize) -> Option<(u32, Option<u32>)> {
        doc.occurrence(group_idx)
    }

    /// Find repeatable groups from the flattened structure.
    /// This searches FlattenedKind::Group elements for Occurrence hints.
    fn find_repeatable_from_flattened(&self, doc: &Document) -> Vec<RepeatableSection> {
        let mut sections = Vec::new();

        // Collect all button y-ranges from the entire flattened tree.
        // XFA add/remove buttons (PlusMinus) may be at any nesting level;
        // we match them to repeatable groups by vertical overlap.
        let button_y_ranges = collect_button_y_ranges(&doc.source.children);

        /// Search for groups with Occurrence hints and track node indices.
        /// `current_index` tracks the current position in the flattened node iteration order.
        fn search_groups(
            children: &[crate::flattened::FlattenedKind],
            sections: &mut Vec<RepeatableSection>,
            current_index: &mut usize,
            button_y_ranges: &[(Num, Num)],
        ) {
            for child in children {
                match child {
                    crate::flattened::FlattenedKind::Group {
                        hints,
                        children: group_children,
                        ..
                    } => {
                        // Check if this group has an Occurrence hint
                        let occur_hint = hints.iter().find_map(|h| {
                            if let crate::flattened::Hint::Occurrence { min, max } = h {
                                Some((*min, *max))
                            } else {
                                None
                            }
                        });

                        if let Some((min, max)) = occur_hint {
                            let is_repeatable = max.map(|m| m > 1).unwrap_or(true);
                            let has_nested_repeatable =
                                contains_nested_repeatable_group(group_children);
                            let has_button = group_contains_button(group_children);
                            let is_xfa = group_has_som_paths(group_children);
                            // For XFA forms: require a spatially nearby button.
                            // Buttons must overlap the group's y-range to count.
                            let nearby_button = if !has_button && is_xfa {
                                if let Some(bounds) = calculate_group_bounds(group_children) {
                                    button_y_ranges.iter().any(|(btn_y_min, btn_y_max)| {
                                        *btn_y_min < bounds.y + bounds.height
                                            && *btn_y_max > bounds.y
                                    })
                                } else {
                                    false
                                }
                            } else {
                                false
                            };
                            let passes_button_check = !is_xfa || has_button || nearby_button;
                            if is_repeatable
                                && group_contains_interactive_field(group_children)
                                && !has_nested_repeatable
                            {
                                let start_index = *current_index;
                                let node_count = count_nodes(group_children);
                                let node_indices: Vec<usize> =
                                    (start_index..start_index + node_count).collect();

                                let bounds = calculate_group_bounds(group_children);

                                sections.push(RepeatableSection {
                                    member_groups: vec![],
                                    min_occurrences: min,
                                    max_occurrences: max,
                                    bounds,
                                    node_indices,
                                    is_user_repeatable: passes_button_check,
                                });
                            }
                        }
                        // Recurse into children (this also advances current_index)
                        search_groups(group_children, sections, current_index, button_y_ranges);
                    }
                    crate::flattened::FlattenedKind::Node(_) => {
                        *current_index += 1;
                    }
                }
            }
        }

        /// Check if a FlattenedKind tree contains at least one interactive field.
        /// This keeps pagination/container wrappers with no real input controls
        /// from being misclassified as repeatables.
        fn group_contains_interactive_field(children: &[crate::flattened::FlattenedKind]) -> bool {
            for child in children {
                match child {
                    crate::flattened::FlattenedKind::Node(node) => {
                        if matches!(node.kind, crate::flattened::FlattenedNodeKind::Field { .. })
                            && node.is_interactive()
                        {
                            return true;
                        }
                    }
                    crate::flattened::FlattenedKind::Group {
                        children: group_children,
                        ..
                    } => {
                        if group_contains_interactive_field(group_children) {
                            return true;
                        }
                    }
                }
            }
            false
        }

        /// Return true if any descendant group is itself a *user-repeatable*
        /// occurrence group, i.e. a repeatable occur group that contains
        /// interactive fields **and** has its own add/remove buttons.
        ///
        /// This is used to skip an outer occur wrapper in favour of a genuine
        /// inner repeater ("prefer innermost repeater"). Button-less occur
        /// groups (e.g. fragment-library `Address`/`Company` subforms that carry
        /// `occur max="-1"` purely as a layout/fragment artifact) must NOT count
        /// here — otherwise they would shadow a real outer repeatable and the
        /// section would end up with no repeatable at all.
        fn contains_nested_repeatable_group(children: &[crate::flattened::FlattenedKind]) -> bool {
            for child in children {
                if let crate::flattened::FlattenedKind::Group {
                    hints,
                    children: group_children,
                } = child
                {
                    let occur_hint = hints.iter().find_map(|h| {
                        if let crate::flattened::Hint::Occurrence { min, max } = h {
                            Some((*min, *max))
                        } else {
                            None
                        }
                    });

                    if let Some((_min, max)) = occur_hint {
                        let is_repeatable = max.map(|m| m > 1).unwrap_or(true);
                        if is_repeatable
                            && group_contains_interactive_field(group_children)
                            && group_contains_button(group_children)
                        {
                            return true;
                        }
                    }

                    if contains_nested_repeatable_group(group_children) {
                        return true;
                    }
                }
            }
            false
        }

        /// Check if a group contains a button widget (indicating user-repeatable).
        /// Forms with add/remove buttons are user-controllable repeatables;
        /// those without are script-managed (e.g., signature sections).
        fn group_contains_button(children: &[crate::flattened::FlattenedKind]) -> bool {
            for child in children {
                match child {
                    crate::flattened::FlattenedKind::Node(node) => {
                        if node.widget_type() == Some(&crate::flattened::WidgetKind::Button) {
                            return true;
                        }
                    }
                    crate::flattened::FlattenedKind::Group {
                        children: group_children,
                        ..
                    } => {
                        if group_contains_button(group_children) {
                            return true;
                        }
                    }
                }
            }
            false
        }

        /// Collect (y_min, y_max) ranges of all button widgets in the tree.
        fn collect_button_y_ranges(
            children: &[crate::flattened::FlattenedKind],
        ) -> Vec<(Num, Num)> {
            let mut ranges = Vec::new();
            fn walk(children: &[crate::flattened::FlattenedKind], out: &mut Vec<(Num, Num)>) {
                for child in children {
                    match child {
                        crate::flattened::FlattenedKind::Node(node) => {
                            if node.widget_type() == Some(&crate::flattened::WidgetKind::Button) {
                                out.push((node.y, node.y + node.height));
                            }
                        }
                        crate::flattened::FlattenedKind::Group { children: gc, .. } => {
                            walk(gc, out)
                        }
                    }
                }
            }
            walk(children, &mut ranges);
            ranges
        }

        /// Check if a group contains any field with a SOM path hint.
        /// Presence of SOM paths indicates an XFA form. AcroForms don't have them.
        fn group_has_som_paths(children: &[crate::flattened::FlattenedKind]) -> bool {
            for child in children {
                match child {
                    crate::flattened::FlattenedKind::Node(node) => {
                        if node.som_path().is_some() {
                            return true;
                        }
                    }
                    crate::flattened::FlattenedKind::Group {
                        children: group_children,
                        ..
                    } => {
                        if group_has_som_paths(group_children) {
                            return true;
                        }
                    }
                }
            }
            false
        }

        /// Count leaf nodes in a FlattenedKind tree.
        fn count_nodes(children: &[crate::flattened::FlattenedKind]) -> usize {
            children
                .iter()
                .map(|c| match c {
                    crate::flattened::FlattenedKind::Node(_) => 1,
                    crate::flattened::FlattenedKind::Group { children, .. } => {
                        count_nodes(children)
                    }
                })
                .sum()
        }

        fn calculate_group_bounds(
            children: &[crate::flattened::FlattenedKind],
        ) -> Option<crate::flattened::Bounds> {
            let mut min_x = None;
            let mut min_y = None;
            let mut max_x = None;
            let mut max_y = None;

            fn update_bounds(
                node: &crate::flattened::FlattenedNode,
                min_x: &mut Option<Num>,
                min_y: &mut Option<Num>,
                max_x: &mut Option<Num>,
                max_y: &mut Option<Num>,
            ) {
                *min_x = Some(min_x.map_or(node.x, |v| v.min(node.x)));
                *min_y = Some(min_y.map_or(node.y, |v| v.min(node.y)));
                *max_x = Some(max_x.map_or(node.x + node.width, |v| v.max(node.x + node.width)));
                *max_y = Some(max_y.map_or(node.y + node.height, |v| v.max(node.y + node.height)));
            }

            fn traverse(
                children: &[crate::flattened::FlattenedKind],
                min_x: &mut Option<Num>,
                min_y: &mut Option<Num>,
                max_x: &mut Option<Num>,
                max_y: &mut Option<Num>,
            ) {
                for child in children {
                    match child {
                        crate::flattened::FlattenedKind::Node(node) => {
                            update_bounds(node, min_x, min_y, max_x, max_y);
                        }
                        crate::flattened::FlattenedKind::Group { children, .. } => {
                            traverse(children, min_x, min_y, max_x, max_y);
                        }
                    }
                }
            }

            traverse(children, &mut min_x, &mut min_y, &mut max_x, &mut max_y);

            match (min_x, min_y, max_x, max_y) {
                (Some(x), Some(y), Some(max_x), Some(max_y)) => {
                    Some(crate::flattened::Bounds::new(x, y, max_x - x, max_y - y))
                }
                _ => None,
            }
        }

        let mut current_index = 0;
        search_groups(
            &doc.source.children,
            &mut sections,
            &mut current_index,
            &button_y_ranges,
        );
        sections
    }

    /// Find all groups that have occurrence hints indicating repeatability.
    fn find_repeatable_groups(&self, doc: &Document) -> Vec<(usize, u32, Option<u32>)> {
        let roots = doc.roots();
        let mut repeatable = Vec::new();

        for &group_idx in &roots {
            if let Some((min, max)) = self.get_occurrence_hint(doc, group_idx) {
                // Check if this is actually repeatable (max > 1 or unlimited)
                let is_repeatable = max.map(|m| m > 1).unwrap_or(true);
                if is_repeatable {
                    repeatable.push((group_idx, min, max));
                }
            }
        }

        repeatable
    }

    /// Group repeatable items by spatial proximity.
    /// Items that are vertically adjacent are likely part of the same repeatable section.
    fn group_by_proximity(
        &self,
        doc: &Document,
        repeatable_groups: Vec<(usize, u32, Option<u32>)>,
    ) -> Vec<RepeatableSection> {
        if repeatable_groups.is_empty() {
            return Vec::new();
        }

        if !self.group_adjacent {
            // Return each as its own section
            return repeatable_groups
                .into_iter()
                .map(|(idx, min, max)| RepeatableSection {
                    member_groups: vec![idx],
                    min_occurrences: min,
                    max_occurrences: max,
                    bounds: doc.get_bounds(idx),
                    node_indices: vec![], // Not used for leaf node-based detection
                    is_user_repeatable: true,
                })
                .collect();
        }

        // Sort by vertical position (top to bottom)
        let mut sorted: Vec<_> = repeatable_groups
            .into_iter()
            .filter_map(|(idx, min, max)| doc.get_bounds(idx).map(|b| (idx, min, max, b)))
            .collect();

        sorted.sort_by_key(|a| a.3.y);

        // Group by occurrence constraints and vertical proximity
        let mut sections: Vec<RepeatableSection> = Vec::new();
        let mut used: HashSet<usize> = HashSet::new();

        for (idx, min, max, bounds) in &sorted {
            if used.contains(idx) {
                continue;
            }

            // Start a new section with this item
            let mut section = RepeatableSection {
                member_groups: vec![*idx],
                min_occurrences: *min,
                max_occurrences: *max,
                bounds: Some(*bounds),
                node_indices: vec![], // Not used for leaf node-based detection
                is_user_repeatable: true,
            };
            used.insert(*idx);

            // Find other items with matching constraints that are vertically adjacent
            for (other_idx, other_min, other_max, other_bounds) in &sorted {
                if used.contains(other_idx) {
                    continue;
                }

                // Must have same occurrence constraints
                if other_min != min || other_max != max {
                    continue;
                }

                // Check vertical proximity - is other_bounds close to section bounds?
                if let Some(ref section_bounds) = section.bounds {
                    let vertical_gap = if other_bounds.y > section_bounds.bottom() {
                        other_bounds.y - section_bounds.bottom()
                    } else if section_bounds.y > other_bounds.bottom() {
                        section_bounds.y - other_bounds.bottom()
                    } else {
                        // Overlapping - no gap
                        Decimal::ZERO
                    };

                    if vertical_gap <= self.max_vertical_gap {
                        section.member_groups.push(*other_idx);
                        // Expand section bounds
                        section.bounds = Some(section_bounds.union(other_bounds));
                        used.insert(*other_idx);
                    }
                }
            }

            sections.push(section);
        }

        sections
    }

    /// Get all detected repeatable sections without modifying the document.
    /// Useful for analysis or debugging.
    pub fn detect_sections(&self, doc: &Document) -> Vec<RepeatableSection> {
        // First try to find repeatables from FlattenedKind::Group hints
        let from_flattened = self.find_repeatable_from_flattened(doc);
        if !from_flattened.is_empty() {
            return from_flattened;
        }

        // Fallback: search leaf node hints (for test cases that add hints to nodes)
        let repeatable_groups = self.find_repeatable_groups(doc);
        self.group_by_proximity(doc, repeatable_groups)
    }
}

impl AnalysisModule for RepeatableDetector {
    fn name(&self) -> &'static str {
        "RepeatableDetector"
    }

    fn process(&self, doc: &mut Document) {
        let sections = self.detect_sections(doc);

        for section in sections {
            let kind = GroupKind::RepeatableSection {
                min_occurrences: section.min_occurrences,
                max_occurrences: section.max_occurrences,
                is_user_repeatable: section.is_user_repeatable,
            };

            if section.member_groups.len() > 1 {
                let has_fields = section.member_groups.iter().any(|&g| doc.contains_field(g));
                if has_fields {
                    doc.merge_inferred(section.member_groups, kind, self.name());
                }
            } else if section.member_groups.is_empty() && !section.node_indices.is_empty() {
                let node_index_set: HashSet<usize> = section.node_indices.iter().copied().collect();
                let roots = doc.roots();
                let mut contained_groups = Vec::new();

                for &group_idx in &roots {
                    let group_node_indices = doc.collect_node_indices(group_idx);
                    let has_section_node = group_node_indices
                        .iter()
                        .any(|idx| node_index_set.contains(idx));

                    if has_section_node {
                        contained_groups.push(group_idx);
                    }
                }

                let has_fields = contained_groups.iter().any(|&g| doc.contains_field(g));
                if !contained_groups.is_empty() && has_fields {
                    doc.merge_inferred(contained_groups, kind, self.name());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flattened::{Flattened, FlattenedKind, FlattenedNode, Hint, Page};
    use crate::xfa::num;

    fn create_test_node_with_occurrence(
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        min: u32,
        max: Option<u32>,
    ) -> FlattenedNode {
        let mut node = FlattenedNode::new_field(
            "test".to_string(),
            "".to_string(),
            "test".to_string(),
            num(x),
            num(y),
            num(w),
            num(h),
        );
        node.add_hint(Hint::Occurrence { min, max });
        node
    }

    #[test]
    fn test_detect_single_repeatable() {
        let nodes = vec![create_test_node_with_occurrence(
            10.0, 10.0, 100.0, 20.0, 1, None,
        )];

        let flattened = Flattened::from_nodes(Page::new(num(595.0), num(842.0)), nodes);

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].min_occurrences, 1);
        assert_eq!(sections[0].max_occurrences, None); // unlimited
    }

    #[test]
    fn test_detect_non_repeatable_ignored() {
        // max=1 means not repeatable
        let nodes = vec![create_test_node_with_occurrence(
            10.0,
            10.0,
            100.0,
            20.0,
            1,
            Some(1),
        )];

        let flattened = Flattened::from_nodes(Page::new(num(595.0), num(842.0)), nodes);

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        assert_eq!(sections.len(), 0); // Not repeatable
    }

    #[test]
    fn test_group_adjacent_repeatables() {
        let nodes = vec![
            create_test_node_with_occurrence(10.0, 10.0, 100.0, 20.0, 0, Some(5)),
            create_test_node_with_occurrence(10.0, 35.0, 100.0, 20.0, 0, Some(5)), // 5px gap
            create_test_node_with_occurrence(10.0, 100.0, 100.0, 20.0, 0, Some(5)), // 45px gap - too far
        ];

        let flattened = Flattened::from_nodes(Page::new(num(595.0), num(842.0)), nodes);

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        // First two should be grouped, third is separate
        assert_eq!(sections.len(), 2);

        // Find the section with 2 members
        let grouped = sections.iter().find(|s| s.member_groups.len() == 2);
        assert!(grouped.is_some());
    }

    #[test]
    fn test_different_constraints_not_grouped() {
        let nodes = vec![
            create_test_node_with_occurrence(10.0, 10.0, 100.0, 20.0, 0, Some(5)),
            create_test_node_with_occurrence(10.0, 35.0, 100.0, 20.0, 1, Some(10)), // Different constraints
        ];

        let flattened = Flattened::from_nodes(Page::new(num(595.0), num(842.0)), nodes);

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        // Should be separate sections despite proximity
        assert_eq!(sections.len(), 2);
        assert!(sections.iter().all(|s| s.member_groups.len() == 1));
    }

    #[test]
    fn test_repeatable_group_without_interactive_field_is_ignored() {
        // A FlattenedKind::Group with Occurrence hint but no interactive fields
        // should NOT be detected as repeatable (e.g. a pagination wrapper).
        let text_node = FlattenedNode::new_text(
            "wrapper".to_string(),
            num(8.0),
            "Arial".to_string(),
            num(10.0),
            num(10.0),
            num(100.0),
            num(20.0),
        );

        let flattened = Flattened {
            page: Page::new(num(595.0), num(842.0)),
            children: vec![FlattenedKind::Group {
                children: vec![FlattenedKind::Node(text_node)],
                hints: vec![Hint::Occurrence { min: 1, max: None }],
            }],
            language: String::new(),
            cached_key: None,
        };

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        assert_eq!(
            sections.len(),
            0,
            "Group without interactive fields should not be repeatable"
        );
    }

    #[test]
    fn test_repeatable_group_with_interactive_field_is_detected() {
        // A FlattenedKind::Group with Occurrence hint AND an interactive field
        // should be detected as repeatable (requires button for user-repeatable)
        let field_node = FlattenedNode::new_field(
            "field".to_string(),
            "".to_string(),
            "field".to_string(),
            num(10.0),
            num(10.0),
            num(100.0),
            num(20.0),
        );

        let mut button_node = FlattenedNode::new_field(
            "Button_Add".to_string(),
            "".to_string(),
            "".to_string(),
            num(170.0),
            num(10.0),
            num(5.0),
            num(5.0),
        );
        button_node.add_hint(Hint::WidgetType(crate::flattened::WidgetKind::Button));

        let flattened = Flattened {
            page: Page::new(num(595.0), num(842.0)),
            children: vec![FlattenedKind::Group {
                children: vec![
                    FlattenedKind::Node(field_node),
                    FlattenedKind::Node(button_node),
                ],
                hints: vec![Hint::Occurrence { min: 1, max: None }],
            }],
            language: String::new(),
            cached_key: None,
        };

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        assert_eq!(
            sections.len(),
            1,
            "Group with interactive field should be repeatable"
        );
        assert_eq!(sections[0].min_occurrences, 1);
        assert_eq!(sections[0].max_occurrences, None);
    }

    #[test]
    fn test_repeatable_group_with_nested_interactive_field_is_detected() {
        // An interactive field inside a nested group should still be found.
        let text_node = FlattenedNode::new_text(
            "header".to_string(),
            num(8.0),
            "Arial".to_string(),
            num(10.0),
            num(10.0),
            num(100.0),
            num(20.0),
        );

        let nested_field_node = FlattenedNode::new_field(
            "field".to_string(),
            "".to_string(),
            "field".to_string(),
            num(20.0),
            num(10.0),
            num(100.0),
            num(20.0),
        );

        let mut button_node = FlattenedNode::new_field(
            "Button_Add".to_string(),
            "".to_string(),
            "".to_string(),
            num(170.0),
            num(10.0),
            num(5.0),
            num(5.0),
        );
        button_node.add_hint(Hint::WidgetType(crate::flattened::WidgetKind::Button));

        let flattened = Flattened {
            page: Page::new(num(595.0), num(842.0)),
            children: vec![FlattenedKind::Group {
                children: vec![
                    FlattenedKind::Node(text_node),
                    FlattenedKind::Node(button_node),
                    // Field inside a nested group should still make parent repeatable
                    FlattenedKind::Group {
                        children: vec![FlattenedKind::Node(nested_field_node)],
                        hints: vec![],
                    },
                ],
                hints: vec![Hint::Occurrence { min: 1, max: None }],
            }],
            language: String::new(),
            cached_key: None,
        };

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        assert_eq!(
            sections.len(),
            1,
            "Nested interactive field should still make group repeatable"
        );
    }

    #[test]
    fn test_outer_repeatable_wrapper_with_nested_repeatable_is_ignored() {
        let nested_field_node = FlattenedNode::new_field(
            "child_field".to_string(),
            "".to_string(),
            "child_field".to_string(),
            num(20.0),
            num(20.0),
            num(100.0),
            num(20.0),
        );

        let mut nested_button = FlattenedNode::new_field(
            "Button_Add".to_string(),
            "".to_string(),
            "".to_string(),
            num(170.0),
            num(20.0),
            num(5.0),
            num(5.0),
        );
        nested_button.add_hint(Hint::WidgetType(crate::flattened::WidgetKind::Button));

        let outer_field_node = FlattenedNode::new_field(
            "outer_field".to_string(),
            "".to_string(),
            "outer_field".to_string(),
            num(10.0),
            num(10.0),
            num(100.0),
            num(20.0),
        );

        let mut outer_button = FlattenedNode::new_field(
            "Button_Add_Outer".to_string(),
            "".to_string(),
            "".to_string(),
            num(170.0),
            num(10.0),
            num(5.0),
            num(5.0),
        );
        outer_button.add_hint(Hint::WidgetType(crate::flattened::WidgetKind::Button));

        let flattened = Flattened {
            page: Page::new(num(595.0), num(842.0)),
            children: vec![FlattenedKind::Group {
                // Outer wrapper is repeatable too, but should be ignored in favor
                // of the nested repeatable subgroup.
                hints: vec![Hint::Occurrence { min: 1, max: None }],
                children: vec![
                    FlattenedKind::Node(outer_field_node),
                    FlattenedKind::Node(outer_button),
                    FlattenedKind::Group {
                        hints: vec![Hint::Occurrence {
                            min: 0,
                            max: Some(3),
                        }],
                        children: vec![
                            FlattenedKind::Node(nested_field_node),
                            FlattenedKind::Node(nested_button),
                        ],
                    },
                ],
            }],
            language: String::new(),
            cached_key: None,
        };

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        assert_eq!(
            sections.len(),
            1,
            "Only the innermost repeatable group should be detected"
        );
        assert_eq!(sections[0].min_occurrences, 0);
        assert_eq!(sections[0].max_occurrences, Some(3));
    }

    #[test]
    fn test_signature_only_repeatable_group_is_ignored() {
        use crate::xfa::scripting::SomPath;

        let mut place_node = FlattenedNode::new_field(
            "Global_SignaturePlace".to_string(),
            "".to_string(),
            "Place".to_string(),
            num(10.0),
            num(10.0),
            num(100.0),
            num(20.0),
        );
        place_node.add_hint(Hint::SomPath(SomPath::new(
            "UBSForms.Page.Signature.DYN_Signature.Signature.Global_SignaturePlace",
        )));

        let mut date_node = FlattenedNode::new_field(
            "Global_SignatureDate".to_string(),
            "".to_string(),
            "Date".to_string(),
            num(10.0),
            num(35.0),
            num(100.0),
            num(20.0),
        );
        date_node.add_hint(Hint::SomPath(SomPath::new(
            "UBSForms.Page.Signature.DYN_Signature.Signature.Global_SignatureDate",
        )));

        let mut full_name_node = FlattenedNode::new_field(
            "FullName".to_string(),
            "".to_string(),
            "FullName".to_string(),
            num(10.0),
            num(60.0),
            num(100.0),
            num(20.0),
        );
        full_name_node.add_hint(Hint::SomPath(SomPath::new(
            "UBSForms.Page.Signature.DYN_Signature.Signature.FullName",
        )));

        let flattened = Flattened {
            page: Page::new(num(595.0), num(842.0)),
            children: vec![FlattenedKind::Group {
                hints: vec![Hint::Occurrence { min: 1, max: None }],
                children: vec![
                    FlattenedKind::Node(place_node),
                    FlattenedKind::Node(date_node),
                    FlattenedKind::Node(full_name_node),
                ],
            }],
            language: String::new(),
            cached_key: None,
        };

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        // The group is detected but NOT user-repeatable (no buttons)
        assert_eq!(
            sections.len(),
            1,
            "Signature occurrence group should be detected"
        );
        assert!(
            !sections[0].is_user_repeatable,
            "Signature-only occurrence groups should not be user-repeatable"
        );
    }
}
