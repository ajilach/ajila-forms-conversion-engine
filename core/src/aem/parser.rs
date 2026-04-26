//! AEM ZIP Package Parser
//!
//! Parses AEM Adaptive Forms content packages (ZIP files) into the `AemNode`
//! tree representation. Handles:
//! - ZIP extraction and form content discovery
//! - JCR XML parsing (`.content.xml` files)
//! - Fragment resolution (inlining `fragRef` content from the ZIP)
//! - Translation extraction (Sling i18n dictionaries)
//! - Script extraction (`fd:scripts` and `fd:rules` JSON attributes)

use std::collections::HashMap;
use std::io::Read;

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use uuid::Uuid;

use super::{AemNode, AemOption, OptionAlignment};

// ============================================================================
// Public types
// ============================================================================

/// A parsed AEM script event extracted from `fd:scripts` or `fd:rules` JSON.
#[derive(Debug, Clone)]
pub struct AemScript {
    /// The event type (e.g. "Initialize", "Value Commit", "Click", "Calculate").
    pub event: String,
    /// The JavaScript source code.
    pub content: String,
    /// The field path this script is associated with.
    pub field: String,
    /// Whether the script is enabled.
    pub enabled: bool,
}

/// Translation data extracted from Sling i18n dictionaries.
///
/// Maps `sling:key` → `HashMap<language, sling:message>`.
#[derive(Debug, Clone, Default)]
pub struct TranslationData {
    /// Maps dictionary key → (language → translated message).
    pub entries: HashMap<String, HashMap<String, String>>,
}

/// A parsed visibility condition extracted from `fd:rules` `fd:visible` JSON.
///
/// Maps a target panel to its trigger field and the value that makes it visible.
#[derive(Debug, Clone)]
pub struct VisibilityCondition {
    /// The trigger field name (e.g. `"RB_GroupTipo"`).
    pub trigger_field: String,
    /// The value that makes the target panel visible.
    pub trigger_value: String,
}

/// Result of parsing an AEM ZIP package.
#[derive(Debug, Clone)]
pub struct ParsedAemPackage {
    /// The parsed AEM node tree.
    pub root: AemNode,
    /// Translation data from Sling dictionaries.
    pub translations: TranslationData,
    /// All scripts extracted from the form, keyed by component name.
    pub scripts: HashMap<String, Vec<AemScript>>,
    /// The detected master language.
    pub language: String,
    /// Detected form title.
    pub form_title: String,
    /// Visibility conditions: panel name → condition that shows it.
    pub visibility_conditions: HashMap<String, VisibilityCondition>,
}

// ============================================================================
// AEM ZIP detection
// ============================================================================

/// Check if the given bytes represent an AEM content package ZIP.
///
/// Returns `true` if the ZIP contains both `META-INF/vault/` (or similar
/// vault metadata) and `jcr_root/content/forms/af/` entries.
pub fn detect_aem_zip(bytes: &[u8]) -> bool {
    let cursor = std::io::Cursor::new(bytes);
    let Ok(mut archive) = zip::ZipArchive::new(cursor) else {
        return false;
    };

    let mut has_meta_inf = false;
    let mut has_forms_af = false;

    for i in 0..archive.len() {
        let Ok(file) = archive.by_index(i) else {
            continue;
        };
        let name = file.name().to_string();
        if name.starts_with("META-INF/") {
            has_meta_inf = true;
        }
        if name.contains("jcr_root/content/forms/af/")
            || name.contains("jcr_root/content/dam/formsanddocuments/")
        {
            has_forms_af = true;
        }
        if has_meta_inf && has_forms_af {
            return true;
        }
    }

    false
}

// ============================================================================
// ZIP parsing
// ============================================================================

/// Parse an AEM ZIP package into a `ParsedAemPackage`.
pub fn parse_aem_zip(bytes: &[u8]) -> Result<ParsedAemPackage, String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("Invalid ZIP: {e}"))?;

    // Collect all file contents from the ZIP into memory for random access
    let mut zip_files: HashMap<String, Vec<u8>> = HashMap::new();
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("ZIP entry error: {e}"))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)
            .map_err(|e| format!("Read error for {name}: {e}"))?;
        zip_files.insert(name, contents);
    }

    // Find the main form .content.xml
    let form_xml_path = find_form_content_xml(&zip_files)?;
    let form_xml = zip_files
        .get(&form_xml_path)
        .ok_or_else(|| format!("Form XML not found at {form_xml_path}"))?;

    // Parse the main form XML
    let xml_str = String::from_utf8_lossy(form_xml);
    let mut parse_ctx = ParseContext::new(&zip_files);
    let (root, form_title, language) = parse_form_xml(&xml_str, &mut parse_ctx)?;

    // Extract translations from dictionary files in the ZIP
    let mut translations = extract_translations(&zip_files);

    // Merge translations collected from resolved fragments
    for (key, lang_map) in parse_ctx.translations.entries {
        translations
            .entries
            .entry(key)
            .or_default()
            .extend(lang_map);
    }

    // Resolve visibility condition trigger values to option labels where possible.
    // E.g. if RB_GroupTipo has options [("1","Individual"),("2","Legal Entity")],
    // a condition with trigger_value="2" becomes trigger_value="Legal Entity".
    for cond in parse_ctx.visibility_conditions.values_mut() {
        if let Some(labels) = parse_ctx.option_labels.get(&cond.trigger_field) {
            if let Some(label) = labels.get(&cond.trigger_value) {
                cond.trigger_value = label.clone();
            }
        }
    }

    Ok(ParsedAemPackage {
        root,
        translations,
        scripts: parse_ctx.scripts,
        language,
        form_title,
        visibility_conditions: parse_ctx.visibility_conditions,
    })
}

/// Find the main form `.content.xml` inside the ZIP.
///
/// Looks for `.content.xml` files under `jcr_root/content/forms/af/` that
/// contain `guideContainer` (the root of an Adaptive Form).
fn find_form_content_xml(zip_files: &HashMap<String, Vec<u8>>) -> Result<String, String> {
    let mut candidates: Vec<String> = zip_files
        .keys()
        .filter(|path| {
            path.contains("jcr_root/content/forms/af/")
                && path.ends_with("/.content.xml")
                && !path.contains("/_jcr_content/")
                && !path.contains("/assets/")
                && !path.contains("/dictionary/")
        })
        .cloned()
        .collect();

    // Sort by depth (prefer shallower paths) then alphabetically
    candidates.sort_by(|a, b| {
        let depth_a = a.matches('/').count();
        let depth_b = b.matches('/').count();
        depth_a.cmp(&depth_b).then(a.cmp(b))
    });

    // Check each candidate for guideContainer content
    for path in &candidates {
        if let Some(content) = zip_files.get(path) {
            let xml_str = String::from_utf8_lossy(content);
            if xml_str.contains("guideContainer") || xml_str.contains("fd/af/components") {
                return Ok(path.clone());
            }
        }
    }

    // Fallback: also check .content-finished.xml files
    let mut finished_candidates: Vec<String> = zip_files
        .keys()
        .filter(|path| {
            path.contains("jcr_root/content/forms/af/")
                && path.ends_with("/.content-finished.xml")
        })
        .cloned()
        .collect();
    finished_candidates.sort_by(|a, b| {
        let depth_a = a.matches('/').count();
        let depth_b = b.matches('/').count();
        depth_a.cmp(&depth_b).then(a.cmp(b))
    });

    for path in &finished_candidates {
        if let Some(content) = zip_files.get(path) {
            let xml_str = String::from_utf8_lossy(content);
            if xml_str.contains("guideContainer") || xml_str.contains("fd/af/components") {
                return Ok(path.clone());
            }
        }
    }

    Err("No Adaptive Form .content.xml found in ZIP".into())
}

// ============================================================================
// XML parsing context
// ============================================================================

/// Context carried through the XML parsing process.
struct ParseContext<'a> {
    /// All files in the ZIP for fragment resolution.
    zip_files: &'a HashMap<String, Vec<u8>>,
    /// Counter for generating deterministic UUIDs.
    counter: u32,
    /// Collected scripts keyed by component name.
    scripts: HashMap<String, Vec<AemScript>>,
    /// Stack of fragment paths being resolved (cycle detection).
    fragment_stack: Vec<String>,
    /// Translation data accumulated from ZIP and resolved fragments.
    translations: TranslationData,
    /// Visibility conditions parsed from fd:visible rules: panel name → condition.
    visibility_conditions: HashMap<String, VisibilityCondition>,
    /// Option value→label mappings for radio buttons and dropdowns: field name → (value → label).
    option_labels: HashMap<String, HashMap<String, String>>,
    /// Fragment library paths whose dictionaries have already been loaded.
    loaded_library_dicts: std::collections::HashSet<String>,
}

impl<'a> ParseContext<'a> {
    fn new(zip_files: &'a HashMap<String, Vec<u8>>) -> Self {
        Self {
            zip_files,
            counter: 0,
            scripts: HashMap::new(),
            fragment_stack: Vec::new(),
            translations: TranslationData::default(),
            visibility_conditions: HashMap::new(),
            option_labels: HashMap::new(),
            loaded_library_dicts: std::collections::HashSet::new(),
        }
    }

    fn next_uuid(&mut self, seed: &str) -> Uuid {
        self.counter += 1;
        let input = format!("{seed}_{}", self.counter);
        Uuid::new_v5(
            &Uuid::from_bytes([
                0x6b, 0xa7, 0xb8, 0x10, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f,
                0xd4, 0x30, 0xc8,
            ]),
            input.as_bytes(),
        )
    }
}

// ============================================================================
// JCR XML parsing
// ============================================================================

/// Parse the main form XML into an AemNode tree.
fn parse_form_xml(
    xml: &str,
    ctx: &mut ParseContext,
) -> Result<(AemNode, String, String), String> {
    // Use a SAX-like approach: build a tree of JCR nodes first, then convert to AemNode.
    let jcr_tree = parse_jcr_xml(xml)?;

    // Find the guideContainer node
    let guide_container = find_node_by_resource_type(&jcr_tree, "fd/af/components/guideContainer")
        .ok_or("No guideContainer found in form XML")?;

    // Extract form title from jcr:content
    let form_title = jcr_tree
        .attributes
        .iter()
        .find(|(k, _)| k == "jcr:title")
        .map(|(_, v)| v.clone())
        .or_else(|| {
            jcr_tree.children.iter().find_map(|child| {
                if child.tag_name == "jcr:content" {
                    child
                        .attributes
                        .iter()
                        .find(|(k, _)| k == "jcr:title")
                        .map(|(_, v)| v.clone())
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| "Untitled".into());

    // Extract language
    let language = jcr_tree
        .children
        .iter()
        .find_map(|child| {
            if child.tag_name == "jcr:content" {
                child
                    .attributes
                    .iter()
                    .find(|(k, _)| k == "jcr:language")
                    .map(|(_, v)| v.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "en".into());

    // Find rootPanel inside guideContainer
    let root_panel = guide_container
        .children
        .iter()
        .find(|c| c.tag_name == "rootPanel")
        .ok_or("No rootPanel found in guideContainer")?;

    // Convert rootPanel to AemNode tree
    let children = convert_items_to_aem_nodes(root_panel, ctx)?;

    let root = AemNode::Root {
        title: form_title.clone(),
        children,
    };

    Ok((root, form_title, language))
}

// ============================================================================
// JCR XML tree
// ============================================================================

/// A parsed JCR XML node.
#[derive(Debug, Clone)]
struct JcrNode {
    tag_name: String,
    attributes: Vec<(String, String)>,
    children: Vec<JcrNode>,
}

impl JcrNode {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn resource_type(&self) -> Option<&str> {
        self.attr("sling:resourceType")
    }

    /// Get the `name` attribute (AEM component name used in scripts).
    fn component_name(&self) -> Option<&str> {
        self.attr("name")
    }
}

/// Parse JCR XML into a tree of JcrNode.
fn parse_jcr_xml(xml: &str) -> Result<JcrNode, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut stack: Vec<JcrNode> = Vec::new();
    let mut root: Option<JcrNode> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let mut attributes = Vec::new();
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                    let value = attr.unescape_value().unwrap_or_default().to_string();
                    attributes.push((key, value));
                }
                stack.push(JcrNode {
                    tag_name,
                    attributes,
                    children: Vec::new(),
                });
            }
            Ok(Event::Empty(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let mut attributes = Vec::new();
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                    let value = attr.unescape_value().unwrap_or_default().to_string();
                    attributes.push((key, value));
                }
                let node = JcrNode {
                    tag_name,
                    attributes,
                    children: Vec::new(),
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    root = Some(node);
                }
            }
            Ok(Event::End(_)) => {
                let node = stack.pop().ok_or("Mismatched XML end tag")?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    root = Some(node);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {} // skip text, comments, etc.
            Err(e) => return Err(format!("XML parse error: {e}")),
        }
    }

    root.ok_or_else(|| "Empty XML document".into())
}

/// Find a node by its `sling:resourceType` (searches recursively).
fn find_node_by_resource_type<'a>(node: &'a JcrNode, resource_type: &str) -> Option<&'a JcrNode> {
    if node.resource_type() == Some(resource_type) {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_node_by_resource_type(child, resource_type) {
            return Some(found);
        }
    }
    None
}

// ============================================================================
// JcrNode → AemNode conversion
// ============================================================================

/// Convert the `items` children of a panel/rootPanel JcrNode into AemNode children.
fn convert_items_to_aem_nodes(
    panel: &JcrNode,
    ctx: &mut ParseContext,
) -> Result<Vec<AemNode>, String> {
    let mut children = Vec::new();

    // Find the `items` child node
    let items_node = panel.children.iter().find(|c| c.tag_name == "items");

    let items = match items_node {
        Some(items) => &items.children,
        None => &panel.children,
    };

    for child in items {
        // Skip non-component nodes (layout, fd:rules, fd:scripts, etc.)
        if child.tag_name == "layout"
            || child.tag_name.starts_with("fd:")
            || child.tag_name == "toolbar"
            || child.tag_name == "items"
        {
            continue;
        }

        if let Some(node) = convert_jcr_to_aem(child, ctx)? {
            children.push(node);
        }
    }

    // Also extract scripts from the panel's fd:scripts child
    if let Some(comp_name) = panel.component_name() {
        extract_scripts_from_node(panel, comp_name, ctx);
    }

    Ok(children)
}

/// Convert a single JcrNode to an AemNode based on its `sling:resourceType`.
fn convert_jcr_to_aem(
    node: &JcrNode,
    ctx: &mut ParseContext,
) -> Result<Option<AemNode>, String> {
    let resource_type = node.resource_type().unwrap_or("");
    let comp_name = node.component_name().unwrap_or(&node.tag_name);

    // Extract scripts from this node
    extract_scripts_from_node(node, comp_name, ctx);

    // Determine component type from resourceType suffix
    let type_suffix = resource_type.rsplit('/').next().unwrap_or(resource_type);

    match type_suffix {
        "textbox" | "guideTextBox" => Ok(Some(convert_textbox(node, ctx))),
        "numericbox" | "guideNumericBox" => Ok(Some(convert_numberfield(node, ctx))),
        "datepicker" | "guideDatePicker" => Ok(Some(convert_datepicker(node, ctx))),
        "radiobutton" | "guideRadioButton" => Ok(Some(convert_radiobutton(node, ctx))),
        "checkbox" | "guideCheckBox" => Ok(Some(convert_checkbox(node, ctx))),
        "dropdownlist" | "guideDropDownList" => Ok(Some(convert_dropdown(node, ctx))),
        "textdraw" | "guideTextDraw" | "messagebox" => Ok(Some(convert_textdraw(node, ctx))),
        "titledraw" | "guideTitleDraw" => Ok(Some(convert_titledraw(node, ctx))),
        "panel" | "guidePanel" | "rootPanel" => convert_panel(node, ctx),
        // Fragment reference
        _ if node.attr("fragRef").is_some() => convert_fragment(node, ctx),
        // Buttons and other controls we skip
        "button" | "guideButton" | "tertiarybutton" | "secondarybutton" | "primarybutton" => {
            Ok(None)
        }
        // Signature, file upload, etc. — treat as textbox
        "signature" | "scribble" | "fileupload" => Ok(Some(convert_textbox(node, ctx))),
        // formtitle, guideheader — skip
        "formtitle" | "guideheader" | "aftemplatedpage" => Ok(None),
        // Unknown — try as panel if it has items children, otherwise skip
        _ => {
            if node.children.iter().any(|c| c.tag_name == "items") {
                convert_panel(node, ctx)
            } else {
                log::debug!(
                    "Skipping unknown AEM component type: {} (tag: {})",
                    resource_type,
                    node.tag_name
                );
                Ok(None)
            }
        }
    }
}

// ============================================================================
// Component converters
// ============================================================================

fn convert_textbox(node: &JcrNode, ctx: &mut ParseContext) -> AemNode {
    let name = node.component_name().unwrap_or("textbox").to_string();
    let label = node.attr("jcr:title").unwrap_or("").to_string();
    let uuid = ctx.next_uuid(&name);
    let visible = parse_visible(node);
    let mandatory = parse_bool_attr(node, "mandatory");
    let max_chars = node
        .attr("maxChars")
        .and_then(|v| v.parse::<usize>().ok());

    AemNode::TextField {
        uuid,
        name,
        label,
        mandatory,
        visible,
        max_chars,
        colspan: 12,
        dor_colspan: None,
        bind_ref: node.attr("bindRef").map(|s| s.to_string()),
    }
}

fn convert_numberfield(node: &JcrNode, ctx: &mut ParseContext) -> AemNode {
    let name = node.component_name().unwrap_or("numericbox").to_string();
    let label = node.attr("jcr:title").unwrap_or("").to_string();
    let uuid = ctx.next_uuid(&name);

    AemNode::NumberField {
        uuid,
        name,
        label,
        mandatory: parse_bool_attr(node, "mandatory"),
        visible: parse_visible(node),
        colspan: 12,
        dor_colspan: None,
        bind_ref: node.attr("bindRef").map(|s| s.to_string()),
    }
}

fn convert_datepicker(node: &JcrNode, ctx: &mut ParseContext) -> AemNode {
    let name = node.component_name().unwrap_or("datepicker").to_string();
    let label = node.attr("jcr:title").unwrap_or("").to_string();
    let uuid = ctx.next_uuid(&name);

    AemNode::DatePicker {
        uuid,
        name,
        label,
        mandatory: parse_bool_attr(node, "mandatory"),
        visible: parse_visible(node),
        colspan: 12,
        dor_colspan: None,
        bind_ref: node.attr("bindRef").map(|s| s.to_string()),
    }
}

fn convert_radiobutton(node: &JcrNode, ctx: &mut ParseContext) -> AemNode {
    let name = node.component_name().unwrap_or("radiobutton").to_string();
    let label = node.attr("jcr:title").unwrap_or("").to_string();
    let uuid = ctx.next_uuid(&name);
    let options = parse_options(node);
    let alignment = if node.attr("optionLayout") == Some("horizontal") {
        OptionAlignment::Horizontal
    } else {
        OptionAlignment::Vertical
    };

    // Store option value→label mapping for visibility condition resolution
    let label_map: HashMap<String, String> = options
        .iter()
        .map(|o| (o.value.clone(), o.label.clone()))
        .collect();
    if !label_map.is_empty() {
        ctx.option_labels.insert(name.clone(), label_map);
    }

    AemNode::RadioButton {
        uuid,
        name,
        label,
        options,
        alignment,
        mandatory: parse_bool_attr(node, "mandatory"),
        visible: parse_visible(node),
        colspan: 12,
        dor_colspan: None,
        field_id: None,
        conditions: Vec::new(),
        bind_ref: node.attr("bindRef").map(|s| s.to_string()),
    }
}

fn convert_checkbox(node: &JcrNode, ctx: &mut ParseContext) -> AemNode {
    let name = node.component_name().unwrap_or("checkbox").to_string();
    let uuid = ctx.next_uuid(&name);
    let options = parse_options(node);
    let alignment = if node.attr("optionLayout") == Some("horizontal") {
        OptionAlignment::Horizontal
    } else {
        OptionAlignment::Vertical
    };

    AemNode::Checkbox {
        uuid,
        name,
        options,
        alignment,
        visible: parse_visible(node),
        colspan: 12,
        dor_colspan: None,
        field_id: None,
        conditions: Vec::new(),
        bind_ref: node.attr("bindRef").map(|s| s.to_string()),
    }
}

fn convert_dropdown(node: &JcrNode, ctx: &mut ParseContext) -> AemNode {
    let name = node.component_name().unwrap_or("dropdownlist").to_string();
    let label = node.attr("jcr:title").unwrap_or("").to_string();
    let uuid = ctx.next_uuid(&name);
    let options = parse_options(node);

    // Store option value→label mapping for visibility condition resolution
    let label_map: HashMap<String, String> = options
        .iter()
        .map(|o| (o.value.clone(), o.label.clone()))
        .collect();
    if !label_map.is_empty() {
        ctx.option_labels.insert(name.clone(), label_map);
    }

    AemNode::Dropdown {
        uuid,
        name,
        label,
        options,
        mandatory: parse_bool_attr(node, "mandatory"),
        visible: parse_visible(node),
        colspan: 12,
        dor_colspan: None,
        field_id: None,
        conditions: Vec::new(),
        bind_ref: node.attr("bindRef").map(|s| s.to_string()),
    }
}

fn convert_textdraw(node: &JcrNode, ctx: &mut ParseContext) -> AemNode {
    let name = node.component_name().unwrap_or("textdraw").to_string();
    let uuid = ctx.next_uuid(&name);
    // For textdraw, content is in _value attribute
    // For messagebox, content is in messageboxBody attribute
    let content = node
        .attr("_value")
        .or_else(|| node.attr("messageboxBody"))
        .unwrap_or("")
        .to_string();

    AemNode::TextDraw {
        uuid,
        name,
        content,
        dor_exclude: parse_bool_attr(node, "dorExclusion"),
        colspan: 12,
        dor_colspan: None,
    }
}

fn convert_titledraw(node: &JcrNode, ctx: &mut ParseContext) -> AemNode {
    let name = node.component_name().unwrap_or("titledraw").to_string();
    let uuid = ctx.next_uuid(&name);
    let content = node.attr("_value").unwrap_or("").to_string();
    let heading_level = node
        .attr("headingLevel")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);

    AemNode::TitleDraw {
        uuid,
        name,
        content,
        heading_level,
        colspan: 12,
        dor_colspan: None,
    }
}

fn convert_panel(
    node: &JcrNode,
    ctx: &mut ParseContext,
) -> Result<Option<AemNode>, String> {
    let name = node.component_name().unwrap_or(&node.tag_name).to_string();
    let title = node.attr("jcr:title").unwrap_or("").to_string();
    let uuid = ctx.next_uuid(&name);
    let visible = parse_visible(node);
    let dor_exclude = parse_bool_attr(node, "dorExclusion")
        || parse_bool_attr(node, "dorExcludeTitle");

    // Check for repeatable
    let min_occur = node
        .attr("minOccur")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let max_occur = node
        .attr("maxOccur")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let is_repeatable = max_occur > 1;

    // Check for page/wizard step (panels with validateOnStepCompletion or
    // that are direct children of a wizard-layout rootPanel)
    let is_page = false; // Determined by parent context, not self attributes

    // Check for fragment reference
    if node.attr("fragRef").is_some() {
        let fragment_result = convert_fragment(node, ctx)?;
        if is_repeatable {
            if let Some(inner) = fragment_result {
                let children = match inner {
                    AemNode::Panel { children, .. } => children,
                    other => vec![other],
                };
                return Ok(Some(AemNode::Repeatable {
                    uuid,
                    name,
                    title,
                    children,
                    min_occur: min_occur.max(1),
                    max_occur,
                }));
            }
        }
        return Ok(fragment_result);
    }

    // Parse children
    let children = convert_items_to_aem_nodes(node, ctx)?;

    // Check for grid layout
    let dor_num_cols = node
        .children
        .iter()
        .find(|c| c.tag_name == "layout")
        .and_then(|layout| layout.attr("columns"))
        .and_then(|v| v.parse().ok());

    if is_repeatable {
        Ok(Some(AemNode::Repeatable {
            uuid,
            name,
            title,
            children,
            min_occur: min_occur.max(1),
            max_occur,
        }))
    } else {
        Ok(Some(AemNode::Panel {
            uuid,
            name,
            title,
            children,
            is_page,
            dor_exclude,
            visible,
            is_conditional: !visible, // Initially hidden panels may be conditional
            dor_num_cols,
            colspan: 12,
            dor_colspan: None,
            bind_ref: node.attr("bindRef").map(|s| s.to_string()),
        }))
    }
}

fn convert_fragment(
    node: &JcrNode,
    ctx: &mut ParseContext,
) -> Result<Option<AemNode>, String> {
    let frag_ref = node.attr("fragRef").unwrap_or("").to_string();
    let name = node.component_name().unwrap_or(&node.tag_name).to_string();
    let uuid = ctx.next_uuid(&name);

    // Cycle detection
    if ctx.fragment_stack.contains(&frag_ref) {
        log::warn!(
            "Fragment cycle detected: {} → {}",
            ctx.fragment_stack.join(" → "),
            frag_ref
        );
        return Ok(None);
    }

    // Try to resolve the fragment from within the ZIP
    let fragment_xml_path = resolve_fragment_path(&frag_ref, ctx.zip_files);

    // Helper closure to parse fragment XML and produce a Panel node
    let parse_fragment_xml = |xml_str: &str, ctx: &mut ParseContext| -> Result<Option<AemNode>, String> {
        ctx.fragment_stack.push(frag_ref.clone());

        let fragment_tree = parse_jcr_xml(xml_str)?;
        let content_node = find_fragment_content(&fragment_tree);

        let children = if let Some(content) = content_node {
            convert_items_to_aem_nodes(content, ctx)?
        } else {
            Vec::new()
        };

        ctx.fragment_stack.pop();

        let title = node.attr("jcr:title").unwrap_or("").to_string();
        Ok(Some(AemNode::Panel {
            uuid,
            name: name.clone(),
            title,
            children,
            is_page: false,
            dor_exclude: false,
            visible: parse_visible(node),
            is_conditional: false,
            dor_num_cols: None,
            colspan: 12,
            dor_colspan: None,
            bind_ref: node.attr("bindRef").map(|s| s.to_string()),
        }))
    };

    // First: try resolving from the ZIP
    if let Some(xml_path) = fragment_xml_path {
        if let Some(xml_bytes) = ctx.zip_files.get(&xml_path) {
            let xml_str = String::from_utf8_lossy(xml_bytes);
            return parse_fragment_xml(&xml_str, ctx);
        }
    }

    // Second: try resolving from embedded profiles (on-disk fragments)
    if let Some(xml_str) = crate::profiles::resolve_embedded_fragment_xml(&frag_ref) {
        // Load dictionaries from the specific fragment
        for (lang, dict_xml) in crate::profiles::resolve_embedded_fragment_dictionaries(&frag_ref)
        {
            parse_sling_dictionary(&dict_xml, &lang, &mut ctx.translations);
        }

        // Also load dictionaries from all sibling fragments in the same library.
        // In AEM, Sling dictionaries are shared across fragments in the same content subtree.
        let library = frag_ref
            .strip_prefix("/content/dam/formsanddocuments/")
            .or_else(|| frag_ref.strip_prefix("/content/forms/af/"))
            .and_then(|r| r.split('/').next())
            .unwrap_or("")
            .to_string();
        if !library.is_empty() && ctx.loaded_library_dicts.insert(library) {
            for (lang, dict_xml) in
                crate::profiles::resolve_embedded_library_dictionaries(&frag_ref)
            {
                parse_sling_dictionary(&dict_xml, &lang, &mut ctx.translations);
            }
        }

        return parse_fragment_xml(&xml_str, ctx);
    }

    // Fragment not found — keep as opaque fragment reference
    log::warn!("Fragment not found: {frag_ref}");
    Ok(Some(AemNode::Fragment {
        uuid,
        name,
        frag_ref,
        bind_ref: node.attr("bindRef").map(|s| s.to_string()),
    }))
}

/// Try to resolve a fragRef path to a ZIP file path.
///
/// `fragRef` looks like `/content/dam/formsanddocuments/path/to/fragment` or
/// `/content/forms/af/path/to/fragment`. We need to find the corresponding
/// `.content.xml` in the ZIP.
fn resolve_fragment_path(
    frag_ref: &str,
    zip_files: &HashMap<String, Vec<u8>>,
) -> Option<String> {
    // Try direct mapping: fragRef → jcr_root/{fragRef}/.content.xml
    let clean_ref = frag_ref.trim_start_matches('/');
    let direct_path = format!("jcr_root/{clean_ref}/.content.xml");
    if zip_files.contains_key(&direct_path) {
        return Some(direct_path);
    }

    // Try with guideContainer path
    let guide_path = format!("jcr_root/{clean_ref}/jcr:content/guideContainer/.content.xml");
    if zip_files.contains_key(&guide_path) {
        return Some(guide_path);
    }

    // Search for any .content.xml that contains the fragment's last path segment
    let fragment_name = frag_ref.rsplit('/').next().unwrap_or(frag_ref);
    for path in zip_files.keys() {
        if path.contains(fragment_name) && path.ends_with("/.content.xml") {
            return Some(path.clone());
        }
    }

    None
}

/// Find the content node inside a fragment XML (typically guideContainer > rootPanel
/// or a top-level panel).
fn find_fragment_content(tree: &JcrNode) -> Option<&JcrNode> {
    // First try: guideContainer > rootPanel
    if let Some(gc) = find_node_by_resource_type(tree, "fd/af/components/guideContainer") {
        if let Some(rp) = gc.children.iter().find(|c| c.tag_name == "rootPanel") {
            return Some(rp);
        }
        return Some(gc);
    }

    // Second try: rootPanel directly
    if let Some(rp) = find_node_by_resource_type(tree, "fd/af/components/rootPanel") {
        return Some(rp);
    }

    // Third try: any panel with items
    if tree.children.iter().any(|c| c.tag_name == "items") {
        return Some(tree);
    }

    // Recurse into jcr:content if present
    for child in &tree.children {
        if child.tag_name == "jcr:content" {
            return find_fragment_content(child);
        }
    }

    None
}

// ============================================================================
// Script extraction
// ============================================================================

/// Extract scripts from `fd:scripts` and `fd:rules` child nodes.
fn extract_scripts_from_node(node: &JcrNode, comp_name: &str, ctx: &mut ParseContext) {
    for child in &node.children {
        if child.tag_name == "fd:scripts" {
            // fd:scripts has attributes like fd:init, fd:click, fd:valueCommit, etc.
            // Each is a JSON array of script models.
            for (attr_name, attr_value) in &child.attributes {
                if attr_name.starts_with("fd:") && attr_name != "fd:translationIds" {
                    let event_name = match attr_name.as_str() {
                        "fd:init" => "Initialize",
                        "fd:click" => "Click",
                        "fd:valueCommit" => "Value Commit",
                        "fd:calc" => "Calculate",
                        "fd:visible" => "Visibility",
                        "fd:navigationChange" => "Navigation Change",
                        "fd:validate" => "Validate",
                        _ => attr_name.trim_start_matches("fd:"),
                    };

                    if let Some(scripts) = parse_fd_scripts_json(attr_value, event_name) {
                        ctx.scripts
                            .entry(comp_name.to_string())
                            .or_default()
                            .extend(scripts);
                    }
                }
            }
        }

        if child.tag_name == "fd:rules" {
            // fd:rules can also contain script content in various attributes
            for (attr_name, attr_value) in &child.attributes {
                if attr_name.starts_with("fd:") {
                    if let Some(scripts) = parse_fd_scripts_json(attr_value, "Rule") {
                        ctx.scripts
                            .entry(comp_name.to_string())
                            .or_default()
                            .extend(scripts);
                    }

                    // Parse structured visibility rules from fd:visible
                    if attr_name == "fd:visible" {
                        parse_visibility_rules(attr_value, ctx);
                    }
                }
            }
        }
    }
}

/// Parse visibility rules from a `fd:visible` attribute on `fd:rules`.
///
/// The raw value (after XML unescaping) contains JCR-serialized JSON with
/// structured rule trees. Deeply nested script content makes full JSON
/// parsing unreliable, so we use regex to extract the structured
/// SHOW_EXPRESSION components.
fn parse_visibility_rules(raw: &str, ctx: &mut ParseContext) {
    use regex_lite::Regex;

    // JCR unescaping: \, → , (commas are escaped in JCR).
    // Quotes are already unescaped by quick-xml.
    let cleaned = raw.replace("\\,", ",");

    // Only process rules that contain a structured SHOW_EXPRESSION
    if !cleaned.contains("SHOW_EXPRESSION") {
        return;
    }

    // Extract the target panel name from AFCOMPONENT
    let re_afcomp = Regex::new(r#""AFCOMPONENT".*?"name":"([^"]+)""#).unwrap();
    // Extract the trigger field name from COMPONENT
    let re_comp = Regex::new(r#""COMPONENT".*?"name":"([^"]+)""#).unwrap();
    // Extract the trigger value from STRING_LITERAL
    let re_strlit = Regex::new(r#""STRING_LITERAL".*?"value":"([^"]+)""#).unwrap();

    if let (Some(target), Some(trigger), Some(value)) = (
        re_afcomp.captures(&cleaned),
        re_comp.captures(&cleaned),
        re_strlit.captures(&cleaned),
    ) {
        let target_panel = target.get(1).unwrap().as_str().to_string();
        let trigger_field = trigger.get(1).unwrap().as_str().to_string();
        let trigger_value = value.get(1).unwrap().as_str().to_string();

        ctx.visibility_conditions.insert(
            target_panel,
            VisibilityCondition {
                trigger_field,
                trigger_value,
            },
        );
    }
}

/// Parse the JSON content of an `fd:scripts` attribute value.
///
/// The format is a JSON array of objects like:
/// ```json
/// [{"script":{"content":"...", "event":"Initialize", "field":"..."}, "nodeName":"SCRIPTMODEL", "version":1, "enabled":true}]
/// ```
///
/// The JSON is typically XML-escaped with `&quot;` replaced by `"` and `\,` etc.
fn parse_fd_scripts_json(raw: &str, default_event: &str) -> Option<Vec<AemScript>> {
    // The raw value has already been XML-unescaped by quick-xml's unescape_value,
    // but it may still have backslash-escaped commas and quotes from JCR serialization.
    let cleaned = raw.replace("\\,", ",").replace("\\\"", "\"");

    let parsed: serde_json::Value = serde_json::from_str(&cleaned).ok()?;
    let arr = parsed.as_array()?;

    let mut scripts = Vec::new();
    for item in arr {
        let script_obj = item.get("script")?;
        let content = script_obj
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let event = script_obj
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or(default_event);
        let field = script_obj
            .get("field")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let enabled = item.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);

        if !content.is_empty() {
            scripts.push(AemScript {
                event: event.to_string(),
                content: content.to_string(),
                field: field.to_string(),
                enabled,
            });
        }
    }

    Some(scripts)
}

// ============================================================================
// Translation extraction
// ============================================================================

/// Extract Sling i18n translations from dictionary files in the ZIP.
fn extract_translations(zip_files: &HashMap<String, Vec<u8>>) -> TranslationData {
    let mut data = TranslationData::default();

    for (path, content) in zip_files {
        // Dictionary files are typically at paths like:
        // .../dictionary/de.xml, .../dictionary/en-us.xml, .../i18n/de.xml
        if !path.contains("dictionary/") && !path.contains("i18n/") {
            continue;
        }
        if !path.ends_with(".xml") {
            continue;
        }

        // Extract language from filename (e.g., "de.xml" → "de")
        let filename = path.rsplit('/').next().unwrap_or("");
        let lang = filename.trim_end_matches(".xml");
        if lang.is_empty() {
            continue;
        }

        let xml_str = String::from_utf8_lossy(content);
        parse_sling_dictionary(&xml_str, lang, &mut data);
    }

    data
}

/// Parse a Sling i18n dictionary XML file.
///
/// Format:
/// ```xml
/// <jcr:root ...>
///   <entry sling:key="some##key##123" sling:message="Translated text" .../>
/// </jcr:root>
/// ```
fn parse_sling_dictionary(xml: &str, language: &str, data: &mut TranslationData) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    loop {
        match reader.read_event() {
            Ok(Event::Empty(ref e) | Event::Start(ref e)) => {
                let mut sling_key = None;
                let mut sling_message = None;

                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                    let value = attr.unescape_value().unwrap_or_default().to_string();
                    match key.as_str() {
                        "sling:key" => sling_key = Some(value),
                        "sling:message" => sling_message = Some(value),
                        _ => {}
                    }
                }

                if let (Some(key), Some(message)) = (sling_key, sling_message) {
                    data.entries
                        .entry(key)
                        .or_default()
                        .insert(language.to_string(), message);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

// ============================================================================
// Attribute helpers
// ============================================================================

/// Parse the `visible` attribute from a JCR node.
/// Default is `true`. `{Boolean}false` or `false` → `false`.
fn parse_visible(node: &JcrNode) -> bool {
    match node.attr("visible") {
        Some(v) => !v.contains("false"),
        None => true,
    }
}

/// Parse a boolean attribute (like `mandatory`, `dorExclusion`).
fn parse_bool_attr(node: &JcrNode, attr: &str) -> bool {
    match node.attr(attr) {
        Some(v) => v.contains("true"),
        None => false,
    }
}

/// Parse radio/checkbox/dropdown options from a JCR node.
///
/// AEM stores options as pipe-delimited strings in `enum` and `enumNames` attributes,
/// or as child `<items>` elements.
fn parse_options(node: &JcrNode) -> Vec<AemOption> {
    let mut options = Vec::new();

    // Try enum/enumNames format: enum="[val1,val2]" enumNames="[Label 1,Label 2]"
    let enum_values = node.attr("enum").map(parse_jcr_array).unwrap_or_default();
    let enum_names = node
        .attr("enumNames")
        .map(parse_jcr_array)
        .unwrap_or_default();

    if !enum_values.is_empty() {
        for (i, value) in enum_values.iter().enumerate() {
            let label = enum_names.get(i).cloned().unwrap_or_else(|| value.clone());
            options.push(AemOption {
                label,
                value: value.clone(),
            });
        }
        return options;
    }

    // Try options="[1=Individual,2=Legal Entity]" format
    if let Some(opts_str) = node.attr("options") {
        let trimmed = opts_str.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let inner = &trimmed[1..trimmed.len() - 1];
            for entry in inner.split(',') {
                let entry = entry.trim();
                if let Some((value, label)) = entry.split_once('=') {
                    options.push(AemOption {
                        label: label.to_string(),
                        value: value.to_string(),
                    });
                }
            }
            if !options.is_empty() {
                return options;
            }
        }
    }

    // Try items child elements
    for child in &node.children {
        if child.tag_name == "items" {
            for item in &child.children {
                let value = item.attr("value").unwrap_or("").to_string();
                let label = item
                    .attr("jcr:title")
                    .or_else(|| item.attr("text"))
                    .unwrap_or(&value)
                    .to_string();
                options.push(AemOption { label, value });
            }
        }
    }

    options
}

/// Parse a JCR multi-value array like `"[val1,val2,val3]"` into a Vec<String>.
fn parse_jcr_array(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        trimmed[1..trimmed.len() - 1]
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![trimmed.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_jcr_array() {
        assert_eq!(parse_jcr_array("[a,b,c]"), vec!["a", "b", "c"]);
        assert_eq!(parse_jcr_array("single"), vec!["single"]);
        assert_eq!(parse_jcr_array("[]"), Vec::<String>::new());
    }

    #[test]
    fn test_parse_fd_scripts_json() {
        let raw = r#"[{"script":{"content":"this.visible = false;","event":"Initialize","field":"guide.rootPanel.myField"},"nodeName":"SCRIPTMODEL","version":1,"enabled":true}]"#;
        let scripts = parse_fd_scripts_json(raw, "Initialize").unwrap();
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].event, "Initialize");
        assert_eq!(scripts[0].content, "this.visible = false;");
        assert!(scripts[0].enabled);
    }

    #[test]
    fn test_detect_aem_zip_false_for_non_zip() {
        assert!(!detect_aem_zip(b"not a zip file"));
    }
}
