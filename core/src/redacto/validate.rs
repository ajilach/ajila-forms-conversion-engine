//! Structural checks on a generated [`RedactoDump`].
//!
//! The Redacto analogue of the `validate_aem_*` checks: it does not compare
//! against the source (that is [`review_redacto`](crate::review_redacto)), it
//! asks whether the dump is a usable document at all.
//!
//! The check that matters most is the dullest one — **a dump with an empty
//! body section is a failure, not an output**. Such a dump is still valid SQL:
//! it inserts a `documents` row that imports cleanly, which is exactly why an
//! empty one once shipped unnoticed. The body is the measure, not the assets:
//! the header and footer sections carry page furniture resolved from the
//! profile, so a contentless document can still have assets.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    INITIAL_VERSION, RedactoComponent, RedactoConfig, RedactoConfiguration, RedactoDump, asset_ref,
};

/// The outcome of validating a dump.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactoValidation {
    /// Reasons the dump should not be shipped.
    pub problems: Vec<String>,
    /// Content the converter could not represent, carried over from
    /// [`RedactoDump::warnings`]. Worth reading, but not disqualifying.
    pub warnings: Vec<String>,
    /// Row counts, for reporting what was produced.
    pub counts: RedactoCounts,
}

/// What a dump contains: row counts per table, and the shape of the component
/// tree in its configuration.
///
/// The component counts are here because row counts alone cannot show a lost
/// layout. A document whose multi-column sections were flattened has exactly
/// the same assets and variants as one that kept them — only its panels
/// disappear, so those have to be reported to be noticed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactoCounts {
    /// `assets` rows (one per text block).
    pub assets: usize,
    /// `asset_version` rows (one per asset per language).
    pub asset_versions: usize,
    /// `document_version` rows (one per language).
    pub document_versions: usize,
    /// Total `INSERT` statements the dump will emit.
    pub rows: usize,
    /// `assetContainer` components across all four sections.
    pub asset_containers: usize,
    /// `styledPanel` components, counted per style (e.g. `layout-split`,
    /// `footnote`), across all four sections.
    pub styled_panels: BTreeMap<String, usize>,
    /// Asset references in the `header` section (the page header ships as a
    /// text asset, so a document with page furniture must show one).
    pub header_assets: usize,
    /// Asset references in the `footer` section.
    pub footer_assets: usize,
}

/// Every section of a configuration, in page order.
fn sections(configuration: &RedactoConfiguration) -> [&[RedactoComponent]; 4] {
    [
        &configuration.first_header,
        &configuration.header,
        &configuration.body,
        &configuration.footer,
    ]
}

/// Count the components of a configuration section, recursing into panels.
fn count_components(
    components: &[RedactoComponent],
    containers: &mut usize,
    panels: &mut BTreeMap<String, usize>,
) {
    for component in components {
        match component {
            RedactoComponent::AssetContainer { .. } => *containers += 1,
            RedactoComponent::StyledPanel {
                style, components, ..
            } => {
                *panels.entry(style.clone()).or_default() += 1;
                count_components(components, containers, panels);
            }
        }
    }
}

/// Collect the asset references of a section, recursing into panels.
fn collect_refs(components: &[RedactoComponent], refs: &mut Vec<String>) {
    for component in components {
        match component {
            RedactoComponent::AssetContainer { assets, .. } => refs.extend(assets.iter().cloned()),
            RedactoComponent::StyledPanel { components, .. } => collect_refs(components, refs),
        }
    }
}

/// The asset references of one section.
fn section_refs(components: &[RedactoComponent]) -> Vec<String> {
    let mut refs = Vec::new();
    collect_refs(components, &mut refs);
    refs
}

impl RedactoValidation {
    /// Whether the dump is fit to ship.
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Check `dump` against `config` and report what would make it unusable.
///
/// Returns a struct rather than formatted text so callers (and tests) can act on
/// the individual findings; rendering is the caller's job.
pub fn validate_dump(dump: &RedactoDump, config: &RedactoConfig) -> RedactoValidation {
    let mut problems = Vec::new();

    // The guard this module exists for. Everything else is secondary.
    if dump.is_empty_document() {
        problems.push(
            "the body section is empty: the dump describes a document with no content".to_string(),
        );
    }

    // Every asset needs a variant in every configured language, or the document
    // renders blank in the languages that are missing.
    for language in &config.languages {
        let assets_in_language = dump
            .asset_versions
            .iter()
            .filter(|v| &v.language == language)
            .count();
        if !dump.assets.is_empty() && assets_in_language != dump.assets.len() {
            problems.push(format!(
                "language '{language}': {assets_in_language} asset_version row(s) for \
                 {} asset(s) — every asset needs one variant per language",
                dump.assets.len()
            ));
        }
        if !dump
            .document_versions
            .iter()
            .any(|v| &v.language == language)
        {
            problems.push(format!("language '{language}': no document_version row"));
        }
    }

    // An asset variant in a language the configuration does not list would be
    // unreachable — most likely the `default` pseudo-language that appears when
    // content lost its translations somewhere upstream.
    for version in &dump.asset_versions {
        if !config.languages.contains(&version.language) {
            problems.push(format!(
                "asset_version in unconfigured language '{}' (configured: {:?})",
                version.language, config.languages
            ));
            break;
        }
    }

    // The document owner row is mandatory: Redacto rejects every authoring write
    // against a document that has none.
    if dump.ownerships.is_empty() {
        problems.push("no ownership rows: Redacto would reject authoring writes".to_string());
    }

    let mut asset_containers = 0;
    let mut styled_panels = BTreeMap::new();
    let mut header_assets = 0;
    let mut footer_assets = 0;
    let emitted: BTreeSet<String> = dump
        .assets
        .iter()
        .map(|a| asset_ref(&a.asset_id, INITIAL_VERSION))
        .collect();

    for document in &dump.documents {
        let configuration = &document.configuration;
        for section in sections(configuration) {
            count_components(section, &mut asset_containers, &mut styled_panels);
        }
        header_assets += section_refs(&configuration.header).len();
        footer_assets += section_refs(&configuration.footer).len();

        // A section referencing an asset the dump does not insert renders as a
        // hole in the page — a wiring bug, not missing content.
        for section in sections(configuration) {
            for reference in section_refs(section) {
                if !emitted.contains(&reference) {
                    problems.push(format!(
                        "the configuration references asset '{reference}', which the dump \
                         does not insert"
                    ));
                }
            }
        }
    }

    RedactoValidation {
        problems,
        warnings: dump.warnings.clone(),
        counts: RedactoCounts {
            assets: dump.assets.len(),
            asset_versions: dump.asset_versions.len(),
            document_versions: dump.document_versions.len(),
            rows: dump.row_count(),
            asset_containers,
            styled_panels,
            header_assets,
            footer_assets,
        },
    }
}
