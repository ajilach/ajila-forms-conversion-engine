//! Structural checks on a generated [`RedactoDump`].
//!
//! The Redacto analogue of the `validate_aem_*` checks: it does not compare
//! against the source (that is [`review_redacto`](crate::review_redacto)), it
//! asks whether the dump is a usable document at all.
//!
//! The check that matters most is the dullest one — **a dump with no text
//! assets is a failure, not an output**. Such a dump is still valid SQL: it
//! inserts a `documents` row with an empty component list and imports cleanly,
//! which is exactly why an empty one once shipped unnoticed.

use super::{RedactoConfig, RedactoDump};

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

/// Per-table row counts of a dump.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RedactoCounts {
    /// `assets` rows (one per text block).
    pub assets: usize,
    /// `asset_version` rows (one per asset per language).
    pub asset_versions: usize,
    /// `document_version` rows (one per language).
    pub document_versions: usize,
    /// Total `INSERT` statements the dump will emit.
    pub rows: usize,
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
    if dump.assets.is_empty() {
        problems.push(
            "the dump contains no text assets: it describes an empty document".to_string(),
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
            problems.push(format!(
                "language '{language}': no document_version row"
            ));
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

    RedactoValidation {
        problems,
        warnings: dump.warnings.clone(),
        counts: RedactoCounts {
            assets: dump.assets.len(),
            asset_versions: dump.asset_versions.len(),
            document_versions: dump.document_versions.len(),
            rows: dump.row_count(),
        },
    }
}
