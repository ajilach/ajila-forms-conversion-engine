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

use crate::flattened::{
    FlattenedKind, FlattenedNode, FlattenedNodeBuilder, Hint, Page, RenderStyle,
    WidgetKind, Flattened,
};
use crate::xfa::{Font, FontWeight, FontPosture, Num};
use acroform::{
    AcroFieldType, AcroFormField, FF_EDIT, FF_MULTILINE, FF_MULTI_SELECT, FF_PASSWORD,
    FF_PUSH_BUTTON, FF_RADIO, FF_READ_ONLY, FF_REQUIRED, extract_acroform_fields,
};
use crate::flattened::FieldAccess;
use content_stream::{TextRun, extract_text_runs};
use lopdf::{Document, Object, ObjectId};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use std::collections::HashMap;

/// Parse a non-XFA PDF from in-memory bytes into a list of [`Flattened`] pages.
///
/// Each page of the PDF produces one `Flattened` instance containing:
/// - Static text extracted from the page content stream
/// - AcroForm fields (text inputs, checkboxes, radios, dropdowns, etc.)
///
/// Returns an error string if the PDF cannot be parsed.
pub fn parse_pdf(pdf_bytes: &[u8]) -> Result<Vec<Flattened>, String> {
    let doc = Document::load_mem(pdf_bytes)
        .map_err(|e| format!("Failed to parse PDF: {}", e))?;

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

        // Convert text runs to FlattenedNodes
        let mut children: Vec<FlattenedKind> = Vec::new();

        for run in &text_runs {
            if run.text.trim().is_empty() {
                continue;
            }
            let node = text_run_to_node(run);
            children.push(FlattenedKind::Node(node));
        }

        // Convert AcroForm fields to FlattenedNodes
        if let Some(page_fields) = fields_by_page.get(&i) {
            for field in page_fields {
                let nodes = acroform_field_to_nodes(field);
                for node in nodes {
                    children.push(FlattenedKind::Node(node));
                }
            }
        }

        let page = Page {
            width: to_num(page_w),
            height: to_num(page_h),
        };

        result.push(Flattened::new(page, children));
    }

    Ok(result)
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
        .build()
}

/// Convert an AcroForm field into one or more FlattenedNodes.
fn acroform_field_to_nodes(field: &AcroFormField) -> Vec<FlattenedNode> {
    let [x, y, w, h] = field.rect.unwrap_or([0.0, 0.0, 100.0, 20.0]);

    let widget_kind = classify_widget(field);
    let is_checked = field.is_checked;

    let mut builder = FlattenedNodeBuilder::new()
        .bounds(to_num(x), to_num(y), to_num(w), to_num(h));

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
            builder = builder.field(
                field.name.clone(),
                field.value.clone(),
                String::new(),
            );
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

    vec![builder.build()]
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
        AcroFieldType::Choice => {
            WidgetKind::Dropdown
        }
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

fn obj_to_f64(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(n) => Some(*n as f64),
        Object::Real(f) => Some(*f as f64),
        _ => None,
    }
}

/// Parse font style hints from a PDF font name.
/// E.g. "Helvetica-Bold" → ("Helvetica", Bold, Normal)
///      "TimesNewRoman,Italic" → ("TimesNewRoman", Normal, Italic)
///      "ArialMT" → ("Arial", Normal, Normal)
fn parse_font_style(name: &str) -> (String, FontWeight, FontPosture) {
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
