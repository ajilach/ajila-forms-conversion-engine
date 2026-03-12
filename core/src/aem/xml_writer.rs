//! XML serialization of an `AemNode` tree into AEM JCR content XML.
//!
//! Uses Tera templates loaded from the profile directory. Each `AemNode` type
//! is rendered by its corresponding `*.xml` template file. The `root.xml`
//! template is the entire XML document — the writer itself generates no XML
//! tags.

use std::collections::HashMap;

use super::{AemConfig, AemNode, AemOption, ConditionRule, OptionAlignment};
use crate::aem::template;
use crate::structured::InputValue;

// ============================================================================
// Public API
// ============================================================================

/// Serialize an `AemNode` tree (starting from `Root`) into a complete AEM
/// JCR content XML string.
///
/// Each node is rendered by the correspondingly named template from
/// `config.component_templates`. Attributes are post-processed to appear
/// one per line (matching AEM's export style).
pub fn generate_aem_xml(root: &AemNode, config: &AemConfig) -> String {
    let rendered = render_node(root, config);
    reformat_attributes(&rendered)
}

// ============================================================================
// Template-based node rendering
// ============================================================================

/// Render a single node using its template from `config.component_templates`.
///
/// If no template exists for the node type, an empty string is returned
/// (the component is omitted from the output).
fn render_node(node: &AemNode, config: &AemConfig) -> String {
    let template_key = match node {
        AemNode::Root { .. } => "root",
        AemNode::Panel { .. } => "panel",
        AemNode::TextField { .. } => "textbox",
        AemNode::NumberField { .. } => "numericbox",
        AemNode::DatePicker { .. } => "datepicker",
        AemNode::Dropdown { .. } => "dropdownlist",
        AemNode::Checkbox { .. } => "checkbox",
        AemNode::RadioButton { .. } => "radiobutton",
        AemNode::TextDraw { .. } => "textdraw",
        AemNode::TitleDraw { .. } => "titledraw",
        AemNode::TextBoxMultiline { .. } => "textbox_multiline",
        AemNode::Repeatable { .. } => "repeatable",
    };

    let template = match config.component_templates.get(template_key) {
        Some(tmpl) => tmpl,
        None => return String::new(),
    };

    let ctx = build_node_context(node, config);
    match template::render_string(template, &ctx) {
        Ok(rendered) => rendered,
        Err(e) => {
            log::error!("Failed to render template '{}': {}", template_key, e);
            String::new()
        }
    }
}

/// Render all children of a node and concatenate the results.
fn render_children(children: &[AemNode], config: &AemConfig) -> String {
    children.iter().map(|c| render_node(c, config)).collect()
}

/// Build a Tera context for a single node.
///
/// The context contains:
/// - Global variables: `xfa.*`, `variables.*`, `author`, `master_language`,
///   `languages`, `expanded_languages`
/// - Node-specific variables depending on the variant
fn build_node_context(node: &AemNode, config: &AemConfig) -> tera::Context {
    let mut ctx = tera::Context::new();

    // ── Global context ─────────────────────────────────────────────────
    ctx.insert("xfa", &config.xfa_vars);
    ctx.insert("variables", &config.user_vars);
    ctx.insert("author", &config.author);
    ctx.insert("master_language", &config.master_language);
    ctx.insert("languages", &config.languages.join(","));
    ctx.insert("expanded_languages", &config.expand_languages().join(","));

    // ── Node-specific context ──────────────────────────────────────────
    ctx.insert("element_name", &node.element_name());

    match node {
        AemNode::Root { title, children } => {
            ctx.insert("title", &xml_escape(title));
            ctx.insert("form_code", &config.form_code);
            ctx.insert("children", &render_children(children, config));
        }

        AemNode::Panel {
            uuid,
            name,
            title,
            children,
            is_page,
            dor_exclude,
            visible,
            dor_num_cols,
            colspan,
            dor_colspan,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("title", &xml_escape(title));
            ctx.insert("is_page", is_page);
            ctx.insert("dor_exclude", dor_exclude);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_num_cols", dor_num_cols);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("children", &render_children(children, config));
        }

        AemNode::TextField {
            uuid,
            name,
            label,
            mandatory,
            visible,
            max_chars,
            colspan,
            dor_colspan,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("max_chars", max_chars);
            ctx.insert("dor_colspan", dor_colspan);
        }

        AemNode::NumberField {
            uuid,
            name,
            label,
            mandatory,
            visible,
            colspan,
            dor_colspan,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
        }

        AemNode::DatePicker {
            uuid,
            name,
            label,
            mandatory,
            visible,
            colspan,
            dor_colspan,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
        }

        AemNode::Dropdown {
            uuid,
            name,
            label,
            options,
            mandatory,
            visible,
            colspan,
            dor_colspan,
            field_id: _,
            conditions,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
            insert_options_context(&mut ctx, options);
            insert_conditions_context(&mut ctx, name, conditions);
        }

        AemNode::Checkbox {
            uuid,
            name,
            options,
            alignment,
            visible,
            colspan,
            dor_colspan,
            field_id: _,
            conditions,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("alignment", alignment_str(*alignment));
            insert_options_context(&mut ctx, options);
            insert_conditions_context(&mut ctx, name, conditions);
            // text_is_rich: array of booleans indicating rich text options
            let text_is_rich: Vec<bool> = options.iter().map(|o| o.label.contains('<')).collect();
            let text_is_rich_str = format!(
                "[{}]",
                text_is_rich
                    .iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            ctx.insert("text_is_rich", &text_is_rich_str);
        }

        AemNode::RadioButton {
            uuid,
            name,
            label,
            options,
            alignment,
            mandatory,
            visible,
            colspan,
            dor_colspan,
            field_id: _,
            conditions,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
            ctx.insert("alignment", alignment_str(*alignment));
            insert_options_context(&mut ctx, options);
            insert_conditions_context(&mut ctx, name, conditions);
            // text_is_rich
            let text_is_rich: Vec<bool> = options.iter().map(|o| o.label.contains('<')).collect();
            let text_is_rich_str = format!(
                "[{}]",
                text_is_rich
                    .iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            ctx.insert("text_is_rich", &text_is_rich_str);
        }

        AemNode::TextDraw {
            uuid,
            name,
            content,
            dor_exclude,
            colspan,
            dor_colspan,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("content", &xml_escape(content));
            ctx.insert("dor_exclude", dor_exclude);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
        }

        AemNode::TitleDraw {
            uuid,
            name,
            content,
            heading_level,
            colspan,
            dor_colspan,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("content", &xml_escape(content));
            ctx.insert("heading_level", heading_level);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
        }

        AemNode::TextBoxMultiline {
            uuid,
            name,
            label,
            mandatory,
            visible,
            colspan,
            dor_colspan,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("label", &xml_escape(label));
            ctx.insert("mandatory", mandatory);
            ctx.insert("visible", visible);
            ctx.insert("colspan", colspan);
            ctx.insert("dor_colspan", dor_colspan);
        }

        AemNode::Repeatable {
            uuid,
            name,
            title,
            children,
            min_occur,
            max_occur,
        } => {
            ctx.insert("uuid", &uuid.as_simple().to_string());
            ctx.insert("name", name);
            ctx.insert("title", &xml_escape(title));
            ctx.insert("min_occur", min_occur);
            ctx.insert("max_occur", max_occur);
            ctx.insert("children", &render_children(children, config));

            // Pre-compute the repeatable button scripts as template variables.
            // These contain complex JCR-escaped JSON that would be very messy
            // to write inline in a Tera template.
            let panel_name = format!("PN_{}", name);
            ctx.insert("panel_name", &panel_name);
            insert_repeatable_scripts(&mut ctx, &panel_name, *max_occur);
        }
    }

    ctx
}

/// Insert options-related variables into a Tera context.
fn insert_options_context(ctx: &mut tera::Context, options: &[AemOption]) {
    ctx.insert("options_attr", &format_options_attr(options));
    ctx.insert("options_count", &options.len());
    let opt_list: Vec<HashMap<&str, &str>> = options
        .iter()
        .map(|o| {
            let mut m = HashMap::new();
            m.insert("label", o.label.as_str());
            m.insert("value", o.value.as_str());
            m
        })
        .collect();
    ctx.insert("options", &opt_list);
}

/// XML-escape a string for safe embedding inside an XML attribute value.
///
/// Encodes `&`, `"`, `<`, and `>` as their XML entity references. This is
/// needed for JSON script strings that will be placed into template
/// expressions like `fd:valueCommit="{{ conditions_script }}"`.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Insert conditions-related variables into a Tera context.
fn insert_conditions_context(
    ctx: &mut tera::Context,
    field_name: &str,
    conditions: &[ConditionRule],
) {
    if !conditions.is_empty() {
        ctx.insert(
            "conditions_script",
            &xml_escape(&generate_value_commit_json(field_name, conditions)),
        );
    }
}

/// Insert pre-computed repeatable button scripts into a Tera context.
///
/// Generates the `fd:click` / `fd:init` attribute values for the remove and
/// add buttons of a repeatable section. These are complex JCR-escaped JSON
/// strings that reference the panel instance manager.
fn insert_repeatable_scripts(ctx: &mut tera::Context, panel_name: &str, max_occur: u32) {
    // --- Remove button click script ---
    let remove_script = format!(
        "{pn}.instanceManager.removeInstance(this.parent.index);\\\
\
\
var len = {pn}.instanceManager.instances.length;\\\
for (var i = 0; i < len; i++) {{\\\
{pn}.instanceManager.instances[i].BT_Remove.visible = (i === (len - 1) && len > 1) ? true : false;\\\
}}",
        pn = panel_name
    );
    let remove_click_json = format!(
        "[{{\"script\":{{\"content\":\"{script}\"\\,\"event\":\"Click\"\\,\"field\":\"BT_Remove\"}}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true}}]",
        script = remove_script
    );
    ctx.insert("remove_click_json", &xml_escape(&remove_click_json));

    // --- Add button click script ---
    let add_click_script = format!(
        "{pn}.instanceManager.addInstance();\\\
\
\
var len = {pn}.instanceManager.instances.length;\\\
for (var i = 0; i < len; i++) {{\\\
{pn}.instanceManager.instances[i].BT_Remove.visible = (i === (len - 1) && len > 1) ? true : false;\\\
}}\\\
if (len >= {max}) {{\\\
this.visible = false;\\\
}}",
        pn = panel_name,
        max = max_occur
    );
    let add_click_json = format!(
        "[{{\"script\":{{\"content\":\"{script}\"\\,\"event\":\"Click\"\\,\"field\":\"BT_Add\"}}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true}}]",
        script = add_click_script
    );
    ctx.insert("add_click_json", &xml_escape(&add_click_json));

    // --- Add button init script ---
    let add_init_script = format!(
        "var len = {pn}.instanceManager.instances.length;\\\
for (var i = 0; i < len; i++) {{\\\
{pn}.instanceManager.instances[i].BT_Remove.visible = (i === (len - 1) && len > 1) ? true : false;\\\
}}\\\
if (len >= {max}) {{\\\
this.visible = false;\\\
}}",
        pn = panel_name,
        max = max_occur
    );
    let add_init_json = format!(
        "[{{\"script\":{{\"content\":\"{script}\"\\,\"event\":\"Initialize\"\\,\"field\":\"BT_Add\"}}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true}}]",
        script = add_init_script
    );
    ctx.insert("add_init_json", &xml_escape(&add_init_json));
}

// ============================================================================
// Conditional visibility scripts (fd:scripts fd:valueCommit)
// ============================================================================

/// Generate the JavaScript if-else chain for a `fd:valueCommit` script.
///
/// Groups conditions by target panel. For each panel, builds an
/// `if (this.value === 'X') { panel.visible = true; ... } else { ... }` block.
///
/// The returned string uses JCR/JSON escaping conventions:
/// - `\\n` for newlines (JCR `\\` → `\`, then JSON `\n` → newline)
/// - Single quotes in JS to avoid nested double-quote escaping
fn generate_value_commit_script(conditions: &[ConditionRule]) -> String {
    use std::collections::HashMap;

    // Group conditions by target panel name. For each panel we collect the
    // values that should make it visible.
    let mut by_panel: HashMap<&str, Vec<&InputValue>> = HashMap::new();
    for rule in conditions {
        if rule.show {
            by_panel
                .entry(&rule.target_panel_name)
                .or_default()
                .push(&rule.value);
        }
    }

    let mut script = String::new();

    for (panel_name, values) in &by_panel {
        // Build the condition: this.value === 'val1' || this.value === 'val2'
        // Uses single quotes in JS to avoid escaping issues within JSON strings.
        let cond_parts: Vec<String> = values
            .iter()
            .map(|v| {
                let val_str = match v {
                    InputValue::Text(s) => s.to_string(),
                    InputValue::Number(n) => n.to_string(),
                    InputValue::Bool(b) => b.to_string(),
                };
                // Use single quotes for string comparison in JS:
                // this.value === 'someValue'
                format!("this.value === '{}'", val_str)
            })
            .collect();
        let condition = cond_parts.join(" || ");

        // \\\\n in Rust source → \\n in Rust string → \\n in XML output
        // → JCR decodes \\ to \ giving \n → JSON decodes \n to newline
        script.push_str(&format!(
            "if ({}) {{\\\\n    {}.visible = true;\\\\n    {}.dorExclusion = false;\\\\n}} else {{\\\\n    {}.visible = false;\\\\n    {}.dorExclusion = true;\\\\n}}\\\\n",
            condition, panel_name, panel_name, panel_name, panel_name
        ));
    }

    script
}

/// Generate the escaped JSON string for the `fd:valueCommit` attribute.
///
/// The format is the `SCRIPTMODEL` pattern used throughout AEM Forms XML.
fn generate_value_commit_json(field_name: &str, conditions: &[ConditionRule]) -> String {
    let script_content = generate_value_commit_script(conditions);

    format!(
        "[{{\"script\":{{\"field\":\"{}\"\\,\"event\":\"Value Commit\"\\,\"model\":{{\"nodeName\":\"EVENT_SCRIPTS\"}}\\,\"content\":\"{}\"}}\\,\"nodeName\":\"SCRIPTMODEL\"\\,\"version\":1\\,\"enabled\":true}}]",
        field_name, script_content,
    )
}

// ============================================================================
// Attribute helpers
// ============================================================================

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
        .map(|o| format!("{}={}", xml_escape(&o.value), xml_escape(&o.label)))
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
    if let Some(stripped) = trimmed.strip_suffix("/>") {
        Some((stripped, "/>"))
    } else if let Some(stripped) = trimmed.strip_suffix('>') {
        Some((stripped, ">"))
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
    use crate::aem::{AemConfig, AemNode, AemOption, ConditionRule, OptionAlignment};
    use uuid::Uuid;

    /// Create a test config with minimal templates for testing.
    fn test_config() -> AemConfig {
        let mut config = AemConfig::test_default("TEST");
        config.deterministic_uuids = true;
        // Simple templates for testing — just enough to verify data flow
        config
            .component_templates
            .insert("root".into(), "{{ children }}".into());
        config.component_templates.insert(
            "panel".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ title }}\"{% if not visible %} visible=\"{Boolean}false\"{% endif %}{% if dor_exclude %} dorExclusion=\"true\"{% endif %}{% if dor_num_cols %} dorNumCols=\"{{ dor_num_cols }}\"{% endif %}{% if dor_colspan %} dorColspan=\"{{ dor_colspan }}\"{% endif %}>{{ children }}</{{ element_name }}>".into(),
        );
        config.component_templates.insert(
            "textbox".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ label }}\"{% if mandatory %} mandatory=\"{Boolean}true\"{% endif %}{% if max_chars %} maxChars=\"{{ max_chars }}\"{% endif %}{% if dor_colspan %} dorColspan=\"{{ dor_colspan }}\"{% endif %}><cq:responsive jcr:primaryType=\"nt:unstructured\"><default jcr:primaryType=\"nt:unstructured\" offset=\"0\" width=\"{{ colspan }}\"/></cq:responsive></{{ element_name }}>".into(),
        );
        config.component_templates.insert(
            "numericbox".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ label }}\"/>".into(),
        );
        config.component_templates.insert(
            "datepicker".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ label }}\"/>".into(),
        );
        config.component_templates.insert(
            "dropdownlist".into(),
            "<{{ element_name }} guideNodeClass=\"guideDropDownList\" name=\"{{ name }}\" jcr:title=\"{{ label }}\" options=\"{{ options_attr }}\"{% if conditions_script %}>{% if conditions_script %}<fd:scripts fd:valueCommit=\"{{ conditions_script }}\" jcr:primaryType=\"nt:unstructured\"/>{% endif %}</{{ element_name }}>{% else %}/>{% endif %}".into(),
        );
        config.component_templates.insert(
            "checkbox".into(),
            "<{{ element_name }} guideNodeClass=\"guideCheckBox\" name=\"{{ name }}\" options=\"{{ options_attr }}\" alignment=\"{{ alignment }}\"{% if conditions_script %}>{% if conditions_script %}<fd:scripts fd:valueCommit=\"{{ conditions_script }}\" jcr:primaryType=\"nt:unstructured\"/>{% endif %}</{{ element_name }}>{% else %}/>{% endif %}".into(),
        );
        config.component_templates.insert(
            "radiobutton".into(),
            "<{{ element_name }} guideNodeClass=\"guideRadioButton\" name=\"{{ name }}\" jcr:title=\"{{ label }}\" options=\"{{ options_attr }}\" alignment=\"{{ alignment }}\"{% if conditions_script %}>{% if conditions_script %}<fd:scripts fd:valueCommit=\"{{ conditions_script }}\" jcr:primaryType=\"nt:unstructured\"/>{% endif %}</{{ element_name }}>{% else %}/>{% endif %}".into(),
        );
        config.component_templates.insert(
            "textdraw".into(),
            "<{{ element_name }} guideNodeClass=\"guideTextDraw\" name=\"{{ name }}\" _value=\"{{ content }}\"/>".into(),
        );
        config.component_templates.insert(
            "titledraw".into(),
            "<{{ element_name }} guideNodeClass=\"guideTextDraw\" name=\"{{ name }}\" _value=\"{{ content }}\" headingLevel=\"{{ heading_level }}\"/>".into(),
        );
        config.component_templates.insert(
            "textbox_multiline".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ label }}\" multiLine=\"{Boolean}true\"/>".into(),
        );
        config.component_templates.insert(
            "repeatable".into(),
            "<{{ element_name }} name=\"{{ name }}\" jcr:title=\"{{ title }}\" minOccur=\"{{ min_occur }}\" maxOccur=\"{{ max_occur }}\">{{ children }}</{{ element_name }}>".into(),
        );
        config
    }

    fn fixed_uuid() -> Uuid {
        Uuid::new_v5(&Uuid::from_bytes([0; 16]), b"test")
    }

    #[test]
    fn xml_output_renders_textdraw() {
        let root = AemNode::Root {
            title: "Test Form".into(),
            children: vec![AemNode::TextDraw {
                uuid: fixed_uuid(),
                name: "ST_1".into(),
                content: "<p>Hello &amp; world</p>".into(),
                dor_exclude: false,
                colspan: 12,
                dor_colspan: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("guideTextDraw"));
        assert!(xml.contains("ST_1"));
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
                dor_colspan: None,
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
                dor_colspan: None,
                field_id: None,
                conditions: vec![],
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("options=\"[1=Yes,0=No]\""));
        assert!(xml.contains("alignment=\"horizontal\""));
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
        assert!(xml.contains("minOccur=\"1\""), "missing minOccur");
        assert!(xml.contains("maxOccur=\"10\""), "missing maxOccur");
        assert!(xml.contains("name=\"RPT_1\""));
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
                dor_colspan: None,
                field_id: None,
                conditions: vec![],
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(xml.contains("guideDropDownList"));
        assert!(xml.contains("options=\"[a=A,b=B]\""));
    }

    #[test]
    fn hidden_panel_emits_visible_false() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Panel {
                uuid: fixed_uuid(),
                name: "COND_Panel".into(),
                title: "Hidden Panel".into(),
                children: vec![],
                is_page: false,
                dor_exclude: true,
                visible: false,
                dor_num_cols: None,
                colspan: 12,
                dor_colspan: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains("visible=\"{Boolean}false\""),
            "Hidden panel should have visible={{Boolean}}false. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("dorExclusion=\"true\""),
            "Hidden panel should have dorExclusion=true. Got:\n{}",
            xml
        );
    }

    #[test]
    fn radio_button_with_conditions_emits_scripts() {
        use crate::structured::InputValue;

        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::RadioButton {
                uuid: fixed_uuid(),
                name: "RB_TriggerField".into(),
                label: "Choose".into(),
                options: vec![
                    AemOption {
                        label: "Yes".into(),
                        value: "yes".into(),
                    },
                    AemOption {
                        label: "No".into(),
                        value: "no".into(),
                    },
                ],
                alignment: OptionAlignment::Vertical,
                mandatory: false,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![ConditionRule {
                    target_panel_name: "COND_TargetPanel".into(),
                    value: InputValue::Text("yes".into()),
                    show: true,
                }],
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains("fd:scripts"),
            "Radio with conditions should emit fd:scripts. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("fd:valueCommit"),
            "Radio with conditions should emit fd:valueCommit. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("COND_TargetPanel.visible"),
            "Script content should reference target panel. Got:\n{}",
            xml
        );
    }

    #[test]
    fn radio_without_conditions_has_no_scripts() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::RadioButton {
                uuid: fixed_uuid(),
                name: "RB_Simple".into(),
                label: "Choose".into(),
                options: vec![AemOption {
                    label: "A".into(),
                    value: "a".into(),
                }],
                alignment: OptionAlignment::Vertical,
                mandatory: false,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![],
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            !xml.contains("fd:scripts"),
            "Radio without conditions should NOT emit fd:scripts. Got:\n{}",
            xml
        );
    }

    #[test]
    fn dropdown_with_conditions_emits_scripts() {
        use crate::structured::InputValue;

        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Dropdown {
                uuid: fixed_uuid(),
                name: "DD_Trigger".into(),
                label: "Select".into(),
                options: vec![
                    AemOption {
                        label: "Option A".into(),
                        value: "a".into(),
                    },
                    AemOption {
                        label: "Option B".into(),
                        value: "b".into(),
                    },
                ],
                mandatory: false,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![ConditionRule {
                    target_panel_name: "COND_PanelA".into(),
                    value: InputValue::Text("a".into()),
                    show: true,
                }],
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains("fd:scripts"),
            "Dropdown with conditions should emit fd:scripts. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("COND_PanelA.visible"),
            "Script content should reference COND_PanelA. Got:\n{}",
            xml
        );
    }

    #[test]
    fn checkbox_with_conditions_emits_scripts() {
        use crate::structured::InputValue;

        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Checkbox {
                uuid: fixed_uuid(),
                name: "CB_Trigger".into(),
                options: vec![AemOption {
                    label: "Accept".into(),
                    value: "true".into(),
                }],
                alignment: OptionAlignment::Horizontal,
                visible: true,
                colspan: 12,
                dor_colspan: None,
                field_id: None,
                conditions: vec![ConditionRule {
                    target_panel_name: "COND_AcceptPanel".into(),
                    value: InputValue::Bool(true),
                    show: true,
                }],
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains("fd:scripts"),
            "Checkbox with conditions should emit fd:scripts. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("COND_AcceptPanel.visible"),
            "Script content should reference COND_AcceptPanel. Got:\n{}",
            xml
        );
    }

    #[test]
    fn value_commit_script_uses_single_quotes() {
        use crate::structured::InputValue;

        let conditions = vec![ConditionRule {
            target_panel_name: "COND_Panel1".into(),
            value: InputValue::Text("option_a".into()),
            show: true,
        }];
        let script = generate_value_commit_script(&conditions);
        assert!(
            script.contains("this.value === 'option_a'"),
            "Script should use single quotes for value comparison. Got: {}",
            script
        );
        assert!(
            script.contains("COND_Panel1.visible = true"),
            "Script should set panel visible. Got: {}",
            script
        );
        assert!(
            script.contains("COND_Panel1.dorExclusion = false"),
            "Script should set dorExclusion to false when visible. Got: {}",
            script
        );
    }

    #[test]
    fn value_commit_json_has_correct_structure() {
        use crate::structured::InputValue;

        let conditions = vec![ConditionRule {
            target_panel_name: "COND_Test".into(),
            value: InputValue::Text("val".into()),
            show: true,
        }];
        let json = generate_value_commit_json("RB_Field", &conditions);
        assert!(
            json.starts_with("[{"),
            "JSON should start with array+object. Got: {}",
            json
        );
        assert!(
            json.contains("\"script\""),
            "JSON should contain \"script\" key. Got: {}",
            json
        );
        assert!(
            json.contains("\"field\":\"RB_Field\""),
            "JSON should contain field name. Got: {}",
            json
        );
        assert!(
            json.contains("\"event\":\"Value Commit\""),
            "JSON should contain Value Commit event. Got: {}",
            json
        );
        assert!(
            json.contains("SCRIPTMODEL"),
            "JSON should contain SCRIPTMODEL. Got: {}",
            json
        );
    }

    #[test]
    fn dor_colspan_emitted_on_fields_in_grid_panel() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::Panel {
                uuid: fixed_uuid(),
                name: "GridPanel".into(),
                title: "Grid Panel".into(),
                is_page: false,
                dor_exclude: false,
                visible: true,
                dor_num_cols: Some(3),
                colspan: 12,
                dor_colspan: None,
                children: vec![
                    AemNode::TextField {
                        uuid: fixed_uuid(),
                        name: "Street".into(),
                        label: "Street".into(),
                        mandatory: false,
                        visible: true,
                        max_chars: None,
                        colspan: 8,
                        dor_colspan: Some(2),
                    },
                    AemNode::TextField {
                        uuid: fixed_uuid(),
                        name: "No".into(),
                        label: "No".into(),
                        mandatory: false,
                        visible: true,
                        max_chars: None,
                        colspan: 4,
                        dor_colspan: Some(1),
                    },
                ],
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            xml.contains("dorNumCols=\"3\""),
            "Panel should have dorNumCols=3. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("dorColspan=\"2\""),
            "Street field should have dorColspan=2. Got:\n{}",
            xml
        );
        assert!(
            xml.contains("dorColspan=\"1\""),
            "No field should have dorColspan=1. Got:\n{}",
            xml
        );
    }

    #[test]
    fn dor_colspan_not_emitted_when_none() {
        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::TextField {
                uuid: fixed_uuid(),
                name: "PlainField".into(),
                label: "Plain".into(),
                mandatory: false,
                visible: true,
                max_chars: None,
                colspan: 12,
                dor_colspan: None,
            }],
        };
        let xml = generate_aem_xml(&root, &test_config());
        assert!(
            !xml.contains("dorColspan"),
            "Field without dor_colspan should not emit dorColspan. Got:\n{}",
            xml
        );
    }

    #[test]
    fn missing_template_omits_component() {
        let mut config = test_config();
        config.component_templates.remove("textdraw");

        let root = AemNode::Root {
            title: "Form".into(),
            children: vec![AemNode::TextDraw {
                uuid: fixed_uuid(),
                name: "ST_1".into(),
                content: "Hello".into(),
                dor_exclude: false,
                colspan: 12,
                dor_colspan: None,
            }],
        };
        let xml = generate_aem_xml(&root, &config);
        assert!(
            !xml.contains("ST_1"),
            "Component with missing template should be omitted. Got:\n{}",
            xml
        );
    }
}
