//! Text Metrics Module - XFA AXTE-Compliant Text Measurement
//! 
//! This module implements text measurement according to the XFA Specification
//! (specifically the AXTE Line Positioning appendix, pages 1521-1532).
//!
//! Key AXTE rules implemented:
//! - Line gap is 20% of font size: `sLG = sFS * 0.2`
//! - If (ascent + descent) < font_size, pad ascent: `sA = sFS - sD`
//! - Text height: `TH = A + D` (accumulated ascent + descent)
//! - Derived line spacing: `DS = TH + LG` (or use override if provided)
//! - Full height: `FH = MT + DS + MB` (with line gap removed on last line)
//! - Baseline: `B = MT + TH - D`

use crate::xfa::{Font, Para, Num, num};
use ab_glyph::{FontRef, Font as AbGlyphFont, ScaleFont, PxScale};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::collections::HashMap;

/// Font metrics extracted from a font at a specific size
#[derive(Debug, Clone)]
pub struct FontMetrics {
    /// Font size in points
    pub font_size: Num,
    /// Ascent: height above baseline (positive)
    pub ascent: Num,
    /// Descent: depth below baseline (positive, even though it goes downward)
    pub descent: Num,
    /// Line gap: extra space between lines (per AXTE: always 20% of font size)
    pub line_gap: Num,
    /// Em width for character width estimation
    pub em_width: Num,
    /// Average character width
    pub avg_char_width: Num,
}

impl FontMetrics {
    /// Create font metrics from an ab_glyph font at a given size
    /// Per AXTE spec: line gap is always 20% of font size
    pub fn from_font(font: &FontRef<'_>, font_size_pt: Num) -> Self {
        let size_f32 = font_size_pt.to_f32().unwrap_or(10.0);
        let scale = PxScale::from(size_f32);
        let scaled_font = font.as_scaled(scale);
        
        // Get metrics from font (these are in pixels at the given scale)
        let ascent_px = scaled_font.ascent();
        let descent_px = scaled_font.descent().abs(); // descent is often negative
        let line_gap_from_font = scaled_font.line_gap();
        
        // Convert to Decimal
        let mut ascent = num(ascent_px as f64);
        let descent = num(descent_px as f64);
        
        // Per AXTE spec: If (ascent + descent) < font_size, pad the ascent
        // "AXTE accommodates a combined ascent and descent larger than the font size.
        //  If the sum of the two is less than the font size, it pads the ascent,
        //  so that the requested font size is always consumed."
        if ascent + descent < font_size_pt {
            ascent = font_size_pt - descent;
        }
        
        // Per AXTE spec: "AXTE adopts the convention embraced by other Adobe applications
        // that line gap is always determined to be 20% of font size."
        // sLG = sFS * 0.2
        let line_gap = font_size_pt * num(0.2);
        
        // Estimate character width using 'M' as em-width
        let glyph_id = font.glyph_id('M');
        let em_width = if glyph_id.0 != 0 {
            let advance = scaled_font.h_advance(glyph_id);
            num(advance as f64)
        } else {
            font_size_pt * num(0.6) // Fallback: 60% of font size
        };
        
        // Average character width (using 'x' for x-height reference, or 'e' as common char)
        let glyph_id_e = font.glyph_id('e');
        let avg_char_width = if glyph_id_e.0 != 0 {
            let advance = scaled_font.h_advance(glyph_id_e);
            num(advance as f64)
        } else {
            em_width * num(0.8) // Fallback: 80% of em width
        };
        
        FontMetrics {
            font_size: font_size_pt,
            ascent,
            descent,
            line_gap,
            em_width,
            avg_char_width,
        }
    }
    
    /// Get text height (ascent + descent)
    /// Per AXTE: TH = A + D
    pub fn text_height(&self) -> Num {
        self.ascent + self.descent
    }
    
    /// Get derived line spacing (text height + line gap)
    /// Per AXTE: DS = TH + LG
    pub fn derived_line_spacing(&self) -> Num {
        self.text_height() + self.line_gap
    }
    
    /// Calculate full height for a single line (no margins)
    /// Per AXTE for a single line: FH = TH + LG (but LG removed on last line)
    pub fn single_line_height(&self, is_last_line: bool) -> Num {
        if is_last_line {
            self.text_height()
        } else {
            self.derived_line_spacing()
        }
    }
    
    /// Get baseline position from top of line
    /// Per AXTE: B = MT + TH - D (when no spacing override)
    pub fn baseline_from_top(&self, margin_top: Num) -> Num {
        margin_top + self.text_height() - self.descent
    }
}

/// A single line of text with its metrics
#[derive(Debug, Clone)]
pub struct TextLine {
    /// The text content of this line
    pub text: String,
    /// Width of this line in points
    pub width: Num,
    /// Accumulated metrics for this line
    pub metrics: LineMetrics,
}

/// Accumulated metrics for a line (may have multiple spans)
#[derive(Debug, Clone)]
pub struct LineMetrics {
    /// Maximum ascent across all spans in this line
    pub ascent: Num,
    /// Maximum descent across all spans in this line
    pub descent: Num,
    /// Maximum line gap across all spans
    pub line_gap: Num,
    /// Line spacing override (from para element), if any
    pub spacing_override: Option<Num>,
    /// Is this the first line in the text block?
    pub is_first_line: bool,
    /// Is this the last line in the text block?
    pub is_last_line: bool,
    /// Top margin (from paragraph)
    pub margin_top: Num,
    /// Bottom margin (from paragraph)
    pub margin_bottom: Num,
}

impl LineMetrics {
    /// Create new line metrics
    pub fn new(is_first_line: bool, is_last_line: bool) -> Self {
        LineMetrics {
            ascent: Decimal::ZERO,
            descent: Decimal::ZERO,
            line_gap: Decimal::ZERO,
            spacing_override: None,
            is_first_line,
            is_last_line,
            margin_top: Decimal::ZERO,
            margin_bottom: Decimal::ZERO,
        }
    }
    
    /// Accumulate font metrics into this line
    /// Per AXTE: accumulate separately, then compute derived values
    pub fn accumulate(&mut self, font_metrics: &FontMetrics) {
        self.ascent = self.ascent.max(font_metrics.ascent);
        self.descent = self.descent.max(font_metrics.descent);
        self.line_gap = self.line_gap.max(font_metrics.line_gap);
    }
    
    /// Get text height
    /// Per AXTE: TH = A + D
    pub fn text_height(&self) -> Num {
        self.ascent + self.descent
    }
    
    /// Calculate derived line spacing
    /// Per AXTE: If line spacing override is set and > font metrics, use it.
    /// Otherwise: DS = TH + LG
    /// 
    /// Special case: On first line in block, if spacing override > default spacing,
    /// the extra space is ignored for consistency with other text processors.
    pub fn derived_spacing(&self) -> Num {
        let default_spacing = self.text_height() + self.line_gap;
        
        if let Some(sp) = self.spacing_override {
            if sp > Decimal::ZERO {
                // Per AXTE: "If there is a line spacing override and it is larger than 
                // the default line spacing, the extra space is ignored on the first line 
                // in a text block, for consistency with other text processing applications."
                if self.is_first_line && sp > default_spacing {
                    // For first line, cap at default spacing if override is larger
                    return default_spacing;
                }
                return sp;
            }
        }
        
        default_spacing
    }
    
    /// Calculate full height of this line
    /// Per AXTE: FH = MT + DS + MB
    /// With special handling: line gap removed on last line in block
    pub fn full_height(&self) -> Num {
        let mut fh = self.margin_top + self.derived_spacing() + self.margin_bottom;
        
        // Per AXTE: "AXTE removes the line gap on the last line in a block 
        // so that bottom-aligned text doesn't appear shifted up."
        if self.is_last_line {
            fh = fh - self.line_gap;
        }
        
        fh
    }
    
    /// Get baseline position from top of line
    /// Per AXTE:
    /// - When no line spacing override: B = MT + TH - D
    /// - When spacing override > TH: B = MT + SP - LG - D
    pub fn baseline_from_top(&self) -> Num {
        let th = self.text_height();
        
        if let Some(sp) = self.spacing_override {
            if sp > Decimal::ZERO && sp - self.line_gap >= th {
                // Spacing override in effect and larger than text height
                return self.margin_top + sp - self.line_gap - self.descent;
            }
        }
        
        // Default formula
        self.margin_top + th - self.descent
    }
}

/// Text measurement result for a complete text block
#[derive(Debug, Clone)]
pub struct TextBlockMetrics {
    /// All lines with their metrics
    pub lines: Vec<TextLine>,
    /// Total width (width of longest line)
    pub total_width: Num,
    /// Total height (sum of all line heights)
    pub total_height: Num,
}

impl TextBlockMetrics {
    /// Get the y-offset for the first line based on vertical alignment
    /// Per AXTE: Block-level first line offset calculation
    pub fn first_line_offset(&self, block_height: Num, v_align: crate::xfa::VAlign) -> Num {
        let total_height = self.total_height;
        
        // Per AXTE: "Under certain circumstances, AXTE may store lines whose total height 
        // is greater than the block height. In such a case, the block is treated as being top-aligned."
        if total_height > block_height {
            return Decimal::ZERO;
        }
        
        match v_align {
            crate::xfa::VAlign::Top => Decimal::ZERO,
            crate::xfa::VAlign::Middle => (block_height - total_height) / num(2.0),
            crate::xfa::VAlign::Bottom => block_height - total_height,
        }
    }
}

/// Text measurement engine with font caching
pub struct TextMeasurer {
    /// Cached font data (static lifetime for ab_glyph)
    font_data: Option<&'static [u8]>,
    /// Cached font reference
    font: Option<FontRef<'static>>,
    /// Cached font metrics by size
    metrics_cache: HashMap<u32, FontMetrics>,
}

impl TextMeasurer {
    /// Create a new text measurer
    pub fn new() -> Self {
        TextMeasurer {
            font_data: None,
            font: None,
            metrics_cache: HashMap::new(),
        }
    }
    
    /// Load a system font
    pub fn load_font(&mut self) -> Result<(), String> {
        if self.font.is_some() {
            return Ok(());
        }
        
        // Try common system font locations
        let font_paths = [
            "/System/Library/Fonts/Helvetica.ttc",           // macOS
            "/System/Library/Fonts/Supplemental/Arial.ttf",   // macOS
            "/Library/Fonts/Arial.ttf",                       // macOS
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", // Linux
            "/usr/share/fonts/TTF/DejaVuSans.ttf",            // Linux
            "C:\\Windows\\Fonts\\arial.ttf",                  // Windows
        ];
        
        for font_path in &font_paths {
            if let Ok(font_data) = std::fs::read(font_path) {
                // Leak the font data so it has 'static lifetime
                let static_data: &'static [u8] = Box::leak(font_data.into_boxed_slice());
                self.font_data = Some(static_data);
                
                if let Ok(font) = FontRef::try_from_slice(static_data) {
                    self.font = Some(font);
                    return Ok(());
                }
            }
        }
        
        Err("Could not load any system font".to_string())
    }
    
    /// Get font metrics for a given font size (with caching)
    pub fn get_metrics(&mut self, font_size: Num) -> Result<FontMetrics, String> {
        self.load_font()?;
        
        let font = self.font.as_ref().ok_or("Font not loaded")?;
        
        // Use font size in hundredths of a point as cache key
        let cache_key = (font_size * num(100.0)).to_u32().unwrap_or(1000);
        
        if let Some(metrics) = self.metrics_cache.get(&cache_key) {
            return Ok(metrics.clone());
        }
        
        let metrics = FontMetrics::from_font(font, font_size);
        self.metrics_cache.insert(cache_key, metrics.clone());
        
        Ok(metrics)
    }
    
    /// Measure text width
    pub fn measure_text_width(&mut self, text: &str, font_size: Num) -> Result<Num, String> {
        self.load_font()?;
        
        let font = self.font.as_ref().ok_or("Font not loaded")?;
        let size_f32 = font_size.to_f32().unwrap_or(10.0);
        let scale = PxScale::from(size_f32);
        let scaled_font = font.as_scaled(scale);
        
        let mut width: f32 = 0.0;
        for ch in text.chars() {
            let glyph_id = font.glyph_id(ch);
            if glyph_id.0 != 0 {
                width += scaled_font.h_advance(glyph_id);
            } else {
                // Fallback for missing glyphs
                width += size_f32 * 0.6;
            }
        }
        
        Ok(num(width as f64))
    }
    
    /// Wrap text to fit within a maximum width
    /// Returns a vector of lines
    pub fn wrap_text(&mut self, text: &str, max_width: Num, font_size: Num) -> Result<Vec<String>, String> {
        self.load_font()?;
        
        let font = self.font.as_ref().ok_or("Font not loaded")?;
        let size_f32 = font_size.to_f32().unwrap_or(10.0);
        let scale = PxScale::from(size_f32);
        let scaled_font = font.as_scaled(scale);
        let max_width_f32 = max_width.to_f32().unwrap_or(1000.0);
        
        let mut lines = Vec::new();
        let mut current_line = String::new();
        let mut current_width: f32 = 0.0;
        let space_glyph = font.glyph_id(' ');
        let space_width = if space_glyph.0 != 0 {
            scaled_font.h_advance(space_glyph)
        } else {
            size_f32 * 0.3
        };
        
        for word in text.split_whitespace() {
            // Measure word width
            let mut word_width: f32 = 0.0;
            for ch in word.chars() {
                let glyph_id = font.glyph_id(ch);
                if glyph_id.0 != 0 {
                    word_width += scaled_font.h_advance(glyph_id);
                } else {
                    word_width += size_f32 * 0.6;
                }
            }
            
            if current_line.is_empty() {
                // First word on line
                current_line = word.to_string();
                current_width = word_width;
            } else if current_width + space_width + word_width <= max_width_f32 {
                // Word fits on current line
                current_line.push(' ');
                current_line.push_str(word);
                current_width += space_width + word_width;
            } else {
                // Word doesn't fit, start new line
                lines.push(current_line);
                current_line = word.to_string();
                current_width = word_width;
            }
        }
        
        if !current_line.is_empty() {
            lines.push(current_line);
        }
        
        if lines.is_empty() {
            lines.push(String::new());
        }
        
        Ok(lines)
    }
    
    /// Measure a text block with wrapping and full metrics
    /// This is the main entry point for text sizing
    pub fn measure_text_block(
        &mut self,
        text: &str,
        font: &Option<Font>,
        para: &Option<Para>,
        max_width: Num,
    ) -> Result<TextBlockMetrics, String> {
        // Get font size from style or use default
        let font_size = font.as_ref()
            .map(|f| f.size)
            .unwrap_or_else(|| num(10.0));
        
        // Get paragraph margins
        let margin_top = para.as_ref()
            .and_then(|p| p.space_above)
            .unwrap_or(Decimal::ZERO);
        let margin_bottom = para.as_ref()
            .and_then(|p| p.space_below)
            .unwrap_or(Decimal::ZERO);
        let line_height_override = para.as_ref()
            .and_then(|p| p.line_height);
        
        // Wrap text
        let wrapped_lines = self.wrap_text(text, max_width, font_size)?;
        let num_lines = wrapped_lines.len();
        
        // Get base font metrics
        let base_metrics = self.get_metrics(font_size)?;
        
        // Build text lines with metrics
        let mut lines = Vec::new();
        let mut total_height = Decimal::ZERO;
        let mut max_width = Decimal::ZERO;
        
        for (i, line_text) in wrapped_lines.into_iter().enumerate() {
            let is_first = i == 0;
            let is_last = i == num_lines - 1;
            
            // Measure line width
            let line_width = self.measure_text_width(&line_text, font_size)?;
            max_width = max_width.max(line_width);
            
            // Create line metrics
            let mut line_metrics = LineMetrics::new(is_first, is_last);
            line_metrics.accumulate(&base_metrics);
            line_metrics.spacing_override = line_height_override;
            
            // Apply paragraph margins only to first and last lines
            if is_first {
                line_metrics.margin_top = margin_top;
            }
            if is_last {
                line_metrics.margin_bottom = margin_bottom;
            }
            
            // Accumulate total height
            total_height = total_height + line_metrics.full_height();
            
            lines.push(TextLine {
                text: line_text,
                width: line_width,
                metrics: line_metrics,
            });
        }
        
        Ok(TextBlockMetrics {
            lines,
            total_width: max_width,
            total_height,
        })
    }
    
    /// Calculate the natural size of a text draw element
    /// Returns (width, height) in points
    pub fn calculate_draw_size(
        &mut self,
        text: &str,
        font: &Option<Font>,
        para: &Option<Para>,
        explicit_width: Option<Num>,
        explicit_height: Option<Num>,
    ) -> Result<(Num, Num), String> {
        // If both dimensions are explicit, return them
        if let (Some(w), Some(h)) = (explicit_width, explicit_height) {
            return Ok((w, h));
        }
        
        let font_size = font.as_ref()
            .map(|f| f.size)
            .unwrap_or_else(|| num(10.0));
        
        // Use explicit width if provided, otherwise measure text for single-line width
        let max_width = explicit_width.unwrap_or_else(|| num(10000.0)); // Large number for single-line
        
        let metrics = self.measure_text_block(text, font, para, max_width)?;
        
        let width = explicit_width.unwrap_or(metrics.total_width);
        let height = explicit_height.unwrap_or(metrics.total_height);
        
        Ok((width, height))
    }
}

impl Default for TextMeasurer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_line_gap_is_20_percent() {
        // Per AXTE spec: line gap is always 20% of font size
        let mut measurer = TextMeasurer::new();
        if measurer.load_font().is_ok() {
            let metrics = measurer.get_metrics(num(10.0)).unwrap();
            assert_eq!(metrics.line_gap, num(2.0)); // 20% of 10pt = 2pt
            
            let metrics = measurer.get_metrics(num(12.0)).unwrap();
            assert_eq!(metrics.line_gap, num(2.4)); // 20% of 12pt = 2.4pt
        }
    }
    
    #[test]
    fn test_text_height() {
        let mut measurer = TextMeasurer::new();
        if measurer.load_font().is_ok() {
            let metrics = measurer.get_metrics(num(10.0)).unwrap();
            // Text height should be at least font size due to ascent padding
            assert!(metrics.text_height() >= num(10.0));
        }
    }
    
    #[test]
    fn test_line_metrics_full_height() {
        let mut line_metrics = LineMetrics::new(true, true);
        line_metrics.ascent = num(8.0);
        line_metrics.descent = num(2.0);
        line_metrics.line_gap = num(2.0);
        
        // Single line (first and last): full height should be TH (no line gap at end)
        // FH = MT + DS + MB - LG (on last line)
        // = 0 + (8+2+2) + 0 - 2 = 10
        assert_eq!(line_metrics.full_height(), num(10.0));
    }
    
    #[test]
    fn test_line_metrics_baseline() {
        let mut line_metrics = LineMetrics::new(true, true);
        line_metrics.ascent = num(8.0);
        line_metrics.descent = num(2.0);
        line_metrics.margin_top = num(0.0);
        
        // B = MT + TH - D = 0 + 10 - 2 = 8
        assert_eq!(line_metrics.baseline_from_top(), num(8.0));
    }
}
