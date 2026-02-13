//! Text block merger module.
//!
//! Merges adjacent TextBlock groups that have the same font size and weight
//! and are very close together vertically (< 0.5 × line height).
//! This runs after the TextBlockGrouper and before the HeadingDetector,
//! so that multi-line headings appear as a single TextBlock and are
//! assigned a single heading level.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::FlattenedNodeKind;
use crate::xfa::FontWeight;
use rust_decimal::prelude::*;

/// Merges vertically adjacent TextBlocks that share the same font properties.
///
/// Two TextBlocks are merged when:
/// 1. They have the same font size (rounded to 0.5pt).
/// 2. They have the same font weight (bold vs non-bold).
/// 3. Their vertical gap is less than 0.5 × the line height (max height of the two blocks).
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
                let bold = node
                    .style
                    .font
                    .as_ref()
                    .map(|f| f.weight == FontWeight::Bold)
                    .unwrap_or(false);

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
        let (top, bottom) = if bounds_a.y <= bounds_b.y {
            (&bounds_a, &bounds_b)
        } else {
            (&bounds_b, &bounds_a)
        };

        // Calculate vertical gap (bottom of top block to top of bottom block)
        let gap = bottom.y - top.bottom();

        // If they overlap vertically or gap is negative, they're on the same line or overlapping
        if gap < Decimal::ZERO {
            // They overlap vertically — check horizontal closeness instead
            // Only merge if they're very close horizontally
            return false;
        }

        // Use the max height of the two blocks as line height proxy
        let line_height = bounds_a.height.max(bounds_b.height);
        let threshold = line_height / Decimal::TWO;

        if gap > threshold {
            return false;
        }

        // They must have some horizontal overlap (not completely separate columns)
        let overlap_left = bounds_a.x.max(bounds_b.x);
        let overlap_right = bounds_a.right().min(bounds_b.right());

        // Require at least some horizontal overlap or very close horizontal proximity
        if overlap_right < overlap_left {
            // No horizontal overlap — check if they're close enough
            let horiz_gap = overlap_left - overlap_right;
            let max_width = bounds_a.width.max(bounds_b.width);
            // Allow small horizontal gap (< 20% of the wider block)
            if horiz_gap > max_width / Decimal::from(5) {
                return false;
            }
        }

        true
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

        // Greedy merge: iterate and merge consecutive blocks with same properties
        let mut merge_groups: Vec<Vec<usize>> = Vec::new();
        let mut used: std::collections::HashSet<usize> = std::collections::HashSet::new();

        for i in 0..text_blocks.len() {
            let (idx_a, ref props_a) = text_blocks[i];
            if used.contains(&idx_a) {
                continue;
            }

            let mut group = vec![idx_a];
            used.insert(idx_a);

            // Try to merge with subsequent blocks
            for j in (i + 1)..text_blocks.len() {
                let (idx_b, ref props_b) = text_blocks[j];
                if used.contains(&idx_b) {
                    continue;
                }

                // Must have matching font properties
                if props_a != props_b {
                    continue;
                }

                // Check spatial proximity against the last block in the group
                let last_in_group = *group.last().unwrap();
                if Self::should_merge(doc, last_in_group, idx_b) {
                    group.push(idx_b);
                    used.insert(idx_b);
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

            doc.merge(
                group_indices,
                GroupKind::TextBlock,
                GroupSource::Inferred {
                    module: self.name().to_string(),
                },
            );
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
            Page {
                width: num(595.0),
                height: num(842.0),
            },
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
            Page {
                width: num(595.0),
                height: num(842.0),
            },
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
            Page {
                width: num(595.0),
                height: num(842.0),
            },
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
            Page {
                width: num(595.0),
                height: num(842.0),
            },
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
}
