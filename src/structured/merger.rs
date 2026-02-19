//! Recursive structural merging for exhaustive mode.
//!
//! This module merges multiple form states (from exhaustive exploration) into a single
//! structured representation with conditional nodes marking where structures diverge.
//!
//! # Algorithm
//!
//! 1. Group states by their first selection (selection hierarchy)
//! 2. For each group with a unique selection, recursively merge states with that selection
//! 3. Find the point of structural divergence between groups
//! 4. Wrap each group's merged content in a single conditional

use std::collections::HashMap;

use crate::structured::{
    ConditionalNode, FieldCondition, FieldId, GroupNode, InputValue, StructuredNode,
};
use crate::xfa::scripting::SomPath;

/// Represents a group with divergent content, paired with its selection condition.
type DivergentGroup = (Option<Selection>, Vec<StructuredNode>);

/// Result of extracting common prefix and suffix from groups.
/// Contains the common prefix, common suffix, and divergent middle content per group.
type CommonPrefixSuffixResult = (
    Vec<StructuredNode>, // common prefix
    Vec<StructuredNode>, // common suffix
    Vec<DivergentGroup>, // middle (divergent) per group
);

/// The kind of selection that was made (determines replay behavior and condition generation).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    /// A radio button was selected
    Radio,
    /// A checkbox was checked or unchecked
    Checkbox,
    /// A dropdown option was selected
    Dropdown,
}

/// A field selection with both the specific field ID and optional container/group ID.
/// For radio buttons in an exclGroup, the group_id is the exclGroup's field ID.
/// For standalone fields, group_id is None.
#[derive(Debug, Clone)]
pub struct Selection {
    /// The field ID of the specific field that was selected
    pub field_path: FieldId,
    /// The field ID of the containing group (e.g., exclGroup for radio buttons)
    /// If None, the field is standalone and field_path is used for conditionals
    pub group_path: Option<FieldId>,
    /// The original SOM path of the field (retained for XFA form interactions)
    pub som_path: SomPath,
    /// The original SOM path of the containing group (retained for XFA form interactions)
    pub group_som_path: Option<SomPath>,
    /// The value that was set for this selection (e.g., radio name, "1"/"0" for checkbox, save value for dropdown)
    pub value: String,
    /// The kind of selection (radio, checkbox, or dropdown)
    pub kind: SelectionKind,
}

impl Selection {
    /// Create a new selection with field path, group path, value, and kind.
    /// Converts SOM paths to deterministic FieldIds while retaining originals.
    pub fn new(
        field_path: SomPath,
        group_path: Option<SomPath>,
        value: String,
        kind: SelectionKind,
    ) -> Self {
        Self {
            field_path: FieldId::from_som_path(&field_path),
            group_path: group_path.as_ref().map(FieldId::from_som_path),
            som_path: field_path,
            group_som_path: group_path,
            value,
            kind,
        }
    }

    /// Create a selection for a standalone field (no containing group).
    /// Converts the SOM path to a deterministic FieldId while retaining the original.
    pub fn standalone(field_path: SomPath, value: String, kind: SelectionKind) -> Self {
        Self {
            field_path: FieldId::from_som_path(&field_path),
            group_path: None,
            som_path: field_path,
            group_som_path: None,
            value,
            kind,
        }
    }

    /// Get the FieldId to use for conditional field_name.
    /// Returns group_path if present, otherwise field_path.
    pub fn condition_path(&self) -> &FieldId {
        self.group_path.as_ref().unwrap_or(&self.field_path)
    }

    /// Get the value name for this selection.
    /// Returns the stored value for all selection kinds.
    pub fn value_name(&self) -> &str {
        &self.value
    }
}

/// Input for merging: a single form state with its selections and structured output
#[derive(Debug, Clone)]
pub struct MergeInput {
    /// The selections that were made to reach this state
    pub selections: Vec<Selection>,
    /// The structured nodes for this state
    pub nodes: Vec<StructuredNode>,
}

impl MergeInput {
    pub fn new(selections: Vec<Selection>, nodes: Vec<StructuredNode>) -> Self {
        Self { selections, nodes }
    }
}

/// Recursive merger for combining multiple form states
pub struct RecursiveMerger {
    /// All input states to merge
    inputs: Vec<MergeInput>,
}

impl RecursiveMerger {
    /// Create a new merger with the given inputs
    pub fn new(inputs: Vec<MergeInput>) -> Self {
        Self { inputs }
    }

    /// Merge all inputs into a single structured representation
    pub fn merge(self) -> Vec<StructuredNode> {
        if self.inputs.is_empty() {
            return Vec::new();
        }

        if self.inputs.len() == 1 {
            return self.inputs.into_iter().next().unwrap().nodes;
        }

        // Start hierarchical merge
        Self::merge_by_selection_hierarchy(&self.inputs, 0)
    }

    /// Merge inputs by grouping them according to their selection at the given depth.
    /// This creates conditionals based on selection groups, not structural differences.
    fn merge_by_selection_hierarchy(
        inputs: &[MergeInput],
        selection_depth: usize,
    ) -> Vec<StructuredNode> {
        if inputs.is_empty() {
            return Vec::new();
        }

        if inputs.len() == 1 {
            return inputs[0].nodes.clone();
        }

        // Group inputs by their selection at this depth
        let groups = Self::group_by_selection(inputs, selection_depth);

        if groups.len() == 1 {
            // All inputs have the same selection at this depth (or no selection)
            // Try the next depth level, or fall back to structural merging
            let (_, group_inputs) = groups.into_iter().next().unwrap();

            // Check if any inputs have selections at deeper levels
            let has_deeper_selections = group_inputs
                .iter()
                .any(|i| i.selections.len() > selection_depth + 1);

            if has_deeper_selections {
                // Recursively merge at the next depth
                Self::merge_by_selection_hierarchy(&group_inputs, selection_depth + 1)
            } else {
                // No more selection levels - do structural merge
                Self::merge_node_lists_structural(&group_inputs)
            }
        } else {
            // Multiple selection groups - find where structures diverge and create conditionals
            Self::create_conditionals_for_groups(groups, selection_depth)
        }
    }

    /// Group inputs by their selection at the given depth.
    /// Inputs without a selection at this depth are grouped by None.
    fn group_by_selection(
        inputs: &[MergeInput],
        depth: usize,
    ) -> Vec<(Option<Selection>, Vec<MergeInput>)> {
        let mut groups: HashMap<Option<String>, Vec<MergeInput>> = HashMap::new();

        for input in inputs {
            // Use field_path + value for grouping to distinguish different values
            // of the same field (e.g., checkbox checked vs unchecked)
            let key = input
                .selections
                .get(depth)
                .map(|s| format!("{}={}", s.field_path, s.value));
            groups.entry(key).or_default().push(input.clone());
        }

        // Convert to Vec, preserving the full Selection.
        // Sort by key to ensure deterministic group ordering — HashMap iteration
        // order is non-deterministic, and the order affects prefix/suffix extraction.
        let mut sorted: Vec<_> = groups.into_iter().collect();
        sorted.sort_by(|(a, _), (b, _)| a.cmp(b));

        sorted
            .into_iter()
            .map(|(key, inputs)| {
                let selection = key.and_then(|_| {
                    inputs
                        .first()
                        .and_then(|i| i.selections.get(depth).cloned())
                });
                (selection, inputs)
            })
            .collect()
    }

    /// Create conditionals for each selection group.
    /// First merges within each group, then wraps in a conditional.
    fn create_conditionals_for_groups(
        groups: Vec<(Option<Selection>, Vec<MergeInput>)>,
        selection_depth: usize,
    ) -> Vec<StructuredNode> {
        let mut result = Vec::new();

        // First, recursively merge within each group to get their final content
        let merged_groups: Vec<(Option<Selection>, Vec<StructuredNode>)> = groups
            .into_iter()
            .map(|(selection, inputs)| {
                let merged = if inputs.len() == 1 {
                    inputs[0].nodes.clone()
                } else {
                    // Check if there are deeper selections to process
                    let has_deeper = inputs
                        .iter()
                        .any(|i| i.selections.len() > selection_depth + 1);
                    if has_deeper {
                        Self::merge_by_selection_hierarchy(&inputs, selection_depth + 1)
                    } else {
                        Self::merge_node_lists_structural(&inputs)
                    }
                };
                (selection, merged)
            })
            .collect();

        // Find the common prefix, suffix, and point of divergence
        let (common_prefix, common_suffix, divergent_groups) =
            Self::extract_common_prefix_and_suffix(merged_groups);

        // Add common prefix to result
        result.extend(common_prefix);

        // Check if all groups have empty middle (prefix + suffix consumed everything)
        let all_empty_middle = divergent_groups.iter().all(|(_, nodes)| nodes.is_empty());

        // Create conditionals for each divergent group (only if there's actual divergent content)
        if !all_empty_middle {
            for (selection, remaining_nodes) in divergent_groups {
                if remaining_nodes.is_empty() {
                    continue;
                }

                let condition = if let Some(sel) = selection {
                    // Use group_path (if present) for field_name, value for the condition
                    let value = match sel.kind {
                        SelectionKind::Checkbox => InputValue::Bool(sel.value == "checked"),
                        _ => InputValue::Text(sel.value_name().to_string()),
                    };
                    FieldCondition {
                        field_name: sel.condition_path().clone(),
                        value,
                    }
                } else {
                    FieldCondition {
                        field_name: FieldId::from_som_path(&SomPath::new("unknown")),
                        value: InputValue::Text("default".to_string()),
                    }
                };

                let content = if remaining_nodes.len() == 1 {
                    remaining_nodes.into_iter().next().unwrap()
                } else {
                    StructuredNode::Group(GroupNode {
                        children: remaining_nodes,
                    })
                };

                result.push(StructuredNode::Conditional(ConditionalNode {
                    condition,
                    content: Box::new(content),
                }));
            }
        }

        // Add common suffix to result (appears after all conditionals)
        result.extend(common_suffix);

        result
    }

    /// Extract common structural prefix AND suffix from all groups.
    /// Returns (common_prefix, common_suffix, middle_content_per_group)
    ///
    /// This extracts content that is structurally identical across ALL groups:
    /// - Prefix: matching nodes from the beginning
    /// - Suffix: matching nodes from the end (after prefix removal)
    /// - Middle: the divergent content unique to each group
    fn extract_common_prefix_and_suffix(
        mut groups: Vec<DivergentGroup>,
    ) -> CommonPrefixSuffixResult {
        if groups.is_empty() {
            return (Vec::new(), Vec::new(), Vec::new());
        }

        // --- Extract prefix (from the beginning) ---
        let mut common_prefix = Vec::new();
        let min_len = groups
            .iter()
            .map(|(_, nodes)| nodes.len())
            .min()
            .unwrap_or(0);

        let mut prefix_idx = 0;
        while prefix_idx < min_len {
            // Check if all groups have structurally equivalent nodes at this position
            let first_node = &groups[0].1[prefix_idx];
            let all_equal = groups.iter().skip(1).all(|(_, nodes)| {
                nodes
                    .get(prefix_idx)
                    .map(|n| first_node.structural_eq(n))
                    .unwrap_or(false)
            });

            if all_equal {
                common_prefix.push(first_node.clone());
                prefix_idx += 1;
            } else {
                break;
            }
        }

        // Remove the common prefix from all groups
        for (_, nodes) in &mut groups {
            *nodes = nodes[prefix_idx..].to_vec();
        }

        // --- Extract suffix (from the end) ---
        let mut common_suffix = Vec::new();
        let min_len_after_prefix = groups
            .iter()
            .map(|(_, nodes)| nodes.len())
            .min()
            .unwrap_or(0);

        let mut suffix_count = 0;
        while suffix_count < min_len_after_prefix {
            // Get node from end of first group
            let first_nodes = &groups[0].1;
            let first_node = &first_nodes[first_nodes.len() - 1 - suffix_count];

            // Check if all groups have structurally equivalent nodes at this position from end
            let all_equal = groups.iter().skip(1).all(|(_, nodes)| {
                if nodes.len() <= suffix_count {
                    return false;
                }
                let idx = nodes.len() - 1 - suffix_count;
                nodes
                    .get(idx)
                    .map(|n| first_node.structural_eq(n))
                    .unwrap_or(false)
            });

            if all_equal {
                common_suffix.push(first_node.clone());
                suffix_count += 1;
            } else {
                break;
            }
        }

        // Reverse suffix since we collected from end to start
        common_suffix.reverse();

        // Remove the common suffix from all groups
        for (_, nodes) in &mut groups {
            let new_len = nodes.len().saturating_sub(suffix_count);
            nodes.truncate(new_len);
        }

        (common_prefix, common_suffix, groups)
    }

    /// Merge node lists structurally (when all inputs have the same selection).
    /// This handles cases where there are no more selections to discriminate on.
    fn merge_node_lists_structural(inputs: &[MergeInput]) -> Vec<StructuredNode> {
        if inputs.is_empty() {
            return Vec::new();
        }

        if inputs.len() == 1 {
            return inputs[0].nodes.clone();
        }

        // For structural merging, we just take the first input's nodes
        // since all inputs should be structurally similar at this point
        inputs[0].nodes.clone()
    }

    /// Merge a single node across multiple inputs with the same structure.
    /// `current_idx` is used to find the corresponding node in each input.
    #[allow(dead_code)]
    fn merge_single_node(
        node: &StructuredNode,
        inputs: &[&MergeInput],
        current_idx: usize,
    ) -> StructuredNode {
        match node {
            StructuredNode::Group(_g) => {
                // Create sub-inputs for children using the group at current_idx
                let child_inputs: Vec<MergeInput> = inputs
                    .iter()
                    .filter_map(|input| {
                        input.nodes.get(current_idx).and_then(|n| {
                            if let StructuredNode::Group(inner) = n {
                                Some(MergeInput {
                                    selections: input.selections.clone(),
                                    nodes: inner.children.clone(),
                                })
                            } else {
                                None
                            }
                        })
                    })
                    .collect();

                if child_inputs.is_empty() {
                    node.clone()
                } else {
                    let merged_children = Self::merge_by_selection_hierarchy(&child_inputs, 0);
                    StructuredNode::Group(GroupNode {
                        children: merged_children,
                    })
                }
            }

            StructuredNode::GridLayout(gl) => {
                // Create sub-inputs for grid elements
                let child_inputs: Vec<MergeInput> = inputs
                    .iter()
                    .filter_map(|input| {
                        input.nodes.get(current_idx).and_then(|n| {
                            if let StructuredNode::GridLayout(inner) = n {
                                Some(MergeInput {
                                    selections: input.selections.clone(),
                                    nodes: inner.elements.iter().map(|e| e.node.clone()).collect(),
                                })
                            } else {
                                None
                            }
                        })
                    })
                    .collect();

                if child_inputs.is_empty() {
                    node.clone()
                } else {
                    let merged_elements = Self::merge_by_selection_hierarchy(&child_inputs, 0);

                    // Reconstruct grid layout with merged elements
                    let mut new_elements = gl.elements.clone();
                    for (i, merged) in merged_elements.into_iter().enumerate() {
                        if i < new_elements.len() {
                            new_elements[i].node = merged;
                        }
                    }

                    StructuredNode::GridLayout(crate::structured::GridLayout {
                        columns: gl.columns,
                        elements: new_elements,
                    })
                }
            }

            StructuredNode::Repeatable(r) => {
                // Merge the repeated item
                let item_inputs: Vec<MergeInput> = inputs
                    .iter()
                    .filter_map(|input| {
                        input.nodes.get(current_idx).and_then(|n| {
                            if let StructuredNode::Repeatable(inner) = n {
                                Some(MergeInput {
                                    selections: input.selections.clone(),
                                    nodes: vec![(*inner.item).clone()],
                                })
                            } else {
                                None
                            }
                        })
                    })
                    .collect();

                if item_inputs.is_empty() {
                    node.clone()
                } else {
                    let merged_items = Self::merge_by_selection_hierarchy(&item_inputs, 0);
                    let merged_item = if merged_items.len() == 1 {
                        merged_items.into_iter().next().unwrap()
                    } else {
                        StructuredNode::Group(GroupNode {
                            children: merged_items,
                        })
                    };

                    StructuredNode::Repeatable(crate::structured::RepeatableNode {
                        item: Box::new(merged_item),
                        min_occurrences: r.min_occurrences,
                        max_occurrences: r.max_occurrences,
                    })
                }
            }

            StructuredNode::Table(_t) => {
                // For tables, recursively merge cells
                // This is more complex - for now, just return the original
                // TODO: Implement table cell merging
                node.clone()
            }

            StructuredNode::Conditional(c) => {
                // Merge the content of the conditional
                let content_inputs: Vec<MergeInput> = inputs
                    .iter()
                    .filter_map(|input| {
                        input.nodes.get(current_idx).and_then(|n| {
                            if let StructuredNode::Conditional(inner) = n {
                                Some(MergeInput {
                                    selections: input.selections.clone(),
                                    nodes: vec![(*inner.content).clone()],
                                })
                            } else {
                                None
                            }
                        })
                    })
                    .collect();

                if content_inputs.is_empty() {
                    node.clone()
                } else {
                    let merged_contents = Self::merge_by_selection_hierarchy(&content_inputs, 0);
                    let merged_content = if merged_contents.len() == 1 {
                        merged_contents.into_iter().next().unwrap()
                    } else {
                        StructuredNode::Group(GroupNode {
                            children: merged_contents,
                        })
                    };

                    StructuredNode::Conditional(ConditionalNode {
                        condition: c.condition.clone(),
                        content: Box::new(merged_content),
                    })
                }
            }

            // Leaf nodes: Heading, Paragraph, Image, Field, Empty
            // These are structurally equal, so just return a clone
            _ => node.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::{HeadingLevel, HeadingNode, InlineText, ParagraphNode};

    #[test]
    fn test_merge_identical_structures() {
        let nodes = vec![StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("Hello"),
        })];

        let input1 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            nodes.clone(),
        );
        let input2 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_2"),
                "RB_2".to_string(),
                SelectionKind::Radio,
            )],
            nodes.clone(),
        );

        let merger = RecursiveMerger::new(vec![input1, input2]);
        let result = merger.merge();

        // Two different selections but identical structure
        // The common prefix extraction should find the shared content
        // Result: just the shared paragraph (no conditionals needed since content is same)
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], StructuredNode::Paragraph(_)));
    }

    #[test]
    fn test_merge_same_selection_different_structure() {
        // Two inputs with the same selection but different structure
        // Should merge into one (taking the first input's structure)
        let nodes1 = vec![StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("Version A"),
        })];

        let nodes2 = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H1,
            content: InlineText::plain("Version B"),
        })];

        let input1 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            nodes1,
        );
        let input2 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            nodes2,
        );

        let merger = RecursiveMerger::new(vec![input1, input2]);
        let result = merger.merge();

        // Same selection → merged into one (no conditional since same selection)
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_merge_with_common_prefix() {
        // Shared prefix, then divergence based on selection
        let nodes1 = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("shared"),
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Option A"),
            }),
        ];

        let nodes2 = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("shared"),
            }),
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H1,
                content: InlineText::plain("Option B"),
            }),
        ];

        let input1 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            nodes1,
        );
        let input2 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_2"),
                "RB_2".to_string(),
                SelectionKind::Radio,
            )],
            nodes2,
        );

        let merger = RecursiveMerger::new(vec![input1, input2]);
        let result = merger.merge();

        // Should have: shared paragraph + 2 conditionals (one per selection)
        assert_eq!(result.len(), 3);
        assert!(matches!(result[0], StructuredNode::Paragraph(_)));
        assert!(matches!(result[1], StructuredNode::Conditional(_)));
        assert!(matches!(result[2], StructuredNode::Conditional(_)));
    }

    #[test]
    fn test_merge_nested_selections() {
        // Simulate nested selections like RB_3 with inner RB_1/RB_2
        let nodes_base = vec![StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("RB_3 content"),
        })];
        let nodes_inner1 = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("RB_3 content"),
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Inner RB_1"),
            }),
        ];
        let nodes_inner2 = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("RB_3 content"),
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Inner RB_2"),
            }),
        ];

        let input1 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_3"),
                "RB_3".to_string(),
                SelectionKind::Radio,
            )],
            nodes_base,
        );
        let input2 = MergeInput::new(
            vec![
                Selection::standalone(
                    SomPath::new("RB_3"),
                    "RB_3".to_string(),
                    SelectionKind::Radio,
                ),
                Selection::standalone(
                    SomPath::new("inner.RB_1"),
                    "RB_1".to_string(),
                    SelectionKind::Radio,
                ),
            ],
            nodes_inner1,
        );
        let input3 = MergeInput::new(
            vec![
                Selection::standalone(
                    SomPath::new("RB_3"),
                    "RB_3".to_string(),
                    SelectionKind::Radio,
                ),
                Selection::standalone(
                    SomPath::new("inner.RB_2"),
                    "RB_2".to_string(),
                    SelectionKind::Radio,
                ),
            ],
            nodes_inner2,
        );

        let merger = RecursiveMerger::new(vec![input1, input2, input3]);
        let result = merger.merge();

        // All have the same first selection (RB_3), so should merge
        // Then diverge on the inner selection → nested conditionals
        // Result: [shared_para, cond(inner_RB_1), cond(inner_RB_2), cond(no_inner)]
        assert!(!result.is_empty());
    }

    #[test]
    fn test_merge_three_top_level_selections() {
        // Three different top-level selections (like Neuanlage, Änderung, Löschung)
        let nodes1 = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H2,
            content: InlineText::plain("Neuanlage"),
        })];
        let nodes2 = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H2,
            content: InlineText::plain("Änderung"),
        })];
        let nodes3 = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H2,
            content: InlineText::plain("Löschung"),
        })];

        let input1 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            nodes1,
        );
        let input2 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_2"),
                "RB_2".to_string(),
                SelectionKind::Radio,
            )],
            nodes2,
        );
        let input3 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_3"),
                "RB_3".to_string(),
                SelectionKind::Radio,
            )],
            nodes3,
        );

        let merger = RecursiveMerger::new(vec![input1, input2, input3]);
        let result = merger.merge();

        // Should have 3 conditional nodes (one per selection)
        assert_eq!(result.len(), 3);
        assert!(
            result
                .iter()
                .all(|n| matches!(n, StructuredNode::Conditional(_)))
        );
    }

    #[test]
    fn test_merge_single_input() {
        let nodes = vec![StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("Only one"),
        })];

        let input = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            nodes.clone(),
        );

        let merger = RecursiveMerger::new(vec![input]);
        let result = merger.merge();

        // Single input should pass through unchanged
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], StructuredNode::Paragraph(_)));
    }

    #[test]
    fn test_merge_empty() {
        let merger = RecursiveMerger::new(vec![]);
        let result = merger.merge();
        assert!(result.is_empty());
    }
}
