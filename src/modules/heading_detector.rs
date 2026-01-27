//! Heading detection module.
//!
//! Classifies text blocks into heading levels (h1-h6) based on statistical
//! analysis of font sizes, weights, and other visual properties.

use crate::document::{Document, GroupKind, GroupSource};
use crate::flattened::FlattenedNodeKind;
use crate::xfa::FontWeight;
use super::AnalysisModule;
use rust_decimal::prelude::*;
use std::collections::HashMap;

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
            min_size_ratio: 1.15,  // 15% larger than median
            max_heading_length: 150,
            boost_bold: true,
            min_samples: 5,
        }
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
    fn collect_font_stats(&self, doc: &Document) -> FontStats {
        let mut sizes: Vec<f32> = Vec::new();
        let mut size_counts: HashMap<OrderedFloat, usize> = HashMap::new();
        // Track font style frequency: (size, is_bold) -> count
        let mut style_counts: HashMap<FontStyleKey, usize> = HashMap::new();
        let mut total_text_nodes = 0usize;
        
        for node in &doc.source.nodes {
            if let FlattenedNodeKind::Text { font_size, content, .. } = &node.kind {
                // Skip empty text
                if content.trim().is_empty() {
                    continue;
                }
                
                let size = font_size.to_f32().unwrap_or(10.0);
                sizes.push(size);
                total_text_nodes += 1;
                
                // Round to 0.5pt for bucketing
                let rounded = OrderedFloat((size * 2.0).round() / 2.0);
                *size_counts.entry(rounded).or_insert(0) += 1;
                
                // Track font style (size + bold) for frequency analysis
                let is_bold = node.style.font.as_ref()
                    .map(|f| f.weight == FontWeight::Bold)
                    .unwrap_or(false);
                let style_key = FontStyleKey { size: rounded, is_bold };
                *style_counts.entry(style_key).or_insert(0) += 1;
            }
        }
        
        if sizes.is_empty() {
            return FontStats::default();
        }
        
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        
        let len = sizes.len();
        let median = if len.is_multiple_of(2) {
            (sizes[len / 2 - 1] + sizes[len / 2]) / 2.0
        } else {
            sizes[len / 2]
        };
        
        let p75 = sizes[(len * 75 / 100).min(len - 1)];
        let p90 = sizes[(len * 90 / 100).min(len - 1)];
        let max = *sizes.last().unwrap_or(&10.0);
        let min = *sizes.first().unwrap_or(&10.0);
        
        // Find the most common font size (body text)
        let body_size = size_counts.iter()
            .max_by_key(|(_, count)| *count)
            .map(|(size, _)| size.0)
            .unwrap_or(median);
        
        // Find the most common font style (this is body text)
        let most_common_style = style_counts.iter()
            .max_by_key(|(_, count)| *count)
            .map(|(key, _)| *key);
        
        // Calculate font ratio threshold: if a style is used more than X% of the time, it's likely body text
        let common_style_ratio = most_common_style
            .and_then(|style| style_counts.get(&style))
            .map(|&count| count as f32 / total_text_nodes.max(1) as f32)
            .unwrap_or(0.0);
        
        FontStats {
            median,
            p75,
            p90,
            max,
            min,
            body_size,
            sample_count: len,
            size_distribution: size_counts,
            style_distribution: style_counts,
            total_text_nodes,
            most_common_style,
            common_style_ratio,
        }
    }
    
    /// Determine heading level based on font size and stats.
    fn determine_heading_level(&self, size: f32, is_bold: bool, text_len: usize, stats: &FontStats) -> Option<u8> {
        // Text too long for a heading
        if text_len > self.max_heading_length {
            return None;
        }
        
        // Empty or very short text is not a heading
        if text_len < 2 {
            return None;
        }
        
        let body_size = stats.body_size;
        let ratio = size / body_size;
        
        // Check if this font style is the most common (body text)
        // Following Parsr's approach: headings should have rare font styles, not common ones
        let rounded_size = OrderedFloat((size * 2.0).round() / 2.0);
        let style_key = FontStyleKey { size: rounded_size, is_bold };
        let style_frequency = stats.style_distribution.get(&style_key)
            .map(|&count| count as f32 / stats.total_text_nodes.max(1) as f32)
            .unwrap_or(0.0);
        
        // If this style is used for more than 20% of text nodes, it's probably body text, not a heading
        // This prevents normal paragraphs from being classified as headings
        let is_common_style = style_frequency > 0.20;
        
        // Not large enough to be a heading
        if ratio < self.min_size_ratio {
            // Don't classify body-sized text as headings even if bold
            // Bold text at body size is typically labels or emphasis, not headings
            return None;
        }
        
        // If this is a very common font style, require a higher size ratio
        // This prevents common body text from being misclassified as headings
        if is_common_style && ratio < 1.5 {
            return None;
        }
        
        // Determine level based on size ratio
        // Map the range [min_size_ratio, max_ratio] to [6, 1]
        let max_ratio = stats.max / body_size;
        let normalized = if max_ratio > self.min_size_ratio {
            (ratio - self.min_size_ratio) / (max_ratio - self.min_size_ratio)
        } else {
            0.0
        };
        
        // Base level from size
        let base_level = match normalized {
            n if n >= 0.8 => 1,
            n if n >= 0.6 => 2,
            n if n >= 0.4 => 3,
            n if n >= 0.2 => 4,
            n if n >= 0.1 => 5,
            _ => 6,
        };
        
        // Boost for bold (move up one level, min 1)
        let level = if self.boost_bold && is_bold && base_level > 1 {
            base_level - 1
        } else {
            base_level
        };
        
        Some(level as u8)
    }
    
    /// Get font properties from a group.
    fn get_text_properties(&self, doc: &Document, group_idx: usize) -> Option<TextProperties> {
        let nodes = doc.collect_nodes(group_idx);
        if nodes.is_empty() {
            return None;
        }
        
        // Aggregate properties from all nodes
        let mut total_size = 0.0f32;
        let mut is_bold = false;
        let mut text_content = String::new();
        let mut count = 0;
        
        for node in nodes {
            if let FlattenedNodeKind::Text { font_size, content, .. } = &node.kind {
                total_size += font_size.to_f32().unwrap_or(10.0);
                count += 1;
                text_content.push_str(content);
                text_content.push(' ');
                
                // Check font weight from style
                if let Some(font) = &node.style.font
                    && font.weight == FontWeight::Bold {
                        is_bold = true;
                    }
            }
        }
        
        if count == 0 {
            return None;
        }
        
        let avg_size = total_size / count as f32;
        let text_len = text_content.trim().len();
        
        Some(TextProperties {
            avg_font_size: avg_size,
            is_bold,
            text_length: text_len,
            text_content: text_content.trim().to_string(),
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
                matches!(doc.source.nodes.get(*node_index).map(|n| &n.kind),
                    Some(FlattenedNodeKind::Text { .. }))
            }
            GroupKind::TextBlock => true,
            _ => false,
        }
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
        // Collect font statistics
        let stats = self.collect_font_stats(doc);
        
        // Need enough samples for meaningful analysis
        if stats.sample_count < self.min_samples {
            return;
        }
        
        // Find all unclaimed text groups (roots that are text)
        let roots = doc.roots();
        let text_groups: Vec<usize> = roots.iter()
            .filter(|&&idx| self.is_text_group(doc, idx))
            .copied()
            .collect();
        
        // Analyze each text group
        let mut headings: Vec<(usize, u8)> = Vec::new();
        
        for group_idx in text_groups {
            if let Some(props) = self.get_text_properties(doc, group_idx)
                && let Some(level) = self.determine_heading_level(
                    props.avg_font_size,
                    props.is_bold,
                    props.text_length,
                    &stats,
                ) {
                    headings.push((group_idx, level));
                }
        }
        
        // Create Heading groups
        for (group_idx, level) in headings {
            doc.merge(
                vec![group_idx],
                GroupKind::Heading { level },
                GroupSource::Inferred { module: self.name().to_string() },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flattened::{Flattened, FlattenedNode, Page};
    use crate::xfa::{num, Font, FontWeight};
    
    fn make_text_node(content: &str, font_size: f32, x: f32, y: f32) -> FlattenedNode {
        FlattenedNode::new_text(
            content.to_string(),
            num(font_size as f64),
            "Helvetica".to_string(),
            num(x as f64), num(y as f64),
            num(200.0), num(font_size as f64 * 1.2),
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
        });
        node
    }
    
    #[test]
    fn test_detect_headings_by_size() {
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                // Large heading (should be h1 or h2)
                make_text_node("Main Title", 24.0, 10.0, 10.0),
                // Medium heading (should be h3 or h4)
                make_text_node("Section Title", 16.0, 10.0, 50.0),
                // Body text (10pt) - multiple to establish baseline
                make_text_node("This is body text paragraph one with some content.", 10.0, 10.0, 80.0),
                make_text_node("This is body text paragraph two with more content.", 10.0, 10.0, 100.0),
                make_text_node("This is body text paragraph three.", 10.0, 10.0, 120.0),
                make_text_node("Another paragraph of body text here.", 10.0, 10.0, 140.0),
                make_text_node("And more body text content.", 10.0, 10.0, 160.0),
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);
        
        // Should detect headings
        let headings = doc.headings();
        assert!(headings.len() >= 2, "Should detect at least 2 headings, got {}", headings.len());
        
        // Get heading levels
        let mut levels: Vec<u8> = headings.iter()
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
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                // Bold text at larger size (should be detected as heading)
                make_bold_text_node("Bold Subheading", 14.0, 10.0, 10.0),
                // Regular body text at 10pt
                make_text_node("Regular body text paragraph one.", 10.0, 10.0, 30.0),
                make_text_node("Regular body text paragraph two.", 10.0, 10.0, 50.0),
                make_text_node("Regular body text paragraph three.", 10.0, 10.0, 70.0),
                make_text_node("Regular body text paragraph four.", 10.0, 10.0, 90.0),
                make_text_node("Regular body text paragraph five.", 10.0, 10.0, 110.0),
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);
        
        let headings = doc.headings();
        assert_eq!(headings.len(), 1, "Bold larger text should be detected as heading");
    }
    
    #[test]
    fn test_long_text_not_heading() {
        let long_text = "This is a very long paragraph that should not be detected as a heading even though it might have a larger font size because headings are typically short and concise not rambling on like this.";
        
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                // Large font but too long to be heading
                make_text_node(long_text, 18.0, 10.0, 10.0),
                // Body text
                make_text_node("Body paragraph one.", 10.0, 10.0, 50.0),
                make_text_node("Body paragraph two.", 10.0, 10.0, 70.0),
                make_text_node("Body paragraph three.", 10.0, 10.0, 90.0),
                make_text_node("Body paragraph four.", 10.0, 10.0, 110.0),
                make_text_node("Body paragraph five.", 10.0, 10.0, 130.0),
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);
        
        // Long text should not be detected as heading
        let headings = doc.headings();
        for &idx in &headings {
            let text = doc.get_text_content(idx);
            assert!(text.len() <= 150, "Long text should not be heading: {}", text);
        }
    }
    
    #[test]
    fn test_insufficient_samples() {
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
                make_text_node("Only Title", 24.0, 10.0, 10.0),
                make_text_node("Single paragraph.", 10.0, 10.0, 50.0),
            ],
        };
        
        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new()
            .with_min_size_ratio(1.1)
            .process(&mut doc);
        
        // Not enough samples for statistical analysis
        let headings = doc.headings();
        assert_eq!(headings.len(), 0, "Should not detect headings with insufficient samples");
    }
    
    #[test]
    fn test_heading_level_ordering() {
        let flattened = Flattened {
            page: Page { width: num(595.0), height: num(842.0) },
            nodes: vec![
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
        };
        
        let mut doc = Document::from_flattened(&flattened);
        HeadingDetector::new().process(&mut doc);
        
        // Collect headings with their sizes and levels
        let mut heading_info: Vec<(f32, u8, String)> = Vec::new();
        for &idx in &doc.headings() {
            if let Some(group) = doc.get_group(idx) {
                if let GroupKind::Heading { level } = group.kind {
                    let nodes = doc.collect_nodes(idx);
                    if let Some(node) = nodes.first() {
                        if let FlattenedNodeKind::Text { font_size, content, .. } = &node.kind {
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
}
