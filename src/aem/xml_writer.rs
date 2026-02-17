//! XML serialization of an `AemNode` tree into AEM JCR content XML.
//!
//! Uses `quick_xml::Writer` for well-formed, properly escaped output.

use std::io::Cursor;

use quick_xml::Writer;
use quick_xml::events::{BytesEnd, BytesStart, Event};

use super::{AemConfig, AemNode, AemOption, OptionAlignment};

// ============================================================================
// Helpers
// ============================================================================

/// Escape a string for use in an XML attribute value, encoding only `<` and `&`.
///
/// Unlike quick_xml's default escaping (which also encodes `>`, `"`, and `'`),
/// this only encodes the two characters that are strictly required inside
/// double-quoted attribute values. This keeps HTML fragments like `<h1>Title</h1>`
/// readable in the output (as `&lt;h1>Title&lt;/h1>` rather than
/// `&lt;h1&gt;Title&lt;/h1&gt;`).
fn escape_xml_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

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
        content_elem.push_attribute(("cq:deviceGroups", "[/etc/mobile/groups/responsive]"));
        content_elem.push_attribute(("cq:lastModified", "{Date}2025-01-01T00:00:00.000+01:00"));
        content_elem.push_attribute(("cq:lastModifiedBy", config.author.as_str()));
        content_elem.push_attribute(("cq:template", config.template_path.as_str()));
        content_elem.push_attribute(("jcr:language", config.master_language.as_str()));
        content_elem.push_attribute(("jcr:primaryType", "cq:PageContent"));
        content_elem.push_attribute(("jcr:title", config.form_code.as_str()));
        content_elem.push_attribute(("sling:resourceType", config.page_resource_type.as_str()));
        content_elem.push_attribute(("ignoreTranslationInvalidation", "{Boolean}true"));
        w.write_event(Event::Start(content_elem)).unwrap();

        // <parsys1>
        write_parsys1(w, title, config);

        // <guideContainer>
        write_guide_container(w, title, children, config);

        // <parsys2>
        write_parsys2(w, config);

        // </jcr:content>
        w.write_event(Event::End(BytesEnd::new("jcr:content")))
            .unwrap();
        // </jcr:root>
        w.write_event(Event::End(BytesEnd::new("jcr:root")))
            .unwrap();
    } else {
        // No page wrapper — emit rootPanel directly
        write_root_panel(w, children, config);
    }
}

/// Write `<parsys1>` with guide header and form title.
fn write_parsys1(w: &mut Writer<&mut Cursor<Vec<u8>>>, form_title: &str, config: &AemConfig) {
    let mut parsys = BytesStart::new("parsys1");
    parsys.push_attribute(("jcr:primaryType", "nt:unstructured"));
    parsys.push_attribute((
        "sling:resourceType",
        "wcm/foundation/components/responsivegrid",
    ));
    w.write_event(Event::Start(parsys)).unwrap();

    // <guideheader>
    let mut header = BytesStart::new("guideheader");
    header.push_attribute(("jcr:primaryType", "nt:unstructured"));
    header.push_attribute((
        "sling:resourceType",
        config.control_resource_type("guideheader").as_str(),
    ));
    w.write_event(Event::Empty(header)).unwrap();

    // <guideformtitle>
    let guide_form_title = format!("<p>{form_title}</p>");
    let mut title_elem = BytesStart::new("guideformtitle");
    title_elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    title_elem.push_attribute((
        "sling:resourceType",
        config.control_resource_type("formtitle").as_str(),
    ));
    let escaped_title = escape_xml_attr(&guide_form_title);
    title_elem.push_attribute(("_value".as_bytes(), escaped_title.as_bytes()));
    title_elem.push_attribute(("css", "guideformtitle container"));
    title_elem.push_attribute(("guideNodeClass", "guideTextDraw"));
    title_elem.push_attribute(("name", "formTitle"));
    title_elem.push_attribute(("textIsRich", "true"));
    w.write_event(Event::Empty(title_elem)).unwrap();

    w.write_event(Event::End(BytesEnd::new("parsys1"))).unwrap();
}

/// Write `<parsys2>` with guide footer.
fn write_parsys2(w: &mut Writer<&mut Cursor<Vec<u8>>>, config: &AemConfig) {
    let mut parsys = BytesStart::new("parsys2");
    parsys.push_attribute(("jcr:primaryType", "nt:unstructured"));
    parsys.push_attribute((
        "sling:resourceType",
        "wcm/foundation/components/responsivegrid",
    ));
    w.write_event(Event::Start(parsys)).unwrap();

    let mut footer = BytesStart::new("guidefooter");
    footer.push_attribute(("jcr:primaryType", "nt:unstructured"));
    footer.push_attribute((
        "sling:resourceType",
        config.control_resource_type("guidefooter").as_str(),
    ));
    w.write_event(Event::Empty(footer)).unwrap();

    w.write_event(Event::End(BytesEnd::new("parsys2"))).unwrap();
}

fn write_guide_container(
    w: &mut Writer<&mut Cursor<Vec<u8>>>,
    _title: &str,
    children: &[AemNode],
    config: &AemConfig,
) {
    let mut elem = BytesStart::new("guideContainer");
    elem.push_attribute(("fd:version", "1.1"));
    elem.push_attribute(("jcr:lastModified", "{Date}2025-01-01T00:00:00.000+01:00"));
    elem.push_attribute(("jcr:lastModifiedBy", config.author.as_str()));
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute((
        "sling:resourceType",
        config.guide_container_resource_type().as_str(),
    ));
    elem.push_attribute(("actionType", config.action_type.as_str()));
    elem.push_attribute((
        "autoSaveStrategyType",
        "fd/fp/components/actions/autosave/timebased",
    ));
    elem.push_attribute(("clientLibRef", config.client_lib_ref.as_str()));
    elem.push_attribute(("disableSwipeGesture", "{Boolean}false"));
    if !config.dor_template_ref.is_empty() {
        elem.push_attribute(("dorTemplateRef", config.dor_template_ref.as_str()));
    }
    elem.push_attribute(("dorType", config.dor_type.as_str()));
    elem.push_attribute(("enableFocusOnFirstField", "{Boolean}true"));
    elem.push_attribute(("enableLayoutLayer", "false"));
    elem.push_attribute(("guideCss", "guideContainer"));
    elem.push_attribute(("guideNodeClass", "guideContainerNode"));
    elem.push_attribute(("name", "guide1"));
    if !config.redirect_url.is_empty() {
        elem.push_attribute(("redirect", config.redirect_url.as_str()));
    }
    elem.push_attribute(("textIsRich", "true"));
    elem.push_attribute(("thankYouMessage", "Thank you for submitting the form."));
    elem.push_attribute(("thankYouOption", "page"));
    if !config.theme_ref.is_empty() {
        elem.push_attribute(("themeRef", config.theme_ref.as_str()));
    }
    elem.push_attribute(("useExistingAF", "false"));
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
    write_root_panel(w, children, config);

    // <autoSaveInfo>
    {
        let mut auto_save = BytesStart::new("autoSaveInfo");
        auto_save.push_attribute(("jcr:primaryType", "nt:unstructured"));
        auto_save.push_attribute(("metadataselector", "global"));
        w.write_event(Event::Empty(auto_save)).unwrap();
    }

    // <signerInfo>
    {
        let mut signer_info = BytesStart::new("signerInfo");
        signer_info.push_attribute(("jcr:primaryType", "nt:unstructured"));
        signer_info.push_attribute(("firstSignerFormFiller", "false"));
        signer_info.push_attribute(("workflowType", "SEQUENTIAL"));
        w.write_event(Event::Start(signer_info)).unwrap();

        let mut signer0 = BytesStart::new("signer0");
        signer0.push_attribute(("jcr:primaryType", "nt:unstructured"));
        signer0.push_attribute(("countryCode", "undefined"));
        signer0.push_attribute(("countryCodeSource", "undefined"));
        signer0.push_attribute(("email", "undefined"));
        signer0.push_attribute(("emailSource", "undefined"));
        signer0.push_attribute(("phone", "undefined"));
        signer0.push_attribute(("phoneSource", "undefined"));
        signer0.push_attribute(("securityOption", "undefined"));
        signer0.push_attribute(("signerTitle", "Signer One"));
        w.write_event(Event::Empty(signer0)).unwrap();

        w.write_event(Event::End(BytesEnd::new("signerInfo")))
            .unwrap();
    }

    // <view> with <print> (DOR branding)
    write_view(w, config);

    // <assets> with <dictionary> (languages)
    write_assets(w, config);

    // </guideContainer>
    w.write_event(Event::End(BytesEnd::new("guideContainer")))
        .unwrap();
}

/// Write the `<view>` element with DOR print/branding settings.
fn write_view(w: &mut Writer<&mut Cursor<Vec<u8>>>, config: &AemConfig) {
    let mut view = BytesStart::new("view");
    view.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Start(view)).unwrap();

    // <print>
    let mut print = BytesStart::new("print");
    print.push_attribute(("jcr:created", "{Date}2025-01-01T00:00:00.000+01:00"));
    print.push_attribute(("jcr:lastModified", "{Date}2025-01-01T00:00:00.000+01:00"));
    print.push_attribute(("jcr:lastModifiedBy", config.author.as_str()));
    print.push_attribute(("jcr:primaryType", "nt:unstructured"));
    print.push_attribute((
        "sling:resourceType",
        "fd/af/authoring/components/dor/dorProperties",
    ));
    print.push_attribute(("accentColor", "#04a6cb"));
    print.push_attribute(("alignment", "dorFieldVerticalAlignment"));
    print.push_attribute(("fontFamily", "Arial"));
    print.push_attribute(("hidePanelDescriptions", "true"));
    print.push_attribute(("includeUnboundFields", "true"));
    print.push_attribute(("metaTemplateRef", config.meta_template_ref.as_str()));
    print.push_attribute(("optionSeparator", ", "));
    print.push_attribute(("optionsNumberInHorizontalAlign", "4"));
    print.push_attribute(("showSelectedOptions", "false"));
    w.write_event(Event::Start(print)).unwrap();

    // <branding>
    write_branding(w, config);

    w.write_event(Event::End(BytesEnd::new("print"))).unwrap();
    w.write_event(Event::End(BytesEnd::new("view"))).unwrap();
}

/// Write DOR branding section inside `<print>`.
fn write_branding(w: &mut Writer<&mut Cursor<Vec<u8>>>, config: &AemConfig) {
    let mut branding = BytesStart::new("branding");
    branding.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Start(branding)).unwrap();

    let mut items = BytesStart::new("items");
    items.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Start(items)).unwrap();

    let mut mp = BytesStart::new("masterpage0");
    mp.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Start(mp)).unwrap();

    let mut mp_items = BytesStart::new("items");
    mp_items.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Start(mp_items)).unwrap();

    // txtBankingRelationship
    let mut txt_br = BytesStart::new("txtBankingRelationship");
    txt_br.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Empty(txt_br)).unwrap();

    // ShowBankingRelationship
    let mut show_br = BytesStart::new("ShowBankingRelationship");
    show_br.push_attribute(("jcr:primaryType", "nt:unstructured"));
    show_br.push_attribute(("value", "1"));
    w.write_event(Event::Empty(show_br)).unwrap();

    // Header
    {
        let mut header = BytesStart::new("Header");
        header.push_attribute(("jcr:primaryType", "nt:unstructured"));
        w.write_event(Event::Start(header)).unwrap();

        let mut h_items = BytesStart::new("items");
        h_items.push_attribute(("jcr:primaryType", "nt:unstructured"));
        w.write_event(Event::Start(h_items)).unwrap();

        let mut logo = BytesStart::new("AF_LOGO_IMAGE");
        logo.push_attribute(("jcr:lastModified", "{Date}2025-01-01T00:00:00.000+01:00"));
        logo.push_attribute(("jcr:lastModifiedBy", config.author.as_str()));
        logo.push_attribute(("jcr:primaryType", "nt:unstructured"));
        logo.push_attribute(("valueFrom", "template"));
        w.write_event(Event::Empty(logo)).unwrap();

        let mut form_type = BytesStart::new("FormType");
        form_type.push_attribute(("jcr:primaryType", "nt:unstructured"));
        form_type.push_attribute((
            "resourceType",
            "granite/ui/components/coral/foundation/form/textarea",
        ));
        form_type.push_attribute(("value", config.form_type.as_str()));
        form_type.push_attribute(("valueFrom", "     "));
        w.write_event(Event::Empty(form_type)).unwrap();

        // HeaderInfo
        {
            let mut hi = BytesStart::new("HeaderInfo");
            hi.push_attribute(("jcr:primaryType", "nt:unstructured"));
            w.write_event(Event::Start(hi)).unwrap();

            let mut hi_items = BytesStart::new("items");
            hi_items.push_attribute(("jcr:primaryType", "nt:unstructured"));
            w.write_event(Event::Start(hi_items)).unwrap();

            let mut ht = BytesStart::new("AF_HEADER_TEXT");
            ht.push_attribute(("jcr:primaryType", "nt:unstructured"));
            ht.push_attribute((
                "resourceType",
                "granite/ui/components/coral/foundation/form/textarea",
            ));
            ht.push_attribute(("valueFrom", "template"));
            w.write_event(Event::Empty(ht)).unwrap();

            let mut ft = BytesStart::new("AF_FORM_TITLE");
            ft.push_attribute(("jcr:primaryType", "nt:unstructured"));
            ft.push_attribute((
                "resourceType",
                "granite/ui/components/coral/foundation/form/textarea",
            ));
            ft.push_attribute(("valueFrom", "formTitle"));
            w.write_event(Event::Empty(ft)).unwrap();

            w.write_event(Event::End(BytesEnd::new("items"))).unwrap();
            w.write_event(Event::End(BytesEnd::new("HeaderInfo")))
                .unwrap();
        }

        // Adressblock
        {
            let mut ab = BytesStart::new("Adressblock");
            ab.push_attribute(("jcr:primaryType", "nt:unstructured"));
            w.write_event(Event::Start(ab)).unwrap();

            let mut ab_items = BytesStart::new("items");
            ab_items.push_attribute(("jcr:primaryType", "nt:unstructured"));
            w.write_event(Event::Start(ab_items)).unwrap();

            let mut sat = BytesStart::new("senderAddressTitle");
            sat.push_attribute(("jcr:primaryType", "nt:unstructured"));
            sat.push_attribute((
                "resourceType",
                "granite/ui/components/coral/foundation/form/textarea",
            ));
            sat.push_attribute(("valueFrom", "template"));
            w.write_event(Event::Empty(sat)).unwrap();

            w.write_event(Event::End(BytesEnd::new("items"))).unwrap();
            w.write_event(Event::End(BytesEnd::new("Adressblock")))
                .unwrap();
        }

        w.write_event(Event::End(BytesEnd::new("items"))).unwrap();
        w.write_event(Event::End(BytesEnd::new("Header"))).unwrap();
    }

    // Remaining simple elements
    for name in &[
        "formId",
        "displayLanguage",
        "language",
        "footerVersion",
        "mandator",
        "footerFormCode",
        "footerFormVersionDate",
    ] {
        let mut e = BytesStart::new(*name);
        e.push_attribute(("jcr:primaryType", "nt:unstructured"));
        w.write_event(Event::Empty(e)).unwrap();
    }

    // APPCode
    let mut app_code = BytesStart::new("APPCode");
    app_code.push_attribute(("jcr:primaryType", "nt:unstructured"));
    app_code.push_attribute(("value", "AFC"));
    w.write_event(Event::Empty(app_code)).unwrap();

    // footerFreeText
    let mut fft = BytesStart::new("footerFreeText");
    fft.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Empty(fft)).unwrap();

    w.write_event(Event::End(BytesEnd::new("items"))).unwrap(); // masterpage0 items
    w.write_event(Event::End(BytesEnd::new("masterpage0")))
        .unwrap();
    w.write_event(Event::End(BytesEnd::new("items"))).unwrap(); // branding items
    w.write_event(Event::End(BytesEnd::new("branding")))
        .unwrap();
}

/// Write `<assets>` with language dictionary entries.
fn write_assets(w: &mut Writer<&mut Cursor<Vec<u8>>>, config: &AemConfig) {
    let mut assets = BytesStart::new("assets");
    assets.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Start(assets)).unwrap();

    let mut dict = BytesStart::new("dictionary");
    dict.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Start(dict)).unwrap();

    for lang in &config.languages {
        let lang_elem = BytesStart::new(lang.as_str());
        w.write_event(Event::Empty(lang_elem)).unwrap();
    }

    w.write_event(Event::End(BytesEnd::new("dictionary")))
        .unwrap();
    w.write_event(Event::End(BytesEnd::new("assets"))).unwrap();
}

fn write_root_panel(
    w: &mut Writer<&mut Cursor<Vec<u8>>>,
    children: &[AemNode],
    config: &AemConfig,
) {
    let mut elem = BytesStart::new("rootPanel");
    elem.push_attribute(("jcr:lastModified", "{Date}2025-01-01T00:00:00.000+01:00"));
    elem.push_attribute(("jcr:lastModifiedBy", config.author.as_str()));
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute(("jcr:title", "Root Panel"));
    elem.push_attribute(("sling:resourceType", "fd/af/components/rootPanel"));
    elem.push_attribute(("completionExpReq", "{Boolean}true"));
    elem.push_attribute(("dorExcludeDescription", "true"));
    elem.push_attribute(("dorExcludeTitle", "true"));
    elem.push_attribute(("guideNodeClass", "rootPanelNode"));
    elem.push_attribute(("name", "guideRootPanel"));
    elem.push_attribute(("panelSetType", "Navigable"));
    elem.push_attribute(("validateOnStepCompletion", "{Boolean}true"));
    w.write_event(Event::Start(elem)).unwrap();

    // <layout> — wizard layout for root panel
    {
        let mut layout = BytesStart::new("layout");
        layout.push_attribute(("jcr:primaryType", "nt:unstructured"));
        layout.push_attribute(("sling:resourceType", config.wizard_layout.as_str()));
        if config.enable_layout_optimization {
            layout.push_attribute(("enableLayoutOptimization", "true"));
        }
        layout.push_attribute(("guideNavigatorTab", "wizard-tab"));
        layout.push_attribute(("toolbarPosition", "Bottom"));
        w.write_event(Event::Empty(layout)).unwrap();
    }

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
        is_page,
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
    if *is_page {
        elem.push_attribute(("css", "stepTitle"));
    }
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
    if *is_page {
        elem.push_attribute(("dorExcludeTitle", "true"));
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
    elem.push_attribute(("dorExclusion", bool_str(!visible)));
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
    elem.push_attribute(("dorExclusion", bool_str(!visible)));
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
    elem.push_attribute(("dorExclusion", bool_str(!visible)));
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
    let escaped_content = escape_xml_attr(content);
    elem.push_attribute(("_value".as_bytes(), escaped_content.as_bytes()));
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

    let panel_name = format!("PN_{name}");

    // Outer wrapping panel (panel_repeatable_<uuid>)
    let outer_tag = format!("repeatable_{}", uuid.as_simple());
    let mut outer = BytesStart::new(outer_tag.clone());
    push_jcr_timestamps(&mut outer, config);
    outer.push_attribute(("jcr:primaryType", "nt:unstructured"));
    outer.push_attribute(("sling:resourceType", config.panel_resource_type().as_str()));
    outer.push_attribute(("dorExcludeDescription", "true"));
    outer.push_attribute(("dorExcludeTitle", "true"));
    outer.push_attribute(("guideNodeClass", "guidePanel"));
    outer.push_attribute(("name", name.as_str()));
    outer.push_attribute(("textIsRich", "true"));
    outer.push_attribute(("validateOnStepCompletion", "{Boolean}false"));
    w.write_event(Event::Start(outer)).unwrap();

    // Outer <layout>
    write_repeatable_layout(w, config);

    // Outer <items>
    write_items_start(w, config);

    // Inner repeatable panel with minOccur/maxOccur — uses custom panel resource type
    {
        let inner_tag = "repeatableInner";
        let mut inner = BytesStart::new(inner_tag);
        push_jcr_timestamps(&mut inner, config);
        inner.push_attribute(("jcr:primaryType", "nt:unstructured"));
        inner.push_attribute((
            "sling:resourceType",
            config.control_resource_type("panel").as_str(),
        ));
        inner.push_attribute(("dorExcludeDescription", "true"));
        inner.push_attribute(("dorExcludeTitle", "true"));
        inner.push_attribute(("guideNodeClass", "guidePanel"));
        inner.push_attribute(("maxOccur", max_occur.to_string().as_str()));
        inner.push_attribute(("minOccur", min_occur.to_string().as_str()));
        inner.push_attribute(("name", panel_name.as_str()));
        inner.push_attribute(("textIsRich", "true"));
        inner.push_attribute(("validateOnStepCompletion", "{Boolean}false"));
        w.write_event(Event::Start(inner)).unwrap();

        // Inner <layout>
        write_repeatable_layout(w, config);

        // Inner <items>
        write_items_start(w, config);

        // Remove button
        write_repeatable_remove_button(w, config);

        // Children wrapper panel (panel_copy_copy)
        {
            let mut wrapper = BytesStart::new("panel_copy_copy");
            push_jcr_timestamps(&mut wrapper, config);
            wrapper.push_attribute(("jcr:primaryType", "nt:unstructured"));
            wrapper.push_attribute((
                "sling:resourceType",
                config.control_resource_type("panel").as_str(),
            ));
            wrapper.push_attribute(("dorExcludeDescription", "true"));
            wrapper.push_attribute(("dorExcludeTitle", "true"));
            wrapper.push_attribute(("guideNodeClass", "guidePanel"));
            wrapper.push_attribute(("name", format!("panel_{title}").as_str()));
            wrapper.push_attribute(("textIsRich", "true"));
            wrapper.push_attribute(("validateOnStepCompletion", "{Boolean}false"));
            w.write_event(Event::Start(wrapper)).unwrap();

            // Wrapper <layout>
            write_repeatable_layout(w, config);

            // Wrapper <items>
            write_items_start(w, config);
            for child in children {
                write_node(w, child, config);
            }
            write_items_end(w);

            // <cq:responsive>
            write_responsive(w, 12);

            w.write_event(Event::End(BytesEnd::new("panel_copy_copy")))
                .unwrap();
        }

        write_items_end(w); // inner items

        w.write_event(Event::End(BytesEnd::new(inner_tag))).unwrap();
    }

    // Add button (tertiarybutton)
    write_repeatable_add_button(w, &panel_name, config);

    write_items_end(w); // outer items

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
    elem.push_attribute(("columns", "1"));
    elem.push_attribute(("dorLayoutType", "columnar"));
    if non_navigable {
        elem.push_attribute(("nonNavigable", "{Boolean}true"));
    }
    elem.push_attribute(("toolbarPosition", "Bottom"));
    w.write_event(Event::Empty(elem)).unwrap();
}

/// Write `<layout>` element for repeatable panels (includes columns and DOR layout attributes).
fn write_repeatable_layout(w: &mut Writer<&mut Cursor<Vec<u8>>>, config: &AemConfig) {
    let mut elem = BytesStart::new("layout");
    elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
    elem.push_attribute(("sling:resourceType", config.default_layout.as_str()));
    elem.push_attribute(("columns", "1"));
    elem.push_attribute(("dorLayoutType", "columnar"));
    elem.push_attribute(("dorNumCols", "1"));
    elem.push_attribute(("nonNavigable", "{Boolean}true"));
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
    toolbar.push_attribute(("jcr:title", "Toolbar"));
    toolbar.push_attribute(("sling:resourceType", "fd/af/components/toolbar"));
    toolbar.push_attribute(("css", ""));
    toolbar.push_attribute(("guideNodeClass", "guideToolbar"));
    toolbar.push_attribute(("name", "toolbar"));
    w.write_event(Event::Start(toolbar)).unwrap();

    // <items> with toolbar layout
    let mut items = BytesStart::new("items");
    items.push_attribute(("jcr:primaryType", "nt:unstructured"));
    items.push_attribute((
        "sling:resourceType",
        "fd/af/layouts/toolbar/defaultToolbarLayout",
    ));
    w.write_event(Event::Start(items)).unwrap();

    // Previous — simple button
    {
        let mut elem = BytesStart::new("previtemnav");
        elem.push_attribute(("fd:targetVersion", "1.1"));
        elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
        elem.push_attribute(("jcr:title", "Back"));
        elem.push_attribute(("sling:resourceType", "fd/af/components/actions/previtemnav"));
        elem.push_attribute(("dorExclusion", "true"));
        elem.push_attribute(("guideNodeClass", "guideButton"));
        elem.push_attribute(("name", "previtemnav"));
        elem.push_attribute(("type", "movePrev"));
        w.write_event(Event::Empty(elem)).unwrap();
    }

    // Next — has fd:rules and fd:scripts
    {
        let mut elem = BytesStart::new("nextitemnav");
        elem.push_attribute(("fd:targetVersion", "1.1"));
        elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
        elem.push_attribute(("jcr:title", "Next"));
        elem.push_attribute(("sling:resourceType", "fd/af/components/actions/nextitemnav"));
        elem.push_attribute(("dorExclusion", "true"));
        elem.push_attribute(("guideNodeClass", "guideButton"));
        elem.push_attribute(("name", "nextitemnav"));
        elem.push_attribute(("type", "moveNext"));
        w.write_event(Event::Start(elem)).unwrap();

        let rules = BytesStart::new("fd:rules");
        w.write_event(Event::Empty(rules)).unwrap();

        let mut scripts = BytesStart::new("fd:scripts");
        scripts.push_attribute((
            "fd:click",
            "[{\"script\":{\"field\":\"guide.guideRootPanel.toolbar.nextitemnav\"\\,\"event\":\"Click\"\\,\"model\":{\"nodeName\":\"EVENT_SCRIPTS\"}\\,\"content\":\"window.com.ajila.forms.ubs.navigation.nextStep(this);\"}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true}]",
        ));
        scripts.push_attribute(("jcr:primaryType", "nt:unstructured"));
        w.write_event(Event::Empty(scripts)).unwrap();

        w.write_event(Event::End(BytesEnd::new("nextitemnav")))
            .unwrap();
    }

    // Submit — has fd:rules and fd:scripts
    {
        let mut elem = BytesStart::new("submit");
        elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
        elem.push_attribute(("jcr:title", "Submit"));
        elem.push_attribute(("sling:resourceType", "fd/af/components/actions/submit"));
        elem.push_attribute(("dorExclusion", "true"));
        elem.push_attribute(("guideNodeClass", "guideButton"));
        elem.push_attribute(("name", "submit"));
        elem.push_attribute(("type", "submit"));
        w.write_event(Event::Start(elem)).unwrap();

        let mut rules = BytesStart::new("fd:rules");
        rules.push_attribute((
            "fd:click",
            "[{\"nodeName\":\"ROOT\"\\,\"items\":[{\"nodeName\":\"STATEMENT\"\\,\"choice\":{\"nodeName\":\"EVENT_SCRIPTS\"\\,\"items\":[{\"nodeName\":\"EVENT_CONDITION\"\\,\"choice\":{\"nodeName\":\"EVENT_AND_COMPARISON\"\\,\"items\":[{\"nodeName\":\"COMPONENT\"\\,\"value\":{\"id\":\"guide.guideRootPanel.toolbar.submit\"\\,\"type\":\"BUTTON\"\\,\"name\":\"submit\"}}\\,{\"nodeName\":\"EVENT_AND_COMPARISON_OPERATOR\"\\,\"choice\":{\"nodeName\":\"is clicked\"\\,\"value\":null}}\\,{\"nodeName\":\"PRIMITIVE_EXPRESSION\"\\,\"choice\":null}]}\\,\"nested\":false}\\,{\"nodeName\":\"Then\"\\,\"value\":null}\\,{\"nodeName\":\"BLOCK_STATEMENTS\"\\,\"items\":[{\"nodeName\":\"BLOCK_STATEMENT\"\\,\"choice\":{\"nodeName\":\"SUBMIT_FORM\"\\,\"items\":[]}}]}]}}]\\,\"isValid\":true\\,\"enabled\":true\\,\"version\":1\\,\"script\":\"/**\\\\n\\\\n * This is a machine-generated code for the rule.\\\\n * If you modify it in the code editor\\, you will not be able to view and edit the rule in the visual editor.\\\\n */\\\\n\\\\nguideBridge.submit();\\\\n\\\\n\"\\,\"eventName\":\"Click\"\\,\"ruleType\":\"\"\\,\"description\":\"\"}]",
        ));
        rules.push_attribute(("jcr:primaryType", "nt:unstructured"));
        w.write_event(Event::Empty(rules)).unwrap();

        let mut scripts = BytesStart::new("fd:scripts");
        scripts.push_attribute((
            "fd:navigationChange",
            "[{\"script\":{\"content\":\"this.visible=(!this.panel.navigationContext.hasNextItem);\"\\,\"event\":\"Navigation\"\\,\"field\":\"guide.guideRootPanel.toolbar.submit\"}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true}]",
        ));
        scripts.push_attribute(("jcr:primaryType", "nt:unstructured"));
        w.write_event(Event::Empty(scripts)).unwrap();

        w.write_event(Event::End(BytesEnd::new("submit"))).unwrap();
    }

    // Preview button
    {
        let mut elem = BytesStart::new("preview");
        push_jcr_timestamps(&mut elem, config);
        elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
        elem.push_attribute(("jcr:title", "Preview"));
        elem.push_attribute((
            "sling:resourceType",
            config.control_resource_type("tertiarybutton").as_str(),
        ));
        elem.push_attribute(("css", "previewGenerationButton"));
        elem.push_attribute(("dorExclusion", "true"));
        elem.push_attribute(("guideNodeClass", "guideButton"));
        elem.push_attribute(("name", "preview"));
        elem.push_attribute(("textIsRich", "[true,true]"));
        elem.push_attribute(("type", "Button"));
        w.write_event(Event::Start(elem)).unwrap();

        let mut rules = BytesStart::new("fd:rules");
        rules.push_attribute((
            "fd:click",
            "[{\"nodeName\":\"ROOT\"\\,\"items\":[{\"nodeName\":\"STATEMENT\"\\,\"choice\":{\"nodeName\":\"EVENT_SCRIPTS\"\\,\"items\":[{\"nodeName\":\"EVENT_CONDITION\"\\,\"choice\":{\"nodeName\":\"EVENT_AND_COMPARISON\"\\,\"items\":[{\"nodeName\":\"COMPONENT\"\\,\"value\":{\"id\":\"guide.guideRootPanel.toolbar.preview\"\\,\"type\":\"BUTTON|AFCOMPONENT|TOOLBAR_BUTTON\"\\,\"name\":\"preview\"}}\\,{\"nodeName\":\"EVENT_AND_COMPARISON_OPERATOR\"\\,\"choice\":{\"nodeName\":\"is clicked\"\\,\"value\":null}}\\,{\"nodeName\":\"PRIMITIVE_EXPRESSION\"\\,\"choice\":null}]}\\,\"nested\":false}\\,{\"nodeName\":\"Then\"\\,\"value\":null}\\,{\"nodeName\":\"BLOCK_STATEMENTS\"\\,\"items\":[{\"nodeName\":\"BLOCK_STATEMENT\"\\,\"choice\":{\"nodeName\":\"SHOW_STATEMENT\"\\,\"items\":[{\"nodeName\":\"AFCOMPONENT\"\\,\"value\":{\"id\":\"guide.guideRootPanel.panel1597308651601.carouselPreview\"\\,\"displayName\":\"Carousel\"\\,\"type\":\"AFCOMPONENT\"\\,\"displayPath\":\"FORM/Root Panel/panel1597308651601/Carousel/\"\\,\"name\":\"carouselPreview\"\\,\"parent\":\"guide.guideRootPanel.panel1597308651601\"}}]}}]}]}}]\\,\"isValid\":true\\,\"enabled\":true\\,\"version\":1\\,\"script\":\"/**\\\\n\\\\n * This is a machine-generated code for the rule.\\\\n * If you modify it in the code editor\\, you will not be able to view and edit the rule in the visual editor.\\\\n */\\\\n\\\\ncarouselPreview.visible = true;\\\\n\\\\n\"\\,\"eventName\":\"Click\"\\,\"ruleType\":\"\"\\,\"description\":\"\"}]",
        ));
        rules.push_attribute(("jcr:primaryType", "nt:unstructured"));
        w.write_event(Event::Empty(rules)).unwrap();

        let mut scripts = BytesStart::new("fd:scripts");
        scripts.push_attribute((
            "fd:click",
            "[{\"script\":{\"field\":\"guide.guideRootPanel.toolbar.preview\"\\,\"event\":\"Click\"\\,\"model\":{\"nodeName\":\"EVENT_SCRIPTS\"}\\,\"content\":\"com.ajila.forms.control.carousel.initializeForPreview(carouselPreview\\, undefined\\, previewErrorMessage);\"}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true}]",
        ));
        scripts.push_attribute((
            "fd:navigationChange",
            "[{\"script\":{\"content\":\"this.visible=(!this.panel.navigationContext.hasNextItem);\"\\,\"event\":\"Navigation\"\\,\"field\":\"guide.guideRootPanel.toolbar.preview\"}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true}]",
        ));
        scripts.push_attribute(("jcr:primaryType", "nt:unstructured"));
        w.write_event(Event::Empty(scripts)).unwrap();

        w.write_event(Event::End(BytesEnd::new("preview"))).unwrap();
    }

    // </items>
    w.write_event(Event::End(BytesEnd::new("items"))).unwrap();

    // <layout>
    let mut layout = BytesStart::new("layout");
    layout.push_attribute(("jcr:primaryType", "nt:unstructured"));
    layout.push_attribute((
        "sling:resourceType",
        "fd/af/layouts/toolbar/defaultToolbarLayout",
    ));
    w.write_event(Event::Empty(layout)).unwrap();

    w.write_event(Event::End(BytesEnd::new("toolbar"))).unwrap();
}

/// Write the remove button inside a repeatable panel's items.
fn write_repeatable_remove_button(w: &mut Writer<&mut Cursor<Vec<u8>>>, config: &AemConfig) {
    let mut btn = BytesStart::new("removebutton");
    push_jcr_timestamps(&mut btn, config);
    btn.push_attribute(("jcr:primaryType", "nt:unstructured"));
    btn.push_attribute((
        "sling:resourceType",
        config.control_resource_type("removebutton").as_str(),
    ));
    btn.push_attribute(("dorExclusion", "true"));
    btn.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    btn.push_attribute(("guideNodeClass", "guideButton"));
    btn.push_attribute(("name", "BT_Remove"));
    btn.push_attribute(("textIsRich", "[true,true]"));
    btn.push_attribute(("type", "Button"));
    btn.push_attribute(("visible", "{Boolean}false"));
    w.write_event(Event::Start(btn)).unwrap();

    // <fd:rules>
    let rules = BytesStart::new("fd:rules");
    w.write_event(Event::Empty(rules)).unwrap();

    // <fd:scripts> — remove instance + show/hide remove buttons + re-show add button
    let mut scripts = BytesStart::new("fd:scripts");
    scripts.push_attribute((
        "fd:click",
        "[{\"script\":{\"content\":\"this.parent.instanceManager.removeInstance(this.parent.instanceIndex);\"\\,\"event\":\"Click\"\\,\"field\":\"\"}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true},{\"script\":{\"content\":\"var len = this.parent.instanceManager.instances.length;\\\\nfor(var i = 0; i < len; i++){\\\\n  this.parent.instanceManager.instances[i].BT_Remove.visible = (i === (len-1) && len > 1) ? true : false;\\\\n}\\\\n\\\\nBT_Add.visible = true;\"\\,\"event\":\"Click\"\\,\"field\":\"\"}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true}]",
    ));
    scripts.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Empty(scripts)).unwrap();

    w.write_event(Event::End(BytesEnd::new("removebutton")))
        .unwrap();
}

/// Write the Add button for a repeatable section.
fn write_repeatable_add_button(
    w: &mut Writer<&mut Cursor<Vec<u8>>>,
    panel_name: &str,
    config: &AemConfig,
) {
    let mut btn = BytesStart::new("tertiarybutton");
    push_jcr_timestamps(&mut btn, config);
    btn.push_attribute(("jcr:primaryType", "nt:unstructured"));
    btn.push_attribute(("jcr:title", "Add"));
    btn.push_attribute((
        "sling:resourceType",
        config.control_resource_type("tertiarybutton").as_str(),
    ));
    btn.push_attribute(("dorExclusion", "true"));
    btn.push_attribute(("dorFieldStyling", config.dor_field_styling.as_str()));
    btn.push_attribute(("guideNodeClass", "guideButton"));
    btn.push_attribute(("name", "BT_Add"));
    btn.push_attribute(("textIsRich", "[true,true]"));
    btn.push_attribute(("type", "Button"));
    w.write_event(Event::Start(btn)).unwrap();

    // <fd:rules>
    let rules = BytesStart::new("fd:rules");
    w.write_event(Event::Empty(rules)).unwrap();

    // <fd:scripts> — add instance + show/hide remove buttons + hide add at max
    let add_click_template = "[{\"script\":{\"content\":\"__PN__.instanceManager.addInstance();\"\\,\"event\":\"Click\"\\,\"field\":\"\"}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true},{\"script\":{\"content\":\"var len = __PN__.instanceManager.instances.length;\\\\n\\\\nfor(var i = 0; i < len; i++){\\\\n  __PN__.instanceManager.instances[i].BT_Remove.visible = (i === (len-1)) ? true : false;\\\\n}\\\\n\\\\n\"\\,\"event\":\"Click\"\\,\"field\":\"\"}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true},{\"script\":{\"content\":\"var myInstance = __PN__.instanceManager.instances.length;\\\\n\\\\nif(myInstance >= parseInt(__PN__.maxOccur)) {\\\\n  this.visible = false;\\\\n}\"\\,\"event\":\"Click\"\\,\"field\":\"\"}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true}]";
    let add_click_value = add_click_template.replace("__PN__", panel_name);
    let mut scripts = BytesStart::new("fd:scripts");
    scripts.push_attribute(("fd:click", add_click_value.as_str()));
    scripts.push_attribute(("jcr:primaryType", "nt:unstructured"));
    w.write_event(Event::Empty(scripts)).unwrap();

    w.write_event(Event::End(BytesEnd::new("tertiarybutton")))
        .unwrap();
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
        // Control resource type must use custom base
        assert!(
            xml.contains("sling:resourceType=\"ajila-forms-customers/ajila-forms-ubs/components/controls/textdraw\""),
            "textdraw resource type should use custom base"
        );
        assert!(
            !xml.contains("components/components"),
            "resource type must not have doubled /components/"
        );
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
        // Toolbar element with correct resource type
        assert!(
            xml.contains("sling:resourceType=\"fd/af/components/toolbar\""),
            "toolbar should use fd/af/components/toolbar"
        );
        assert!(xml.contains("guideNodeClass=\"guideToolbar\""));
        // Toolbar items use toolbar layout
        assert!(
            xml.contains("sling:resourceType=\"fd/af/layouts/toolbar/defaultToolbarLayout\""),
            "toolbar items should use toolbar layout"
        );
        // Button resource types include actions/ prefix
        assert!(
            xml.contains("sling:resourceType=\"fd/af/components/actions/previtemnav\""),
            "previtemnav should use actions/ prefix"
        );
        assert!(
            xml.contains("sling:resourceType=\"fd/af/components/actions/nextitemnav\""),
            "nextitemnav should use actions/ prefix"
        );
        assert!(
            xml.contains("sling:resourceType=\"fd/af/components/actions/submit\""),
            "submit should use actions/ prefix"
        );
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

        // Inner panel carries minOccur/maxOccur
        assert!(xml.contains("minOccur=\"1\""), "missing minOccur");
        assert!(xml.contains("maxOccur=\"10\""), "missing maxOccur");

        // Remove button inside items (not toolbar)
        assert!(xml.contains("<removebutton"), "missing removebutton");
        assert!(
            xml.contains("name=\"BT_Remove\""),
            "removebutton should be named BT_Remove"
        );
        assert!(
            xml.contains("visible=\"{Boolean}false\""),
            "removebutton should be hidden by default"
        );

        // Add button as tertiarybutton
        assert!(
            xml.contains("<tertiarybutton"),
            "missing tertiarybutton (Add)"
        );
        assert!(
            xml.contains("name=\"BT_Add\""),
            "add button should be named BT_Add"
        );
        assert!(
            xml.contains("jcr:title=\"Add\""),
            "add button title should be 'Add'"
        );

        // Inner panel uses custom resource type
        let config = test_config();
        assert!(
            xml.contains(&config.control_resource_type("panel")),
            "inner panel should use custom panel resource type"
        );

        // panel_copy_copy wrapper exists
        assert!(
            xml.contains("<panel_copy_copy"),
            "children should be wrapped in panel_copy_copy"
        );

        // Outer panel name
        assert!(
            xml.contains("name=\"RPT_1\""),
            "outer panel should carry the repeatable name"
        );

        // Inner panel name = PN_<name>
        assert!(
            xml.contains("name=\"PN_RPT_1\""),
            "inner panel should be named PN_RPT_1"
        );

        // Remove button has fd:scripts with remove-instance and show/hide logic
        assert!(
            xml.contains("this.parent.instanceManager.removeInstance(this.parent.instanceIndex);"),
            "remove button should have remove-instance script"
        );
        assert!(
            xml.contains("BT_Add.visible = true;"),
            "remove button script should re-show add button"
        );

        // Add button has fd:scripts with panel_name-based scripts
        assert!(
            xml.contains("PN_RPT_1.instanceManager.addInstance();"),
            "add button should have addInstance script using panel name"
        );
        assert!(
            xml.contains("PN_RPT_1.instanceManager.instances.length;"),
            "add button should reference panel instances"
        );
        assert!(
            xml.contains("PN_RPT_1.maxOccur"),
            "add button should check maxOccur to hide itself"
        );
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

    #[test]
    fn textdraw_html_preserves_gt_unescaped() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::TextDraw {
                uuid: fixed_uuid(),
                name: "ST_1".into(),
                content: "<h1>Title</h1>".into(),
                dor_exclude: false,
                colspan: 12,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        // > must NOT be escaped — only < and & are encoded
        assert!(
            xml.contains("&lt;h1>Title&lt;/h1>"),
            "> should stay unescaped in _value attribute. Got: {}",
            xml
        );
        assert!(
            !xml.contains("&lt;h1&gt;"),
            "> must not be escaped to &gt; in _value attribute",
        );
    }

    #[test]
    fn escape_xml_attr_only_escapes_lt_amp_quot() {
        assert_eq!(escape_xml_attr("<h1>Title</h1>"), "&lt;h1>Title&lt;/h1>");
        assert_eq!(escape_xml_attr("A & B"), "A &amp; B");
        assert_eq!(escape_xml_attr("no special"), "no special");
        assert_eq!(escape_xml_attr("say \"hello\""), "say &quot;hello&quot;");
    }
}
