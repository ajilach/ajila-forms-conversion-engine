//! Text block merger module.
//!
//! Merges adjacent TextBlock groups that have the same font size and weight
//! and are very close together vertically (gap < 0.5 × font line height).
//! The threshold is based on the font size, not the total block height,
//! so that tall multi-line blocks do not inflate the merge distance.
//! This runs after the TextBlockGrouper and before the HeadingDetector,
//! so that multi-line headings appear as a single TextBlock and are
//! assigned a single heading level.

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use crate::flattened::FlattenedNodeKind;
use rust_decimal::prelude::*;

/// Merges vertically adjacent TextBlocks that share the same font properties.
///
/// Two TextBlocks are merged when:
/// 1. They have the same font size (rounded to 0.5pt).
/// 2. They have the same font weight (bold vs non-bold).
/// 3. Their vertical gap is less than 0.5 × the font-derived line height.
/// 4. They have reasonable horizontal overlap (not completely separated horizontally).
pub struct TextBlockMerger;

impl Default for TextBlockMerger {
    fn default() -> Self {
        Self::new()
    }
}

/// Font properties extracted from a TextBlock for comparison.
#[derive(Debug, Clone, PartialEq)]
struct TextBlockProps {
    /// Font size rounded to 0.5pt for comparison
    font_size_half_pt: u32,
    /// Whether the text is bold
    is_bold: bool,
}

impl TextBlockMerger {
    pub fn new() -> Self {
        TextBlockMerger
    }

    /// Extract font properties from a TextBlock group.
    /// Returns None if the group contains no text nodes or has mixed properties.
    fn get_text_block_props(doc: &Document, group_idx: usize) -> Option<TextBlockProps> {
        let nodes = doc.collect_nodes(group_idx);
        if nodes.is_empty() {
            return None;
        }

        let mut font_size: Option<f32> = None;
        let mut is_bold: Option<bool> = None;

        for node in &nodes {
            if let FlattenedNodeKind::Text {
                font_size: fs,
                content,
                ..
            } = &node.kind
            {
                if content.trim().is_empty() {
                    continue;
                }

                let size = fs.to_f32().unwrap_or(10.0);
                let rounded = (size * 2.0).round() / 2.0;
                let bold = node.is_bold();

                match (font_size, is_bold) {
                    (None, None) => {
                        font_size = Some(rounded);
                        is_bold = Some(bold);
                    }
                    (Some(existing_size), Some(existing_bold)) => {
                        // Allow merging only if all text nodes share the same properties
                        if (existing_size - rounded).abs() > 0.01 || existing_bold != bold {
                            return None;
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }

        Some(TextBlockProps {
            font_size_half_pt: font_size?.to_bits(),
            is_bold: is_bold?,
        })
    }

    /// Extract the dominant (max) font size from a TextBlock's text nodes.
    fn dominant_font_size(doc: &Document, idx: usize) -> Option<Decimal> {
        doc.collect_nodes(idx)
            .iter()
            .filter_map(|n| {
                if let FlattenedNodeKind::Text {
                    font_size, content, ..
                } = &n.kind
                {
                    if !content.trim().is_empty() {
                        Some(*font_size)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .max()
    }

    /// Check whether two TextBlocks are close enough to merge.
    /// Returns true if the vertical gap between them is less than 0.5 × line height.
    fn should_merge(doc: &Document, idx_a: usize, idx_b: usize) -> bool {
        let bounds_a = match doc.get_bounds(idx_a) {
            Some(b) => b,
            None => return false,
        };
        let bounds_b = match doc.get_bounds(idx_b) {
            Some(b) => b,
            None => return false,
        };

        // Determine which is above and which is below
        let top_idx = if bounds_a.y <= bounds_b.y {
            idx_a
        } else {
            idx_b
        };
        let (top, bottom) = if bounds_a.y <= bounds_b.y {
            (&bounds_a, &bounds_b)
        } else {
            (&bounds_b, &bounds_a)
        };

        // Don't merge if the upper block ends with a colon ':'.
        // A colon typically indicates an introductory phrase or heading
        // that should remain separate from the content that follows.
        let top_text = doc.get_text_content(top_idx);
        let trimmed = top_text.trim();
        if trimmed.ends_with(':') || trimmed.ends_with("：") {
            return false;
        }

        // Calculate vertical gap (bottom of top block to top of bottom block)
        let gap = bottom.y - top.bottom();

        // If they overlap vertically or gap is negative, they're on the same line or overlapping
        if gap < Decimal::ZERO {
            return false;
        }

        // Use the font size as line-height proxy so that the merge threshold
        // scales with the actual text size, not the total block height.
        // This prevents tall multi-line paragraphs from inflating the
        // threshold and absorbing unrelated nearby text.
        // Fall back to the smaller block height when no font info is available
        // (single-line blocks where block height ≈ line height).
        let font_a = Self::dominant_font_size(doc, idx_a);
        let font_b = Self::dominant_font_size(doc, idx_b);
        let line_height = match (font_a, font_b) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => bounds_a.height.min(bounds_b.height),
        };
        let threshold = line_height / Decimal::TWO;

        if gap > threshold {
            return false;
        }

        // They must have some horizontal overlap (not completely separate columns)
        let overlap_left = bounds_a.x.max(bounds_b.x);
        let overlap_right = bounds_a.right().min(bounds_b.right());

        // Require horizontal overlap or very small gap. Same-column blocks share
        // a left margin and always overlap. A large gap (> 15pt) indicates different
        // columns that shouldn't be merged.
        if overlap_right < overlap_left {
            let horiz_gap = overlap_left - overlap_right;
            let small_gap_tolerance = Decimal::from(15);
            if horiz_gap > small_gap_tolerance {
                return false;
            }
        }

        // Don't merge blocks that aren't in the same horizontal flow.
        // With text-content bounds, line widths vary naturally within a
        // paragraph (e.g. a final short line), so we check left-edge
        // alignment rather than width ratio.  Two blocks whose left
        // margins are close belong to the same column flow.
        let left_diff = (bounds_a.x - bounds_b.x).abs();
        let margin_tolerance = Decimal::from_str("15.0").unwrap();
        if left_diff > margin_tolerance {
            // Different left margins — use width ratio as a secondary guard
            // to prevent merging across columns.
            let narrow = bounds_a.width.min(bounds_b.width);
            let wide = bounds_a.width.max(bounds_b.width);
            if wide > Decimal::ZERO && narrow * Decimal::TWO < wide {
                return false;
            }
        }

        // Don't merge blocks with very different heights. A single-line
        // heading and a multi-line paragraph may share the same font
        // properties and be vertically close, but they are separate
        // logical elements (heading vs body text).
        let short_h = bounds_a.height.min(bounds_b.height);
        let tall_h = bounds_a.height.max(bounds_b.height);
        if tall_h > Decimal::ZERO && short_h * Decimal::TWO < tall_h {
            return false;
        }

        true
    }

    /// Check whether `idx_b` sits in the same text flow lane as `idx_a`.
    ///
    /// "Same lane" means the blocks share horizontal extent (with a small
    /// gap tolerance for slight misalignment within a single column) **and**
    /// are no more than one line-height apart vertically.
    ///
    /// This is used to decide whether a different-style block should stop
    /// scanning for later merge candidates. In multi-column layouts, unrelated
    /// blocks in another column should not block merging within the current
    /// column.
    fn is_same_flow_lane(doc: &Document, idx_a: usize, idx_b: usize) -> bool {
        let bounds_a = match doc.get_bounds(idx_a) {
            Some(b) => b,
            None => return false,
        };
        let bounds_b = match doc.get_bounds(idx_b) {
            Some(b) => b,
            None => return false,
        };

        let overlap_left = bounds_a.x.max(bounds_b.x);
        let overlap_right = bounds_a.right().min(bounds_b.right());

        let horizontal_aligned = if overlap_right >= overlap_left {
            true
        } else {
            let horiz_gap = overlap_left - overlap_right;
            let min_width = bounds_a.width.min(bounds_b.width);
            horiz_gap <= min_width / Decimal::from(5)
        };

        if !horizontal_aligned {
            return false;
        }

        let (top, bottom) = if bounds_a.y <= bounds_b.y {
            (&bounds_a, &bounds_b)
        } else {
            (&bounds_b, &bounds_a)
        };
        let vertical_gap = bottom.y - top.bottom();
        let line_height = bounds_a.height.max(bounds_b.height);

        vertical_gap <= line_height
    }

    /// Check whether `idx_b` is in the same column as `idx_a` based on
    /// strict horizontal overlap (no gap tolerance).
    ///
    /// Used when deciding whether a same-style block that doesn't merge
    /// should act as a barrier.  Two blocks in separate columns (no x
    /// overlap) must not block continuation scanning in the other column.
    fn is_same_column(doc: &Document, idx_a: usize, idx_b: usize) -> bool {
        let bounds_a = match doc.get_bounds(idx_a) {
            Some(b) => b,
            None => return false,
        };
        let bounds_b = match doc.get_bounds(idx_b) {
            Some(b) => b,
            None => return false,
        };
        let overlap_left = bounds_a.x.max(bounds_b.x);
        let overlap_right = bounds_a.right().min(bounds_b.right());
        overlap_right > overlap_left
    }
}

impl AnalysisModule for TextBlockMerger {
    fn name(&self) -> &'static str {
        "TextBlockMerger"
    }

    fn process(&self, doc: &mut Document) {
        // Collect all root TextBlock groups with their properties
        let roots = doc.roots();
        let mut text_blocks: Vec<(usize, TextBlockProps)> = Vec::new();

        for &idx in &roots {
            if !matches!(doc.groups[idx].kind, GroupKind::TextBlock) {
                continue;
            }
            if let Some(props) = Self::get_text_block_props(doc, idx) {
                text_blocks.push((idx, props));
            }
        }

        // Sort by vertical position (y), then horizontal (x)
        text_blocks.sort_by(|a, b| {
            let bounds_a = doc.get_bounds(a.0);
            let bounds_b = doc.get_bounds(b.0);
            match (bounds_a, bounds_b) {
                (Some(ba), Some(bb)) => ba.y.cmp(&bb.y).then(ba.x.cmp(&bb.x)),
                _ => std::cmp::Ordering::Equal,
            }
        });

        // Greedy merge: only merge contiguous runs in reading order.
        // If a block with different properties or non-mergeable spacing appears
        // between candidates, do not merge across it.
        let mut merge_groups: Vec<Vec<usize>> = Vec::new();
        let mut used: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for i in 0..text_blocks.len() {
            let (idx_a, ref props_a) = text_blocks[i];
            if used.contains(&idx_a) {
                continue;
            }

            let mut group = vec![idx_a];
            used.insert(idx_a);

            // Try to merge with immediately subsequent blocks only.
            for j in (i + 1)..text_blocks.len() {
                let (idx_b, ref props_b) = text_blocks[j];
                if used.contains(&idx_b) {
                    continue;
                }

                // A different-style block in the same flow lane acts as a
                // barrier (e.g. heading line between two labels). Blocks in
                // other columns should not block scanning.
                if props_a != props_b {
                    let last_in_group = *group.last().unwrap();
                    if Self::is_same_flow_lane(doc, last_in_group, idx_b) {
                        break;
                    }
                    continue;
                }

                // Check spatial proximity against the last block in the group
                let last_in_group = *group.last().unwrap();
                if Self::should_merge(doc, last_in_group, idx_b) {
                    group.push(idx_b);
                    used.insert(idx_b);
                } else {
                    // Same visual style but not close enough.
                    // Only treat as a barrier when the block is in the same
                    // column (strict horizontal overlap).  Blocks in a
                    // different column must not prevent scanning further in
                    // the current column.
                    if Self::is_same_column(doc, last_in_group, idx_b) {
                        break;
                    }
                }
            }

            if group.len() > 1 {
                merge_groups.push(group);
            }
        }

        // Create merged TextBlock groups
        for group_indices in merge_groups {
            // Collect all children from the original TextBlock groups
            let mut all_children: Vec<usize> = Vec::new();
            for &tb_idx in &group_indices {
                if let Some(group) = doc.get_group(tb_idx) {
                    all_children.extend_from_slice(&group.children);
                }
            }

            doc.merge_inferred(group_indices, GroupKind::TextBlock, self.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::modules::TextBlockGrouper;
    use crate::document::{Document, GroupKind};
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;

    #[test]
    fn test_merges_adjacent_blocks_same_font() {
        // Two text blocks with same font, close vertically → should merge
        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                FlattenedNode::new_text(
                    "Line one".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(100.0),
                    num(14.0),
                ),
                FlattenedNode::new_text(
                    "Line two".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(116.0), // gap = 116 - 114 = 2pt, threshold = 14 * 0.5 = 7pt → merge
                    num(100.0),
                    num(14.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);

        // Should have 2 Leaf + 2 TextBlock = 4 groups
        assert_eq!(
            doc.find_groups(|k| matches!(k, GroupKind::TextBlock)).len(),
            2
        );

        TextBlockMerger::new().process(&mut doc);

        // After merging: the 2 TextBlocks should be merged into 1 new TextBlock
        let root_text_blocks = doc.root_text_blocks();
        assert_eq!(
            root_text_blocks.len(),
            1,
            "Two adjacent blocks with same font should merge into one"
        );

        // Merged block should contain text from both
        let text = doc.get_text_content(root_text_blocks[0]);
        assert!(
            text.contains("Line one"),
            "Merged block should contain 'Line one'"
        );
        assert!(
            text.contains("Line two"),
            "Merged block should contain 'Line two'"
        );
    }

    #[test]
    fn test_does_not_merge_different_font_size() {
        // Two text blocks with different font sizes → should NOT merge
        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                FlattenedNode::new_text(
                    "Large text".to_string(),
                    num(16.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(100.0),
                    num(18.0),
                ),
                FlattenedNode::new_text(
                    "Small text".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(120.0),
                    num(100.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);

        let root_text_blocks = doc.root_text_blocks();
        assert_eq!(
            root_text_blocks.len(),
            2,
            "Blocks with different font sizes should not merge"
        );
    }

    #[test]
    fn test_does_not_merge_far_apart() {
        // Two text blocks with same font but far apart → should NOT merge
        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                FlattenedNode::new_text(
                    "Top text".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(100.0),
                    num(14.0),
                ),
                FlattenedNode::new_text(
                    "Bottom text".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(200.0), // gap = 200 - 114 = 86pt, threshold = 7pt → no merge
                    num(100.0),
                    num(14.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);

        let root_text_blocks = doc.root_text_blocks();
        assert_eq!(
            root_text_blocks.len(),
            2,
            "Blocks far apart should not merge"
        );
    }

    #[test]
    fn test_merges_three_adjacent_blocks() {
        // Three text blocks with same font, all close → should merge into one
        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                FlattenedNode::new_text(
                    "Line one".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(100.0),
                    num(14.0),
                ),
                FlattenedNode::new_text(
                    "Line two".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(116.0), // gap = 2pt < 7pt → merge with line one
                    num(100.0),
                    num(14.0),
                ),
                FlattenedNode::new_text(
                    "Line three".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(132.0), // gap = 132 - 130 = 2pt < 7pt → merge with line two
                    num(100.0),
                    num(14.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);

        let root_text_blocks = doc.root_text_blocks();
        assert_eq!(
            root_text_blocks.len(),
            1,
            "Three adjacent blocks with same font should merge into one"
        );

        let text = doc.get_text_content(root_text_blocks[0]);
        assert!(text.contains("Line one"));
        assert!(text.contains("Line two"));
        assert!(text.contains("Line three"));
    }

    #[test]
    fn test_does_not_merge_across_intervening_text_block() {
        // "Line A" and "Line C" have matching style and could merge by distance,
        // but "Middle heading" is between them and should block cross-merge.
        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                FlattenedNode::new_text(
                    "Line A".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(100.0),
                    num(14.0),
                ),
                FlattenedNode::new_text(
                    "Middle heading".to_string(),
                    num(14.0), // different size => different text block props
                    "Helvetica".to_string(),
                    num(10.0),
                    num(116.0),
                    num(140.0),
                    num(16.0),
                ),
                FlattenedNode::new_text(
                    "Line C".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(134.0),
                    num(100.0),
                    num(14.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);

        let root_text_blocks = doc.root_text_blocks();
        assert_eq!(
            root_text_blocks.len(),
            3,
            "Text blocks separated by an intervening block must not be merged"
        );
    }

    #[test]
    fn test_multi_column_interleaving_does_not_block_same_column_merge() {
        // Left-column lines should still merge even if a right-column text block
        // appears between them in global y/x sort order.
        let flattened = Flattened::from_nodes(
            Page::new(num(595.0), num(842.0)),
            vec![
                FlattenedNode::new_text(
                    "Left line 1".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(100.0),
                    num(100.0),
                    num(14.0),
                ),
                FlattenedNode::new_text(
                    "Right heading".to_string(),
                    num(14.0),
                    "Helvetica".to_string(),
                    num(250.0),
                    num(100.0),
                    num(120.0),
                    num(16.0),
                ),
                FlattenedNode::new_text(
                    "Left line 2".to_string(),
                    num(12.0),
                    "Helvetica".to_string(),
                    num(10.0),
                    num(116.0),
                    num(100.0),
                    num(14.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        TextBlockMerger::new().process(&mut doc);

        let root_text_blocks = doc.root_text_blocks();
        assert_eq!(
            root_text_blocks.len(),
            2,
            "Left column lines should merge; right column text remains separate"
        );

        let merged_texts: Vec<String> = root_text_blocks
            .iter()
            .map(|&idx| doc.get_text_content(idx))
            .collect();
        assert!(
            merged_texts
                .iter()
                .any(|t| t.contains("Left line 1") && t.contains("Left line 2")),
            "Expected merged left-column text block"
        );
    }
}
