//! XML serialization of an `AemNode` tree into AEM JCR content XML.
//!
//! Uses `quick_xml::Writer` for well-formed, properly escaped output.

use std::io::Cursor;

use quick_xml::Writer;
use quick_xml::events::{BytesEnd, BytesStart, Event};

use super::{AemConfig, AemNode, AemOption, OptionAlignment};

// ============================================================================
// Public API
// ============================================================================

/// Serialize an `AemNode` tree (starting from `Root`) into a complete AEM
/// JCR content XML string.
///
/// Attributes are formatted one-per-line (matching AEM's export style).
pub fn generate_aem_xml(root: &AemNode, config: &AemConfig) -> String {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut w = Writer::new_with_indent(&mut buf, b' ', 4);

        // XML declaration
        w.write_event(Event::Decl(quick_xml::events::BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            None,
        )))
        .expect("write xml decl");

        write_node(&mut w, root, config);
    }
    let raw = String::from_utf8(buf.into_inner()).expect("UTF-8 xml output");
    reformat_attributes(&raw)
}

// ============================================================================
// Node writers
// ============================================================================

fn write_node(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    match node {
        AemNode::Root { .. } => write_root(w, node, config),
        AemNode::Panel { .. } => write_panel(w, node, config),
        AemNode::TextField { .. } => write_text_field(w, node, config),
        AemNode::NumberField { .. } => write_number_field(w, node, config),
        AemNode::DatePicker { .. } => write_date_picker(w, node, config),
        AemNode::Dropdown { .. } => write_dropdown(w, node, config),
        AemNode::Checkbox { .. } => write_checkbox(w, node, config),
        AemNode::RadioButton { .. } => write_radio_button(w, node, config),
        AemNode::TextDraw { .. } => write_text_draw(w, node, config),
        AemNode::TextBoxMultiline { .. } => write_text_box_multiline(w, node, config),
        AemNode::Repeatable { .. } => write_repeatable(w, node, config),
    }
}

// ============================================================================
// Root
// ============================================================================

fn write_root(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    let AemNode::Root { title, children } = node else {
        return;
    };

    if config.include_page_wrapper {
        // <jcr:root> with namespace declarations
        let mut root_elem = BytesStart::new("jcr:root");
        root_elem.push_attribute(("xmlns:sling", "http://sling.apache.org/jcr/sling/1.0"));
        root_elem.push_attribute(("xmlns:fd", "http://www.adobe.com/aemfd/fd/1.0"));
        root_elem.push_attribute(("xmlns:cq", "http://www.day.com/jcr/cq/1.0"));
        root_elem.push_attribute(("xmlns:jcr", "http://www.jcp.org/jcr/1.0"));
        root_elem.push_attribute(("xmlns:nt", "http://www.jcp.org/jcr/nt/1.0"));
        root_elem.push_attribute(("jcr:primaryType", "cq:Page"));
        w.write_event(Event::Start(root_elem)).unwrap();

        // <jcr:content>
        let mut content_elem = BytesStart::new("jcr:content");
        content_elem.push_attribute(("jcr:primaryType", "cq:PageContent"));
        content_elem.push_attribute(("jcr:title", title.as_str()));
        content_elem.push_attribute((
            "sling:resourceType",
            config.page_resource_type.as_str(),
        ));
        content_elem.push_attribute(("cq:template", config.template_path.as_str()));
        content_elem.push_attribute(("jcr:language", config.master_language.as_str()));
        w.write_event(Event::Start(content_elem)).unwrap();

        // <guideContainer>
        write_guide_container(w, title, children, config);

        // </jcr:content>
        w.write_event(Event::End(BytesEnd::new("jcr:content")))
            .unwrap();
        // </jcr:root>
        w.write_event(Event::End(BytesEnd::new("jcr:root")))
            .unwrap();
    } else {
        // No page wrapper — emit rootPanel directly
        write_root_panel(w, title, children, config);
    }
}

fn write_guide_container(
    w: &mut Writer<&mut Cursor<Vec<u8>>>,
    title: &str,
    children: &[AemNode],
    config: &AemConfig,
) {
    let mut elem = BytesStart::new("guideContainer");
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute((
        "sling:resourceType",
        config.guide_container_resource_type().as_str(),
    ));
    elem.push_attribute(("guideNodeClass", "guideContainerNode"));
    elem.push_attribute(("fd:version", "2.1"));
    elem.push_attribute(("dorType", config.dor_type.as_str()));
    elem.push_attribute(("guideCss", "guideContainer"));
    elem.push_attribute(("name", "guide1"));
    elem.push_attribute(("textIsRich", "true"));
    if !config.theme_ref.is_empty() {
        elem.push_attribute(("themeRef", config.theme_ref.as_str()));
    }
    if !config.dor_template_ref.is_empty() {
        elem.push_attribute(("dorTemplateRef", config.dor_template_ref.as_str()));
    }
    if !config.redirect_url.is_empty() {
        elem.push_attribute(("redirect", config.redirect_url.as_str()));
    }
    w.write_event(Event::Start(elem)).unwrap();

    // <layout>
    {
        let mut layout = BytesStart::new("layout");
        layout.push_attribute(("jcr:primaryType", "nt:unstructured"));
        layout.push_attribute(("sling:resourceType", "fd/af/layouts/defaultGuideLayout"));
        layout.push_attribute(("mobileLayout", "fd/af/layouts/mobile/step"));
        layout.push_attribute(("toolbarPosition", "Bottom"));
        w.write_event(Event::Empty(layout)).unwrap();
    }

    // <rootPanel>
    write_root_panel(w, title, children, config);

    // </guideContainer>
    w.write_event(Event::End(BytesEnd::new("guideContainer")))
        .unwrap();
}

fn write_root_panel(
    w: &mut Writer<&mut Cursor<Vec<u8>>>,
    title: &str,
    children: &[AemNode],
    config: &AemConfig,
) {
    let mut elem = BytesStart::new("rootPanel");
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute(("sling:resourceType", "fd/af/components/rootPanel"));
    elem.push_attribute(("guideNodeClass", "rootPanelNode"));
    elem.push_attribute(("jcr:title", title));
    elem.push_attribute(("name", "guideRootPanel"));
    elem.push_attribute(("textIsRich", "true"));
    elem.push_attribute(("completionExpReq", "{Boolean}true"));
    elem.push_attribute(("dorExcludeDescription", "true"));
    elem.push_attribute(("dorExcludeTitle", "true"));
    elem.push_attribute(("panelSetType", "Navigable"));
    elem.push_attribute(("validateOnStepCompletion", "{Boolean}true"));
    w.write_event(Event::Start(elem)).unwrap();

    // <layout>
    write_layout(w, config, false);

    // <items>
    write_items_start(w, config);
    for child in children {
        write_node(w, child, config);
    }
    write_items_end(w);

    // Toolbar (optional)
    if config.include_toolbar {
        write_toolbar(w, config);
    }

    w.write_event(Event::End(BytesEnd::new("rootPanel")))
        .unwrap();
}

// ============================================================================
// Panel
// ============================================================================

fn write_panel(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    let AemNode::Panel {
        uuid,
        name,
        title,
        children,
        is_page: _,
        dor_exclude,
    } = node
    else {
        return;
    };

    let tag = format!("panel_{}", uuid.as_simple());
    let mut elem = BytesStart::new(tag.clone());
    push_jcr_timestamps(&mut elem, config);
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute(("sling:resourceType", config.panel_resource_type().as_str()));
    elem.push_attribute(("guideNodeClass", "guidePanel"));
    elem.push_attribute(("name", name.as_str()));
    if !title.is_empty() {
        elem.push_attribute(("jcr:title", title.as_str()));
    }
    elem.push_attribute(("textIsRich", "true"));
    elem.push_attribute(("dorExclusion", bool_str(*dor_exclude)));
    if config.dor_exclude_description {
        elem.push_attribute(("dorExcludeDescription", "true"));
    }
    elem.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    elem.push_attribute(("validateOnStepCompletion", "{Boolean}false"));
    w.write_event(Event::Start(elem)).unwrap();

    // <items>
    write_items_start(w, config);
    for child in children {
        write_node(w, child, config);
    }
    write_items_end(w);

    // <layout>
    write_layout(w, config, true);

    w.write_event(Event::End(BytesEnd::new(tag))).unwrap();
}

// ============================================================================
// Text Field
// ============================================================================

fn write_text_field(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    let AemNode::TextField {
        uuid,
        name,
        label,
        mandatory,
        visible,
        max_chars,
        colspan,
    } = node
    else {
        return;
    };

    let tag = format!("textbox_{}", uuid.as_simple());
    let mut elem = BytesStart::new(tag.clone());
    push_jcr_timestamps(&mut elem, config);
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    if !label.is_empty() {
        elem.push_attribute(("jcr:title", label.as_str()));
    }
    elem.push_attribute((
        "sling:resourceType",
        config.control_resource_type("textbox").as_str(),
    ));
    elem.push_attribute(("assistPriority", "label"));
    elem.push_attribute(("visible", format!("{{Boolean}}{visible}").as_str()));
    elem.push_attribute(("dorExclusion", format!("{{Boolean}}{}", !visible).as_str()));
    elem.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    elem.push_attribute(("guideNodeClass", "guideTextBox"));
    elem.push_attribute(("mandatory", bool_str(*mandatory)));
    elem.push_attribute(("name", name.as_str()));
    elem.push_attribute(("css", config.css_class("textbox").as_str()));
    elem.push_attribute(("textIsRich", "[true,true,true]"));
    if let Some(max) = max_chars {
        elem.push_attribute(("maxChars", max.to_string().as_str()));
    }
    w.write_event(Event::Start(elem)).unwrap();

    write_responsive(w, *colspan);

    w.write_event(Event::End(BytesEnd::new(tag))).unwrap();
}

// ============================================================================
// Number Field
// ============================================================================

fn write_number_field(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    let AemNode::NumberField {
        uuid,
        name,
        label,
        mandatory,
        visible,
        colspan,
    } = node
    else {
        return;
    };

    let tag = format!("numericbox_{}", uuid.as_simple());
    let mut elem = BytesStart::new(tag.clone());
    push_jcr_timestamps(&mut elem, config);
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    if !label.is_empty() {
        elem.push_attribute(("jcr:title", label.as_str()));
    }
    elem.push_attribute((
        "sling:resourceType",
        config.control_resource_type("numericbox").as_str(),
    ));
    elem.push_attribute(("assistPriority", "label"));
    elem.push_attribute(("visible", format!("{{Boolean}}{visible}").as_str()));
    elem.push_attribute(("dorExclusion", format!("{{Boolean}}{}", !visible).as_str()));
    elem.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    elem.push_attribute(("guideNodeClass", "guideNumberBox"));
    elem.push_attribute(("mandatory", bool_str(*mandatory)));
    elem.push_attribute(("name", name.as_str()));
    elem.push_attribute(("css", config.css_class("numericbox").as_str()));
    elem.push_attribute(("textIsRich", "[true,true,true]"));
    w.write_event(Event::Start(elem)).unwrap();

    write_responsive(w, *colspan);

    w.write_event(Event::End(BytesEnd::new(tag))).unwrap();
}

// ============================================================================
// Date Picker
// ============================================================================

fn write_date_picker(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    let AemNode::DatePicker {
        uuid,
        name,
        label,
        mandatory,
        visible,
        colspan,
    } = node
    else {
        return;
    };

    let tag = format!("datepicker_{}", uuid.as_simple());
    let mut elem = BytesStart::new(tag.clone());
    push_jcr_timestamps(&mut elem, config);
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    if !label.is_empty() {
        elem.push_attribute(("jcr:title", label.as_str()));
    }
    elem.push_attribute((
        "sling:resourceType",
        config.control_resource_type("datepicker").as_str(),
    ));
    elem.push_attribute(("visible", format!("{{Boolean}}{visible}").as_str()));
    elem.push_attribute(("dorExclusion", format!("{{Boolean}}{}", !visible).as_str()));
    elem.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    elem.push_attribute(("defaultToCurrentDate", "true"));
    elem.push_attribute(("guideNodeClass", "guideDatePicker"));
    elem.push_attribute(("mandatory", bool_str(*mandatory)));
    elem.push_attribute(("name", name.as_str()));
    elem.push_attribute(("css", config.css_class("datepicker").as_str()));
    elem.push_attribute(("placeholderText", ""));
    elem.push_attribute(("textIsRich", "[true,true]"));
    elem.push_attribute(("validatePictureClause", "date{YYYY-MM-DD}"));
    elem.push_attribute((
        "validatePictureClauseMessage",
        "Please enter the date using the format YYYY-MM-DD.",
    ));
    elem.push_attribute(("validationPatternType", "custom"));
    elem.push_attribute(("yearRangeFrom", "100"));
    elem.push_attribute(("yearRangeTo", "10"));
    w.write_event(Event::Start(elem)).unwrap();

    write_responsive(w, *colspan);

    w.write_event(Event::End(BytesEnd::new(tag))).unwrap();
}

// ============================================================================
// Dropdown
// ============================================================================

fn write_dropdown(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    let AemNode::Dropdown {
        uuid,
        name,
        label,
        options,
        mandatory,
        visible,
        colspan,
    } = node
    else {
        return;
    };

    let tag = format!("dropdownlist_{}", uuid.as_simple());
    let mut elem = BytesStart::new(tag.clone());
    push_jcr_timestamps(&mut elem, config);
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    if !label.is_empty() {
        elem.push_attribute(("jcr:title", label.as_str()));
    }
    elem.push_attribute((
        "sling:resourceType",
        config.control_resource_type("dropdownlist").as_str(),
    ));
    elem.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    elem.push_attribute(("guideNodeClass", "guideDropDownList"));
    elem.push_attribute(("mandatory", bool_str(*mandatory)));
    elem.push_attribute(("visible", format!("{{Boolean}}{visible}").as_str()));
    elem.push_attribute(("name", name.as_str()));
    elem.push_attribute(("css", config.css_class("dropdownlist").as_str()));
    elem.push_attribute(("options", format_options_attr(options).as_str()));
    elem.push_attribute(("textIsRich", "[true,true]"));
    w.write_event(Event::Start(elem)).unwrap();

    write_responsive(w, *colspan);

    w.write_event(Event::End(BytesEnd::new(tag))).unwrap();
}

// ============================================================================
// Checkbox
// ============================================================================

fn write_checkbox(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    let AemNode::Checkbox {
        uuid,
        name,
        options,
        alignment,
        visible,
        colspan,
    } = node
    else {
        return;
    };

    let tag = format!("checkbox_{}", uuid.as_simple());
    let mut elem = BytesStart::new(tag.clone());
    push_jcr_timestamps(&mut elem, config);
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute((
        "sling:resourceType",
        config.control_resource_type("checkbox").as_str(),
    ));
    elem.push_attribute(("alignment", alignment_str(*alignment)));
    elem.push_attribute(("assistPriority", "caption"));
    elem.push_attribute(("css", config.css_class("checkbox").as_str()));
    elem.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    elem.push_attribute(("richTextOptions", "true"));
    elem.push_attribute(("guideNodeClass", "guideCheckBox"));
    elem.push_attribute(("hideTitle", "true"));
    elem.push_attribute(("visible", format!("{{Boolean}}{visible}").as_str()));
    elem.push_attribute(("name", name.as_str()));
    elem.push_attribute(("options", format_options_attr(options).as_str()));
    // textIsRich array with one entry per option + 1
    let text_is_rich = format!(
        "[{}]",
        std::iter::repeat("true")
            .take(options.len() + 1)
            .collect::<Vec<_>>()
            .join(",")
    );
    elem.push_attribute(("textIsRich", text_is_rich.as_str()));
    w.write_event(Event::Start(elem)).unwrap();

    write_responsive(w, *colspan);

    w.write_event(Event::End(BytesEnd::new(tag))).unwrap();
}

// ============================================================================
// Radio Button
// ============================================================================

fn write_radio_button(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    let AemNode::RadioButton {
        uuid,
        name,
        label,
        options,
        alignment,
        mandatory,
        visible,
        colspan,
    } = node
    else {
        return;
    };

    let tag = format!("radiobutton_{}", uuid.as_simple());
    let mut elem = BytesStart::new(tag.clone());
    push_jcr_timestamps(&mut elem, config);
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    if !label.is_empty() {
        elem.push_attribute(("jcr:title", label.as_str()));
    }
    elem.push_attribute((
        "sling:resourceType",
        config.control_resource_type("radiobutton").as_str(),
    ));
    elem.push_attribute(("css", config.css_class("radiobutton").as_str()));
    elem.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    elem.push_attribute(("guideNodeClass", "guideRadioButton"));
    elem.push_attribute(("mandatory", bool_str(*mandatory)));
    elem.push_attribute(("richTextOptions", "true"));
    elem.push_attribute(("visible", format!("{{Boolean}}{visible}").as_str()));
    elem.push_attribute(("name", name.as_str()));
    elem.push_attribute(("alignment", alignment_str(*alignment)));
    elem.push_attribute(("options", format_options_attr(options).as_str()));
    let text_is_rich = format!(
        "[{}]",
        std::iter::repeat("true")
            .take(options.len() + 1)
            .collect::<Vec<_>>()
            .join(",")
    );
    elem.push_attribute(("textIsRich", text_is_rich.as_str()));
    w.write_event(Event::Start(elem)).unwrap();

    write_responsive(w, *colspan);

    w.write_event(Event::End(BytesEnd::new(tag))).unwrap();
}

// ============================================================================
// Text Draw (static text / headings)
// ============================================================================

fn write_text_draw(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    let AemNode::TextDraw {
        uuid,
        name,
        content,
        dor_exclude,
        colspan,
    } = node
    else {
        return;
    };

    let tag = format!("textdraw_{}", uuid.as_simple());
    let mut elem = BytesStart::new(tag.clone());
    push_jcr_timestamps(&mut elem, config);
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute((
        "sling:resourceType",
        config.control_resource_type("textdraw").as_str(),
    ));
    elem.push_attribute(("_value", content.as_str()));
    elem.push_attribute(("css", ""));
    elem.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    elem.push_attribute(("guideNodeClass", "guideTextDraw"));
    elem.push_attribute(("name", name.as_str()));
    elem.push_attribute(("textIsRich", "true"));
    if *dor_exclude {
        elem.push_attribute(("dorExclusion", "true"));
    }
    w.write_event(Event::Start(elem)).unwrap();

    // <fd:rules>
    let mut rules = BytesStart::new("fd:rules");
    rules.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Empty(rules)).unwrap();

    write_responsive(w, *colspan);

    w.write_event(Event::End(BytesEnd::new(tag))).unwrap();
}

// ============================================================================
// Text Box Multiline
// ============================================================================

fn write_text_box_multiline(
    w: &mut Writer<&mut Cursor<Vec<u8>>>,
    node: &AemNode,
    config: &AemConfig,
) {
    let AemNode::TextBoxMultiline {
        uuid,
        name,
        label,
        mandatory,
        visible,
        colspan,
    } = node
    else {
        return;
    };

    let tag = format!("textboxmultiline_{}", uuid.as_simple());
    let mut elem = BytesStart::new(tag.clone());
    push_jcr_timestamps(&mut elem, config);
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    if !label.is_empty() {
        elem.push_attribute(("jcr:title", label.as_str()));
    }
    elem.push_attribute((
        "sling:resourceType",
        config.control_resource_type("textboxMultiline").as_str(),
    ));
    elem.push_attribute((
        "css",
        format!("{} ubs-textbox-multiline", config.css_class("textbox")).as_str(),
    ));
    elem.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    elem.push_attribute(("guideNodeClass", "guideTextBox"));
    elem.push_attribute(("mandatory", bool_str(*mandatory)));
    elem.push_attribute(("visible", format!("{{Boolean}}{visible}").as_str()));
    elem.push_attribute(("name", name.as_str()));
    elem.push_attribute(("multiLine", "true"));
    w.write_event(Event::Start(elem)).unwrap();

    write_responsive(w, *colspan);

    w.write_event(Event::End(BytesEnd::new(tag))).unwrap();
}

// ============================================================================
// Repeatable
// ============================================================================

fn write_repeatable(w: &mut Writer<&mut Cursor<Vec<u8>>>, node: &AemNode, config: &AemConfig) {
    let AemNode::Repeatable {
        uuid,
        name,
        title,
        children,
        min_occur,
        max_occur,
    } = node
    else {
        return;
    };

    // Outer wrapping panel
    let outer_tag = format!("repeatable_{}", uuid.as_simple());
    let mut outer = BytesStart::new(outer_tag.clone());
    push_jcr_timestamps(&mut outer, config);
    outer.push_attribute(("jcr:primaryType", "nt:unstructured"));
    outer.push_attribute(("sling:resourceType", config.panel_resource_type().as_str()));
    outer.push_attribute(("guideNodeClass", "guidePanel"));
    outer.push_attribute(("jcr:title", title.as_str()));
    outer.push_attribute(("name", name.as_str()));
    outer.push_attribute(("textIsRich", "true"));
    outer.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    w.write_event(Event::Start(outer)).unwrap();

    // <items>
    write_items_start(w, config);

    // Inner repeatable panel with minOccur/maxOccur
    {
        let inner_name = format!("PN_{name}");
        let mut inner = BytesStart::new("repeatableInner");
        inner.push_attribute(("jcr:primaryType", "nt:unstructured"));
        inner.push_attribute(("sling:resourceType", config.panel_resource_type().as_str()));
        inner.push_attribute(("guideNodeClass", "guidePanel"));
        inner.push_attribute(("name", inner_name.as_str()));
        inner.push_attribute(("textIsRich", "true"));
        inner.push_attribute(("maxOccur", max_occur.to_string().as_str()));
        inner.push_attribute(("minOccur", min_occur.to_string().as_str()));
        w.write_event(Event::Start(inner)).unwrap();

        // Inner <items>
        write_items_start(w, config);
        for child in children {
            write_node(w, child, config);
        }
        write_items_end(w);

        // Inner <layout>
        write_layout(w, config, true);

        // <toolbar> with remove button
        write_repeatable_toolbar_remove(w, config);

        w.write_event(Event::End(BytesEnd::new("repeatableInner")))
            .unwrap();
    }

    // Add button (tertiary)
    write_repeatable_add_button(w, name, config);

    write_items_end(w);

    // <layout>
    write_layout(w, config, true);

    w.write_event(Event::End(BytesEnd::new(outer_tag))).unwrap();
}

// ============================================================================
// Shared sub-elements
// ============================================================================

/// Write `<items jcr:primaryType="nt:unstructured" sling:resourceType="...">`.
fn write_items_start(w: &mut Writer<&mut Cursor<Vec<u8>>>, config: &AemConfig) {
    let mut elem = BytesStart::new("items");
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute(("sling:resourceType", config.default_layout.as_str()));
    w.write_event(Event::Start(elem)).unwrap();
}

fn write_items_end(w: &mut Writer<&mut Cursor<Vec<u8>>>) {
    w.write_event(Event::End(BytesEnd::new("items"))).unwrap();
}

/// Write `<layout>` element.
fn write_layout(w: &mut Writer<&mut Cursor<Vec<u8>>>, config: &AemConfig, non_navigable: bool) {
    let mut elem = BytesStart::new("layout");
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute(("sling:resourceType", config.default_layout.as_str()));
    if config.enable_layout_optimization {
        elem.push_attribute(("enableLayoutOptimization", "{Boolean}true"));
    }
    if non_navigable {
        elem.push_attribute(("nonNavigable", "{Boolean}true"));
    }
    elem.push_attribute(("toolbarPosition", "Bottom"));
    w.write_event(Event::Empty(elem)).unwrap();
}

/// Write `<cq:responsive>` with column width.
fn write_responsive(w: &mut Writer<&mut Cursor<Vec<u8>>>, width: u32) {
    let mut responsive = BytesStart::new("cq:responsive");
    responsive.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Start(responsive)).unwrap();

    let mut default = BytesStart::new("default");
    default.push_attribute(("jcr:primaryType", "nt:unstructured"));
    default.push_attribute(("offset", "0"));
    default.push_attribute(("width", width.to_string().as_str()));
    w.write_event(Event::Empty(default)).unwrap();

    w.write_event(Event::End(BytesEnd::new("cq:responsive")))
        .unwrap();
}

/// Write the standard toolbar with prev/next/submit buttons.
fn write_toolbar(w: &mut Writer<&mut Cursor<Vec<u8>>>, config: &AemConfig) {
    let mut toolbar = BytesStart::new("toolbar");
    toolbar.push_attribute(("jcr:primaryType", "nt:unstructured"));
    toolbar.push_attribute(("sling:resourceType", config.default_layout.as_str()));
    w.write_event(Event::Start(toolbar)).unwrap();

    // Previous
    write_toolbar_button(
        w,
        "previtemnav",
        "Previous",
        "fd/af/components/previtemnav",
        config,
    );
    // Next
    write_toolbar_button(
        w,
        "nextitemnav",
        "Next",
        "fd/af/components/nextitemnav",
        config,
    );
    // Submit
    write_toolbar_button(w, "submit", "Submit", "fd/af/components/submit", config);

    w.write_event(Event::End(BytesEnd::new("toolbar"))).unwrap();
}

fn write_toolbar_button(
    w: &mut Writer<&mut Cursor<Vec<u8>>>,
    tag_name: &str,
    title: &str,
    resource_type: &str,
    config: &AemConfig,
) {
    let mut elem = BytesStart::new(tag_name);
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute(("sling:resourceType", resource_type));
    elem.push_attribute(("jcr:title", title));
    elem.push_attribute(("guideNodeClass", "guideButton"));
    elem.push_attribute(("dorExclusion", "true"));
    elem.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    w.write_event(Event::Empty(elem)).unwrap();
}

/// Write the remove button inside a repeatable toolbar.
fn write_repeatable_toolbar_remove(w: &mut Writer<&mut Cursor<Vec<u8>>>, config: &AemConfig) {
    let mut toolbar = BytesStart::new("toolbar");
    toolbar.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Start(toolbar)).unwrap();

    let mut btn = BytesStart::new("removebutton");
    btn.push_attribute(("jcr:primaryType", "nt:unstructured"));
    btn.push_attribute((
        "sling:resourceType",
        config.control_resource_type("removebutton").as_str(),
    ));
    btn.push_attribute(("guideNodeClass", "guideButton"));
    btn.push_attribute(("dorExclusion", "true"));
    btn.push_attribute(("jcr:title", "Remove"));
    w.write_event(Event::Empty(btn)).unwrap();

    w.write_event(Event::End(BytesEnd::new("toolbar"))).unwrap();
}

/// Write the Add button for a repeatable section.
fn write_repeatable_add_button(
    w: &mut Writer<&mut Cursor<Vec<u8>>>,
    repeatable_name: &str,
    config: &AemConfig,
) {
    let mut btn = BytesStart::new("addbutton");
    btn.push_attribute(("jcr:primaryType", "nt:unstructured"));
    btn.push_attribute((
        "sling:resourceType",
        config.control_resource_type("tertiarybutton").as_str(),
    ));
    btn.push_attribute(("guideNodeClass", "guideButton"));
    btn.push_attribute(("dorExclusion", "true"));
    btn.push_attribute(("jcr:title", format!("Add {repeatable_name}").as_str()));
    btn.push_attribute(("type", "button"));
    w.write_event(Event::Empty(btn)).unwrap();
}

// ============================================================================
// Attribute helpers
// ============================================================================

/// Push standard JCR timestamp attributes onto an element.
fn push_jcr_timestamps(elem: &mut BytesStart, config: &AemConfig) {
    elem.push_attribute(("jcr:created", "{Date}2025-01-01T00:00:00.000Z"));
    elem.push_attribute(("jcr:createdBy", config.author.as_str()));
    elem.push_attribute(("jcr:lastModified", "{Date}2025-01-01T00:00:00.000Z"));
    elem.push_attribute(("jcr:lastModifiedBy", config.author.as_str()));
}

fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

fn alignment_str(a: OptionAlignment) -> &'static str {
    match a {
        OptionAlignment::Horizontal => "horizontal",
        OptionAlignment::Vertical => "vertical",
    }
}

/// Format options for checkbox/radio/dropdown as `[value1=label1,value2=label2,...]`.
fn format_options_attr(options: &[AemOption]) -> String {
    let inner: Vec<String> = options
        .iter()
        .map(|o| format!("{}={}", o.value, o.label))
        .collect();
    format!("[{}]", inner.join(","))
}

// ============================================================================
// Attribute reformatting (one-per-line)
// ============================================================================

/// Reformat XML so that element attributes appear one per line, indented to
/// align with the first attribute.
///
/// Turns:
/// ```xml
///     <tag attr1="v1" attr2="v2">
/// ```
/// into:
/// ```xml
///     <tag
///         attr1="v1"
///         attr2="v2">
/// ```
///
/// Only elements with more than one attribute are reformatted.
pub(crate) fn reformat_attributes(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len() + xml.len() / 4);

    for line in xml.lines() {
        if let Some(reformatted) = try_reformat_line(line) {
            out.push_str(&reformatted);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }

    out
}

/// Try to reformat a single XML line. Returns `None` if the line should be
/// kept as-is (not an element, or has ≤1 attribute).
fn try_reformat_line(line: &str) -> Option<String> {
    let trimmed = line.trim_start();

    // Must start with '<' and not be a closing tag, comment, PI, or declaration
    if !trimmed.starts_with('<')
        || trimmed.starts_with("</")
        || trimmed.starts_with("<?")
        || trimmed.starts_with("<!")
    {
        return None;
    }

    // Find the leading indentation
    let indent = &line[..line.len() - trimmed.len()];

    // Parse the tag name and attributes.
    // Find the end of the opening tag (matching '>' or '/>').
    let (tag_content, suffix) = extract_tag_content(trimmed)?;

    // Split into tag name and attributes
    let first_space = tag_content.find(' ')?;
    let tag_name = &tag_content[1..first_space]; // skip '<'
    let attrs_str = &tag_content[first_space + 1..];

    // Parse attributes
    let attrs = parse_attributes(attrs_str);
    if attrs.len() <= 1 {
        return None;
    }

    // Build reformatted output
    let attr_indent = format!("{}{}", indent, " ".repeat(tag_name.len() + 2)); // +2 for '<' and space
    let mut result = format!("{}<{}", indent, tag_name);
    for (i, attr) in attrs.iter().enumerate() {
        if i == 0 {
            result.push(' ');
        } else {
            result.push('\n');
            result.push_str(&attr_indent);
        }
        result.push_str(attr);
    }
    result.push_str(suffix);

    Some(result)
}

/// Extract the content between '<' ... '>' or '<' ... '/>', returning
/// (content_without_close, suffix). Suffix is ">" or "/>" or "/>".
fn extract_tag_content(trimmed: &str) -> Option<(&str, &str)> {
    if trimmed.ends_with("/>") {
        Some((&trimmed[..trimmed.len() - 2], "/>"))
    } else if trimmed.ends_with('>') {
        Some((&trimmed[..trimmed.len() - 1], ">"))
    } else {
        None
    }
}

/// Parse a string of XML attributes like `attr1="val1" attr2="val2"` into
/// a vec of individual attribute strings.
fn parse_attributes(s: &str) -> Vec<&str> {
    let mut attrs = Vec::new();
    let mut i = 0;
    let bytes = s.as_bytes();
    let len = bytes.len();

    while i < len {
        // Skip whitespace
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= len {
            break;
        }

        // Start of attribute name
        let start = i;

        // Find '='
        while i < len && bytes[i] != b'=' {
            i += 1;
        }
        if i >= len {
            break;
        }
        i += 1; // skip '='

        // Expect opening quote
        if i >= len {
            break;
        }
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            break;
        }
        i += 1; // skip opening quote

        // Find closing quote
        while i < len && bytes[i] != quote {
            i += 1;
        }
        if i >= len {
            break;
        }
        i += 1; // skip closing quote

        attrs.push(&s[start..i]);
    }

    attrs
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aem::{AemConfig, AemNode, AemOption, OptionAlignment};
    use uuid::Uuid;

    fn test_config() -> AemConfig {
        AemConfig {
            deterministic_uuids: true,
            include_page_wrapper: false,
            include_toolbar: false,
            ..Default::default()
        }
    }

    fn fixed_uuid() -> Uuid {
        Uuid::new_v5(&Uuid::from_bytes([0; 16]), b"test")
    }

    #[test]
    fn xml_output_is_well_formed() {
        let root = AemNode::Root {
            title: "Test Form".into(),
            children: vec![AemNode::TextDraw {
                uuid: fixed_uuid(),
                name: "ST_1".into(),
                content: "<p>Hello &amp; world</p>".into(),
                dor_exclude: false,
                colspan: 12,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("rootPanel"));
        assert!(xml.contains("guideTextDraw"));
        assert!(xml.contains("ST_1"));
        // Ensure the XML is parseable
        let mut reader = quick_xml::Reader::from_str(&xml);
        loop {
            match reader.read_event() {
                Ok(Event::Eof) => break,
                Err(e) => panic!("XML parse error: {e}"),
                _ => {}
            }
        }
    }

    #[test]
    fn page_wrapper_includes_jcr_root() {
        let config = AemConfig {
            include_page_wrapper: true,
            include_toolbar: false,
            ..test_config()
        };
        let root = AemNode::Root {
            title: "Wrapped Form".into(),
            children: vec![],
        };
        let xml = generate_aem_xml(&root, &config);
        assert!(xml.contains("jcr:root"));
        assert!(xml.contains("jcr:content"));
        assert!(xml.contains("guideContainer"));
        assert!(xml.contains("rootPanel"));
    }

    #[test]
    fn page_wrapper_has_correct_resource_types() {
        let config = AemConfig {
            include_page_wrapper: true,
            include_toolbar: false,
            page_resource_type: "fd/af/components/page2".into(),
            ..test_config()
        };
        let root = AemNode::Root {
            title: "Test Form".into(),
            children: vec![],
        };
        let xml = generate_aem_xml(&root, &config);

        // jcr:content must use page resource type, NOT guideContainer
        assert!(
            xml.contains("sling:resourceType=\"fd/af/components/page2\""),
            "jcr:content should use page_resource_type"
        );
        // guideContainer must use the guide container resource type
        assert!(
            xml.contains("sling:resourceType=\"fd/af/components/guideContainer\""),
            "guideContainer should use guide_container_resource_type"
        );
        // rootPanel must use rootPanel resource type
        assert!(
            xml.contains("sling:resourceType=\"fd/af/components/rootPanel\""),
            "rootPanel should use rootPanel resource type"
        );
        // jcr:language attribute present
        assert!(xml.contains("jcr:language="));
        // guideContainer attributes
        assert!(xml.contains("guideCss=\"guideContainer\""));
        assert!(xml.contains("name=\"guide1\""));
        // rootPanel attributes
        assert!(xml.contains("guideNodeClass=\"rootPanelNode\""));
        assert!(xml.contains("name=\"guideRootPanel\""));
        assert!(xml.contains("panelSetType=\"Navigable\""));
        // guideContainer layout element
        assert!(xml.contains("sling:resourceType=\"fd/af/layouts/defaultGuideLayout\""));
        assert!(xml.contains("mobileLayout=\"fd/af/layouts/mobile/step\""));
    }

    #[test]
    fn text_field_has_responsive_width() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::TextField {
                uuid: fixed_uuid(),
                name: "TF_test".into(),
                label: "Test Label".into(),
                mandatory: false,
                visible: true,
                max_chars: Some(100),
                colspan: 6,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("cq:responsive"));
        assert!(xml.contains("width=\"6\""));
        assert!(xml.contains("maxChars=\"100\""));
    }

    #[test]
    fn checkbox_options_serialized() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Checkbox {
                uuid: fixed_uuid(),
                name: "CB_test".into(),
                options: vec![
                    AemOption {
                        label: "Yes".into(),
                        value: "1".into(),
                    },
                    AemOption {
                        label: "No".into(),
                        value: "0".into(),
                    },
                ],
                alignment: OptionAlignment::Horizontal,
                visible: true,
                colspan: 12,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("options=\"[1=Yes,0=No]\""));
        assert!(xml.contains("alignment=\"horizontal\""));
    }

    #[test]
    fn toolbar_buttons_when_enabled() {
        let config = AemConfig {
            include_page_wrapper: false,
            include_toolbar: true,
            ..test_config()
        };
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![],
        };
        let xml = generate_aem_xml(&root, &config);
        assert!(xml.contains("previtemnav"));
        assert!(xml.contains("nextitemnav"));
        assert!(xml.contains("submit"));
    }

    #[test]
    fn repeatable_has_min_max_occur() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Repeatable {
                uuid: fixed_uuid(),
                name: "RPT_1".into(),
                title: "Repeat Section".into(),
                children: vec![],
                min_occur: 1,
                max_occur: 10,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("minOccur=\"1\""));
        assert!(xml.contains("maxOccur=\"10\""));
        assert!(xml.contains("addbutton"));
        assert!(xml.contains("removebutton"));
    }

    #[test]
    fn dropdown_has_options() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Dropdown {
                uuid: fixed_uuid(),
                name: "DD_test".into(),
                label: "Pick one".into(),
                options: vec![
                    AemOption {
                        label: "A".into(),
                        value: "a".into(),
                    },
                    AemOption {
                        label: "B".into(),
                        value: "b".into(),
                    },
                ],
                mandatory: true,
                visible: true,
                colspan: 12,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("guideDropDownList"));
        assert!(xml.contains("options=\"[a=A,b=B]\""));
    }
}
