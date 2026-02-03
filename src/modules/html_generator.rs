//! HTML Form Generator Module
//!
//! Converts a structured NodeTree into a standalone, interactive HTML form
//! with embedded JavaScript for dynamic repeatables and conditionals.

use crate::structured::{
    ConditionalNode, FieldCondition, FieldNode, FieldType, GroupNode, HeadingLevel, HeadingNode,
    ImageNode, InlineNode, InlineText, InputValue, ParagraphNode, RepeatableNode, StructuredNode,
    TableNode,
};

/// Configuration for HTML generation
#[derive(Debug, Clone)]
pub struct HtmlConfig {
    /// Form ID attribute
    pub form_id: String,
    /// Include inline CSS styles
    pub include_styles: bool,
    /// Include JavaScript for dynamic behavior
    pub include_scripts: bool,
}

impl Default for HtmlConfig {
    fn default() -> Self {
        Self {
            form_id: "generated-form".to_string(),
            include_styles: true,
            include_scripts: true,
        }
    }
}

/// Generate a complete HTML document from structured nodes
pub fn generate_html(nodes: &[StructuredNode], config: &HtmlConfig) -> String {
    let mut html = String::new();

    // HTML document header
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("  <meta charset=\"UTF-8\">\n");
    html.push_str("  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n");
    html.push_str("  <title>Generated Form</title>\n");

    if config.include_styles {
        html.push_str(&generate_styles());
    }

    html.push_str("</head>\n<body>\n");

    // Form container
    html.push_str(&format!(
        "  <form id=\"{}\" class=\"generated-form\">\n",
        escape_attr(&config.form_id)
    ));

    // Generate form content
    let mut ctx = GeneratorContext::new();
    for node in nodes {
        html.push_str(&generate_node(node, &mut ctx, 2));
    }

    html.push_str("  </form>\n");

    if config.include_scripts {
        html.push_str(&generate_scripts(&config.form_id));
    }

    html.push_str("</body>\n</html>\n");

    html
}

/// Generate just the form body (without HTML document wrapper)
pub fn generate_form_body(nodes: &[StructuredNode]) -> String {
    let mut html = String::new();
    let mut ctx = GeneratorContext::new();
    for node in nodes {
        html.push_str(&generate_node(node, &mut ctx, 0));
    }
    html
}

/// Internal context for tracking generation state
struct GeneratorContext {
    /// Counter for generating unique IDs
    id_counter: usize,
    /// Current repeatable nesting depth for index tracking
    repeatable_depth: usize,
}

impl GeneratorContext {
    fn new() -> Self {
        Self {
            id_counter: 0,
            repeatable_depth: 0,
        }
    }

    fn next_id(&mut self, prefix: &str) -> String {
        self.id_counter += 1;
        format!("{}_{}", prefix, self.id_counter)
    }
}

/// Generate HTML for a single node
fn generate_node(node: &StructuredNode, ctx: &mut GeneratorContext, indent: usize) -> String {
    let ind = "  ".repeat(indent);

    match node {
        StructuredNode::Heading(h) => generate_heading(h, &ind),
        StructuredNode::Paragraph(p) => generate_paragraph(p, &ind),
        StructuredNode::Image(img) => generate_image(img, ctx, &ind),
        StructuredNode::Table(t) => generate_table(t, ctx, &ind),
        StructuredNode::Field(f) => generate_field(f, ctx, &ind),
        StructuredNode::Repeatable(r) => generate_repeatable(r, ctx, indent),
        StructuredNode::Group(g) => generate_group(g, ctx, indent),
        StructuredNode::Conditional(c) => generate_conditional(c, ctx, indent),
        StructuredNode::Empty => String::new(),
    }
}

fn generate_heading(h: &HeadingNode, ind: &str) -> String {
    let tag = match h.level {
        HeadingLevel::H1 => "h1",
        HeadingLevel::H2 => "h2",
        HeadingLevel::H3 => "h3",
        HeadingLevel::H4 => "h4",
        HeadingLevel::H5 => "h5",
        HeadingLevel::H6 => "h6",
    };
    format!(
        "{}<{}>{}</{}>\n",
        ind,
        tag,
        generate_inline_text(&h.content),
        tag
    )
}

fn generate_paragraph(p: &ParagraphNode, ind: &str) -> String {
    format!("{}<p>{}</p>\n", ind, generate_inline_text(&p.content))
}

fn generate_image(img: &ImageNode, ctx: &mut GeneratorContext, ind: &str) -> String {
    let id = ctx.next_id("img");
    let alt = img
        .alt_text
        .as_ref()
        .map(|s| escape_attr(s))
        .unwrap_or_default();

    // Images are embedded as base64 data URIs
    if !img.content.is_empty() {
        let b64 = base64_encode(&img.content);
        format!(
            "{}<img id=\"{}\" src=\"data:image/png;base64,{}\" alt=\"{}\" class=\"form-image\">\n",
            ind, id, b64, alt
        )
    } else {
        format!(
            "{}<img id=\"{}\" alt=\"{}\" class=\"form-image form-image-placeholder\">\n",
            ind, id, alt
        )
    }
}

fn generate_table(t: &TableNode, ctx: &mut GeneratorContext, ind: &str) -> String {
    let mut html = format!("{}<table class=\"form-table\">\n", ind);

    if let Some(caption) = &t.caption {
        html.push_str(&format!(
            "{}  <caption>{}</caption>\n",
            ind,
            generate_inline_text(caption)
        ));
    }

    if let Some(header) = &t.header {
        html.push_str(&format!("{}  <thead>\n{}    <tr>\n", ind, ind));
        for cell in &header.cells {
            html.push_str(&format!("{}      <th>", ind));
            html.push_str(&generate_node_inline(cell, ctx));
            html.push_str("</th>\n");
        }
        html.push_str(&format!("{}    </tr>\n{}  </thead>\n", ind, ind));
    }

    html.push_str(&format!("{}  <tbody>\n", ind));
    for row in &t.rows {
        html.push_str(&format!("{}    <tr>\n", ind));
        for cell in &row.cells {
            html.push_str(&format!("{}      <td>", ind));
            html.push_str(&generate_node_inline(cell, ctx));
            html.push_str("</td>\n");
        }
        html.push_str(&format!("{}    </tr>\n", ind));
    }
    html.push_str(&format!("{}  </tbody>\n", ind));

    html.push_str(&format!("{}</table>\n", ind));
    html
}

/// Generate node content inline (for table cells)
fn generate_node_inline(node: &StructuredNode, ctx: &mut GeneratorContext) -> String {
    match node {
        StructuredNode::Paragraph(p) => generate_inline_text(&p.content),
        StructuredNode::Field(f) => generate_field_input(f, ctx),
        StructuredNode::Group(g) => {
            let mut html = String::new();
            for child in &g.children {
                html.push_str(&generate_node_inline(child, ctx));
            }
            html
        }
        _ => generate_node(node, ctx, 0).trim().to_string(),
    }
}

fn generate_field(f: &FieldNode, ctx: &mut GeneratorContext, ind: &str) -> String {
    let mut html = format!("{}<div class=\"form-field\">\n", ind);

    // Generate label if present
    if let Some(label) = &f.label {
        let label_text = generate_inline_text(label);
        if !label_text.is_empty() {
            html.push_str(&format!(
                "{}  <label for=\"{}\">{}</label>\n",
                ind,
                escape_attr(&f.name),
                label_text
            ));
        }
    }

    // Generate the input element
    html.push_str(&format!("{}  ", ind));
    html.push_str(&generate_field_input(f, ctx));
    html.push('\n');

    html.push_str(&format!("{}</div>\n", ind));
    html
}

fn generate_field_input(f: &FieldNode, _ctx: &mut GeneratorContext) -> String {
    let name = escape_attr(&f.name);
    let placeholder = f
        .placeholder
        .as_ref()
        .map(|p| format!(" placeholder=\"{}\"", escape_attr(p)))
        .unwrap_or_default();

    match &f.input_type {
        FieldType::Text {
            regex,
            max_length,
            min_length,
        } => {
            let mut attrs = format!(
                "<input type=\"text\" id=\"{}\" name=\"{}\"{}",
                name, name, placeholder
            );
            if let Some(pattern) = regex {
                attrs.push_str(&format!(" pattern=\"{}\"", escape_attr(pattern)));
            }
            if let Some(max) = max_length {
                attrs.push_str(&format!(" maxlength=\"{}\"", max));
            }
            if let Some(min) = min_length {
                attrs.push_str(&format!(" minlength=\"{}\"", min));
            }
            if let Some(InputValue::Text(v)) = &f.value {
                attrs.push_str(&format!(" value=\"{}\"", escape_attr(v)));
            }
            attrs.push_str(" class=\"form-input\">");
            attrs
        }

        FieldType::Number { min, max, step } => {
            let mut attrs = format!(
                "<input type=\"number\" id=\"{}\" name=\"{}\"{}",
                name, name, placeholder
            );
            if let Some(m) = min {
                attrs.push_str(&format!(" min=\"{}\"", m));
            }
            if let Some(m) = max {
                attrs.push_str(&format!(" max=\"{}\"", m));
            }
            if let Some(s) = step {
                attrs.push_str(&format!(" step=\"{}\"", s));
            }
            if let Some(InputValue::Number(v)) = &f.value {
                attrs.push_str(&format!(" value=\"{}\"", v));
            }
            attrs.push_str(" class=\"form-input\">");
            attrs
        }

        FieldType::Date => {
            let mut attrs = format!(
                "<input type=\"date\" id=\"{}\" name=\"{}\"{}",
                name, name, placeholder
            );
            if let Some(InputValue::Date(v)) = &f.value {
                attrs.push_str(&format!(" value=\"{}\"", escape_attr(v)));
            }
            attrs.push_str(" class=\"form-input\">");
            attrs
        }

        FieldType::Email => {
            let mut attrs = format!(
                "<input type=\"email\" id=\"{}\" name=\"{}\"{}",
                name, name, placeholder
            );
            if let Some(InputValue::Email(v)) = &f.value {
                attrs.push_str(&format!(" value=\"{}\"", escape_attr(v)));
            }
            attrs.push_str(" class=\"form-input\">");
            attrs
        }

        FieldType::Tel => {
            let mut attrs = format!(
                "<input type=\"tel\" id=\"{}\" name=\"{}\"{}",
                name, name, placeholder
            );
            if let Some(InputValue::Tel(v)) = &f.value {
                attrs.push_str(&format!(" value=\"{}\"", escape_attr(v)));
            }
            attrs.push_str(" class=\"form-input\">");
            attrs
        }

        FieldType::Checkbox => {
            let checked = matches!(&f.value, Some(InputValue::Checkbox(true)));
            let checked_attr = if checked { " checked" } else { "" };
            format!(
                "<input type=\"checkbox\" id=\"{}\" name=\"{}\" class=\"form-checkbox\"{}>",
                name, name, checked_attr
            )
        }

        FieldType::Radio { options, .. } => {
            let mut html = format!("<div class=\"radio-group\" data-field=\"{}\">\n", name);
            let selected = f.value.as_ref().and_then(|v| {
                if let InputValue::Radio(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            });

            for (i, option) in options.iter().enumerate() {
                let option_id = format!("{}_{}", name, i);
                let checked = selected == Some(option.as_str());
                let checked_attr = if checked { " checked" } else { "" };
                html.push_str(&format!(
                    "  <label class=\"radio-option\">\n    <input type=\"radio\" id=\"{}\" name=\"{}\" value=\"{}\" class=\"form-radio\"{}>\n    <span>{}</span>\n  </label>\n",
                    escape_attr(&option_id),
                    name,
                    escape_attr(option),
                    checked_attr,
                    escape_html(option)
                ));
            }
            html.push_str("</div>");
            html
        }

        FieldType::Select { options } => {
            let mut html = format!(
                "<select id=\"{}\" name=\"{}\" class=\"form-select\">\n",
                name, name
            );
            html.push_str("  <option value=\"\">-- Select --</option>\n");

            let selected = f.value.as_ref().and_then(|v| {
                if let InputValue::Select(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            });

            for option in options {
                let selected_attr = if selected == Some(option.as_str()) {
                    " selected"
                } else {
                    ""
                };
                html.push_str(&format!(
                    "  <option value=\"{}\"{}>{}</option>\n",
                    escape_attr(option),
                    selected_attr,
                    escape_html(option)
                ));
            }
            html.push_str("</select>");
            html
        }
    }
}

fn generate_repeatable(r: &RepeatableNode, ctx: &mut GeneratorContext, indent: usize) -> String {
    let ind = "  ".repeat(indent);
    let container_id = ctx.next_id("repeatable");
    ctx.repeatable_depth += 1;

    let min = r.min_occurrences;
    let max = r.max_occurrences.map(|m| m.to_string()).unwrap_or_default();

    let mut html = format!(
        "{}<div class=\"repeatable-container\" id=\"{}\" data-min=\"{}\" data-max=\"{}\">\n",
        ind, container_id, min, max
    );

    // Template for cloning (hidden)
    html.push_str(&format!(
        "{}  <template class=\"repeatable-template\">\n",
        ind
    ));
    html.push_str(&format!(
        "{}    <div class=\"repeatable-item\" data-index=\"{{INDEX}}\">\n",
        ind
    ));
    html.push_str(&generate_node(&r.item, ctx, indent + 3));
    html.push_str(&format!(
        "{}      <button type=\"button\" class=\"remove-item-btn\" aria-label=\"Remove item\">×</button>\n",
        ind
    ));
    html.push_str(&format!("{}    </div>\n", ind));
    html.push_str(&format!("{}  </template>\n", ind));

    // Items container
    html.push_str(&format!(
        "{}  <div class=\"repeatable-items\"></div>\n",
        ind
    ));

    // Add button
    html.push_str(&format!(
        "{}  <button type=\"button\" class=\"add-item-btn\">+ Add Item</button>\n",
        ind
    ));

    html.push_str(&format!("{}</div>\n", ind));

    ctx.repeatable_depth -= 1;
    html
}

fn generate_group(g: &GroupNode, ctx: &mut GeneratorContext, indent: usize) -> String {
    let ind = "  ".repeat(indent);
    let mut html = format!("{}<div class=\"form-group\">\n", ind);
    for child in &g.children {
        html.push_str(&generate_node(child, ctx, indent + 1));
    }
    html.push_str(&format!("{}</div>\n", ind));
    html
}

fn generate_conditional(c: &ConditionalNode, ctx: &mut GeneratorContext, indent: usize) -> String {
    let ind = "  ".repeat(indent);

    let condition_attr = encode_condition(&c.condition);
    let condition_id = ctx.next_id("conditional");

    let mut html = format!(
        "{}<div class=\"conditional\" id=\"{}\" data-condition=\"{}\" hidden>\n",
        ind, condition_id, condition_attr
    );
    html.push_str(&generate_node(&c.content, ctx, indent + 1));
    html.push_str(&format!("{}</div>\n", ind));

    html
}

/// Encode a condition as a data attribute value
fn encode_condition(cond: &FieldCondition) -> String {
    let value_str = match &cond.value {
        InputValue::Text(s) => format!("text:{}", s),
        InputValue::Number(n) => format!("number:{}", n),
        InputValue::Date(s) => format!("date:{}", s),
        InputValue::Email(s) => format!("email:{}", s),
        InputValue::Tel(s) => format!("tel:{}", s),
        InputValue::Checkbox(b) => format!("checkbox:{}", b),
        InputValue::Radio(s) => format!("radio:{}", s),
        InputValue::Select(s) => format!("select:{}", s),
    };
    escape_attr(&format!("{}={}", cond.field_name, value_str))
}

fn generate_inline_text(text: &InlineText) -> String {
    let mut html = String::new();
    for node in &text.0 {
        html.push_str(&generate_inline_node(node));
    }
    html
}

fn generate_inline_node(node: &InlineNode) -> String {
    match node {
        InlineNode::Text(s) => escape_html(s),
        InlineNode::Link(link) => {
            format!(
                "<a href=\"{}\">{}</a>",
                escape_attr(&link.href),
                generate_inline_text(&link.content)
            )
        }
        InlineNode::Strong(inner) => {
            format!("<strong>{}</strong>", generate_inline_node(inner))
        }
        InlineNode::Emphasis(inner) => {
            format!("<em>{}</em>", generate_inline_node(inner))
        }
    }
}

/// Generate embedded CSS styles
fn generate_styles() -> String {
    r#"  <style>
    :root {
      --primary: #2563eb;
      --primary-hover: #1d4ed8;
      --danger: #dc2626;
      --danger-hover: #b91c1c;
      --border: #d1d5db;
      --bg: #f9fafb;
      --text: #111827;
      --text-muted: #6b7280;
    }

    * {
      box-sizing: border-box;
    }

    body {
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
      line-height: 1.6;
      color: var(--text);
      background: var(--bg);
      padding: 2rem;
      max-width: 800px;
      margin: 0 auto;
    }

    .generated-form {
      background: white;
      padding: 2rem;
      border-radius: 8px;
      box-shadow: 0 1px 3px rgba(0,0,0,0.1);
    }

    h1, h2, h3, h4, h5, h6 {
      margin: 1.5rem 0 0.75rem;
      line-height: 1.3;
    }

    h1 { font-size: 1.875rem; }
    h2 { font-size: 1.5rem; }
    h3 { font-size: 1.25rem; }
    h4 { font-size: 1.125rem; }
    h5 { font-size: 1rem; }
    h6 { font-size: 0.875rem; }

    p {
      margin: 0.5rem 0;
    }

    .form-field {
      margin-bottom: 1rem;
    }

    .form-field label {
      display: block;
      font-weight: 500;
      margin-bottom: 0.25rem;
      color: var(--text);
    }

    .form-input,
    .form-select {
      width: 100%;
      padding: 0.5rem 0.75rem;
      border: 1px solid var(--border);
      border-radius: 4px;
      font-size: 1rem;
      transition: border-color 0.15s, box-shadow 0.15s;
    }

    .form-input:focus,
    .form-select:focus {
      outline: none;
      border-color: var(--primary);
      box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.1);
    }

    .form-checkbox,
    .form-radio {
      width: 1rem;
      height: 1rem;
      margin-right: 0.5rem;
    }

    .radio-group {
      display: flex;
      flex-direction: column;
      gap: 0.5rem;
    }

    .radio-option {
      display: flex;
      align-items: center;
      cursor: pointer;
    }

    .radio-option span {
      user-select: none;
    }

    .form-group {
      margin-bottom: 1.5rem;
      padding: 1rem;
      border: 1px solid var(--border);
      border-radius: 4px;
      background: var(--bg);
    }

    .form-table {
      width: 100%;
      border-collapse: collapse;
      margin: 1rem 0;
    }

    .form-table th,
    .form-table td {
      padding: 0.5rem;
      border: 1px solid var(--border);
      text-align: left;
    }

    .form-table th {
      background: var(--bg);
      font-weight: 600;
    }

    .form-image {
      max-width: 100%;
      height: auto;
      margin: 1rem 0;
    }

    .form-image-placeholder {
      background: var(--bg);
      min-height: 100px;
      display: flex;
      align-items: center;
      justify-content: center;
    }

    /* Repeatable sections */
    .repeatable-container {
      margin: 1rem 0;
      padding: 1rem;
      border: 2px dashed var(--border);
      border-radius: 8px;
    }

    .repeatable-items {
      display: flex;
      flex-direction: column;
      gap: 1rem;
    }

    .repeatable-item {
      position: relative;
      padding: 1rem;
      padding-right: 2.5rem;
      border: 1px solid var(--border);
      border-radius: 4px;
      background: white;
    }

    .remove-item-btn {
      position: absolute;
      top: 0.5rem;
      right: 0.5rem;
      width: 1.5rem;
      height: 1.5rem;
      padding: 0;
      border: none;
      background: var(--danger);
      color: white;
      border-radius: 50%;
      cursor: pointer;
      font-size: 1rem;
      line-height: 1;
      display: flex;
      align-items: center;
      justify-content: center;
    }

    .remove-item-btn:hover {
      background: var(--danger-hover);
    }

    .remove-item-btn:disabled {
      opacity: 0.5;
      cursor: not-allowed;
    }

    .add-item-btn {
      margin-top: 1rem;
      padding: 0.5rem 1rem;
      border: 1px solid var(--primary);
      background: white;
      color: var(--primary);
      border-radius: 4px;
      cursor: pointer;
      font-size: 0.875rem;
      transition: background 0.15s, color 0.15s;
    }

    .add-item-btn:hover {
      background: var(--primary);
      color: white;
    }

    .add-item-btn:disabled {
      opacity: 0.5;
      cursor: not-allowed;
    }

    /* Conditionals */
    .conditional {
      /* Hidden by default via [hidden] attribute */
    }

    .conditional[data-visible="true"] {
      display: block !important;
    }

    /* Nested repeatables */
    .repeatable-container .repeatable-container {
      border-color: var(--primary);
      background: rgba(37, 99, 235, 0.02);
    }

    .repeatable-container .repeatable-container .repeatable-container {
      border-color: #7c3aed;
      background: rgba(124, 58, 237, 0.02);
    }
  </style>
"#
    .to_string()
}

/// Generate embedded JavaScript for dynamic behavior
fn generate_scripts(form_id: &str) -> String {
    format!(
        r#"  <script>
(function() {{
  'use strict';

  const form = document.getElementById('{}');
  if (!form) return;

  // =====================
  // REPEATABLE SECTIONS
  // =====================

  function initRepeatables() {{
    const containers = form.querySelectorAll('.repeatable-container');
    containers.forEach(initRepeatableContainer);
  }}

  function initRepeatableContainer(container) {{
    const template = container.querySelector(':scope > .repeatable-template');
    const itemsContainer = container.querySelector(':scope > .repeatable-items');
    const addBtn = container.querySelector(':scope > .add-item-btn');

    if (!template || !itemsContainer || !addBtn) return;

    const min = parseInt(container.dataset.min) || 0;
    const max = container.dataset.max ? parseInt(container.dataset.max) : Infinity;
    let index = 0;

    function getItemCount() {{
      return itemsContainer.querySelectorAll(':scope > .repeatable-item').length;
    }}

    function updateButtonStates() {{
      const count = getItemCount();
      addBtn.disabled = count >= max;

      // Update remove buttons
      const removeButtons = itemsContainer.querySelectorAll(':scope > .repeatable-item > .remove-item-btn');
      removeButtons.forEach(btn => {{
        btn.disabled = count <= min;
      }});
    }}

    function updateFieldIndices(item, newIndex) {{
      // Update data-index
      item.dataset.index = newIndex;

      // Update name and id attributes with new index
      const fields = item.querySelectorAll('input, select, textarea');
      fields.forEach(field => {{
        if (field.name) {{
          field.name = field.name.replace(/\[\d+\]/, `[${{newIndex}}]`);
        }}
        if (field.id) {{
          field.id = field.id.replace(/_\d+$/, `_${{newIndex}}`);
        }}
      }});

      // Update labels
      const labels = item.querySelectorAll('label');
      labels.forEach(label => {{
        if (label.htmlFor) {{
          label.htmlFor = label.htmlFor.replace(/_\d+$/, `_${{newIndex}}`);
        }}
      }});
    }}

    function reindexItems() {{
      const items = itemsContainer.querySelectorAll(':scope > .repeatable-item');
      items.forEach((item, i) => {{
        updateFieldIndices(item, i);
      }});
      index = items.length;
    }}

    function addItem() {{
      if (getItemCount() >= max) return;

      const content = template.content.cloneNode(true);
      const item = content.querySelector('.repeatable-item');

      // Replace {{{{INDEX}}}} placeholders with actual index
      const html = item.innerHTML.replace(/\{{{{INDEX}}}}/g, index.toString());
      item.innerHTML = html;
      item.dataset.index = index;

      // Update field names/ids to include index
      const fields = item.querySelectorAll('input, select, textarea');
      fields.forEach(field => {{
        if (field.name && !field.name.includes('[')) {{
          field.name = `${{field.name}}[${{index}}]`;
        }}
        if (field.id) {{
          field.id = `${{field.id}}_${{index}}`;
        }}
      }});

      // Update labels
      const labels = item.querySelectorAll('label');
      labels.forEach(label => {{
        if (label.htmlFor) {{
          label.htmlFor = `${{label.htmlFor}}_${{index}}`;
        }}
      }});

      // Add remove handler
      const removeBtn = item.querySelector('.remove-item-btn');
      if (removeBtn) {{
        removeBtn.addEventListener('click', () => removeItem(item));
      }}

      itemsContainer.appendChild(item);
      index++;

      // Initialize nested repeatables
      const nestedContainers = item.querySelectorAll('.repeatable-container');
      nestedContainers.forEach(initRepeatableContainer);

      // Initialize conditionals in the new item
      const nestedConditionals = item.querySelectorAll('.conditional');
      nestedConditionals.forEach(el => evaluateConditional(el));

      updateButtonStates();
      updateConditionals();
    }}

    function removeItem(item) {{
      if (getItemCount() <= min) return;
      item.remove();
      reindexItems();
      updateButtonStates();
      updateConditionals();
    }}

    // Add button handler
    addBtn.addEventListener('click', addItem);

    // Create initial items (minimum required)
    for (let i = 0; i < min; i++) {{
      addItem();
    }}

    updateButtonStates();
  }}

  // =====================
  // CONDITIONAL SECTIONS
  // =====================

  function parseCondition(condStr) {{
    const [fieldPart, valuePart] = condStr.split('=');
    if (!fieldPart || !valuePart) return null;

    const [type, ...rest] = valuePart.split(':');
    const value = rest.join(':');

    return {{
      fieldName: fieldPart,
      type: type,
      value: value
    }};
  }}

  function getFieldValue(fieldName) {{
    // Try to find the field by name
    const field = form.querySelector(`[name="${{fieldName}}"]`);
    if (!field) {{
      // Try radio buttons
      const radios = form.querySelectorAll(`[name="${{fieldName}}"]`);
      if (radios.length > 0) {{
        const checked = Array.from(radios).find(r => r.checked);
        return checked ? {{ type: 'radio', value: checked.value }} : null;
      }}
      return null;
    }}

    if (field.type === 'checkbox') {{
      return {{ type: 'checkbox', value: field.checked.toString() }};
    }}

    if (field.type === 'radio') {{
      const radios = form.querySelectorAll(`[name="${{fieldName}}"]`);
      const checked = Array.from(radios).find(r => r.checked);
      return checked ? {{ type: 'radio', value: checked.value }} : null;
    }}

    if (field.tagName === 'SELECT') {{
      return {{ type: 'select', value: field.value }};
    }}

    return {{ type: 'text', value: field.value }};
  }}

  function conditionMatches(condition, fieldValue) {{
    if (!fieldValue) return false;
    return condition.value === fieldValue.value;
  }}

  function evaluateConditional(el) {{
    const condStr = el.dataset.condition;
    if (!condStr) return;

    const condition = parseCondition(condStr);
    if (!condition) return;

    const fieldValue = getFieldValue(condition.fieldName);
    const matches = conditionMatches(condition, fieldValue);

    if (matches) {{
      el.removeAttribute('hidden');
      el.dataset.visible = 'true';
    }} else {{
      el.setAttribute('hidden', '');
      el.dataset.visible = 'false';
    }}
  }}

  function updateConditionals() {{
    const conditionals = form.querySelectorAll('.conditional');
    conditionals.forEach(evaluateConditional);
  }}

  function initConditionals() {{
    // Initial evaluation
    updateConditionals();

    // Listen for changes on all form fields
    form.addEventListener('change', updateConditionals);
    form.addEventListener('input', updateConditionals);
  }}

  // =====================
  // INITIALIZATION
  // =====================

  initRepeatables();
  initConditionals();

}})();
  </script>
"#,
        escape_attr(form_id)
    )
}

// =====================
// UTILITY FUNCTIONS
// =====================

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_attr(s: &str) -> String {
    escape_html(s)
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_simple_field() {
        let field = FieldNode {
            name: "test_field".to_string(),
            label: Some(InlineText::plain("Test Label")),
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: Some("Enter text".to_string()),
        };

        let node = StructuredNode::Field(field);
        let mut ctx = GeneratorContext::new();
        let html = generate_node(&node, &mut ctx, 0);

        assert!(html.contains("form-field"));
        assert!(html.contains("Test Label"));
        assert!(html.contains("test_field"));
        assert!(html.contains("Enter text"));
    }

    #[test]
    fn test_generate_radio_field() {
        let field = FieldNode {
            name: "choice".to_string(),
            label: Some(InlineText::plain("Choose one")),
            input_type: FieldType::Radio {
                options: vec!["Option A".to_string(), "Option B".to_string()],
                option_names: None,
            },
            value: Some(InputValue::Radio("Option A".to_string())),
            placeholder: None,
        };

        let node = StructuredNode::Field(field);
        let mut ctx = GeneratorContext::new();
        let html = generate_node(&node, &mut ctx, 0);

        assert!(html.contains("radio-group"));
        assert!(html.contains("Option A"));
        assert!(html.contains("Option B"));
        assert!(html.contains("checked"));
    }

    #[test]
    fn test_generate_repeatable() {
        let inner = StructuredNode::Field(FieldNode {
            name: "item".to_string(),
            label: Some(InlineText::plain("Item")),
            input_type: FieldType::Text {
                regex: None,
                max_length: None,
                min_length: None,
            },
            value: None,
            placeholder: None,
        });

        let repeatable = RepeatableNode {
            item: Box::new(inner),
            min_occurrences: 1,
            max_occurrences: Some(5),
        };

        let node = StructuredNode::Repeatable(repeatable);
        let mut ctx = GeneratorContext::new();
        let html = generate_node(&node, &mut ctx, 0);

        assert!(html.contains("repeatable-container"));
        assert!(html.contains("repeatable-template"));
        assert!(html.contains("data-min=\"1\""));
        assert!(html.contains("data-max=\"5\""));
        assert!(html.contains("add-item-btn"));
    }

    #[test]
    fn test_generate_conditional() {
        let content = StructuredNode::Paragraph(ParagraphNode {
            content: InlineText::plain("Conditional content"),
        });

        let conditional = ConditionalNode {
            condition: FieldCondition {
                field_name: "toggle".to_string(),
                value: InputValue::Checkbox(true),
            },
            content: Box::new(content),
        };

        let node = StructuredNode::Conditional(conditional);
        let mut ctx = GeneratorContext::new();
        let html = generate_node(&node, &mut ctx, 0);

        assert!(html.contains("conditional"));
        assert!(html.contains("data-condition"));
        assert!(html.contains("hidden"));
        assert!(html.contains("Conditional content"));
    }
}
