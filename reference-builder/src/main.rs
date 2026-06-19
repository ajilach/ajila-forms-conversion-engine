//! `reference-builder` — offline generator for the reference-form dataset the
//! app imports.
//!
//! Given a manifest pairing each reference's **input PDF**, **final AEM
//! package**, and **description**, it produces a profile-agnostic SQLite file
//! using the shared [`blueprint::reference_db`] schema. The app imports that
//! file via Settings (stamping the chosen profile).
//!
//! Usage:
//! ```text
//! reference-builder build --manifest refs.toml --out references-export.db
//! ```
//!
//! Manifest (`refs.toml`):
//! ```toml
//! [[reference]]
//! pdf = "forms/account-opening.pdf"
//! package = "forms/account-opening.zip"
//! description = "Account opening form ..."     # or: description_file = "desc.txt"
//! label = "Account opening"                     # optional (defaults to the PDF stem)
//! ```
//! Paths are resolved relative to the manifest file.

use std::io::Read;
use std::path::{Path, PathBuf};

use blueprint::reference_db::{EMBEDDING_MODEL_VERSION, SCHEMA_SQL, vec_to_blob};
use blueprint::semantic::SemanticMatcher;
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    reference: Vec<ReferenceRow>,
}

#[derive(Deserialize)]
struct ReferenceRow {
    pdf: String,
    package: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    description_file: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    // Parse: build --manifest <path> --out <path>
    let mut manifest_path: Option<String> = None;
    let mut out_path: Option<String> = None;
    let mut i = 1;
    if args.get(1).map(String::as_str) != Some("build") {
        return Err("usage: reference-builder build --manifest <refs.toml> --out <out.db>".into());
    }
    i += 1;
    while i < args.len() {
        match args[i].as_str() {
            "--manifest" => {
                manifest_path = args.get(i + 1).cloned();
                i += 2;
            }
            "--out" => {
                out_path = args.get(i + 1).cloned();
                i += 2;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let manifest_path = manifest_path.ok_or("missing --manifest")?;
    let out_path = out_path.ok_or("missing --out")?;

    let manifest_dir = Path::new(&manifest_path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest_text =
        std::fs::read_to_string(&manifest_path).map_err(|e| format!("read manifest: {e}"))?;
    let manifest: Manifest =
        toml::from_str(&manifest_text).map_err(|e| format!("parse manifest: {e}"))?;
    if manifest.reference.is_empty() {
        return Err("manifest has no [[reference]] entries".into());
    }

    // Fresh output DB with the shared schema.
    let conn = rusqlite::Connection::open(&out_path).map_err(|e| format!("open out: {e}"))?;
    conn.execute_batch(SCHEMA_SQL)
        .map_err(|e| format!("create schema: {e}"))?;

    let matcher = SemanticMatcher::new().map_err(|e| format!("load embedding model: {e}"))?;
    let created_at = "1970-01-01T00:00:00Z"; // deterministic; app stamps real time on use

    let mut written = 0usize;
    for (idx, row) in manifest.reference.iter().enumerate() {
        let pdf_path = resolve(&manifest_dir, &row.pdf);
        let pkg_path = resolve(&manifest_dir, &row.package);
        let pdf_bytes = std::fs::read(&pdf_path)
            .map_err(|e| format!("[{idx}] read pdf {}: {e}", pdf_path.display()))?;
        let pkg_bytes = std::fs::read(&pkg_path)
            .map_err(|e| format!("[{idx}] read package {}: {e}", pkg_path.display()))?;

        let description = match (&row.description, &row.description_file) {
            (Some(d), _) => d.clone(),
            (None, Some(df)) => std::fs::read_to_string(resolve(&manifest_dir, df))
                .map_err(|e| format!("[{idx}] read description_file: {e}"))?,
            (None, None) => {
                return Err(format!(
                    "[{idx}] reference needs `description` or `description_file`"
                ));
            }
        };
        let label = row.label.clone().unwrap_or_else(|| {
            Path::new(&row.pdf)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("reference {idx}"))
        });

        let ref_id = ref_id_for(&pdf_bytes);
        let embedding = matcher
            .embed(&description)
            .map_err(|e| format!("[{idx}] embed: {e}"))?;
        let page_count = blueprint::Blueprint::from_pdf_bytes(&pdf_bytes)
            .ok()
            .and_then(|mut bp| bp.states().ok().map(|s| s.iter().count() as i64))
            .unwrap_or(0);
        let files = unzip_text(&pkg_bytes).map_err(|e| format!("[{idx}] unzip package: {e}"))?;

        conn.execute(
            "INSERT OR REPLACE INTO references_
             (ref_id, profile, label, description, description_embedding, model_version, created_at)
             VALUES (?1, '', ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                ref_id,
                label,
                description,
                vec_to_blob(&embedding),
                EMBEDDING_MODEL_VERSION,
                created_at
            ],
        )
        .map_err(|e| format!("[{idx}] insert reference: {e}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO reference_pdfs (ref_id, pdf_index, page_count, pdf_bytes)
             VALUES (?1, 0, ?2, ?3)",
            rusqlite::params![ref_id, page_count, pdf_bytes],
        )
        .map_err(|e| format!("[{idx}] insert pdf: {e}"))?;
        for (path, content) in &files {
            conn.execute(
                "INSERT OR REPLACE INTO reference_files (ref_id, path, content) VALUES (?1, ?2, ?3)",
                rusqlite::params![ref_id, path, content],
            )
            .map_err(|e| format!("[{idx}] insert file {path}: {e}"))?;
        }

        println!("✓ {label}  ({ref_id}, {} package files)", files.len());
        written += 1;
    }

    println!("Wrote {written} reference(s) to {out_path}");
    Ok(())
}

fn resolve(base: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// Reproduce the app's `document_hash` for a single input file so app-added and
/// builder-built references share the same `ref_id`. The app hashes each file,
/// sorts the hex digests, and hashes their concatenation.
fn ref_id_for(pdf_bytes: &[u8]) -> String {
    let digest = sha256_hex(pdf_bytes);
    sha256_hex(digest.as_bytes())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Unzip an AEM package into `(path, content)` UTF-8 text rows, skipping
/// directories and binary entries.
fn unzip_text(zip_bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
    let reader = std::io::Cursor::new(zip_bytes);
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut entry = match zip.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        if entry.read_to_end(&mut bytes).is_err() {
            continue;
        }
        if let Ok(text) = String::from_utf8(bytes) {
            out.push((name, text));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the exact `ref_id` for a fixed input. The app's `references` module
    /// has the identical assertion, so builder-built and app-added references
    /// for the same form share an id (and import upserts rather than duplicates).
    #[test]
    fn ref_id_matches_canonical_hash() {
        assert_eq!(
            ref_id_for(b"reference-id-test"),
            "c0314ff86abc2dcf5cfd090203e5d24eb456bbff56e290ead6cd302fd16af90a"
        );
    }
}
