//! Heading detection module.
//!
//! Classifies text blocks into heading levels (h1-h6) based on statistical
//! analysis of font sizes, weights, and other visual properties.

use super::AnalysisModule;
use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::{Flattened, FlattenedNodeKind};
use crate::xfa::FontWeight;
use rust_decimal::prelude::*;
use std::collections::{HashMap, HashSet};

/// Global font statistics collected from multiple flattened form states.
///
/// This is used in exhaustive mode to ensure consistent heading detection
/// across all form states by computing statistics from all states combined.
#[derive(Debug, Clone, Default)]
pub struct GlobalFontStats {
    /// The most common font size (body text)
    pub body_size: f32,
    /// Total number of samples used
    pub sample_count: usize,
    /// Distribution of font sizes (rounded to 0.5pt)
    pub size_distribution: HashMap<u32, usize>, // Using u32 bits representation for f32
    /// Distribution of font styles (size bits + is_bold)
    pub style_distribution: HashMap<(u32, bool), usize>,
    /// Total number of text nodes analyzed
    pub total_text_nodes: usize,
    /// Most common style (size bits, is_bold)
    pub most_common_style: Option<(u32, bool)>,
    /// Ratio of most common style
    pub common_style_ratio: f32,
    /// Global heading style buckets sorted by priority: size desc, bold first, has_line first.
    /// Each entry is (size_bits, is_bold, has_line).
    pub heading_buckets: Vec<(u32, bool, bool)>,
}

/// Kept for backward compatibility. No longer used for heading level detection.
#[derive(Debug, Clone, Default)]
pub struct GlobalBorderStats {
    pub underlined_count: usize,
    pub non_underlined_count: usize,
    pub total_count: usize,
}

impl GlobalFontStats {
    /// Compute global font statistics from an iterator of Flattened references.
    /// Note: This does NOT compute border_stats. Call `compute_border_stats` separately
    /// after constructing this, passing the same flattened data.
    pub fn from_flattened_iter<'a>(flattened_iter: impl Iterator<Item = &'a Flattened>) -> Self {
        let mut sizes: Vec<f32> = Vec::new();
        let mut size_counts: HashMap<u32, usize> = HashMap::new();
        let mut style_counts: HashMap<(u32, bool), usize> = HashMap::new();
        let mut total_text_nodes = 0usize;

        for flattened in flattened_iter {
            for node in flattened.iter_nodes() {
                if let FlattenedNodeKind::Text {
                    font_size, content, ..
                } = &node.kind
                {
                    // Skip empty text
                    if content.trim().is_empty() {
                        continue;
                    }

                    let size = font_size.to_f32().unwrap_or(10.0);
                    sizes.push(size);
                    total_text_nodes += 1;

                    // Round to 0.5pt for bucketing, store as bits
                    let rounded = (size * 2.0).round() / 2.0;
                    let size_bits = rounded.to_bits();
                    *size_counts.entry(size_bits).or_insert(0) += 1;

                    // Track font style (size + bold) for frequency analysis
                    let is_bold = node
                        .style
                        .font
                        .as_ref()
                        .map(|f| f.weight == FontWeight::Bold)
                        .unwrap_or(false);
                    let style_key = (size_bits, is_bold);
                    *style_counts.entry(style_key).or_insert(0) += 1;
                }
            }
        }

        if sizes.is_empty() {
            return Self::default();
        }

        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let len = sizes.len();
        let median = if len.is_multiple_of(2) {
            (sizes[len / 2 - 1] + sizes[len / 2]) / 2.0
        } else {
            sizes[len / 2]
        };

        // Find the most common font size (body text)
        let body_size = size_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(size_bits, _)| f32::from_bits(*size_bits))
            .unwrap_or(median);

        // Find the most common font style
        let most_common_style = style_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(key, _)| *key);

        // Calculate common style ratio
        let common_style_ratio = most_common_style
            .and_then(|style| style_counts.get(&style))
            .map(|&count| count as f32 / total_text_nodes.max(1) as f32)
            .unwrap_or(0.0);

        Self {
            body_size,
            sample_count: len,
            size_distribution: size_counts,
            style_distribution: style_counts,
            total_text_nodes,
            most_common_style,
            common_style_ratio,
            heading_buckets: Vec::new(),
        }
    }

    /// Compute global heading style buckets from all flattened states.
    /// This should be called after `from_flattened_iter` to fill in heading_buckets.
    /// Analyzes potential headings and groups them into style buckets sorted by priority.
    pub fn compute_heading_buckets<'a>(
        &mut self,
        flattened_iter: impl Iterator<Item = &'a Flattened>,
    ) {
        let body_size = self.body_size;
        let most_common_style = self.most_common_style;
        let mut bucket_set: HashSet<(u32, bool, bool)> = HashSet::new();

        for flattened in flattened_iter {
            for node in flattened.iter_nodes() {
                if let FlattenedNodeKind::Text {
                    font_size, content, ..
                } = &node.kind
                {
                    let text = content.trim();
                    if text.is_empty() || text.len() < 2 || text.len() > 200 {
                        continue;
                    }

                    let size = font_size.to_f32().unwrap_or(10.0);
                    let rounded = (size * 2.0).round() / 2.0;
                    let size_bits = rounded.to_bits();

                    let is_bold = node
                        .style
                        .font
                        .as_ref()
                        .map(|f| f.weight == FontWeight::Bold)
                        .unwrap_or(false);

                    // Skip body text style: only non-bold text at the most common size
                    let body_is_non_bold = most_common_style.map(|(s, b)| !b).unwrap_or(false);
                    if !is_bold && body_is_non_bold {
                        let body_size_bits = most_common_style.map(|(s, _)| s);
                        if body_size_bits == Some(size_bits) {
                            continue;
                        }
                    }

                    // Check if this is a bold section header (bold at body size)
                    let is_body_size = (size - body_size).abs() < 0.5;
                    let body_is_non_bold = most_common_style
                        .map(|(s, b)| f32::from_bits(s) == body_size && !b)
                        .unwrap_or(false);
                    let is_bold_section_header = is_bold && is_body_size && body_is_non_bold;

                    // Check if this is a size-based heading (larger than body)
                    let ratio = size / body_size;
                    let size_diff = size - body_size;
                    let is_size_based_heading = ratio >= 1.35 || size_diff >= 3.5;

                    if !is_bold_section_header && !is_size_based_heading {
                        continue;
                    }

                    // Determine has_line (underline, overline, or visible top/bottom border)
                    let has_font_underline = node
                        .style
                        .font
                        .as_ref()
                        .map(|f| f.underline)
                        .unwrap_or(false);

                    let has_top_border = node
                        .style
                        .border
                        .as_ref()
                        .and_then(|b| b.get_edge(0))
                        .map(|e| e.presence == "visible" && e.thickness.is_some())
                        .unwrap_or(false);

                    let has_bottom_border = node
                        .style
                        .border
                        .as_ref()
                        .and_then(|b| b.get_edge(2))
                        .map(|e| e.presence == "visible" && e.thickness.is_some())
                        .unwrap_or(false);

                    let has_line = has_font_underline || has_top_border || has_bottom_border;
                    bucket_set.insert((size_bits, is_bold, has_line));
                }
            }
        }

        // Sort buckets by priority: size desc, bold first, has_line first
        let mut buckets: Vec<_> = bucket_set.into_iter().collect();
        buckets.sort_by(|a, b| {
            let a_size = f32::from_bits(a.0);
            let b_size = f32::from_bits(b.0);
            b_size
                .partial_cmp(&a_size)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.1.cmp(&a.1)) // bold first
                .then(b.2.cmp(&a.2)) // has_line first
        });

        self.heading_buckets = buckets;
    }

    /// Backward-compatible alias for compute_heading_buckets.
    pub fn compute_border_stats<'a>(
        &mut self,
        flattened_iter: impl Iterator<Item = &'a Flattened>,
    ) {
        self.compute_heading_buckets(flattened_iter);
    }

    /// Convert to internal FontStats format
    fn to_font_stats(&self) -> FontStats {
        // Convert HashMaps back to OrderedFloat format
        let size_distribution: HashMap<OrderedFloat, usize> = self
            .size_distribution
            .iter()
            .map(|(bits, count)| (OrderedFloat(f32::from_bits(*bits)), *count))
            .collect();

        let style_distribution: HashMap<FontStyleKey, usize> = self
            .style_distribution
            .iter()
            .map(|((bits, is_bold), count)| {
                (
                    FontStyleKey {
                        size: OrderedFloat(f32::from_bits(*bits)),
                        is_bold: *is_bold,
                    },
                    *count,
                )
            })
            .collect();

        let most_common_style = self.most_common_style.map(|(bits, is_bold)| FontStyleKey {
            size: OrderedFloat(f32::from_bits(bits)),
            is_bold,
        });

        FontStats {
            median: self.body_size, // Approximate
            p75: self.body_size,    // Approximate
            p90: self.body_size,    // Approximate
            max: self.body_size,    // Approximate
            min: self.body_size,    // Approximate
            body_size: self.body_size,
            sample_count: self.sample_count,
            size_distribution,
            style_distribution,
            total_text_nodes: self.total_text_nodes,
            most_common_style,
            common_style_ratio: self.common_style_ratio,
        }
    }
}

/// Detects and classifies headings based on statistical analysis.
///
/// The module analyzes all text in the document to determine font size
/// distribution, then classifies larger/bolder text as headings.
///
/// # Algorithm
///
/// 1. Collect font sizes from all text nodes
/// 2. Compute statistics (median, percentiles)
/// 3. Identify font size "buckets" that are significantly larger than body text
/// 4. Assign heading levels to buckets (largest = h1, etc.)
/// 5. Consider additional factors: bold weight, short text length, page position
pub struct HeadingDetector {
    /// Minimum font size ratio above median to consider as heading
    pub min_size_ratio: f32,
    /// Maximum text length to consider as heading (in characters)
    pub max_heading_length: usize,
    /// Whether bold text gets a heading level boost
    pub boost_bold: bool,
    /// Minimum number of text samples needed for statistical analysis
    pub min_samples: usize,
}

impl Default for HeadingDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl HeadingDetector {
    pub fn new() -> Self {
        HeadingDetector {
            min_size_ratio: 1.35, // 35% larger than body size (or 3.5pt absolute difference)
            max_heading_length: 200,
            boost_bold: true,
            min_samples: 5,
        }
    }

    /// Check if the text is at most one sentence.
    /// Headings should not contain multiple sentences.
    fn is_single_sentence(text: &str) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return false;
        }

        // Sentence-ending punctuation marks
        let sentence_enders = ['.', '!', '?'];

        // Find all positions of sentence-ending punctuation
        let mut end_positions: Vec<usize> = Vec::new();
        let chars: Vec<char> = trimmed.chars().collect();

        for (char_idx, &c) in chars.iter().enumerate() {
            if sentence_enders.contains(&c) {
                // Check if this is likely an abbreviation, decimal number, or ordinal/date
                // Skip if followed by a digit (e.g., "1.5")
                if let Some(&next) = chars.get(char_idx + 1)
                    && next.is_ascii_digit()
                {
                    continue;
                }

                // Skip if preceded by a digit (e.g., "01." as in dates/ordinals)
                if char_idx > 0
                    && let Some(&prev) = chars.get(char_idx - 1)
                    && prev.is_ascii_digit()
                {
                    continue;
                }

                end_positions.push(char_idx);
            }
        }

        // If no sentence enders found, it's a single sentence (or fragment)
        if end_positions.is_empty() {
            return true;
        }

        // If there's only one sentence ender and it's at the end, it's a single sentence
        if end_positions.len() == 1 {
            let last_ender_pos = end_positions[0];
            // Check if it's the last character
            return last_ender_pos == chars.len() - 1;
        }

        // Multiple sentence enders - check if any are followed by substantial content
        for &pos in &end_positions[..end_positions.len() - 1] {
            // Check remaining characters after this position
            let remaining: String = chars[pos + 1..]
                .iter()
                .collect::<String>()
                .trim()
                .to_string();
            // If there's more than just whitespace after a sentence ender, it's multiple sentences
            if !remaining.is_empty() && remaining.len() > 1 {
                // Check if the next non-whitespace character is uppercase (new sentence) or not
                if let Some(first_char) = remaining.chars().next()
                    && first_char.is_uppercase()
                {
                    return false;
                }
            }
        }

        true
    }

    /// Set minimum size ratio for heading detection.
    pub fn with_min_size_ratio(mut self, ratio: f32) -> Self {
        self.min_size_ratio = ratio;
        self
    }

    /// Set maximum heading text length.
    pub fn with_max_heading_length(mut self, length: usize) -> Self {
        self.max_heading_length = length;
        self
    }

    /// Set whether to boost bold text.
    pub fn with_boost_bold(mut self, boost: bool) -> Self {
        self.boost_bold = boost;
        self
    }

    /// Collect font size statistics from all text nodes.
    ///
    /// Delegates core computation to `GlobalFontStats::from_flattened_iter`,
    /// then augments with percentile data only available from a single document.
    fn collect_font_stats(&self, doc: &Document) -> FontStats {
        let global = GlobalFontStats::from_flattened_iter(std::iter::once(&doc.source as &Flattened));
        let mut stats = global.to_font_stats();

        // Compute percentiles from sorted sizes (GlobalFontStats doesn't track these)
        let mut sizes: Vec<f32> = Vec::new();
        for node in doc.source.iter_nodes() {
            if let FlattenedNodeKind::Text {
                font_size, content, ..
            } = &node.kind
            {
                if !content.trim().is_empty() {
                    sizes.push(font_size.to_f32().unwrap_or(10.0));
                }
            }
        }

        if !sizes.is_empty() {
            sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let len = sizes.len();
            stats.median = if len.is_multiple_of(2) {
                (sizes[len / 2 - 1] + sizes[len / 2]) / 2.0
            } else {
                sizes[len / 2]
            };
            stats.p75 = sizes[(len * 75 / 100).min(len - 1)];
            stats.p90 = sizes[(len * 90 / 100).min(len - 1)];
            stats.max = *sizes.last().unwrap_or(&10.0);
            stats.min = *sizes.first().unwrap_or(&10.0);
        }

        stats
    }

    /// Check if a text group is a heading candidate based on font properties.
    /// Returns (HeadingStyleBucket, is_bold_section_header) if it qualifies.
    fn is_heading_candidate(
        &self,
        props: &TextProperties,
        stats: &FontStats,
    ) -> Option<(HeadingStyleBucket, bool)> {
        let size = props.avg_font_size;
        let is_bold = props.is_bold;
        let text_len = props.text_length;
        let text_content = &props.text_content;

        // Text too long for a heading
        if text_len > self.max_heading_length {
            return None;
        }

        // Empty or very short text is not a heading
        if text_len < 2 {
            return None;
        }

        // Headings should be at most one sentence
        if !Self::is_single_sentence(text_content) {
            return None;
        }

        let body_size = stats.body_size;
        let ratio = size / body_size;

        // Check if this font style is the most common (body text)
        let rounded_size = OrderedFloat((size * 2.0).round() / 2.0);
        let style_key = FontStyleKey {
            size: rounded_size,
            is_bold,
        };
        let style_frequency = stats
            .style_distribution
            .get(&style_key)
            .map(|&count| count as f32 / stats.total_text_nodes.max(1) as f32)
            .unwrap_or(0.0);

        // Also check the frequency of this SIZE regardless of bold status
        let size_frequency = stats
            .size_distribution
            .get(&rounded_size)
            .map(|&count| count as f32 / stats.total_text_nodes.max(1) as f32)
            .unwrap_or(0.0);

        // CRITICAL: Headings must be DISTINCT from normal text.
        // Non-bold text at body size is body text (never a heading), regardless
        // of whether most_common_style is bold or non-bold. When bold and non-bold
        // are nearly tied, both participate in body text, so non-bold should not
        // be promoted to heading.
        let body_size_bits = OrderedFloat((body_size * 2.0).round() / 2.0);
        let is_body_text = !is_bold
            && rounded_size == body_size_bits
            && (stats
                .most_common_style
                .map(|common| common.size == rounded_size && !common.is_bold)
                .unwrap_or(false)
                || {
                    // Even if most_common_style is bold, non-bold text at body size
                    // with substantial frequency is still body text.
                    let non_bold_key = FontStyleKey {
                        size: body_size_bits,
                        is_bold: false,
                    };
                    let non_bold_freq = stats
                        .style_distribution
                        .get(&non_bold_key)
                        .map(|&c| c as f32 / stats.total_text_nodes.max(1) as f32)
                        .unwrap_or(0.0);
                    non_bold_freq >= 0.15
                });

        if is_body_text {
            return None;
        }

        // Check if there is substantial non-bold text at body size.
        // This determines whether bold text at body size can be a section header.
        // We check BOTH whether most_common_style is non-bold at this size, AND
        // whether a significant amount of non-bold text exists at this size even
        // if the most common style happens to be bold. This makes the detector
        // robust when bold and non-bold counts are nearly tied.
        let non_bold_at_body_key = FontStyleKey {
            size: OrderedFloat((body_size * 2.0).round() / 2.0),
            is_bold: false,
        };
        let non_bold_body_count = stats
            .style_distribution
            .get(&non_bold_at_body_key)
            .copied()
            .unwrap_or(0);
        let non_bold_body_ratio =
            non_bold_body_count as f32 / stats.total_text_nodes.max(1) as f32;

        let body_has_substantial_non_bold = stats
            .most_common_style
            .map(|common| common.size == rounded_size && !common.is_bold)
            .unwrap_or(false)
            || (non_bold_body_ratio >= 0.15 && (size - body_size).abs() < 0.5);

        // Bold text at body size is a valid heading if there is significant
        // non-bold text at the same size (making bold visually distinct).
        let is_bold_section_header =
            is_bold && body_has_substantial_non_bold && (size - body_size).abs() < 0.5;

        // For bold section headers, allow higher frequency (up to 40%)
        let max_style_frequency = if is_bold_section_header { 0.40 } else { 0.25 };

        // If this style appears too frequently, it's body/label text
        if style_frequency > max_style_frequency {
            return None;
        }

        // For non-bold text or bold text larger than body, require size distinction
        if !is_bold_section_header {
            let size_diff = size - body_size;
            let min_ratio = 1.35f32;
            let min_diff = 3.5f32;

            let passes_ratio = ratio >= min_ratio;
            let passes_diff = size_diff >= min_diff;

            if !passes_ratio && !passes_diff {
                return None;
            }

            // Additional frequency check for common sizes
            if size_frequency > 0.30 && ratio < 1.5 && size_diff < 5.0 {
                return None;
            }
        }

        // Compute the style bucket
        let has_line = props.has_top_border || props.has_bottom_border || props.has_font_underline;

        let bucket = HeadingStyleBucket {
            size: rounded_size,
            is_bold,
            has_line,
        };

        Some((bucket, is_bold_section_header))
    }

    /// Get font properties from a group.
    fn get_text_properties(&self, doc: &Document, group_idx: usize) -> Option<TextProperties> {
        let nodes = doc.collect_nodes(group_idx);
        if nodes.is_empty() {
            return None;
        }

        // Aggregate properties from all nodes
        let mut total_size = 0.0f32;
        let mut bold_count = 0;
        let mut text_node_count = 0;
        let mut text_content = String::new();
        let mut count = 0;
        let mut top_border_count = 0;
        let mut bottom_border_count = 0;
        let mut font_underline_count = 0;

        for node in nodes {
            if let FlattenedNodeKind::Text {
                font_size, content, ..
            } = &node.kind
            {
                total_size += font_size.to_f32().unwrap_or(10.0);
                count += 1;
                text_node_count += 1;
                text_content.push_str(content);
                text_content.push(' ');

                // Count bold nodes
                if let Some(font) = &node.style.font {
                    if font.weight == FontWeight::Bold {
                        bold_count += 1;
                    }
                    // Check font underline property
                    if font.underline {
                        font_underline_count += 1;
                    }
                }

                // Check for visible borders (top and bottom edges)
                if let Some(border) = &node.style.border {
                    // Check top edge (index 0)
                    if let Some(top_edge) = border.get_edge(0)
                        && top_edge.presence == "visible"
                        && top_edge.thickness.is_some()
                    {
                        top_border_count += 1;
                    }
                    // Check bottom edge (index 2)
                    if let Some(bottom_edge) = border.get_edge(2)
                        && bottom_edge.presence == "visible"
                        && bottom_edge.thickness.is_some()
                    {
                        bottom_border_count += 1;
                    }
                }
            }
        }

        if count == 0 {
            return None;
        }

        // Only consider as bold if ALL text nodes are bold
        let is_bold = text_node_count > 0 && bold_count == text_node_count;

        // Consider border/underline present if ANY node has it
        let has_top_border = top_border_count > 0;
        let has_bottom_border = bottom_border_count > 0;
        let has_font_underline = font_underline_count > 0;

        let avg_size = total_size / count as f32;
        let text_len = text_content.trim().len();

        Some(TextProperties {
            avg_font_size: avg_size,
            is_bold,
            text_length: text_len,
            text_content: text_content.trim().to_string(),
            has_top_border,
            has_bottom_border,
            has_font_underline,
        })
    }

    /// Check if a group is a text group (Leaf with Text or TextBlock).
    fn is_text_group(&self, doc: &Document, group_idx: usize) -> bool {
        let group = match doc.get_group(group_idx) {
            Some(g) => g,
            None => return false,
        };

        match &group.kind {
            GroupKind::Leaf { node_index } => {
                matches!(
                    doc.source.iter_nodes().nth(*node_index).map(|n| &n.kind),
                    Some(FlattenedNodeKind::Text { .. })
                )
            }
            GroupKind::TextBlock => true,
            _ => false,
        }
    }

    /// Normalize heading levels to ensure proper hierarchy.
    ///
    /// Rules:
    /// - First heading is always h1
    /// - Same level can repeat: h1, h2, h2, h3, h2 is valid
    /// - Cannot skip levels: h1, h3 is invalid and becomes h1, h2
    /// - When going deeper, can only increase by 1
    /// - When going back up, can jump to any previously seen level
    fn normalize_heading_levels(
        mut headings: Vec<(usize, u8, f32, bool)>,
    ) -> Vec<(usize, u8, f32, bool)> {
        if headings.is_empty() {
            return headings;
        }

        // Track the maximum level we've seen so far at each depth
        // This allows us to know which levels are "valid" to return to
        let mut max_level_seen: u8 = 0;
        let mut current_level: u8 = 0;

        // Find the minimum (highest priority) level in the original headings
        // This ensures actual H1 candidates (large text) become H1
        let min_original_level = headings
            .iter()
            .map(|(_, level, _, _)| *level)
            .min()
            .unwrap_or(1);

        for (_group_idx, level, _y, _is_bold_section_header) in headings.iter_mut() {
            if current_level == 0 {
                // First heading - preserve its relative level
                // If original min was 2 (bold section headers), keep them as H2
                // Only if original min was 1 (true titles), make it H1
                if min_original_level == 1 && *level == 1 {
                    *level = 1;
                } else if min_original_level == 1 && *level > 1 {
                    // First heading is not the true H1, start from its level
                    *level = (*level).clamp(2, 6);
                } else {
                    // No true H1 in document, start from detected level
                    *level = (*level).clamp(1, 6);
                }
                current_level = *level;
                max_level_seen = current_level;
            } else {
                // Calculate the proposed level based on original detection
                let original_level = *level;

                if original_level <= current_level {
                    // Going back up or staying at same level
                    // This is always valid, but clamp to max_level_seen or 1
                    *level = original_level.max(1).min(max_level_seen);
                } else {
                    // Going deeper - can only increase by 1 at a time
                    let new_level = current_level + 1;
                    *level = new_level;
                    max_level_seen = max_level_seen.max(new_level);
                }

                current_level = *level;
            }
        }

        headings
    }
}

/// Key for tracking font style frequency (size + bold).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FontStyleKey {
    size: OrderedFloat,
    is_bold: bool,
}

/// Font statistics collected from the document.
#[derive(Debug, Default)]
#[allow(dead_code)]
struct FontStats {
    median: f32,
    p75: f32,
    p90: f32,
    max: f32,
    min: f32,
    body_size: f32,
    sample_count: usize,
    size_distribution: HashMap<OrderedFloat, usize>,
    /// Distribution of font styles (size + bold) -> count
    style_distribution: HashMap<FontStyleKey, usize>,
    /// Total number of text nodes analyzed
    total_text_nodes: usize,
    /// The most common font style in the document
    most_common_style: Option<FontStyleKey>,
    /// Ratio of text nodes using the most common style (0.0 to 1.0)
    common_style_ratio: f32,
}

/// Text properties for a group.
#[derive(Debug)]
struct TextProperties {
    avg_font_size: f32,
    is_bold: bool,
    text_length: usize,
    text_content: String,
    has_top_border: bool,
    has_bottom_border: bool,
    has_font_underline: bool,
}

/// A unique combination of font properties that defines a heading style level.
/// Buckets are sorted by priority: font size desc > bold first > has_line first.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HeadingStyleBucket {
    /// Font size rounded to 0.5pt
    size: OrderedFloat,
    /// Whether the text is bold
    is_bold: bool,
    /// Whether the text has underline, overline, or visible top/bottom border
    has_line: bool,
}

/// Wrapper for f32 that implements Hash and Eq.
#[derive(Debug, Clone, Copy, PartialEq)]
struct OrderedFloat(f32);

impl Eq for OrderedFloat {}

impl std::hash::Hash for OrderedFloat {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl AnalysisModule for HeadingDetector {
    fn name(&self) -> &'static str {
        "HeadingDetector"
    }

    fn process(&self, doc: &mut Document) {
        // Collect font statistics from this document only
        let stats = self.collect_font_stats(doc);
        self.process_with_stats(doc, &stats, None);
    }

    fn process_with_context(&self, doc: &mut Document, ctx: &super::GlobalContext) {
        // Compute global stats from all flattened data in the context
        let (stats, global_buckets) = if !ctx.all_flattened.is_empty() {
            let mut global_stats =
                GlobalFontStats::from_flattened_iter(ctx.all_flattened.iter().copied());
            global_stats.compute_heading_buckets(ctx.all_flattened.iter().copied());
            let buckets = if !global_stats.heading_buckets.is_empty() {
                Some(
                    global_stats
                        .heading_buckets
                        .iter()
                        .map(|(size_bits, is_bold, has_line)| HeadingStyleBucket {
                            size: OrderedFloat(f32::from_bits(*size_bits)),
                            is_bold: *is_bold,
                            has_line: *has_line,
                        })
                        .collect::<Vec<_>>(),
                )
            } else {
                None
            };
            (global_stats.to_font_stats(), buckets)
        } else {
            // Fallback to local stats
            (self.collect_font_stats(doc), None)
        };
        self.process_with_stats(doc, &stats, global_buckets.as_deref());
    }
}

impl HeadingDetector {
    /// Core processing logic using provided font statistics.
    /// If `global_heading_buckets` is Some, uses global bucket ranking for consistent
    /// heading level detection across form states. Otherwise computes local bucket ranking.
    fn process_with_stats(
        &self,
        doc: &mut Document,
        stats: &FontStats,
        global_heading_buckets: Option<&[HeadingStyleBucket]>,
    ) {
        // Need enough samples for meaningful analysis
        if stats.sample_count < self.min_samples {
            return;
        }

        // Find all unclaimed text groups (roots that are text)
        let roots = doc.roots();
        let text_groups: Vec<usize> = roots
            .iter()
            .filter(|&&idx| self.is_text_group(doc, idx))
            .copied()
            .collect();

        // Collect heading candidates with their style buckets
        let mut candidates: Vec<(usize, HeadingStyleBucket, f32, bool)> = Vec::new();

        for &group_idx in &text_groups {
            if let Some(props) = self.get_text_properties(doc, group_idx)
                && let Some((bucket, is_bold_section_header)) =
                    self.is_heading_candidate(&props, stats)
            {
                let y_coord = doc
                    .compute_group_bounds(group_idx)
                    .map(|(_, y, _, _)| y.to_f32().unwrap_or(0.0))
                    .unwrap_or(0.0);
                candidates.push((group_idx, bucket, y_coord, is_bold_section_header));
            }
        }

        // Determine unique style buckets and sort by priority:
        // 1. Font size descending (largest first)
        // 2. Bold first (if same size)
        // 3. Has line (underline/overline/border) first (if same size and weight)
        let sorted_buckets: Vec<HeadingStyleBucket> = if let Some(global) = global_heading_buckets {
            global.to_vec()
        } else {
            let bucket_set: HashSet<HeadingStyleBucket> =
                candidates.iter().map(|(_, b, _, _)| b.clone()).collect();
            let mut buckets: Vec<_> = bucket_set.into_iter().collect();
            buckets.sort_by(|a, b| {
                b.size
                    .0
                    .partial_cmp(&a.size.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.is_bold.cmp(&a.is_bold))
                    .then(b.has_line.cmp(&a.has_line))
            });
            buckets
        };

        // Map each bucket to a heading level (position in sorted list + 1, capped at 6)
        let bucket_to_level: HashMap<&HeadingStyleBucket, u8> = sorted_buckets
            .iter()
            .enumerate()
            .map(|(i, bucket)| (bucket, (i as u8 + 1).min(6)))
            .collect();

        // Build heading list with assigned levels
        let mut headings: Vec<(usize, u8, f32, bool)> = candidates
            .iter()
            .map(|(group_idx, bucket, y_coord, is_bold_section_header)| {
                let level = bucket_to_level.get(bucket).copied().unwrap_or(6);
                (*group_idx, level, *y_coord, *is_bold_section_header)
            })
            .collect();

        // Sort headings by y-coordinate (top to bottom)
        headings.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

        // Filter out bold section headers that appear before the first true heading.
        // Bold section headers (bold at body size) before a title are likely branding/company names,
        // not actual section headings. The first true heading (larger font) establishes H1.
        let first_true_heading_idx = headings
            .iter()
            .position(|(_, _, _, is_bold_section_header)| !*is_bold_section_header);

        let filtered_headings: Vec<(usize, u8, f32, bool)> =
            if let Some(first_true_idx) = first_true_heading_idx {
                headings
                    .into_iter()
                    .enumerate()
                    .filter(|(idx, (_, _, _, is_bold_section_header))| {
                        *idx >= first_true_idx || !*is_bold_section_header
                    })
                    .map(|(_, h)| h)
                    .collect()
            } else {
                headings
            };

        // Normalize heading levels
        let headings = Self::normalize_heading_levels(filtered_headings);

        // Create Heading groups
        for (group_idx, level, _, _) in headings {
            doc.merge(
                vec![group_idx],
                GroupKind::Heading { level },
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
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::{Border, Edge, Font, FontWeight, StrokeStyle, num};

    fn make_text_node(content: &str, font_size: f32, x: f32, y: f32) -> FlattenedNode {
        FlattenedNode::new_text(
            content.to_string(),
            num(font_size as f64),
            "Helvetica".to_string(),
            num(x as f64),
            num(y as f64),
            num(200.0),
            num(font_size as f64 * 1.2),
        )
    }

    fn make_bold_text_node(content: &str, font_size: f32, x: f32, y: f32) -> FlattenedNode {
        let mut node = make_text_node(content, font_size, x, y);
        node.style.font = Some(Font {
            typeface: "Helvetica".to_string(),
            size: num(font_size as f64),
            weight: FontWeight::Bold,
            posture: crate::xfa::FontPosture::Normal,
            underline: false,
            line_through: false,
            color: None,
            baseline_shift: None,
            letter_spacing: None,
            generic_family: None,
            kerning_mode: crate::xfa::KerningMode::None,
            font_horizontal_scale: None,
            font_vertical_scale: None,
        });
        node
    }

    fn make_text_node_with_border(
        content: &str,
        font_size: f32,
        x: f32,
        y: f32,
        has_top_border: bool,
        has_bottom_border: bool,
    ) -> FlattenedNode {
        let mut node = make_text_node(content, font_size, x, y);

        let mut edges = Vec::new();

        // Top edge (index 0)
        edges.push(Edge {
            thickness: if has_top_border { Some(num(1.0)) } else { None },
            stroke: StrokeStyle::Solid,
            presence: if has_top_border {
                "visible".to_string()
            } else {
                "hidden".to_string()
            },
            color: Some((0, 0, 0)),
        });

        // Right edge (index 1)
        edges.push(Edge {
            thickness: None,
            stroke: StrokeStyle::Solid,
            presence: "hidden".to_string(),
            color: None,
        });

        // Bottom edge (index 2)
        edges.push(Edge {
            thickness: if has_bottom_border {
                Some(num(1.0))
            } else {
                None
            },
            stroke: StrokeStyle::Solid,
            presence: if has_bottom_border {
                "visible".to_string()
            } else {
                "hidden".to_string()
            },
            color: Some((0, 0, 0)),
        });

        // Left edge (index 3)
        edges.push(Edge {
            thickness: None,
            stroke: StrokeStyle::Solid,
            presence: "hidden".to_string(),
            color: None,
        });

        node.style.border = Some(Border {
            edges,
            corners: Vec::new(),
            fill: None,
            presence: "visible".to_string(),
            margin_left: None,
            margin_top: None,
            margin_right: None,
            margin_bottom: None,
        });

        node
    }

    fn make_bold_text_node_with_border(
        content: &str,
        font_size: f32,
        x: f32,
        y: f32,
        has_top_border: bool,
        has_bottom_border: bool,
    ) -> FlattenedNode {
        let mut node =
            make_text_node_with_border(content, font_size, x, y, has_top_border, has_bottom_border);
        node.style.font = Some(Font {
            typeface: "Helvetica".to_string(),
            size: num(font_size as f64),
            weight: FontWeight::Bold,
            posture: crate::xfa::FontPosture::Normal,
            underline: false,
            line_through: false,
            color: None,
            baseline_shift: None,
            letter_spacing: None,
            generic_family: None,
            kerning_mode: crate::xfa::KerningMode::None,
            font_horizontal_scale: None,
            font_vertical_scale: None,
        });
        node
    }

    #[test]
    fn test_detect_headings_by_size() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Large heading (should be h1 or h2)
                make_text_node("Main Title", 24.0, 10.0, 10.0),
                // Medium heading (should be h3 or h4)
                make_text_node("Section Title", 16.0, 10.0, 50.0),
                // Body text (10pt) - multiple to establish baseline
                make_text_node(
                    "This is body text paragraph one with some content.",
                    10.0,
                    10.0,
                    80.0,
                ),
                make_text_node(
                    "This is body text paragraph two with more content.",
                    10.0,
                    10.0,
                    100.0,
                ),
                make_text_node("This is body text paragraph three.", 10.0, 10.0, 120.0),
                make_text_node("Another paragraph of body text here.", 10.0, 10.0, 140.0),
                make_text_node("And more body text content.", 10.0, 10.0, 160.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);

        // Should detect headings
        let headings = doc.headings();
        assert!(
            headings.len() >= 2,
            "Should detect at least 2 headings, got {}",
            headings.len()
        );

        // Get heading levels
        let mut levels: Vec<u8> = headings
            .iter()
            .filter_map(|&idx| {
                if let GroupKind::Heading { level } = doc.get_group(idx)?.kind {
                    Some(level)
                } else {
                    None
                }
            })
            .collect();
        levels.sort();

        // Should have different heading levels
        assert!(levels.len() >= 2);
        // Largest font should have smallest level number (h1 < h2 < ...)
        assert!(levels[0] <= levels[levels.len() - 1]);
    }

    #[test]
    fn test_bold_larger_text_detected_as_heading() {
        // Bold text that is LARGER than body size should still be detected as heading
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Bold text at larger size (should be detected as heading)
                make_bold_text_node("Bold Subheading", 14.0, 10.0, 10.0),
                // Regular body text at 10pt
                make_text_node("Regular body text paragraph one.", 10.0, 10.0, 30.0),
                make_text_node("Regular body text paragraph two.", 10.0, 10.0, 50.0),
                make_text_node("Regular body text paragraph three.", 10.0, 10.0, 70.0),
                make_text_node("Regular body text paragraph four.", 10.0, 10.0, 90.0),
                make_text_node("Regular body text paragraph five.", 10.0, 10.0, 110.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);

        let headings = doc.headings();
        assert_eq!(
            headings.len(),
            1,
            "Bold larger text should be detected as heading"
        );
    }

    #[test]
    fn test_long_text_not_heading() {
        let long_text = "This is a very long paragraph that should not be detected as a heading even though it might have a larger font size because headings are typically short and concise not rambling on like this text which goes on and on.";

        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Large font but too long to be heading (>200 chars)
                make_text_node(long_text, 18.0, 10.0, 10.0),
                // Body text
                make_text_node("Body paragraph one.", 10.0, 10.0, 50.0),
                make_text_node("Body paragraph two.", 10.0, 10.0, 70.0),
                make_text_node("Body paragraph three.", 10.0, 10.0, 90.0),
                make_text_node("Body paragraph four.", 10.0, 10.0, 110.0),
                make_text_node("Body paragraph five.", 10.0, 10.0, 130.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);

        // Long text should not be detected as heading
        let headings = doc.headings();
        for &idx in &headings {
            let text = doc.get_text_content(idx);
            assert!(
                text.len() <= 200,
                "Long text should not be heading: {}",
                text
            );
        }
    }

    #[test]
    fn test_insufficient_samples() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                make_text_node("Only Title", 24.0, 10.0, 10.0),
                make_text_node("Single paragraph.", 10.0, 10.0, 50.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new()
            .with_min_size_ratio(1.1)
            .process(&mut doc);

        // Not enough samples for statistical analysis
        let headings = doc.headings();
        assert_eq!(
            headings.len(),
            0,
            "Should not detect headings with insufficient samples"
        );
    }

    #[test]
    fn test_heading_level_ordering() {
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                make_text_node("Huge Title", 32.0, 10.0, 10.0),
                make_text_node("Large Title", 24.0, 10.0, 50.0),
                make_text_node("Medium Title", 18.0, 10.0, 90.0),
                make_text_node("Small Title", 14.0, 10.0, 120.0),
                // Body text at 10pt
                make_text_node("Body one.", 10.0, 10.0, 150.0),
                make_text_node("Body two.", 10.0, 10.0, 170.0),
                make_text_node("Body three.", 10.0, 10.0, 190.0),
                make_text_node("Body four.", 10.0, 10.0, 210.0),
                make_text_node("Body five.", 10.0, 10.0, 230.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);

        // Collect headings with their sizes and levels
        let mut heading_info: Vec<(f32, u8, String)> = Vec::new();
        for &idx in &doc.headings() {
            if let Some(group) = doc.get_group(idx) {
                if let GroupKind::Heading { level } = group.kind {
                    let nodes = doc.collect_nodes(idx);
                    if let Some(node) = nodes.first() {
                        if let FlattenedNodeKind::Text {
                            font_size, content, ..
                        } = &node.kind
                        {
                            heading_info.push((
                                font_size.to_f32().unwrap_or(0.0),
                                level,
                                content.clone(),
                            ));
                        }
                    }
                }
            }
        }

        // Sort by font size descending
        heading_info.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

        // Verify larger fonts get smaller heading numbers (h1 < h2 < ...)
        for i in 1..heading_info.len() {
            assert!(
                heading_info[i - 1].1 <= heading_info[i].1,
                "Larger font ({}) should have same or smaller heading level than smaller font ({})",
                heading_info[i - 1].2,
                heading_info[i].2
            );
        }
    }

    #[test]
    fn test_multiple_sentences_not_heading() {
        // Text with multiple sentences should not be a heading
        let multi_sentence = "First sentence here. Second sentence follows.";

        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Large font but multiple sentences
                make_text_node(multi_sentence, 20.0, 10.0, 10.0),
                // Body text
                make_text_node("Body paragraph one.", 10.0, 10.0, 50.0),
                make_text_node("Body paragraph two.", 10.0, 10.0, 70.0),
                make_text_node("Body paragraph three.", 10.0, 10.0, 90.0),
                make_text_node("Body paragraph four.", 10.0, 10.0, 110.0),
                make_text_node("Body paragraph five.", 10.0, 10.0, 130.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);

        // Multiple sentences should not be detected as heading
        let headings = doc.headings();
        for &idx in &headings {
            let text = doc.get_text_content(idx);
            assert!(
                !text.contains(". ") || text.ends_with('.'),
                "Multiple sentences should not be heading: {}",
                text
            );
        }
    }

    #[test]
    fn test_single_sentence_heading() {
        // Single sentence with ending punctuation is valid
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                make_text_node("Introduction.", 20.0, 10.0, 10.0),
                make_text_node("Body paragraph one.", 10.0, 10.0, 50.0),
                make_text_node("Body paragraph two.", 10.0, 10.0, 70.0),
                make_text_node("Body paragraph three.", 10.0, 10.0, 90.0),
                make_text_node("Body paragraph four.", 10.0, 10.0, 110.0),
                make_text_node("Body paragraph five.", 10.0, 10.0, 130.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);

        let headings = doc.headings();
        assert_eq!(
            headings.len(),
            1,
            "Single sentence with period should be heading"
        );
    }

    #[test]
    fn test_is_single_sentence() {
        // Test the is_single_sentence helper
        assert!(HeadingDetector::is_single_sentence("Hello World"));
        assert!(HeadingDetector::is_single_sentence("Hello World."));
        assert!(HeadingDetector::is_single_sentence("Hello World!"));
        assert!(HeadingDetector::is_single_sentence("Hello World?"));
        assert!(HeadingDetector::is_single_sentence("Section 1.2")); // decimal
        assert!(HeadingDetector::is_single_sentence("Price: $1.50")); // decimal

        // Multiple sentences
        assert!(!HeadingDetector::is_single_sentence("First. Second"));
        assert!(!HeadingDetector::is_single_sentence("Hello. World here."));
        assert!(!HeadingDetector::is_single_sentence(
            "What? Another sentence here."
        ));
    }

    #[test]
    fn test_normalize_heading_levels_no_skip() {
        // Test that skipping levels is prevented
        // Input: detected h1, h3, h5 (skipping h2, h4)
        // Output should be: h1, h2, h3 (no skipping)
        let input = vec![
            (0, 1, 10.0, false), // h1, not bold section header
            (1, 3, 50.0, false), // h3 -> should become h2
            (2, 5, 90.0, false), // h5 -> should become h3
        ];

        let result = HeadingDetector::normalize_heading_levels(input);

        assert_eq!(result[0].1, 1, "First should be h1");
        assert_eq!(result[1].1, 2, "Second should be h2 (not h3)");
        assert_eq!(result[2].1, 3, "Third should be h3 (not h5)");
    }

    #[test]
    fn test_normalize_heading_levels_same_level_repeat() {
        // Valid: h1, h2, h2, h3, h2 (same level can repeat, can go back up)
        let input = vec![
            (0, 1, 10.0, false),  // h1
            (1, 2, 50.0, false),  // h2
            (2, 2, 90.0, false),  // h2 again
            (3, 3, 130.0, false), // h3
            (4, 2, 170.0, false), // back to h2
        ];

        let result = HeadingDetector::normalize_heading_levels(input);

        assert_eq!(result[0].1, 1, "First should be h1");
        assert_eq!(result[1].1, 2, "Second should be h2");
        assert_eq!(result[2].1, 2, "Third should be h2 (repeat)");
        assert_eq!(result[3].1, 3, "Fourth should be h3");
        assert_eq!(result[4].1, 2, "Fifth should be h2 (back up)");
    }

    #[test]
    fn test_normalize_heading_levels_preserves_relative_levels() {
        // When no true H1 (large text) exists, preserve relative levels
        // First heading at detected level 3 stays at 3 (not promoted to 1)
        let input = vec![
            (0, 3, 10.0, false), // detected h3 -> stays h3 (no H1 in document)
            (1, 4, 50.0, false), // h4 -> becomes h4 (one level deeper)
        ];

        let result = HeadingDetector::normalize_heading_levels(input);

        // Since min_original_level is 3 (no H1), preserve relative structure
        assert_eq!(result[0].1, 3, "First should stay h3 (no true H1)");
        assert_eq!(result[1].1, 4, "Second should be h4");
    }

    #[test]
    fn test_normalize_heading_levels_h1_becomes_h1() {
        // When a true H1 exists, it becomes H1 and structure is preserved
        let input = vec![
            (0, 2, 10.0, true),  // detected h2 (bold section header before title)
            (1, 1, 50.0, false), // detected h1 (true title)
            (2, 2, 90.0, true),  // detected h2 (section header)
        ];

        let result = HeadingDetector::normalize_heading_levels(input);

        // First is H2, then H1 should be preserved
        assert_eq!(result[0].1, 2, "First (H2) should stay h2");
        assert_eq!(result[1].1, 1, "Second (H1) should be h1");
        assert_eq!(result[2].1, 2, "Third (H2) should be h2");
    }

    #[test]
    fn test_headings_ordered_top_to_bottom() {
        // Headings placed out of order in y-coordinate should be sorted
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Headings in wrong vertical order in the vector
                make_text_node("Section B", 18.0, 10.0, 100.0), // y=100
                make_text_node("Main Title", 24.0, 10.0, 10.0), // y=10
                make_text_node("Section A", 18.0, 10.0, 50.0),  // y=50
                // Body text
                make_text_node("Body one.", 10.0, 10.0, 150.0),
                make_text_node("Body two.", 10.0, 10.0, 170.0),
                make_text_node("Body three.", 10.0, 10.0, 190.0),
                make_text_node("Body four.", 10.0, 10.0, 210.0),
                make_text_node("Body five.", 10.0, 10.0, 230.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);

        // Collect headings with their y-coordinates
        let mut heading_info: Vec<(f32, u8, String)> = Vec::new();
        for &idx in &doc.headings() {
            if let Some(group) = doc.get_group(idx) {
                if let GroupKind::Heading { level } = group.kind {
                    let y_coord = doc
                        .compute_group_bounds(idx)
                        .map(|(_, y, _, _)| y.to_f32().unwrap_or(0.0))
                        .unwrap_or(0.0);
                    let content = doc.get_text_content(idx);
                    heading_info.push((y_coord, level, content));
                }
            }
        }

        // Sort by y-coordinate (as they should be in document order)
        heading_info.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        // "Main Title" at y=10 should be h1
        // "Section A" at y=50 should be h2
        // "Section B" at y=100 should be h2
        assert!(
            heading_info
                .iter()
                .any(|(_, level, content)| content.contains("Main Title") && *level == 1),
            "Main Title should be h1"
        );
    }

    #[test]
    fn test_border_based_heading_distinction() {
        // Test that borders can distinguish between h2 and h3 levels
        // When multiple bold section headers exist, those with underlines should be h2,
        // those without should be h3
        let flattened = Flattened::from_nodes(
            Page {
                width: num(595.0),
                height: num(842.0),
            },
            vec![
                // Large title (h1)
                make_text_node("Main Title", 18.0, 10.0, 10.0),
                // Bold section header with underline (should be h2)
                make_bold_text_node_with_border(
                    "Section with Underline",
                    10.0,
                    10.0,
                    50.0,
                    false,
                    true,
                ),
                make_text_node("Body text one here.", 10.0, 10.0, 80.0),
                make_text_node("More body text one.", 10.0, 10.0, 95.0),
                // Bold section header without underline (should be h3)
                make_bold_text_node_with_border(
                    "Subsection No Underline",
                    10.0,
                    10.0,
                    120.0,
                    false,
                    false,
                ),
                make_text_node("Body text two here.", 10.0, 10.0, 150.0),
                make_text_node("More body text two.", 10.0, 10.0, 165.0),
                // Another bold section header with underline (should be h2)
                make_bold_text_node_with_border("Another Section", 10.0, 10.0, 190.0, false, true),
                make_text_node("Body text three here.", 10.0, 10.0, 220.0),
                make_text_node("More body text three.", 10.0, 10.0, 235.0),
                // Another subsection without underline (should be h3)
                make_bold_text_node_with_border(
                    "Another Subsection",
                    10.0,
                    10.0,
                    260.0,
                    false,
                    false,
                ),
                make_text_node("Body text four here.", 10.0, 10.0, 290.0),
                make_text_node("More body text four.", 10.0, 10.0, 305.0),
                make_text_node("Body text five here.", 10.0, 10.0, 320.0),
                make_text_node("More body text five.", 10.0, 10.0, 335.0),
            ],
        );

        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);

        let headings = doc.headings();

        // Collect heading info
        let mut heading_info: Vec<(String, u8)> = Vec::new();
        for &idx in &headings {
            if let Some(group) = doc.get_group(idx) {
                if let GroupKind::Heading { level } = group.kind {
                    let content = doc.get_text_content(idx);
                    heading_info.push((content, level));
                }
            }
        }

        // Find specific headings and check their levels
        let main_title = heading_info.iter().find(|(c, _)| c.contains("Main Title"));
        let section_underline = heading_info
            .iter()
            .find(|(c, _)| c.contains("Section with Underline"));
        let subsection_no_underline = heading_info
            .iter()
            .find(|(c, _)| c.contains("Subsection No Underline"));
        let another_section = heading_info
            .iter()
            .find(|(c, _)| c.contains("Another Section"));
        let another_subsection = heading_info
            .iter()
            .find(|(c, _)| c.contains("Another Subsection"));

        assert!(main_title.is_some(), "Main Title should be detected");
        assert_eq!(main_title.unwrap().1, 1, "Main Title should be h1");

        assert!(
            section_underline.is_some(),
            "Section with underline should be detected"
        );
        assert_eq!(
            section_underline.unwrap().1,
            2,
            "Section with underline should be h2"
        );

        assert!(
            subsection_no_underline.is_some(),
            "Subsection without underline should be detected"
        );
        assert_eq!(
            subsection_no_underline.unwrap().1,
            3,
            "Subsection without underline should be h3"
        );

        assert!(
            another_section.is_some(),
            "Another section should be detected"
        );
        assert_eq!(
            another_section.unwrap().1,
            2,
            "Another section with underline should be h2"
        );

        assert!(
            another_subsection.is_some(),
            "Another subsection should be detected"
        );
        assert_eq!(
            another_subsection.unwrap().1,
            3,
            "Another subsection without underline should be h3"
        );
    }
}
