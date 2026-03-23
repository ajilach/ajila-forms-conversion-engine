//! Detects inset content between/after vertical radio buttons and wraps it in
//! `RadioButtonContent` groups so the structured converter can render it as conditional.
//!
//! # Algorithm
//!
//! For every `RadioButtonGroup` that is vertically stacked:
//!
//! 1. Sort its `RadioButton` children by their field-circle top-Y (ascending).
//! 2. Compute `rb_left` = the minimum left edge of all field circles.
//! 3. For each radio button `i`:
//!    - Collect all unclaimed root elements whose top-Y lies in
//!      `[rb_i.top, rb_{i+1}.top)` **and** whose left edge is ≥ `rb_left + 10pt`
//!      (the same "inset" definition used by `RadioButtonGrouper`).
//!    - For the **last** radio button the upper bound is open; we walk downward
//!      collecting inset roots and stop as soon as a non-inset root is encountered.
//! 4. Merge the collected elements into a `RadioButtonContent` group keyed by the
//!    option's `ExclGroupSomPath` and XFA field `name`.

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use crate::flattened::{Bounds, FlattenedNodeKind};
use crate::xfa::scripting::SomPath;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Detects inset content between/after vertical radio buttons.
pub struct RadioButtonContentDetector;

impl Default for RadioButtonContentDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl RadioButtonContentDetector {
    pub fn new() -> Self {
        Self
    }

    /// Return the bounds of the *field circle* (not the label) for a `RadioButton` group.
    fn get_field_bounds(&self, doc: &Document, rb_idx: usize) -> Option<Bounds> {
        let group = doc.get_group(rb_idx)?;
        if let GroupKind::RadioButton { field, .. } = &group.kind {
            let field_group_idx = *group.children.get(*field)?;
            doc.get_bounds(field_group_idx)
        } else {
            None
        }
    }

    /// Extract the `ExclGroupSomPath` and XFA field `name` from a `RadioButton` group.
    fn extract_option_info(&self, doc: &Document, rb_idx: usize) -> Option<(SomPath, String)> {
        let group = doc.get_group(rb_idx)?;
        if let GroupKind::RadioButton { field, .. } = &group.kind {
            let field_group_idx = *group.children.get(*field)?;
            let excl_path = doc.excl_group_som_path(field_group_idx)?;
            let nodes = doc.collect_nodes(field_group_idx);
            let field_name = nodes.iter().find_map(|node| {
                if let FlattenedNodeKind::Field { name, .. } = &node.kind {
                    Some(name.clone())
                } else {
                    None
                }
            })?;
            Some((excl_path, field_name))
        } else {
            None
        }
    }
}

impl AnalysisModule for RadioButtonContentDetector {
    fn name(&self) -> &'static str {
        "RadioButtonContentDetector"
    }

    fn process(&self, doc: &mut Document) {
        let inset_threshold = Decimal::from_str("10.0").unwrap();
        let alignment_tolerance = Decimal::from_str("10.0").unwrap();

        let rb_group_indices = doc.find_groups(|k| matches!(k, GroupKind::RadioButtonGroup));

        for rb_group_idx in rb_group_indices {
            let children = {
                let group = match doc.get_group(rb_group_idx) {
                    Some(g) => g,
                    None => continue,
                };
                group.children.clone()
            };

            // Sort RadioButton children by the top-Y of their field circle
            let mut rb_with_tops: Vec<(usize, Decimal)> = children
                .iter()
                .filter_map(|&child_idx| {
                    let fb = self.get_field_bounds(doc, child_idx)?;
                    Some((child_idx, fb.top()))
                })
                .collect();
            rb_with_tops.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

            if rb_with_tops.len() < 2 {
                continue;
            }

            // Only process vertical groups (first two share the same X within tolerance)
            let first_fb = match self.get_field_bounds(doc, rb_with_tops[0].0) {
                Some(b) => b,
                None => continue,
            };
            let second_fb = match self.get_field_bounds(doc, rb_with_tops[1].0) {
                Some(b) => b,
                None => continue,
            };
            if !first_fb.is_vertically_aligned(&second_fb, alignment_tolerance) {
                continue; // horizontal group — no inset content detection
            }

            // rb_left = minimum left edge of all field circles in this group
            let rb_left = rb_with_tops
                .iter()
                .filter_map(|(idx, _)| self.get_field_bounds(doc, *idx).map(|b| b.left()))
                .fold(Decimal::MAX, |a, b| a.min(b));

            let n = rb_with_tops.len();

            // Use the ExclGroupSomPath from the FIRST radio button (in sorted order) as the
            // canonical group identifier — exactly as `convert_radio_button_group` does.
            // That way every RadioButtonContent under this group maps to the same FieldId.
            let Some((shared_excl_path, _)) = self.extract_option_info(doc, rb_with_tops[0].0)
            else {
                continue;
            };

            for i in 0..n {
                let (rb_child_idx, rb_top) = rb_with_tops[i];
                let is_last = i == n - 1;

                let Some((_, field_name)) = self.extract_option_info(doc, rb_child_idx) else {
                    continue;
                };

                let content_indices: Vec<usize> = if !is_last {
                    // Non-last: collect all roots whose top is in [rb_top, next_rb_top) and
                    // whose left is at or to the right of the radio button circles.
                    // We don't require an inset here because the Y bounds already precisely
                    // define which option the content belongs to.
                    let y_end = rb_with_tops[i + 1].1;
                    let mut collected = doc
                        .roots()
                        .into_iter()
                        .filter(|&root_idx| root_idx != rb_group_idx)
                        .filter(|&root_idx| {
                            let Some(b) = doc.get_bounds(root_idx) else {
                                return false;
                            };
                            b.top() >= rb_top && b.top() < y_end && b.left() >= rb_left
                        })
                        .collect::<Vec<_>>();
                    // Sort by vertical position for deterministic ordering
                    collected.sort_by(|&a, &b| {
                        let ya = doc.get_bounds(a).map(|b| b.top()).unwrap_or_default();
                        let yb = doc.get_bounds(b).map(|b| b.top()).unwrap_or_default();
                        ya.partial_cmp(&yb).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    collected
                } else {
                    // Last radio button: walk downward from rb_top, collecting inset roots,
                    // stopping as soon as a non-inset root is encountered.
                    let mut candidates: Vec<(usize, Decimal)> = doc
                        .roots()
                        .into_iter()
                        .filter(|&root_idx| root_idx != rb_group_idx)
                        .filter_map(|root_idx| {
                            let b = doc.get_bounds(root_idx)?;
                            if b.top() > rb_top {
                                Some((root_idx, b.top()))
                            } else {
                                None
                            }
                        })
                        .collect();
                    candidates
                        .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

                    let mut collected = Vec::new();
                    for (root_idx, _) in candidates {
                        let b = doc.get_bounds(root_idx).unwrap();
                        if b.left() >= rb_left + inset_threshold {
                            collected.push(root_idx);
                        } else {
                            break; // stop at the first non-inset element
                        }
                    }
                    collected
                };

                if content_indices.is_empty() {
                    continue;
                }

                doc.merge_inferred(
                    content_indices,
                    GroupKind::RadioButtonContent {
                        excl_group_som_path: shared_excl_path.clone(),
                        option_field_name: field_name,
                    },
                    self.name(),
                );
            }
        }
    }
}
