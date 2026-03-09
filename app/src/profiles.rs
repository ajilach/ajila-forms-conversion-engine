//! Embedded profile loading.
//!
//! The entire `profiles/` directory tree is baked into the binary at compile
//! time via [`include_dir!`].  This module provides helpers to list available
//! profiles and to load their AEM / HTML configuration from the embedded data.

use blueprint::{
    AemProfile, HtmlCustomStyles, HtmlProfile, ResolvedFontFamily, ResolvedFontVariant,
};
use include_dir::{Dir, include_dir};
use std::collections::HashMap;

/// The embedded `profiles/` directory.
static PROFILES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../profiles");

/// Return the names of all available profiles (top-level subdirectories).
pub fn list_profiles() -> Vec<String> {
    PROFILES_DIR
        .dirs()
        .filter_map(|d| d.path().file_name())
        .filter_map(|n| n.to_str())
        .map(String::from)
        .collect()
}

/// Load the AEM profile and component templates for the given profile name.
///
/// Returns `(AemProfile, HashMap<template_stem, template_xml>)`.
pub fn load_aem_profile(name: &str) -> Result<(AemProfile, HashMap<String, String>), String> {
    let aem_dir = PROFILES_DIR
        .get_dir(format!("{name}/aem"))
        .ok_or_else(|| format!("Profile '{name}' has no aem/ subdirectory"))?;

    // Read and parse config.toml
    let config_file = aem_dir
        .get_file(format!("{name}/aem/config.toml"))
        .ok_or_else(|| format!("Profile '{name}/aem' has no config.toml"))?;
    let toml_str = config_file
        .contents_utf8()
        .ok_or_else(|| "config.toml is not valid UTF-8".to_string())?;
    let profile: AemProfile =
        toml::from_str(toml_str).map_err(|e| format!("Failed to parse config.toml: {e}"))?;

    // Collect *.xml templates
    let mut templates = HashMap::new();
    for entry in aem_dir.files() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("xml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Some(content) = entry.contents_utf8() {
                    templates.insert(stem.to_string(), content.to_string());
                }
            }
        }
    }

    Ok((profile, templates))
}

/// Load the HTML custom styles for the given profile name.
///
/// Returns `None` if the profile has no `html/` subdirectory.
pub fn load_html_custom_styles(name: &str) -> Result<Option<HtmlCustomStyles>, String> {
    let html_dir = match PROFILES_DIR.get_dir(format!("{name}/html")) {
        Some(d) => d,
        None => return Ok(None),
    };

    // Read config.toml
    let config_file = match html_dir.get_file(format!("{name}/html/config.toml")) {
        Some(f) => f,
        None => return Ok(None),
    };
    let toml_str = config_file
        .contents_utf8()
        .ok_or_else(|| "html/config.toml is not valid UTF-8".to_string())?;
    let profile: HtmlProfile =
        toml::from_str(toml_str).map_err(|e| format!("Failed to parse html/config.toml: {e}"))?;

    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;

    // Resolve stylesheet
    let stylesheet_css = match &profile.stylesheet {
        Some(path) => {
            let full = format!("{name}/html/{}", path.display());
            let file = html_dir
                .get_file(&full)
                .ok_or_else(|| format!("Stylesheet '{full}' not found in embedded profile"))?;
            Some(
                file.contents_utf8()
                    .ok_or_else(|| "Stylesheet is not valid UTF-8".to_string())?
                    .to_string(),
            )
        }
        None => None,
    };

    // Resolve logo
    let logo_data_uri = match &profile.logo {
        Some(path) => {
            let full = format!("{name}/html/{}", path.display());
            let file = html_dir
                .get_file(&full)
                .ok_or_else(|| format!("Logo '{full}' not found in embedded profile"))?;
            let mime = mime_from_extension(path);
            let encoded = b64.encode(file.contents());
            Some(format!("data:{mime};base64,{encoded}"))
        }
        None => None,
    };

    // Resolve fonts
    let mut font_faces = Vec::new();
    for font_profile in &profile.fonts {
        let mut variants = Vec::new();

        let variant_specs: &[(
            &Option<std::path::PathBuf>,
            &str, // weight
            &str, // style
        )] = &[
            (&font_profile.regular, "normal", "normal"),
            (&font_profile.bold, "bold", "normal"),
            (&font_profile.italic, "normal", "italic"),
            (&font_profile.bold_italic, "bold", "italic"),
        ];

        for (opt_path, weight, style) in variant_specs {
            if let Some(path) = opt_path {
                let full = format!("{name}/html/{}", path.display());
                let file = match html_dir.get_file(&full) {
                    Some(f) => f,
                    None => continue,
                };
                let encoded = b64.encode(file.contents());
                variants.push(ResolvedFontVariant {
                    weight: weight.to_string(),
                    style: style.to_string(),
                    data_uri: format!("data:font/ttf;base64,{encoded}"),
                });
            }
        }

        font_faces.push(ResolvedFontFamily {
            family: font_profile.family.clone(),
            variants,
        });
    }

    Ok(Some(HtmlCustomStyles {
        stylesheet_css,
        logo_data_uri,
        font_faces,
    }))
}

/// Guess MIME type from file extension.
fn mime_from_extension(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}
