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
use crate::document::{Document, GroupKind, GroupSource};
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
        // Check if this is a leaf group pointing to a node with occurrence hint
        let group = doc.groups.get(group_idx)?;
        if let crate::document::GroupKind::Leaf { node_index } = &group.kind {
            // Get the node and check its hints
            if let Some(node) = doc.get_node(*node_index) {
                for hint in &node.hints {
                    if let crate::flattened::Hint::Occurrence { min, max } = hint {
                        return Some((*min, *max));
                    }
                }
            }
        }

        None
    }

    /// Find repeatable groups from the flattened structure.
    /// This searches FlattenedKind::Group elements for Occurrence hints.
    fn find_repeatable_from_flattened(&self, doc: &Document) -> Vec<RepeatableSection> {
        let mut sections = Vec::new();

        /// Search for groups with Occurrence hints and track node indices.
        /// `current_index` tracks the current position in the flattened node iteration order.
        fn search_groups(
            children: &[crate::flattened::FlattenedKind],
            sections: &mut Vec<RepeatableSection>,
            current_index: &mut usize,
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
                            if is_repeatable && group_contains_button(group_children) {
                                // Collect node indices for this group
                                let start_index = *current_index;
                                let node_count = count_nodes(group_children);
                                let node_indices: Vec<usize> =
                                    (start_index..start_index + node_count).collect();

                                // Calculate bounds from all nodes in this group
                                let bounds = calculate_group_bounds(group_children);

                                sections.push(RepeatableSection {
                                    member_groups: vec![],
                                    min_occurrences: min,
                                    max_occurrences: max,
                                    bounds,
                                    node_indices,
                                });
                            }
                        }
                        // Recurse into children (this also advances current_index)
                        search_groups(group_children, sections, current_index);
                    }
                    crate::flattened::FlattenedKind::Node(_) => {
                        // Leaf node - increment the index
                        *current_index += 1;
                    }
                }
            }
        }

        /// Check if a FlattenedKind tree contains at least one button node.
        /// A button is identified by having a WidgetType(Button) hint.
        /// Only groups with buttons are treated as user-facing repeatables;
        /// groups with occur max="-1" but no buttons are just pagination wrappers.
        fn group_contains_button(children: &[crate::flattened::FlattenedKind]) -> bool {
            for child in children {
                match child {
                    crate::flattened::FlattenedKind::Node(node) => {
                        if node.hints.iter().any(|h| {
                            matches!(
                                h,
                                crate::flattened::Hint::WidgetType(
                                    crate::flattened::WidgetKind::Button
                                )
                            )
                        }) {
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
        search_groups(&doc.source.children, &mut sections, &mut current_index);
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
                })
                .collect();
        }

        // Sort by vertical position (top to bottom)
        let mut sorted: Vec<_> = repeatable_groups
            .into_iter()
            .filter_map(|(idx, min, max)| doc.get_bounds(idx).map(|b| (idx, min, max, b)))
            .collect();

        sorted.sort_by(|a, b| a.3.y.cmp(&b.3.y));

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
            if section.member_groups.len() > 1 {
                // Only create a repeatable section if it contains at least one field
                let has_fields = section.member_groups.iter().any(|&g| doc.contains_field(g));
                if has_fields {
                    // Multiple member groups - merge them into a RepeatableSection
                    doc.merge(
                        section.member_groups,
                        GroupKind::RepeatableSection {
                            min_occurrences: section.min_occurrences,
                            max_occurrences: section.max_occurrences,
                        },
                        GroupSource::Inferred {
                            module: self.name().to_string(),
                        },
                    );
                }
            } else if section.member_groups.is_empty() && !section.node_indices.is_empty() {
                // Sections from find_repeatable_from_flattened have node_indices but no member_groups.
                // Find all ROOT groups (not referenced by other groups) that contain at least one
                // node from the section's node_indices. This ensures we only include groups that
                // are structurally within the repeatable section, not sibling elements that happen
                // to fall within the spatial bounds.
                let node_index_set: HashSet<usize> = section.node_indices.iter().copied().collect();
                let roots = doc.roots();
                let mut contained_groups = Vec::new();

                for &group_idx in &roots {
                    // Check if this group contains any of the section's nodes
                    let group_node_indices = doc.collect_node_indices(group_idx);
                    let has_section_node = group_node_indices
                        .iter()
                        .any(|idx| node_index_set.contains(idx));

                    if has_section_node {
                        contained_groups.push(group_idx);
                    }
                }

                // Only create a repeatable section if it contains at least one field
                let has_fields = contained_groups.iter().any(|&g| doc.contains_field(g));
                if !contained_groups.is_empty() && has_fields {
                    doc.merge(
                        contained_groups,
                        GroupKind::RepeatableSection {
                            min_occurrences: section.min_occurrences,
                            max_occurrences: section.max_occurrences,
                        },
                        GroupSource::Inferred {
                            module: self.name().to_string(),
                        },
                    );
                }
            }
            // Single-member sections (member_groups.len() == 1) don't need a wrapper group
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flattened::{Flattened, FlattenedKind, FlattenedNode, Hint, Page, WidgetKind};
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

        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            nodes,
        );

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

        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            nodes,
        );

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

        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            nodes,
        );

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

        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            nodes,
        );

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        // Should be separate sections despite proximity
        assert_eq!(sections.len(), 2);
        assert!(sections.iter().all(|s| s.member_groups.len() == 1));
    }

    #[test]
    fn test_repeatable_group_without_button_is_ignored() {
        // A FlattenedKind::Group with Occurrence hint but NO button nodes
        // should NOT be detected as repeatable (it's just a pagination wrapper)
        let field_node = FlattenedNode::new_field(
            "field".to_string(),
            "".to_string(),
            "field".to_string(),
            num(10.0),
            num(10.0),
            num(100.0),
            num(20.0),
        );

        let flattened = Flattened {
            page: Page {
                width: num(595.0),
                height: num(842.0),
            },
            children: vec![FlattenedKind::Group {
                children: vec![FlattenedKind::Node(field_node)],
                hints: vec![Hint::Occurrence {
                    min: 1,
                    max: None,
                }],
            }],
        };

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        assert_eq!(sections.len(), 0, "Group without buttons should not be repeatable");
    }

    #[test]
    fn test_repeatable_group_with_button_is_detected() {
        // A FlattenedKind::Group with Occurrence hint AND a button node
        // should be detected as repeatable
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
            "Button_Add".to_string(),
            num(175.0),
            num(10.0),
            num(5.0),
            num(5.0),
        );
        button_node.add_hint(Hint::WidgetType(WidgetKind::Button));
        button_node.add_hint(Hint::NoPrint);

        let flattened = Flattened {
            page: Page {
                width: num(595.0),
                height: num(842.0),
            },
            children: vec![FlattenedKind::Group {
                children: vec![
                    FlattenedKind::Node(field_node),
                    FlattenedKind::Node(button_node),
                ],
                hints: vec![Hint::Occurrence {
                    min: 1,
                    max: None,
                }],
            }],
        };

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        assert_eq!(sections.len(), 1, "Group with button should be repeatable");
        assert_eq!(sections[0].min_occurrences, 1);
        assert_eq!(sections[0].max_occurrences, None);
    }

    #[test]
    fn test_repeatable_group_with_nested_button_is_detected() {
        // A button inside a nested group should still be found
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
            "Button_Add".to_string(),
            num(175.0),
            num(10.0),
            num(5.0),
            num(5.0),
        );
        button_node.add_hint(Hint::WidgetType(WidgetKind::Button));

        let flattened = Flattened {
            page: Page {
                width: num(595.0),
                height: num(842.0),
            },
            children: vec![FlattenedKind::Group {
                children: vec![
                    FlattenedKind::Node(field_node),
                    // Button inside a nested group (like STP_PlusMinus subform)
                    FlattenedKind::Group {
                        children: vec![FlattenedKind::Node(button_node)],
                        hints: vec![],
                    },
                ],
                hints: vec![Hint::Occurrence {
                    min: 1,
                    max: None,
                }],
            }],
        };

        let doc = Document::from_flattened(&flattened);
        let detector = RepeatableDetector::new();
        let sections = detector.detect_sections(&doc);

        assert_eq!(sections.len(), 1, "Nested button should still make group repeatable");
    }
}
