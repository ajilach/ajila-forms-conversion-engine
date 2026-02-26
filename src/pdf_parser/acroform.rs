//! AcroForm field extractor for non-XFA PDFs.
//!
//! Traverses the PDF's interactive form (AcroForm) field tree and extracts
//! field metadata: name, type, value, position (from widget annotations),
//! and options (for choice fields).

use lopdf::{Document, Object, ObjectId};
use std::collections::HashMap;

/// A single AcroForm field extracted from the PDF.
#[derive(Debug, Clone)]
pub struct AcroFormField {
    /// Fully-qualified field name (e.g. "form.address.city").
    pub name: String,
    /// Field type.
    pub field_type: AcroFieldType,
    /// Current field value (text content, selected option, etc.).
    pub value: String,
    /// Whether the field is checked (for checkboxes/radios).
    pub is_checked: Option<bool>,
    /// Bounding rectangle [x, y, width, height] in PDF user-space points.
    /// Coordinates use top-left origin (converted from PDF's bottom-left).
    pub rect: Option<[f64; 4]>,
    /// Options for choice fields (dropdown/listbox).
    /// Each entry is (display_text, export_value).
    pub options: Vec<(String, String)>,
    /// Field flags (/Ff).
    pub flags: u32,
    /// The page index (0-based) this field's widget is on, if determinable.
    pub page_index: Option<usize>,
}

/// AcroForm field type (from /FT dictionary entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcroFieldType {
    /// Text field (/Tx)
    Text,
    /// Button (/Btn) — checkbox, radio, or push button
    Button,
    /// Choice (/Ch) — dropdown (combo box) or list box
    Choice,
    /// Signature (/Sig)
    Signature,
    /// Unknown field type
    Unknown,
}

// Common /Ff flag bits (PDF spec Table 226, 227, 228)
/// If set, the field is read-only.
pub const FF_READ_ONLY: u32 = 1 << 0;
/// If set, the field is required.
pub const FF_REQUIRED: u32 = 1 << 1;
/// Button: if set, this is a radio button (not checkbox).
pub const FF_RADIO: u32 = 1 << 15;
/// Button: if set, this is a push button.
pub const FF_PUSH_BUTTON: u32 = 1 << 16;
/// Text: if set, multiline input allowed.
pub const FF_MULTILINE: u32 = 1 << 12;
/// Text: if set, password field.
pub const FF_PASSWORD: u32 = 1 << 13;
/// Choice: if set, this is a combo box (dropdown); otherwise list box.
pub const FF_COMBO: u32 = 1 << 17;
/// Choice: if set, user can type a custom value.
pub const FF_EDIT: u32 = 1 << 18;
/// Choice: if set, multiple selections allowed.
pub const FF_MULTI_SELECT: u32 = 1 << 21;

/// Extract all AcroForm fields from a PDF document.
///
/// `page_heights` maps page index → page height (for coordinate conversion).
pub fn extract_acroform_fields(
    doc: &Document,
    page_heights: &HashMap<usize, f64>,
    page_id_to_index: &HashMap<ObjectId, usize>,
) -> Vec<AcroFormField> {
    let mut fields = Vec::new();

    // Get the AcroForm dictionary from the catalog
    let catalog_dict = match doc.catalog() {
        Ok(d) => d,
        Err(_) => return fields,
    };

    let acroform = match catalog_dict.get(b"AcroForm") {
        Ok(obj) => resolve(doc, obj),
        Err(_) => return fields,
    };

    let acroform_dict = match acroform.and_then(|o| o.as_dict().ok()) {
        Some(d) => d,
        None => return fields,
    };

    // Get the Fields array
    let fields_array = match acroform_dict.get(b"Fields") {
        Ok(obj) => match resolve(doc, obj) {
            Some(Object::Array(arr)) => arr.clone(),
            _ => return fields,
        },
        Err(_) => return fields,
    };

    // Recursively traverse the field tree
    for field_obj in &fields_array {
        traverse_field(
            doc,
            field_obj,
            &mut String::new(),
            None, // inherited /FT
            0,    // inherited /Ff
            &mut fields,
            page_heights,
            page_id_to_index,
        );
    }

    fields
}

/// Recursively traverse the AcroForm field tree.
///
/// Fields can have `/Kids` which are either intermediate nodes (with more kids)
/// or terminal widget annotations. The `/T` (partial name) entries are
/// concatenated with dots to form the fully-qualified name.
fn traverse_field(
    doc: &Document,
    field_ref: &Object,
    parent_name: &mut String,
    inherited_ft: Option<AcroFieldType>,
    inherited_ff: u32,
    fields: &mut Vec<AcroFormField>,
    page_heights: &HashMap<usize, f64>,
    page_id_to_index: &HashMap<ObjectId, usize>,
) {
    let field_obj = match resolve(doc, field_ref) {
        Some(obj) => obj,
        None => return,
    };
    let field_dict = match field_obj.as_dict() {
        Ok(d) => d,
        Err(_) => return,
    };

    // Get partial name (/T)
    let partial_name = field_dict
        .get(b"T")
        .ok()
        .and_then(|o| match o {
            Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).to_string()),
            _ => None,
        })
        .unwrap_or_default();

    // Build the fully-qualified name
    let fq_name = if parent_name.is_empty() {
        partial_name.clone()
    } else if partial_name.is_empty() {
        parent_name.clone()
    } else {
        format!("{}.{}", parent_name, partial_name)
    };

    // Get field type (/FT) — inheritable
    let field_type = field_dict
        .get(b"FT")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|name| match name {
            b"Tx" => AcroFieldType::Text,
            b"Btn" => AcroFieldType::Button,
            b"Ch" => AcroFieldType::Choice,
            b"Sig" => AcroFieldType::Signature,
            _ => AcroFieldType::Unknown,
        })
        .or(inherited_ft);

    // Get field flags (/Ff) — inheritable
    let flags = field_dict
        .get(b"Ff")
        .ok()
        .and_then(|o| match o {
            Object::Integer(n) => Some(*n as u32),
            _ => None,
        })
        .unwrap_or(inherited_ff);

    // Check for /Kids
    if let Ok(kids_obj) = field_dict.get(b"Kids") {
        if let Some(Object::Array(kids)) = resolve(doc, kids_obj).cloned() {
            // Check if kids are widget annotations (have /Subtype /Widget) or intermediate nodes
            let mut has_widget_kids = false;
            let mut has_field_kids = false;

            for kid in &kids {
                if let Some(kid_obj) = resolve(doc, kid) {
                    if let Ok(kid_dict) = kid_obj.as_dict() {
                        if kid_dict.get(b"T").is_ok() {
                            has_field_kids = true;
                        } else {
                            has_widget_kids = true;
                        }
                    }
                }
            }

            if has_field_kids {
                // Intermediate node: recurse into children
                for kid in &kids {
                    traverse_field(
                        doc,
                        kid,
                        &mut fq_name.clone(),
                        field_type,
                        flags,
                        fields,
                        page_heights,
                        page_id_to_index,
                    );
                }
                return;
            }

            if has_widget_kids {
                // Widget annotations as kids — each is a separate widget for this field
                // (common for radio buttons where each kid is one option)
                for kid in &kids {
                    if let Some(kid_obj) = resolve(doc, kid) {
                        if let Ok(kid_dict) = kid_obj.as_dict() {
                            let rect = extract_rect(kid_dict, page_heights, page_id_to_index, doc);
                            let page_index = get_page_index(kid_dict, doc, page_id_to_index);
                            let value = extract_value(field_dict);
                            let is_checked = extract_checked_state(kid_dict, &value, field_type);

                            // For radio buttons, get the appearance state name as the value
                            let widget_value =
                                extract_widget_value(kid_dict).unwrap_or_else(|| value.clone());

                            fields.push(AcroFormField {
                                name: fq_name.clone(),
                                field_type: field_type.unwrap_or(AcroFieldType::Unknown),
                                value: widget_value,
                                is_checked,
                                rect,
                                options: Vec::new(),
                                flags,
                                page_index,
                            });
                        }
                    }
                }
                return;
            }
        }
    }

    // Terminal field (leaf node) — this is both a field and its own widget annotation
    let rect = extract_rect(field_dict, page_heights, page_id_to_index, doc);
    let page_index = get_page_index(field_dict, doc, page_id_to_index);
    let value = extract_value(field_dict);
    let is_checked = extract_checked_state(field_dict, &value, field_type);
    let options = extract_options(doc, field_dict);

    fields.push(AcroFormField {
        name: fq_name,
        field_type: field_type.unwrap_or(AcroFieldType::Unknown),
        value,
        is_checked,
        rect,
        options,
        flags,
        page_index,
    });
}

/// Extract the field value (/V).
fn extract_value(dict: &lopdf::Dictionary) -> String {
    dict.get(b"V")
        .ok()
        .map(|v| match v {
            Object::String(bytes, _) => String::from_utf8_lossy(bytes).to_string(),
            Object::Name(name) => {
                let s = String::from_utf8_lossy(name).to_string();
                if s == "Off" { String::new() } else { s }
            }
            Object::Integer(n) => n.to_string(),
            Object::Real(f) => f.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

/// Extract the widget appearance state value (/AS or from /AP /N keys).
fn extract_widget_value(dict: &lopdf::Dictionary) -> Option<String> {
    // Try /AS (appearance state)
    if let Ok(as_obj) = dict.get(b"AS") {
        if let Ok(name) = as_obj.as_name() {
            let s = String::from_utf8_lossy(name).to_string();
            if s != "Off" {
                return Some(s);
            }
        }
    }

    // Try /AP /N keys (appearance dictionary normal entry)
    if let Ok(ap) = dict.get(b"AP") {
        if let Ok(ap_dict) = ap.as_dict() {
            if let Ok(n_obj) = ap_dict.get(b"N") {
                if let Ok(n_dict) = n_obj.as_dict() {
                    for (key, _) in n_dict.iter() {
                        let name = String::from_utf8_lossy(key).to_string();
                        if name != "Off" {
                            return Some(name);
                        }
                    }
                }
            }
        }
    }

    None
}

/// Determine if a button field is currently checked.
fn extract_checked_state(
    dict: &lopdf::Dictionary,
    parent_value: &str,
    field_type: Option<AcroFieldType>,
) -> Option<bool> {
    if field_type != Some(AcroFieldType::Button) {
        return None;
    }

    // Check /AS (appearance state)
    if let Ok(as_obj) = dict.get(b"AS") {
        if let Ok(name) = as_obj.as_name() {
            let s = String::from_utf8_lossy(name);
            return Some(s != "Off");
        }
    }

    // Check /V against widget value
    if let Some(widget_val) = extract_widget_value(dict) {
        return Some(!parent_value.is_empty() && parent_value == widget_val);
    }

    None
}

/// Extract the bounding rectangle, converting from PDF coordinates (bottom-left origin)
/// to top-left origin.
fn extract_rect(
    dict: &lopdf::Dictionary,
    page_heights: &HashMap<usize, f64>,
    page_id_to_index: &HashMap<ObjectId, usize>,
    doc: &Document,
) -> Option<[f64; 4]> {
    let rect_obj = dict.get(b"Rect").ok()?;
    let rect_array = match rect_obj {
        Object::Array(arr) => arr,
        _ => return None,
    };

    if rect_array.len() < 4 {
        return None;
    }

    let x1 = obj_to_f64(&rect_array[0])?;
    let y1 = obj_to_f64(&rect_array[1])?;
    let x2 = obj_to_f64(&rect_array[2])?;
    let y2 = obj_to_f64(&rect_array[3])?;

    let x = x1.min(x2);
    let y_bottom = y1.min(y2);
    let width = (x2 - x1).abs();
    let height = (y2 - y1).abs();

    // Convert to top-left origin using page height
    let page_idx = get_page_index(dict, doc, page_id_to_index).unwrap_or(0);
    let page_height = page_heights.get(&page_idx).copied().unwrap_or(842.0); // A4 default

    let y_top = page_height - y_bottom - height;

    Some([x, y_top, width, height])
}

/// Extract options for choice fields (/Opt array).
fn extract_options(doc: &Document, dict: &lopdf::Dictionary) -> Vec<(String, String)> {
    let opt_obj = match dict.get(b"Opt") {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    let opt_array = match resolve(doc, opt_obj) {
        Some(Object::Array(arr)) => arr.clone(),
        _ => return Vec::new(),
    };

    let mut options = Vec::new();
    for item in &opt_array {
        match item {
            Object::String(bytes, _) => {
                let s = String::from_utf8_lossy(bytes).to_string();
                options.push((s.clone(), s));
            }
            Object::Array(pair) if pair.len() >= 2 => {
                let export = match &pair[0] {
                    Object::String(b, _) => String::from_utf8_lossy(b).to_string(),
                    _ => String::new(),
                };
                let display = match &pair[1] {
                    Object::String(b, _) => String::from_utf8_lossy(b).to_string(),
                    _ => export.clone(),
                };
                options.push((display, export));
            }
            _ => {}
        }
    }

    options
}

/// Get the page index for a widget annotation.
fn get_page_index(
    dict: &lopdf::Dictionary,
    _doc: &Document,
    page_id_to_index: &HashMap<ObjectId, usize>,
) -> Option<usize> {
    // Try /P (page reference)
    if let Ok(p_obj) = dict.get(b"P") {
        if let Object::Reference(page_ref) = p_obj {
            return page_id_to_index.get(page_ref).copied();
        }
    }
    None
}

/// Resolve an object reference, returning the dereferenced object.
fn resolve<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Object> {
    match obj {
        Object::Reference(r) => doc.get_object(*r).ok(),
        other => Some(other),
    }
}

fn obj_to_f64(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(n) => Some(*n as f64),
        Object::Real(f) => Some(*f as f64),
        _ => None,
    }
}
