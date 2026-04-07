//! Overlapping text block merger module.
//!
//! Merges text blocks that are spatially contained within other text blocks.
//! When a smaller text block's bounds are mostly (≥80%) inside a larger text
//! block's bounds, they are merged:
//!
//! - If the inner block is toward the **left** side of the outer block,
//!   its text is joined as a **prefix** (inner text + " " + outer text).
//! - If the inner block is toward the **right** side, its text is joined
//!   as a **postfix** (outer text + " " + inner text).
//!
//! This handles XFA forms where bullet markers (e.g. "–") are separate
//! `<draw>` elements overlapping with paragraph text.

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use crate::flattened::Bounds;
use rust_decimal::prelude::ToPrimitive;

/// Minimum fraction of the inner block's area that must fall within
/// the outer block for the pair to be considered overlapping.
const CONTAINMENT_THRESHOLD: f64 = 0.80;

/// Merges text blocks that are spatially contained within other text blocks.
pub struct OverlappingTextBlockMerger;

impl Default for OverlappingTextBlockMerger {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlappingTextBlockMerger {
    pub fn new() -> Self {
        OverlappingTextBlockMerger
    }

    /// Given two bounds, determine which is "inner" (smaller, mostly contained)
    /// and which is "outer" (larger, containing). Returns `(outer_idx, inner_idx)`
    /// from the original indices, or `None` if neither contains the other.
    fn find_containment(
        bounds_a: &Bounds,
        bounds_b: &Bounds,
        idx_a: usize,
        idx_b: usize,
    ) -> Option<(usize, usize)> {
        let a_contains_b = bounds_a.contains_percentage(bounds_b);
        let b_contains_a = bounds_b.contains_percentage(bounds_a);

        if a_contains_b >= CONTAINMENT_THRESHOLD && a_contains_b >= b_contains_a {
            // A contains B → A is outer, B is inner
            Some((idx_a, idx_b))
        } else if b_contains_a >= CONTAINMENT_THRESHOLD {
            // B contains A → B is outer, A is inner
            Some((idx_b, idx_a))
        } else {
            None
        }
    }
}

impl AnalysisModule for OverlappingTextBlockMerger {
    fn name(&self) -> &'static str {
        "OverlappingTextBlockMerger"
    }

    fn process(&self, doc: &mut Document) {
        // Collect all root TextBlock groups with their bounds
        let text_blocks: Vec<(usize, Bounds)> = doc
            .root_text_blocks()
            .into_iter()
            .filter_map(|idx| {
                let bounds = doc.get_bounds(idx)?;
                Some((idx, bounds))
            })
            .collect();

        // Find groups where small text blocks are contained within larger ones.
        // An outer (larger) block can absorb multiple inner (smaller) blocks.
        let mut consumed_inner: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut outer_inners: std::collections::BTreeMap<usize, Vec<usize>> =
            std::collections::BTreeMap::new();

        for i in 0..text_blocks.len() {
            let (idx_a, ref bounds_a) = text_blocks[i];
            if consumed_inner.contains(&idx_a) {
                continue;
            }

            for j in (i + 1)..text_blocks.len() {
                let (idx_b, ref bounds_b) = text_blocks[j];
                if consumed_inner.contains(&idx_b) {
                    continue;
                }

                if let Some((outer_idx, inner_idx)) =
                    Self::find_containment(bounds_a, bounds_b, idx_a, idx_b)
                {
                    // An inner block that was already assigned as an outer to
                    // other inners should not be consumed again.
                    if outer_inners.contains_key(&inner_idx) {
                        continue;
                    }
                    consumed_inner.insert(inner_idx);
                    outer_inners.entry(outer_idx).or_default().push(inner_idx);
                }
            }
        }

        // Perform merges: for each outer with its inner blocks, merge all.
        for (outer_idx, inner_indices) in outer_inners {
            let outer_bounds = match doc.get_bounds(outer_idx) {
                Some(b) => b,
                None => continue,
            };

            // Sort inner blocks: left-side inners first (by y, then x), then
            // outer, then right-side inners.
            let mut left_inners = Vec::new();
            let mut right_inners = Vec::new();
            for &inner_idx in &inner_indices {
                let inner_bounds = match doc.get_bounds(inner_idx) {
                    Some(b) => b,
                    None => continue,
                };
                if inner_bounds.center_x() < outer_bounds.center_x() {
                    left_inners.push((inner_idx, inner_bounds));
                } else {
                    right_inners.push((inner_idx, inner_bounds));
                }
            }

            // Sort left inners by y then x (reading order for prefixes)
            left_inners.sort_by(|a, b| a.1.y.cmp(&b.1.y).then(a.1.x.cmp(&b.1.x)));
            right_inners.sort_by(|a, b| a.1.y.cmp(&b.1.y).then(a.1.x.cmp(&b.1.x)));

            // Deduplicate: when multiple inner blocks have the same text
            // (e.g. two "–" marker glyphs for a multi-line item), keep
            // only the first occurrence on each side.
            let mut seen_left = std::collections::HashSet::new();
            left_inners.retain(|(idx, _)| {
                let text = doc.get_text_content(*idx);
                seen_left.insert(text)
            });
            let mut seen_right = std::collections::HashSet::new();
            right_inners.retain(|(idx, _)| {
                let text = doc.get_text_content(*idx);
                seen_right.insert(text)
            });

            let mut children: Vec<usize> = Vec::new();
            for (idx, _) in &left_inners {
                children.push(*idx);
            }
            children.push(outer_idx);
            for (idx, _) in &right_inners {
                children.push(*idx);
            }

            doc.merge_inferred(children, GroupKind::TextBlock, self.name());
        }

        // Second pass: Disabled for now due to causing heading detection issues.
        // TODO: Consider an alternate approach that doesn't affect headings.
        // self.merge_narrow_marker_columns(doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::document::modules::TextBlockGrouper;
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;

    #[test]
    fn test_merges_inner_block_as_prefix() {
        // Small text block on the left, fully inside a wider block → prefix
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Outer: wide text block
                FlattenedNode::new_text(
                    "main paragraph text here".to_string(),
                    num(8.0),
                    "Helvetica".to_string(),
                    num(10.0),  // x
                    num(100.0), // y
                    num(400.0), // width
                    num(12.0),  // height
                ),
                // Inner: narrow dash on the left side, within outer's bounds
                FlattenedNode::new_text(
                    "\u{2013}".to_string(), // en-dash
                    num(8.0),
                    "Helvetica".to_string(),
                    num(12.0),  // x — inside outer's x range, toward left
                    num(100.0), // y — same line
                    num(8.0),   // width — small
                    num(12.0),  // height — same as outer
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);

        assert_eq!(doc.root_text_blocks().len(), 2);

        OverlappingTextBlockMerger::new().process(&mut doc);

        let root_text_blocks = doc.root_text_blocks();
        assert_eq!(
            root_text_blocks.len(),
            1,
            "Inner block should be merged with outer block"
        );

        let text = doc.get_text_content(root_text_blocks[0]);
        // Dash is on the left → prefix: "– main paragraph text here"
        assert!(
            text.starts_with("\u{2013}"),
            "Merged text should start with the dash prefix, got: {}",
            text
        );
        assert!(
            text.contains("main paragraph text here"),
            "Merged text should contain the outer text"
        );
    }

    #[test]
    fn test_merges_inner_block_as_postfix() {
        // Small text block on the right, inside a wider block → postfix
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Outer: wide text block
                FlattenedNode::new_text(
                    "main paragraph text here".to_string(),
                    num(8.0),
                    "Helvetica".to_string(),
                    num(10.0),  // x
                    num(100.0), // y
                    num(400.0), // width
                    num(12.0),  // height
                ),
                // Inner: narrow marker on the right side, within outer's bounds
                FlattenedNode::new_text(
                    "*".to_string(),
                    num(8.0),
                    "Helvetica".to_string(),
                    num(350.0), // x — right side of outer
                    num(100.0), // y — same line
                    num(8.0),   // width — small
                    num(12.0),  // height — same as outer
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);

        assert_eq!(doc.root_text_blocks().len(), 2);

        OverlappingTextBlockMerger::new().process(&mut doc);

        let root_text_blocks = doc.root_text_blocks();
        assert_eq!(
            root_text_blocks.len(),
            1,
            "Inner block should be merged with outer block"
        );

        let text = doc.get_text_content(root_text_blocks[0]);
        // Marker is on the right → postfix: "main paragraph text here *"
        assert!(
            text.ends_with("*"),
            "Merged text should end with the postfix marker, got: {}",
            text
        );
        assert!(
            text.starts_with("main paragraph text here"),
            "Merged text should start with the outer text"
        );
    }

    #[test]
    fn test_does_not_merge_non_overlapping() {
        // Two text blocks side by side, not overlapping → no merge
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_text(
                    "left block".to_string(),
                    num(8.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(100.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "right block".to_string(),
                    num(8.0),
                    "Helvetica".to_string(),
                    num(200.0), // clearly outside left block's bounds
                    num(100.0),
                    num(100.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        OverlappingTextBlockMerger::new().process(&mut doc);

        let root_text_blocks = doc.root_text_blocks();
        assert_eq!(
            root_text_blocks.len(),
            2,
            "Non-overlapping blocks should not be merged"
        );
    }
}
