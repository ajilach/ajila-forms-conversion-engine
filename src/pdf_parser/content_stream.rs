//! Content stream parser for extracting positioned text from PDF pages.
//!
//! Interprets PDF page content stream operators to extract text runs with
//! their absolute positions, font information, and decoded Unicode content.

use super::font_decoder::{FontEntry, FontMap, build_font_map};
use lopdf::{Document, Object, ObjectId};
use lopdf::content::Content;

/// A single run of text extracted from the PDF content stream,
/// with its absolute position and font information.
#[derive(Debug, Clone)]
pub struct TextRun {
    /// Decoded Unicode text content.
    pub text: String,
    /// Absolute X position in PDF user-space points (from page origin).
    pub x: f64,
    /// Absolute Y position in PDF user-space points (from page origin).
    pub y: f64,
    /// Font size in points.
    pub font_size: f64,
    /// Font name (the base font name, e.g. "Helvetica", "ArialMT").
    pub font_name: String,
    /// Approximate width of this text run in user-space points.
    pub width: f64,
    /// Approximate height (based on font size).
    pub height: f64,
}

/// Graphics state for tracking text positioning.
#[derive(Debug, Clone)]
struct GraphicsState {
    /// Current transformation matrix [a, b, c, d, e, f].
    ctm: [f64; 6],
    /// Text state parameters
    text_state: TextState,
}

#[derive(Debug, Clone)]
struct TextState {
    /// Character spacing (Tc)
    char_spacing: f64,
    /// Word spacing (Tw)
    word_spacing: f64,
    /// Horizontal scaling as a fraction (Tz / 100)
    h_scaling: f64,
    /// Text leading (TL)
    leading: f64,
    /// Current font name (resource key, e.g. "F1")
    font_key: String,
    /// Current font size
    font_size: f64,
    /// Text rise (Ts)
    text_rise: f64,
}

impl Default for TextState {
    fn default() -> Self {
        TextState {
            char_spacing: 0.0,
            word_spacing: 0.0,
            h_scaling: 1.0,
            leading: 0.0,
            font_key: String::new(),
            font_size: 12.0,
            text_rise: 0.0,
        }
    }
}

impl Default for GraphicsState {
    fn default() -> Self {
        GraphicsState {
            ctm: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            text_state: TextState::default(),
        }
    }
}

/// Text matrix state during a BT..ET block.
#[derive(Debug, Clone)]
struct TextMatrixState {
    /// Text matrix [a, b, c, d, e, f]
    tm: [f64; 6],
    /// Text line matrix (set by Td/TD/T*, used for T*)
    tlm: [f64; 6],
}

impl Default for TextMatrixState {
    fn default() -> Self {
        let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        TextMatrixState {
            tm: identity,
            tlm: identity,
        }
    }
}

/// Multiply two 3×3 matrices (stored as [a, b, c, d, e, f] for the first two rows;
/// the third row is implicitly [0, 0, 1]).
fn mat_mul(a: &[f64; 6], b: &[f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// Transform a point (x, y) by a matrix.
fn transform_point(m: &[f64; 6], x: f64, y: f64) -> (f64, f64) {
    (
        m[0] * x + m[2] * y + m[4],
        m[1] * x + m[3] * y + m[5],
    )
}

/// Get the effective font size after applying the text matrix and CTM.
fn effective_font_size(text_state: &TextState, tm: &[f64; 6], ctm: &[f64; 6]) -> f64 {
    // The effective rendering matrix is: Trm = [fontSize 0 0 fontSize 0 Trise] × Tm × CTM
    // We only need the vertical scaling component for the effective size.
    let combined = mat_mul(tm, ctm);
    // The font size in user space is font_size × |combined_y_scale|
    let y_scale = (combined[2] * combined[2] + combined[3] * combined[3]).sqrt();
    text_state.font_size * y_scale
}

/// Extract all text runs from a single PDF page.
pub fn extract_text_runs(
    doc: &Document,
    page_id: ObjectId,
    page_height: f64,
) -> Vec<TextRun> {
    let font_map = build_font_map(doc, page_id);

    // Get page content stream bytes
    let content_bytes = match doc.get_page_content(page_id) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(),
    };

    // Parse content stream operations
    let operations = match Content::decode(&content_bytes) {
        Ok(content) => content.operations,
        Err(_) => return Vec::new(),
    };

    let mut runs = Vec::new();
    let mut gs_stack: Vec<GraphicsState> = Vec::new();
    let mut gs = GraphicsState::default();
    let mut text_matrix: Option<TextMatrixState> = None; // None when outside BT..ET

    for op in &operations {
        match op.operator.as_ref() {
            // ==== Graphics state ====
            "q" => {
                gs_stack.push(gs.clone());
            }
            "Q" => {
                if let Some(restored) = gs_stack.pop() {
                    gs = restored;
                }
            }
            "cm" => {
                if let Some(m) = parse_matrix(&op.operands) {
                    gs.ctm = mat_mul(&m, &gs.ctm);
                }
            }

            // ==== Text state ====
            "Tc" => {
                gs.text_state.char_spacing = get_number(&op.operands, 0).unwrap_or(0.0);
            }
            "Tw" => {
                gs.text_state.word_spacing = get_number(&op.operands, 0).unwrap_or(0.0);
            }
            "Tz" => {
                let pct = get_number(&op.operands, 0).unwrap_or(100.0);
                gs.text_state.h_scaling = pct / 100.0;
            }
            "TL" => {
                gs.text_state.leading = get_number(&op.operands, 0).unwrap_or(0.0);
            }
            "Tf" => {
                if let Some(name) = get_name(&op.operands, 0) {
                    gs.text_state.font_key = name;
                }
                gs.text_state.font_size = get_number(&op.operands, 1).unwrap_or(12.0);
            }
            "Ts" => {
                gs.text_state.text_rise = get_number(&op.operands, 0).unwrap_or(0.0);
            }
            "Tr" => {
                // Text rendering mode — we don't need this for extraction
            }

            // ==== Text object ====
            "BT" => {
                text_matrix = Some(TextMatrixState::default());
            }
            "ET" => {
                text_matrix = None;
            }

            // ==== Text positioning ====
            "Td" => {
                if let Some(ref mut tms) = text_matrix {
                    let tx = get_number(&op.operands, 0).unwrap_or(0.0);
                    let ty = get_number(&op.operands, 1).unwrap_or(0.0);
                    let translate = [1.0, 0.0, 0.0, 1.0, tx, ty];
                    tms.tlm = mat_mul(&translate, &tms.tlm);
                    tms.tm = tms.tlm;
                }
            }
            "TD" => {
                if let Some(ref mut tms) = text_matrix {
                    let tx = get_number(&op.operands, 0).unwrap_or(0.0);
                    let ty = get_number(&op.operands, 1).unwrap_or(0.0);
                    gs.text_state.leading = -ty;
                    let translate = [1.0, 0.0, 0.0, 1.0, tx, ty];
                    tms.tlm = mat_mul(&translate, &tms.tlm);
                    tms.tm = tms.tlm;
                }
            }
            "Tm" => {
                if let Some(ref mut tms) = text_matrix {
                    if let Some(m) = parse_matrix(&op.operands) {
                        tms.tm = m;
                        tms.tlm = m;
                    }
                }
            }
            "T*" => {
                if let Some(ref mut tms) = text_matrix {
                    let leading = gs.text_state.leading;
                    let translate = [1.0, 0.0, 0.0, 1.0, 0.0, -leading];
                    tms.tlm = mat_mul(&translate, &tms.tlm);
                    tms.tm = tms.tlm;
                }
            }

            // ==== Text showing ====
            "Tj" => {
                if let Some(ref mut tms) = text_matrix {
                    if let Some(bytes) = get_string_bytes(&op.operands, 0) {
                        let run = show_text(
                            &bytes,
                            &gs,
                            tms,
                            &font_map,
                            page_height,
                        );
                        if let Some(r) = run {
                            runs.push(r);
                        }
                    }
                }
            }
            "TJ" => {
                if let Some(ref mut tms) = text_matrix {
                    if let Some(Object::Array(array)) = op.operands.first() {
                        show_text_array(
                            array,
                            &gs,
                            tms,
                            &font_map,
                            page_height,
                            &mut runs,
                        );
                    }
                }
            }
            "'" => {
                // Move to next line and show text: T*; string Tj
                if let Some(ref mut tms) = text_matrix {
                    let leading = gs.text_state.leading;
                    let translate = [1.0, 0.0, 0.0, 1.0, 0.0, -leading];
                    tms.tlm = mat_mul(&translate, &tms.tlm);
                    tms.tm = tms.tlm;

                    if let Some(bytes) = get_string_bytes(&op.operands, 0) {
                        let run = show_text(
                            &bytes,
                            &gs,
                            tms,
                            &font_map,
                            page_height,
                        );
                        if let Some(r) = run {
                            runs.push(r);
                        }
                    }
                }
            }
            "\"" => {
                // Set word/char spacing, move to next line, show text
                if let Some(ref mut tms) = text_matrix {
                    gs.text_state.word_spacing = get_number(&op.operands, 0).unwrap_or(0.0);
                    gs.text_state.char_spacing = get_number(&op.operands, 1).unwrap_or(0.0);

                    let leading = gs.text_state.leading;
                    let translate = [1.0, 0.0, 0.0, 1.0, 0.0, -leading];
                    tms.tlm = mat_mul(&translate, &tms.tlm);
                    tms.tm = tms.tlm;

                    if let Some(bytes) = get_string_bytes(&op.operands, 2) {
                        let run = show_text(
                            &bytes,
                            &gs,
                            tms,
                            &font_map,
                            page_height,
                        );
                        if let Some(r) = run {
                            runs.push(r);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    runs
}

/// Show a single text string and return a TextRun.
/// Updates the text matrix to advance past the shown text.
fn show_text(
    bytes: &[u8],
    gs: &GraphicsState,
    tms: &mut TextMatrixState,
    font_map: &FontMap,
    page_height: f64,
) -> Option<TextRun> {
    let font_entry = font_map.get(&gs.text_state.font_key)?;
    let decoded = font_entry.decode_bytes(bytes);

    if decoded.is_empty() || decoded.chars().all(|c| c == '\u{FFFD}') {
        return None;
    }

    // Record the starting position
    let start_tm = tms.tm;
    let (abs_x, abs_y) = transform_point(&mat_mul(&start_tm, &gs.ctm), 0.0, 0.0);

    // Compute total text width and advance the text matrix
    let width = advance_text_matrix(bytes, &gs.text_state, font_entry, tms);

    let eff_size = effective_font_size(&gs.text_state, &start_tm, &gs.ctm);

    // Convert PDF coordinates (origin at bottom-left) to our coordinate system (origin at top-left)
    let y_top = page_height - abs_y;

    Some(TextRun {
        text: decoded,
        x: abs_x,
        y: y_top - eff_size, // Adjust so y is the top of the text
        font_size: eff_size,
        font_name: font_entry.base_font.clone(),
        width: width * gs.text_state.h_scaling,
        height: eff_size,
    })
}

/// Process a TJ array: alternating strings and numeric adjustments.
fn show_text_array(
    array: &[Object],
    gs: &GraphicsState,
    tms: &mut TextMatrixState,
    font_map: &FontMap,
    page_height: f64,
    runs: &mut Vec<TextRun>,
) {
    let font_entry = match font_map.get(&gs.text_state.font_key) {
        Some(fe) => fe,
        None => return,
    };

    // Collect the entire TJ array into a single text run for better readability
    let mut full_text = String::new();
    let start_tm = tms.tm;
    let (abs_x, abs_y) = transform_point(&mat_mul(&start_tm, &gs.ctm), 0.0, 0.0);
    let mut total_width: f64 = 0.0;

    for item in array {
        match item {
            Object::String(bytes, _) => {
                let decoded = font_entry.decode_bytes(bytes);
                full_text.push_str(&decoded);
                total_width += advance_text_matrix(bytes, &gs.text_state, font_entry, tms);
            }
            Object::Integer(n) => {
                // Negative = move right (add space), Positive = move left (kern)
                let adjustment = -*n as f64 / 1000.0 * gs.text_state.font_size;
                total_width += adjustment;
                // Advance the text matrix
                let advance = [1.0, 0.0, 0.0, 1.0, adjustment * gs.text_state.h_scaling, 0.0];
                tms.tm = mat_mul(&advance, &tms.tm);

                // If the adjustment is large enough, it represents a word space
                if *n < -100 {
                    full_text.push(' ');
                }
            }
            Object::Real(f) => {
                let adjustment = -*f as f64 / 1000.0 * gs.text_state.font_size;
                total_width += adjustment;
                let advance = [1.0, 0.0, 0.0, 1.0, adjustment * gs.text_state.h_scaling, 0.0];
                tms.tm = mat_mul(&advance, &tms.tm);

                if *f < -100.0 {
                    full_text.push(' ');
                }
            }
            _ => {}
        }
    }

    if !full_text.is_empty() && !full_text.chars().all(|c| c == '\u{FFFD}') {
        let eff_size = effective_font_size(&gs.text_state, &start_tm, &gs.ctm);
        let y_top = page_height - abs_y;

        runs.push(TextRun {
            text: full_text,
            x: abs_x,
            y: y_top - eff_size,
            font_size: eff_size,
            font_name: font_entry.base_font.clone(),
            width: total_width.abs() * gs.text_state.h_scaling,
            height: eff_size,
        });
    }
}

/// Advance the text matrix by the width of the given bytes.
/// Returns the total advance width in text space units.
fn advance_text_matrix(
    bytes: &[u8],
    text_state: &TextState,
    font_entry: &FontEntry,
    tms: &mut TextMatrixState,
) -> f64 {
    let mut total_advance = 0.0;
    let font_size = text_state.font_size;

    // Determine if this is a 2-byte font
    let is_two_byte = font_entry.is_two_byte_public();

    if is_two_byte {
        let mut i = 0;
        while i + 1 < bytes.len() {
            let code = u32::from(bytes[i]) << 8 | u32::from(bytes[i + 1]);
            let w = font_entry.char_width(code) / 1000.0 * font_size;
            let advance = w + text_state.char_spacing;
            total_advance += advance;
            i += 2;
        }
    } else {
        for &b in bytes {
            let code = u32::from(b);
            let w = font_entry.char_width(code) / 1000.0 * font_size;
            let mut advance = w + text_state.char_spacing;
            // Add word spacing for space characters (code 32)
            if b == 0x20 {
                advance += text_state.word_spacing;
            }
            total_advance += advance;
        }
    }

    // Advance the text matrix
    let tx = total_advance * text_state.h_scaling;
    let advance_matrix = [1.0, 0.0, 0.0, 1.0, tx, 0.0];
    tms.tm = mat_mul(&advance_matrix, &tms.tm);

    total_advance
}

// ============================================================================
// Operand helpers
// ============================================================================

fn get_number(operands: &[Object], index: usize) -> Option<f64> {
    operands.get(index).and_then(|o| match o {
        Object::Integer(n) => Some(*n as f64),
        Object::Real(f) => Some(*f as f64),
        _ => None,
    })
}

fn get_name(operands: &[Object], index: usize) -> Option<String> {
    operands.get(index).and_then(|o| match o {
        Object::Name(n) => Some(String::from_utf8_lossy(n).to_string()),
        _ => None,
    })
}

fn get_string_bytes(operands: &[Object], index: usize) -> Option<Vec<u8>> {
    operands.get(index).and_then(|o| match o {
        Object::String(bytes, _) => Some(bytes.clone()),
        _ => None,
    })
}

fn parse_matrix(operands: &[Object]) -> Option<[f64; 6]> {
    if operands.len() < 6 {
        return None;
    }
    Some([
        get_number(operands, 0)?,
        get_number(operands, 1)?,
        get_number(operands, 2)?,
        get_number(operands, 3)?,
        get_number(operands, 4)?,
        get_number(operands, 5)?,
    ])
}
