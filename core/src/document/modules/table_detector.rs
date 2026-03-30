//! Table structure detector.
//!
//! Detects XFA table and row subforms from `Hint::TableLayout` and
//! `Hint::TableRowLayout` hints on `FlattenedKind::Group` elements,
//! then creates `GroupKind::Table` and `GroupKind::TableRow` groups
//! in the Document.

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use crate::flattened::{FlattenedKind, FlattenedNodeKind, Hint};
use std::collections::HashSet;

pub struct TableDetector;

impl TableDetector {
    pub fn new() -> Self {
        Self
    }
}

/// A detected table from flattened hints.
struct DetectedTable {
    /// Column widths from the TableLayout hint.
    column_widths: Vec<crate::xfa::Num>,
    /// Rows within this table.
    rows: Vec<DetectedRow>,
    /// All node indices within this table (for mapping to Document groups).
    all_node_indices: Vec<usize>,
}

/// A detected row within a table.
struct DetectedRow {
    /// Node indices belonging to this row.
    node_indices: Vec<usize>,
    /// Number of non-line leaf nodes in this row.
    non_line_cell_count: usize,
}

impl TableDetector {
    /// Walk the FlattenedKind tree to find groups with TableLayout and TableRowLayout hints.
    fn find_tables_from_flattened(&self, doc: &Document) -> Vec<DetectedTable> {
        let mut tables = Vec::new();
        let mut current_index = 0usize;
        Self::search_groups(&doc.source.children, &mut tables, &mut current_index);
        tables
    }

    fn search_groups(
        children: &[FlattenedKind],
        tables: &mut Vec<DetectedTable>,
        current_index: &mut usize,
    ) {
        for child in children {
            match child {
                FlattenedKind::Group {
                    hints,
                    children: group_children,
                    ..
                } => {
                    // Check if this group has a TableLayout hint
                    let table_hint = hints.iter().find_map(|h| {
                        if let Hint::TableLayout { column_widths } = h {
                            Some(column_widths.clone())
                        } else {
                            None
                        }
                    });

                    if let Some(column_widths) = table_hint {
                        // This is a table — find its row children
                        let start_index = *current_index;
                        let mut rows = Vec::new();
                        Self::find_rows(group_children, &mut rows, current_index);
                        let end_index = *current_index;
                        let all_node_indices: Vec<usize> = (start_index..end_index).collect();

                        tables.push(DetectedTable {
                            column_widths,
                            rows,
                            all_node_indices,
                        });
                    } else {
                        // Not a table — recurse to find nested tables
                        Self::search_groups(group_children, tables, current_index);
                    }
                }
                FlattenedKind::Node(_) => {
                    *current_index += 1;
                }
            }
        }
    }

    fn find_rows(
        children: &[FlattenedKind],
        rows: &mut Vec<DetectedRow>,
        current_index: &mut usize,
    ) {
        for child in children {
            match child {
                FlattenedKind::Group {
                    hints,
                    children: group_children,
                    ..
                } => {
                    let is_row = hints
                        .iter()
                        .any(|h| matches!(h, Hint::TableRowLayout));

                    if is_row {
                        let non_line_count = Self::count_non_line_nodes(group_children);
                        let start = *current_index;
                        Self::count_nodes_in(group_children, current_index);
                        let end = *current_index;
                        rows.push(DetectedRow {
                            node_indices: (start..end).collect(),
                            non_line_cell_count: non_line_count,
                        });
                    } else {
                        // Non-row child within table — just advance the index
                        Self::count_nodes_in(group_children, current_index);
                    }
                }
                FlattenedKind::Node(_) => {
                    *current_index += 1;
                }
            }
        }
    }

    fn count_nodes_in(children: &[FlattenedKind], current_index: &mut usize) {
        for child in children {
            match child {
                FlattenedKind::Group { children, .. } => {
                    Self::count_nodes_in(children, current_index);
                }
                FlattenedKind::Node(_) => {
                    *current_index += 1;
                }
            }
        }
    }

    /// Count non-line leaf nodes in a flattened subtree.
    fn count_non_line_nodes(children: &[FlattenedKind]) -> usize {
        let mut count = 0;
        for child in children {
            match child {
                FlattenedKind::Group { children, .. } => {
                    count += Self::count_non_line_nodes(children);
                }
                FlattenedKind::Node(node) => {
                    if !matches!(node.kind, FlattenedNodeKind::Line { .. }) {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    /// Find Document root groups whose node indices are contained within the given set.
    fn find_contained_root_groups(
        doc: &Document,
        node_index_set: &HashSet<usize>,
    ) -> Vec<usize> {
        let roots = doc.roots();
        let mut result = Vec::new();
        for &group_idx in &roots {
            let group_node_indices = doc.collect_node_indices(group_idx);
            let all_contained = !group_node_indices.is_empty()
                && group_node_indices
                    .iter()
                    .all(|idx| node_index_set.contains(idx));
            if all_contained {
                result.push(group_idx);
            }
        }
        result
    }
}

impl AnalysisModule for TableDetector {
    fn name(&self) -> &'static str {
        "TableDetector"
    }

    fn process(&self, doc: &mut Document) {
        let tables = self.find_tables_from_flattened(doc);

        for table in tables {
            if table.rows.is_empty() {
                continue;
            }

            // Require at least 2 rows with substantive content (2+ non-line cells)
            let substantive_rows = table
                .rows
                .iter()
                .filter(|r| r.non_line_cell_count >= 2)
                .count();
            if substantive_rows < 2 {
                continue;
            }

            // First, create TableRow groups for each row
            let mut row_group_indices = Vec::new();
            for row in &table.rows {
                let row_node_set: HashSet<usize> = row.node_indices.iter().copied().collect();
                let row_root_groups = Self::find_contained_root_groups(doc, &row_node_set);
                if !row_root_groups.is_empty() {
                    let row_idx = doc.merge_inferred(
                        row_root_groups,
                        GroupKind::TableRow,
                        self.name(),
                    );
                    row_group_indices.push(row_idx);
                }
            }

            if row_group_indices.is_empty() {
                continue;
            }

            // Then, create the Table group wrapping all rows
            doc.merge_inferred(
                row_group_indices,
                GroupKind::Table {
                    column_widths: table.column_widths.clone(),
                },
                self.name(),
            );
        }
    }
}
