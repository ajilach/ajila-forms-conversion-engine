//! AEM FileVault ZIP package writer.
//!
//! Wraps a generated AEM Forms `.content.xml` in a complete Apache Jackrabbit
//! FileVault content package (ZIP) that can be uploaded directly to an AEM
//! instance via the Package Manager.

use std::collections::{BTreeSet, HashMap};
use std::io::{Cursor, Write};
use std::time::SystemTime;

use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use uuid::Uuid;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::{AemConfig, AemNode};
use crate::aem::generate_aem_xml;
use crate::aem::xml_writer::reformat_attributes;
use crate::structured::{
    FieldType, InlineNode, InlineText, StructuredNode, TranslatableString,
};

// ============================================================================
// Public API
// ============================================================================

/// Generate a complete AEM FileVault content package (ZIP) containing the
/// form page and its DAM asset metadata.
///
/// Returns the raw ZIP bytes that can be written to disk as a `.zip` file.
pub fn generate_aem_package(
    root: &AemNode,
    config: &AemConfig,
    content: &[StructuredNode],
) -> Vec<u8> {
    let form_xml = generate_aem_xml(root, config);
    let dam_xml = generate_dam_asset_xml(config);

    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let package_name = format!("BlueprintFormsPackage_{}", timestamp);

    let form_jcr_path = format!(
        "/content/forms/af/{}/{}",
        config.form_path, config.form_code
    );
    let dam_jcr_path = format!(
        "/content/dam/formsanddocuments/{}/{}",
        config.form_path, config.form_code
    );

    let filter_roots = vec![form_jcr_path.clone(), dam_jcr_path.clone()];

    let buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // ── META-INF ────────────────────────────────────────────────────────
    write_entry(
        &mut zip,
        &opts,
        "META-INF/MANIFEST.MF",
        &generate_manifest(&package_name, &filter_roots),
    );
    write_entry(&mut zip, &opts, "META-INF/vault/config.xml", VAULT_CONFIG);
    write_entry(
        &mut zip,
        &opts,
        "META-INF/vault/nodetypes.cnd",
        NODETYPES_CND,
    );
    write_entry(
        &mut zip,
        &opts,
        "META-INF/vault/filter.xml",
        &generate_filter_xml(&filter_roots),
    );
    write_entry(
        &mut zip,
        &opts,
        "META-INF/vault/properties.xml",
        &generate_properties_xml(&package_name, &config.author),
    );
    write_entry(
        &mut zip,
        &opts,
        "META-INF/vault/definition/.content.xml",
        &generate_definition_xml(&package_name, &config.author, &filter_roots),
    );

    // ── jcr_root boilerplate ────────────────────────────────────────────
    write_entry(&mut zip, &opts, "jcr_root/.content.xml", JCR_ROOT_XML);
    write_entry(
        &mut zip,
        &opts,
        "jcr_root/content/.content.xml",
        CONTENT_XML,
    );
    write_entry(
        &mut zip,
        &opts,
        "jcr_root/content/forms/.content.xml",
        FORMS_XML,
    );
    write_entry(
        &mut zip,
        &opts,
        "jcr_root/content/forms/af/.content.xml",
        AF_XML,
    );
    write_entry(
        &mut zip,
        &opts,
        "jcr_root/content/dam/.content.xml",
        DAM_XML,
    );
    write_entry(
        &mut zip,
        &opts,
        "jcr_root/content/dam/formsanddocuments/.content.xml",
        FORMSANDDOCUMENTS_XML,
    );

    // ── Intermediate folder .content.xml files ──────────────────────────
    let path_segments: Vec<&str> = config.form_path.split('/').collect();
    // content/forms/af/<seg1>/<seg2>/.../<form_code>
    write_intermediate_folders(&mut zip, &opts, "jcr_root/content/forms/af", &path_segments);
    // content/dam/formsanddocuments/<seg1>/<seg2>/.../<form_code>
    write_intermediate_folders(
        &mut zip,
        &opts,
        "jcr_root/content/dam/formsanddocuments",
        &path_segments,
    );

    // ── Form content .content.xml ───────────────────────────────────────
    let form_content_path = format!(
        "jcr_root/content/forms/af/{}/{}/.content.xml",
        config.form_path, config.form_code
    );
    write_entry(&mut zip, &opts, &form_content_path, &form_xml);

    // ── DAM asset .content.xml ──────────────────────────────────────────
    let dam_content_path = format!(
        "jcr_root/content/dam/formsanddocuments/{}/{}/.content.xml",
        config.form_path, config.form_code
    );
    write_entry(&mut zip, &opts, &dam_content_path, &dam_xml);

    // ── Translation dictionaries ────────────────────────────────────────
    let translations = extract_translations(content, &config.master_language);
    if !translations.is_empty() {
        let dict_base = format!(
            "jcr_root/content/forms/af/{}/{}/_jcr_content/guideContainer/assets/dictionary",
            config.form_path, config.form_code
        );
        let basename = format!(
            "/content/forms/af/{}/{}/jcr:content/guideContainer/assets/dictionary",
            config.form_path, config.form_code
        );

        // Collect all languages that have translations
        let mut languages = BTreeSet::<String>::new();
        for lang_map in translations.values() {
            languages.extend(lang_map.keys().cloned());
        }

        for lang in &languages {
            let entries: Vec<(String, String)> = translations
                .iter()
                .filter_map(|(master_text, lang_map)| {
                    lang_map
                        .get(lang.as_str())
                        .map(|translated| (master_text.clone(), translated.clone()))
                })
                .collect();

            if !entries.is_empty() {
                let dict_xml = generate_dictionary_xml(lang, &entries, &basename);
                let dict_path = format!("{}/{}.xml", dict_base, lang);
                write_entry(&mut zip, &opts, &dict_path, &dict_xml);
            }
        }
    }

    zip.finish().expect("finalize zip").into_inner()
}

// ============================================================================
// Helpers
// ============================================================================

fn write_entry(
    zip: &mut ZipWriter<Cursor<Vec<u8>>>,
    opts: &SimpleFileOptions,
    path: &str,
    content: &str,
) {
    zip.start_file(path, *opts).expect("zip start_file");
    zip.write_all(content.as_bytes()).expect("zip write");
}

/// Write `sling:OrderedFolder` `.content.xml` files for each intermediate
/// directory segment (e.g. `ajila-forms-ubs`, `output`, `Germany_Tranch_1`).
fn write_intermediate_folders(
    zip: &mut ZipWriter<Cursor<Vec<u8>>>,
    opts: &SimpleFileOptions,
    base: &str,
    segments: &[&str],
) {
    let mut current = base.to_string();
    for seg in segments {
        current = format!("{}/{}", current, seg);
        let path = format!("{}/.content.xml", current);
        write_entry(zip, opts, &path, ORDERED_FOLDER_XML);
    }
}

// ============================================================================
// DAM Asset XML generation
// ============================================================================

/// Generate the `dam:Asset` `.content.xml` for the DAM entry of the form.
fn generate_dam_asset_xml(config: &AemConfig) -> String {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut w = Writer::new_with_indent(&mut buf, b' ', 4);

        w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
            .unwrap();

        // <jcr:root>
        let mut root = BytesStart::new("jcr:root");
        root.push_attribute(("xmlns:sling", "http://sling.apache.org/jcr/sling/1.0"));
        root.push_attribute(("xmlns:fd", "http://www.adobe.com/aemfd/fd/1.0"));
        root.push_attribute(("xmlns:dam", "http://www.day.com/dam/1.0"));
        root.push_attribute(("xmlns:jcr", "http://www.jcp.org/jcr/1.0"));
        root.push_attribute(("xmlns:nt", "http://www.jcp.org/jcr/nt/1.0"));
        root.push_attribute(("jcr:primaryType", "dam:Asset"));
        w.write_event(Event::Start(root)).unwrap();

        // <jcr:content>
        let mut jcr_content = BytesStart::new("jcr:content");
        jcr_content.push_attribute(("jcr:primaryType", "dam:AssetContent"));
        jcr_content.push_attribute(("sling:resourceType", "fd/fm/af/render"));
        jcr_content.push_attribute(("guide", "1"));
        jcr_content.push_attribute(("type", "guide"));
        w.write_event(Event::Start(jcr_content)).unwrap();

        // <metadata>
        let mut meta = BytesStart::new("metadata");
        meta.push_attribute(("fd:version", "1.1"));
        meta.push_attribute(("jcr:primaryType", "nt:unstructured"));
        meta.push_attribute(("allowedRenderFormat", "HTML"));
        meta.push_attribute(("author", config.author.as_str()));
        if !config.dor_template_ref.is_empty() {
            meta.push_attribute(("dorTemplateRef", config.dor_template_ref.as_str()));
        }
        meta.push_attribute(("dorType", config.dor_type.as_str()));
        meta.push_attribute(("formmodel", "none"));
        meta.push_attribute(("hasCustomThumbnail", "{Boolean}false"));
        if !config.theme_ref.is_empty() {
            meta.push_attribute(("themeRef", config.theme_ref.as_str()));
        }
        meta.push_attribute(("title", config.form_title.as_str()));
        w.write_event(Event::Empty(meta)).unwrap();

        // </jcr:content>
        w.write_event(Event::End(BytesEnd::new("jcr:content")))
            .unwrap();
        // </jcr:root>
        w.write_event(Event::End(BytesEnd::new("jcr:root")))
            .unwrap();
    }

    let raw = String::from_utf8(buf.into_inner()).expect("UTF-8 dam xml");
    reformat_attributes(&raw)
}

// ============================================================================
// META-INF generators
// ============================================================================

fn generate_manifest(package_name: &str, roots: &[String]) -> String {
    // MANIFEST.MF has a 72-byte line limit. We use continuation lines
    // (starting with a single space) for long values.
    let roots_value = roots.join(",");
    let mut manifest = String::new();
    manifest.push_str("Manifest-Version: 1.0\r\n");
    write_manifest_entry(
        &mut manifest,
        "Content-Package-Id",
        &format!("fd/export:{}", package_name),
    );
    write_manifest_entry(&mut manifest, "Content-Package-Roots", &roots_value);
    write_manifest_entry(&mut manifest, "Content-Package-Type", "mixed");
    manifest.push_str("\r\n");
    manifest
}

/// Write a MANIFEST.MF entry, wrapping at 72 bytes with continuation lines.
fn write_manifest_entry(manifest: &mut String, key: &str, value: &str) {
    let line = format!("{}: {}", key, value);
    let bytes = line.as_bytes();
    if bytes.len() <= 72 {
        manifest.push_str(&line);
        manifest.push_str("\r\n");
    } else {
        // First line: up to 72 bytes
        let first = &bytes[..72];
        manifest.push_str(&String::from_utf8_lossy(first));
        manifest.push_str("\r\n");
        // Continuation lines: space + up to 71 bytes
        let mut pos = 72;
        while pos < bytes.len() {
            let end = (pos + 71).min(bytes.len());
            manifest.push(' ');
            manifest.push_str(&String::from_utf8_lossy(&bytes[pos..end]));
            manifest.push_str("\r\n");
            pos = end;
        }
    }
}

fn generate_filter_xml(roots: &[String]) -> String {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut w = Writer::new_with_indent(&mut buf, b' ', 4);

        w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
            .unwrap();

        let mut ws = BytesStart::new("workspaceFilter");
        ws.push_attribute(("version", "1.0"));
        w.write_event(Event::Start(ws)).unwrap();

        for root in roots {
            let mut f = BytesStart::new("filter");
            f.push_attribute(("root", root.as_str()));
            w.write_event(Event::Empty(f)).unwrap();
        }

        w.write_event(Event::End(BytesEnd::new("workspaceFilter")))
            .unwrap();
    }
    String::from_utf8(buf.into_inner()).expect("UTF-8 filter xml")
}

fn generate_properties_xml(package_name: &str, author: &str) -> String {
    let now = iso_now();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE properties SYSTEM "http://java.sun.com/dtd/properties.dtd">
<properties>
<comment>FileVault Package Properties</comment>
<entry key="packageType">mixed</entry>
<entry key="lastWrappedBy">{author}</entry>
<entry key="packageFormatVersion">2</entry>
<entry key="group">fd/export</entry>
<entry key="created">{now}</entry>
<entry key="lastModifiedBy">{author}</entry>
<entry key="buildCount">1</entry>
<entry key="lastWrapped">{now}</entry>
<entry key="version"></entry>
<entry key="dependencies"></entry>
<entry key="createdBy">{author}</entry>
<entry key="name">{package_name}</entry>
<entry key="lastModified">{now}</entry>
</properties>
"#
    )
}

fn generate_definition_xml(package_name: &str, author: &str, roots: &[String]) -> String {
    let now = iso_now();
    let mut buf = Cursor::new(Vec::new());
    {
        let mut w = Writer::new_with_indent(&mut buf, b' ', 4);

        w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
            .unwrap();

        let mut root_elem = BytesStart::new("jcr:root");
        root_elem.push_attribute(("xmlns:vlt", "http://www.day.com/jcr/vault/1.0"));
        root_elem.push_attribute(("xmlns:jcr", "http://www.jcp.org/jcr/1.0"));
        root_elem.push_attribute(("xmlns:nt", "http://www.jcp.org/jcr/nt/1.0"));
        root_elem.push_attribute(("jcr:primaryType", "vlt:PackageDefinition"));
        root_elem.push_attribute(("buildCount", "1"));
        root_elem.push_attribute(("group", "fd/export"));
        let created = format!("{{Date}}{}", now);
        root_elem.push_attribute(("jcr:created", created.as_str()));
        root_elem.push_attribute(("jcr:createdBy", author));
        root_elem.push_attribute(("jcr:lastModified", created.as_str()));
        root_elem.push_attribute(("jcr:lastModifiedBy", author));
        root_elem.push_attribute(("lastWrapped", created.as_str()));
        root_elem.push_attribute(("lastWrappedBy", author));
        root_elem.push_attribute(("name", package_name));
        root_elem.push_attribute(("version", ""));
        w.write_event(Event::Start(root_elem)).unwrap();

        // <filter>
        let mut filter_elem = BytesStart::new("filter");
        filter_elem.push_attribute(("jcr:primaryType", "nt:unstructured"));
        w.write_event(Event::Start(filter_elem)).unwrap();

        for (i, root) in roots.iter().enumerate() {
            let tag = format!("f{}", i);
            let mut f = BytesStart::new(tag.as_str());
            f.push_attribute(("jcr:primaryType", "nt:unstructured"));
            f.push_attribute(("mode", "replace"));
            f.push_attribute(("root", root.as_str()));
            f.push_attribute(("rules", "[]"));
            w.write_event(Event::Empty(f)).unwrap();
        }

        w.write_event(Event::End(BytesEnd::new("filter"))).unwrap();
        w.write_event(Event::End(BytesEnd::new("jcr:root")))
            .unwrap();
    }
    let raw = String::from_utf8(buf.into_inner()).expect("UTF-8 definition xml");
    reformat_attributes(&raw)
}

/// Produce an ISO 8601 timestamp like `2026-02-16T12:00:00.000+00:00`.
fn iso_now() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();

    // Simple UTC timestamp (no chrono dependency)
    let days = secs / 86400;
    let day_secs = secs % 86400;
    let hours = day_secs / 3600;
    let minutes = (day_secs % 3600) / 60;
    let seconds = day_secs % 60;

    // Calculate date from days since epoch (1970-01-01)
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}+00:00",
        year, month, day, hours, minutes, seconds, millis
    )
}

/// Convert days since 1970-01-01 to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ============================================================================
// Language detection
// ============================================================================

/// Detect the best master language from the structured content.
///
/// Scans all `TranslatableString::Translated` and `InlineNode::TranslatedText`
/// entries. Returns `"en"` if English is among the available languages,
/// otherwise the first language alphabetically. Falls back to `"en"` if no
/// translations are present at all.
pub fn detect_master_language(content: &[StructuredNode]) -> String {
    let langs = collect_languages(content);
    if langs.is_empty() || langs.contains("en") {
        "en".into()
    } else {
        // First alphabetically
        langs.into_iter().next().unwrap()
    }
}

/// Collect all language codes present in the structured content.
pub fn collect_languages(content: &[StructuredNode]) -> BTreeSet<String> {
    let mut langs = BTreeSet::new();
    for node in content {
        collect_langs_from_node(node, &mut langs);
    }
    langs
}

fn collect_langs_from_node(node: &StructuredNode, langs: &mut BTreeSet<String>) {
    match node {
        StructuredNode::Heading(h) => collect_langs_from_inline_text(&h.content, langs),
        StructuredNode::Paragraph(p) => collect_langs_from_inline_text(&p.content, langs),
        StructuredNode::Field(f) => {
            if let Some(label) = &f.label {
                collect_langs_from_inline_text(label, langs);
            }
            if let Some(TranslatableString::Translated(tmap)) = &f.placeholder {
                langs.extend(tmap.keys().cloned());
            }
            match &f.input_type {
                FieldType::Radio { options } | FieldType::Select { options } => {
                    for opt in options {
                        if let TranslatableString::Translated(tmap) = &opt.name {
                            langs.extend(tmap.keys().cloned());
                        }
                    }
                }
                _ => {}
            }
        }
        StructuredNode::Table(t) => {
            if let Some(caption) = &t.caption {
                collect_langs_from_inline_text(caption, langs);
            }
            if let Some(header) = &t.header {
                for cell in &header.cells {
                    collect_langs_from_node(cell, langs);
                }
            }
            for row in &t.rows {
                for cell in &row.cells {
                    collect_langs_from_node(cell, langs);
                }
            }
        }
        StructuredNode::Group(g) => {
            for child in &g.children {
                collect_langs_from_node(child, langs);
            }
        }
        StructuredNode::Repeatable(r) => {
            collect_langs_from_node(&r.item, langs);
        }
        StructuredNode::Conditional(c) => {
            collect_langs_from_node(&c.content, langs);
        }
        StructuredNode::GridLayout(g) => {
            for elem in &g.elements {
                collect_langs_from_node(&elem.node, langs);
            }
        }
        _ => {}
    }
}

fn collect_langs_from_inline_text(text: &InlineText, langs: &mut BTreeSet<String>) {
    for node in &text.0 {
        collect_langs_from_inline_node(node, langs);
    }
}

fn collect_langs_from_inline_node(node: &InlineNode, langs: &mut BTreeSet<String>) {
    match node {
        InlineNode::TranslatedText(tmap) => {
            langs.extend(tmap.keys().cloned());
        }
        InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
            collect_langs_from_inline_node(inner, langs);
        }
        InlineNode::Link(link) => {
            collect_langs_from_inline_text(&link.content, langs);
        }
        InlineNode::Text(_) => {}
    }
}

// ============================================================================
// Translation extraction
// ============================================================================

/// A map of master-language text → { lang_code → translated_text }.
type TranslationMap = HashMap<String, HashMap<String, String>>;

/// Walk the structured node tree and extract all translatable strings.
///
/// Returns a map where each key is the master-language text and each value
/// is a map of language codes to their translations.
fn extract_translations(nodes: &[StructuredNode], master_lang: &str) -> TranslationMap {
    let mut map = TranslationMap::new();
    for node in nodes {
        extract_from_node(node, master_lang, &mut map);
    }
    map
}

fn extract_from_node(node: &StructuredNode, master_lang: &str, map: &mut TranslationMap) {
    match node {
        StructuredNode::Heading(h) => extract_from_inline_text(&h.content, master_lang, map),
        StructuredNode::Paragraph(p) => extract_from_inline_text(&p.content, master_lang, map),
        StructuredNode::Field(f) => {
            if let Some(label) = &f.label {
                extract_from_inline_text(label, master_lang, map);
            }
            if let Some(TranslatableString::Translated(tmap)) = &f.placeholder {
                if let Some(master) = tmap.get(master_lang) {
                    let others: HashMap<String, String> = tmap
                        .iter()
                        .filter(|(k, _)| k.as_str() != master_lang)
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    if !others.is_empty() {
                        map.insert(master.clone(), others);
                    }
                }
            }
            match &f.input_type {
                FieldType::Radio { options } | FieldType::Select { options } => {
                    for opt in options {
                        if let TranslatableString::Translated(tmap) = &opt.name {
                            if let Some(master) = tmap.get(master_lang) {
                                let others: HashMap<String, String> = tmap
                                    .iter()
                                    .filter(|(k, _)| k.as_str() != master_lang)
                                    .map(|(k, v)| (k.clone(), v.clone()))
                                    .collect();
                                if !others.is_empty() {
                                    map.insert(master.clone(), others);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        StructuredNode::Table(t) => {
            if let Some(caption) = &t.caption {
                extract_from_inline_text(caption, master_lang, map);
            }
            if let Some(header) = &t.header {
                for cell in &header.cells {
                    extract_from_node(cell, master_lang, map);
                }
            }
            for row in &t.rows {
                for cell in &row.cells {
                    extract_from_node(cell, master_lang, map);
                }
            }
        }
        StructuredNode::Group(g) => {
            for child in &g.children {
                extract_from_node(child, master_lang, map);
            }
        }
        StructuredNode::Repeatable(r) => {
            extract_from_node(&r.item, master_lang, map);
        }
        StructuredNode::Conditional(c) => {
            extract_from_node(&c.content, master_lang, map);
        }
        StructuredNode::GridLayout(g) => {
            for elem in &g.elements {
                extract_from_node(&elem.node, master_lang, map);
            }
        }
        _ => {}
    }
}

fn extract_from_inline_text(text: &InlineText, master_lang: &str, map: &mut TranslationMap) {
    for node in &text.0 {
        extract_from_inline_node(node, master_lang, map);
    }
}

fn extract_from_inline_node(node: &InlineNode, master_lang: &str, map: &mut TranslationMap) {
    match node {
        InlineNode::TranslatedText(tmap) => {
            if let Some(master) = tmap.get(master_lang) {
                let others: HashMap<String, String> = tmap
                    .iter()
                    .filter(|(k, _)| k.as_str() != master_lang)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if !others.is_empty() {
                    map.insert(master.clone(), others);
                }
            }
        }
        InlineNode::Strong(inner) | InlineNode::Emphasis(inner) => {
            extract_from_inline_node(inner, master_lang, map);
        }
        InlineNode::Link(link) => {
            extract_from_inline_text(&link.content, master_lang, map);
        }
        InlineNode::Text(_) => {}
    }
}

// ============================================================================
// Dictionary XML generation
// ============================================================================

/// Generate a Sling dictionary XML file for a single locale.
fn generate_dictionary_xml(locale: &str, entries: &[(String, String)], basename: &str) -> String {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut w = Writer::new_with_indent(&mut buf, b' ', 4);

        w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
            .unwrap();

        // <jcr:root>
        let mut root = BytesStart::new("jcr:root");
        root.push_attribute(("xmlns:sling", "http://sling.apache.org/jcr/sling/1.0"));
        root.push_attribute(("xmlns:jcr", "http://www.jcp.org/jcr/1.0"));
        root.push_attribute(("xmlns:mix", "http://www.jcp.org/jcr/mix/1.0"));
        root.push_attribute(("xmlns:nt", "http://www.jcp.org/jcr/nt/1.0"));
        root.push_attribute(("jcr:language", locale));
        root.push_attribute(("jcr:mixinTypes", "[mix:language]"));
        root.push_attribute(("jcr:primaryType", "sling:Folder"));
        root.push_attribute(("sling:basename", basename));
        w.write_event(Event::Start(root)).unwrap();

        // Fixed namespace for deterministic UUIDs
        let ns = Uuid::NAMESPACE_URL;

        for (master_text, translated_text) in entries {
            let key = format!("fd_{}", master_text);

            // Deterministic element name from the key
            let uuid = Uuid::new_v5(&ns, key.as_bytes());
            let elem_name = format!("fd_{}", uuid.as_hyphenated());

            let mut entry = BytesStart::new(elem_name.as_str());
            entry.push_attribute(("jcr:mixinTypes", "[sling:Message]"));
            entry.push_attribute(("jcr:primaryType", "nt:folder"));
            entry.push_attribute(("sling:key", key.as_str()));
            entry.push_attribute(("sling:message", translated_text.as_str()));
            w.write_event(Event::Empty(entry)).unwrap();
        }

        w.write_event(Event::End(BytesEnd::new("jcr:root"))).unwrap();
    }

    let raw = String::from_utf8(buf.into_inner()).expect("UTF-8 dictionary xml");
    reformat_attributes(&raw)
}

// ============================================================================
// Static boilerplate content
// ============================================================================

const JCR_ROOT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<jcr:root xmlns:sling="http://sling.apache.org/jcr/sling/1.0" xmlns:jcr="http://www.jcp.org/jcr/1.0" xmlns:rep="internal"
    jcr:mixinTypes="[rep:AccessControllable,rep:RepoAccessControllable]"
    jcr:primaryType="rep:root"
    sling:resourceType="sling:redirect"
    sling:target="/index.html"/>
"#;

const CONTENT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<jcr:root xmlns:sling="http://sling.apache.org/jcr/sling/1.0" xmlns:cq="http://www.day.com/jcr/cq/1.0" xmlns:jcr="http://www.jcp.org/jcr/1.0" xmlns:rep="internal"
    jcr:mixinTypes="[rep:AccessControllable]"
    jcr:primaryType="sling:OrderedFolder">
    <rep:policy/>
    <dam/>
    <forms/>
</jcr:root>
"#;

const FORMS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<jcr:root xmlns:sling="http://sling.apache.org/jcr/sling/1.0" xmlns:jcr="http://www.jcp.org/jcr/1.0"
    jcr:primaryType="sling:OrderedFolder">
    <af/>
</jcr:root>
"#;

const AF_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<jcr:root xmlns:sling="http://sling.apache.org/jcr/sling/1.0" xmlns:jcr="http://www.jcp.org/jcr/1.0" xmlns:rep="internal"
    jcr:mixinTypes="[rep:AccessControllable]"
    jcr:primaryType="sling:Folder"
    hidden="true"/>
"#;

const DAM_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<jcr:root xmlns:sling="http://sling.apache.org/jcr/sling/1.0" xmlns:jcr="http://www.jcp.org/jcr/1.0" xmlns:rep="internal"
    jcr:mixinTypes="[rep:AccessControllable]"
    jcr:primaryType="sling:Folder"/>
"#;

const FORMSANDDOCUMENTS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<jcr:root xmlns:sling="http://sling.apache.org/jcr/sling/1.0" xmlns:jcr="http://www.jcp.org/jcr/1.0" xmlns:rep="internal"
    jcr:mixinTypes="[rep:AccessControllable]"
    jcr:primaryType="sling:Folder"
    hidden="true"/>
"#;

const ORDERED_FOLDER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<jcr:root xmlns:sling="http://sling.apache.org/jcr/sling/1.0" xmlns:jcr="http://www.jcp.org/jcr/1.0"
    jcr:primaryType="sling:OrderedFolder"/>
"#;

const VAULT_CONFIG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<vaultfs version="1.1">
    <aggregates>
        <aggregate type="file" title="File Aggregate"/>
        <aggregate type="filefolder" title="File/Folder Aggregate"/>
        <aggregate type="nodetype" title="Node Type Aggregate" />
        <aggregate type="full" title="Full Coverage Aggregate">
            <matches>
                <include nodeType="rep:AccessControl" respectSupertype="true" />
                <include nodeType="rep:Policy" respectSupertype="true" />
                <include nodeType="cq:Widget" respectSupertype="true" />
                <include nodeType="cq:EditConfig" respectSupertype="true" />
                <include nodeType="cq:WorkflowModel" respectSupertype="true" />
                <include nodeType="vlt:FullCoverage" respectSupertype="true" />
                <include nodeType="mix:language" respectSupertype="true" />
                <include nodeType="sling:OsgiConfig" respectSupertype="true" />
            </matches>
        </aggregate>
        <aggregate type="generic" title="Folder Aggregate">
            <matches>
                <include nodeType="nt:folder" respectSupertype="true" />
            </matches>
            <contains>
                <exclude isNode="true" />
            </contains>
        </aggregate>
        <aggregate type="generic" title="Default Aggregator" isDefault="true">
            <matches>
            </matches>
            <contains>
                <exclude nodeType="nt:hierarchyNode" respectSupertype="true" />
            </contains>
        </aggregate>
    </aggregates>
    <handlers>
        <handler type="folder"/>
        <handler type="file"/>
        <handler type="nodetype"/>
        <handler type="generic"/>
    </handlers>
</vaultfs>
"#;

const NODETYPES_CND: &str = r#"<'sling'='http://sling.apache.org/jcr/sling/1.0'>
<'cq'='http://www.day.com/jcr/cq/1.0'>
<'nt'='http://www.jcp.org/jcr/nt/1.0'>
<'jcr'='http://www.jcp.org/jcr/1.0'>
<'rep'='internal'>
<'dam'='http://www.day.com/dam/1.0'>
<'oak'='http://jackrabbit.apache.org/oak/ns/1.0'>
<'mix'='http://www.jcp.org/jcr/mix/1.0'>
<'fd'='http://www.adobe.com/aemfd/fd/1.0'>

[sling:Resource]
  mixin
  - sling:resourceType (string)

[cq:ClientLibraryFolder] > sling:Folder
  - dependencies (string) multiple
  - categories (string) multiple
  - embed (string) multiple
  - channels (string) multiple

[sling:Folder] > nt:folder
  - * (undefined) multiple
  - * (undefined)
  + * (nt:base) = sling:Folder version

[cq:Page] > nt:hierarchyNode
  orderable primaryitem jcr:content
  + jcr:content (nt:base) = nt:unstructured
  + * (nt:base) = nt:base version

[cq:Taggable]
  mixin
  - cq:tags (string) multiple

[sling:Message]
  mixin
  - sling:key (string)
  - sling:message (undefined)

[sling:OrderedFolder] > sling:Folder
  orderable
  + * (nt:base) = sling:OrderedFolder version

[cq:ReplicationStatus]
  mixin
  - cq:lastReplicatedBy (string) ignore
  - cq:lastPublished (date) ignore
  - cq:lastReplicationStatus (string) ignore
  - cq:lastPublishedBy (string) ignore
  - cq:lastReplicationAction (string) ignore
  - cq:lastReplicated (date) ignore

[rep:RepoAccessControllable]
  mixin
  + rep:repoPolicy (rep:Policy) protected ignore

[dam:Asset] > nt:hierarchyNode
  primaryitem jcr:content
  + jcr:content (dam:AssetContent) = dam:AssetContent
  + * (nt:base) = nt:base version

[dam:AssetContent] > nt:unstructured
  + metadata (nt:unstructured)
  + related (nt:unstructured)
  + renditions (nt:folder)

[oak:Resource] > mix:lastModified, mix:mimeType
  primaryitem jcr:data
  - jcr:data (binary) mandatory

[cq:PageContent] > cq:OwnerTaggable, cq:ReplicationStatus, mix:created, mix:title, nt:unstructured, sling:Resource, sling:VanityPath
  orderable
  - cq:lastModified (date)
  - cq:template (string)
  - pageTitle (string)
  - offTime (date)
  - hideInNav (boolean)
  - cq:lastModifiedBy (string)
  - onTime (date)
  - jcr:language (string)
  - cq:allowedTemplates (string) multiple
  - cq:designPath (string)
  - navTitle (string)

[cq:OwnerTaggable] > cq:Taggable
  mixin

[sling:VanityPath]
  mixin
  - sling:vanityPath (string) multiple
  - sling:redirect (boolean)
  - sling:vanityOrder (long)
  - sling:redirectStatus (long)

[sling:MessageEntry] > nt:hierarchyNode, sling:Message

[fd:xdp]
  mixin
  - fd:trusted (boolean)
"#;
