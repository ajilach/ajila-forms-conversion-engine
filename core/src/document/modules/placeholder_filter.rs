//! Placeholder filter module.
//!
//! Detects and claims text nodes that consist entirely of repeated placeholder
//! characters such as dots (`"............."`) or underscores (`"___________"`).
//! These are common in non-XFA PDF forms as visual fillers for empty fields and
//! should not appear in the structured output.
//!
//! This module should run early in the pipeline—after `TextBlockGrouper` wraps
//! text nodes—so that placeholder text is claimed before downstream modules
//! (e.g. `LabelAttacher`, `HeadingDetector`) try to use it.

use super::AnalysisModule;
use crate::document::{Document, GroupKind};
use crate::flattened::FlattenedNodeKind;

/// Claims text nodes whose content is nothing but repeated placeholder characters.
pub struct PlaceholderFilter;

impl Default for PlaceholderFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaceholderFilter {
    pub fn new() -> Self {
        PlaceholderFilter
    }

    /// Returns `true` if `text` looks like a placeholder filler.
    ///
    /// A string is considered a placeholder when, after trimming whitespace, it
    /// consists entirely of one or more "filler" characters:
    ///
    /// - `.` (dots)
    /// - `_` (underscores)
    /// - `…` (ellipsis)
    /// - `-` (dashes / hyphens)
    ///
    /// Short strings (fewer than 3 filler characters) are *not* treated as
    /// placeholders to avoid false positives (e.g. a single dot used as a
    /// decimal separator or a single dash as a minus sign).
    pub fn is_placeholder(text: &str) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return false;
        }

        let filler_count = trimmed
            .chars()
            .filter(|c| matches!(c, '.' | '_' | '…' | '-' | '–' | '—'))
            .count();

        // All characters must be filler characters, and there must be at least 3
        filler_count == trimmed.chars().count() && filler_count >= 3
    }
}

impl AnalysisModule for PlaceholderFilter {
    fn name(&self) -> &'static str {
        "PlaceholderFilter"
    }

    fn process(&self, doc: &mut Document) {
        let mut placeholder_groups = Vec::new();

        for (idx, group) in doc.groups.iter().enumerate() {
            if doc.is_claimed(idx) {
                continue;
            }

            // Check both bare Leaf text nodes and TextBlock groups
            let text = match &group.kind {
                GroupKind::Leaf { node_index } => {
                    if let Some(node) = doc.get_node(*node_index) {
                        if let FlattenedNodeKind::Text { content, .. } = &node.kind {
                            Some(content.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                GroupKind::TextBlock => Some(doc.get_text_content(idx)),
                _ => None,
            };

            if let Some(content) = text {
                if Self::is_placeholder(&content) {
                    placeholder_groups.push(idx);
                }
            }
        }

        // Claim each placeholder by wrapping it in a NoPrint group
        for group_idx in placeholder_groups {
            doc.merge_inferred(
                vec![group_idx],
                GroupKind::NoPrint,
                self.name(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dots_are_placeholder() {
        assert!(PlaceholderFilter::is_placeholder("..............."));
        assert!(PlaceholderFilter::is_placeholder("  ..........  "));
    }

    #[test]
    fn test_underscores_are_placeholder() {
        assert!(PlaceholderFilter::is_placeholder("___________"));
        assert!(PlaceholderFilter::is_placeholder("  _________  "));
    }

    #[test]
    fn test_dashes_are_placeholder() {
        assert!(PlaceholderFilter::is_placeholder("----------"));
        assert!(PlaceholderFilter::is_placeholder("——————————"));
        assert!(PlaceholderFilter::is_placeholder("––––––––––"));
    }

    #[test]
    fn test_ellipsis_are_placeholder() {
        assert!(PlaceholderFilter::is_placeholder("………"));
    }

    #[test]
    fn test_short_strings_are_not_placeholder() {
        assert!(!PlaceholderFilter::is_placeholder("."));
        assert!(!PlaceholderFilter::is_placeholder(".."));
        assert!(!PlaceholderFilter::is_placeholder("_"));
        assert!(!PlaceholderFilter::is_placeholder("__"));
        assert!(!PlaceholderFilter::is_placeholder("-"));
    }

    #[test]
    fn test_real_text_is_not_placeholder() {
        assert!(!PlaceholderFilter::is_placeholder("Name"));
        assert!(!PlaceholderFilter::is_placeholder("Geburtsdatum"));
        assert!(!PlaceholderFilter::is_placeholder("Strasse, Nr."));
        assert!(!PlaceholderFilter::is_placeholder("Hello World"));
    }

    #[test]
    fn test_mixed_content_is_not_placeholder() {
        assert!(!PlaceholderFilter::is_placeholder("Name: ..........."));
        assert!(!PlaceholderFilter::is_placeholder("_____ test"));
    }

    #[test]
    fn test_empty_is_not_placeholder() {
        assert!(!PlaceholderFilter::is_placeholder(""));
        assert!(!PlaceholderFilter::is_placeholder("   "));
    }
}
