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
                .is_some_and(|c| c.is_ascii_uppercase());
            if is_upper {
                ListStyleType::UpperRoman
            } else {
                ListStyleType::LowerRoman
            }
        } else if content.len() == 1
            && content
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
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

    // Collect root TextBlocks with their bounds.
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

        // Find the closest non-marker TextBlock on the same line.
        // Prefer: (1) wider content blocks (they likely contain the marker spatially),
        // (2) blocks to the right of the marker.
        let mut best: Option<(usize, i32, Decimal, Decimal)> = None; // (j, priority, x_dist, -width)

        for j in 0..root_tbs.len() {
            if j == i {
                continue;
            }
            let (content_idx, ref content_bounds) = root_tbs[j];
            if used.contains(&content_idx) {
                continue;
            }

            // Same line: y positions within tolerance, or marker bottom
            // aligns with content top (within tolerance).
            let same_y = (content_bounds.y - marker_bounds.y).abs() <= y_tol;
            let marker_bottom = marker_bounds.y + marker_bounds.height;
            let marker_above = (content_bounds.y - marker_bottom).abs() <= y_tol;
            if !same_y && !marker_above {
                continue;
            }

            // Content must not start far to the left of marker (different column).
            // Allow content that starts somewhat to the left, as multi-line text
            // blocks may have their first line next to the marker but wrap to
            // the left margin on subsequent lines.
            let max_left_offset = Decimal::from(50);
            if content_bounds.x + max_left_offset < marker_bounds.x {
                continue;
            }

            // Content must not be too far to the right of the marker either
            // (different column). The content should start near the marker's
            // right edge, not hundreds of points away.
            let marker_right = marker_bounds.x + marker_bounds.width;
            let max_right_gap = Decimal::from(50);
            if content_bounds.x > marker_right + max_right_gap {
                continue;
            }

            // Skip other standalone markers.
            let content_text = doc.get_text_content(content_idx);
            if is_standalone_marker(&content_text).is_some() {
                continue;
            }

            // When the candidate is below the marker (marker_above) rather
            // than on the exact same line, also skip candidates that start
            // with their own list marker.  Those are separate list items,
            // not the continuation content of the standalone marker above.
            if !same_y && detect_marker(&content_text).is_some() {
                continue;
            }

            // Score: prefer same_y over marker_above, then wider blocks,
            // then closest by x-distance.
            let priority = if same_y { 0i32 } else { 1i32 };
            let dist = (content_bounds.x - marker_bounds.x).abs();
            let neg_width = -content_bounds.width;
            let is_better = match best {
                None => true,
                Some((_, bp, bd, bw)) => {
                    priority < bp
                        || (priority == bp && neg_width < bw)
                        || (priority == bp && neg_width == bw && dist < bd)
                }
            };
            if is_better {
                best = Some((j, priority, dist, neg_width));
            }
        }

        if let Some((j, _, _, _)) = best {
            let (content_idx, _) = root_tbs[j];
            merges.push((marker_idx, content_idx));
            used.insert(marker_idx);
            used.insert(content_idx);
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

/// Merges standalone list-marker TextBlocks (e.g. a lone "–" draw) with
/// their adjacent content TextBlocks.  Runs early in the pipeline so that
/// downstream modules (RadioButtonDetector, CheckboxDetector, etc.) see
/// complete text blocks rather than orphaned marker fragments.
pub struct StandaloneMarkerMerger;

impl Default for StandaloneMarkerMerger {
    fn default() -> Self {
        Self::new()
    }
}

impl StandaloneMarkerMerger {
    pub fn new() -> Self {
        StandaloneMarkerMerger
    }
}

impl AnalysisModule for StandaloneMarkerMerger {
    fn name(&self) -> &'static str {
        "StandaloneMarkerMerger"
    }

    fn process(&self, doc: &mut Document) {
        merge_standalone_markers(doc, self.name());
        merge_adjacent_same_line_text_blocks(doc, self.name());
    }
}

/// Merges horizontally adjacent TextBlocks that are on the same line.
/// This handles cases where a single logical text span is split into
/// multiple XFA draw elements (e.g. "Finanzportfolioverwalter..." and
/// "PLF – KVG" on the same line with a small gap).
fn merge_adjacent_same_line_text_blocks(doc: &mut Document, module_name: &str) {
    /// Check if any node in a group has a visible horizontal border (top or bottom).
    fn has_horizontal_border(doc: &Document, group_idx: usize) -> bool {
        for node in doc.collect_nodes(group_idx) {
            if let Some(border) = &node.style.border {
                let has_top = border.get_edge(0).is_some_and(|e| {
                    e.presence != "hidden" && e.thickness.is_some_and(|t| t > Decimal::ZERO)
                });
                let has_bottom = border.get_edge(2).is_some_and(|e| {
                    e.presence != "hidden" && e.thickness.is_some_and(|t| t > Decimal::ZERO)
                });
                if has_top || has_bottom {
                    return true;
                }
            }
        }
        false
    }

    /// Check if two groups have overlapping container bounds (using node.bounds(),
    /// not text_bounds). This prevents merging elements that are in different
    /// columns — their containers won't overlap horizontally even if their
    /// text content has a small gap.
    fn containers_overlap_horizontally(doc: &Document, idx_a: usize, idx_b: usize) -> bool {
        let nodes_a: Vec<_> = doc.collect_nodes(idx_a);
        let nodes_b: Vec<_> = doc.collect_nodes(idx_b);
        if nodes_a.is_empty() || nodes_b.is_empty() {
            return false;
        }
        // Use the container bounds of the first node as representative
        let a_bounds = nodes_a[0].bounds();
        let b_bounds = nodes_b[0].bounds();
        let a_right = a_bounds.x + a_bounds.width;
        let b_left = b_bounds.x;
        let b_right = b_bounds.x + b_bounds.width;
        let a_left = a_bounds.x;
        // Check if containers overlap or are adjacent (within 5pt)
        let tolerance = Decimal::from(5);
        let overlap_left = a_left.max(b_left);
        let overlap_right = a_right.min(b_right);
        overlap_right + tolerance >= overlap_left
    }

    let roots = doc.roots();
    let y_tol = Decimal::from_f64(Y_SAME_LINE_TOLERANCE).unwrap_or(Decimal::TWO);
    // Maximum horizontal gap (in points) to consider two blocks as adjacent.
    let max_gap = Decimal::from(20);

    // Collect root TextBlocks with bounds.
    let mut tbs: Vec<(usize, Bounds)> = Vec::new();
    for &idx in &roots {
        if !matches!(doc.groups[idx].kind, GroupKind::TextBlock) {
            continue;
        }
        if let Some(b) = doc.get_bounds(idx) {
            tbs.push((idx, b));
        }
    }

    // Sort by y then x.
    tbs.sort_by(|a, b| a.1.y.cmp(&b.1.y).then(a.1.x.cmp(&b.1.x)));

    let mut used: HashSet<usize> = HashSet::new();
    let mut merges: Vec<(usize, usize)> = Vec::new();

    for i in 0..tbs.len() {
        let (left_idx, ref left_bounds) = tbs[i];
        if used.contains(&left_idx) {
            continue;
        }

        for j in (i + 1)..tbs.len() {
            let (right_idx, ref right_bounds) = tbs[j];
            if used.contains(&right_idx) {
                continue;
            }

            // Must be on the same line.
            if (right_bounds.y - left_bounds.y).abs() > y_tol {
                // Since sorted by y, if we're past the tolerance, stop.
                if right_bounds.y - left_bounds.y > y_tol {
                    break;
                }
                continue;
            }

            // Right block must start after left block ends, with a small gap.
            let gap = right_bounds.x - (left_bounds.x + left_bounds.width);
            if gap < Decimal::ZERO || gap > max_gap {
                continue;
            }

            // Don't merge bordered table cells — both blocks having horizontal
            // borders indicates they are separate table cells, not fragments of
            // the same text span.
            if has_horizontal_border(doc, left_idx) && has_horizontal_border(doc, right_idx) {
                continue;
            }

            // Don't merge blocks with very different heights.
            // Fragments of the same text span have similar heights (both are
            // single-line or both are multi-line). A tall multi-line paragraph
            // next to a short single-line element indicates they are separate
            // content (e.g., different columns at the same y-position).
            let short_h = left_bounds.height.min(right_bounds.height);
            let tall_h = left_bounds.height.max(right_bounds.height);
            if tall_h > Decimal::ZERO && short_h * Decimal::from(3) < tall_h {
                continue;
            }

            // Don't merge blocks whose containers are in different columns.
            // Text fragments of the same line have adjacent/overlapping containers,
            // while elements in different columns have clearly separated containers.
            if !containers_overlap_horizontally(doc, left_idx, right_idx) {
                continue;
            }

            merges.push((left_idx, right_idx));
            used.insert(left_idx);
            used.insert(right_idx);
            break; // Only merge once per left block.
        }
    }

    for (left_idx, right_idx) in merges {
        doc.merge_inferred(vec![left_idx, right_idx], GroupKind::TextBlock, module_name);
    }
}

impl AnalysisModule for ListDetector {
    fn name(&self) -> &'static str {
        "ListDetector"
    }

    fn process(&self, doc: &mut Document) {
        // Phase 0 (standalone marker merge) now runs earlier in the pipeline
        // via StandaloneMarkerMerger. Re-run here as a no-op safety net.
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

        // Collect y-positions of whitespace-only Leaf roots separately.
        // These are typically empty placeholder draw elements or page-layout
        // groups (Header/Footer).  For non-bold list candidates they are
        // ignored as intervening content because they do not represent
        // meaningful content separators between list items.
        //
        // Footer and Header groups are added unconditionally: they are
        // page-layout elements that appear at fixed vertical positions on
        // each page and should never be treated as separators within a
        // logical list that happens to span a page boundary or to be
        // positioned near the footer region.
        let ws_leaf_ys: HashSet<Decimal> = roots
            .iter()
            .filter(|&&idx| match &doc.groups[idx].kind {
                GroupKind::Leaf { .. } => doc.get_text_content(idx).trim().is_empty(),
                GroupKind::Footer | GroupKind::Header => true,
                _ => false,
            })
            .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.y))
            .collect();

        // Collect bounds of root TextBlocks that have non-empty text and
        // do NOT start with a list marker.  Non-marker TextBlocks are
        // paragraphs/headings that naturally separate lists.  They may
        // appear earlier in index order than the merged list items
        // (because StandaloneMarkerMerger creates merged groups at higher
        // indices) and therefore won't be encountered between them during
        // the sequential Phase 1 walk.

        // Collect (bounds, marker_kind) of ALL marker TextBlocks so that
        // can_extend_run can detect when a different-kind marker from
        // another column lies in the y-gap between two same-kind items.
        let all_marker_tb_info: Vec<(Bounds, ListStyleType)> = roots
            .iter()
            .filter_map(|&idx| {
                if !matches!(doc.groups[idx].kind, GroupKind::TextBlock) {
                    return None;
                }
                let text = doc.get_text_content(idx);
                let marker = detect_marker(&text)?;
                let bounds = doc.get_bounds(idx)?;
                Some((bounds, marker.kind))
            })
            .collect();

        let non_marker_tb_bounds: Vec<Bounds> = roots
            .iter()
            .filter(|&&idx| {
                if !matches!(doc.groups[idx].kind, GroupKind::TextBlock) {
                    return false;
                }
                let text = doc.get_text_content(idx);
                let trimmed = text.trim();
                if trimmed.is_empty() || detect_marker(&text).is_some() {
                    return false;
                }
                // Bold TextBlocks (headings) are always reliable separators.
                if doc.is_bold_group(idx) {
                    return true;
                }
                // Non-bold TextBlocks are separators UNLESS they are
                // continuations of a marker TextBlock (i.e., immediately
                // below a marker TB at the same or indented x position).
                // Multi-line list items are often split into the marker line
                // plus one or more continuation TextBlocks that may be
                // indented relative to the marker.
                let bounds = match doc.get_bounds(idx) {
                    Some(b) => b,
                    None => return false,
                };
                let continuation_gap = Decimal::from(5);
                let is_continuation = all_marker_tb_info.iter().any(|(mb, _)| {
                    let mb_bottom = mb.y + mb.height;
                    let gap = bounds.y - mb_bottom;
                    if gap < Decimal::ZERO || gap > continuation_gap {
                        return false;
                    }
                    // Continuation can be at the same x or indented to the
                    // right (but not far to the left of the marker).
                    let x_offset = bounds.x - mb.x;
                    x_offset >= -x_tol && x_offset <= Decimal::from(50)
                });
                !is_continuation
            })
            .filter_map(|&idx| doc.get_bounds(idx))
            .collect();

        // Each entry: (group_idx, text, bounds, marker)
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

        // Minimum x-distance to consider items to be in different columns.
        // This must be substantially larger than X_TOLERANCE (which allows
        // for minor alignment differences within the same column).
        let column_gap = Decimal::from(100);

        // Multi-column aware Phase 1 walk.
        //
        // In two-column PDF layouts, root TextBlocks from the left and right
        // columns are interleaved by index.  A naïve sequential walk would
        // flush the left-column run every time it encounters a right-column
        // item, splitting lists that span multiple index-interleaved rows.
        //
        // To handle this, we maintain per-column runs.  When we encounter
        // a marker item whose x position is far from the current run (i.e.
        // in a different column), we park the current run and switch to (or
        // start) the run for that column.  Non-marker TextBlocks and
        // non-TextBlock roots only flush the runs for columns whose x range
        // they overlap.
        type Run = Vec<(usize, String, Bounds, DetectedMarker)>;
        let mut column_runs: Vec<(Decimal, Decimal, Run)> = Vec::new(); // (representative x, max right edge, run)

        /// Find the column run whose representative x is within `tol` of `x`.
        fn find_column(
            column_runs: &[(Decimal, Decimal, Run)],
            x: Decimal,
            tol: Decimal,
        ) -> Option<usize> {
            column_runs
                .iter()
                .position(|(col_x, _, _)| (x - *col_x).abs() <= tol)
        }

        /// Check whether a candidate item can extend a run (same marker kind,
        /// no intervening content in the same column).
        fn can_extend_run(
            run: &Run,
            idx: usize,
            bounds: &Bounds,
            marker: &DetectedMarker,
            doc: &Document,
            non_tb_root_ys: &[Decimal],
            ws_leaf_ys: &HashSet<Decimal>,
            non_marker_tb_bounds: &[Bounds],
            all_marker_tb_info: &[(Bounds, ListStyleType)],
            x_tol: Decimal,
        ) -> bool {
            let last = match run.last() {
                Some(l) => l,
                None => return true,
            };

            if marker.kind != last.3.kind {
                return false;
            }

            let last_bottom = last.2.y + last.2.height;
            let curr_top = bounds.y;
            let (range_lo, range_hi) = if last_bottom <= curr_top {
                (last_bottom, curr_top)
            } else {
                (curr_top, last_bottom)
            };

            let both_non_bold = !doc.is_bold_group(last.0) && !doc.is_bold_group(idx);
            let has_intervening = non_tb_root_ys.iter().any(|&y| {
                if y > range_lo && y < range_hi {
                    if both_non_bold && ws_leaf_ys.contains(&y) {
                        return false;
                    }
                    true
                } else {
                    false
                }
            }) || non_marker_tb_bounds.iter().any(|sep| {
                let sep_bottom = sep.y + sep.height;
                let y_overlap = sep_bottom > range_lo && sep.y < range_hi;
                let item_x_lo = last.2.x.min(bounds.x);
                let item_x_hi = (last.2.x + last.2.width).max(bounds.x + bounds.width);
                let x_overlap = sep.x < item_x_hi && (sep.x + sep.width) > item_x_lo;
                let not_continuation = sep.x <= item_x_lo + x_tol;
                y_overlap && x_overlap && not_continuation
            });

            if has_intervening {
                return false;
            }

            // In multi-column layouts, a marker TextBlock of a DIFFERENT kind
            // from another column may lie in the y-gap.  This typically means a
            // section boundary (e.g. a numbered heading like "2. Konto-..."
            // between dash items).  Treat it as intervening.
            let has_cross_column_different_marker = all_marker_tb_info.iter().any(|(mb, mk)| {
                if *mk == marker.kind {
                    return false; // same kind — not a boundary
                }
                let mb_bottom = mb.y + mb.height;
                let y_overlap = mb_bottom > range_lo && mb.y < range_hi;
                if !y_overlap {
                    return false;
                }
                // Only count markers that are NOT in the same column as our
                // run (items in the same column with different kinds are
                // already handled by the marker.kind != last.kind check).
                let item_x = last.2.x;
                (mb.x - item_x).abs() > x_tol
            });

            !has_cross_column_different_marker
        }

        for &idx in &roots {
            if !matches!(doc.groups[idx].kind, GroupKind::TextBlock) {
                // Non-TextBlock root: flush runs for columns whose x range
                // overlaps with this element.  Runs in other columns are
                // unaffected.
                if let Some(non_tb_bounds) = doc.get_bounds(idx) {
                    let ntb_left = non_tb_bounds.x;
                    let ntb_right = non_tb_bounds.x + non_tb_bounds.width;
                    for (col_x, col_right_edge, run) in &mut column_runs {
                        let col_left = *col_x - x_tol;
                        let col_right = *col_right_edge + x_tol;
                        if ntb_right > col_left && ntb_left < col_right {
                            flush(run, &mut groups, &mut group_styles);
                        }
                    }
                } else {
                    // No bounds: flush all runs
                    for (_, _, run) in &mut column_runs {
                        flush(run, &mut groups, &mut group_styles);
                    }
                }
                continue;
            }

            let text = doc.get_text_content(idx);
            let bounds = match doc.get_bounds(idx) {
                Some(b) => b,
                None => {
                    for (_, _, run) in &mut column_runs {
                        flush(run, &mut groups, &mut group_styles);
                    }
                    continue;
                }
            };

            if let Some(marker) = detect_marker(&text) {
                // Find which column this item belongs to.
                let col_idx = find_column(&column_runs, bounds.x, column_gap);

                if let Some(ci) = col_idx {
                    let (_, ref mut col_right_edge, ref mut run) = column_runs[ci];
                    if can_extend_run(
                        run,
                        idx,
                        &bounds,
                        &marker,
                        doc,
                        &non_tb_root_ys,
                        &ws_leaf_ys,
                        &non_marker_tb_bounds,
                        &all_marker_tb_info,
                        x_tol,
                    ) {
                        *col_right_edge = (*col_right_edge).max(bounds.x + bounds.width);
                        run.push((idx, text, bounds, marker));
                    } else {
                        flush(run, &mut groups, &mut group_styles);
                        *col_right_edge = bounds.x + bounds.width;
                        run.push((idx, text, bounds, marker));
                    }
                } else {
                    // New column — start a fresh run.
                    let right_edge = bounds.x + bounds.width;
                    let mut run = Vec::new();
                    run.push((idx, text, bounds, marker));
                    column_runs.push((bounds.x, right_edge, run));
                }
            } else {
                // TextBlock without a marker: flush runs in the same column.
                // If the TB doesn't match any known column, flush ALL runs
                // to preserve the original single-column break behavior.
                let col_idx = find_column(&column_runs, bounds.x, column_gap);
                if let Some(ci) = col_idx {
                    flush(&mut column_runs[ci].2, &mut groups, &mut group_styles);
                } else if !column_runs.is_empty() {
                    for (_, _, run) in &mut column_runs {
                        flush(run, &mut groups, &mut group_styles);
                    }
                }
            }
        }

        // Flush all remaining column runs.
        for (_, _, mut run) in column_runs {
            flush(&mut run, &mut groups, &mut group_styles);
        }

        // Phase 1b: Sublist merging.
        //
        // When a run of one marker type (e.g. Dash) is interrupted by a run
        // of a different type (e.g. LowerRoman) and then the original type
        // resumes, the interrupting run is a sublist of the last item in the
        // first run.  We merge the two same-type runs and record the
        // interrupting run as a sublist.
        //
        // Each SublistEntry records where (after which item) to insert a
        // sublist, plus the sublist's own indices, style, and *children*
        // (sub-sublists that were attached by an earlier inner merge).
        #[derive(Clone, Debug)]
        struct SublistEntry {
            after_item: usize,
            indices: Vec<usize>,
            style: ListStyleType,
            children: Vec<SublistEntry>,
        }

        let mut sublists: Vec<Vec<SublistEntry>> = vec![Vec::new(); groups.len()];
        let mut consumed: Vec<bool> = vec![false; groups.len()];

        // Scan for A-B-A patterns among unconsumed groups.
        //
        // After an inner merge (e.g. Alpha-Roman-Alpha), the consumed groups
        // leave gaps in the index sequence.  The outer pattern
        // (e.g. Dash-Alpha-Dash) then has non-consecutive raw indices and
        // would be missed by a simple i/i+1/i+2 scan.  We therefore iterate
        // over *unconsumed* indices and repeat until no more merges occur.
        loop {
            let active: Vec<usize> = (0..groups.len()).filter(|&j| !consumed[j]).collect();
            let mut merged_any = false;
            let mut k = 0;
            while k + 2 < active.len() {
                let a1 = active[k];
                let b = active[k + 1];
                let a2 = active[k + 2];
                if group_styles[a1] == group_styles[a2] && group_styles[a1] != group_styles[b] {
                    // A-B-A pattern found.  Only merge when Group A already
                    // has ≥ 2 items so that B is genuinely a sublist.
                    if groups[a1].len() < 2 {
                        k += 1;
                        continue;
                    }

                    // Check for intervening non-list content between Group B's
                    // last item and Group A2's first item.  If a paragraph or
                    // heading sits between the sublist and the supposed
                    // continuation, these are separate lists and should not be
                    // merged.
                    let a1_last_bottom = groups[a1]
                        .last()
                        .and_then(|&idx| doc.get_bounds(idx))
                        .map(|b| b.y + b.height);
                    let b_first_top = groups[b]
                        .first()
                        .and_then(|&idx| doc.get_bounds(idx))
                        .map(|b| b.y);
                    let b_last_bottom = groups[b]
                        .last()
                        .and_then(|&idx| doc.get_bounds(idx))
                        .map(|b| b.y + b.height);
                    let a2_first_top = groups[a2]
                        .first()
                        .and_then(|&idx| doc.get_bounds(idx))
                        .map(|b| b.y);

                    // Helper: check if meaningful content exists in a y-range.
                    let has_separator_in_range = |lo: Decimal, hi: Decimal| -> bool {
                        non_marker_tb_bounds.iter().any(|sep| {
                            let sep_bottom = sep.y + sep.height;
                            sep_bottom > lo && sep.y < hi
                        }) || non_tb_root_ys
                            .iter()
                            .any(|&y| y > lo && y < hi && !ws_leaf_ys.contains(&y))
                    };

                    let has_intervening_between_b_and_a2 =
                        if let (Some(b_bot), Some(a2_top)) = (b_last_bottom, a2_first_top) {
                            let (lo, hi) = if b_bot <= a2_top {
                                (b_bot, a2_top)
                            } else {
                                (a2_top, b_bot)
                            };
                            has_separator_in_range(lo, hi)
                        } else {
                            false
                        };

                    // Also check between A1's last item and B's first item.
                    // A heading or paragraph between A1 and B means B is not
                    // really a sublist of A1.
                    let has_intervening_between_a1_and_b =
                        if let (Some(a1_bot), Some(b_top)) = (a1_last_bottom, b_first_top) {
                            let (lo, hi) = if a1_bot <= b_top {
                                (a1_bot, b_top)
                            } else {
                                (b_top, a1_bot)
                            };
                            has_separator_in_range(lo, hi)
                        } else {
                            false
                        };

                    // B should be plausibly a sublist of A1's last item:
                    // either A1's last item introduces B (ends with ':') or
                    // B is indented to the right of A1.
                    let a1_last_text = groups[a1]
                        .last()
                        .map(|&idx| doc.get_text_content(idx))
                        .unwrap_or_default();
                    let a1_last_trimmed = a1_last_text.trim();
                    let a1_intro =
                        a1_last_trimmed.ends_with(':') || a1_last_trimmed.ends_with("：");

                    let a1_x = groups[a1]
                        .iter()
                        .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.x))
                        .min();
                    let b_x = groups[b]
                        .iter()
                        .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.x))
                        .min();
                    let b_indented = match (a1_x, b_x) {
                        (Some(ax), Some(bx)) => bx > ax + x_tol,
                        _ => false,
                    };

                    // If there is meaningful content between A1's last item
                    // and B's first item, B cannot be a sublist of A1.
                    if has_intervening_between_a1_and_b {
                        k += 1;
                        continue;
                    }

                    // Even with an intro signal, a bold heading or
                    // non-TextBlock element between B and A2 is a definitive
                    // section break that prevents merging.
                    let has_definitive_separator =
                        if let (Some(b_bot), Some(a2_top)) = (b_last_bottom, a2_first_top) {
                            let (lo, hi) = if b_bot <= a2_top {
                                (b_bot, a2_top)
                            } else {
                                (a2_top, b_bot)
                            };
                            roots.iter().any(|&idx| {
                                if !matches!(doc.groups[idx].kind, GroupKind::TextBlock) {
                                    return false;
                                }
                                if !doc.is_bold_group(idx) {
                                    return false;
                                }
                                let text = doc.get_text_content(idx);
                                if text.trim().is_empty() || detect_marker(&text).is_some() {
                                    return false;
                                }
                                if let Some(sep) = doc.get_bounds(idx) {
                                    let sep_bottom = sep.y + sep.height;
                                    sep_bottom > lo && sep.y < hi
                                } else {
                                    false
                                }
                            }) || non_tb_root_ys.iter().any(|&y| {
                                y > lo && y < hi && !ws_leaf_ys.contains(&y)
                            })
                        } else {
                            false
                        };

                    if has_definitive_separator {
                        k += 1;
                        continue;
                    }

                    // When there is intervening non-list content between B
                    // and A2, only merge if A1 introduces B (colon) or B is
                    // indented — otherwise these are separate lists.
                    if has_intervening_between_b_and_a2 && !a1_intro && !b_indented {
                        k += 1;
                        continue;
                    }

                    let after_count = groups[a1].len();
                    // Take B's sublists first to avoid borrow conflicts.
                    let b_children = std::mem::take(&mut sublists[b]);
                    sublists[a1].push(SublistEntry {
                        after_item: after_count,
                        indices: groups[b].clone(),
                        style: group_styles[b],
                        children: b_children,
                    });
                    let tail: Vec<usize> = groups[a2].clone();
                    groups[a1].extend(tail);
                    // Transfer any sublists that were already recorded on a2
                    // (from an earlier inner merge) so they stay attached to
                    // the correct item offsets after merging.
                    for mut entry in std::mem::take(&mut sublists[a2]) {
                        entry.after_item += after_count;
                        sublists[a1].push(entry);
                    }
                    consumed[b] = true;
                    consumed[a2] = true;
                    merged_any = true;
                    // Don't advance k — re-check from same position for
                    // chained patterns.
                    break;
                }
                k += 1;
            }
            if !merged_any {
                break;
            }
        }

        // Phase 1c: Trailing A-B sublist detection.
        //
        // After A-B-A merging, some groups remain as A-B without a trailing A
        // (e.g. Dash → Roman at end of document).  We merge B as a sublist of
        // A's last item when EITHER:
        //   (a) B is visually indented to the right of A (different x-position), OR
        //   (b) A's last item text ends with ':' — a strong intro signal that
        //       the following items of a different style belong to it.
        //
        // We also handle chains: A-B-B-B where multiple consecutive B groups
        // are all sublists of A's last item (e.g. three separate roman groups
        // that are all sublists of the same parent dash list).
        {
            let active: Vec<usize> = (0..groups.len()).filter(|&j| !consumed[j]).collect();

            // Compute the median x-position for a group's items.
            let group_x = |gi: usize| -> Option<Decimal> {
                let mut xs: Vec<Decimal> = groups[gi]
                    .iter()
                    .filter_map(|&idx| doc.get_bounds(idx).map(|b| b.x))
                    .collect();
                if xs.is_empty() {
                    return None;
                }
                xs.sort();
                Some(xs[xs.len() / 2])
            };

            // Check whether the last item of a group ends with ':' (sublist intro).
            let last_item_is_intro = |gi: usize| -> bool {
                if let Some(&last_idx) = groups[gi].last() {
                    let text = doc.get_text_content(last_idx);
                    let trimmed = text.trim();
                    trimmed.ends_with(':') || trimmed.ends_with("：")
                } else {
                    false
                }
            };

            let mut k = 0;
            while k + 1 < active.len() {
                let a = active[k];

                // A must have ≥ 2 items to be a genuine parent list.
                if groups[a].len() < 2 {
                    k += 1;
                    continue;
                }

                let a_x = group_x(a);
                let intro = last_item_is_intro(a);

                // Consume consecutive groups that qualify as sublists of A.
                let mut j = k + 1;
                let mut merging = false; // set once first B is confirmed
                while j < active.len() {
                    let b = active[j];
                    if group_styles[a] == group_styles[b] {
                        break; // same style → not a sublist, stop
                    }

                    // Check signal (a): indentation
                    let indented = match (a_x, group_x(b)) {
                        (Some(ax), Some(bx)) => bx > ax + x_tol,
                        _ => false,
                    };

                    // Check signal (b): A's last item is an intro ending with ':'
                    let is_intro_sublist = intro && j == k + 1;

                    // Once we've started merging (first B confirmed), subsequent
                    // groups of the same style as B continue the sublist chain.
                    let continues_chain = merging
                        && j > k + 1
                        && sublists[a].last().map(|s| s.style) == Some(group_styles[b]);

                    if !indented && !is_intro_sublist && !continues_chain {
                        break;
                    }
                    merging = true;

                    // If B has the same style as the last sublist entry we
                    // just added, extend that entry (combine split runs into
                    // one sublist) instead of creating a separate SublistEntry
                    // that would overwrite the previous one in the converter.
                    let b_children = std::mem::take(&mut sublists[b]);
                    if let Some(last_entry) = sublists[a].last_mut() {
                        if last_entry.style == group_styles[b] {
                            last_entry.indices.extend(groups[b].iter().copied());
                            last_entry.children.extend(b_children);
                            consumed[b] = true;
                            j += 1;
                            continue;
                        }
                    }

                    let after_count = groups[a].len();
                    sublists[a].push(SublistEntry {
                        after_item: after_count,
                        indices: groups[b].clone(),
                        style: group_styles[b],
                        children: b_children,
                    });
                    consumed[b] = true;
                    j += 1;
                }
                k = if j > k + 1 { j } else { k + 1 };
            }
        }

        // Remove consumed groups and compact
        let mut compacted_groups: Vec<Vec<usize>> = Vec::new();
        let mut compacted_styles: Vec<ListStyleType> = Vec::new();
        let mut compacted_sublists: Vec<Vec<SublistEntry>> = Vec::new();
        for (idx, group) in groups.into_iter().enumerate() {
            if !consumed[idx] {
                compacted_groups.push(group);
                compacted_styles.push(group_styles[idx]);
                compacted_sublists.push(sublists[idx].clone());
            }
        }
        let mut groups = compacted_groups;
        let group_styles = compacted_styles;
        let group_sublists = compacted_sublists;

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

            // Compute maximum text length of marked items in the group.
            // Used to guard against absorbing introductory paragraphs that
            // are longer than all actual list items.
            let max_item_text_len: usize = group
                .iter()
                .map(|&idx| doc.get_text_content(idx).trim().len())
                .max()
                .unwrap_or(0);

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

                // Must not be longer than all marked items — a candidate that
                // is longer than every item in the group is likely an
                // introductory paragraph (e.g. "Dies umfasst ... insbesondere")
                // rather than a list item that lost its marker.
                let cand_len = cand_trimmed.len();
                let not_too_long = cand_len <= max_item_text_len || cand_len <= 40;

                // Must not have a non-marker TextBlock (paragraph) immediately
                // above it — if there is one, the candidate is part of a
                // paragraph block rather than a lost first list item.
                let no_paragraph_above = !all_marker_tb_info.is_empty()
                    && !root_tb_sorted.iter().any(|(tb_idx, tb_b)| {
                        // Skip the candidate itself
                        if *tb_idx == cand_idx {
                            return false;
                        }
                        // Must be a non-marker TextBlock
                        if !current_roots.contains(tb_idx) {
                            return false;
                        }
                        let tb_text = doc.get_text_content(*tb_idx);
                        if tb_text.trim().is_empty() || detect_marker(&tb_text).is_some() {
                            return false;
                        }
                        // Must be above the candidate and at the same x
                        let sep_bottom = tb_b.y + tb_b.height;
                        let gap_above = cand_bounds.y - sep_bottom;
                        gap_above >= Decimal::ZERO
                            && gap_above < max_item_height * Decimal::TWO
                            && (tb_b.x - cand_bounds.x).abs() <= x_tol
                    });

                not_heading_like
                    && is_single_line
                    && is_adjacent
                    && not_too_long
                    && no_paragraph_above
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
        for (i, (group_indices, list_style)) in groups.into_iter().zip(group_styles).enumerate() {
            if group_indices.len() < 2 {
                continue;
            }

            // Check if this group has sublists that need to be interleaved.
            let subs = if i < group_sublists.len() {
                &group_sublists[i]
            } else {
                &[][..]
            };

            if subs.is_empty() {
                // Simple case: no sublists
                doc.merge_inferred(group_indices, GroupKind::List { list_style }, self.name());
            } else {
                // Recursive helper: build the child_indices for a list,
                // interleaving item indices and sublist groups (which may
                // themselves have sub-sublists).
                fn build_list_children(
                    doc: &mut Document,
                    group_indices: &[usize],
                    subs: &[SublistEntry],
                    module_name: &str,
                ) -> Vec<usize> {
                    let mut child_indices: Vec<usize> = Vec::new();
                    let mut item_count = 0usize;

                    for &idx in group_indices {
                        child_indices.push(idx);
                        item_count += 1;

                        for entry in subs {
                            if entry.after_item == item_count {
                                // Recursively build children for this sublist
                                let sub_children = if entry.children.is_empty() {
                                    entry.indices.clone()
                                } else {
                                    build_list_children(
                                        doc,
                                        &entry.indices,
                                        &entry.children,
                                        module_name,
                                    )
                                };
                                let sub_group_idx = doc.merge_inferred(
                                    sub_children,
                                    GroupKind::List {
                                        list_style: entry.style,
                                    },
                                    module_name,
                                );
                                child_indices.push(sub_group_idx);
                            }
                        }
                    }
                    child_indices
                }

                let child_indices = build_list_children(doc, &group_indices, subs, self.name());
                doc.merge_inferred(child_indices, GroupKind::List { list_style }, self.name());
            }
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
            Page::new(num(595.0), num(842.0)),
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
            Page::new(num(595.0), num(842.0)),
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
            Page::new(num(595.0), num(842.0)),
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
            Page::new(num(595.0), num(842.0)),
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
            Page::new(num(595.0), num(842.0)),
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
            Page::new(num(595.0), num(842.0)),
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
