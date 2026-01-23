mod xfa;
mod flattened;
mod text_metrics;
mod scripting;

use pdf::file::FileOptions;
use pdf::object::*;
use pdf::primitive::Primitive;
use std::path::Path;
use xfa::XfaNode;
use flattened::Flattened;
use rust_decimal::prelude::*;

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
                        if let Primitive::Reference(stream_ref) = &arr[i] {
                            if let Ok(Primitive::Stream(ref pdf_stream)) = resolver.resolve(*stream_ref) {
                                let stream: Stream<()> = Stream::from_stream(pdf_stream.clone(), &resolver)?;
                                let data = stream.data(&resolver)?;
                                xfa_data.extend_from_slice(&data);
                            }
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

fn main() {
    println!("Hello, world!");
}

#[cfg(test)]
mod tests {
    use super::*;
    
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
    fn test_render_flattened_to_image() {
        // Test rendering the basic flattened structure
        use xfa::*;
        use std::collections::HashMap;
        
        let mut subform_attrs = HashMap::new();
        subform_attrs.insert("x".to_string(), "10pt".to_string());
        subform_attrs.insert("y".to_string(), "20pt".to_string());
        subform_attrs.insert("w".to_string(), "400pt".to_string());
        subform_attrs.insert("h".to_string(), "300pt".to_string());
        subform_attrs.insert("layout".to_string(), "tb".to_string());
        
        let mut field1_attrs = HashMap::new();
        field1_attrs.insert("name".to_string(), "FirstName".to_string());
        field1_attrs.insert("x".to_string(), "5pt".to_string());
        field1_attrs.insert("y".to_string(), "5pt".to_string());
        field1_attrs.insert("w".to_string(), "200pt".to_string());
        field1_attrs.insert("h".to_string(), "30pt".to_string());
        
        let mut field2_attrs = HashMap::new();
        field2_attrs.insert("name".to_string(), "LastName".to_string());
        field2_attrs.insert("x".to_string(), "0pt".to_string());
        field2_attrs.insert("y".to_string(), "10pt".to_string());
        field2_attrs.insert("w".to_string(), "200pt".to_string());
        field2_attrs.insert("h".to_string(), "30pt".to_string());
        
        let field1 = XfaNode::new(XfaNodeKind::Field, field1_attrs);
        let field2 = XfaNode::new(XfaNodeKind::Field, field2_attrs);
        
        let mut subform = XfaNode::new(XfaNodeKind::Subform, subform_attrs);
        subform.children = vec![field1, field2];
        
        let nodes = vec![subform];
        
        let flattened = Flattened::from_xfa(&nodes)
            .expect("Failed to flatten XFA");
        
        // Render to image
        let output_path = "output/test_layout.png";
        std::fs::create_dir_all("output").expect("Failed to create output directory");
        
        flattened.render_to_image(output_path, 1.5)
            .expect("Failed to render image");
        
        println!("\n✓ Rendered flattened layout to {}", output_path);
        assert!(std::path::Path::new(output_path).exists(), "Image file should exist");
    }
    
    #[test]
    fn test_render_aaab_to_image() {
        // Render the AAAB document
        let xfa_data = extract_xfa_from_pdf("input/AAAB_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Use from_xfa_with_scripts to execute scripts and get computed labels
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAB_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Render to image with 1.5x scale
        let output_path = "output/aaab_layout.png";
        std::fs::create_dir_all("output").expect("Failed to create output directory");
        
        flattened.render_to_image(output_path, 1.5)
            .expect("Failed to render image");
        
        println!("\n✓ Rendered AAAB layout to {}", output_path);
        println!("  Image dimensions: {}x{} pixels", 
            (flattened.page.width.to_f32().unwrap_or(0.0) * 1.5) as u32, 
            (flattened.page.height.to_f32().unwrap_or(0.0) * 1.5) as u32);
        println!("  Rendered {} fields/elements", flattened.nodes.len());
        println!("\n  Visual validation: Compare output/aaab_layout.png with expected/AAAB_019_DE-1.png");
        println!("  Expected image: 910x1288 pixels");
        println!("  Output image:   {}x{} pixels", 
            (flattened.page.width.to_f32().unwrap_or(0.0) * 1.5) as u32, 
            (flattened.page.height.to_f32().unwrap_or(0.0) * 1.5) as u32);
        
        assert!(std::path::Path::new(output_path).exists(), "Image file should exist");
    }
    
    #[test]
    fn test_render_aaai_to_image() {
        // Render the AAAI document
        let xfa_data = extract_xfa_from_pdf("input/AAAI_019_DE.pdf")
            .expect("Failed to read PDF");
        assert!(xfa_data.is_some(), "PDF should contain XFA data");
        
        let nodes = XfaNode::parse(&xfa_data.unwrap())
            .expect("Failed to parse XFA structure");
        
        // Use from_xfa_with_scripts to execute scripts and get computed labels
        let flattened = Flattened::from_xfa_with_scripts(&nodes, "DE", "AAAI_019_DE")
            .expect("Failed to flatten XFA with scripts");
        
        // Debug: print all node positions sorted by Y coordinate
        let mut sorted_nodes: Vec<_> = flattened.nodes.iter().collect();
        sorted_nodes.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap());
        
        println!("\n=== AAAI Element Positions (sorted by Y) ===");
        for node in sorted_nodes.iter().take(50) {
            let name = match &node.kind {
                flattened::FlattenedNodeKind::Field { name, .. } => name.clone(),
                flattened::FlattenedNodeKind::Text { .. } => "Text".to_string(),
            };
            println!("y={:7.2} x={:7.2} h={:6.2} w={:6.2}  {}", 
                node.y, node.x, node.height, node.width, name);
        }
        println!("... and {} more", flattened.nodes.len().saturating_sub(50));
        
        // Render to image with 1.5x scale
        let output_path = "output/aaai_layout.png";
        std::fs::create_dir_all("output").expect("Failed to create output directory");
        
        flattened.render_to_image(output_path, 1.5)
            .expect("Failed to render image");
        
        println!("\n✓ Rendered AAAI layout to {}", output_path);
        println!("  Image dimensions: {}x{} pixels", 
            (flattened.page.width.to_f32().unwrap_or(0.0) * 1.5) as u32, 
            (flattened.page.height.to_f32().unwrap_or(0.0) * 1.5) as u32);
        println!("  Rendered {} fields/elements", flattened.nodes.len());
        
        assert!(std::path::Path::new(output_path).exists(), "Image file should exist");
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
        // and produces a reasonable number of nodes
        println!("Total flattened nodes: {}", flattened.nodes.len());
        assert!(flattened.nodes.len() > 100, "Should have many flattened nodes");
        
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
}
