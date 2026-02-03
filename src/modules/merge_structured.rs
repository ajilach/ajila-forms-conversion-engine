//! Merge multiple structured node trees into a single tree with conditional nodes.
//!
//! This module takes structured representations from exhaustive exploration (where different
//! form states produce different visible content) and merges them into a single tree.
//! State-specific content is wrapped in `ConditionalNode` referencing the field value
//! that controls visibility.
//!
//! ## Algorithm (Multi-Way Merge)
//!
//! 1. Collect all trees with their complete field value state
//! 2. Use multi-way LCS to find structurally common nodes across all states
//! 3. For nodes that appear in a subset of states:
//!    - Find the field value(s) that distinguish those states
//!    - Wrap in ConditionalNode(s) - one per distinct field value
//! 4. Recursively merge children of matching nodes

use std::collections::HashMap;

use crate::scripting::SomPath;
use crate::structured::{ConditionalNode, FieldCondition, GroupNode, InputValue, StructuredNode};
use crate::structured_diff::structured_node_structural_eq;

/// Input for the merge operation: a structured tree paired with its complete state
#[derive(Debug, Clone)]
pub struct MergeInput {
    /// The structured node tree for this state
    pub tree: StructuredNode,
    /// Complete field value state: field_name -> value
    /// This captures ALL field values that define this state
    pub state_values: HashMap<String, InputValue>,
    /// The path of the last selection (for backwards compatibility / debugging)
    pub last_path: SomPath,
}

/// Merge multiple structured trees into a single tree with conditional nodes.
///
/// Uses a multi-way merge algorithm that compares all trees simultaneously
/// to properly identify which field values control which content variations.
///
/// # Arguments
/// * `inputs` - List of trees with their complete field value state
///
/// # Returns
/// A single merged tree where state-specific content is wrapped in ConditionalNode
pub fn merge_structured_trees(inputs: Vec<MergeInput>) -> StructuredNode {
    if inputs.is_empty() {
        return StructuredNode::Empty;
    }

    if inputs.len() == 1 {
        return inputs.into_iter().next().unwrap().tree;
    }

    eprintln!(
        "[MERGE] Starting multi-way merge with {} inputs",
        inputs.len()
    );
    for (i, input) in inputs.iter().enumerate() {
        eprintln!("[MERGE]   {}: state_values={:?}", i, input.state_values);
    }

    // Perform multi-way merge
    let merged = multi_way_merge(&inputs);

    // Post-processing optimizations
    optimize_tree(merged)
}

/// Perform multi-way merge of all input trees.
///
/// The algorithm:
/// 1. For each position in the child lists, group nodes by structural equivalence
/// 2. For variants that appear in a subset of states, wrap in conditionals
/// 3. For variants that appear in ALL states, keep unconditionally
fn multi_way_merge(inputs: &[MergeInput]) -> StructuredNode {
    if inputs.is_empty() {
        return StructuredNode::Empty;
    }

    // All inputs should have trees - merge them
    let trees: Vec<&StructuredNode> = inputs.iter().map(|i| &i.tree).collect();
    merge_nodes_multi(&trees, inputs)
}

/// Merge multiple nodes (one from each state)
fn merge_nodes_multi(nodes: &[&StructuredNode], inputs: &[MergeInput]) -> StructuredNode {
    assert_eq!(nodes.len(), inputs.len());

    if nodes.is_empty() {
        return StructuredNode::Empty;
    }

    // Check if all nodes are structurally equal
    let first = nodes[0];
    let all_equal = nodes
        .iter()
        .skip(1)
        .all(|n| structured_node_structural_eq(first, n));

    if all_equal {
        // All nodes are the same - return one of them (possibly recursing into children)
        return merge_equal_nodes_multi(nodes, inputs);
    }

    // Nodes differ - we need to wrap variants in conditionals
    // Group nodes by structural equivalence
    let groups = group_by_structure(nodes);

    eprintln!(
        "[MERGE MULTI] Nodes differ at position, {} unique variants across {} states",
        groups.len(),
        nodes.len()
    );

    // If there's only one group, all nodes are equivalent (shouldn't happen given all_equal check above)
    if groups.len() == 1 {
        return merge_equal_nodes_multi(nodes, inputs);
    }

    // Create conditionals for each variant
    let mut result_children = Vec::new();

    for (representative_idx, state_indices) in &groups {
        let variant_node = nodes[*representative_idx];

        // Check if this variant appears in ALL states - only then is it unconditional
        if state_indices.len() == inputs.len() {
            result_children.push(variant_node.clone());
            continue;
        }

        // Variant appears in a subset of states - needs to be conditional
        // First, try to find a distinguishing field for all states in the subset
        if let Some((field_name, values)) = find_distinguishing_field(state_indices, inputs) {
            // Create one conditional for each unique value
            let unique_values = dedupe_values(values);
            for value in unique_values {
                result_children.push(StructuredNode::Conditional(ConditionalNode {
                    condition: FieldCondition {
                        field_name: field_name.clone(),
                        value,
                    },
                    content: Box::new(variant_node.clone()),
                }));
            }
        } else {
            // No distinguishing field found for entire subset
            // This happens when the subset includes default state (empty state_values)
            // Filter out default states and try again with just the non-default states
            let non_default_indices: Vec<usize> = state_indices
                .iter()
                .filter(|&&idx| !inputs[idx].state_values.is_empty())
                .copied()
                .collect();

            if !non_default_indices.is_empty() {
                if let Some((field_name, values)) =
                    find_distinguishing_field(&non_default_indices, inputs)
                {
                    let unique_values = dedupe_values(values);
                    for value in unique_values {
                        result_children.push(StructuredNode::Conditional(ConditionalNode {
                            condition: FieldCondition {
                                field_name: field_name.clone(),
                                value,
                            },
                            content: Box::new(variant_node.clone()),
                        }));
                    }
                } else {
                    // Still no distinguishing field - this shouldn't happen in practice
                    // Include unconditionally as fallback (but this is a bug if it happens)
                    eprintln!(
                        "[MERGE MULTI] WARNING: Could not find distinguishing field for variant appearing in {} states",
                        state_indices.len()
                    );
                    result_children.push(variant_node.clone());
                }
            }
            // If only default states have this variant, skip it (duplicate of another variant)
        }
    }

    if result_children.len() == 1 {
        result_children.pop().unwrap()
    } else if result_children.is_empty() {
        StructuredNode::Empty
    } else {
        StructuredNode::Group(GroupNode {
            children: result_children,
        })
    }
}

/// Remove duplicate InputValues from a Vec
fn dedupe_values(values: Vec<InputValue>) -> Vec<InputValue> {
    let mut unique = Vec::new();
    for v in values {
        if !unique.iter().any(|existing| existing == &v) {
            unique.push(v);
        }
    }
    unique
}

/// Merge nodes that are structurally equal - recurse into children
fn merge_equal_nodes_multi(nodes: &[&StructuredNode], inputs: &[MergeInput]) -> StructuredNode {
    let first = nodes[0];

    match first {
        StructuredNode::Group(group) => {
            // Collect all children lists
            let children_lists: Vec<&Vec<StructuredNode>> = nodes
                .iter()
                .map(|n| match n {
                    StructuredNode::Group(g) => &g.children,
                    _ => unreachable!("All nodes should be groups"),
                })
                .collect();

            let merged_children = merge_children_multi(&children_lists, inputs);
            StructuredNode::Group(GroupNode {
                children: merged_children,
            })
        }

        StructuredNode::Repeatable(rep) => {
            // Collect all item nodes
            let items: Vec<&StructuredNode> = nodes
                .iter()
                .map(|n| match n {
                    StructuredNode::Repeatable(r) => r.item.as_ref(),
                    _ => unreachable!("All nodes should be repeatables"),
                })
                .collect();

            let merged_item = merge_nodes_multi(&items, inputs);
            StructuredNode::Repeatable(crate::structured::RepeatableNode {
                item: Box::new(merged_item),
                min_occurrences: rep.min_occurrences,
                max_occurrences: rep.max_occurrences,
            })
        }

        StructuredNode::Conditional(cond) => {
            // Collect all content nodes
            let contents: Vec<&StructuredNode> = nodes
                .iter()
                .map(|n| match n {
                    StructuredNode::Conditional(c) => c.content.as_ref(),
                    _ => unreachable!("All nodes should be conditionals"),
                })
                .collect();

            let merged_content = merge_nodes_multi(&contents, inputs);
            StructuredNode::Conditional(ConditionalNode {
                condition: cond.condition.clone(),
                content: Box::new(merged_content),
            })
        }

        StructuredNode::Table(table) => {
            // Tables are complex - for now, return the first one
            // TODO: Implement proper table merging if needed
            StructuredNode::Table(table.clone())
        }

        // Leaf nodes - just return the first (they're all equal)
        _ => first.clone(),
    }
}

/// Merge multiple child lists using multi-way LCS
fn merge_children_multi(
    children_lists: &[&Vec<StructuredNode>],
    inputs: &[MergeInput],
) -> Vec<StructuredNode> {
    if children_lists.is_empty() || children_lists.iter().all(|c| c.is_empty()) {
        return vec![];
    }

    // Compute multi-way LCS
    let lcs = compute_multi_lcs(children_lists);

    eprintln!(
        "[MERGE CHILDREN] LCS has {} common positions across {} lists",
        lcs.len(),
        children_lists.len()
    );

    let mut result = Vec::new();
    let mut positions: Vec<usize> = vec![0; children_lists.len()];

    for lcs_positions in &lcs {
        // Process nodes before this LCS position
        let mut before_nodes: Vec<Vec<(usize, &StructuredNode)>> =
            vec![Vec::new(); children_lists.len()];

        for (list_idx, &lcs_pos) in lcs_positions.iter().enumerate() {
            while positions[list_idx] < lcs_pos {
                before_nodes[list_idx]
                    .push((list_idx, &children_lists[list_idx][positions[list_idx]]));
                positions[list_idx] += 1;
            }
        }

        // Handle nodes that appear before this LCS position (state-specific)
        result.extend(wrap_state_specific_nodes(&before_nodes, inputs));

        // Handle the LCS node itself - collect from all lists and merge
        let lcs_nodes: Vec<&StructuredNode> = lcs_positions
            .iter()
            .enumerate()
            .map(|(list_idx, &pos)| &children_lists[list_idx][pos])
            .collect();

        let merged = merge_nodes_multi(&lcs_nodes, inputs);
        result.push(merged);

        // Advance positions past the LCS node
        for (list_idx, _) in lcs_positions.iter().enumerate() {
            positions[list_idx] += 1;
        }
    }

    // Process any remaining nodes after the last LCS position
    let mut after_nodes: Vec<Vec<(usize, &StructuredNode)>> =
        vec![Vec::new(); children_lists.len()];
    for (list_idx, children) in children_lists.iter().enumerate() {
        while positions[list_idx] < children.len() {
            after_nodes[list_idx].push((list_idx, &children[positions[list_idx]]));
            positions[list_idx] += 1;
        }
    }
    result.extend(wrap_state_specific_nodes(&after_nodes, inputs));

    result
}

/// Compute multi-way LCS for multiple child lists.
/// Returns a list of position tuples - each tuple has one position per input list.
fn compute_multi_lcs(children_lists: &[&Vec<StructuredNode>]) -> Vec<Vec<usize>> {
    if children_lists.is_empty() {
        return vec![];
    }

    if children_lists.len() == 1 {
        // Single list - all positions are "common"
        return (0..children_lists[0].len()).map(|i| vec![i]).collect();
    }

    // Use pairwise LCS approach: find common subsequence between all lists
    // Start with positions from first list, then filter by intersection with each subsequent list

    // First, compute LCS between first two lists
    let first_pair_lcs = compute_lcs_pair(children_lists[0], children_lists[1]);

    if first_pair_lcs.is_empty() {
        return vec![];
    }

    // Convert to position tuples
    let mut multi_lcs: Vec<Vec<usize>> = first_pair_lcs
        .into_iter()
        .map(|(a, b)| vec![a, b])
        .collect();

    // For each additional list, extend the LCS
    for (list_idx, children) in children_lists.iter().enumerate().skip(2) {
        if multi_lcs.is_empty() {
            break;
        }

        // Find which LCS nodes also match in this new list
        let mut new_multi_lcs = Vec::new();
        let mut search_start = 0;

        for positions in &multi_lcs {
            // Get the representative node from first list
            let repr_node = &children_lists[0][positions[0]];

            // Find matching node in current list starting from search_start
            for j in search_start..children.len() {
                if structured_node_structural_eq(repr_node, &children[j]) {
                    let mut new_positions = positions.clone();
                    new_positions.push(j);
                    new_multi_lcs.push(new_positions);
                    search_start = j + 1;
                    break;
                }
            }
        }

        multi_lcs = new_multi_lcs;
    }

    multi_lcs
}

/// Compute LCS between two child lists
fn compute_lcs_pair(list_a: &[StructuredNode], list_b: &[StructuredNode]) -> Vec<(usize, usize)> {
    let m = list_a.len();
    let n = list_b.len();

    if m == 0 || n == 0 {
        return vec![];
    }

    // Build LCS table
    let mut table = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if structured_node_structural_eq(&list_a[i - 1], &list_b[j - 1]) {
                table[i][j] = table[i - 1][j - 1] + 1;
            } else {
                table[i][j] = table[i - 1][j].max(table[i][j - 1]);
            }
        }
    }

    // Backtrack to find matches
    let mut matches = Vec::new();
    let mut i = m;
    let mut j = n;

    while i > 0 && j > 0 {
        if structured_node_structural_eq(&list_a[i - 1], &list_b[j - 1]) {
            matches.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if table[i - 1][j] >= table[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }

    matches.reverse();
    matches
}

/// Wrap state-specific nodes in conditionals.
/// before_nodes[list_idx] contains nodes that appear only in that state.
fn wrap_state_specific_nodes(
    state_nodes: &[Vec<(usize, &StructuredNode)>],
    inputs: &[MergeInput],
) -> Vec<StructuredNode> {
    let mut result = Vec::new();

    // Group nodes by structural equivalence across all states
    let mut node_groups: Vec<(StructuredNode, Vec<usize>)> = Vec::new();

    for (state_idx, nodes) in state_nodes.iter().enumerate() {
        for (_, node) in nodes {
            // Check if this node matches any existing group
            let mut found = false;
            for (repr, state_indices) in &mut node_groups {
                if structured_node_structural_eq(repr, node) {
                    state_indices.push(state_idx);
                    found = true;
                    break;
                }
            }
            if !found {
                node_groups.push(((*node).clone(), vec![state_idx]));
            }
        }
    }

    // Create conditionals for each group
    for (node, state_indices) in node_groups {
        if state_indices.len() == inputs.len() {
            // Appears in all states - no conditional needed
            result.push(node);
        } else if let Some((field_name, values)) = find_distinguishing_field(&state_indices, inputs)
        {
            // Create one conditional for each value
            for value in values {
                result.push(StructuredNode::Conditional(ConditionalNode {
                    condition: FieldCondition {
                        field_name: field_name.clone(),
                        value,
                    },
                    content: Box::new(node.clone()),
                }));
            }
        } else {
            // No distinguishing field found - this might be because some states are "default" states
            // (with empty state_values). Try to create conditionals for non-default states only.
            let non_default_indices: Vec<usize> = state_indices
                .iter()
                .filter(|&&idx| !inputs[idx].state_values.is_empty())
                .copied()
                .collect();

            if !non_default_indices.is_empty() {
                // Try to find distinguishing field for non-default states only
                if let Some((field_name, values)) =
                    find_distinguishing_field(&non_default_indices, inputs)
                {
                    for value in values {
                        result.push(StructuredNode::Conditional(ConditionalNode {
                            condition: FieldCondition {
                                field_name: field_name.clone(),
                                value,
                            },
                            content: Box::new(node.clone()),
                        }));
                    }
                } else {
                    // Still no distinguishing field - include unconditionally
                    // (this shouldn't happen often if state_values are properly populated)
                    result.push(node);
                }
            }
            // If all states are default states, we drop the content (it's likely duplicate)
            // unless it's the only occurrence, in which case include it
            else if state_indices.len() == 1 {
                result.push(node);
            }
            // Otherwise, drop default-only content (it's already represented elsewhere)
        }
    }

    result
}

/// Group nodes by structural equivalence.
/// Returns map of representative_index -> list of state indices with that structure.
fn group_by_structure(nodes: &[&StructuredNode]) -> Vec<(usize, Vec<usize>)> {
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();

    for (idx, node) in nodes.iter().enumerate() {
        let mut found = false;
        for (repr_idx, state_indices) in &mut groups {
            if structured_node_structural_eq(nodes[*repr_idx], node) {
                state_indices.push(idx);
                found = true;
                break;
            }
        }
        if !found {
            groups.push((idx, vec![idx]));
        }
    }

    groups
}

/// Find the field and values that distinguish a subset of states from the others.
/// Returns (field_name, vec_of_values) where each value corresponds to one state in the subset.
fn find_distinguishing_field(
    state_indices: &[usize],
    inputs: &[MergeInput],
) -> Option<(String, Vec<InputValue>)> {
    if state_indices.is_empty() || inputs.is_empty() {
        return None;
    }

    // Get all field names that appear in any state
    let mut all_fields: std::collections::HashSet<&String> = std::collections::HashSet::new();
    for input in inputs {
        all_fields.extend(input.state_values.keys());
    }

    // For each field, check if it distinguishes the subset
    for field_name in all_fields {
        // Collect values for states in the subset
        let subset_values: Vec<Option<&InputValue>> = state_indices
            .iter()
            .map(|&idx| inputs[idx].state_values.get(field_name))
            .collect();

        // Collect values for states NOT in the subset
        let other_indices: Vec<usize> = (0..inputs.len())
            .filter(|idx| !state_indices.contains(idx))
            .collect();
        let other_values: Vec<Option<&InputValue>> = other_indices
            .iter()
            .map(|&idx| inputs[idx].state_values.get(field_name))
            .collect();

        // Check if subset values are distinct from other values
        // All subset states should have a value, and those values should not appear in others
        if subset_values.iter().all(|v| v.is_some()) {
            // Collect unique values from subset (using Vec since InputValue doesn't impl Hash)
            let mut subset_unique: Vec<&InputValue> = Vec::new();
            for v in subset_values.iter().filter_map(|v| *v) {
                if !subset_unique.iter().any(|existing| *existing == v) {
                    subset_unique.push(v);
                }
            }

            // Check that no subset value appears in other_values
            let has_overlap = subset_unique
                .iter()
                .any(|sv| other_values.iter().filter_map(|v| *v).any(|ov| ov == *sv));

            // Check if there's no overlap
            if !has_overlap && !subset_unique.is_empty() {
                // This field distinguishes the subset
                let values: Vec<InputValue> = subset_values
                    .into_iter()
                    .filter_map(|v| v.cloned())
                    .collect();
                return Some((field_name.clone(), values));
            }
        }
    }

    // No distinguishing field found
    None
}

// ============================================================================
// Post-processing optimizations
// ============================================================================

/// Apply all optimization passes to the merged tree
fn optimize_tree(node: StructuredNode) -> StructuredNode {
    //let node = merge_adjacent_conditionals(node);
    //let node = consolidate_conditionals_same_field(node);
    //let node = remove_redundant_conditionals(node);
    node
}

/// Consolidate conditionals with the same field name and value, even when separated
/// by other conditionals for the same field with different values.
///
/// For example:
///   conditional(field=X, value=A) -> content1
///   conditional(field=X, value=B) -> content2
///   conditional(field=X, value=A) -> content3
///
/// Becomes:
///   conditional(field=X, value=A) -> group[content1, content3]
///   conditional(field=X, value=B) -> content2
fn consolidate_conditionals_same_field(node: StructuredNode) -> StructuredNode {
    match node {
        StructuredNode::Group(mut group) => {
            // First, recursively optimize children
            group.children = group
                .children
                .into_iter()
                .map(consolidate_conditionals_same_field)
                .collect();

            // Then consolidate conditionals
            group.children = consolidate_conditionals_in_list(group.children);

            StructuredNode::Group(group)
        }

        StructuredNode::Table(mut table) => {
            if let Some(header) = table.header.take() {
                table.header = Some(crate::structured::TableHeader {
                    cells: header
                        .cells
                        .into_iter()
                        .map(consolidate_conditionals_same_field)
                        .collect(),
                });
            }
            table.rows = table
                .rows
                .into_iter()
                .map(|row| crate::structured::TableRow {
                    cells: row
                        .cells
                        .into_iter()
                        .map(consolidate_conditionals_same_field)
                        .collect(),
                })
                .collect();
            StructuredNode::Table(table)
        }

        StructuredNode::Repeatable(mut rep) => {
            rep.item = Box::new(consolidate_conditionals_same_field(*rep.item));
            StructuredNode::Repeatable(rep)
        }

        StructuredNode::Conditional(mut cond) => {
            cond.content = Box::new(consolidate_conditionals_same_field(*cond.content));
            StructuredNode::Conditional(cond)
        }

        // Leaf nodes
        other => other,
    }
}

/// Consolidate conditionals in a list - merge those with same condition even if
/// separated by other conditionals for the same field.
fn consolidate_conditionals_in_list(children: Vec<StructuredNode>) -> Vec<StructuredNode> {
    if children.is_empty() {
        return children;
    }

    // First, identify runs of conditionals with the same field_name
    let mut result: Vec<StructuredNode> = Vec::with_capacity(children.len());
    let mut i = 0;

    while i < children.len() {
        // Check if this is a conditional
        if let StructuredNode::Conditional(ref cond) = children[i] {
            let field_name = &cond.condition.field_name;

            // Collect all consecutive conditionals with the same field_name
            let mut run_end = i + 1;
            while run_end < children.len() {
                if let StructuredNode::Conditional(ref next_cond) = children[run_end] {
                    if &next_cond.condition.field_name == field_name {
                        run_end += 1;
                        continue;
                    }
                }
                break;
            }

            // If we have a run of more than 1 conditional with the same field
            if run_end > i + 1 {
                // Group by condition value
                let mut by_condition: Vec<(FieldCondition, Vec<StructuredNode>)> = Vec::new();

                for j in i..run_end {
                    if let StructuredNode::Conditional(cond) = children[j].clone() {
                        // Find existing entry for this condition
                        let found = by_condition
                            .iter_mut()
                            .find(|(c, _)| conditions_equal(c, &cond.condition));

                        if let Some((_, contents)) = found {
                            // Add to existing group
                            match *cond.content {
                                StructuredNode::Group(g) => contents.extend(g.children),
                                other => contents.push(other),
                            }
                        } else {
                            // New condition value
                            let contents = match *cond.content {
                                StructuredNode::Group(g) => g.children,
                                other => vec![other],
                            };
                            by_condition.push((cond.condition, contents));
                        }
                    }
                }

                // Emit consolidated conditionals
                for (condition, contents) in by_condition {
                    let content = if contents.len() == 1 {
                        contents.into_iter().next().unwrap()
                    } else {
                        StructuredNode::Group(GroupNode { children: contents })
                    };
                    result.push(StructuredNode::Conditional(ConditionalNode {
                        condition,
                        content: Box::new(content),
                    }));
                }

                i = run_end;
            } else {
                // Single conditional, just pass through
                result.push(children[i].clone());
                i += 1;
            }
        } else {
            // Not a conditional, pass through
            result.push(children[i].clone());
            i += 1;
        }
    }

    result
}

/// Merge adjacent conditional nodes with the same condition into a single conditional
/// containing a group of their contents.
fn merge_adjacent_conditionals(node: StructuredNode) -> StructuredNode {
    match node {
        StructuredNode::Group(mut group) => {
            // First, recursively optimize children
            group.children = group
                .children
                .into_iter()
                .map(merge_adjacent_conditionals)
                .collect();

            // Then merge adjacent conditionals with same condition
            group.children = merge_adjacent_conditionals_in_list(group.children);

            StructuredNode::Group(group)
        }

        StructuredNode::Table(mut table) => {
            if let Some(header) = table.header.take() {
                table.header = Some(crate::structured::TableHeader {
                    cells: header
                        .cells
                        .into_iter()
                        .map(merge_adjacent_conditionals)
                        .collect(),
                });
            }
            table.rows = table
                .rows
                .into_iter()
                .map(|row| crate::structured::TableRow {
                    cells: row
                        .cells
                        .into_iter()
                        .map(merge_adjacent_conditionals)
                        .collect(),
                })
                .collect();
            StructuredNode::Table(table)
        }

        StructuredNode::Repeatable(mut rep) => {
            rep.item = Box::new(merge_adjacent_conditionals(*rep.item));
            StructuredNode::Repeatable(rep)
        }

        StructuredNode::Conditional(mut cond) => {
            cond.content = Box::new(merge_adjacent_conditionals(*cond.content));
            StructuredNode::Conditional(cond)
        }

        // Leaf nodes
        other => other,
    }
}

/// Merge adjacent conditionals with the same condition in a list of nodes
fn merge_adjacent_conditionals_in_list(children: Vec<StructuredNode>) -> Vec<StructuredNode> {
    if children.is_empty() {
        return children;
    }

    let mut result: Vec<StructuredNode> = Vec::with_capacity(children.len());

    for child in children {
        // Check if we can merge with the last element in result
        let should_merge = match (result.last(), &child) {
            (
                Some(StructuredNode::Conditional(last_cond)),
                StructuredNode::Conditional(curr_cond),
            ) => conditions_equal(&last_cond.condition, &curr_cond.condition),
            _ => false,
        };

        if should_merge {
            // Pop the last conditional and merge with current
            let last = result.pop().unwrap();
            if let (
                StructuredNode::Conditional(last_cond),
                StructuredNode::Conditional(curr_cond),
            ) = (last, child)
            {
                // Merge contents into a group
                let merged_children =
                    merge_conditional_contents(*last_cond.content, *curr_cond.content);
                result.push(StructuredNode::Conditional(ConditionalNode {
                    condition: last_cond.condition,
                    content: Box::new(StructuredNode::Group(GroupNode {
                        children: merged_children,
                    })),
                }));
            }
        } else {
            result.push(child);
        }
    }

    result
}

/// Merge the contents of two conditionals being combined
fn merge_conditional_contents(a: StructuredNode, b: StructuredNode) -> Vec<StructuredNode> {
    let mut children = Vec::new();

    // Flatten if already a group
    match a {
        StructuredNode::Group(g) => children.extend(g.children),
        other => children.push(other),
    }

    match b {
        StructuredNode::Group(g) => children.extend(g.children),
        other => children.push(other),
    }

    children
}

/// Check if two FieldConditions are equal
fn conditions_equal(a: &FieldCondition, b: &FieldCondition) -> bool {
    a.field_name == b.field_name && input_values_equal(&a.value, &b.value)
}

/// Check if two InputValues are equal
fn input_values_equal(a: &InputValue, b: &InputValue) -> bool {
    match (a, b) {
        (InputValue::Text(a), InputValue::Text(b)) => a == b,
        (InputValue::Number(a), InputValue::Number(b)) => a == b,
        (InputValue::Date(a), InputValue::Date(b)) => a == b,
        (InputValue::Email(a), InputValue::Email(b)) => a == b,
        (InputValue::Tel(a), InputValue::Tel(b)) => a == b,
        (InputValue::Checkbox(a), InputValue::Checkbox(b)) => a == b,
        (InputValue::Radio(a), InputValue::Radio(b)) => a == b,
        (InputValue::Select(a), InputValue::Select(b)) => a == b,
        _ => false,
    }
}

/// Remove redundant conditionals where all branches yield the same result.
/// This detects when adjacent conditionals with different values for the same field
/// all contain structurally identical content.
fn remove_redundant_conditionals(node: StructuredNode) -> StructuredNode {
    match node {
        StructuredNode::Group(mut group) => {
            // First, recursively optimize children
            group.children = group
                .children
                .into_iter()
                .map(remove_redundant_conditionals)
                .collect();

            // Then check for redundant conditional groups
            group.children = simplify_redundant_conditionals(group.children);

            // Flatten single-child groups
            if group.children.len() == 1 {
                return group.children.into_iter().next().unwrap();
            }

            StructuredNode::Group(group)
        }

        StructuredNode::Table(mut table) => {
            if let Some(header) = table.header.take() {
                table.header = Some(crate::structured::TableHeader {
                    cells: header
                        .cells
                        .into_iter()
                        .map(remove_redundant_conditionals)
                        .collect(),
                });
            }
            table.rows = table
                .rows
                .into_iter()
                .map(|row| crate::structured::TableRow {
                    cells: row
                        .cells
                        .into_iter()
                        .map(remove_redundant_conditionals)
                        .collect(),
                })
                .collect();
            StructuredNode::Table(table)
        }

        StructuredNode::Repeatable(mut rep) => {
            rep.item = Box::new(remove_redundant_conditionals(*rep.item));
            StructuredNode::Repeatable(rep)
        }

        StructuredNode::Conditional(mut cond) => {
            cond.content = Box::new(remove_redundant_conditionals(*cond.content));
            StructuredNode::Conditional(cond)
        }

        // Leaf nodes
        other => other,
    }
}

/// Simplify redundant conditionals in a list.
/// If adjacent conditionals for the same field with different values contain
/// structurally identical content, replace them with the unconditional content.
fn simplify_redundant_conditionals(children: Vec<StructuredNode>) -> Vec<StructuredNode> {
    use crate::structured_diff::structured_node_structural_eq;

    if children.is_empty() {
        return children;
    }

    let mut result: Vec<StructuredNode> = Vec::with_capacity(children.len());
    let mut i = 0;

    while i < children.len() {
        // Check if this starts a run of conditionals for the same field
        if let StructuredNode::Conditional(ref first_cond) = children[i] {
            let field_name = &first_cond.condition.field_name;

            // Collect all adjacent conditionals for the same field
            let mut run_end = i + 1;
            while run_end < children.len() {
                if let StructuredNode::Conditional(ref next_cond) = children[run_end] {
                    if &next_cond.condition.field_name == field_name {
                        run_end += 1;
                        continue;
                    }
                }
                break;
            }

            // If we have multiple conditionals for the same field
            if run_end > i + 1 {
                let run = &children[i..run_end];

                // Check if all contents are structurally equal
                let all_equal = run.windows(2).all(|pair| {
                    if let (StructuredNode::Conditional(a), StructuredNode::Conditional(b)) =
                        (&pair[0], &pair[1])
                    {
                        structured_node_structural_eq(&a.content, &b.content)
                    } else {
                        false
                    }
                });

                if all_equal {
                    // All branches are identical - unwrap the content (use first one)
                    if let StructuredNode::Conditional(cond) = &children[i] {
                        result.push((*cond.content).clone());
                    }
                    i = run_end;
                    continue;
                }
            }
        }

        // Not a redundant conditional run - keep the node
        result.push(children[i].clone());
        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structured::{
        FieldNode, FieldType, HeadingLevel, HeadingNode, InlineText, ParagraphNode,
    };

    fn make_heading(level: u8, text: &str) -> StructuredNode {
        StructuredNode::Heading(HeadingNode {
            level: HeadingLevel::from_u8(level),
            content: InlineText::plain(text),
        })
    }

    fn make_paragraph(text: &str) -> StructuredNode {
        StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain(text),
        })
    }

    fn make_field(name: &str, value: Option<InputValue>) -> StructuredNode {
        StructuredNode::Field(FieldNode {
            name: name.to_string(),
            label: None,
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value,
            placeholder: None,
        })
    }

    fn make_radio_field(name: &str, options: Vec<&str>, value: Option<&str>) -> StructuredNode {
        StructuredNode::Field(FieldNode {
            name: name.to_string(),
            label: None,
            input_type: FieldType::Radio {
                options: options.into_iter().map(String::from).collect(),
            },
            value: value.map(|v| InputValue::Radio(v.to_string())),
            placeholder: None,
        })
    }

    fn make_group(children: Vec<StructuredNode>) -> StructuredNode {
        StructuredNode::Group(GroupNode { children })
    }

    fn make_input(tree: StructuredNode, field_values: Vec<(&str, InputValue)>) -> MergeInput {
        let mut state_values = HashMap::new();
        for (name, value) in field_values {
            state_values.insert(name.to_string(), value);
        }
        MergeInput {
            tree,
            state_values,
            last_path: SomPath::new("__test__"),
        }
    }

    #[test]
    fn test_merge_empty_inputs() {
        let result = merge_structured_trees(vec![]);
        assert!(matches!(result, StructuredNode::Empty));
    }

    #[test]
    fn test_merge_single_input() {
        let tree = make_group(vec![make_heading(1, "Title")]);
        let input = make_input(
            tree.clone(),
            vec![("choice", InputValue::Radio("Option1".to_string()))],
        );

        let result = merge_structured_trees(vec![input]);
        // Should return the tree unchanged
        if let StructuredNode::Group(g) = result {
            assert_eq!(g.children.len(), 1);
        } else {
            panic!("Expected Group");
        }
    }

    #[test]
    fn test_merge_identical_trees() {
        let tree = make_group(vec![
            make_heading(1, "Title"),
            make_radio_field("choice", vec!["A", "B"], Some("A")),
        ]);

        let input_a = make_input(
            tree.clone(),
            vec![("choice", InputValue::Radio("A".to_string()))],
        );
        let input_b = make_input(
            tree.clone(),
            vec![("choice", InputValue::Radio("B".to_string()))],
        );

        // Identical trees should merge without conditionals
        let result = merge_structured_trees(vec![input_a, input_b]);
        if let StructuredNode::Group(g) = result {
            assert_eq!(g.children.len(), 2);
        } else {
            panic!("Expected Group");
        }
    }

    #[test]
    fn test_merge_with_structural_difference() {
        // Tree A: has an extra paragraph when radio = "A"
        let tree_a = make_group(vec![
            make_heading(1, "Title"),
            make_radio_field("choice", vec!["A", "B"], Some("A")),
            make_paragraph("Only visible when A"),
        ]);

        // Tree B: no extra paragraph when radio = "B"
        let tree_b = make_group(vec![
            make_heading(1, "Title"),
            make_radio_field("choice", vec!["A", "B"], Some("B")),
        ]);

        let input_a = make_input(tree_a, vec![("choice", InputValue::Radio("A".to_string()))]);
        let input_b = make_input(tree_b, vec![("choice", InputValue::Radio("B".to_string()))]);

        let result = merge_structured_trees(vec![input_a, input_b]);

        // Result should have the paragraph wrapped in a conditional
        if let StructuredNode::Group(g) = &result {
            // Should have: heading, field, conditional(paragraph)
            assert!(g.children.len() >= 2, "Expected at least heading and field");

            // Find the conditional node
            let has_conditional = g
                .children
                .iter()
                .any(|child| matches!(child, StructuredNode::Conditional(_)));
            assert!(
                has_conditional,
                "Expected a conditional node wrapping the paragraph"
            );
        } else {
            panic!("Expected Group");
        }
    }

    #[test]
    fn test_merge_preserves_nested_structure() {
        // Tree with nested groups
        let tree_a = make_group(vec![
            make_heading(1, "Title"),
            make_group(vec![
                make_radio_field("choice", vec!["A", "B"], Some("A")),
                make_paragraph("Nested content A"),
            ]),
        ]);

        let tree_b = make_group(vec![
            make_heading(1, "Title"),
            make_group(vec![
                make_radio_field("choice", vec!["A", "B"], Some("B")),
                make_paragraph("Nested content B"),
            ]),
        ]);

        let input_a = make_input(tree_a, vec![("choice", InputValue::Radio("A".to_string()))]);
        let input_b = make_input(tree_b, vec![("choice", InputValue::Radio("B".to_string()))]);

        let result = merge_structured_trees(vec![input_a, input_b]);

        // Should have nested structure preserved with conditionals
        if let StructuredNode::Group(outer) = &result {
            assert_eq!(outer.children.len(), 2); // heading + inner group
            if let StructuredNode::Group(inner) = &outer.children[1] {
                // Inner group should have field + conditionals for the different paragraphs
                assert!(inner.children.len() >= 2);
            }
        }
    }

    // ========================================================================
    // Optimization tests
    // ========================================================================

    fn make_conditional(field_name: &str, value: &str, content: StructuredNode) -> StructuredNode {
        StructuredNode::Conditional(ConditionalNode {
            condition: FieldCondition {
                field_name: field_name.to_string(),
                value: InputValue::Radio(value.to_string()),
            },
            content: Box::new(content),
        })
    }

    #[test]
    fn test_merge_adjacent_conditionals_same_condition() {
        // Two adjacent conditionals with the same condition should be merged
        let input = make_group(vec![
            make_heading(1, "Title"),
            make_conditional("choice", "A", make_paragraph("Para 1")),
            make_conditional("choice", "A", make_paragraph("Para 2")),
            make_heading(2, "Footer"),
        ]);

        let result = merge_adjacent_conditionals(input);

        if let StructuredNode::Group(g) = result {
            // Should have: heading, merged_conditional, footer
            assert_eq!(g.children.len(), 3);

            // The second element should be a conditional with grouped content
            if let StructuredNode::Conditional(cond) = &g.children[1] {
                assert_eq!(cond.condition.field_name, "choice");
                if let StructuredNode::Group(inner) = &*cond.content {
                    assert_eq!(inner.children.len(), 2);
                } else {
                    panic!("Expected merged content to be a Group");
                }
            } else {
                panic!("Expected Conditional at index 1");
            }
        } else {
            panic!("Expected Group");
        }
    }

    #[test]
    fn test_merge_adjacent_conditionals_different_conditions() {
        // Two adjacent conditionals with different conditions should NOT be merged
        let input = make_group(vec![
            make_conditional("choice", "A", make_paragraph("Para A")),
            make_conditional("choice", "B", make_paragraph("Para B")),
        ]);

        let result = merge_adjacent_conditionals(input);

        if let StructuredNode::Group(g) = result {
            // Should still have 2 conditionals
            assert_eq!(g.children.len(), 2);
            assert!(matches!(g.children[0], StructuredNode::Conditional(_)));
            assert!(matches!(g.children[1], StructuredNode::Conditional(_)));
        } else {
            panic!("Expected Group");
        }
    }

    #[test]
    fn test_remove_redundant_conditionals() {
        // Two conditionals for the same field with different values but identical content
        let input = make_group(vec![
            make_conditional("choice", "A", make_paragraph("Same content")),
            make_conditional("choice", "B", make_paragraph("Same content")),
        ]);

        let result = remove_redundant_conditionals(input);

        // Should simplify to just the paragraph without conditionals
        if let StructuredNode::Paragraph(p) = result {
            assert_eq!(p.content.as_plain_text(), "Same content");
        } else {
            panic!(
                "Expected Paragraph after removing redundant conditionals, got {:?}",
                result
            );
        }
    }

    #[test]
    fn test_keep_non_redundant_conditionals() {
        // Two conditionals for the same field with different content should be kept
        let input = make_group(vec![
            make_conditional("choice", "A", make_paragraph("Content A")),
            make_conditional("choice", "B", make_paragraph("Content B")),
        ]);

        let result = remove_redundant_conditionals(input);

        if let StructuredNode::Group(g) = result {
            // Should still have 2 conditionals
            assert_eq!(g.children.len(), 2);
        } else {
            panic!("Expected Group with conditionals");
        }
    }

    #[test]
    fn test_optimize_tree_combines_both_passes() {
        // Create input with adjacent same-condition conditionals AND redundant conditionals
        let input = make_group(vec![
            // These should be merged (same condition)
            make_conditional("field1", "X", make_paragraph("A")),
            make_conditional("field1", "X", make_paragraph("B")),
            // These should be simplified (redundant - same content)
            make_conditional("field2", "Y", make_paragraph("Same")),
            make_conditional("field2", "Z", make_paragraph("Same")),
        ]);

        let result = optimize_tree(input);

        if let StructuredNode::Group(g) = result {
            // First pair merged into 1 conditional, second pair simplified to 1 paragraph
            assert_eq!(g.children.len(), 2);

            // First should be a conditional with merged content
            assert!(matches!(g.children[0], StructuredNode::Conditional(_)));

            // Second should be the unconditional paragraph
            assert!(matches!(g.children[1], StructuredNode::Paragraph(_)));
        } else {
            panic!("Expected Group");
        }
    }
}
