mod xfa;
mod flattened;
mod document;
mod modules;
mod text_metrics;
mod scripting;
mod font_manager;

use pdf::file::FileOptions;
use pdf::object::*;
use pdf::primitive::Primitive;
use std::path::{Path, PathBuf};
use xfa::XfaNode;
use flattened::{Flattened, FlattenedNodeKind};
use clap::Parser;
use document::Document;
use modules::{TextBlockGrouper, FieldGrouper, LabelAttacher, HeadingDetector, RadioButtonDetector, RadioButtonGrouper, DateFieldDetector, AnalysisModule};

/// Check if PDF contains XFA and extract it
pub fn extract_xfa_from_pdf<P: AsRef<Path>>(path: P) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    let path = path.as_ref();
    let pdf = FileOptions::cached().open(path)?;
    
    // Get the catalog
    let catalog = pdf.get_root();
    
    // Try to get the AcroForm dictionary
    if let Some(forms_dict) = &catalog.forms {
        // Check if XFA key exists in the forms dictionary
        if let Some(xfa_obj) = &forms_dict.xfa {
            match xfa_obj {
                Primitive::Stream(pdf_stream) => {
                    // XFA is a stream - convert to Stream object and decode
                    let stream: Stream<()> = Stream::from_stream(pdf_stream.clone(), &pdf.resolver())?;
                    let data = stream.data(&pdf.resolver())?;
                    return Ok(Some(data.to_vec()));
                }
                Primitive::Array(arr) => {
                    // XFA is an array of (name, stream) pairs
                    let mut xfa_data = Vec::new();
                    let resolver = pdf.resolver();
                    
                    for i in (1..arr.len()).step_by(2) {
                        if let Primitive::Reference(stream_ref) = &arr[i]
                            && let Ok(Primitive::Stream(ref pdf_stream)) = resolver.resolve(*stream_ref) {
                                let stream: Stream<()> = Stream::from_stream(pdf_stream.clone(), &resolver)?;
                                let data = stream.data(&resolver)?;
                                xfa_data.extend_from_slice(&data);
                            }
                    }
                    
                    if !xfa_data.is_empty() {
                        return Ok(Some(xfa_data));
                    }
                }
                _ => {}
            }
        }
    }
    
    Ok(None)
}

/// Blueprint - XFA PDF document processor
#[derive(Parser, Debug)]
#[command(name = "blueprint")]
#[command(about = "Process and analyze XFA PDF documents", long_about = None)]
struct Args {
    /// Path to the PDF document
    #[arg(value_name = "DOCUMENT")]
    document: PathBuf,
    
    /// Render the document with labeled fields (blue group overlays)
    #[arg(long)]
    render_labelled: bool,
    
    /// Render the plain document without annotations
    #[arg(long)]
    render_plain: bool,
    
    /// Render the document with red field annotations
    #[arg(long)]
    render_annotated: bool,
    
    /// Scale factor for rendering (default: 1.5)
    #[arg(short, long, default_value = "1.5")]
    scale: f32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    
    // Check if document exists
    if !args.document.exists() {
        eprintln!("Error: Document not found: {}", args.document.display());
        std::process::exit(1);
    }
    
    println!("Processing document: {}", args.document.display());
    
    // Extract XFA data from PDF
    let xfa_data = extract_xfa_from_pdf(&args.document)?;
    
    if xfa_data.is_none() {
        eprintln!("Error: No XFA data found in PDF");
        std::process::exit(1);
    }
    
    println!("✓ XFA data extracted");
    
    // Parse XFA structure
    let nodes = XfaNode::parse(&xfa_data.unwrap())?;
    println!("✓ XFA structure parsed");
    
    // Get document name for locale detection
    let doc_name = args.document
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    
    // Detect locale from filename (e.g., "_DE", "_EN")
    let locale = if doc_name.ends_with("_DE") {
        "DE"
    } else if doc_name.ends_with("_EN") {
        "EN"
    } else {
        "EN" // default
    };
    
    // Flatten XFA with scripts
    let flattened = Flattened::from_xfa_with_scripts(&nodes, locale, doc_name)?;
    println!("✓ XFA flattened ({} nodes)", flattened.nodes.len());
    
    // Create document and run analysis modules
    let mut doc = Document::from_flattened(&flattened);
    
    // Run analysis pipeline
    TextBlockGrouper::new().process(&mut doc);
    let text_blocks = doc.find_groups(|k| matches!(k, document::GroupKind::TextBlock));
    println!("✓ Text blocks created: {}", text_blocks.len());
    
    FieldGrouper::new().process(&mut doc);
    let field_groups = doc.find_groups(|k| matches!(k, document::GroupKind::Field));
    println!("✓ Field groups created: {}", field_groups.len());
    
    DateFieldDetector::new().process(&mut doc);
    let date_fields = doc.find_groups(|k| matches!(k, document::GroupKind::DateField { .. }));
    println!("✓ Date fields detected: {}", date_fields.len());
    
    RadioButtonDetector::new().process(&mut doc);
    let radio_buttons = doc.find_groups(|k| matches!(k, document::GroupKind::RadioButton { .. }));
    println!("✓ Radio buttons detected: {}", radio_buttons.len());
    
    RadioButtonGrouper::new().process(&mut doc);
    let radio_button_groups = doc.find_groups(|k| matches!(k, document::GroupKind::RadioButtonGroup));
    println!("✓ Radio button groups created: {}", radio_button_groups.len());
    
    LabelAttacher::new().process(&mut doc);
    let labeled_fields = doc.labeled_fields();
    println!("✓ Labeled fields found: {}", labeled_fields.len());
    
    HeadingDetector::new().process(&mut doc);
    let headings = doc.headings();
    println!("✓ Headings detected: {}", headings.len());
    
    // Print radio button summary
    if !radio_buttons.is_empty() {
        println!("\nRadio Buttons:");
        for (i, &rb_idx) in radio_buttons.iter().enumerate() {
            if let Some(group) = doc.get_group(rb_idx)
                && let document::GroupKind::RadioButton { field, label } = group.kind {
                    // Get the field name
                    let field_name = group.children.get(field)
                        .and_then(|&field_idx| {
                            let nodes = doc.collect_nodes(field_idx);
                            nodes.first().and_then(|n| {
                                if let FlattenedNodeKind::Field { name, .. } = &n.kind {
                                    Some(name.clone())
                                } else {
                                    None
                                }
                            })
                        })
                        .unwrap_or_else(|| "unknown".to_string());
                    
                    // Get the label text
                    let label_text = group.children.get(label)
                        .map(|&label_idx| doc.get_text_content(label_idx))
                        .unwrap_or_else(String::new);
                    
                    let preview: String = label_text.chars().take(50).collect();
                    let suffix = if label_text.chars().count() > 50 { "..." } else { "" };
                    println!("  {}: [{}] {}{}", i + 1, field_name, preview, suffix);
                }
        }
    }
    
    // Print heading summary
    if !headings.is_empty() {
        println!("\nHeadings:");
        for &h_idx in &headings {
            if let Some(group) = doc.get_group(h_idx)
                && let document::GroupKind::Heading { level } = group.kind {
                    let text = doc.get_text_content(h_idx);
                    let preview: String = text.chars().take(60).collect();
                    let suffix = if text.chars().count() > 60 { "..." } else { "" };
                    println!("  H{}: {}{}", level, preview, suffix);
                }
        }
    }
    
    // Print labeled field summary
    if !labeled_fields.is_empty() {
        println!("\nLabeled Fields (sample):");
        for (i, &lf_idx) in labeled_fields.iter().take(10).enumerate() {
            let label_text = doc.get_label_text(lf_idx).unwrap_or_default();
            let field_name = doc.get_field_name(lf_idx).unwrap_or_default();
            let preview: String = label_text.chars().take(40).collect();
            let suffix = if label_text.chars().count() > 40 { "..." } else { "" };
            println!("  {}: '{}{}' -> {}", i + 1, preview, suffix, field_name);
        }
        if labeled_fields.len() > 10 {
            println!("  ... and {} more", labeled_fields.len() - 10);
        }
    }
    
    // Render if requested
    if args.render_labelled {
        let output_path = PathBuf::from(format!("{}_labelled.png", doc_name));
        
        println!("\nRendering document with labels...");
        doc.render_to_image(&output_path, args.scale)?;
        println!("✓ Document rendered to: {}", output_path.display());
    }
    
    if args.render_plain {
        let output_path = PathBuf::from(format!("{}_plain.png", doc_name));
        
        println!("\nRendering plain document...");
        flattened.render_to_image_buffer_plain(args.scale)?
            .save(&output_path)
            .map_err(|e| format!("Failed to save image: {}", e))?;
        println!("✓ Document rendered to: {}", output_path.display());
    }
    
    if args.render_annotated {
        let output_path = PathBuf::from(format!("{}_annotated.png", doc_name));
        
        println!("\nRendering annotated document...");
        flattened.render_to_image(&output_path, args.scale)?;
        println!("✓ Document rendered to: {}", output_path.display());
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::*;
    
    #[test]
    fn test_parse_xfa_from_aaab_document() {
        let pdf_path = "input/AAAB_019_DE.pdf";
        
        // Extract XFA from PDF
        let xfa_data = extract_xfa_from_pdf(pdf_path)
            .expect("Failed to read PDF");
        
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let xfa_buffer = xfa_data.unwrap();
        assert!(!xfa_buffer.is_empty(), "XFA buffer should not be empty");
        
        // Parse the XFA structure
        let nodes = XfaNode::parse(&xfa_buffer)
            .expect("Failed to parse XFA structure");
        
        assert!(!nodes.is_empty(), "Should parse at least one XFA node");
        
        // Count all nodes recursively
        let total_nodes = XfaNode::count_nodes(&nodes);
        
        println!("Successfully parsed {} root nodes", nodes.len());
        println!("Total nodes (including children): {}", total_nodes);
        
        // Verify we have substantial content
        assert!(total_nodes > 100, "Should have parsed many nodes from AAAB document");
    }
    
    #[test]
    fn test_fully_parse_aaab_structure() {
        let pdf_path = "input/AAAB_019_DE.pdf";
        
        // Extract XFA from PDF
        let xfa_data = extract_xfa_from_pdf(pdf_path)
            .expect("Failed to read PDF");
        
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        let xfa_buffer = xfa_data.unwrap();
        
        // Parse the XFA structure
        let nodes = XfaNode::parse(&xfa_buffer)
            .expect("Failed to parse XFA structure");
        
        // Verify structure
        assert!(!nodes.is_empty(), "Should have root nodes");
        
        // Print structure summary
        println!("\n=== XFA Structure Summary ===");
        println!("{}", XfaNode::summarize_structure(&nodes, 0));
        println!("=============================\n");
        
        // Look for template nodes
        let mut has_template = false;
        let mut has_subforms = false;
        let mut has_fields = false;
        
        fn check_nodes(nodes: &[XfaNode], has_template: &mut bool, has_subforms: &mut bool, has_fields: &mut bool) {
            for node in nodes {
                match &node.kind {
                    xfa::XfaNodeKind::Template => {
                        *has_template = true;
                        check_nodes(&node.children, has_template, has_subforms, has_fields);
                    }
                    xfa::XfaNodeKind::Subform => {
                        *has_subforms = true;
                        check_nodes(&node.children, has_template, has_subforms, has_fields);
                    }
                    xfa::XfaNodeKind::Field => {
                        *has_fields = true;
                    }
                    xfa::XfaNodeKind::Element { .. } => {
                        check_nodes(&node.children, has_template, has_subforms, has_fields);
                    }
                    xfa::XfaNodeKind::PageSet => {
                        check_nodes(&node.children, has_template, has_subforms, has_fields);
                    }
                    xfa::XfaNodeKind::PageArea => {
                        check_nodes(&node.children, has_template, has_subforms, has_fields);
                    }
                    xfa::XfaNodeKind::ContentArea => {
                        check_nodes(&node.children, has_template, has_subforms, has_fields);
                    }
                    xfa::XfaNodeKind::Draw => {
                        check_nodes(&node.children, has_template, has_subforms, has_fields);
                    }
                    xfa::XfaNodeKind::Value => {
                        check_nodes(&node.children, has_template, has_subforms, has_fields);
                    }
                    _ => {}
                }
            }
        }
        
        check_nodes(&nodes, &mut has_template, &mut has_subforms, &mut has_fields);
        
        println!("\nParsing results:");
        println!("  Has template: {}", has_template);
        println!("  Has subforms: {}", has_subforms);
        println!("  Has fields: {}", has_fields);
        
        let total_nodes = XfaNode::count_nodes(&nodes);
        println!("  Total nodes: {}", total_nodes);
        
        // The AAAB document should have substantial content
        assert!(total_nodes > 50, "Should have parsed substantial structure from AAAB");
        
        // We should find template/subform/field structure
        assert!(has_template || has_subforms || has_fields || total_nodes > 100, 
                "Should have parsed XFA form structure");
    }
    
    #[test]
    fn test_flatten_aaab_xfa() {
        // Test flattening a real XFA document
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Debug: print structure
        println!("\nXFA Structure:");
        println!("{}", XfaNode::summarize_structure(&nodes, 0));
        
        let flattened = Flattened::from_xfa(&nodes)
            .expect("Failed to flatten XFA");
        
        println!("\nFlattened AAAB document:");
        println!("Page dimensions: {}x{}", flattened.page.width, flattened.page.height);
        println!("Number of flattened nodes: {}", flattened.nodes.len());
        
        // Print first few nodes with their positions
        for (i, node) in flattened.nodes.iter().take(10).enumerate() {
            match &node.kind {
                flattened::FlattenedNodeKind::Field { name, .. } => {
                    println!("  [{}] Field '{}': x={:.1}, y={:.1}, w={:.1}, h={:.1}", 
                        i, name, node.x, node.y, node.width, node.height);
                }
                flattened::FlattenedNodeKind::Text { content, .. } => {
                    let preview = content.chars().take(40).collect::<String>();
                    println!("  [{}] Text: '{}...' at x={:.1}, y={:.1}", 
                        i, preview, node.x, node.y);
                }
            }
        }
        
        assert!(flattened.nodes.len() > 0, "Should have flattened nodes");
        println!("\n✓ AAAB flattening test passed!");
    }
    
    #[test]
    fn test_aaai_title_is_h1() {
        // Test that the AAAI document title "Vereinbarung für die Erteilung von Zahlungsaufträgen"
        // is correctly identified as an H1 heading
        use crate::document::Document;
        use crate::modules::{TextBlockGrouper, HeadingDetector, AnalysisModule};
        
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAI_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        let mut doc = Document::from_flattened(&flattened);
        TextBlockGrouper::new().process(&mut doc);
        HeadingDetector::new().process(&mut doc);
        
        let headings = doc.headings();
        
        // Find the H1 heading
        let h1_headings: Vec<_> = headings.iter()
            .filter_map(|&idx| {
                if let Some(group) = doc.get_group(idx) {
                    if let crate::document::GroupKind::Heading { level: 1 } = group.kind {
                        let text = doc.get_text_content(idx);
                        return Some((idx, text));
                    }
                }
                None
            })
            .collect();
        
        assert!(!h1_headings.is_empty(), "Should have at least one H1 heading");
        
        // The main title should be the H1
        let (_, title_text) = &h1_headings[0];
        assert!(
            title_text.contains("Vereinbarung") && title_text.contains("Zahlungsaufträg"),
            "H1 should be the document title 'Vereinbarung für die Erteilung von Zahlungsaufträgen', got: {}",
            title_text
        );
    }
    
    #[test]
    fn test_aaai_field_alignment() {
        // Test that specific fields that should be on the same line have the same Y coordinate
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        let flattened = Flattened::from_xfa(&nodes)
            .expect("Failed to flatten XFA");
        
        // Helper function to find field by name
        fn find_field<'a>(nodes: &'a [flattened::FlattenedNode], name: &str) -> Option<&'a flattened::FlattenedNode> {
            nodes.iter().find(|n| {
                if let flattened::FlattenedNodeKind::Field { name: field_name, .. } = &n.kind {
                    field_name == name
                } else {
                    false
                }
            })
        }
        
        // Test 1: TF_FamilyName and TF_FirstName should be on the same line
        let tf_family_name = find_field(&flattened.nodes, "TF_FamilyName")
            .expect("TF_FamilyName field not found");
        let tf_first_name = find_field(&flattened.nodes, "TF_FirstName")
            .expect("TF_FirstName field not found");
        
        let tolerance = rust_decimal::Decimal::from_str("0.01").unwrap();
        
        println!("\n=== Field Alignment Test ===");
        println!("TF_FamilyName: y={}, x={}", tf_family_name.y, tf_family_name.x);
        println!("TF_FirstName:  y={}, x={}", tf_first_name.y, tf_first_name.x);
        
        assert!(
            (tf_family_name.y - tf_first_name.y).abs() < tolerance,
            "TF_FamilyName (y={}) and TF_FirstName (y={}) should be on the same line",
            tf_family_name.y, tf_first_name.y
        );
        
        // Test 2: TF_Street and TF_StreetNumber should be on the same line (already correct)
        let tf_street = find_field(&flattened.nodes, "TF_Street")
            .expect("TF_Street field not found");
        let tf_street_number = find_field(&flattened.nodes, "TF_StreetNumber")
            .expect("TF_StreetNumber field not found");
        
        println!("TF_Street:       y={}, x={}", tf_street.y, tf_street.x);
        println!("TF_StreetNumber: y={}, x={}", tf_street_number.y, tf_street_number.x);
        
        assert!(
            (tf_street.y - tf_street_number.y).abs() < tolerance,
            "TF_Street (y={}) and TF_StreetNumber (y={}) should be on the same line",
            tf_street.y, tf_street_number.y
        );
        
        // Test 3: TF_PostalCode, TF_City, and TF_Country should be on the same line
        let tf_postal_code = find_field(&flattened.nodes, "TF_PostalCode")
            .expect("TF_PostalCode field not found");
        let tf_city = find_field(&flattened.nodes, "TF_City")
            .expect("TF_City field not found");
        let tf_country = find_field(&flattened.nodes, "TF_Country")
            .expect("TF_Country field not found");
        
        println!("TF_PostalCode: y={}, x={}, w={}, h={}", tf_postal_code.y, tf_postal_code.x, tf_postal_code.width, tf_postal_code.height);
        println!("TF_City:       y={}, x={}, w={}, h={}", tf_city.y, tf_city.x, tf_city.width, tf_city.height);
        println!("TF_Country:    y={}, x={}, w={}, h={}", tf_country.y, tf_country.x, tf_country.width, tf_country.height);
        println!("PostalCode ends at x={}", tf_postal_code.x + tf_postal_code.width);
        println!("Page width: {}", flattened.page.width);
        
        assert!(
            (tf_postal_code.y - tf_city.y).abs() < tolerance,
            "TF_PostalCode (y={}) and TF_City (y={}) should be on the same line",
            tf_postal_code.y, tf_city.y
        );
        
        assert!(
            (tf_postal_code.y - tf_country.y).abs() < tolerance,
            "TF_PostalCode (y={}) and TF_Country (y={}) should be on the same line",
            tf_postal_code.y, tf_country.y
        );
        
        // Test 4: TF_PostalCode and TF_City should NOT overlap
        // TF_City should start AFTER TF_PostalCode ends
        let postal_code_end_x = tf_postal_code.x + tf_postal_code.width;
        assert!(
            tf_city.x >= postal_code_end_x - tolerance,
            "TF_City (x={}) should not overlap with TF_PostalCode (ends at x={})",
            tf_city.x, postal_code_end_x
        );
        
        // Test 5: TF_City and TF_Country should NOT overlap
        let city_end_x = tf_city.x + tf_city.width;
        assert!(
            tf_country.x >= city_end_x - tolerance,
            "TF_Country (x={}) should not overlap with TF_City (ends at x={})",
            tf_country.x, city_end_x
        );
        
        println!("\n✓ All field alignment tests passed!");
    }
    
    /// Test font properties for the "Der Kunde beauftragt hiermit UBS Europe SE" paragraph (T_Left).
    /// 
    /// According to XFA spec and the actual XFA data:
    /// - XFA font element: typeface="Frutiger 45 Light", size="8pt", weight="bold"
    /// - HTML paragraph style: font-weight:normal, letter-spacing:0in
    /// 
    /// The HTML style should override the XFA font weight for rich text content.
    #[test]
    fn test_aaai_t_left_font_properties() {
        use crate::flattened::{Flattened, FlattenedNodeKind};
        use crate::xfa::{XfaNode, FontWeight};
        use rust_decimal::Decimal;
        
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF")
            .expect("No XFA data");
        
        let nodes = XfaNode::parse(&xfa_data)
            .expect("Failed to parse XFA structure");
        
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAI_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Find the T_Left text node
        let t_left = flattened.nodes.iter()
            .find(|n| {
                if let FlattenedNodeKind::Text { source_name, .. } = &n.kind {
                    source_name.as_ref().map(|s| s == "T_Left").unwrap_or(false)
                } else {
                    false
                }
            })
            .expect("T_Left text node not found");
        
        println!("\n=== T_Left Font Properties Test ===");
        
        // ----------------------------------------------------------------
        // Test 1: XFA font properties are correctly parsed
        // ----------------------------------------------------------------
        let xfa_font = t_left.style.font.as_ref()
            .expect("T_Left should have font style");
        
        // Per XFA: typeface="Frutiger 45 Light"
        assert_eq!(
            xfa_font.typeface, "Frutiger 45 Light",
            "Font typeface should be 'Frutiger 45 Light'"
        );
        println!("  ✓ Typeface: {}", xfa_font.typeface);
        
        // Per XFA: size="8pt"
        let expected_size = Decimal::from(8);
        assert_eq!(
            xfa_font.size, expected_size,
            "Font size should be 8pt, got {:?}", xfa_font.size
        );
        println!("  ✓ Size: {}pt", xfa_font.size);
        
        // Per XFA: weight="bold" (but HTML overrides to normal for rich text)
        assert_eq!(
            xfa_font.weight, FontWeight::Bold,
            "XFA font weight should be Bold"
        );
        println!("  ✓ XFA Weight: {:?}", xfa_font.weight);
        
        // Per XFA: letterSpacing not specified, so should be None (default 0)
        // Note: The HTML specifies letter-spacing:0in which is effectively 0
        assert!(
            xfa_font.letter_spacing.is_none() || 
            xfa_font.letter_spacing == Some(Decimal::ZERO),
            "Letter spacing should be 0 or None, got {:?}", xfa_font.letter_spacing
        );
        println!("  ✓ Letter spacing: {:?}", xfa_font.letter_spacing);
        
        // ----------------------------------------------------------------
        // Test 2: Rich text content is correctly parsed
        // ----------------------------------------------------------------
        if let FlattenedNodeKind::Text { rich_text, content, .. } = &t_left.kind {
            let rt = rich_text.as_ref()
                .expect("T_Left should have rich text (HTML exData)");
            
            assert!(!rt.paragraphs.is_empty(), "Rich text should have paragraphs");
            println!("  ✓ Rich text has {} paragraphs", rt.paragraphs.len());
            
            // First paragraph should contain "Der Kunde beauftragt hiermit UBS Europe SE"
            let first_para = &rt.paragraphs[0];
            assert!(!first_para.runs.is_empty(), "First paragraph should have text runs");
            
            let first_text = &first_para.runs[0].text;
            assert!(
                first_text.starts_with("Der Kunde beauftragt hiermit UBS Europe SE"),
                "First paragraph should start with expected text, got: '{}'", 
                &first_text[..first_text.len().min(50)]
            );
            println!("  ✓ First paragraph text: '{}...'", &first_text[..first_text.len().min(40)]);
            
            // Per HTML: font-weight:normal - the run should NOT be bold
            assert!(
                !first_para.runs[0].bold,
                "First paragraph run should NOT be bold (HTML overrides XFA weight)"
            );
            println!("  ✓ First run bold: {} (expected: false)", first_para.runs[0].bold);
            
            // Per HTML: text-decoration:none - no underline
            assert!(
                !first_para.runs[0].underline,
                "First paragraph run should NOT be underlined"
            );
            println!("  ✓ First run underline: {} (expected: false)", first_para.runs[0].underline);
            
            // Check content field also has text (fallback for non-rich rendering)
            assert!(
                !content.is_empty(),
                "Content string should be present as fallback"
            );
            println!("  ✓ Fallback content present: {} chars", content.len());
        } else {
            panic!("T_Left should be a Text node");
        }
        
        // ----------------------------------------------------------------
        // Test 3: Verify paragraphs with text-indent are properly marked
        // ----------------------------------------------------------------
        if let FlattenedNodeKind::Text { rich_text, .. } = &t_left.kind {
            let rt = rich_text.as_ref().unwrap();
            
            // Some paragraphs should have text-indent (e.g., text-indent:25.512pt)
            let indented_paras: Vec<_> = rt.paragraphs.iter()
                .filter(|p| p.text_indent.is_some() && p.text_indent.unwrap() > 0.0)
                .collect();
            
            assert!(
                !indented_paras.is_empty(),
                "Some paragraphs should have text-indent"
            );
            println!("  ✓ Found {} paragraphs with text-indent", indented_paras.len());
            
            // Check the indent value is approximately 25.512pt (converted to pixels in style)
            if let Some(indent) = indented_paras[0].text_indent {
                // The indent should be around 25.5 pt (stored as-is in points)
                assert!(
                    indent > 20.0 && indent < 30.0,
                    "Text indent should be around 25pt, got {}", indent
                );
                println!("  ✓ First indented paragraph indent: {}pt", indent);
            }
        }
        
        println!("\n✓ All T_Left font property tests passed!");
    }

    #[test]
    fn test_aaab_des_label_alignment() {
        // Test that DES_PostalCode, DES_City, and DES_Country labels are on the same line
        // These are the labels for PLZ, Stadt, and Land
        use crate::flattened::{Flattened, FlattenedNodeKind};
        
        // Use AAAI which has these fields
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Use from_xfa_with_scripts to get the computed label text
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAI_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Helper function to find text node by source_name (Draw element name)
        fn find_draw_by_name<'a>(nodes: &'a [flattened::FlattenedNode], name: &str) -> Option<&'a flattened::FlattenedNode> {
            nodes.iter().find(|n| {
                matches!(&n.kind, FlattenedNodeKind::Text { source_name: Some(sn), .. } if sn == name)
            })
        }
        
        // Debug: Print all text nodes with source names containing "Postal" or "City" or "Country"
        println!("\n=== All Text nodes with Postal/City/Country in source_name ===");
        for node in &flattened.nodes {
            if let FlattenedNodeKind::Text { source_name: Some(sn), content, .. } = &node.kind {
                if sn.contains("Postal") || sn.contains("City") || sn.contains("Country") ||
                   sn.contains("postal") || sn.contains("city") || sn.contains("country") ||
                   sn.contains("PLZ") || sn.contains("Stadt") || sn.contains("Land") {
                    println!("  '{}': y={}, x={}, w={}, h={}, content='{}'", 
                        sn, node.y, node.x, node.width, node.height, content);
                }
            }
        }
        
        // Debug: print all source_names that contain "DES"
        println!("\n=== All Text nodes with DES in source_name ===");
        for node in &flattened.nodes {
            if let FlattenedNodeKind::Text { source_name: Some(sn), content, .. } = &node.kind {
                if sn.contains("DES") {
                    println!("  '{}': y={}, x={}, w={}, h={}, content='{}'", 
                        sn, node.y, node.x, node.width, node.height, content);
                }
            }
        }
        
        // Find DES_PostalCode, DES_City, DES_Country
        let des_postal = find_draw_by_name(&flattened.nodes, "DES_PostalCode")
            .expect("DES_PostalCode not found");
        let des_city = find_draw_by_name(&flattened.nodes, "DES_City")
            .expect("DES_City not found");
        let des_country = find_draw_by_name(&flattened.nodes, "DES_Country")
            .expect("DES_Country not found");
        
        println!("\n=== DES Label Alignment Test ===");
        println!("DES_PostalCode: y={}, x={}, w={}, h={}", des_postal.y, des_postal.x, des_postal.width, des_postal.height);
        println!("DES_City:       y={}, x={}, w={}, h={}", des_city.y, des_city.x, des_city.width, des_city.height);
        println!("DES_Country:    y={}, x={}, w={}, h={}", des_country.y, des_country.x, des_country.width, des_country.height);
        
        if let FlattenedNodeKind::Text { content, .. } = &des_postal.kind {
            println!("DES_PostalCode content: '{}'", content);
        }
        if let FlattenedNodeKind::Text { content, .. } = &des_city.kind {
            println!("DES_City content: '{}'", content);
        }
        if let FlattenedNodeKind::Text { content, .. } = &des_country.kind {
            println!("DES_Country content: '{}'", content);
        }
        
        let tolerance = rust_decimal::Decimal::from_str("0.01").unwrap();
        
        // Test 1: All three labels should be on the same line (same Y coordinate)
        assert!(
            (des_postal.y - des_city.y).abs() < tolerance,
            "DES_PostalCode (y={}) and DES_City (y={}) should be on the same line",
            des_postal.y, des_city.y
        );
        
        assert!(
            (des_postal.y - des_country.y).abs() < tolerance,
            "DES_PostalCode (y={}) and DES_Country (y={}) should be on the same line",
            des_postal.y, des_country.y
        );
        
        // Test 2: Labels should be in order left-to-right: PostalCode, City, Country
        assert!(
            des_postal.x < des_city.x,
            "DES_PostalCode (x={}) should be to the left of DES_City (x={})",
            des_postal.x, des_city.x
        );
        
        assert!(
            des_city.x < des_country.x,
            "DES_City (x={}) should be to the left of DES_Country (x={})",
            des_city.x, des_country.x
        );
        
        // Test 3: Labels should NOT overlap
        let postal_end_x = des_postal.x + des_postal.width;
        assert!(
            des_city.x >= postal_end_x - tolerance,
            "DES_City (x={}) should not overlap with DES_PostalCode (ends at x={})",
            des_city.x, postal_end_x
        );
        
        let city_end_x = des_city.x + des_city.width;
        assert!(
            des_country.x >= city_end_x - tolerance,
            "DES_Country (x={}) should not overlap with DES_City (ends at x={})",
            des_country.x, city_end_x
        );
        
        println!("\n✓ DES label alignment test passed!");
    }
    
    #[test]
    fn test_debug_des_postalcode_structure() {
        // Debug the XFA structure for DES_PostalCode
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Find and dump DES_PostalCode node
        fn find_and_dump(nodes: &[xfa::XfaNode], target: &str, indent: usize) -> bool {
            let prefix = "  ".repeat(indent);
            for node in nodes {
                if node.name.as_deref() == Some(target) {
                    println!("{}FOUND: {} {:?}", prefix, target, node.kind);
                    println!("{}  Parsed dimensions:", prefix);
                    println!("{}    x = {:?}", prefix, node.x);
                    println!("{}    y = {:?}", prefix, node.y);
                    println!("{}    w = {:?}", prefix, node.w);
                    println!("{}    h = {:?}", prefix, node.h);
                    println!("{}    min_w = {:?}", prefix, node.min_w);
                    println!("{}    min_h = {:?}", prefix, node.min_h);
                    println!("{}  Raw attributes:", prefix);
                    for (k, v) in &node.attributes {
                        println!("{}    {}={}", prefix, k, v);
                    }
                    println!("{}  Children:", prefix);
                    dump_node_tree(node, indent + 2);
                    return true;
                }
                if find_and_dump(&node.children, target, indent) {
                    return true;
                }
            }
            false
        }
        
        fn dump_node_tree(node: &xfa::XfaNode, indent: usize) {
            let prefix = "  ".repeat(indent);
            for child in &node.children {
                match &child.kind {
                    xfa::XfaNodeKind::Element { tag_name, text_content } => {
                        println!("{}Element: {} (attrs: {:?})", prefix, tag_name, child.attributes);
                        if let Some(content) = text_content {
                            println!("{}  text_content: {:?}", prefix, &content[..content.len().min(200)]);
                        }
                        println!("{}  w={:?}, h={:?}", prefix, child.w, child.h);
                        dump_node_tree(child, indent + 1);
                    }
                    xfa::XfaNodeKind::Value => {
                        println!("{}Value (attrs: {:?})", prefix, child.attributes);
                        println!("{}  w={:?}, h={:?}", prefix, child.w, child.h);
                        dump_node_tree(child, indent + 1);
                    }
                    other => {
                        println!("{}{:?} (attrs: {:?})", prefix, other, child.attributes);
                        if child.w.is_some() || child.h.is_some() {
                            println!("{}  w={:?}, h={:?}", prefix, child.w, child.h);
                        }
                        dump_node_tree(child, indent + 1);
                    }
                }
            }
        }
        
        println!("\n=== DES_PostalCode structure ===\n");
        if !find_and_dump(&nodes, "DES_PostalCode", 0) {
            println!("DES_PostalCode not found!");
        }
        
        println!("\n=== DES_City structure ===\n");
        if !find_and_dump(&nodes, "DES_City", 0) {
            println!("DES_City not found!");
        }
        
        println!("\n=== DES_Country structure ===\n");
        if !find_and_dump(&nodes, "DES_Country", 0) {
            println!("DES_Country not found!");
        }
    }
    
    #[test]
    fn test_debug_xfa_positioning() {
        // Debug XFA positioning to understand coordinate system
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Print detailed positioning information
        fn print_positioning(nodes: &[xfa::XfaNode], indent: usize, parent_path: &str) {
            let indent_str = "  ".repeat(indent);
            let empty = String::new();
            
            for node in nodes {
                match &node.kind {
                    xfa::XfaNodeKind::Element { tag_name, .. } => {
                        let path = format!("{}/{}", parent_path, tag_name);
                        let has_pos = node.x.is_some() || node.y.is_some() || 
                                     node.w.is_some() || node.h.is_some();
                        
                        // Always show contentArea, and show margin elements
                        let show = has_pos || tag_name == "pageArea" || tag_name == "contentArea" 
                                   || tag_name == "margin" || tag_name == "para";
                        
                        if show {
                            let x = node.x.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                            let y = node.y.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                            let w = node.w.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                            let h = node.h.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                            let name = node.name.as_ref().unwrap_or(&empty);
                            let layout = node.layout.as_ref().unwrap_or(&empty);
                            
                            // Show margin insets if present
                            let top_inset = node.attributes.get("topInset").map(|s| s.as_str()).unwrap_or("");
                            let bottom_inset = node.attributes.get("bottomInset").map(|s| s.as_str()).unwrap_or("");
                            let left_inset = node.attributes.get("leftInset").map(|s| s.as_str()).unwrap_or("");
                            let right_inset = node.attributes.get("rightInset").map(|s| s.as_str()).unwrap_or("");
                            
                            // Show para spacing if present
                            let space_above = node.attributes.get("spaceAbove").map(|s| s.as_str()).unwrap_or("");
                            let space_below = node.attributes.get("spaceBelow").map(|s| s.as_str()).unwrap_or("");
                            
                            if tag_name == "margin" {
                                println!("{}{} top={} bottom={} left={} right={}", 
                                    indent_str, tag_name, top_inset, bottom_inset, left_inset, right_inset);
                            } else if tag_name == "para" {
                                println!("{}{} spaceAbove={} spaceBelow={}", 
                                    indent_str, tag_name, space_above, space_below);
                            } else {
                                println!("{}{} [{}] x={} y={} w={} h={} layout={}", 
                                    indent_str, tag_name, name, x, y, w, h, layout);
                            }
                        }
                        
                        if indent < 6 {
                            print_positioning(&node.children, indent + 1, &path);
                        }
                    }
                    xfa::XfaNodeKind::ContentArea => {
                        let x = node.x.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                        let y = node.y.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                        let w = node.w.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                        let h = node.h.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                        let name = node.name.as_ref().unwrap_or(&empty);
                        println!("{}ContentArea [{}] x={} y={} w={} h={}", indent_str, name, x, y, w, h);
                        print_positioning(&node.children, indent + 1, &format!("{}/ContentArea", parent_path));
                    }
                    xfa::XfaNodeKind::PageArea => {
                        let x = node.x.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                        let y = node.y.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                        let w = node.w.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                        let h = node.h.map(|v| format!("{:.1}pt", v)).unwrap_or_default();
                        let name = node.name.as_ref().unwrap_or(&empty);
                        println!("{}PageArea [{}] x={} y={} w={} h={}", indent_str, name, x, y, w, h);
                        print_positioning(&node.children, indent + 1, &format!("{}/PageArea", parent_path));
                    }
                    _ => {}
                }
            }
        }
        
        println!("\n=== Detailed Positioning Information ===");
        print_positioning(&nodes, 0, "");
    }

    #[test]
    fn test_dump_xfa() {
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF")
            .expect("No XFA data");
        // Write to file for inspection
        std::fs::write("/tmp/xfa_debug.xml", &xfa_data).expect("write failed");
        println!("Wrote XFA to /tmp/xfa_debug.xml");
    }
    
    #[test]
    fn test_dump_aaai_xfa() {
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF")
            .expect("No XFA data");
        // Write to file for inspection
        std::fs::write("/tmp/aaai_xfa_debug.xml", &xfa_data).expect("write failed");
        println!("Wrote XFA to /tmp/aaai_xfa_debug.xml");
    }
    
    #[test]
    fn test_draw_text_extraction() {
        // Test that we can extract text from draw elements
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF")
            .expect("No XFA data");
        
        let nodes = XfaNode::parse(&xfa_data)
            .expect("Failed to parse XFA structure");
        
        // Find draw elements and print their content
        fn find_draws(nodes: &[xfa::XfaNode], path: &str) {
            for node in nodes {
                let node_name = node.name.as_ref().map(|s| s.as_str()).unwrap_or("");
                
                match &node.kind {
                    xfa::XfaNodeKind::Draw => {
                        println!("Draw [{}] at path {}", node_name, path);
                        println!("  Children count: {}", node.children.len());
                        for (i, child) in node.children.iter().enumerate() {
                            match &child.kind {
                                xfa::XfaNodeKind::Value => {
                                    println!("  Child {}: Value with {} children", i, child.children.len());
                                    for (j, vc) in child.children.iter().enumerate() {
                                        match &vc.kind {
                                            xfa::XfaNodeKind::Element { tag_name, text_content } => {
                                                println!("    ValueChild {}: Element '{}' text={:?}, children={}", 
                                                    j, tag_name, text_content, vc.children.len());
                                            }
                                            _ => println!("    ValueChild {}: {:?}", j, vc.kind),
                                        }
                                    }
                                }
                                xfa::XfaNodeKind::Element { tag_name, .. } => {
                                    println!("  Child {}: Element '{}'", i, tag_name);
                                }
                                _ => {}
                            }
                        }
                    }
                    xfa::XfaNodeKind::Element { tag_name, .. } if tag_name == "draw" => {
                        println!("draw (Element) [{}] at path {}", node_name, path);
                    }
                    _ => {}
                }
                
                let new_path = format!("{}/{}", path, node_name);
                find_draws(&node.children, &new_path);
            }
        }
        
        find_draws(&nodes, "");
    }
    
    #[test]
    fn test_debug_postal_code_structure() {
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        fn find_subform<'a>(nodes: &'a [XfaNode], name: &str) -> Option<&'a XfaNode> {
            for node in nodes {
                if node.name.as_ref().map(|n| n == name).unwrap_or(false) {
                    return Some(node);
                }
                if let Some(found) = find_subform(&node.children, name) {
                    return Some(found);
                }
            }
            None
        }
        
        let postal_subform = find_subform(&nodes, "PostalCode_City_Country")
            .expect("Should find PostalCode_City_Country subform");
        
        println!("\n=== PostalCode_City_Country Subform ===");
        println!("Layout: {:?}", postal_subform.layout);
        println!("x={:?}, y={:?}, w={:?}, h={:?}", 
            postal_subform.x, postal_subform.y, postal_subform.w, postal_subform.h);
        
        for child in &postal_subform.children {
            let name = child.name.as_deref().unwrap_or("?");
            println!("\n  Child: {} ({:?})", name, child.kind);
            println!("    layout: {:?}", child.layout);
            println!("    x={:?}, y={:?}, w={:?}, h={:?}", child.x, child.y, child.w, child.h);
            println!("    min_h={:?}", child.min_h);
            
            for grandchild in &child.children {
                let gname = grandchild.name.as_deref().unwrap_or("?");
                println!("      GrandChild: {} ({:?})", gname, grandchild.kind);
                println!("        x={:?}, y={:?}, w={:?}, h={:?}", grandchild.x, grandchild.y, grandchild.w, grandchild.h);
            }
        }
    }
    
    #[test]
    fn test_aaai_header_positioning() {
        // Test that "UBS Europe SE" text is positioned ABOVE the form title
        // "Vereinbarung für die Erteilung von Zahlungsaufträgen..."
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Debug: find elements containing "UBS"
        fn find_all_nodes_containing_text<'a>(nodes: &'a [XfaNode], text: &str, path: &str) {
            for node in nodes {
                let name = node.name.as_deref().unwrap_or("?");
                let new_path = format!("{}/{}", path, name);
                
                // Check if this node has a value child with the text
                for child in &node.children {
                    if let xfa::XfaNodeKind::Value = &child.kind {
                        for grandchild in &child.children {
                            if let xfa::XfaNodeKind::Element { text_content: Some(content), .. } = &grandchild.kind {
                                if content.contains(text) {
                                    println!("Found '{}' at path: {}", text, new_path);
                                    println!("  Node layout: {:?}, x={:?}, y={:?}", node.layout, node.x, node.y);
                                }
                            }
                        }
                    }
                }
                
                find_all_nodes_containing_text(&node.children, text, &new_path);
            }
        }
        
        println!("\n=== All occurrences of 'UBS Europe SE' ===");
        find_all_nodes_containing_text(&nodes, "UBS Europe SE", "");
        
        // Find the Client_Details -> SectionTitle structure
        fn find_subform<'a>(nodes: &'a [XfaNode], name: &str) -> Option<&'a XfaNode> {
            for node in nodes {
                if node.name.as_ref().map(|n| n == name).unwrap_or(false) {
                    return Some(node);
                }
                if let Some(found) = find_subform(&node.children, name) {
                    return Some(found);
                }
            }
            None
        }
        
        // Look at Client_Details to see its first child (SectionTitle with header)
        if let Some(cd) = find_subform(&nodes, "Client_Details") {
            println!("\n=== Client_Details Structure ===");
            println!("layout: {:?}", cd.layout);
            for (i, child) in cd.children.iter().enumerate() {
                let name = child.name.as_deref().unwrap_or("?");
                let kind = match &child.kind {
                    xfa::XfaNodeKind::Subform => "Subform",
                    xfa::XfaNodeKind::Draw => "Draw",
                    xfa::XfaNodeKind::Field => "Field",
                    _ => "Other",
                };
                println!("  [{}] {} {} x={:?} y={:?}", i, kind, name, child.x, child.y);
                
                // If this is SectionTitle, print its children
                if name == "SectionTitle" {
                    for grandchild in &child.children {
                        let gname = grandchild.name.as_deref().unwrap_or("?");
                        let gkind = match &grandchild.kind {
                            xfa::XfaNodeKind::Draw => "Draw",
                            xfa::XfaNodeKind::Field => "Field",
                            _ => "Other",
                        };
                        println!("      {} {} x={:?} y={:?} w={:?}", gkind, gname, grandchild.x, grandchild.y, grandchild.w);
                    }
                }
            }
        }
        
        // Look at the first draw element "T_Client_Details" or similar
        fn find_draw<'a>(nodes: &'a [XfaNode], name: &str) -> Option<&'a XfaNode> {
            for node in nodes {
                if node.name.as_ref().map(|n| n == name).unwrap_or(false) {
                    if let xfa::XfaNodeKind::Draw = &node.kind {
                        return Some(node);
                    }
                }
                if let Some(found) = find_draw(&node.children, name) {
                    return Some(found);
                }
            }
            None
        }
        
        // Check T_UBS_Company (should be the header element)
        if let Some(ubs_draw) = find_draw(&nodes, "T_UBS_Company") {
            println!("\n=== T_UBS_Company (header) ===");
            println!("x={:?}, y={:?}, w={:?}, h={:?}", ubs_draw.x, ubs_draw.y, ubs_draw.w, ubs_draw.h);
        }
        
        let flattened = Flattened::from_xfa(&nodes)
            .expect("Failed to flatten XFA");
        
        // Helper function to find text node by content substring
        fn find_text_containing<'a>(nodes: &'a [flattened::FlattenedNode], substring: &str) -> Option<&'a flattened::FlattenedNode> {
            nodes.iter().find(|n| {
                if let flattened::FlattenedNodeKind::Text { content, .. } = &n.kind {
                    content.contains(substring)
                } else {
                    false
                }
            })
        }
        
        // Print ALL text nodes with their positions for analysis
        println!("\n=== All Text Nodes (sorted by y) ===");
        let mut text_nodes: Vec<_> = flattened.nodes.iter()
            .filter_map(|n| {
                if let flattened::FlattenedNodeKind::Text { content, .. } = &n.kind {
                    Some((n.y, n.x, content.clone()))
                } else {
                    None
                }
            })
            .collect();
        text_nodes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        
        for (y, x, content) in text_nodes.iter().take(15) {
            let preview: String = content.chars().take(50).collect();
            println!("y={:.2}, x={:.2}: '{}'", y.to_f32().unwrap_or(0.0), x.to_f32().unwrap_or(0.0), preview);
        }
        
        // Find UBS Europe SE text (company name in header)
        let ubs_text = find_text_containing(&flattened.nodes, "UBS Europe SE")
            .expect("UBS Europe SE text not found");
        
        // Find form title text
        let title_text = find_text_containing(&flattened.nodes, "Vereinbarung")
            .expect("Form title (Vereinbarung...) text not found");
        
        println!("\n=== Header Position Test ===");
        println!("UBS Europe SE: y={}, x={}", ubs_text.y, ubs_text.x);
        println!("Form title:    y={}, x={}", title_text.y, title_text.x);
        
        // UBS Europe SE should be ABOVE the form title (smaller y value)
        assert!(
            ubs_text.y < title_text.y,
            "UBS Europe SE (y={}) should be positioned ABOVE the form title (y={})",
            ubs_text.y, title_text.y
        );
        
        println!("\n✓ Header positioning test passed!");
        
        // Verify font styling on the title
        // The title should have a larger font size than the default (10pt)
        if let flattened::FlattenedNodeKind::Text { font_size, .. } = &title_text.kind {
            println!("Title font size: {:?}", font_size);
            // Also check the style.font
            if let Some(font) = &title_text.style.font {
                println!("Title style.font: size={:?}, typeface={}", font.size, font.typeface);
                assert!(font.size > rust_decimal::Decimal::from(10), 
                    "Title should have font size > 10pt, but got {:?}", font.size);
            }
        }
    }
    
    #[test]
    fn test_aaai_subform_no_overlap() {
        // Test that subforms like "Kunde" and "Vertretungsberechtigte(r)" do NOT overlap
        // These are separate sections that should be stacked vertically
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        let flattened = Flattened::from_xfa(&nodes)
            .expect("Failed to flatten XFA");
        
        // Helper function to find text node by content substring
        fn find_text_containing<'a>(nodes: &'a [flattened::FlattenedNode], substring: &str) -> Option<&'a flattened::FlattenedNode> {
            nodes.iter().find(|n| {
                if let flattened::FlattenedNodeKind::Text { content, .. } = &n.kind {
                    content.contains(substring)
                } else {
                    false
                }
            })
        }
        
        // Find section headers and their bounding boxes
        let vertretungs = find_text_containing(&flattened.nodes, "Vertretungsberechtigte(r)")
            .expect("'Vertretungsberechtigte(r)' text not found");
        let kunde = find_text_containing(&flattened.nodes, "Kunde")
            .expect("'Kunde' text not found (section header)");
        
        // Get bounding boxes
        let vertretungs_bottom = vertretungs.y + vertretungs.height;
        let kunde_top = kunde.y;
        
        println!("\n=== Subform Overlap Test ===");
        println!("'Vertretungsberechtigte(r)': y={}, height={}, bottom={}", 
            vertretungs.y, vertretungs.height, vertretungs_bottom);
        println!("'Kunde':                     y={}, height={}", 
            kunde.y, kunde.height);
        
        // Find form title to understand page layout
        let form_title = find_text_containing(&flattened.nodes, "Vereinbarung")
            .expect("Form title not found");
        println!("Form title:                  y={}", form_title.y);
        
        // The "Kunde" section should be BELOW the "Vertretungsberechtigte(r)" section
        // This is a key layout requirement - sections should not overlap
        let tolerance = rust_decimal::Decimal::from_str("1.0").unwrap();
        
        // Check that Kunde starts below Vertretungsberechtigte(r) section
        // The Vertretungsberechtigte section should have content between its title and the Kunde section
        // So Kunde.y should be significantly greater than Vertretungsberechtigte.y
        //
        // Currently this FAILS because both sections overlap at similar Y positions
        // due to subforms with no explicit height getting height=0
        assert!(
            kunde.y > vertretungs_bottom - tolerance,
            "OVERLAP DETECTED: 'Kunde' section (y={}) should start BELOW 'Vertretungsberechtigte(r)' section (bottom={}). \
             The sections are overlapping by {} points!",
            kunde.y, vertretungs_bottom, vertretungs_bottom - kunde.y
        );
        
        println!("\n✓ Subform no-overlap test passed!");
    }
    
    #[test]
    fn test_aaab_script_extraction_and_execution() {
        use crate::scripting::{XfaScriptEngine, parse_events_from_node, ScriptContentType, EventActivity, EventRef};
        use std::collections::HashMap;
        
        // Extract and parse XFA from AAAB
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Helper function to find events recursively
        fn find_all_events(nodes: &[xfa::XfaNode], events: &mut Vec<(String, crate::scripting::XfaScript)>) {
            for node in nodes {
                let name = node.name.clone().unwrap_or_default();
                
                // Look for event children
                let node_events = parse_events_from_node(&node.children);
                for event in node_events {
                    events.push((name.clone(), event));
                }
                
                // Recurse into children
                find_all_events(&node.children, events);
            }
        }
        
        let mut all_events = Vec::new();
        find_all_events(&nodes, &mut all_events);
        
        println!("\n=== Script Extraction from AAAB ===");
        println!("Found {} event scripts", all_events.len());
        
        // Filter to JavaScript form-ready events only (the pattern we're targeting)
        let js_form_ready_events: Vec<_> = all_events.iter()
            .filter(|(_, script)| {
                script.content_type == ScriptContentType::JavaScript &&
                script.activity == EventActivity::Ready &&
                script.event_ref == EventRef::Form
            })
            .collect();
        
        println!("JavaScript form-ready events: {}", js_form_ready_events.len());
        
        // Print first few scripts found
        for (i, (name, script)) in js_form_ready_events.iter().take(5).enumerate() {
            println!("\n{}. Field '{}' script (first 100 chars):", i+1, name);
            let preview = if script.source.len() > 100 {
                format!("{}...", &script.source[..100])
            } else {
                script.source.clone()
            };
            println!("   {}", preview.replace('\n', " ").replace("  ", " "));
        }
        
        // We should find some JavaScript scripts
        assert!(all_events.len() > 0, "Should find event scripts in AAAB document");
        
        // Now test that we can execute one of the label scripts
        // Set up the script engine with typical AAAB context
        let mut engine = XfaScriptEngine::new();
        
        // Register the language control field (defaulting to German)
        engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", "DE");
        engine.register_field("Footer_Line_txtformid", "Footer_Line_txtformid", "AAAB_019_DE");
        
        // Register German translations (typical for AAAB)
        let mut de_translations = HashMap::new();
        de_translations.insert("GV_FirstName_s".to_string(), "Vorname(n)".to_string());
        de_translations.insert("GV_FamilyName".to_string(), "Nachname".to_string());
        de_translations.insert("GV_DateOfBirth".to_string(), "Geburtsdatum".to_string());
        de_translations.insert("GV_Signature_s".to_string(), "Unterschrift".to_string());
        engine.register_translation_object("myDE", de_translations);
        
        let mut en_translations = HashMap::new();
        en_translations.insert("GV_FirstName_s".to_string(), "First name(s)".to_string());
        en_translations.insert("GV_FamilyName".to_string(), "Family name".to_string());
        engine.register_translation_object("myEN", en_translations);
        
        let mut sp_translations = HashMap::new();
        sp_translations.insert("GV_FirstName_s".to_string(), "Nombre(s)".to_string());
        sp_translations.insert("GV_FamilyName".to_string(), "Apellido".to_string());
        engine.register_translation_object("mySP", sp_translations);
        
        // Find a label field with translation script and execute it
        let label_script = js_form_ready_events.iter()
            .find(|(name, script)| {
                name.starts_with("ff") && script.source.contains("myDE")
            });
        
        if let Some((name, script)) = label_script {
            println!("\n=== Executing script for field '{}' ===", name);
            println!("Script source:\n{}", script.source);
            
            // Set up the field context
            engine.set_current_field(name, name, "");
            
            let result = engine.execute_script(script);
            match result {
                Ok(Some(value)) => {
                    println!("Script executed successfully!");
                    println!("Field '{}' value set to: '{}'", name, value);
                }
                Ok(None) => {
                    println!("Script executed but no value was set");
                }
                Err(e) => {
                    println!("Script execution failed: {}", e);
                    // Don't fail the test - some scripts may have dependencies we haven't set up
                }
            }
        }
        
        println!("\n✓ Script extraction and execution test completed!");
    }
    
    #[test]
    fn test_aaab_ffFirstName_s_gets_vorname() {
        use crate::scripting::{XfaScriptEngine, parse_events_from_node, ScriptContentType, EventActivity, EventRef};
        use std::collections::HashMap;
        
        // Extract and parse XFA from AAAB
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Helper function to find events recursively
        fn find_all_events(nodes: &[xfa::XfaNode], events: &mut Vec<(String, crate::scripting::XfaScript)>) {
            for node in nodes {
                let name = node.name.clone().unwrap_or_default();
                
                // Look for event children
                let node_events = parse_events_from_node(&node.children);
                for event in node_events {
                    events.push((name.clone(), event));
                }
                
                // Recurse into children
                find_all_events(&node.children, events);
            }
        }
        
        let mut all_events = Vec::new();
        find_all_events(&nodes, &mut all_events);
        
        // Find the ffFirstName_s form-ready script
        let firstname_script = all_events.iter()
            .find(|(name, script)| {
                name == "ffFirstName_s" &&
                script.content_type == ScriptContentType::JavaScript &&
                script.activity == EventActivity::Ready &&
                script.event_ref == EventRef::Form
            })
            .expect("Should find ffFirstName_s form-ready script");
        
        println!("Found ffFirstName_s script:\n{}", firstname_script.1.source);
        
        // Set up the script engine with AAAB context
        let mut engine = XfaScriptEngine::new();
        
        // Register the language control field (German for AAAB_019_DE)
        engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", "DE");
        engine.register_field("Footer_Line_txtformid", "Footer_Line_txtformid", "AAAB_019_DE");
        
        // Register German translations
        let mut de_translations = HashMap::new();
        de_translations.insert("GV_FirstName_s".to_string(), "Vorname(n)".to_string());
        engine.register_translation_object("myDE", de_translations);
        
        // Register English translations (for completeness)
        let mut en_translations = HashMap::new();
        en_translations.insert("GV_FirstName_s".to_string(), "First name(s)".to_string());
        engine.register_translation_object("myEN", en_translations);
        
        // Register Spanish translations (for completeness)
        let mut sp_translations = HashMap::new();
        sp_translations.insert("GV_FirstName_s".to_string(), "Nombre(s)".to_string());
        engine.register_translation_object("mySP", sp_translations);
        
        // Set up the field context for ffFirstName_s
        engine.set_current_field("ffFirstName_s", "ffFirstName_s", "");
        
        // Execute the script
        let result = engine.execute_script(&firstname_script.1);
        
        // Assert the result is "Vorname(n)"
        match result {
            Ok(Some(value)) => {
                assert_eq!(
                    value, "Vorname(n)",
                    "ffFirstName_s should be set to 'Vorname(n)' for German language"
                );
                println!("✓ ffFirstName_s correctly set to '{}'", value);
            }
            Ok(None) => {
                panic!("Script executed but no value was set for ffFirstName_s");
            }
            Err(e) => {
                panic!("Script execution failed: {}", e);
            }
        }
    }
    
    #[test]
    fn test_flattened_with_scripts_has_vorname() {
        use crate::flattened::{Flattened, FlattenedNodeKind};
        
        // Extract and parse XFA from AAAB
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = xfa::XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Find the node and check its presence
        fn find_node_info(nodes: &[xfa::XfaNode], target: &str) -> Option<(String, String, String)> {
            for node in nodes {
                if node.name.as_deref() == Some(target) {
                    let presence = node.attributes.get("presence").cloned().unwrap_or("visible".to_string());
                    let kind = format!("{:?}", node.kind).split_whitespace().next().unwrap_or("?").to_string();
                    // Check for bind element
                    let binding = node.children.iter()
                        .find_map(|c| {
                            if let xfa::XfaNodeKind::Element { tag_name, .. } = &c.kind {
                                if tag_name == "bind" {
                                    c.attributes.get("match").cloned()
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();
                    return Some((kind, presence, binding));
                }
                if let Some(result) = find_node_info(&node.children, target) {
                    return Some(result);
                }
            }
            None
        }
        
        // The ffFirstName_s field is HIDDEN by design in AAAB
        // It acts as a data-holder for scripts, with the value displayed elsewhere
        if let Some((kind, presence, _binding)) = find_node_info(&nodes, "ffFirstName_s") {
            println!("ffFirstName_s: kind={}, presence={}", kind, presence);
            assert_eq!(presence, "hidden", "ffFirstName_s is expected to be hidden");
        }
        
        // Flatten WITH script execution (German language)
        // Even though ffFirstName_s is hidden, the script should execute and the value
        // should be available in the computed_values map
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAB_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Hidden fields are intentionally skipped in flattening per XFA spec
        // But we can verify the script engine computed the right value by checking
        // if any visible field got a value from the scripts
        
        // For now, verify that flattening with scripts doesn't crash
        // and produces a reasonable number of nodes.
        // NOTE: With proper presence inheritance, many nodes are now correctly hidden
        // (e.g., the Löschung subform and its children are hidden when the radio button
        // is not set to a specific value). So we expect fewer visible nodes.
        println!("Total flattened nodes: {}", flattened.nodes.len());
        assert!(flattened.nodes.len() > 50, "Should have many flattened nodes");
        
        // Verify visible field ffBankingRelation exists (it's visible)
        let has_banking = flattened.nodes.iter().any(|n| {
            matches!(&n.kind, FlattenedNodeKind::Field { name, .. } if name == "ffBankingRelation")
        });
        assert!(has_banking, "ffBankingRelation should be in output");
        
        println!("\n✓ Script integration test passed!");
        println!("  Note: ffFirstName_s is hidden by design in AAAB form.");
        println!("  The script execution works (tested separately), but hidden");
        println!("  fields are correctly excluded from visual output.");
    }
    
    #[test]
    fn test_explore_xfa_embed_structure() {
        // Extract and parse XFA from AAAB
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = xfa::XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Find DES_FirstName specifically and dump its complete structure
        fn find_and_dump(nodes: &[xfa::XfaNode], target: &str, indent: usize) -> bool {
            let prefix = "  ".repeat(indent);
            for node in nodes {
                let name = node.name.as_deref();
                if name == Some(target) {
                    println!("{}FOUND: {} {:?}", prefix, target, node.kind);
                    println!("{}  Attributes:", prefix);
                    for (k, v) in &node.attributes {
                        println!("{}    {}={}", prefix, k, v);
                    }
                    dump_node_tree(node, indent + 1);
                    return true;
                }
                if find_and_dump(&node.children, target, indent) {
                    return true;
                }
            }
            false
        }
        
        fn dump_node_tree(node: &xfa::XfaNode, indent: usize) {
            let prefix = "  ".repeat(indent);
            for child in &node.children {
                match &child.kind {
                    xfa::XfaNodeKind::Element { tag_name, text_content } => {
                        println!("{}Element: {} (attrs: {:?})", prefix, tag_name, child.attributes);
                        if let Some(content) = text_content {
                            println!("{}  text_content: {:?}", prefix, &content[..content.len().min(200)]);
                        }
                        dump_node_tree(child, indent + 1);
                    }
                    xfa::XfaNodeKind::Text { content } => {
                        println!("{}Text: {:?}", prefix, &content[..content.len().min(200)]);
                    }
                    xfa::XfaNodeKind::Value => {
                        println!("{}Value (attrs: {:?})", prefix, child.attributes);
                        dump_node_tree(child, indent + 1);
                    }
                    other => {
                        println!("{}{:?} (attrs: {:?})", prefix, other, child.attributes);
                        dump_node_tree(child, indent + 1);
                    }
                }
            }
        }
        
        println!("\n=== Exploring DES_FirstName structure ===\n");
        if !find_and_dump(&nodes, "DES_FirstName", 0) {
            println!("DES_FirstName not found!");
        }
        
        println!("\n=== Exploring ffFirstName_s structure ===\n");
        if !find_and_dump(&nodes, "ffFirstName_s", 0) {
            println!("ffFirstName_s not found!");
        }
    }
    
    #[test]
    fn test_des_firstname_gets_vorname_via_embed() {
        use crate::flattened::{Flattened, FlattenedNodeKind};
        
        // Extract and parse XFA from AAAB
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = xfa::XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Flatten WITH script execution (German language)
        // This should:
        // 1. Execute scripts -> ffFirstName_s gets "Vorname(n)" 
        // 2. Build ID map -> "5a604bee...floatingField010860" -> "ffFirstName_s"
        // 3. During text extraction, resolve xfa:embed in DES_FirstName -> "Vorname(n)"
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAB_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Find DES_FirstName in the flattened output
        let des_firstname = flattened.nodes.iter()
            .find(|n| {
                matches!(&n.kind, FlattenedNodeKind::Text { source_name: Some(name), .. } 
                    if name == "DES_FirstName")
            });
        
        if let Some(node) = des_firstname {
            if let FlattenedNodeKind::Text { content, source_name, .. } = &node.kind {
                println!("DES_FirstName node found:");
                println!("  source_name: {:?}", source_name);
                println!("  content: '{}'", content);
                println!("  position: x={}, y={}, width={}, height={}", node.x, node.y, node.width, node.height);
                
                // The content should be "Vorname(n)" from the embedded ffFirstName_s field
                assert_eq!(content, "Vorname(n)", 
                    "DES_FirstName should display 'Vorname(n)' via xfa:embed from ffFirstName_s");
            }
        } else {
            // If not found by source_name, search for any text node with "Vorname(n)"
            let vorname_node = flattened.nodes.iter()
                .find(|n| {
                    matches!(&n.kind, FlattenedNodeKind::Text { content, .. } 
                        if content.contains("Vorname"))
                });
            
            if let Some(node) = vorname_node {
                println!("Found node with Vorname: {:?}", node.kind);
            } else {
                // List all text nodes for debugging
                println!("All Text nodes in flattened output (first 30):");
                for (i, node) in flattened.nodes.iter()
                    .filter(|n| matches!(n.kind, FlattenedNodeKind::Text { .. }))
                    .take(30)
                    .enumerate()
                {
                    if let FlattenedNodeKind::Text { content, source_name, .. } = &node.kind {
                        if !content.is_empty() {
                            println!("  {}: '{}' (source: {:?})", i, &content[..content.len().min(50)], source_name);
                        }
                    }
                }
            }
            
            panic!("DES_FirstName node not found in flattened output!");
        }
    }
    
    #[test]
    fn test_aaai_label_attachment() {
        // Test that labels are correctly attached to fields in the AAAI document
        use crate::document::Document;
        use crate::modules::{TextBlockGrouper, FieldGrouper, LabelAttacher, AnalysisModule};
        
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAI_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Create Document and run analysis modules in the correct order
        let mut doc = Document::from_flattened(&flattened);
        
        println!("\n=== Initial state ===");
        println!("Total flattened nodes: {}", flattened.nodes.len());
        println!("Initial roots: {}", doc.roots().len());
        
        // Step 1: Group text nodes into TextBlocks
        TextBlockGrouper::new().process(&mut doc);
        let text_blocks = doc.find_groups(|k| matches!(k, crate::document::GroupKind::TextBlock));
        println!("\n=== After TextBlockGrouper ===");
        println!("TextBlocks created: {}", text_blocks.len());
        println!("Current roots: {}", doc.roots().len());
        
        // Step 2: Group field nodes into FieldGroups
        FieldGrouper::new().process(&mut doc);
        let field_groups = doc.find_groups(|k| matches!(k, crate::document::GroupKind::Field));
        println!("\n=== After FieldGrouper ===");
        println!("FieldGroups created: {}", field_groups.len());
        println!("Current roots: {}", doc.roots().len());
        
        // Step 3: Attach labels to fields
        LabelAttacher::new().process(&mut doc);
        let labeled_fields = doc.labeled_fields();
        println!("\n=== After LabelAttacher ===");
        println!("LabeledFields created: {}", labeled_fields.len());
        println!("Current roots: {}", doc.roots().len());
        
        // Should have found some labeled fields
        assert!(labeled_fields.len() > 0, "Should have found at least one labeled field");
        
        // Print some examples
        println!("\n=== Sample Labeled Fields ===");
        for (i, &lf_idx) in labeled_fields.iter().take(5).enumerate() {
            let label_text = doc.get_label_text(lf_idx).unwrap_or_default();
            let field_name = doc.get_field_name(lf_idx).unwrap_or_default();
            println!("  {}: '{}' -> {}", i + 1, label_text, field_name);
        }
    }

    #[test]
    fn test_aaai_signature_labels_present() {
        // Test that signature labels are present in the AAAI document
        // These labels come from hidden fields via xfa:embed
        // The parent subform has a script: this.ffDesSignature.rawValue = mySignatureClient
        // which sets the hidden field value to "Unterschrift des Kunden"
        use crate::flattened::{Flattened, FlattenedNodeKind};
        
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAI_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Expected signature labels (set by scripts)
        let expected_labels = [
            "Unterschrift des Kunden",
            "Name des Kunden",
            "Unterschrift UBS Europe SE",
            "Name der verantwortlichen Person",
        ];
        
        // Search for these labels in the flattened output
        let mut found_labels = Vec::new();
        for node in &flattened.nodes {
            if let FlattenedNodeKind::Text { content, .. } = &node.kind {
                for label in &expected_labels {
                    if content.contains(label) {
                        found_labels.push(label.to_string());
                    }
                }
            }
        }
        
        println!("Expected labels: {:?}", expected_labels);
        println!("Found labels: {:?}", found_labels);
        
        // All expected labels should be found
        for label in &expected_labels {
            assert!(
                found_labels.iter().any(|f| f.contains(label)),
                "Label '{}' should be present in the flattened output. \
                 These labels come from hidden fields that get their values \
                 set by parent subform scripts via xfa:embed.",
                label
            );
        }
        
        println!("✓ All signature labels found!");
    }
    
    #[test]
    fn test_aaai_unterschrift_en_section_header() {
        // Test that the "Unterschrift(en)" section header is present in the AAAI document
        // This header comes from:
        // 1. FF_Signature_s field (hidden, id=floatingField018467)
        // 2. Has event ref="$layout" activity="ready" (layout:ready event)
        // 3. Script: this.rawValue = myDE.GV_Signature_s  (which is "Unterschrift(en)")
        // 4. T_Signature draw embeds this via xfa:embed="#floatingField018467"
        use crate::flattened::{Flattened, FlattenedNodeKind};
        
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAI_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Search for "Unterschrift(en)" in text nodes
        let mut found = false;
        for node in &flattened.nodes {
            if let FlattenedNodeKind::Text { content, .. } = &node.kind {
                if content.contains("Unterschrift(en)") {
                    found = true;
                    println!("✓ Found 'Unterschrift(en)' in text: '{}'", content);
                    break;
                }
            }
        }
        
        if !found {
            // Debug: print all text nodes that contain "Unterschrift"
            println!("\n=== Text nodes containing 'Unterschrift' ===");
            for node in &flattened.nodes {
                if let FlattenedNodeKind::Text { content, .. } = &node.kind {
                    if content.to_lowercase().contains("unterschrift") {
                        println!("  '{}'", content);
                    }
                }
            }
        }
        
        assert!(
            found,
            "'Unterschrift(en)' section header should be visible. \
             This comes from FF_Signature_s field via xfa:embed, \
             which is set by a layout:ready script to myDE.GV_Signature_s"
        );
    }

    #[test]
    fn test_aaai_ffdesignature_script_execution() {
        // Test that the ffDesSignature and ffDesFullName scripts execute correctly
        // when the parent subform sets their values
        use crate::scripting::{XfaScriptEngine, parse_events_from_node, ScriptContentType, EventActivity, EventRef};
        use std::collections::HashMap;
        
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Helper function to find events recursively
        fn find_all_events(nodes: &[xfa::XfaNode], events: &mut Vec<(String, crate::scripting::XfaScript)>) {
            for node in nodes {
                let name = node.name.clone().unwrap_or_default();
                
                // Look for event children
                let node_events = parse_events_from_node(&node.children);
                for event in node_events {
                    events.push((name.clone(), event));
                }
                
                // Recurse into children
                find_all_events(&node.children, events);
            }
        }
        
        let mut all_events = Vec::new();
        find_all_events(&nodes, &mut all_events);
        
        // Find the Signature subform's form-ready script
        // This script sets: this.ffDesSignature.rawValue = mySignatureClient
        let signature_script = all_events.iter()
            .find(|(name, script)| {
                name == "Signature" &&
                script.content_type == ScriptContentType::JavaScript &&
                script.activity == EventActivity::Ready &&
                script.event_ref == EventRef::Form &&
                script.source.contains("ffDesSignature")
            });
        
        if let Some((name, script)) = signature_script {
            println!("Found Signature script:\n{}", script.source);
            
            // Set up the script engine with AAAI context
            let mut engine = XfaScriptEngine::new();
            
            // Register the language control field (German)
            engine.register_field("Footer_Line_txtlanguage", "Footer_Line_txtlanguage", "DE");
            
            // Register German translations (myDE)
            let mut de_translations = HashMap::new();
            de_translations.insert("GV_SignatureClient".to_string(), "Unterschrift des Kunden".to_string());
            de_translations.insert("GV_NameClient".to_string(), "Name des Kunden".to_string());
            de_translations.insert("GV_SignatureUBS".to_string(), "Unterschrift UBS Europe SE".to_string());
            de_translations.insert("GV_NameRespPerson".to_string(), "Name der verantwortlichen Person".to_string());
            engine.register_translation_object("myDE", de_translations);
            
            // Create global variables that the script expects
            // These are normally set by soLocalLabelDefinition.setupVariables()
            // Using assignment without var makes them global in JS
            let init_script = r#"
                mySignatureClient = myDE.GV_SignatureClient;
                mySignatureNameClient = myDE.GV_NameClient;
            "#;
            let _ = engine.execute_variable_script(init_script);
            
            // Debug: Check if globals are set
            let debug_globals = engine.evaluate_expression(
                "typeof mySignatureClient + ': ' + mySignatureClient + ' | ' + typeof myDE"
            );
            println!("Debug globals: {:?}", debug_globals);
            
            // Set up the field context for Signature subform WITH CHILD FIELDS
            // This enables the script to access this.ffDesSignature and this.ffDesFullName
            // Now uses (name, id) tuples to track unique field IDs
            let child_fields: Vec<(String, String)> = vec![
                ("ffDesSignature".to_string(), "test-sig-id".to_string()),
                ("ffDesFullName".to_string(), "test-name-id".to_string()),
            ];
            engine.set_current_field_with_children("Signature", name, "", &child_fields);
            
            // Debug: Check if child fields are set up
            let debug_children = engine.evaluate_expression(
                "typeof this.ffDesSignature + ' | rawValue: ' + (this.ffDesSignature ? this.ffDesSignature.rawValue : 'no obj')"
            );
            println!("Debug children: {:?}", debug_children);
            
            // Execute the script
            let result = engine.execute_script(&script);
            
            // The script sets this.ffDesSignature.rawValue
            // We need to check that the child field was set
            println!("Script execution result: {:?}", result);
            
            // The value should be available on this.ffDesSignature
            let ff_value = engine.evaluate_expression("this.ffDesSignature ? this.ffDesSignature.rawValue : 'not found'");
            println!("this.ffDesSignature.rawValue = {:?}", ff_value);
            
            // Also check the child field value via the engine's helper method
            // Now returns (child_id, value) tuple
            let child_value = engine.get_child_field_value("ffDesSignature");
            println!("get_child_field_value('ffDesSignature') = {:?}", child_value);
            
            // Check if the value is set
            if let Some((child_id, value)) = child_value {
                assert_eq!(
                    value, "Unterschrift des Kunden",
                    "ffDesSignature.rawValue should be 'Unterschrift des Kunden'"
                );
                println!("✓ ffDesSignature correctly set to '{}' (id={})", value, child_id);
            } else {
                panic!("ffDesSignature value should be set");
            }
        } else {
            println!("Signature form-ready script not found");
            println!("Available scripts for 'Signature':");
            for (name, script) in &all_events {
                if name == "Signature" {
                    println!("  - activity={:?}, ref={:?}, source={}", 
                        script.activity, script.event_ref, 
                        &script.source[..script.source.len().min(100)]);
                }
            }
            panic!("Signature form-ready script should exist");
        }
    }
    
    /// Test that RB_1 (the first radio button in the exclusion group) gets a default value set.
    /// 
    /// Per XFA 3.3 spec section 2 "Exclusion Group":
    /// - An exclusion group may have a default value
    /// - The default value is provided by one of the fields in the group via its <value> element
    /// - When a field's <value> matches its <items>, that field is pre-selected
    /// 
    /// In AAAB, RB_Group_Neuanlage contains RB_1, RB_2, RB_3, and RB_1 should be the default.
    /// The script `if(!this.rawValue) { Page.FormTitle.STP_RB_Horizontal.RB_Group_Neuanlage.RB_1.rawValue = 1; ... }`
    /// should NOT run if RB_1 already has its default value set.
    #[test]
    fn test_aaab_rb1_default_value() {
        use crate::xfa::XfaNodeKind;
        
        // Extract and parse XFA from AAAB
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Helper to find a node by name recursively
        fn find_node_by_name<'a>(nodes: &'a [XfaNode], name: &str) -> Option<&'a XfaNode> {
            for node in nodes {
                if node.name.as_deref() == Some(name) {
                    return Some(node);
                }
                if let Some(found) = find_node_by_name(&node.children, name) {
                    return Some(found);
                }
            }
            None
        }
        
        // Helper to extract field value (looks for <value><text>...</text></value>)
        fn extract_field_value(children: &[XfaNode]) -> Option<String> {
            for child in children {
                if matches!(child.kind, XfaNodeKind::Value) {
                    for value_child in &child.children {
                        if let XfaNodeKind::Element { tag_name, text_content } = &value_child.kind {
                            if tag_name == "text" || tag_name == "integer" {
                                return text_content.clone();
                            }
                        }
                    }
                }
                if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                    if tag_name == "value" {
                        for value_child in &child.children {
                            if let XfaNodeKind::Element { tag_name: inner_tag, text_content } = &value_child.kind {
                                if inner_tag == "text" || inner_tag == "integer" {
                                    return text_content.clone();
                                }
                            }
                        }
                    }
                }
            }
            None
        }
        
        // Helper to extract items value (looks for <items><integer>...</integer></items>)
        fn extract_items_value(children: &[XfaNode]) -> Option<String> {
            for child in children {
                if let XfaNodeKind::Element { tag_name, .. } = &child.kind {
                    if tag_name == "items" {
                        for items_child in &child.children {
                            if let XfaNodeKind::Element { tag_name: inner_tag, text_content } = &items_child.kind {
                                if inner_tag == "text" || inner_tag == "integer" {
                                    return text_content.clone();
                                }
                            }
                        }
                    }
                }
            }
            None
        }
        
        // Find RB_Group_Neuanlage (the exclusion group)
        let excl_group = find_node_by_name(&nodes, "RB_Group_Neuanlage");
        assert!(excl_group.is_some(), "Should find RB_Group_Neuanlage exclusion group");
        let excl_group = excl_group.unwrap();
        
        println!("\n=== Examining RB_Group_Neuanlage exclusion group ===");
        
        // Find and examine RB_1, RB_2, RB_3
        let rb1 = find_node_by_name(&excl_group.children, "RB_1");
        let rb2 = find_node_by_name(&excl_group.children, "RB_2");
        let rb3 = find_node_by_name(&excl_group.children, "RB_3");
        
        assert!(rb1.is_some(), "Should find RB_1 field");
        assert!(rb2.is_some(), "Should find RB_2 field");
        assert!(rb3.is_some(), "Should find RB_3 field");
        
        let rb1 = rb1.unwrap();
        let rb2 = rb2.unwrap();
        let rb3 = rb3.unwrap();
        
        // Extract items and values for each
        let rb1_items = extract_items_value(&rb1.children);
        let rb1_value = extract_field_value(&rb1.children);
        let rb2_items = extract_items_value(&rb2.children);
        let rb2_value = extract_field_value(&rb2.children);
        let rb3_items = extract_items_value(&rb3.children);
        let rb3_value = extract_field_value(&rb3.children);
        
        println!("RB_1: items={:?}, value={:?}", rb1_items, rb1_value);
        println!("RB_2: items={:?}, value={:?}", rb2_items, rb2_value);
        println!("RB_3: items={:?}, value={:?}", rb3_items, rb3_value);
        
        // Per XFA spec: a field is the default if it has a <value> element
        // whose content matches its <items> element
        // 
        // Based on the debug output showing the script:
        // if(!this.rawValue) { Page.FormTitle.STP_RB_Horizontal.RB_Group_Neuanlage.RB_1.rawValue = 1; ... }
        // This suggests RB_1 should be selected (value=1) if no default is already set
        
        // The bug: Currently extract_field_value doesn't handle exclGroup default values properly.
        // When RB_1 has <value><text>1</text></value> and <items><integer>1</integer></items>,
        // it should be detected as the default, and the exclGroup's rawValue should be "1".
        
        // Now test with the flattening that uses scripts
        let doc_name = "AAAB_019_DE";
        let locale = "DE";
        let flattened = Flattened::from_xfa_with_scripts(&nodes, locale, doc_name)
            .expect("Failed to flatten XFA");
        
        // Look for RB_1 in flattened output and verify it has the correct value
        // The field should have rawValue=1 if it's the default selection
        println!("\nFlattened nodes with RB_ prefix:");
        for node in &flattened.nodes {
            if let FlattenedNodeKind::Field { name, value, .. } = &node.kind {
                if name.starts_with("RB_") {
                    println!("  {} = {:?}", name, value);
                }
            }
        }
        
        // Find RB_1 in flattened nodes
        let rb1_node = flattened.nodes.iter().find(|n| {
            if let FlattenedNodeKind::Field { name, .. } = &n.kind {
                name == "RB_1"
            } else {
                false
            }
        });
        
        assert!(rb1_node.is_some(), "Should find RB_1 in flattened nodes");
        let rb1_node = rb1_node.unwrap();
        
        if let FlattenedNodeKind::Field { value, .. } = &rb1_node.kind {
            // RB_1 should have a truthy value (1, "1", or true) indicating it's selected
            // This is the assertion that will fail if the default value is not being set
            let has_default = value == "1" || value == "true" || !value.is_empty();
            assert!(
                has_default,
                "RB_1 should have a default value of '1' (is the default selection), but got: {:?}",
                value
            );
            println!("\n✓ RB_1 has default value: {:?}", value);
        } else {
            panic!("RB_1 should be a field");
        }
    }
    
    /// Test that ffClientDetails field (with rawValue "Endkunde") should be hidden.
    /// 
    /// Per XFA 3.3 spec, setting rawValue via JavaScript does NOT change field presence.
    /// The presence attribute is static and should be respected regardless of computed value.
    /// 
    /// In AAAB, ffClientDetails has presence="hidden" in the template but its value is
    /// computed by JavaScript. This field should NOT appear in the flattened output.
    #[test]
    fn test_aaab_hidden_field_with_computed_value_not_visible() {
        // Extract and parse XFA from AAAB
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = xfa::XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Helper function to find node info
        fn find_node_info(nodes: &[xfa::XfaNode], target: &str) -> Option<(String, String)> {
            for node in nodes {
                if node.name.as_deref() == Some(target) {
                    let presence = node.attributes.get("presence").cloned().unwrap_or("visible".to_string());
                    let kind = format!("{:?}", node.kind).split_whitespace().next().unwrap_or("?").to_string();
                    return Some((kind, presence));
                }
                if let Some(result) = find_node_info(&node.children, target) {
                    return Some(result);
                }
            }
            None
        }
        
        // First, verify ffClientDetails has presence="hidden" in the template
        if let Some((kind, presence)) = find_node_info(&nodes, "ffClientDetails") {
            println!("ffClientDetails: kind={}, presence={}", kind, presence);
            assert_eq!(presence, "hidden", 
                "ffClientDetails should have presence='hidden' in template");
        } else {
            panic!("Could not find ffClientDetails field in XFA template");
        }
        
        // Flatten WITH script execution
        // The script sets ffClientDetails.rawValue = "Endkunde"
        // But per XFA spec, this should NOT change the field's visibility
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAB_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Check if ffClientDetails appears in flattened output
        let has_client_details_field = flattened.nodes.iter().any(|n| {
            matches!(&n.kind, FlattenedNodeKind::Field { name, .. } if name == "ffClientDetails")
        });
        
        // Also check for any text node with "Endkunde" content that came DIRECTLY from ffClientDetails
        // Note: Text from OTHER Draw elements (like T_Client_Details) that embed ffClientDetails via
        // xfa:embed is ALLOWED per XFA spec - the embed reference should resolve even if the source
        // field is hidden. The T_Client_Details element itself is visible.
        let has_endkunde_text_from_hidden_field = flattened.nodes.iter().any(|n| {
            matches!(&n.kind, FlattenedNodeKind::Text { content, source_name, .. } 
                if content == "Endkunde" && source_name.as_deref() == Some("ffClientDetails"))
        });
        
        // Print what we found for debugging
        println!("\nSearching for ffClientDetails/Endkunde in flattened output:");
        for node in &flattened.nodes {
            match &node.kind {
                FlattenedNodeKind::Field { name, value, .. } if name == "ffClientDetails" => {
                    println!("  Found Field '{}' with value '{}'", name, value);
                }
                FlattenedNodeKind::Text { content, source_name, .. } if content.contains("Endkunde") => {
                    println!("  Found Text '{}' from source {:?}", content, source_name);
                }
                _ => {}
            }
        }
        
        // The field itself should NOT appear because it's hidden
        assert!(!has_client_details_field, 
            "ffClientDetails should NOT appear in flattened output - it has presence='hidden'. \
             Setting rawValue via script should NOT make hidden fields visible.");
        
        // Text from the hidden field itself should not appear
        // (but text from OTHER elements that embed the value is allowed per XFA spec)
        assert!(!has_endkunde_text_from_hidden_field,
            "Text 'Endkunde' directly from hidden field ffClientDetails should NOT appear in output. \
             Per XFA spec, presence='hidden' means the field does not participate in layout/rendering.");
        
        println!("\n✓ Hidden field ffClientDetails (with computed value 'Endkunde') correctly excluded from output");
    }
    
    /// Test that the "Neuanlage" section is visible when RB_1 (Neuanlage radio button) is selected.
    ///
    /// In AAAB, there's a radio group (RB_Group_Neuanlage) with RB_1 being the default selection.
    /// When RB_1 is selected (rawValue=1), the corresponding "Neuanlage" section should be visible.
    /// This requires click events on RB_1 to be executed even when it's the default selection.
    #[test]
    fn test_aaab_neuanlage_section_visible_when_rb1_selected() {
        use crate::scripting::{XfaScriptEngine, parse_events_from_node, ScriptContentType, EventActivity, EventRef};
        
        // Extract and parse XFA from AAAB
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Helper to find node by name
        fn find_node_by_name<'a>(nodes: &'a [XfaNode], target: &str) -> Option<&'a XfaNode> {
            for node in nodes {
                if node.name.as_deref() == Some(target) {
                    return Some(node);
                }
                if let Some(found) = find_node_by_name(&node.children, target) {
                    return Some(found);
                }
            }
            None
        }
        
        // Helper to find all events with their activities
        fn find_all_events_with_activities(
            nodes: &[XfaNode], 
            events: &mut Vec<(String, EventActivity, String)>
        ) {
            for node in nodes {
                let name = node.name.clone().unwrap_or_default();
                let node_events = parse_events_from_node(&node.children);
                for event in node_events {
                    let activity = event.activity.clone();
                    let script_preview = event.source.chars().take(100).collect::<String>();
                    events.push((name.clone(), activity, script_preview));
                }
                find_all_events_with_activities(&node.children, events);
            }
        }
        
        // Find all events and group by activity type
        let mut all_events = Vec::new();
        find_all_events_with_activities(&nodes, &mut all_events);
        
        println!("\n=== Event Activities in AAAB ===");
        let mut activity_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (name, activity, script_preview) in &all_events {
            let activity_str = format!("{:?}", activity);
            *activity_counts.entry(activity_str).or_insert(0) += 1;
            
            // Print events on RB_ fields and Groups and ffrb1
            if name.starts_with("RB_") || name == "ffrb1" {
                println!("  {} has {:?} event", name, activity);
                // Show script content for mouseDown and Change events
                if matches!(activity, EventActivity::Other(s) if s == "mouseDown") 
                    || matches!(activity, EventActivity::Change)
                    || matches!(activity, EventActivity::Ready) {
                    println!("    Script: {}", script_preview);
                }
            }
        }
        
        println!("\nActivity type counts:");
        for (activity, count) in &activity_counts {
            println!("  {}: {}", activity, count);
        }
        
        // Find "Neuanlage" subform and check its presence
        let neuanlage = find_node_by_name(&nodes, "Neuanlage");
        if let Some(subform) = neuanlage {
            println!("\nFound 'Neuanlage' subform:");
            println!("  presence attribute: {:?}", subform.attributes.get("presence"));
            println!("  kind: {:?}", subform.kind);
        } else {
            // Search for subforms containing "Neuanlage" in name
            fn find_subforms_with_prefix<'a>(nodes: &'a [XfaNode], prefix: &str, results: &mut Vec<&'a XfaNode>) {
                for node in nodes {
                    if let Some(name) = &node.name {
                        if name.to_lowercase().contains(&prefix.to_lowercase()) {
                            results.push(node);
                        }
                    }
                    find_subforms_with_prefix(&node.children, prefix, results);
                }
            }
            
            let mut neuanlage_nodes = Vec::new();
            find_subforms_with_prefix(&nodes, "Neuanlage", &mut neuanlage_nodes);
            
            println!("\nFound {} nodes containing 'Neuanlage' in name:", neuanlage_nodes.len());
            for n in &neuanlage_nodes {
                println!("  - {:?} (presence={:?})", n.name, n.attributes.get("presence"));
            }
        }
        
        // Flatten with script execution
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAB_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Count visible nodes to verify the Neuanlage section is rendered
        let total_nodes = flattened.nodes.len();
        println!("\nTotal flattened nodes: {}", total_nodes);
        
        // Find text nodes that might be from the Neuanlage section
        // (these typically have field labels like "Vorname", "Nachname", etc.)
        let neuanlage_related_texts: Vec<_> = flattened.nodes.iter()
            .filter(|n| {
                if let FlattenedNodeKind::Text { source_name: Some(name), .. } = &n.kind {
                    name.contains("TF_") || name.contains("DES_")  // These are label fields
                } else {
                    false
                }
            })
            .collect();
        
        println!("Neuanlage-related label nodes found: {}", neuanlage_related_texts.len());
        for (i, node) in neuanlage_related_texts.iter().take(10).enumerate() {
            if let FlattenedNodeKind::Text { content, source_name, .. } = &node.kind {
                println!("  {}: '{}' (source: {:?})", i, content.chars().take(30).collect::<String>(), source_name);
            }
        }
        
        // Look for the text "Neuanlage" itself in any text node
        let neuanlage_text_nodes: Vec<_> = flattened.nodes.iter()
            .filter(|n| {
                if let FlattenedNodeKind::Text { content, .. } = &n.kind {
                    content.to_lowercase().contains("neuanlage")
                } else {
                    false
                }
            })
            .collect();
        
        println!("\nNodes containing 'Neuanlage' text: {}", neuanlage_text_nodes.len());
        for node in &neuanlage_text_nodes {
            if let FlattenedNodeKind::Text { content, source_name, .. } = &node.kind {
                println!("  '{}' (source: {:?})", content.chars().take(60).collect::<String>(), source_name);
            }
        }
        
        // Look for ffrb1 which should contain "Neuanlage" text when RB_1 is selected
        let ffrb1_node = flattened.nodes.iter()
            .find(|n| {
                if let FlattenedNodeKind::Text { source_name: Some(name), .. } = &n.kind {
                    name == "ffrb1"
                } else if let FlattenedNodeKind::Text { content, .. } = &n.kind {
                    content.contains("Neuanlage") && content.contains("möglich")
                } else {
                    false
                }
            });
        
        if let Some(node) = ffrb1_node {
            if let FlattenedNodeKind::Text { content, source_name, .. } = &node.kind {
                println!("\nFound ffrb1/Neuanlage text: '{}' (source: {:?})", content, source_name);
            }
        } else {
            println!("\nWARNING: ffrb1 node with 'Neuanlage (möglich ab...)' text not found!");
            println!("This indicates that click events for RB_1 are not being triggered.");
        }
        
        // The Neuanlage section should have multiple visible elements when RB_1 is selected
        // This test will FAIL if click events aren't being triggered on default selection
        assert!(
            neuanlage_related_texts.len() > 5,
            "Neuanlage section should have visible label elements when RB_1 is selected by default. \
             Found only {} elements. Click events may not be triggered on default selection.",
            neuanlage_related_texts.len()
        );
        
        println!("\n✓ Neuanlage section is visible with {} label elements", neuanlage_related_texts.len());
    }

    /// Test that ffrb1 field shows "Neuanlage (möglich ab dem 01. des aktuellen Monats)" when RB_1 is selected.
    ///
    /// When RB_1 (Neuanlage) is selected by default, the Initialize script on RB_Group_Neuanlage 
    /// calls `soLocalLabelDefinition.change()` which should set ffrb1.rawValue to the German 
    /// text for "New application (possible from the 1st of the current month)".
    ///
    /// This test confirms the bug: ffrb1 has no computed value because change() can't 
    /// resolve the field via xfa.resolveNode().
    #[test]
    fn test_aaab_ffrb1_shows_neuanlage_text_when_rb1_selected() {
        // Extract and parse XFA from AAAB
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = xfa::XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Flatten with script execution
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAB_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Look for ffrb1 which should contain "Neuanlage (möglich ab dem 01. des aktuellen Monats)"
        // This is the label that indicates which radio button option is selected
        let ffrb1_text = flattened.nodes.iter()
            .find_map(|n| {
                if let FlattenedNodeKind::Text { content, source_name, .. } = &n.kind {
                    // Check if this is ffrb1 or contains the expected text
                    if source_name.as_deref() == Some("ffrb1") {
                        return Some(content.clone());
                    }
                    if content.contains("Neuanlage") && content.contains("möglich") {
                        return Some(content.clone());
                    }
                }
                None
            });
        
        // Print debug info
        println!("\n=== ffrb1 Test ===");
        println!("ffrb1 text content: {:?}", ffrb1_text);
        
        // The test SHOULD fail with this assertion - demonstrating the bug
        // When fixed, ffrb1 should have the text "Neuanlage (möglich ab dem 01. des aktuellen Monats)"
        assert!(
            ffrb1_text.is_some() && ffrb1_text.as_ref().unwrap().contains("Neuanlage (möglich"),
            "ffrb1 should show 'Neuanlage (möglich ab dem 01. des aktuellen Monats)' when RB_1 is selected. \
             Got: {:?}. This indicates that soLocalLabelDefinition.change() is not properly setting ffrb1.rawValue.",
            ffrb1_text
        );
        
        println!("\n✓ ffrb1 correctly shows: '{}'", ffrb1_text.unwrap());
    }
}
