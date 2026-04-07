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

use crate::structured::merge_engine::{lcs_align_with, lcs_table_with, merge_duplicate_conditionals};
use crate::structured::{
    ConditionalNode, FieldCondition, FieldId, GridLayout, GridLayoutElement, GroupNode, InputValue,
    RepeatableNode, StructuredNode, TableHeader, TableRow,
};
use crate::xfa::scripting::SomPath;

/// Minimum total recursive node count for a common run to be worth hoisting.
const MIN_COMMON_RUN_WEIGHT: usize = 3;

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
///
/// A selection may carry multiple values when structurally identical branches
/// have been deduplicated.  For example, nested radio buttons RB_1, RB_2, RB_3
/// that produce the same visible output are collapsed into a single
/// representative whose `values` list contains all three names.
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
    /// The values that were set for this selection.  Normally contains a single
    /// entry, but may contain multiple entries when structurally identical
    /// branches have been merged (e.g., `["RB_1", "RB_2", "RB_3"]`).
    pub values: Vec<String>,
    /// The kind of selection (radio, checkbox, or dropdown)
    pub kind: SelectionKind,
    /// Language-agnostic positional index of the selected option within its
    /// field's option list (0-based).  Used by the pipeline to match
    /// corresponding states across languages without relying on
    /// language-dependent value strings.
    pub option_index: usize,
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
            values: vec![value],
            kind,
            option_index: 0,
        }
    }

    /// Create a new selection with an explicit option index.
    pub fn new_with_index(
        field_path: SomPath,
        group_path: Option<SomPath>,
        value: String,
        kind: SelectionKind,
        option_index: usize,
    ) -> Self {
        Self {
            field_path: FieldId::from_som_path(&field_path),
            group_path: group_path.as_ref().map(FieldId::from_som_path),
            som_path: field_path,
            group_som_path: group_path,
            values: vec![value],
            kind,
            option_index,
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
            values: vec![value],
            kind,
            option_index: 0,
        }
    }

    /// Create a standalone selection with an explicit option index.
    pub fn standalone_with_index(
        field_path: SomPath,
        value: String,
        kind: SelectionKind,
        option_index: usize,
    ) -> Self {
        Self {
            field_path: FieldId::from_som_path(&field_path),
            group_path: None,
            som_path: field_path,
            group_som_path: None,
            values: vec![value],
            kind,
            option_index,
        }
    }

    /// Get the FieldId to use for conditional field_name.
    /// Returns group_path if present, otherwise field_path.
    pub fn condition_path(&self) -> &FieldId {
        self.group_path.as_ref().unwrap_or(&self.field_path)
    }

    /// The primary (first) value for this selection.
    ///
    /// For replay purposes (applying the selection to a form) the first value
    /// is always the representative that was actually explored.
    pub fn primary_value(&self) -> &str {
        &self.values[0]
    }

    /// Add an additional equivalent value to this selection.
    ///
    /// Used by the deduplication logic when structurally identical branches
    /// are collapsed: the duplicate branch's value is recorded here so that
    /// the merger can later emit one conditional per value.
    pub fn add_value(&mut self, value: String) {
        if !self.values.contains(&value) {
            self.values.push(value);
        }
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
            // Use field_path + primary value for grouping to distinguish different values
            // of the same field (e.g., checkbox checked vs unchecked)
            let key = input
                .selections
                .get(depth)
                .map(|s| format!("{}={}", s.field_path, s.primary_value()));
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
            // Optimisation: when ALL divergent groups are non-empty and contain
            // structurally identical content, emit it once as common content rather
            // than wrapping each copy in a separate ConditionalNode.
            // Important: only apply when every group has content, because empty
            // groups represent states where the content should NOT appear.
            let all_non_empty = divergent_groups.iter().all(|(_, nodes)| !nodes.is_empty());

            let all_identical = all_non_empty
                && divergent_groups.len() >= 2
                && divergent_groups.windows(2).all(|w| {
                    let a = &w[0].1;
                    let b = &w[1].1;
                    a.len() == b.len()
                        && a.iter().zip(b.iter()).all(|(na, nb)| na.structural_eq(nb))
                });

            if all_identical {
                // All groups share the same content — emit it once.
                result.extend(divergent_groups[0].1.clone());
            } else {
                for (selection, remaining_nodes) in divergent_groups {
                    if remaining_nodes.is_empty() {
                        continue;
                    }

                    if let Some(sel) = selection {
                        // Build the content node once (shared for all values).
                        let content = if remaining_nodes.len() == 1 {
                            remaining_nodes.into_iter().next().unwrap()
                        } else {
                            StructuredNode::Group(GroupNode {
                                children: remaining_nodes,
                            })
                        };

                        // Emit one ConditionalNode per value.  When branches
                        // have been deduplicated, `sel.values` may contain
                        // multiple entries (e.g., ["RB_1", "RB_2", "RB_3"]).
                        for v in &sel.values {
                            let value = match sel.kind {
                                SelectionKind::Checkbox => InputValue::Bool(v == "checked"),
                                _ => InputValue::Text(v.clone()),
                            };
                            result.push(StructuredNode::Conditional(ConditionalNode {
                                condition: FieldCondition {
                                    field_name: sel.condition_path().clone(),
                                    value,
                                },
                                content: Box::new(content.clone()),
                            }));
                        }
                    } else {
                        // No concrete selection at this depth means this branch is the
                        // default/unconditional path for the current parent selection.
                        // Keep it as plain content instead of wrapping it in an
                        // unreachable synthetic conditional.
                        result.extend(remaining_nodes);
                    }
                }
            }
        }

        // Add common suffix to result (appears after all conditionals)
        result.extend(common_suffix);

        // Merge duplicate ConditionalNodes that share the same condition.
        // This can happen when RadioButtonContentDetector creates a ConditionalNode for
        // inset content and the merger creates another ConditionalNode for the same
        // field+value pair. Both should be combined into a single ConditionalNode.
        result = merge_duplicate_conditionals(result);

        // Hoist common content from groups of adjacent sibling Conditionals.
        // This splits large conditionals into smaller fragments separated by
        // shared unconditional content.
        result = hoist_common_from_sibling_conditionals(result);

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

        // Fast path: if all inputs are structurally identical, return the first.
        let reference = &inputs[0].nodes;
        let all_equal = inputs[1..].iter().all(|input| {
            input.nodes.len() == reference.len()
                && input
                    .nodes
                    .iter()
                    .zip(reference.iter())
                    .all(|(a, b)| a.structural_eq(b))
        });

        if all_equal {
            return reference.clone();
        }

        // Branches diverged despite sharing the same selection path.
        // Produce a best-effort union: extract the common structural prefix and
        // suffix, then collect all structurally unique divergent nodes from every
        // branch so that no content is silently dropped.
        log::warn!(
            "Structurally divergent branches share the same selection path \
             ({} branches); content from all branches will be preserved",
            inputs.len()
        );

        let groups: Vec<DivergentGroup> = inputs
            .iter()
            .map(|input| (None, input.nodes.clone()))
            .collect();

        let (common_prefix, common_suffix, divergent_groups) =
            Self::extract_common_prefix_and_suffix(groups);

        // Union divergent content, skipping structural duplicates.
        let mut unique_divergent: Vec<StructuredNode> = Vec::new();
        for (_, nodes) in divergent_groups {
            for node in nodes {
                if !unique_divergent
                    .iter()
                    .any(|existing| existing.structural_eq(&node))
                {
                    unique_divergent.push(node);
                }
            }
        }

        let mut result = common_prefix;
        result.extend(unique_divergent);
        result.extend(common_suffix);
        merge_duplicate_conditionals(result)
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

// ============================================================================
// Common-content extraction from sibling conditionals
// ============================================================================

/// Count the total number of nodes in a `StructuredNode` tree (including the
/// root node itself). Leaf nodes count as 1. Container nodes count as
/// 1 + sum of children counts.
fn recursive_node_count(node: &StructuredNode) -> usize {
    match node {
        StructuredNode::Group(g) => {
            1 + g.children.iter().map(recursive_node_count).sum::<usize>()
        }
        StructuredNode::Conditional(c) => 1 + recursive_node_count(&c.content),
        StructuredNode::Repeatable(r) => 1 + recursive_node_count(&r.item),
        StructuredNode::GridLayout(gl) => {
            1 + gl
                .elements
                .iter()
                .map(|e| recursive_node_count(&e.node))
                .sum::<usize>()
        }
        StructuredNode::Table(t) => {
            let header_count = t
                .header
                .as_ref()
                .map(|h| h.cells.iter().map(recursive_node_count).sum::<usize>())
                .unwrap_or(0);
            let row_count: usize = t
                .rows
                .iter()
                .flat_map(|r| &r.cells)
                .map(recursive_node_count)
                .sum();
            1 + header_count + row_count
        }
        // Leaf nodes: Heading, Paragraph, Image, Field, Empty, List
        _ => 1,
    }
}

/// Segments produced by `split_at_common_runs`.
enum Segment {
    /// Nodes that are identical across all sibling lists — emitted unconditionally.
    Common(Vec<StructuredNode>),
    /// Per-sibling divergent content — each inner Vec corresponds to one sibling.
    Divergent(Vec<Vec<StructuredNode>>),
}

/// Given N parallel node-lists (one per sibling conditional), find sub-sequences
/// that are structurally identical across ALL lists and partition into alternating
/// `Common` / `Divergent` segments.
///
/// A common run is only emitted when its total `recursive_node_count` ≥
/// `MIN_COMMON_RUN_WEIGHT`.
///
/// The function uses pairwise LCS alignment (list[0] vs list[k]) and intersects
/// matches to find positions that are universally matched across all lists.
fn split_at_common_runs(lists: &[Vec<StructuredNode>]) -> Vec<Segment> {
    if lists.len() < 2 {
        // Nothing to split — return a single Divergent with the input.
        return vec![Segment::Divergent(lists.to_vec())];
    }

    let ref_list = &lists[0];
    if ref_list.is_empty() {
        return vec![Segment::Divergent(lists.to_vec())];
    }

    // For each position in ref_list, compute the matched position in every other list.
    // A position is "universally matched" if it has a match in ALL other lists.
    // matched_positions[i] = Some(vec![pos_in_list1, pos_in_list2, ...]) or None
    let mut matched_positions: Vec<Option<Vec<usize>>> = vec![Some(Vec::new()); ref_list.len()];

    for other_list in &lists[1..] {
        let dp = lcs_table_with(ref_list, other_list, |a, b| a.structural_eq(b));
        let alignment = lcs_align_with(ref_list, other_list, &dp, |a, b| a.structural_eq(b));

        // Build a map: ref_idx -> other_idx for matched pairs
        let mut ref_to_other: HashMap<usize, usize> = HashMap::new();
        for (a_opt, b_opt) in &alignment {
            if let (Some(a_idx), Some(b_idx)) = (a_opt, b_opt) {
                ref_to_other.insert(*a_idx, *b_idx);
            }
        }

        // Update matched_positions: unmatched positions become None
        for (ref_idx, mp) in matched_positions.iter_mut().enumerate() {
            if let Some(positions) = mp {
                if let Some(&other_idx) = ref_to_other.get(&ref_idx) {
                    positions.push(other_idx);
                } else {
                    *mp = None;
                }
            }
        }
    }

    // Find maximal consecutive runs of universally-matched positions where the
    // matched positions in each other list are also consecutive.
    let mut runs: Vec<(usize, usize, Vec<usize>)> = Vec::new(); // (ref_start, ref_end_excl, start_in_each_other_list)

    let mut i = 0;
    while i < ref_list.len() {
        if let Some(positions) = &matched_positions[i] {
            let run_start = i;
            let start_positions = positions.clone();
            let mut run_len = 1;

            // Extend while next positions are consecutive in all lists
            while i + run_len < ref_list.len() {
                if let Some(next_positions) = &matched_positions[i + run_len] {
                    let all_consecutive = start_positions
                        .iter()
                        .zip(next_positions.iter())
                        .all(|(start, next)| *next == start + run_len);
                    if all_consecutive {
                        run_len += 1;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            // Check if the run meets the weight threshold
            let weight: usize = ref_list[run_start..run_start + run_len]
                .iter()
                .map(recursive_node_count)
                .sum();

            if weight >= MIN_COMMON_RUN_WEIGHT {
                runs.push((run_start, run_start + run_len, start_positions));
            }

            i = run_start + run_len;
        } else {
            i += 1;
        }
    }

    if runs.is_empty() {
        return vec![Segment::Divergent(lists.to_vec())];
    }

    // Partition each list into segments around the common runs.
    let mut segments: Vec<Segment> = Vec::new();
    let n_others = lists.len() - 1;

    // Current position in each list
    let mut cur_ref = 0usize;
    let mut cur_others: Vec<usize> = vec![0usize; n_others];

    for (ref_start, ref_end, other_starts) in &runs {
        // Emit divergent segment before this run (if any list has content)
        let div_ref = ref_list[cur_ref..*ref_start].to_vec();
        let mut div_others: Vec<Vec<StructuredNode>> = Vec::new();
        for (k, other_list) in lists[1..].iter().enumerate() {
            div_others.push(other_list[cur_others[k]..other_starts[k]].to_vec());
        }

        let has_content = !div_ref.is_empty() || div_others.iter().any(|d| !d.is_empty());
        if has_content {
            let mut all_divs = vec![div_ref];
            all_divs.extend(div_others);
            segments.push(Segment::Divergent(all_divs));
        }

        // Emit common segment
        segments.push(Segment::Common(ref_list[*ref_start..*ref_end].to_vec()));

        // Advance cursors past the run
        cur_ref = *ref_end;
        for (k, start) in other_starts.iter().enumerate() {
            cur_others[k] = start + (ref_end - ref_start);
        }
    }

    // Emit trailing divergent segment (if any list has content)
    let div_ref = ref_list[cur_ref..].to_vec();
    let mut div_others: Vec<Vec<StructuredNode>> = Vec::new();
    for (k, other_list) in lists[1..].iter().enumerate() {
        div_others.push(other_list[cur_others[k]..].to_vec());
    }

    let has_content = !div_ref.is_empty() || div_others.iter().any(|d| !d.is_empty());
    if has_content {
        let mut all_divs = vec![div_ref];
        all_divs.extend(div_others);
        segments.push(Segment::Divergent(all_divs));
    }

    segments
}

/// Unwrap a `StructuredNode` into a list of children for hoisting analysis.
/// `Group` → its children; anything else → a single-element vec.
fn unwrap_to_children(node: &StructuredNode) -> Vec<StructuredNode> {
    match node {
        StructuredNode::Group(g) => g.children.clone(),
        other => vec![other.clone()],
    }
}

/// Re-wrap a list of children back into a single `StructuredNode`.
/// Single child → that child; multiple → Group.
fn wrap_children(children: Vec<StructuredNode>) -> StructuredNode {
    if children.len() == 1 {
        children.into_iter().next().unwrap()
    } else {
        StructuredNode::Group(GroupNode { children })
    }
}

/// Recursive post-processing pass that extracts common content from groups of
/// adjacent sibling `Conditional` nodes sharing the same `condition.field_name`.
///
/// For each such group the pass:
/// 1. Unwraps each sibling's content into a children list.
/// 2. Calls `split_at_common_runs` to find sub-sequences identical across ALL siblings.
/// 3. Emits common segments as plain (unconditional) nodes and re-wraps divergent
///    segments in their respective Conditional nodes.
/// 4. Recurses into all child-bearing node types.
fn hoist_common_from_sibling_conditionals(nodes: Vec<StructuredNode>) -> Vec<StructuredNode> {
    if nodes.is_empty() {
        return nodes;
    }

    // --- Step 1: find groups of adjacent same-field conditionals and process them ---
    let mut result: Vec<StructuredNode> = Vec::new();
    let mut i = 0;

    // Pre-compute which field_names have conditionals at non-adjacent positions
    // so we can skip groups that don't cover all values of a field.
    let field_name_positions: HashMap<FieldId, Vec<usize>> = {
        let mut map: HashMap<FieldId, Vec<usize>> = HashMap::new();
        for (idx, node) in nodes.iter().enumerate() {
            if let StructuredNode::Conditional(c) = node {
                map.entry(c.condition.field_name.clone())
                    .or_default()
                    .push(idx);
            }
        }
        map
    };

    while i < nodes.len() {
        // Check if this starts a group of ≥2 adjacent Conditionals with the same field_name.
        if let StructuredNode::Conditional(c) = &nodes[i] {
            let field = &c.condition.field_name;
            let group_start = i;

            // Collect all adjacent conditionals with the same field_name.
            while i < nodes.len() {
                if let StructuredNode::Conditional(ci) = &nodes[i] {
                    if ci.condition.field_name == *field {
                        i += 1;
                        continue;
                    }
                }
                break;
            }

            let group_end = i;
            let group_len = group_end - group_start;

            if group_len < 2 {
                // Single conditional — no sibling hoisting possible, but recurse into content.
                result.push(recurse_into_node(nodes[group_start].clone()));
                continue;
            }

            // Safety check: only process this group if it covers ALL conditionals
            // for this field in the list. If there are other same-field conditionals
            // elsewhere (non-adjacent), hoisting would be semantically incorrect
            // because the group doesn't represent all possible values.
            let all_positions = &field_name_positions[field];
            let group_covers_all = all_positions.len() == group_len
                && all_positions
                    .iter()
                    .all(|&pos| pos >= group_start && pos < group_end);

            if !group_covers_all {
                // Not safe to hoist — emit all siblings unchanged (but recurse).
                for node in &nodes[group_start..group_end] {
                    result.push(recurse_into_node(node.clone()));
                }
                continue;
            }

            // Extract children lists from each sibling.
            let siblings: Vec<&ConditionalNode> = nodes[group_start..group_end]
                .iter()
                .map(|n| match n {
                    StructuredNode::Conditional(c) => c,
                    _ => unreachable!(),
                })
                .collect();

            let children_lists: Vec<Vec<StructuredNode>> =
                siblings.iter().map(|c| unwrap_to_children(&c.content)).collect();

            let segments = split_at_common_runs(&children_lists);

            // Check if splitting produced anything useful (more than a single Divergent).
            let has_common = segments.iter().any(|s| matches!(s, Segment::Common(_)));

            if !has_common {
                // No common content found — emit all siblings unchanged (but recurse).
                for node in &nodes[group_start..group_end] {
                    result.push(recurse_into_node(node.clone()));
                }
                continue;
            }

            // Emit segments, recursing into transformed output.
            for segment in segments {
                match segment {
                    Segment::Common(common_nodes) => {
                        for node in common_nodes {
                            result.push(recurse_into_node(node));
                        }
                    }
                    Segment::Divergent(per_sibling) => {
                        for (k, div_nodes) in per_sibling.into_iter().enumerate() {
                            if div_nodes.is_empty() {
                                continue;
                            }
                            result.push(recurse_into_node(
                                StructuredNode::Conditional(ConditionalNode {
                                    condition: siblings[k].condition.clone(),
                                    content: Box::new(wrap_children(div_nodes)),
                                }),
                            ));
                        }
                    }
                }
            }
        } else {
            // Non-conditional node — pass through (but recurse into its children).
            result.push(recurse_into_node(nodes[i].clone()));
            i += 1;
        }
    }

    result
}

/// Recursively apply `hoist_common_from_sibling_conditionals` into all
/// child-bearing node types.
fn recurse_into_node(node: StructuredNode) -> StructuredNode {
    match node {
        StructuredNode::Group(g) => {
            let children = hoist_common_from_sibling_conditionals(g.children);
            StructuredNode::Group(GroupNode { children })
        }
        StructuredNode::Conditional(c) => {
            let inner = unwrap_to_children(&c.content);
            let hoisted = hoist_common_from_sibling_conditionals(inner);
            StructuredNode::Conditional(ConditionalNode {
                condition: c.condition,
                content: Box::new(wrap_children(hoisted)),
            })
        }
        StructuredNode::Repeatable(r) => {
            let inner = recurse_into_node(*r.item);
            StructuredNode::Repeatable(RepeatableNode {
                item: Box::new(inner),
                min_occurrences: r.min_occurrences,
                max_occurrences: r.max_occurrences,
            })
        }
        StructuredNode::GridLayout(gl) => {
            let elements = gl
                .elements
                .into_iter()
                .map(|e| GridLayoutElement {
                    span: e.span,
                    node: recurse_into_node(e.node),
                })
                .collect();
            StructuredNode::GridLayout(GridLayout {
                columns: gl.columns,
                elements,
            })
        }
        StructuredNode::Table(t) => {
            let header = t.header.map(|h| TableHeader {
                cells: hoist_common_from_sibling_conditionals(h.cells),
            });
            let rows = t
                .rows
                .into_iter()
                .map(|r| TableRow {
                    cells: hoist_common_from_sibling_conditionals(r.cells),
                })
                .collect();
            StructuredNode::Table(crate::structured::TableNode {
                header,
                rows,
                caption: t.caption,
            })
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::{
        HeadingLevel, HeadingNode, InlineNode, InlineText, ParagraphNode, StructuredNode,
    };
    use std::collections::HashMap;

    fn translated_text(entries: &[(&str, &str)]) -> InlineText {
        let map: crate::structured::TranslationMap = entries
            .iter()
            .map(|(lang, text)| ((*lang).to_string(), Some((*text).to_string())))
            .collect();
        InlineText(vec![InlineNode::TranslatedText(map)])
    }

    #[test]
    fn test_merge_identical_structures() {
        let nodes = vec![StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("Hello"),
            som_path: None,
            source_name: None,
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
        // Two inputs with the same selection but different structure.
        // Before the fix: only branch 1 was kept (first-branch-wins).
        // After the fix: both branches are preserved via structural union.
        let nodes1 = vec![StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("Version A"),
            som_path: None,
            source_name: None,
        })];

        let nodes2 = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H1,
            content: InlineText::plain("Version B"),
            som_path: None,
            source_name: None,
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

        // No common prefix — Paragraph ≠ Heading. Divergent union: both nodes.
        assert_eq!(result.len(), 2);
        assert!(matches!(result[0], StructuredNode::Paragraph(_)));
        assert!(matches!(result[1], StructuredNode::Heading(_)));
    }

    #[test]
    fn test_merge_with_common_prefix() {
        // Shared prefix, then divergence based on selection
        let nodes1 = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("shared"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Option A"),
                som_path: None,
                source_name: None,
            }),
        ];

        let nodes2 = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("shared"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H1,
                content: InlineText::plain("Option B"),
                som_path: None,
                source_name: None,
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
            som_path: None,
            source_name: None,
        })];
        let nodes_inner1 = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("RB_3 content"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Inner RB_1"),
                som_path: None,
                source_name: None,
            }),
        ];
        let nodes_inner2 = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("RB_3 content"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Inner RB_2"),
                som_path: None,
                source_name: None,
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
    fn test_no_selection_group_content_stays_unconditional() {
        // One state ends at depth 1 while another has an additional nested selection.
        // The shorter path content at the deeper depth must remain unconditional,
        // not wrapped in a synthetic unknown/default conditional.
        let base_nodes = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("shared"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("base only"),
                som_path: None,
                source_name: None,
            }),
        ];

        let nested_nodes = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("shared"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("nested only"),
                som_path: None,
                source_name: None,
            }),
        ];

        let input_base = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_3"),
                "RB_3".to_string(),
                SelectionKind::Radio,
            )],
            base_nodes,
        );

        let input_nested = MergeInput::new(
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
            nested_nodes,
        );

        let result = RecursiveMerger::new(vec![input_base, input_nested]).merge();

        assert_eq!(result.len(), 3);
        assert!(matches!(result[0], StructuredNode::Paragraph(_)));
        assert!(matches!(result[1], StructuredNode::Paragraph(_)));
        assert!(matches!(result[2], StructuredNode::Conditional(_)));

        if let StructuredNode::Paragraph(p) = &result[1] {
            assert_eq!(p.content.as_plain_text(), "base only");
        } else {
            panic!("Expected unconditional base-only paragraph");
        }
    }

    #[test]
    fn test_merge_three_top_level_selections() {
        // Three different top-level selections (like Neuanlage, Änderung, Löschung)
        let nodes1 = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H2,
            content: InlineText::plain("Neuanlage"),
            som_path: None,
            source_name: None,
        })];
        let nodes2 = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H2,
            content: InlineText::plain("Änderung"),
            som_path: None,
            source_name: None,
        })];
        let nodes3 = vec![StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::H2,
            content: InlineText::plain("Löschung"),
            som_path: None,
            source_name: None,
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
            som_path: None,
            source_name: None,
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

    #[test]
    fn test_merge_same_selection_divergent_content_preserves_both_branches() {
        // Regression test: when two inputs share the same selection path but produce
        // structurally different content, the branch with identical selection used to
        // silently drop all but the first branch. The fix must preserve both.
        //
        // Both inputs have selection=[RB_1], but:
        //   Branch 1: [Paragraph("Common"), Paragraph("Branch A only")]
        //   Branch 2: [Paragraph("Common"), Heading("Branch B only")]
        //
        // Expected output after fix:
        //   [Paragraph("Common"), Paragraph("Branch A only"), Heading("Branch B only")]
        //   (common prefix extracted, divergent union, no silent drop)
        let nodes1 = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Common"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Branch A only"),
                som_path: None,
                source_name: None,
            }),
        ];
        let nodes2 = vec![
            StructuredNode::Paragraph(ParagraphNode {
                content: InlineText::plain("Common"),
                som_path: None,
                source_name: None,
            }),
            StructuredNode::Heading(HeadingNode {
                level: HeadingLevel::H1,
                content: InlineText::plain("Branch B only"),
                som_path: None,
                source_name: None,
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
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            nodes2,
        );

        let merger = RecursiveMerger::new(vec![input1, input2]);
        let result = merger.merge();

        // Before the fix: result.len() == 1 (only branch A, branch B dropped).
        // After the fix: 3 nodes — common prefix, branch A divergent, branch B divergent.
        assert_eq!(
            result.len(),
            3,
            "Both branches must be preserved, got {} node(s)",
            result.len()
        );
        assert!(
            matches!(result[0], StructuredNode::Paragraph(_)),
            "First node must be the common paragraph"
        );
        assert!(
            matches!(result[1], StructuredNode::Paragraph(_)),
            "Second node must be branch A paragraph"
        );
        assert!(
            matches!(result[2], StructuredNode::Heading(_)),
            "Third node must be branch B heading"
        );
    }

    #[test]
    fn test_merge_same_selection_without_shared_language_keeps_both() {
        // Regression for language-aware structural equality: these two nodes
        // carry the same literal text but in disjoint language keys.
        // They must not be treated as equal.
        let input1 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: translated_text(&[("de", "Gemeinsamer Text")]),
                som_path: None,
                source_name: None,
            })],
        );

        let input2 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: translated_text(&[("en", "Gemeinsamer Text")]),
                som_path: None,
                source_name: None,
            })],
        );

        let result = RecursiveMerger::new(vec![input1, input2]).merge();

        assert_eq!(
            result.len(),
            2,
            "No shared language means nodes must not collapse to one"
        );
    }

    #[test]
    fn test_merge_same_selection_with_shared_language_can_merge() {
        let input1 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: translated_text(&[("de", "Gemeinsamer Text")]),
                som_path: None,
                source_name: None,
            })],
        );

        let input2 = MergeInput::new(
            vec![Selection::standalone(
                SomPath::new("RB_1"),
                "RB_1".to_string(),
                SelectionKind::Radio,
            )],
            vec![StructuredNode::Paragraph(ParagraphNode {
                content: translated_text(&[("de", "Gemeinsamer Text")]),
                som_path: None,
                source_name: None,
            })],
        );

        let result = RecursiveMerger::new(vec![input1, input2]).merge();

        assert_eq!(result.len(), 1, "Shared language match should merge");
    }

    // ========================================================================
    // Tests for common-content extraction / hoisting
    // ========================================================================

    /// Helper: build a paragraph node with the given text.
    fn para(text: &str) -> StructuredNode {
        StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain(text),
            som_path: None,
            source_name: None,
        })
    }

    /// Helper: build a Conditional node.
    fn cond(field: &str, value: &str, content: Vec<StructuredNode>) -> StructuredNode {
        StructuredNode::Conditional(ConditionalNode {
            condition: FieldCondition {
                field_name: FieldId::from(field),
                value: InputValue::Text(value.to_string()),
            },
            content: Box::new(if content.len() == 1 {
                content.into_iter().next().unwrap()
            } else {
                StructuredNode::Group(GroupNode { children: content })
            }),
        })
    }

    #[test]
    fn test_hoist_common_run_in_middle() {
        // Two sibling conditionals each containing:
        //   [unique1, C1, C2, C3, unique2]
        // The common run C1–C3 (weight=3, ≥ threshold) should be hoisted out.
        let nodes = vec![
            cond("A", "v1", vec![
                para("A-only-1"),
                para("common1"),
                para("common2"),
                para("common3"),
                para("A-only-2"),
            ]),
            cond("A", "v2", vec![
                para("B-only-1"),
                para("common1"),
                para("common2"),
                para("common3"),
                para("B-only-2"),
            ]),
        ];

        let result = hoist_common_from_sibling_conditionals(nodes);

        // Expected: Cond(A=v1){A-only-1}, Cond(A=v2){B-only-1},
        //           common1, common2, common3,
        //           Cond(A=v1){A-only-2}, Cond(A=v2){B-only-2}
        assert_eq!(result.len(), 7, "got: {result:#?}");

        assert!(matches!(&result[0], StructuredNode::Conditional(c) if c.condition.value == InputValue::Text("v1".into())));
        assert!(matches!(&result[1], StructuredNode::Conditional(c) if c.condition.value == InputValue::Text("v2".into())));
        assert!(matches!(&result[2], StructuredNode::Paragraph(_)));
        assert!(matches!(&result[3], StructuredNode::Paragraph(_)));
        assert!(matches!(&result[4], StructuredNode::Paragraph(_)));
        assert!(matches!(&result[5], StructuredNode::Conditional(c) if c.condition.value == InputValue::Text("v1".into())));
        assert!(matches!(&result[6], StructuredNode::Conditional(c) if c.condition.value == InputValue::Text("v2".into())));
    }

    #[test]
    fn test_hoist_below_threshold_no_split() {
        // Common run of 2 paragraphs (weight=2 < 3) — should NOT be hoisted.
        let nodes = vec![
            cond("A", "v1", vec![
                para("A-only"),
                para("common1"),
                para("common2"),
            ]),
            cond("A", "v2", vec![
                para("B-only"),
                para("common1"),
                para("common2"),
            ]),
        ];

        let result = hoist_common_from_sibling_conditionals(nodes);

        // No hoisting — two conditionals remain as-is.
        assert_eq!(result.len(), 2, "got: {result:#?}");
        assert!(matches!(&result[0], StructuredNode::Conditional(_)));
        assert!(matches!(&result[1], StructuredNode::Conditional(_)));
    }

    #[test]
    fn test_hoist_single_heavy_node() {
        // A single common node whose recursive weight ≥ 3 (e.g., a Group with 3 children).
        let heavy_common = StructuredNode::Group(GroupNode {
            children: vec![para("child1"), para("child2"), para("child3")],
        });

        let nodes = vec![
            cond("A", "v1", vec![para("A-only"), heavy_common.clone()]),
            cond("A", "v2", vec![para("B-only"), heavy_common]),
        ];

        let result = hoist_common_from_sibling_conditionals(nodes);

        // Heavy common node (weight=4) should be hoisted.
        // Expected: Cond(A=v1){A-only}, Cond(A=v2){B-only}, Group{3 children}
        assert_eq!(result.len(), 3, "got: {result:#?}");
        assert!(matches!(&result[0], StructuredNode::Conditional(_)));
        assert!(matches!(&result[1], StructuredNode::Conditional(_)));
        assert!(matches!(&result[2], StructuredNode::Group(_)));
    }

    #[test]
    fn test_hoist_three_siblings_requires_all_match() {
        // 3 siblings: common run present in first two but NOT the third → no hoisting.
        let nodes = vec![
            cond("A", "v1", vec![
                para("unique-1"),
                para("common1"),
                para("common2"),
                para("common3"),
            ]),
            cond("A", "v2", vec![
                para("unique-2"),
                para("common1"),
                para("common2"),
                para("common3"),
            ]),
            cond("A", "v3", vec![
                para("unique-3"),
                para("different1"),
                para("different2"),
                para("different3"),
            ]),
        ];

        let result = hoist_common_from_sibling_conditionals(nodes);

        // No common run across ALL 3 siblings — three conditionals remain.
        assert_eq!(result.len(), 3, "got: {result:#?}");
        assert!(result.iter().all(|n| matches!(n, StructuredNode::Conditional(_))));
    }

    #[test]
    fn test_hoist_three_siblings_all_match() {
        // 3 siblings: common run of 3 nodes present in ALL → hoisted.
        let nodes = vec![
            cond("A", "v1", vec![
                para("unique-1"),
                para("common1"),
                para("common2"),
                para("common3"),
            ]),
            cond("A", "v2", vec![
                para("unique-2"),
                para("common1"),
                para("common2"),
                para("common3"),
            ]),
            cond("A", "v3", vec![
                para("unique-3"),
                para("common1"),
                para("common2"),
                para("common3"),
            ]),
        ];

        let result = hoist_common_from_sibling_conditionals(nodes);

        // Expected: 3 Conditionals (each with unique content) + 3 common paragraphs
        assert_eq!(result.len(), 6, "got: {result:#?}");
        assert!(matches!(&result[0], StructuredNode::Conditional(_)));
        assert!(matches!(&result[1], StructuredNode::Conditional(_)));
        assert!(matches!(&result[2], StructuredNode::Conditional(_)));
        assert!(matches!(&result[3], StructuredNode::Paragraph(_)));
        assert!(matches!(&result[4], StructuredNode::Paragraph(_)));
        assert!(matches!(&result[5], StructuredNode::Paragraph(_)));
    }

    #[test]
    fn test_hoist_nested_conditional() {
        // Conditional(A=v1) { [p1, Conditional(B=x){p2}, p3] }
        // Conditional(A=v2) { [q1, Conditional(B=x){p2}, q3] }
        // The inner Conditional(B=x) is identical in both A-siblings →
        // should be hoisted if its weight ≥ 3.
        // Conditional(B=x){p2} has weight=2, so we need to make it heavier:
        let inner_b = cond("B", "x", vec![para("b1"), para("b2"), para("b3")]);

        let nodes = vec![
            cond("A", "v1", vec![para("A-only"), inner_b.clone(), para("A-tail")]),
            cond("A", "v2", vec![para("B-only"), inner_b, para("B-tail")]),
        ];

        let result = hoist_common_from_sibling_conditionals(nodes);

        // inner_b has weight=4 (1 cond + 1 group + ... actually 1 + 3 children via group = 5)
        // Actually: Conditional { Group { [p(b1), p(b2), p(b3)] } } → 1 + 1 + 3 = 5? No:
        // recursive_node_count(Conditional) = 1 + recursive_node_count(Group)
        // recursive_node_count(Group) = 1 + 3 = 4
        // So total = 5 ≥ 3. Should be hoisted.
        // Expected: Cond(A=v1){A-only}, Cond(A=v2){B-only}, Conditional(B=x){...},
        //           Cond(A=v1){A-tail}, Cond(A=v2){B-tail}
        assert_eq!(result.len(), 5, "got: {result:#?}");
        assert!(matches!(&result[2], StructuredNode::Conditional(c) if c.condition.field_name == FieldId::from("B")));
    }

    #[test]
    fn test_hoist_no_common_content() {
        // Two siblings with completely different content → no hoisting.
        let nodes = vec![
            cond("A", "v1", vec![para("only-A1"), para("only-A2"), para("only-A3")]),
            cond("A", "v2", vec![para("only-B1"), para("only-B2"), para("only-B3")]),
        ];

        let result = hoist_common_from_sibling_conditionals(nodes);

        assert_eq!(result.len(), 2, "got: {result:#?}");
        assert!(result.iter().all(|n| matches!(n, StructuredNode::Conditional(_))));
    }

    #[test]
    fn test_hoist_recursive_into_group() {
        // A Group containing sibling conditionals → hoisting applied inside the Group.
        let inner = vec![
            cond("A", "v1", vec![
                para("u1"),
                para("common1"),
                para("common2"),
                para("common3"),
            ]),
            cond("A", "v2", vec![
                para("u2"),
                para("common1"),
                para("common2"),
                para("common3"),
            ]),
        ];

        let nodes = vec![StructuredNode::Group(GroupNode {
            children: inner,
        })];

        let result = hoist_common_from_sibling_conditionals(nodes);

        // The Group's children should have been processed.
        assert_eq!(result.len(), 1);
        if let StructuredNode::Group(g) = &result[0] {
            // Inside the group: 2 conditionals (unique parts) + 3 common paragraphs
            assert_eq!(g.children.len(), 5, "got: {:#?}", g.children);
        } else {
            panic!("Expected Group, got: {result:#?}");
        }
    }

    #[test]
    fn test_hoist_multiple_common_runs() {
        // Two siblings each with: [u1, C1, C2, C3, u2, D1, D2, D3, u3]
        // Two common runs (C and D), both weight ≥ 3 → both hoisted.
        let nodes = vec![
            cond("A", "v1", vec![
                para("A1"),
                para("C1"), para("C2"), para("C3"),
                para("A2"),
                para("D1"), para("D2"), para("D3"),
                para("A3"),
            ]),
            cond("A", "v2", vec![
                para("B1"),
                para("C1"), para("C2"), para("C3"),
                para("B2"),
                para("D1"), para("D2"), para("D3"),
                para("B3"),
            ]),
        ];

        let result = hoist_common_from_sibling_conditionals(nodes);

        // Expected:
        // Cond(v1){A1}, Cond(v2){B1},       ← divergent before first run
        // C1, C2, C3,                         ← first common run
        // Cond(v1){A2}, Cond(v2){B2},       ← divergent between runs
        // D1, D2, D3,                         ← second common run
        // Cond(v1){A3}, Cond(v2){B3}        ← divergent after second run
        assert_eq!(result.len(), 12, "got: {result:#?}");
    }

    #[test]
    fn test_hoist_non_adjacent_same_field_skipped() {
        // Two groups of Conditional(A=*) siblings separated by a non-conditional node.
        // Neither group covers all A-conditionals in the list, so hoisting must NOT happen.
        let nodes = vec![
            cond("A", "v1", vec![
                para("unique-1"),
                para("common1"),
                para("common2"),
                para("common3"),
            ]),
            cond("A", "v2", vec![
                para("unique-2"),
                para("common1"),
                para("common2"),
                para("common3"),
            ]),
            para("separator"),
            cond("A", "v3", vec![
                para("unique-3"),
                para("different1"),
                para("different2"),
                para("different3"),
            ]),
        ];

        let result = hoist_common_from_sibling_conditionals(nodes);

        // No hoisting should occur — the {v1,v2} group doesn't cover all A-conditionals.
        assert_eq!(result.len(), 4, "got: {result:#?}");
        assert!(matches!(&result[0], StructuredNode::Conditional(_)));
        assert!(matches!(&result[1], StructuredNode::Conditional(_)));
        assert!(matches!(&result[2], StructuredNode::Paragraph(_)));
        assert!(matches!(&result[3], StructuredNode::Conditional(_)));
    }

    #[test]
    fn test_recursive_node_count_leaf() {
        assert_eq!(recursive_node_count(&para("hello")), 1);
    }

    #[test]
    fn test_recursive_node_count_group() {
        let g = StructuredNode::Group(GroupNode {
            children: vec![para("a"), para("b"), para("c")],
        });
        assert_eq!(recursive_node_count(&g), 4); // 1 (group) + 3 (children)
    }

    #[test]
    fn test_recursive_node_count_nested() {
        let inner = StructuredNode::Group(GroupNode {
            children: vec![para("a"), para("b")],
        });
        let outer = cond("F", "v", vec![inner]);
        // Conditional(1) + Group(1) + 2 paragraphs = 4
        assert_eq!(recursive_node_count(&outer), 4);
    }
}
