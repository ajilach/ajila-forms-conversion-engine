//! Embedded profile loading for AEM / HTML / XSD outputs.
//!
//! The entire `profiles/` directory is baked into the core crate at compile
//! time. Consumers (CLI, app, server) should load profile data through this
//! module instead of duplicating profile I/O logic.

use crate::{
    AemConfig, AemProfile, Context, HtmlCustomStyles, HtmlProfile, ResolvedFontFamily,
    ResolvedFontVariant, XsdConfig, XsdProfile, build_xsd_config_from_type_sources,
    parse_fragment_content,
};
use include_dir::{Dir, include_dir};
use serde::de::DeserializeOwned;
use std::collections::HashMap;

static PROFILES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../profiles");

/// Return all embedded profile names (top-level profile directories).
pub fn list_profiles() -> Vec<String> {
    PROFILES_DIR
        .dirs()
        .filter_map(|d| d.path().file_name())
        .filter_map(|n| n.to_str())
        .map(String::from)
        .collect()
}

/// Return whether `{profile}/aem/config.toml` exists.
pub fn has_aem_config(name: &str) -> bool {
    has_profile_config(name, "aem")
}

/// Return whether `{profile}/html/config.toml` exists.
pub fn has_html_config(name: &str) -> bool {
    has_profile_config(name, "html")
}

/// Return whether `{profile}/xsd/config.toml` exists.
pub fn has_xsd_config(name: &str) -> bool {
    has_profile_config(name, "xsd")
}

/// Load and parse `{profile}/aem/config.toml` and all top-level `*.xml`
/// component templates from `{profile}/aem/`.
pub fn load_aem_profile(name: &str) -> Result<(AemProfile, HashMap<String, String>), String> {
    let aem_dir = PROFILES_DIR
        .get_dir(format!("{name}/aem"))
        .ok_or_else(|| format!("Profile '{name}' has no aem/ subdirectory"))?;

    let profile: AemProfile = read_profile_config_toml(name, "aem")?;

    let mut templates = HashMap::new();
    for entry in aem_dir.files() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("xml")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            && let Some(content) = entry.contents_utf8()
        {
            templates.insert(stem.to_string(), content.to_string());
        }
    }

    Ok((profile, templates))
}

/// Build a full `AemConfig` for an embedded profile.
///
/// This includes:
/// - AEM profile + templates
/// - optional XSD binding (requires embedded xsd config when bind_to_xsd=true)
/// - optional embedded fragment scan
pub fn load_aem_config(name: &str, ctx: &Context) -> Result<AemConfig, String> {
    let (profile, templates) = load_aem_profile(name)?;

    let mut config = AemConfig::from_profile(&profile, templates, ctx)
        .map_err(|e| format!("Failed to build AEM config: {e}"))?;

    if config.bind_to_xsd {
        if !has_xsd_config(name) {
            return Err(format!(
                "bind_to_xsd=true requires profile '{name}' to provide xsd/config.toml"
            ));
        }
        config.xsd_config = Some(load_xsd_config(name)?);
    }

    if config.use_fragments {
        config.fragments = load_aem_fragments(name, &config.fragment_ref_prefix)?;
    }

    Ok(config)
}

/// Load HTML custom styles for an embedded profile.
pub fn load_html_custom_styles(name: &str) -> Result<HtmlCustomStyles, String> {
    let html_dir = PROFILES_DIR
        .get_dir(format!("{name}/html"))
        .ok_or_else(|| format!("Profile '{name}' has no html/ subdirectory"))?;

    let profile: HtmlProfile = read_profile_config_toml(name, "html")?;

    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;

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

    let mut font_faces = Vec::new();
    for font_profile in &profile.fonts {
        let mut variants = Vec::new();

        let variant_specs: &[(&Option<std::path::PathBuf>, &str, &str)] = &[
            (&font_profile.regular, "normal", "normal"),
            (&font_profile.bold, "bold", "normal"),
            (&font_profile.italic, "normal", "italic"),
            (&font_profile.bold_italic, "bold", "italic"),
        ];

        for (opt_path, weight, style) in variant_specs {
            if let Some(path) = opt_path {
                let full = format!("{name}/html/{}", path.display());
                let file = html_dir
                    .get_file(&full)
                    .ok_or_else(|| format!("Font '{full}' not found in embedded profile"))?;
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

    Ok(HtmlCustomStyles {
        stylesheet_css,
        logo_data_uri,
        font_faces,
    })
}

/// Load XSD config for an embedded profile.
pub fn load_xsd_config(name: &str) -> Result<XsdConfig, String> {
    let xsd_dir = PROFILES_DIR
        .get_dir(format!("{name}/xsd"))
        .ok_or_else(|| format!("Profile '{name}' has no xsd/ subdirectory"))?;

    let profile: XsdProfile = read_profile_config_toml(name, "xsd")?;

    let mut type_sources: Vec<(String, String)> = Vec::new();
    if let Some(types_dir) = xsd_dir.get_dir(format!("{name}/xsd/types")) {
        let types_root = types_dir.path();
        walk_embedded_dirs(types_dir, &mut |embedded_dir| {
            for file in embedded_dir.files() {
                if file.path().extension().and_then(|e| e.to_str()) == Some("xsd")
                    && let Some(content) = file.contents_utf8()
                {
                    let rel = relative_embedded_path(file.path(), types_root);
                    type_sources.push((rel, content.to_string()));
                }
            }
        });
        type_sources.sort_by(|a, b| a.0.cmp(&b.0));
    }

    Ok(build_xsd_config_from_type_sources(profile, &type_sources))
}

/// Load parsed AEM fragments from `{profile}/aem/fragments`.
pub fn load_aem_fragments(
    name: &str,
    fragment_ref_prefix: &str,
) -> Result<Vec<crate::ParsedFragment>, String> {
    let fragments_root = PROFILES_DIR
        .get_dir(format!("{name}/aem/fragments"))
        .ok_or_else(|| format!("Profile '{name}' has no aem/fragments directory"))?;

    let base = fragments_root.path();
    let prefix = fragment_ref_prefix.trim_end_matches('/');
    let mut fragments = Vec::new();

    walk_embedded_dirs(fragments_root, &mut |embedded_dir| {
        if let Some(content_file) = embedded_dir
            .files()
            .find(|f| f.path().file_name().and_then(|n| n.to_str()) == Some(".content.xml"))
            && let Some(content) = content_file.contents_utf8()
            && let Some(fragment) =
                parse_embedded_fragment(embedded_dir.path(), base, prefix, content)
        {
            fragments.push(fragment);
        }
    });

    fragments.sort_by(|a, b| a.frag_ref.cmp(&b.frag_ref));
    Ok(fragments)
}

fn has_profile_config(name: &str, section: &str) -> bool {
    PROFILES_DIR
        .get_file(format!("{name}/{section}/config.toml"))
        .is_some()
}

fn read_profile_config_toml<T>(name: &str, section: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let config_path = format!("{name}/{section}/config.toml");
    let config_file = PROFILES_DIR
        .get_file(&config_path)
        .ok_or_else(|| format!("Profile '{name}/{section}' has no config.toml"))?;
    let toml_str = config_file
        .contents_utf8()
        .ok_or_else(|| format!("{section}/config.toml is not valid UTF-8"))?;
    toml::from_str::<T>(toml_str).map_err(|e| format!("Failed to parse {section}/config.toml: {e}"))
}

fn walk_embedded_dirs(dir: &Dir<'_>, visit: &mut impl FnMut(&Dir<'_>)) {
    visit(dir);
    for child in dir.dirs() {
        walk_embedded_dirs(child, visit);
    }
}

fn parse_embedded_fragment(
    current_path: &std::path::Path,
    base_path: &std::path::Path,
    fragment_ref_prefix: &str,
    content: &str,
) -> Option<crate::ParsedFragment> {
    let rel = relative_embedded_path(current_path, base_path);
    parse_fragment_content(&rel, fragment_ref_prefix, content)
}

fn normalize_embedded_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn relative_embedded_path(path: &std::path::Path, root: &std::path::Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    normalize_embedded_path(rel)
        .trim_start_matches('/')
        .to_string()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ubs_profile_has_configs() {
        assert!(has_aem_config("ubs"));
        assert!(has_html_config("ubs"));
        assert!(has_xsd_config("ubs"));
        assert!(!has_aem_config("missing-profile"));
        assert!(!has_html_config("missing-profile"));
        assert!(!has_xsd_config("missing-profile"));
    }

    #[test]
    fn embedded_xsd_loader_fails_without_profile_config() {
        let err = load_xsd_config("akb").expect_err("akb has no xsd config");
        assert!(
            err.contains("has no xsd/ subdirectory") || err.contains("has no config.toml"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn embedded_html_loader_fails_without_profile_config() {
        let err = load_html_custom_styles("missing-profile").expect_err("missing profile");
        assert!(
            err.contains("has no html/ subdirectory") || err.contains("has no config.toml"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn embedded_xsd_loader_discovers_nested_type_files() {
        let cfg = load_xsd_config("ubs").expect("load ubs xsd config");

        assert!(
            cfg.type_to_file.contains_key("AddressType"),
            "Expected AddressType from nested xsd/types/** files"
        );
        assert!(
            cfg.registered_types.contains_key("AddressType"),
            "Expected parsed registered type AddressType"
        );
    }

    #[test]
    fn embedded_fragment_loader_parses_known_fragments() {
        let fragments =
            load_aem_fragments("ubs", "/content/forms/af/").expect("load embedded fragments");

        assert!(
            !fragments.is_empty(),
            "Expected at least one embedded AEM fragment"
        );
        assert!(
            fragments.iter().any(|f| f.xsd_type_name == "AddressType"),
            "Expected AddressType fragment in embedded profile"
        );
    }
}
