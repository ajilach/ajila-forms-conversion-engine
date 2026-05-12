//! Non-XFA (AcroForm) PDF parser module.
//!
//! Parses regular PDFs directly into the [`Flattened`] representation so the
//! entire downstream pipeline (Document → Structured → HTML/AEM/JSON) works
//! unchanged.
//!
//! # Architecture
//!
//! ```text
//! PDF bytes (lopdf)
//!   │
//!   ├── content_stream  → positioned text runs
//!   ├── acroform        → interactive form fields
//!   └── font_decoder    → glyph code → Unicode mapping
//!   │
//!   ▼
//! Vec<Flattened>  (one per page)
//! ```

pub mod acroform;
pub mod content_stream;
pub mod font_decoder;

use crate::flattened::FieldAccess;
use crate::flattened::{
    Flattened, FlattenedKind, FlattenedNode, FlattenedNodeBuilder, FlattenedNodeKind, Hint,
    MasterPageRegion, Page, RenderStyle, WidgetKind,
};
use crate::xfa::{Border, Fill, Font, FontPosture, FontWeight, GenericFamily, Num};
use acroform::{
    AcroFieldType, AcroFormField, FF_EDIT, FF_MULTI_SELECT, FF_MULTILINE, FF_PASSWORD,
    FF_PUSH_BUTTON, FF_RADIO, FF_READ_ONLY, FF_REQUIRED, extract_acroform_fields,
};
use content_stream::{TextRun, extract_page_graphics, extract_text_runs};
use lopdf::{Document, Object, ObjectId};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::prelude::ToPrimitive;
use std::collections::HashMap;

/// Parse a non-XFA PDF from in-memory bytes into a list of [`Flattened`] pages.
///
/// Each page of the PDF produces one `Flattened` instance containing:
/// - Static text extracted from the page content stream
/// - AcroForm fields (text inputs, checkboxes, radios, dropdowns, etc.)
///
/// Returns an error string if the PDF cannot be parsed.
pub fn parse_pdf(pdf_bytes: &[u8]) -> Result<Vec<Flattened>, String> {
    let doc = Document::load_mem(pdf_bytes).map_err(|e| format!("Failed to parse PDF: {}", e))?;

    // Extract and register embedded TrueType fonts before any text measurement
    register_embedded_pdf_fonts(&doc);

    let pages = doc.get_pages();
    let num_pages = pages.len();

    if num_pages == 0 {
        return Ok(Vec::new());
    }

    // Build page index mappings and page dimensions
    let mut page_ids: Vec<(u32, ObjectId)> = pages.into_iter().collect();
    page_ids.sort_by_key(|(num, _)| *num);

    let mut page_id_to_index: HashMap<ObjectId, usize> = HashMap::new();
    let mut page_heights: HashMap<usize, f64> = HashMap::new();
    let mut page_dimensions: Vec<(f64, f64)> = Vec::new(); // (width, height) per page

    for (i, (_page_num, page_id)) in page_ids.iter().enumerate() {
        page_id_to_index.insert(*page_id, i);
        let (w, h) = get_page_dimensions(&doc, *page_id);
        page_heights.insert(i, h);
        page_dimensions.push((w, h));
    }

    // Extract AcroForm fields
    let acro_fields = extract_acroform_fields(&doc, &page_heights, &page_id_to_index);

    // Group fields by page
    let mut fields_by_page: HashMap<usize, Vec<&AcroFormField>> = HashMap::new();
    for field in &acro_fields {
        let page_idx = field.page_index.unwrap_or(0);
        fields_by_page.entry(page_idx).or_default().push(field);
    }

    // Build one Flattened per page
    let mut result = Vec::with_capacity(num_pages);

    for (i, (_page_num, page_id)) in page_ids.iter().enumerate() {
        let (page_w, page_h) = page_dimensions[i];

        // Extract text runs from content stream
        let text_runs = extract_text_runs(&doc, *page_id, page_h);

        // Extract graphic elements (filled rects, lines)
        let graphics = extract_page_graphics(&doc, *page_id, page_h);

        // Collect bounding boxes of checkbox/radio fields on this page
        // so we can suppress text runs from their appearance streams.
        let button_rects: Vec<[f64; 4]> = fields_by_page
            .get(&i)
            .map(|fields| {
                fields
                    .iter()
                    .filter(|f| f.field_type == acroform::AcroFieldType::Button)
                    .filter_map(|f| f.rect)
                    .collect()
            })
            .unwrap_or_default();

        // Convert text runs to FlattenedNodes
        let mut children: Vec<FlattenedKind> = Vec::new();

        // Add filled rectangles as background nodes (before text so they render behind)
        for rect in &graphics.filled_rects {
            let node = filled_rect_to_node(rect);
            children.push(FlattenedKind::Node(node));
        }

        // Track which text runs survive the button-overlap filter so we can
        // deduplicate AcroForm-generated labels against them later.
        let mut surviving_runs: Vec<&content_stream::TextRun> = Vec::new();

        for run in &text_runs {
            if run.text.trim().is_empty() {
                continue;
            }
            // Skip text runs that overlap with checkbox/radio field rectangles.
            // These are appearance stream artifacts (e.g. "On", "Off", symbol glyphs).
            let overlaps_button = button_rects.iter().any(|[fx, fy, fw, fh]| {
                let margin = 2.0;
                run.x + run.width > fx - margin
                    && run.x < fx + fw + margin
                    && run.y + run.height > fy - margin
                    && run.y < fy + fh + margin
            });
            if overlaps_button {
                continue;
            }
            surviving_runs.push(run);
            let node = text_run_to_node(run);
            children.push(FlattenedKind::Node(node));
        }

        // Convert AcroForm fields to FlattenedNodes.
        // For checkbox/radio fields, decide whether to generate a label from the
        // field name by checking if the content stream already provides label text
        // to the right of the checkbox on the same line.
        if let Some(page_fields) = fields_by_page.get(&i) {
            for field in page_fields {
                let is_button = field.field_type == acroform::AcroFieldType::Button;
                let skip_label = if is_button {
                    let [fx, fy, fw, _fh] = field.rect.unwrap_or([0.0, 0.0, 0.0, 0.0]);
                    surviving_runs.iter().any(|r| {
                        // A label text run to the right of the checkbox, on the same line
                        r.x > fx + fw - 2.0
                            && r.x < fx + fw + 30.0
                            && (r.y - fy).abs() < 10.0
                            && !r.text.trim().is_empty()
                    })
                } else {
                    false
                };

                let nodes = acroform_field_to_nodes(field);
                for node in nodes {
                    // Skip generated label text nodes when content stream already has one
                    if skip_label {
                        if let FlattenedNodeKind::Text { ref content, .. } = node.kind {
                            if !content.is_empty() {
                                continue;
                            }
                        }
                    }
                    children.push(FlattenedKind::Node(node));
                }
            }
        }

        let page = Page::new(to_num(page_w), to_num(page_h));

        result.push(Flattened::new(page, children));
    }

    // Merge all pages into a single Flattened with header/footer detection
    let merged = merge_pages(result);
    Ok(vec![merged])
}

/// Extract embedded TrueType fonts from the PDF and register them with the
/// global [`FontManager`] so they are available for text measurement during
/// the flattening phase.
///
/// Fonts with a subset prefix (e.g. "BCDFEE+Calibri-Bold") are skipped because
/// they only contain a small subset of glyphs and would produce tofu (□)
/// characters when used for rendering. Full (non-subset) embedded fonts are
/// registered normally.
fn register_embedded_pdf_fonts(doc: &Document) {
    use crate::xfa::font_manager::{EmbeddedFont, register_embedded_font_global};
    use font_decoder::extract_embedded_fonts;

    let raw_fonts = extract_embedded_fonts(doc);
    for raw in raw_fonts {
        // Skip subset fonts — they contain only a few glyphs and corrupt rendering.
        // Subset fonts are identified by a 6-letter uppercase prefix followed by '+'.
        if is_subset_font(&raw.base_font) {
            continue;
        }

        let (clean_name, weight, posture) = parse_font_style(&raw.base_font);
        let generic_family = Some(infer_generic_family(&clean_name));

        let embedded = EmbeddedFont {
            name: clean_name,
            data: raw.data,
            weight,
            posture,
            generic_family,
        };

        // Best-effort: ignore registration errors (e.g. unparseable font data)
        let _ = register_embedded_font_global(embedded);
    }
}

/// Check if a font name has a subset prefix (e.g. "BCDFEE+Calibri-Bold").
fn is_subset_font(name: &str) -> bool {
    name.len() > 7
        && name.as_bytes()[6] == b'+'
        && name[..6].chars().all(|c| c.is_ascii_uppercase())
}

/// Convert a PDF point value to our Num (Decimal) type.
fn to_num(v: f64) -> Num {
    Decimal::from_f64(v).unwrap_or(Decimal::ZERO)
}

/// Convert a content-stream TextRun into a FlattenedNode.
fn text_run_to_node(run: &TextRun) -> FlattenedNode {
    // Determine font properties from the PDF font name
    let (clean_name, weight, posture) = parse_font_style(&run.font_name);

    let font = Font {
        typeface: clean_name.clone(),
        size: to_num(run.font_size),
        weight,
        posture,
        generic_family: Some(infer_generic_family(&clean_name)),
        ..Font::default()
    };

    let style = RenderStyle {
        font: Some(font),
        ..RenderStyle::default()
    };

    FlattenedNodeBuilder::new()
        .bounds(
            to_num(run.x),
            to_num(run.y),
            to_num(run.width),
            to_num(run.height),
        )
        .text(run.text.clone(), to_num(run.font_size), clean_name)
        .style(style)
        .no_wrap(true)
        .build()
}

/// Convert a filled rectangle from the PDF content stream into a FlattenedNode
/// with a background fill. This is used for section header backgrounds, etc.
fn filled_rect_to_node(rect: &content_stream::FilledRect) -> FlattenedNode {
    let (r, g, b) = rect.color;
    let border = Border {
        fill: Some(Fill {
            color: Some((r, g, b)),
            presence: "visible".to_string(),
        }),
        presence: "visible".to_string(),
        ..Border::default()
    };

    let style = RenderStyle {
        border: Some(border),
        ..RenderStyle::default()
    };

    // Use an empty text node as the carrier for the fill
    FlattenedNodeBuilder::new()
        .bounds(
            to_num(rect.x),
            to_num(rect.y),
            to_num(rect.width),
            to_num(rect.height),
        )
        .text(String::new(), Decimal::ZERO, String::new())
        .style(style)
        .hint(Hint::NoPrint)
        .build()
}

/// Convert an AcroForm field into one or more FlattenedNodes.
fn acroform_field_to_nodes(field: &AcroFormField) -> Vec<FlattenedNode> {
    let [x, y, w, h] = field.rect.unwrap_or([0.0, 0.0, 100.0, 20.0]);

    let widget_kind = classify_widget(field);
    let is_checked = field.is_checked;

    let mut builder =
        FlattenedNodeBuilder::new().bounds(to_num(x), to_num(y), to_num(w), to_num(h));

    // Set up as a field node
    match widget_kind {
        WidgetKind::Checkbox | WidgetKind::Radio => {
            builder = builder.field_checked(
                field.name.clone(),
                field.value.clone(),
                String::new(), // Label will be detected by analysis pipeline
                is_checked,
            );
        }
        _ => {
            builder = builder.field(field.name.clone(), field.value.clone(), String::new());
        }
    }

    // Attach widget type hint
    builder = builder.hint(Hint::WidgetType(widget_kind.clone()));

    // Attach validation hint
    let required = (field.flags & FF_REQUIRED) != 0;
    if required {
        builder = builder.hint(Hint::Validation {
            required: true,
            format_pattern: None,
            error_message: None,
        });
    }

    // Attach field behavior hint
    let read_only = (field.flags & FF_READ_ONLY) != 0;
    let multiline = (field.flags & FF_MULTILINE) != 0;
    builder = builder.hint(Hint::FieldBehavior {
        access: if read_only {
            FieldAccess::ReadOnly
        } else {
            FieldAccess::Open
        },
        multiline,
        max_length: None,
        comb_cells: None,
    });

    // Attach dropdown options if present
    if !field.options.is_empty() {
        let text_entry = (field.flags & FF_EDIT) != 0;
        let multi_select = (field.flags & FF_MULTI_SELECT) != 0;
        builder = builder.hint(Hint::Dropdown {
            options: field.options.clone(),
            text_entry,
            multi_select,
        });
    }

    let mut nodes = vec![builder.build()];

    // For checkbox/radio buttons, emit a label text node next to the checkbox
    // using the field name as label text (common in AcroForm PDFs where the
    // label is only stored as the field name, not as a separate text run).
    if matches!(widget_kind, WidgetKind::Checkbox | WidgetKind::Radio) {
        let label_text = &field.name;
        // Strip trailing numeric suffixes like "_2", "_13" from duplicate field names
        let label = label_text
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .trim_end_matches('_');
        if !label.is_empty() {
            let label_font_size = 9.0;
            let gap = 2.0; // small gap after checkbox
            let label_x = x + w + gap;
            let label_w = label.len() as f64 * label_font_size * 0.5; // rough estimate
            let label_node = FlattenedNodeBuilder::new()
                .bounds(to_num(label_x), to_num(y), to_num(label_w), to_num(h))
                .text(label.to_string(), to_num(label_font_size), String::new())
                .no_wrap(true)
                .build();
            nodes.push(label_node);
        }
    }

    nodes
}

/// Classify an AcroForm field into a WidgetKind.
fn classify_widget(field: &AcroFormField) -> WidgetKind {
    match field.field_type {
        AcroFieldType::Text => {
            if (field.flags & FF_PASSWORD) != 0 {
                WidgetKind::Password
            } else if (field.flags & FF_MULTILINE) != 0 {
                WidgetKind::TextArea
            } else {
                WidgetKind::Text
            }
        }
        AcroFieldType::Button => {
            if (field.flags & FF_PUSH_BUTTON) != 0 {
                WidgetKind::Button
            } else if (field.flags & FF_RADIO) != 0 {
                WidgetKind::Radio
            } else {
                WidgetKind::Checkbox
            }
        }
        AcroFieldType::Choice => WidgetKind::Dropdown,
        AcroFieldType::Signature => WidgetKind::Signature,
        AcroFieldType::Unknown => WidgetKind::Text,
    }
}

/// Get page dimensions (width, height) from the page's MediaBox or CropBox.
fn get_page_dimensions(doc: &Document, page_id: ObjectId) -> (f64, f64) {
    let page_obj = match doc.get_object(page_id) {
        Ok(obj) => obj,
        Err(_) => return (595.0, 842.0), // A4 default
    };

    // Try CropBox first (effective visible area), then MediaBox
    let rect = get_box(doc, page_obj, b"CropBox")
        .or_else(|| get_box(doc, page_obj, b"MediaBox"))
        .or_else(|| {
            // Try parent page tree node for inherited MediaBox
            if let Ok(dict) = page_obj.as_dict() {
                if let Ok(parent) = dict.get(b"Parent") {
                    if let Object::Reference(r) = parent {
                        if let Ok(parent_obj) = doc.get_object(*r) {
                            return get_box(doc, parent_obj, b"MediaBox");
                        }
                    }
                }
            }
            None
        });

    match rect {
        Some([x1, y1, x2, y2]) => ((x2 - x1).abs(), (y2 - y1).abs()),
        None => (595.0, 842.0), // A4 default
    }
}

/// Extract a rectangle array ([x1, y1, x2, y2]) from a dictionary entry.
fn get_box(doc: &Document, obj: &Object, key: &[u8]) -> Option<[f64; 4]> {
    let dict = obj.as_dict().ok()?;
    let box_obj = dict.get(key).ok()?;

    let resolved = match box_obj {
        Object::Reference(r) => doc.get_object(*r).ok()?,
        other => other,
    };

    let arr = resolved.as_array().ok()?;
    if arr.len() < 4 {
        return None;
    }

    Some([
        obj_to_f64(&arr[0])?,
        obj_to_f64(&arr[1])?,
        obj_to_f64(&arr[2])?,
        obj_to_f64(&arr[3])?,
    ])
}

pub(crate) fn obj_to_f64(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(n) => Some(*n as f64),
        Object::Real(f) => Some(*f as f64),
        _ => None,
    }
}

/// Infer the generic font family from a cleaned PDF font name.
/// Uses well-known font name patterns to classify as serif, sans-serif, or monospace.
fn infer_generic_family(name: &str) -> GenericFamily {
    let lower = name.to_lowercase();
    // Monospace fonts
    if lower.contains("courier")
        || lower.contains("consolas")
        || lower.contains("mono")
        || lower.contains("menlo")
        || lower.contains("lucida console")
    {
        return GenericFamily::Monospace;
    }
    // Serif fonts
    if lower.contains("times")
        || lower.contains("georgia")
        || lower.contains("garamond")
        || lower.contains("palatino")
        || lower.contains("cambria")
        || lower.contains("book antiqua")
        || lower.contains("bodoni")
    {
        return GenericFamily::Serif;
    }
    // Default: sans-serif (covers Helvetica, Arial, Verdana, Calibri, etc.)
    GenericFamily::SansSerif
}

/// Strip the PDF subset prefix from a font name.
///
/// PDF fonts are often subsetted and prefixed with 6 uppercase ASCII letters
/// followed by a '+' (e.g. "IHBBBY+HelveticaNeue-Light" → "HelveticaNeue-Light").
fn strip_subset_prefix(name: &str) -> &str {
    if name.len() > 7
        && name.as_bytes()[6] == b'+'
        && name[..6].chars().all(|c| c.is_ascii_uppercase())
    {
        &name[7..]
    } else {
        name
    }
}

/// Parse font style hints from a PDF font name.
/// E.g. "Helvetica-Bold" → ("Helvetica", Bold, Normal)
///      "TimesNewRoman,Italic" → ("TimesNewRoman", Normal, Italic)
///      "ArialMT" → ("Arial", Normal, Normal)
///      "IHBBBY+HelveticaNeue-Light" → ("HelveticaNeue", Normal, Normal)
fn parse_font_style(name: &str) -> (String, FontWeight, FontPosture) {
    // First strip the PDF subset prefix (e.g. "IHBBBY+" → "")
    let name = strip_subset_prefix(name);

    let name_lower = name.to_lowercase();
    let is_bold = name_lower.contains("bold");
    let is_italic = name_lower.contains("italic") || name_lower.contains("oblique");

    // Clean up the font name: remove style suffixes and common PDF name conventions
    let clean = name
        .replace("-Bold", "")
        .replace("-Italic", "")
        .replace("-BoldItalic", "")
        .replace("-BoldOblique", "")
        .replace("-Oblique", "")
        .replace("-Light", "")
        .replace("-Medium", "")
        .replace("-Thin", "")
        .replace("-UltraLight", "")
        .replace("-SemiBold", "")
        .replace("-ExtraBold", "")
        .replace("-Black", "")
        .replace("-Heavy", "")
        .replace("-Condensed", "")
        .replace(",Bold", "")
        .replace(",Italic", "")
        .replace(",BoldItalic", "")
        .replace("MT", "") // ArialMT → Arial
        .replace("-Roman", ""); // TimesNewRoman-Roman → TimesNewRoman

    let weight = if is_bold {
        FontWeight::Bold
    } else {
        FontWeight::Normal
    };

    let posture = if is_italic {
        FontPosture::Italic
    } else {
        FontPosture::Normal
    };

    (clean, weight, posture)
}

// ============================================================================
// Multi-page merging with header/footer detection
// ============================================================================

/// Tolerance in points for comparing element positions and sizes across pages.
const POSITION_TOLERANCE: f64 = 1.0;

/// A fingerprint that identifies an element by its kind, relative position
/// within a page, and dimensions. Used to detect elements that are repeated
/// across multiple pages (header/footer candidates).
#[derive(Clone, Debug)]
struct ElementFingerprint {
    /// Discriminant: "text" or "field"
    kind: &'static str,
    /// For Text nodes: the text content. For Field nodes: the field name.
    content_key: String,
    /// Font size (only meaningful for text nodes; 0 for fields).
    font_size: f64,
    /// Font name (only meaningful for text nodes; empty for fields).
    font_name: String,
    /// Relative X position within the page.
    rel_x: f64,
    /// Relative Y position within the page.
    rel_y: f64,
    /// Element width.
    width: f64,
    /// Element height.
    height: f64,
}

impl ElementFingerprint {
    fn from_node(node: &FlattenedNode) -> Self {
        let (kind, content_key, font_size, font_name) = match &node.kind {
            FlattenedNodeKind::Text {
                content,
                font_size,
                font_name,
                ..
            } => (
                "text",
                content.clone(),
                font_size.to_f64().unwrap_or(0.0),
                font_name.clone(),
            ),
            FlattenedNodeKind::Field { name, .. } => ("field", name.clone(), 0.0, String::new()),
        };

        ElementFingerprint {
            kind,
            content_key,
            font_size,
            font_name,
            rel_x: node.x.to_f64().unwrap_or(0.0),
            rel_y: node.y.to_f64().unwrap_or(0.0),
            width: node.width.to_f64().unwrap_or(0.0),
            height: node.height.to_f64().unwrap_or(0.0),
        }
    }

    /// Check if two fingerprints are "identical" within tolerance.
    fn matches(&self, other: &ElementFingerprint) -> bool {
        self.kind == other.kind
            && self.content_key == other.content_key
            && self.font_name == other.font_name
            && (self.font_size - other.font_size).abs() < POSITION_TOLERANCE
            && (self.rel_x - other.rel_x).abs() < POSITION_TOLERANCE
            && (self.rel_y - other.rel_y).abs() < POSITION_TOLERANCE
            && (self.width - other.width).abs() < POSITION_TOLERANCE
            && (self.height - other.height).abs() < POSITION_TOLERANCE
    }

    /// Bottom edge of this element (relative to page).
    fn bottom(&self) -> f64 {
        self.rel_y + self.height
    }

    /// Top edge of this element (relative to page).
    fn top(&self) -> f64 {
        self.rel_y
    }
}

/// Merge multiple per-page `Flattened` instances into a single `Flattened`
/// by stacking pages vertically.
///
/// Before merging, detects elements that appear on ≥50% of pages. These are
/// header/footer candidates:
/// - If a repeated element is in the upper half of the page → header candidate.
/// - If in the lower half → footer candidate.
///
/// The header boundary is the lowest bottom edge of all header candidates.
/// The footer boundary is the highest top edge of all footer candidates.
/// All elements within those regions on every page receive
/// `Hint::MasterPage { region: Header/Footer }`.
pub fn merge_pages(pages: Vec<Flattened>) -> Flattened {
    if pages.len() <= 1 {
        return pages.into_iter().next().unwrap_or_else(|| {
            Flattened::new(
                Page::new(Decimal::ZERO, Decimal::ZERO),
                Vec::new(),
            )
        });
    }

    let num_pages = pages.len();

    // -- Step 1: Build fingerprints per page (using original page-relative coordinates) --

    // Collect all leaf nodes per page with their fingerprints
    let mut page_fingerprints: Vec<Vec<ElementFingerprint>> = Vec::with_capacity(num_pages);
    let mut page_heights: Vec<f64> = Vec::with_capacity(num_pages);

    for page_flat in &pages {
        let page_h = page_flat.page.height.to_f64().unwrap_or(0.0);
        page_heights.push(page_h);

        let mut fps = Vec::new();
        for node in page_flat.iter_nodes() {
            fps.push(ElementFingerprint::from_node(node));
        }
        page_fingerprints.push(fps);
    }

    // -- Step 2: Find elements repeated on ≥50% of pages --

    // Use the first page as reference and count matches across other pages.
    // Then also check elements unique to other pages.
    // A simpler approach: collect all unique fingerprints, count how many
    // pages each appears on.

    struct FingerprintCount {
        fp: ElementFingerprint,
        page_count: usize,
    }

    let mut unique_fps: Vec<FingerprintCount> = Vec::new();

    for page_fps in &page_fingerprints {
        for fp in page_fps {
            // Check if this fingerprint already exists in our unique list
            let found = unique_fps.iter_mut().find(|u| u.fp.matches(fp));
            if let Some(existing) = found {
                // We need to track which pages it appeared on, not double-count
                // the same page. But since we iterate page by page, and a
                // fingerprint can appear at most once per page (by position),
                // incrementing is correct.
                existing.page_count += 1;
            } else {
                unique_fps.push(FingerprintCount {
                    fp: fp.clone(),
                    page_count: 1,
                });
            }
        }
    }

    // Filter to those appearing on ≥50% of pages
    let threshold = num_pages.div_ceil(2); // ceil(num_pages / 2)
    let repeated: Vec<&ElementFingerprint> = unique_fps
        .iter()
        .filter(|fc| fc.page_count >= threshold)
        .map(|fc| &fc.fp)
        .collect();

    // -- Step 3: Classify header/footer candidates --

    // Use the first page's height as reference for the midpoint
    let ref_page_height = page_heights[0];
    let midpoint = ref_page_height / 2.0;

    let mut header_boundary: Option<f64> = None; // lowest bottom edge of header candidates
    let mut footer_boundary: Option<f64> = None; // highest top edge of footer candidates

    for fp in &repeated {
        let center_y = fp.rel_y + fp.height / 2.0;
        if center_y < midpoint {
            // Header candidate — track the lowest bottom edge
            let bottom = fp.bottom();
            header_boundary = Some(match header_boundary {
                Some(prev) => prev.max(bottom),
                None => bottom,
            });
        } else {
            // Footer candidate — track the highest top edge
            let top = fp.top();
            footer_boundary = Some(match footer_boundary {
                Some(prev) => prev.min(top),
                None => top,
            });
        }
    }

    // -- Step 4: Merge pages into a single Flattened, applying hints --

    let max_width = pages
        .iter()
        .map(|p| p.page.width)
        .max()
        .unwrap_or(Decimal::ZERO);

    let total_height: Num = pages.iter().map(|p| p.page.height).sum();

    let mut merged_page = Page::new(max_width, total_height);

    let mut merged_children: Vec<FlattenedKind> = Vec::new();
    let mut y_offset = Decimal::ZERO;

    for (page_idx, page_flat) in pages.into_iter().enumerate() {
        let page_h = page_heights[page_idx];

        for mut kind in page_flat.children {
            // Offset Y coordinates and apply header/footer hints
            offset_and_tag_kind(
                &mut kind,
                y_offset,
                page_h,
                header_boundary,
                footer_boundary,
            );
            merged_children.push(kind);
        }

        y_offset += to_num(page_h);
        // Record page boundary for column layout detection
        merged_page.page_breaks.push(y_offset);
    }

    Flattened::new(merged_page, merged_children)
}

/// Recursively offset Y coordinates of all nodes in a `FlattenedKind` and
/// apply `MasterPage` hints based on header/footer boundaries.
fn offset_and_tag_kind(
    kind: &mut FlattenedKind,
    y_offset: Num,
    page_height: f64,
    header_boundary: Option<f64>,
    footer_boundary: Option<f64>,
) {
    match kind {
        FlattenedKind::Node(node) => {
            let rel_y = node.y.to_f64().unwrap_or(0.0);

            // Determine if this node is in a header/footer region.
            // Both regions are clamped to the page dimensions (before merging).
            let node_bottom = rel_y + node.height.to_f64().unwrap_or(0.0);

            if let Some(hb) = header_boundary {
                if node_bottom <= hb + POSITION_TOLERANCE {
                    node.add_hint(Hint::MasterPage {
                        region: MasterPageRegion::Header,
                    });
                }
            }
            if let Some(fb) = footer_boundary {
                if rel_y >= fb - POSITION_TOLERANCE
                    && node_bottom <= page_height + POSITION_TOLERANCE
                {
                    node.add_hint(Hint::MasterPage {
                        region: MasterPageRegion::Footer,
                    });
                }
            }

            // Offset Y by cumulative page heights
            node.y += y_offset;
        }
        FlattenedKind::Group { children, .. } => {
            for child in children {
                offset_and_tag_kind(
                    child,
                    y_offset,
                    page_height,
                    header_boundary,
                    footer_boundary,
                );
            }
        }
    }
}
