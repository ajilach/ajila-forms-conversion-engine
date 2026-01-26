//! Text block grouper module.
//!
//! Wraps each flattened text node in its own TextBlock group.
//! This provides a one-to-one mapping from text nodes to TextBlock groups.

use crate::document::{Document, GroupKind, GroupSource};
use super::AnalysisModule;

/// Wraps each text node in its own TextBlock group.
///
/// Creates a one-to-one mapping: each flattened text node gets its own TextBlock.
pub struct TextBlockGrouper;

impl Default for TextBlockGrouper {
    fn default() -> Self {
        Self::new()
    }
}

impl TextBlockGrouper {
    pub fn new() -> Self {
        TextBlockGrouper
    }
}

impl AnalysisModule for TextBlockGrouper {
    fn name(&self) -> &'static str {
        "TextBlockGrouper"
    }
    
    fn process(&self, doc: &mut Document) {
        // Get all unclaimed text leaves
        let text_leaves = doc.unclaimed_text_leaves();
        
        // Wrap each text leaf in its own TextBlock group
        for leaf_idx in text_leaves {
            doc.merge(
                vec![leaf_idx],
                GroupKind::TextBlock,
                GroupSource::Inferred { module: self.name().to_string() },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, GroupKind};
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;
    
    #[test]
    fn test_each_text_gets_own_block() {
        // Each text node should be wrapped in its own TextBlock group
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                FlattenedNode::new_text(
                    "First".to_string(), num(10.0), "Helvetica".to_string(),
                    num(10.0), num(100.0), num(25.0), num(12.0),
                ),
                FlattenedNode::new_text(
                    "Name:".to_string(), num(10.0), "Helvetica".to_string(),
                    num(37.0), num(100.0), num(30.0), num(12.0),
                ),
                FlattenedNode::new_text(
                    "Address:".to_string(), num(10.0), "Helvetica".to_string(),
                    num(10.0), num(150.0), num(45.0), num(12.0),
                ),
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        assert_eq!(doc.groups.len(), 3); // 3 Leaf groups initially
        
        TextBlockGrouper::new().process(&mut doc);
        
        // Should have 3 Leaf groups + 3 TextBlock groups = 6 groups
        assert_eq!(doc.groups.len(), 6);
        
        // Should have 3 TextBlock groups
        let text_blocks: Vec<_> = doc.find_groups(|k| matches!(k, GroupKind::TextBlock));
        assert_eq!(text_blocks.len(), 3);
        
        // Each TextBlock should wrap exactly one Leaf
        for &tb_idx in &text_blocks {
            let group = doc.get_group(tb_idx).unwrap();
            assert_eq!(group.children.len(), 1);
        }
    }
}
