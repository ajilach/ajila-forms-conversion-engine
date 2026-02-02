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
use crate::flattened::{Bounds, Hint};
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

        fn search_groups(
            children: &[crate::flattened::FlattenedKind],
            sections: &mut Vec<RepeatableSection>,
        ) {
            for child in children {
                if let crate::flattened::FlattenedKind::Group {
                    hints,
                    children: group_children,
                    ..
                } = child
                {
                    // Check if this group has an Occurrence hint
                    for hint in hints {
                        if let crate::flattened::Hint::Occurrence { min, max } = hint {
                            // This is a repeatable group - check if actually repeatable
                            let is_repeatable = max.map(|m| m > 1).unwrap_or(true);
                            if is_repeatable {
                                // Calculate bounds from all nodes in this group
                                let bounds = calculate_group_bounds(group_children);
                                sections.push(RepeatableSection {
                                    member_groups: vec![], // We don't track doc group indices here
                                    min_occurrences: *min,
                                    max_occurrences: *max,
                                    bounds,
                                });
                            }
                        }
                    }
                    // Recurse into children
                    search_groups(group_children, sections);
                }
            }
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

        search_groups(&doc.source.children, &mut sections);
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
            } else if section.member_groups.is_empty() && section.bounds.is_some() {
                // Sections from find_repeatable_from_flattened have bounds but no member_groups.
                // Find all ROOT groups (not referenced by other groups) whose bounds fall within
                // the section bounds. This ensures we collect outermost groups like LabeledField
                // rather than raw Leaf groups.
                let bounds = section.bounds.unwrap();
                let roots = doc.roots();
                let mut contained_groups = Vec::new();

                for &group_idx in &roots {
                    if let Some(group_bounds) = doc.get_bounds(group_idx) {
                        // Check if group is within the section bounds
                        if group_bounds.x >= bounds.x
                            && group_bounds.y >= bounds.y
                            && group_bounds.right() <= bounds.right()
                            && group_bounds.bottom() <= bounds.bottom()
                        {
                            contained_groups.push(group_idx);
                        }
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
    use crate::flattened::{Flattened, FlattenedNode, FlattenedNodeKind, Hint, Page};
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
}
