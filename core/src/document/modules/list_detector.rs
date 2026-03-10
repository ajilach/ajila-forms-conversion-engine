//! List detector module.
//!
//! Detects text blocks that start with list markers and groups them into
//! ordered or unordered lists.
//!
//! # Supported markers
//!
//! **Unordered:**
//! - `-`, `–` (en-dash), `—` (em-dash), `•`, `◦`, `▪`, `*`
//!
//! **Ordered:**
//! - `1.`, `2.`, ... (decimal numbers followed by `.` or `)`)
//! - `a.`, `b.`, ... or `A.`, `B.`, ... (letters followed by `.` or `)`)
//! - `i.`, `ii.`, `iv.`, ... (roman numerals followed by `.` or `)`)
//!
//! # Grouping rules
//!
//! Consecutive root TextBlock groups are grouped into a list when:
//! 1. Each starts with a recognized list marker (all ordered or all unordered).
//! 2. They have similar left indentation (x position within 5pt).
//! 3. They are vertically sequential (sorted by y position).
//!
//! After grouping, the marker prefix is stripped from each item's text content
//! so the structured converter receives clean item text.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource, ListStyleType};
use crate::flattened::Bounds;
use rust_decimal::prelude::*;
use std::collections::HashSet;

/// Maximum horizontal distance (in points) between text blocks' x positions
/// for them to be considered part of the same list.
const X_TOLERANCE: f64 = 5.0;

/// Maximum vertical distance (in points) between a standalone marker
/// TextBlock and a content TextBlock for them to be considered on the same line.
const Y_SAME_LINE_TOLERANCE: f64 = 2.0;

/// Detects and groups text blocks that form ordered or unordered lists.
pub struct ListDetector;

impl Default for ListDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ListDetector {
    pub fn new() -> Self {
        ListDetector
    }
}

/// A detected list marker with its kind and the byte length of the marker
/// prefix (including trailing whitespace) to strip.
#[derive(Debug, Clone)]
struct DetectedMarker {
    kind: ListStyleType,
    /// Number of bytes to strip from the start of the text content.
    #[allow(dead_code)] // used in tests
    prefix_len: usize,
}

/// Try to detect a list marker at the start of the given text.
///
/// Returns `Some(DetectedMarker)` if the text starts with a recognized marker
/// followed by at least one whitespace character (or end of string for
/// single-char bullet markers).
fn detect_marker(text: &str) -> Option<DetectedMarker> {
    let trimmed = text.trim_start();
    let leading_ws = text.len() - trimmed.len();

    if trimmed.is_empty() {
        return None;
    }

    // Check unordered markers: single characters
    // For non-ambiguous bullet chars (–, —, •, ◦, ▪), space after is optional.
    // For ambiguous chars (-, *), space after is required.

    // Disc: • (bullet)
    if trimmed.starts_with('\u{2022}') {
        let ch = '\u{2022}';
        let after = &trimmed[ch.len_utf8()..];
        let ws_after = after.len() - after.trim_start().len();
        return Some(DetectedMarker {
            kind: ListStyleType::Disc,
            prefix_len: leading_ws + ch.len_utf8() + ws_after,
        });
    }

    // Circle: ◦ (white bullet)
    if trimmed.starts_with('\u{25E6}') {
        let ch = '\u{25E6}';
        let after = &trimmed[ch.len_utf8()..];
        let ws_after = after.len() - after.trim_start().len();
        return Some(DetectedMarker {
            kind: ListStyleType::Circle,
            prefix_len: leading_ws + ch.len_utf8() + ws_after,
        });
    }

    // Square: ▪ (black small square)
    if trimmed.starts_with('\u{25AA}') {
        let ch = '\u{25AA}';
        let after = &trimmed[ch.len_utf8()..];
        let ws_after = after.len() - after.trim_start().len();
        return Some(DetectedMarker {
            kind: ListStyleType::Square,
            prefix_len: leading_ws + ch.len_utf8() + ws_after,
        });
    }

    // Dash markers: – (en-dash), — (em-dash) — space optional
    let dash_no_space = ['\u{2013}', '\u{2014}'];
    for &ch in &dash_no_space {
        if trimmed.starts_with(ch) {
            let after = &trimmed[ch.len_utf8()..];
            let ws_after = after.len() - after.trim_start().len();
            return Some(DetectedMarker {
                kind: ListStyleType::Dash,
                prefix_len: leading_ws + ch.len_utf8() + ws_after,
            });
        }
    }

    // Dash markers: -, * — space required
    let dash_need_space = ['-', '*'];
    for &ch in &dash_need_space {
        if trimmed.starts_with(ch) {
            let after = &trimmed[ch.len_utf8()..];
            if after.is_empty() || after.starts_with(char::is_whitespace) {
                let ws_after = after.len() - after.trim_start().len();
                return Some(DetectedMarker {
                    kind: ListStyleType::Dash,
                    prefix_len: leading_ws + ch.len_utf8() + ws_after,
                });
            }
        }
    }

    // Check ordered markers: <digits>. or <digits>)
    if let Some(marker) = detect_numeric_marker(trimmed) {
        return Some(DetectedMarker {
            kind: ListStyleType::Decimal,
            prefix_len: leading_ws + marker,
        });
    }

    // Check ordered markers: <letter>. or <letter>)
    if let Some((marker, is_upper)) = detect_letter_marker(trimmed) {
        return Some(DetectedMarker {
            kind: if is_upper {
                ListStyleType::UpperAlpha
            } else {
                ListStyleType::LowerAlpha
            },
            prefix_len: leading_ws + marker,
        });
    }

    // Check ordered markers: <roman>. or <roman>)
    if let Some((marker, is_upper)) = detect_roman_marker(trimmed) {
        return Some(DetectedMarker {
            kind: if is_upper {
                ListStyleType::UpperRoman
            } else {
                ListStyleType::LowerRoman
            },
            prefix_len: leading_ws + marker,
        });
    }

    None
}

/// Detect a numeric marker like "1.", "12.", "3)" at the start of text.
/// Returns the byte length of the marker + trailing whitespace.
fn detect_numeric_marker(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;

    // Must start with at least one digit
    if i >= bytes.len() || !bytes[i].is_ascii_digit() {
        return None;
    }

    // Consume digits
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }

    // Must be followed by '.' or ')'
    if i >= bytes.len() || (bytes[i] != b'.' && bytes[i] != b')') {
        return None;
    }
    i += 1;

    // Must be followed by whitespace or end of string
    let after = &text[i..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }

    let ws_after = after.len() - after.trim_start().len();
    Some(i + ws_after)
}

/// Detect a letter marker like "a.", "b)", "A." at the start of text.
/// Single letter only (not multi-letter like "aa.").
/// Returns the byte length of the marker + trailing whitespace, and whether uppercase.
fn detect_letter_marker(text: &str) -> Option<(usize, bool)> {
    let bytes = text.as_bytes();

    // Must be exactly one ASCII letter
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() {
        return None;
    }

    // Second char must be '.' or ')'
    if bytes[1] != b'.' && bytes[1] != b')' {
        return None;
    }

    // Must be followed by whitespace or end of string
    let after = &text[2..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }

    // Exclude roman numeral single letters that would be ambiguous
    // (handled by roman marker detector already — but 'i', 'v', 'x' etc.
    // are valid letter markers too, so we keep them)

    let ws_after = after.len() - after.trim_start().len();
    let is_upper = bytes[0].is_ascii_uppercase();
    Some((2 + ws_after, is_upper))
}

/// Detect a roman numeral marker like "i.", "ii.", "iv.", "xi)" at the start of text.
/// Returns the byte length of the marker + trailing whitespace, and whether uppercase.
fn detect_roman_marker(text: &str) -> Option<(usize, bool)> {
    let bytes = text.as_bytes();
    let mut i = 0;

    // Must start with a roman numeral character (lowercase)
    let roman_chars = b"ivxlcdm";
    if i >= bytes.len() || !roman_chars.contains(&bytes[i].to_ascii_lowercase()) {
        return None;
    }

    // Consume roman numeral characters (case-insensitive, but must be consistent)
    let is_upper = bytes[0].is_ascii_uppercase();
    while i < bytes.len() {
        let ch = bytes[i];
        let is_roman = if is_upper {
            roman_chars.contains(&ch.to_ascii_lowercase()) && ch.is_ascii_uppercase()
        } else {
            roman_chars.contains(&ch) && ch.is_ascii_lowercase()
        };
        if !is_roman {
            break;
        }
        i += 1;
    }

    // Must have consumed at least 2 roman chars (single char is a letter marker)
    // Exception: 'i' alone is a valid roman numeral but also a valid letter marker
    if i < 2 {
        return None;
    }

    // Must be followed by '.' or ')'
    if i >= bytes.len() || (bytes[i] != b'.' && bytes[i] != b')') {
        return None;
    }
    i += 1;

    // Must be followed by whitespace or end of string
    let after = &text[i..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }

    let ws_after = after.len() - after.trim_start().len();
    Some((i + ws_after, is_upper))
}

/// Check if a text block contains *only* a list marker (with optional whitespace).
/// Returns the marker kind if so.
fn is_standalone_marker(text: &str) -> Option<ListStyleType> {
    let marker = detect_marker(text)?;
    // After stripping the marker prefix the remaining text must be empty.
    let remaining = text[marker.prefix_len..].trim();
    if remaining.is_empty() {
        Some(marker.kind)
    } else {
        None
    }
}

/// Pre-processing: merge standalone marker TextBlocks with their adjacent
/// content TextBlocks that sit on the same line to the right.
///
/// In some PDFs the list bullet/dash is a separate text run positioned to the
/// left of the item text.  Without this merge the list detector cannot
/// recognise the items because neither the marker-only block nor the content
/// block satisfies the detection criteria on its own.
fn merge_standalone_markers(doc: &mut Document, module_name: &str) {
    let roots = doc.roots();
    let y_tol = Decimal::from_f64(Y_SAME_LINE_TOLERANCE).unwrap_or(Decimal::TWO);

    let mut merges: Vec<(usize, usize)> = Vec::new(); // (marker_idx, content_idx)
    let mut used: HashSet<usize> = HashSet::new();

    // Collect root TextBlocks with their bounds, sorted by position.
    let mut root_tbs: Vec<(usize, Bounds)> = Vec::new();
    for &idx in &roots {
        if !matches!(doc.groups[idx].kind, GroupKind::TextBlock) {
            continue;
        }
        if let Some(b) = doc.get_bounds(idx) {
            root_tbs.push((idx, b));
        }
    }

    // Sort by y then x so we can find same-line neighbours.
    root_tbs.sort_by(|a, b| a.1.y.cmp(&b.1.y).then(a.1.x.cmp(&b.1.x)));

    for i in 0..root_tbs.len() {
        let (marker_idx, ref marker_bounds) = root_tbs[i];
        if used.contains(&marker_idx) {
            continue;
        }

        let text = doc.get_text_content(marker_idx);
        if is_standalone_marker(&text).is_none() {
            continue;
        }

        // Find the next root TextBlock on the same line to the right.
        for j in (i + 1)..root_tbs.len() {
            let (content_idx, ref content_bounds) = root_tbs[j];
            if used.contains(&content_idx) {
                continue;
            }

            // Past the line – stop looking.
            if (content_bounds.y - marker_bounds.y).abs() > y_tol {
                break;
            }

            // Content must be to the right of the marker.
            if content_bounds.x <= marker_bounds.x {
                continue;
            }

            // The content block must not itself be a standalone marker (avoid
            // merging two markers).
            let content_text = doc.get_text_content(content_idx);
            if is_standalone_marker(&content_text).is_some() {
                continue;
            }

            merges.push((marker_idx, content_idx));
            used.insert(marker_idx);
            used.insert(content_idx);
            break;
        }
    }

    // Apply merges: wrap each (marker, content) pair in a new TextBlock.
    for (marker_idx, content_idx) in merges {
        doc.merge(
            vec![marker_idx, content_idx],
            GroupKind::TextBlock,
            GroupSource::Inferred {
                module: module_name.to_string(),
            },
        );
    }
}
impl AnalysisModule for ListDetector {
    fn name(&self) -> &'static str {
        "ListDetector"
    }

    fn process(&self, doc: &mut Document) {
        // Phase 0: Merge standalone marker blocks (e.g. a "– " text node)
        // with their adjacent content blocks on the same line.  This handles
        // PDFs where the bullet character is a separate text run.
        merge_standalone_markers(doc, self.name());

        // Phase 1: Walk root groups in document order.  For each TextBlock
        // that starts with a list marker we record it as a candidate.  Any
        // non-TextBlock root (field, checkbox, heading, …) or a TextBlock
        // without a marker acts as a separator that breaks the current run
        // of list items.
        //
        // Composite groups (headings, field labels, …) created by later
        // modules have higher indices than TextBlocks, so iterating roots
        // by index would not see them between TextBlocks.  To account for
        // this, we pre-collect all non-TextBlock roots with their y-positions
        // and check for intervening content when extending a run.
        let roots = doc.roots();
        let x_tol = Decimal::from_f64(X_TOLERANCE).unwrap_or(Decimal::new(50, 1));

        // Collect y-positions of all non-TextBlock root groups.  If any of
        // these sit between two consecutive list candidates, the run must be
        // broken because there is other content separating them.
        let non_tb_root_ys: Vec<Decimal> = roots
            .iter()
            .filter(|&&idx| !matches!(doc.groups[idx].kind, GroupKind::TextBlock))
            .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.y))
            .collect();

        // Each entry: (group_idx, text, bounds, marker)
        let mut current_run: Vec<(usize, String, Bounds, DetectedMarker)> = Vec::new();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut group_styles: Vec<ListStyleType> = Vec::new();

        let flush = |run: &mut Vec<(usize, String, Bounds, DetectedMarker)>,
                     groups: &mut Vec<Vec<usize>>,
                     group_styles: &mut Vec<ListStyleType>| {
            if run.len() >= 2 {
                let child_indices: Vec<usize> = run.iter().map(|(idx, _, _, _)| *idx).collect();
                let kind = run[0].3.kind;
                groups.push(child_indices);
                group_styles.push(kind);
            }
            run.clear();
        };

        for &idx in &roots {
            // Only TextBlock groups can be list items
            if !matches!(doc.groups[idx].kind, GroupKind::TextBlock) {
                // Non-TextBlock root breaks any ongoing list run
                flush(&mut current_run, &mut groups, &mut group_styles);
                continue;
            }

            let text = doc.get_text_content(idx);
            let bounds = match doc.get_bounds(idx) {
                Some(b) => b,
                None => {
                    flush(&mut current_run, &mut groups, &mut group_styles);
                    continue;
                }
            };

            if let Some(marker) = detect_marker(&text) {
                if let Some(last) = current_run.last() {
                    let same_kind = marker.kind == last.3.kind;
                    let similar_x = (bounds.x - last.2.x).abs() <= x_tol;

                    // Check whether any non-TextBlock root group sits
                    // between the previous item and this candidate (by
                    // y-position).  If so, there is other content between
                    // them and they belong to separate lists.
                    let last_bottom = last.2.y + last.2.height;
                    let curr_top = bounds.y;
                    let (range_lo, range_hi) = if last_bottom <= curr_top {
                        (last_bottom, curr_top)
                    } else {
                        (curr_top, last_bottom)
                    };
                    let has_intervening =
                        non_tb_root_ys.iter().any(|&y| y > range_lo && y < range_hi);

                    if same_kind && similar_x && !has_intervening {
                        current_run.push((idx, text, bounds, marker));
                    } else {
                        flush(&mut current_run, &mut groups, &mut group_styles);
                        current_run.push((idx, text, bounds, marker));
                    }
                } else {
                    current_run.push((idx, text, bounds, marker));
                }
            } else {
                // TextBlock without a marker also breaks the run
                flush(&mut current_run, &mut groups, &mut group_styles);
            }
        }

        // Flush the final run
        flush(&mut current_run, &mut groups, &mut group_styles);

        // For each list group, merge into a List group
        for (group_indices, list_style) in groups.into_iter().zip(group_styles) {
            let child_group_indices = group_indices;

            doc.merge(
                child_group_indices,
                GroupKind::List { list_style },
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
    use crate::document::{Document, GroupKind, ListStyleType};
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::num;

    #[test]
    fn test_detect_unordered_dash() {
        let m = detect_marker("- Item one").unwrap();
        assert_eq!(m.kind, ListStyleType::Dash);
        assert_eq!(m.prefix_len, 2);
    }

    #[test]
    fn test_detect_unordered_bullet() {
        let m = detect_marker("• Bullet item").unwrap();
        assert_eq!(m.kind, ListStyleType::Disc);
        assert_eq!(m.prefix_len, "• ".len());
    }

    #[test]
    fn test_detect_unordered_endash() {
        let m = detect_marker("– Item").unwrap();
        assert_eq!(m.kind, ListStyleType::Dash);
    }

    #[test]
    fn test_detect_ordered_number() {
        let m = detect_marker("1. First item").unwrap();
        assert_eq!(m.kind, ListStyleType::Decimal);
        assert_eq!(m.prefix_len, 3);
    }

    #[test]
    fn test_detect_ordered_number_paren() {
        let m = detect_marker("2) Second item").unwrap();
        assert_eq!(m.kind, ListStyleType::Decimal);
        assert_eq!(m.prefix_len, 3);
    }

    #[test]
    fn test_detect_ordered_letter() {
        let m = detect_marker("a. First sub-item").unwrap();
        assert_eq!(m.kind, ListStyleType::LowerAlpha);
        assert_eq!(m.prefix_len, 3);
    }

    #[test]
    fn test_detect_ordered_letter_upper() {
        let m = detect_marker("A) First sub-item").unwrap();
        assert_eq!(m.kind, ListStyleType::UpperAlpha);
        assert_eq!(m.prefix_len, 3);
    }

    #[test]
    fn test_detect_ordered_roman() {
        let m = detect_marker("ii. Second roman item").unwrap();
        assert_eq!(m.kind, ListStyleType::LowerRoman);
        assert_eq!(m.prefix_len, 4);
    }

    #[test]
    fn test_detect_no_marker() {
        assert!(detect_marker("Just normal text").is_none());
        assert!(detect_marker("").is_none());
        assert!(detect_marker("Hello world").is_none());
    }

    #[test]
    fn test_detect_no_marker_dash_no_space() {
        // "-word" without space should not match
        assert!(detect_marker("-word").is_none());
    }

    #[test]
    fn test_groups_unordered_list() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_text(
                    "- First item".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(20.0),
                    num(100.0),
                    num(200.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "- Second item".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(20.0),
                    num(114.0),
                    num(200.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "- Third item".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(20.0),
                    num(128.0),
                    num(200.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        ListDetector::new().process(&mut doc);

        let lists = doc.find_groups(|k| matches!(k, GroupKind::List { .. }));
        assert_eq!(lists.len(), 1, "Should detect one list");

        let list_group = doc.get_group(lists[0]).unwrap();
        assert_eq!(list_group.children.len(), 3, "List should have 3 items");

        if let GroupKind::List { list_style } = &list_group.kind {
            assert!(!list_style.is_ordered(), "Should be an unordered list");
            assert_eq!(*list_style, ListStyleType::Dash);
        } else {
            panic!("Expected List group kind");
        }
    }

    #[test]
    fn test_groups_ordered_list() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_text(
                    "1. First".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(30.0),
                    num(100.0),
                    num(200.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "2. Second".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(30.0),
                    num(114.0),
                    num(200.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        ListDetector::new().process(&mut doc);

        let lists = doc.find_groups(|k| matches!(k, GroupKind::List { .. }));
        assert_eq!(lists.len(), 1, "Should detect one ordered list");

        let list_group = doc.get_group(lists[0]).unwrap();
        if let GroupKind::List { list_style } = &list_group.kind {
            assert!(list_style.is_ordered(), "Should be an ordered list");
            assert_eq!(*list_style, ListStyleType::Decimal);
        } else {
            panic!("Expected List group kind");
        }
    }

    #[test]
    fn test_does_not_group_mixed_markers() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_text(
                    "- Unordered item".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(20.0),
                    num(100.0),
                    num(200.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "1. Ordered item".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(20.0),
                    num(114.0),
                    num(200.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        ListDetector::new().process(&mut doc);

        let lists = doc.find_groups(|k| matches!(k, GroupKind::List { .. }));
        assert_eq!(
            lists.len(),
            0,
            "Mixed markers should not form a list (need at least 2 of same kind)"
        );
    }

    #[test]
    fn test_does_not_group_single_item() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![FlattenedNode::new_text(
                "- Only one item".to_string(),
                num(10.0),
                "Helvetica".to_string(),
                num(20.0),
                num(100.0),
                num(200.0),
                num(12.0),
            )],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        ListDetector::new().process(&mut doc);

        let lists = doc.find_groups(|k| matches!(k, GroupKind::List { .. }));
        assert_eq!(lists.len(), 0, "Single item should not form a list");
    }

    #[test]
    fn test_non_list_text_blocks_untouched() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_text(
                    "Normal text".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(20.0),
                    num(100.0),
                    num(200.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "- List item 1".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(20.0),
                    num(130.0),
                    num(200.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "- List item 2".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(20.0),
                    num(144.0),
                    num(200.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "More normal text".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(20.0),
                    num(170.0),
                    num(200.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        ListDetector::new().process(&mut doc);

        let lists = doc.find_groups(|k| matches!(k, GroupKind::List { .. }));
        assert_eq!(lists.len(), 1, "Should find exactly one list");

        // The non-list text blocks should remain as root TextBlocks
        let roots = doc.roots();
        let root_text_blocks: Vec<usize> = roots
            .iter()
            .filter(|&&idx| matches!(doc.groups[idx].kind, GroupKind::TextBlock))
            .copied()
            .collect();
        assert_eq!(
            root_text_blocks.len(),
            2,
            "Two normal text blocks should remain as roots"
        );
    }

    #[test]
    fn test_different_x_positions_not_grouped() {
        // Text blocks at very different x positions should not be grouped
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                FlattenedNode::new_text(
                    "- Left item".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(20.0),
                    num(100.0),
                    num(200.0),
                    num(12.0),
                ),
                FlattenedNode::new_text(
                    "- Right item".to_string(),
                    num(10.0),
                    "Helvetica".to_string(),
                    num(300.0), // far to the right
                    num(114.0),
                    num(200.0),
                    num(12.0),
                ),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        ListDetector::new().process(&mut doc);

        let lists = doc.find_groups(|k| matches!(k, GroupKind::List { .. }));
        assert_eq!(
            lists.len(),
            0,
            "Items at different x positions should not be grouped"
        );
    }
}
