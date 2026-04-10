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
//! - `(1)`, `(2)`, ... (decimal numbers in parentheses)
//! - `(a)`, `(b)`, ... or `(A)`, `(B)`, ... (letters in parentheses)
//! - `(i)`, `(ii)`, `(iv)`, ... (roman numerals in parentheses)
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
use crate::document::{Document, GroupKind, ListStyleType};
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

    // Check parenthesized ordered markers: (1), (a), (i), (ii), etc.
    if let Some((marker, kind)) = detect_parenthesized_marker(trimmed) {
        return Some(DetectedMarker {
            kind,
            prefix_len: leading_ws + marker,
        });
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

/// Detect a parenthesized marker like "(i)", "(ii)", "(1)", "(a)" at the start of text.
/// Returns the byte length of the marker + trailing whitespace, and the detected kind.
fn detect_parenthesized_marker(text: &str) -> Option<(usize, ListStyleType)> {
    let bytes = text.as_bytes();

    // Must start with '('
    if bytes.is_empty() || bytes[0] != b'(' {
        return None;
    }

    // Find the closing ')'
    let close_paren = bytes.iter().position(|&b| b == b')')?;
    if close_paren < 2 {
        // Need at least "(X)" where X is content
        return None;
    }

    // Extract the content between parentheses
    let content = &text[1..close_paren];

    // Determine the kind of marker inside the parentheses
    let kind = if content.chars().all(|c| c.is_ascii_digit()) {
        // Numeric: (1), (2), (10), etc.
        ListStyleType::Decimal
    } else {
        // Check if it's a roman numeral first: (i), (ii), (iii), (iv), (I), (II), etc.
        // This includes single characters like 'i', 'v', 'x' which are common in legal docs
        let roman_chars = ['i', 'v', 'x', 'l', 'c', 'd', 'm'];
        let content_lower = content.to_ascii_lowercase();
        if content_lower.chars().all(|c| roman_chars.contains(&c)) {
            // Determine if uppercase or lowercase
            let is_upper = content
                .chars()
                .next()
                .map_or(false, |c| c.is_ascii_uppercase());
            if is_upper {
                ListStyleType::UpperRoman
            } else {
                ListStyleType::LowerRoman
            }
        } else if content.len() == 1
            && content
                .chars()
                .next()
                .map_or(false, |c| c.is_ascii_alphabetic())
        {
            // Single non-roman letter: (a), (b), (A), (B) - but not (i), (v), (x) which are roman
            let ch = content.chars().next().unwrap();
            if ch.is_ascii_uppercase() {
                ListStyleType::UpperAlpha
            } else {
                ListStyleType::LowerAlpha
            }
        } else {
            return None;
        }
    };

    // Calculate prefix length: opening paren + content + closing paren + trailing whitespace
    let marker_len = close_paren + 1; // includes '(' and ')'
    let after = &text[marker_len..];

    // Must be followed by whitespace or end of string
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }

    let ws_after = after.len() - after.trim_start().len();
    Some((marker_len + ws_after, kind))
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

            // Check if the content is on the "same line" as the marker.
            // Three criteria:
            //   a) Same y within tolerance (marker and content have similar top), OR
            //   b) Marker sits directly above content (marker bottom ≈ content top), OR
            //   c) Content top is within the marker's vertical extent (common in
            //      XFA forms where the marker draw has a tall bounding box that
            //      overlaps with the content draw below it).
            // Cases (b) and (c) handle XFA forms where the marker draw element is
            // positioned one line above the content draw element.
            let same_y = (content_bounds.y - marker_bounds.y).abs() <= y_tol;
            let marker_bottom = marker_bounds.y + marker_bounds.height;
            let marker_above = (content_bounds.y - marker_bottom).abs() <= y_tol;
            let content_within_marker =
                content_bounds.y > marker_bounds.y && content_bounds.y < marker_bottom;
            let on_same_line = same_y || marker_above || content_within_marker;

            // Past the line – stop looking.
            if !on_same_line {
                break;
            }

            // For same-line / marker-above: content must be to the right.
            // For content-within-marker: content can start at the same x
            // (same column, line below the marker at the same indent).
            if content_within_marker {
                if content_bounds.x + y_tol < marker_bounds.x {
                    continue;
                }
            } else if content_bounds.x <= marker_bounds.x {
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
        doc.merge_inferred(
            vec![marker_idx, content_idx],
            GroupKind::TextBlock,
            module_name,
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

        // Also collect bounds of root TextBlocks that are bold, have
        // non-empty text, and do NOT start with a list marker.  Bold
        // non-marker TextBlocks are headings or labels that naturally
        // separate lists.  Because OverlappingTextBlockMerger creates
        // groups with higher indices, these separator TextBlocks may
        // appear earlier in index order than the merged list items and
        // therefore won't be encountered between them during the
        // sequential Phase 1 walk.  Only bold TextBlocks are considered
        // to avoid breaking lists where a non-bold continuation paragraph
        // (e.g. multi-line item text) appears between marker items.
        let non_marker_tb_bounds: Vec<Bounds> = roots
            .iter()
            .filter(|&&idx| {
                if !matches!(doc.groups[idx].kind, GroupKind::TextBlock) {
                    return false;
                }
                if !doc.is_bold_group(idx) {
                    return false;
                }
                let text = doc.get_text_content(idx);
                let trimmed = text.trim();
                !trimmed.is_empty() && detect_marker(&text).is_none()
            })
            .filter_map(|&idx| doc.get_bounds(idx))
            .collect();

        // Each entry: (group_idx, text, bounds, marker)
        let mut current_run: Vec<(usize, String, Bounds, DetectedMarker)> = Vec::new();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut group_styles: Vec<ListStyleType> = Vec::new();

        let flush = |run: &mut Vec<(usize, String, Bounds, DetectedMarker)>,
                     groups: &mut Vec<Vec<usize>>,
                     group_styles: &mut Vec<ListStyleType>| {
            // Normally require >= 2 items to form a list.  However, a single
            // item with actual content after the marker prefix is also kept
            // so that Phase 2 backward extension can try to find preceding
            // items that were missing their markers.  A standalone marker
            // ("–" with no content) is never promoted alone because it does
            // not carry enough signal.
            let keep = if run.len() >= 2 {
                true
            } else if run.len() == 1 {
                let remaining = run[0].1[run[0].3.prefix_len..].trim();
                !remaining.is_empty()
            } else {
                false
            };
            if keep {
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

                    // Check whether any non-TextBlock root group or
                    // non-marker TextBlock root sits between the previous
                    // item and this candidate (by y-position).  If so,
                    // there is other content between them and they belong
                    // to separate lists.
                    let last_bottom = last.2.y + last.2.height;
                    let curr_top = bounds.y;
                    let (range_lo, range_hi) = if last_bottom <= curr_top {
                        (last_bottom, curr_top)
                    } else {
                        (curr_top, last_bottom)
                    };
                    let has_intervening = non_tb_root_ys
                        .iter()
                        .any(|&y| y > range_lo && y < range_hi)
                        || non_marker_tb_bounds.iter().any(|sep| {
                            // Check vertical overlap: separator spans
                            // [y, y+h], gap spans (range_lo, range_hi).
                            let sep_bottom = sep.y + sep.height;
                            let y_overlap = sep_bottom > range_lo && sep.y < range_hi;
                            // Check horizontal overlap: separator must
                            // share x-range with the list items to avoid
                            // false positives from unrelated columns.
                            let item_x_lo = last.2.x.min(bounds.x);
                            let item_x_hi = (last.2.x + last.2.width).max(bounds.x + bounds.width);
                            let x_overlap = sep.x < item_x_hi && (sep.x + sep.width) > item_x_lo;
                            // The separator must bridge to the next item:
                            // the gap between separator bottom and the
                            // next item's top must be less than the
                            // same-line tolerance.  This prevents body
                            // paragraphs (under numbered headings) from
                            // breaking the list — they typically end well
                            // above the next heading item.
                            let gap_after_sep = range_hi - sep_bottom;
                            let tol =
                                Decimal::from_f64(Y_SAME_LINE_TOLERANCE).unwrap_or(Decimal::TWO);
                            let bridges = gap_after_sep < tol;
                            y_overlap && x_overlap && bridges
                        });

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

        // Phase 2: Backward extension.
        //
        // Some XFA forms have list items where the marker text for the first
        // N items is missing (empty draw elements).  Only the last few items
        // receive visible markers.  Without this phase, the marker-less items
        // are left as plain TextBlocks and get merged into the preceding
        // paragraph by the TextBlockMerger.
        //
        // For each detected list group, we walk backwards through the root
        // TextBlocks looking for items that share the same x position and are
        // vertically contiguous.  We stop when we encounter a TextBlock whose
        // height exceeds 2× the tallest item in the confirmed list (heuristic
        // to avoid absorbing a preceding multi-line paragraph).
        //
        // NOTE: We re-query current roots here because Phase 0 and Phase 1
        // may have created new groups (merged standalone markers, lists).
        // We need the current state for accurate root detection.
        let current_roots: HashSet<usize> = doc.roots().iter().copied().collect();

        // Collect root TextBlocks sorted by y then x for backward scanning.
        // Use current roots, not the original `roots` captured at the start.
        let mut root_tb_sorted: Vec<(usize, Bounds)> = Vec::new();
        for &idx in &current_roots {
            if matches!(doc.groups[idx].kind, GroupKind::TextBlock) {
                if let Some(b) = doc.get_bounds(idx) {
                    root_tb_sorted.push((idx, b));
                }
            }
        }
        root_tb_sorted.sort_by(|a, b| a.1.y.cmp(&b.1.y).then(a.1.x.cmp(&b.1.x)));

        // Build a quick lookup: group_idx → position in root_tb_sorted
        let tb_pos: std::collections::HashMap<usize, usize> = root_tb_sorted
            .iter()
            .enumerate()
            .map(|(pos, (idx, _))| (*idx, pos))
            .collect();

        // Track indices already claimed by a list group so we don't double-count.
        let mut claimed: HashSet<usize> = HashSet::new();
        for group in &groups {
            for &idx in group {
                claimed.insert(idx);
            }
        }

        for (group_idx, group) in groups.iter_mut().enumerate() {
            if group.is_empty() {
                continue;
            }

            // Max height among confirmed list items (used as paragraph guard).
            let max_item_height = group
                .iter()
                .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.height))
                .max()
                .unwrap_or(Decimal::ZERO);
            let height_limit = max_item_height * Decimal::TWO;

            // Reference width: use the widest item in the group for
            // comparison.  This ensures backward candidates of normal width
            // are not rejected when the list also contains narrow standalone
            // markers.
            let ref_item_width = group
                .iter()
                .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.width))
                .max()
                .unwrap_or(Decimal::ZERO);

            let first_idx = group[0];
            // Find the topmost item by y position for backward scanning.
            let (topmost_idx, topmost_bounds) = group
                .iter()
                .filter_map(|&idx| doc.get_bounds(idx).map(|b| (idx, b)))
                .min_by_key(|(_, b)| b.y)
                .unwrap_or_else(|| {
                    let b = doc.get_bounds(first_idx).unwrap();
                    (first_idx, b)
                });

            // Find position of topmost item in the sorted list
            let start_pos = match tb_pos.get(&topmost_idx) {
                Some(&pos) => pos,
                None => continue,
            };

            // Walk backwards from the item before `start_pos`
            let mut prepend = Vec::new();
            let mut current_top = topmost_bounds.y;
            for pos in (0..start_pos).rev() {
                let (cand_idx, ref cand_bounds) = root_tb_sorted[pos];

                if claimed.contains(&cand_idx) {
                    break;
                }

                // Must still be a root TextBlock (not claimed by another module)
                if !current_roots.contains(&cand_idx) {
                    break;
                }
                if !matches!(doc.groups[cand_idx].kind, GroupKind::TextBlock) {
                    break;
                }

                // Same x within tolerance
                if (cand_bounds.x - topmost_bounds.x).abs() > x_tol {
                    break;
                }

                // Vertically adjacent: candidate bottom ≈ current top
                let cand_bottom = cand_bounds.y + cand_bounds.height;
                let gap = current_top - cand_bottom;
                let gap_limit = max_item_height / Decimal::TWO;
                if gap < Decimal::ZERO || gap > gap_limit {
                    break;
                }

                // Guard: skip multi-line paragraphs (much taller than list items)
                if cand_bounds.height > height_limit {
                    break;
                }

                // Guard: skip TextBlocks with very different width (e.g. narrow
                // standalone markers that formed their own list group).
                let width_narrow = cand_bounds.width.min(ref_item_width);
                let width_wide = cand_bounds.width.max(ref_item_width);
                if width_wide > Decimal::ZERO && width_narrow * Decimal::TWO < width_wide {
                    break;
                }

                // Check no intervening non-TextBlock root or non-marker
                // TextBlock between candidate and current top
                let has_intervening = non_tb_root_ys
                    .iter()
                    .any(|&y| y > cand_bottom && y < current_top)
                    || non_marker_tb_bounds.iter().any(|sep| {
                        let sep_bottom = sep.y + sep.height;
                        let y_ok = sep_bottom > cand_bottom && sep.y < current_top;
                        let x_ok = sep.x < (cand_bounds.x + cand_bounds.width)
                            && (sep.x + sep.width) > cand_bounds.x;
                        let gap = current_top - sep_bottom;
                        let tol = Decimal::from_f64(Y_SAME_LINE_TOLERANCE).unwrap_or(Decimal::TWO);
                        let bridges = gap < tol;
                        y_ok && x_ok && bridges
                    });
                if has_intervening {
                    break;
                }

                // Guard: stop at heading-like text (ends with ':')
                // Such text blocks are introductions to a list, not list items.
                let cand_text = doc.get_text_content(cand_idx);
                let cand_trimmed = cand_text.trim();
                if cand_trimmed.ends_with(':') || cand_trimmed.ends_with("：") {
                    break;
                }

                prepend.push(cand_idx);
                claimed.insert(cand_idx);
                current_top = cand_bounds.y;
            }

            // Apply backward extension when either:
            // 1. At least 5 items are prepended (clear pattern of missing markers), OR
            // 2. Exactly 1 item is prepended AND it meets stricter criteria:
            //    - The list style is unordered (Dash/Disc) - numbered lists should
            //      have explicit numbering, so single-item prepending risks absorbing
            //      a heading like "2. Title" that precedes "2. First list item..."
            //    - It's immediately adjacent to the first marked item
            //    - It doesn't end with ':' (which would indicate a heading)
            //    - It's a single line (not a paragraph)
            //
            // The threshold of 5 guards against false positives where a
            // heading, introductory sentence, or nearby paragraph text
            // sitting directly above a list would be incorrectly absorbed.
            // The stricter criteria for single-item prepending allows
            // absorbing the first unmarked item in lists where only the
            // first item lacks a marker.
            let group_style = group_styles.get(group_idx);
            let is_unordered_style = matches!(
                group_style,
                Some(ListStyleType::Dash) | Some(ListStyleType::Disc)
            );

            // Helper to check if a candidate meets single-item criteria
            let check_single_item = |cand_idx: usize| -> bool {
                let cand_bounds = match doc.get_bounds(cand_idx) {
                    Some(b) => b,
                    None => return false,
                };
                let cand_text = doc.get_text_content(cand_idx);
                let cand_trimmed = cand_text.trim();

                // Must not end with ':' (would be a heading)
                let not_heading_like =
                    !cand_trimmed.ends_with(':') && !cand_trimmed.ends_with("：");

                // Must be single line (height similar to list items)
                let is_single_line = cand_bounds.height <= max_item_height * Decimal::new(12, 1);

                // Must be immediately adjacent (gap <= 0.3 * line height)
                let first_marked_y = topmost_bounds.y;
                let cand_bottom = cand_bounds.y + cand_bounds.height;
                let gap = first_marked_y - cand_bottom;
                let is_adjacent =
                    gap >= Decimal::ZERO && gap <= max_item_height * Decimal::new(3, 1);

                not_heading_like && is_single_line && is_adjacent
            };

            let should_extend = if prepend.len() >= 5 && group.len() >= 2 {
                // Large backward extension requires at least 2 confirmed items.
                // A single marker item (e.g. "4. Heading text") is not strong
                // enough to absorb 5+ preceding paragraphs.
                true
            } else if !prepend.is_empty() && is_unordered_style {
                // Check if the item closest to the list (first in prepend) meets criteria.
                // Note: prepend is built from closest-to-list to farthest, so first item
                // is the one immediately above the list.
                check_single_item(prepend[0])
            } else {
                false
            };

            if should_extend {
                if prepend.len() >= 5 {
                    // Keep all prepended items
                    prepend.reverse();
                    prepend.append(group);
                    *group = prepend;
                } else {
                    // Only keep the single item closest to the list (prepend[0])
                    let single_item = prepend[0];
                    // Remove other items from claimed set since we're not using them
                    for &idx in prepend.iter().skip(1) {
                        claimed.remove(&idx);
                    }
                    let mut new_group = vec![single_item];
                    new_group.append(group);
                    *group = new_group;
                }
            }
        }

        // Phase 3: List consolidation.
        //
        // Standalone marker merging (Phase 0) can create TextBlocks with
        // higher group indices than the rest of the list.  These end up as
        // separate single-item runs because Phase 1 iterates by index.
        //
        // For each list group with ≥ 2 items, look for root TextBlocks
        // that (a) carry the same marker style, (b) share the same x
        // position, and (c) have a y position between the topmost and
        // bottommost items of the list.  Items from single-item groups
        // are eligible for adoption into a larger group.
        //
        // The loop repeats until convergence because adopting an item can
        // extend the list's y range, making further items eligible.
        let final_roots: HashSet<usize> = doc.roots().iter().copied().collect();

        // Indices from single-item groups are eligible for consolidation
        // into larger groups.
        let single_item_indices: HashSet<usize> = groups
            .iter()
            .filter(|g| g.len() == 1)
            .flat_map(|g| g.iter().copied())
            .collect();

        let mut adopted: HashSet<usize> = HashSet::new();

        loop {
            let mut changed = false;

            for (gi, group) in groups.iter_mut().enumerate() {
                if group.len() < 2 {
                    continue;
                }

                let style = group_styles[gi];
                let list_x = group
                    .iter()
                    .find_map(|&idx| doc.get_bounds(idx).map(|b| b.x));
                let list_top = group
                    .iter()
                    .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.y))
                    .min();
                let list_bottom = group
                    .iter()
                    .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.y + b.height))
                    .max();

                let (Some(lx), Some(lt), Some(lb)) = (list_x, list_top, list_bottom) else {
                    continue;
                };

                for &(tb_idx, ref tb_bounds) in &root_tb_sorted {
                    // Allow items from single-item groups to be adopted.
                    let is_available =
                        !claimed.contains(&tb_idx) || single_item_indices.contains(&tb_idx);
                    if !is_available || group.contains(&tb_idx) {
                        continue;
                    }
                    if !final_roots.contains(&tb_idx) {
                        continue;
                    }
                    if !matches!(doc.groups[tb_idx].kind, GroupKind::TextBlock) {
                        continue;
                    }
                    if (tb_bounds.x - lx).abs() > x_tol {
                        continue;
                    }
                    // Must fall within the list's y extent.
                    if tb_bounds.y < lt || tb_bounds.y > lb {
                        continue;
                    }
                    let text = doc.get_text_content(tb_idx);
                    if let Some(marker) = detect_marker(&text) {
                        if marker.kind == style {
                            group.push(tb_idx);
                            claimed.insert(tb_idx);
                            adopted.insert(tb_idx);
                            changed = true;
                        }
                    }
                }

                // Sort items by y position so the list is in document order.
                group.sort_by(|&a, &b| {
                    let a_y = doc.get_bounds(a).map(|b| b.y).unwrap_or(Decimal::ZERO);
                    let b_y = doc.get_bounds(b).map(|b| b.y).unwrap_or(Decimal::ZERO);
                    a_y.cmp(&b_y)
                });
            }

            if !changed {
                break;
            }
        }

        // Remove adopted items from their original single-item groups.
        for group in groups.iter_mut() {
            if group.len() == 1 && adopted.contains(&group[0]) {
                group.clear();
            }
        }

        // For each list group, merge into a List group (skip groups with < 2 items
        // after backward extension; these are single-item runs that didn't grow
        // enough to form a proper list).
        for (group_indices, list_style) in groups.into_iter().zip(group_styles) {
            if group_indices.len() < 2 {
                continue;
            }
            let child_group_indices = group_indices;

            doc.merge_inferred(
                child_group_indices,
                GroupKind::List { list_style },
                self.name(),
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
    fn test_detect_parenthesized_roman() {
        // Parenthesized roman numerals like (i), (ii), (iii)
        let m = detect_marker("(i) First item").unwrap();
        assert_eq!(m.kind, ListStyleType::LowerRoman);
        assert_eq!(m.prefix_len, 4); // "(i) " = 4 bytes

        let m = detect_marker("(ii) Second item").unwrap();
        assert_eq!(m.kind, ListStyleType::LowerRoman);
        assert_eq!(m.prefix_len, 5); // "(ii) " = 5 bytes

        let m = detect_marker("(iii) Third item").unwrap();
        assert_eq!(m.kind, ListStyleType::LowerRoman);
        assert_eq!(m.prefix_len, 6); // "(iii) " = 6 bytes
    }

    #[test]
    fn test_detect_parenthesized_numeric() {
        // Parenthesized numbers like (1), (2), (3)
        let m = detect_marker("(1) First item").unwrap();
        assert_eq!(m.kind, ListStyleType::Decimal);
        assert_eq!(m.prefix_len, 4); // "(1) " = 4 bytes
    }

    #[test]
    fn test_detect_parenthesized_alpha() {
        // Parenthesized letters like (a), (b), (c)
        let m = detect_marker("(a) First item").unwrap();
        assert_eq!(m.kind, ListStyleType::LowerAlpha);
        assert_eq!(m.prefix_len, 4); // "(a) " = 4 bytes

        let m = detect_marker("(A) First item").unwrap();
        assert_eq!(m.kind, ListStyleType::UpperAlpha);
        assert_eq!(m.prefix_len, 4); // "(A) " = 4 bytes
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
