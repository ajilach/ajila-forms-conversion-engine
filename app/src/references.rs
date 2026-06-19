//! Per-profile **reference-form store** and the searches that back the LLM
//! tools.
//!
//! A reference form is a worked example: the original input PDF, the final AEM
//! package, and an LLM-written description. References live in the app's single
//! `history.db` (shared schema in [`blueprint::reference_db`]); only the
//! reference tables are ever written here, so settings/sessions are untouched.
//!
//! The store is keyed by `ref_id` = `document_hash(input PDF)` (the same hash
//! mechanism sessions use), so re-adding the same form is an idempotent upsert.
//!
//! Everything is desktop-only; on `wasm32` the functions are no-op stubs.

/// Metadata about one stored reference, for listing in Settings and the
/// `list_reference_forms` tool.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReferenceInfo {
    pub ref_id: String,
    pub profile: String,
    pub label: String,
    pub description: String,
    pub pdf_count: usize,
    /// Top-level package file paths (for tool orientation).
    pub files: Vec<String>,
}

/// One hit from [`search_references`], tagged with which signal matched.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub ref_id: String,
    pub label: String,
    /// Where the match was found: `"description"` or a package file path.
    pub location: String,
    /// `"semantic"`, `"description"`, or `"package"`.
    pub matched: &'static str,
    pub snippet: String,
    /// Cosine score for semantic hits; `None` for literal hits.
    pub score: Option<f32>,
}

/// One stored reference-documentation entry (a plain `.txt`/`.md` file), keyed
/// by a content hash. Independent of reference forms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReferenceDocInfo {
    pub doc_id: String,
    pub profile: String,
    pub label: String,
    pub content: String,
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::{ReferenceDocInfo, ReferenceInfo, SearchHit};
    use blueprint::reference_db::{
        EMBEDDING_MODEL_VERSION, SCHEMA_SQL, blob_to_vec, vec_to_blob,
    };
    use blueprint::semantic::SemanticMatcher;
    use rusqlite::{Connection, params};

    /// Cosine threshold for the semantic (RAG) signal over descriptions. Lower
    /// than the strict cross-lingual match threshold because a short query is
    /// compared against a longer description.
    const SEMANTIC_THRESHOLD: f32 = 0.4;
    /// Render scale for on-demand page images (matches the pipeline default).
    const RENDER_SCALE: f32 = 1.5;

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    // ── Write path ───────────────────────────────────────────────────────────

    /// Insert or replace one reference and its child rows.
    ///
    /// `pdfs` is `(page_count, pdf_bytes)`; `package_files` is `(path, content)`
    /// from the unzipped AEM package. Child rows for `ref_id` are cleared first
    /// so re-adding a form never leaves stale pages/files behind.
    #[allow(clippy::too_many_arguments)]
    pub fn add_reference(
        profile: &str,
        ref_id: &str,
        label: &str,
        description: &str,
        embedding: &[f32],
        pdfs: &[(u32, Vec<u8>)],
        package_files: &[(String, String)],
    ) -> Result<(), String> {
        let mut conn = crate::db::open().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR REPLACE INTO references_
             (ref_id, profile, label, description, description_embedding, model_version, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                ref_id,
                profile,
                label,
                description,
                vec_to_blob(embedding),
                EMBEDDING_MODEL_VERSION,
                now()
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM reference_pdfs WHERE ref_id = ?1", [ref_id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM reference_files WHERE ref_id = ?1", [ref_id])
            .map_err(|e| e.to_string())?;
        for (i, (page_count, bytes)) in pdfs.iter().enumerate() {
            tx.execute(
                "INSERT INTO reference_pdfs (ref_id, pdf_index, page_count, pdf_bytes)
                 VALUES (?1, ?2, ?3, ?4)",
                params![ref_id, i as i64, *page_count as i64, bytes],
            )
            .map_err(|e| e.to_string())?;
        }
        for (path, content) in package_files {
            tx.execute(
                "INSERT INTO reference_files (ref_id, path, content) VALUES (?1, ?2, ?3)",
                params![ref_id, path, content],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_reference(ref_id: &str) {
        let Ok(conn) = crate::db::open() else { return };
        let _ = conn.execute("DELETE FROM reference_pdfs WHERE ref_id = ?1", [ref_id]);
        let _ = conn.execute("DELETE FROM reference_files WHERE ref_id = ?1", [ref_id]);
        let _ = conn.execute("DELETE FROM references_ WHERE ref_id = ?1", [ref_id]);
    }

    // ── Reference documentation (plain txt/md) ──────────────────────────────────

    /// `doc_id` for a documentation file — a content hash (the same mechanism
    /// references and sessions use), so identical content deduplicates.
    pub fn compute_doc_id(content: &str) -> String {
        crate::db::document_hash(&[("doc".to_string(), content.as_bytes().to_vec())])
    }

    /// Insert or replace one documentation entry.
    pub fn add_doc(profile: &str, doc_id: &str, label: &str, content: &str) -> Result<(), String> {
        let conn = crate::db::open().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO reference_docs (doc_id, profile, label, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![doc_id, profile, label, content, now()],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn list_docs(profile: &str) -> Vec<ReferenceDocInfo> {
        let Ok(conn) = crate::db::open() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT doc_id, profile, label, content FROM reference_docs
             WHERE profile = ?1 ORDER BY created_at DESC",
        ) else {
            return Vec::new();
        };
        stmt.query_map([profile], |row| {
            Ok(ReferenceDocInfo {
                doc_id: row.get(0)?,
                profile: row.get(1)?,
                label: row.get(2)?,
                content: row.get(3)?,
            })
        })
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    pub fn delete_doc(doc_id: &str) {
        let Ok(conn) = crate::db::open() else { return };
        let _ = conn.execute("DELETE FROM reference_docs WHERE doc_id = ?1", [doc_id]);
    }

    // ── Read / list ────────────────────────────────────────────────────────────

    pub fn list_references(profile: &str) -> Vec<ReferenceInfo> {
        let Ok(conn) = crate::db::open() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT ref_id, profile, label, description,
                    (SELECT COUNT(*) FROM reference_pdfs p WHERE p.ref_id = r.ref_id)
             FROM references_ r WHERE profile = ?1 ORDER BY created_at DESC",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map([profile], |row| {
            Ok(ReferenceInfo {
                ref_id: row.get(0)?,
                profile: row.get(1)?,
                label: row.get(2)?,
                description: row.get(3)?,
                pdf_count: row.get::<_, i64>(4)? as usize,
                files: Vec::new(),
            })
        });
        let mut out: Vec<ReferenceInfo> = match rows {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(_) => return Vec::new(),
        };
        for info in &mut out {
            info.files = list_reference_files(&info.ref_id);
        }
        out
    }

    pub fn count(profile: &str) -> usize {
        let Ok(conn) = crate::db::open() else { return 0 };
        conn.query_row(
            "SELECT COUNT(*) FROM references_ WHERE profile = ?1",
            [profile],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n as usize)
        .unwrap_or(0)
    }

    pub fn list_reference_files(ref_id: &str) -> Vec<String> {
        let Ok(conn) = crate::db::open() else {
            return Vec::new();
        };
        let Ok(mut stmt) =
            conn.prepare("SELECT path FROM reference_files WHERE ref_id = ?1 ORDER BY path")
        else {
            return Vec::new();
        };
        stmt.query_map([ref_id], |r| r.get::<_, String>(0))
            .map(|it| it.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Read a slice (by line) of a reference's description (`path ==
    /// "description"`) or one of its package files.
    pub fn read_reference_file(
        ref_id: &str,
        path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<String, String> {
        let conn = crate::db::open().map_err(|e| e.to_string())?;
        let content: String = if path == "description" {
            conn.query_row(
                "SELECT description FROM references_ WHERE ref_id = ?1",
                [ref_id],
                |r| r.get(0),
            )
            .map_err(|_| format!("Unknown ref_id: {ref_id}"))?
        } else {
            conn.query_row(
                "SELECT content FROM reference_files WHERE ref_id = ?1 AND path = ?2",
                params![ref_id, path],
                |r| r.get(0),
            )
            .map_err(|_| format!("No such file {path:?} in reference {ref_id}"))?
        };
        if offset == 0 && limit == 0 {
            return Ok(content);
        }
        let sliced: String = content
            .lines()
            .skip(offset)
            .take(if limit == 0 { usize::MAX } else { limit })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(sliced)
    }

    // ── Hybrid search ──────────────────────────────────────────────────────────

    /// Search the profile's references three ways and merge: (a) semantic (RAG)
    /// over description embeddings, (b) literal substring in descriptions, (c)
    /// literal substring in the AEM package text. De-duplicated by
    /// `(ref_id, location)`, capped at `top_k` per signal.
    pub fn search_references(
        profile: &str,
        query: &str,
        matcher: &SemanticMatcher,
        top_k: usize,
    ) -> Vec<SearchHit> {
        let Ok(conn) = crate::db::open() else {
            return Vec::new();
        };
        let needle = query.to_lowercase();
        let mut hits: Vec<SearchHit> = Vec::new();

        // Load (ref_id, label, description, embedding) for the profile once.
        struct Row {
            ref_id: String,
            label: String,
            description: String,
            embedding: Vec<f32>,
        }
        let rows: Vec<Row> = {
            let Ok(mut stmt) = conn.prepare(
                "SELECT ref_id, label, description, description_embedding
                 FROM references_ WHERE profile = ?1",
            ) else {
                return Vec::new();
            };
            stmt.query_map([profile], |r| {
                Ok(Row {
                    ref_id: r.get(0)?,
                    label: r.get(1)?,
                    description: r.get(2)?,
                    embedding: blob_to_vec(&r.get::<_, Vec<u8>>(3)?),
                })
            })
            .map(|it| it.filter_map(Result::ok).collect())
            .unwrap_or_default()
        };

        // (a) Semantic / RAG over descriptions.
        if let Ok(q) = matcher.embed(query) {
            let mut scored: Vec<(f32, &Row)> = rows
                .iter()
                .filter(|row| row.embedding.len() == q.len())
                .map(|row| (SemanticMatcher::cosine_similarity(&q, &row.embedding), row))
                .filter(|(s, _)| *s >= SEMANTIC_THRESHOLD)
                .collect();
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            for (score, row) in scored.into_iter().take(top_k) {
                hits.push(SearchHit {
                    ref_id: row.ref_id.clone(),
                    label: row.label.clone(),
                    location: "description".to_string(),
                    matched: "semantic",
                    snippet: snippet(&row.description, &needle),
                    score: Some(score),
                });
            }
        }

        // (b) Literal substring in descriptions.
        for row in rows.iter().filter(|r| r.description.to_lowercase().contains(&needle)) {
            push_dedup(
                &mut hits,
                SearchHit {
                    ref_id: row.ref_id.clone(),
                    label: row.label.clone(),
                    location: "description".to_string(),
                    matched: "description",
                    snippet: snippet(&row.description, &needle),
                    score: None,
                },
            );
        }

        // (c) Literal substring in the AEM package text.
        if let Ok(mut stmt) = conn.prepare(
            "SELECT rf.ref_id, r.label, rf.path, rf.content
             FROM reference_files rf JOIN references_ r ON r.ref_id = rf.ref_id
             WHERE r.profile = ?1",
        ) {
            let pkg = stmt
                .query_map([profile], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map(|it| it.filter_map(Result::ok).collect::<Vec<_>>())
                .unwrap_or_default();
            let mut pkg_hits = 0usize;
            for (ref_id, label, path, content) in pkg {
                if pkg_hits >= top_k {
                    break;
                }
                if content.to_lowercase().contains(&needle) {
                    pkg_hits += 1;
                    push_dedup(
                        &mut hits,
                        SearchHit {
                            ref_id,
                            label,
                            location: path,
                            matched: "package",
                            snippet: snippet(&content, &needle),
                            score: None,
                        },
                    );
                }
            }
        }

        hits
    }

    fn push_dedup(hits: &mut Vec<SearchHit>, hit: SearchHit) {
        if !hits
            .iter()
            .any(|h| h.ref_id == hit.ref_id && h.location == hit.location)
        {
            hits.push(hit);
        }
    }

    /// A short one-line snippet around the first occurrence of `needle`
    /// (lowercased) in `text`, or the first ~120 chars if not found.
    fn snippet(text: &str, needle: &str) -> String {
        let lower = text.to_lowercase();
        let start = lower.find(needle).map(|i| i.saturating_sub(40)).unwrap_or(0);
        let slice: String = text
            .chars()
            .skip(start)
            .take(160)
            .collect::<String>()
            .replace('\n', " ");
        slice.trim().to_string()
    }

    // ── Page rendering ─────────────────────────────────────────────────────────

    /// Render a source-PDF page of a reference to JPEG bytes. `page` is
    /// currently advisory — the default form state is rendered.
    pub fn render_reference_page(
        ref_id: &str,
        pdf_index: usize,
        _page: usize,
    ) -> Result<Vec<u8>, String> {
        let conn = crate::db::open().map_err(|e| e.to_string())?;
        let bytes: Vec<u8> = conn
            .query_row(
                "SELECT pdf_bytes FROM reference_pdfs WHERE ref_id = ?1 AND pdf_index = ?2",
                params![ref_id, pdf_index as i64],
                |r| r.get(0),
            )
            .map_err(|_| format!("No PDF {pdf_index} for reference {ref_id}"))?;

        let mut bp = blueprint::Blueprint::from_pdf_bytes(&bytes)
            .map_err(|e| format!("PDF parse failed: {e}"))?;
        let states = bp.states().map_err(|e| format!("State discovery failed: {e}"))?;
        let state = states
            .iter()
            .next()
            .ok_or_else(|| "No renderable state in PDF".to_string())?;
        let img = state
            .render_plain(RENDER_SCALE)
            .map_err(|e| format!("Render failed: {e}"))?;
        crate::pipeline::encode_rgba_to_jpeg(&img, 82).map_err(|e| format!("Encode failed: {e}"))
    }

    // ── Import / export ──────────────────────────────────────────────────────────

    /// Import a reference dataset file, stamping `profile` onto the rows. Only
    /// the reference tables are written. Returns `(references, docs)` imported.
    /// If imported rows used a different embedding model, their embeddings are
    /// recomputed.
    pub fn import_reference_db(path: &str, profile: &str) -> Result<(usize, usize), String> {
        let conn = crate::db::open().map_err(|e| e.to_string())?;
        conn.execute("ATTACH DATABASE ?1 AS imp", [path])
            .map_err(|e| format!("Cannot open dataset: {e}"))?;

        let result = (|| -> Result<(usize, usize), String> {
            conn.execute(
                "INSERT OR REPLACE INTO references_
                 (ref_id, profile, label, description, description_embedding, model_version, created_at)
                 SELECT ref_id, ?1, label, description, description_embedding, model_version, created_at
                 FROM imp.references_",
                [profile],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT OR REPLACE INTO reference_pdfs SELECT * FROM imp.reference_pdfs",
                [],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT OR REPLACE INTO reference_files SELECT * FROM imp.reference_files",
                [],
            )
            .map_err(|e| e.to_string())?;
            // Backward-compat: older datasets lack reference_docs; ensure the
            // attached source has it (empty) before selecting from it.
            conn.execute(
                "CREATE TABLE IF NOT EXISTS imp.reference_docs (
                    doc_id TEXT PRIMARY KEY, profile TEXT NOT NULL, label TEXT NOT NULL,
                    content TEXT NOT NULL, created_at TEXT NOT NULL
                )",
                [],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT OR REPLACE INTO reference_docs (doc_id, profile, label, content, created_at)
                 SELECT doc_id, ?1, label, content, created_at FROM imp.reference_docs",
                [profile],
            )
            .map_err(|e| e.to_string())?;
            let refs: i64 = conn
                .query_row("SELECT COUNT(*) FROM imp.references_", [], |r| r.get(0))
                .unwrap_or(0);
            let docs: i64 = conn
                .query_row("SELECT COUNT(*) FROM imp.reference_docs", [], |r| r.get(0))
                .unwrap_or(0);
            Ok((refs as usize, docs as usize))
        })();

        let _ = conn.execute("DETACH DATABASE imp", []);
        let counts = result?;
        recompute_stale_embeddings(profile)?;
        Ok(counts)
    }

    /// Recompute embeddings for any reference whose stored `model_version`
    /// differs from the current model, so semantic search compares like with like.
    fn recompute_stale_embeddings(profile: &str) -> Result<(), String> {
        let conn = crate::db::open().map_err(|e| e.to_string())?;
        let stale: Vec<(String, String)> = {
            let Ok(mut stmt) = conn.prepare(
                "SELECT ref_id, description FROM references_
                 WHERE profile = ?1 AND model_version != ?2",
            ) else {
                return Ok(());
            };
            stmt.query_map(params![profile, EMBEDDING_MODEL_VERSION], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map(|it| it.filter_map(Result::ok).collect())
            .unwrap_or_default()
        };
        if stale.is_empty() {
            return Ok(());
        }
        let matcher = SemanticMatcher::new().map_err(|e| e.to_string())?;
        for (ref_id, description) in stale {
            if let Ok(emb) = matcher.embed(&description) {
                let _ = conn.execute(
                    "UPDATE references_ SET description_embedding = ?2, model_version = ?3
                     WHERE ref_id = ?1",
                    params![ref_id, vec_to_blob(&emb), EMBEDDING_MODEL_VERSION],
                );
            }
        }
        Ok(())
    }

    /// Export references (all, or one profile) to a fresh profile-agnostic
    /// SQLite file containing only the reference tables. Returns
    /// `(references, docs)` exported.
    pub fn export_references(
        out_path: &str,
        profile: Option<&str>,
    ) -> Result<(usize, usize), String> {
        // Create the destination with the shared schema first.
        {
            let out = Connection::open(out_path)
                .map_err(|e| format!("Cannot create export file: {e}"))?;
            out.execute_batch(SCHEMA_SQL).map_err(|e| e.to_string())?;
        }

        let conn = crate::db::open().map_err(|e| e.to_string())?;
        conn.execute("ATTACH DATABASE ?1 AS exp", [out_path])
            .map_err(|e| format!("Cannot attach export file: {e}"))?;

        let result = (|| -> Result<(usize, usize), String> {
            // profile is blanked to '' for portability; bound at import.
            let filter = match profile {
                Some(_) => "WHERE profile = ?1",
                None => "",
            };
            let refs_sql = format!(
                "INSERT INTO exp.references_
                 (ref_id, profile, label, description, description_embedding, model_version, created_at)
                 SELECT ref_id, '', label, description, description_embedding, model_version, created_at
                 FROM references_ {filter}"
            );
            let pdfs_sql = format!(
                "INSERT INTO exp.reference_pdfs SELECT * FROM reference_pdfs
                 WHERE ref_id IN (SELECT ref_id FROM references_ {filter})"
            );
            let files_sql = format!(
                "INSERT INTO exp.reference_files SELECT * FROM reference_files
                 WHERE ref_id IN (SELECT ref_id FROM references_ {filter})"
            );
            // Documentation has its own `profile` column, so it filters directly.
            let docs_sql = format!(
                "INSERT INTO exp.reference_docs (doc_id, profile, label, content, created_at)
                 SELECT doc_id, '', label, content, created_at FROM reference_docs {filter}"
            );
            match profile {
                Some(p) => {
                    conn.execute(&refs_sql, [p]).map_err(|e| e.to_string())?;
                    conn.execute(&pdfs_sql, [p]).map_err(|e| e.to_string())?;
                    conn.execute(&files_sql, [p]).map_err(|e| e.to_string())?;
                    conn.execute(&docs_sql, [p]).map_err(|e| e.to_string())?;
                }
                None => {
                    conn.execute(&refs_sql, []).map_err(|e| e.to_string())?;
                    conn.execute(&pdfs_sql, []).map_err(|e| e.to_string())?;
                    conn.execute(&files_sql, []).map_err(|e| e.to_string())?;
                    conn.execute(&docs_sql, []).map_err(|e| e.to_string())?;
                }
            }
            let refs: i64 = conn
                .query_row("SELECT COUNT(*) FROM exp.references_", [], |r| r.get(0))
                .unwrap_or(0);
            let docs: i64 = conn
                .query_row("SELECT COUNT(*) FROM exp.reference_docs", [], |r| r.get(0))
                .unwrap_or(0);
            Ok((refs as usize, docs as usize))
        })();

        let _ = conn.execute("DETACH DATABASE exp", []);
        result
    }

    // ── Build-from-uploads helpers (used by the Settings add flow) ──────────────

    /// `ref_id` for the input PDF(s) — the same order-independent content hash
    /// sessions use. Filenames are ignored, so the id depends only on contents
    /// and is stable regardless of selection order. A single-PDF call yields the
    /// same value as before, preserving cross-crate (`reference-builder`) ids.
    pub fn compute_ref_id(pdfs: &[(String, Vec<u8>)]) -> String {
        crate::db::document_hash(pdfs)
    }

    /// Unzip an AEM package (FileVault ZIP) into `(path, content)` text rows.
    /// Binary / non-UTF-8 entries and directories are skipped.
    pub fn unzip_package(zip_bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
        use std::io::Read;
        let reader = std::io::Cursor::new(zip_bytes);
        let mut zip = zip::ZipArchive::new(reader).map_err(|e| format!("Bad ZIP: {e}"))?;
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

    /// Embed a description with a freshly-loaded matcher (used by the add flow).
    pub fn embed_description(text: &str) -> Result<Vec<f32>, String> {
        let matcher = SemanticMatcher::new().map_err(|e| e.to_string())?;
        matcher.embed(text).map_err(|e| e.to_string())
    }

    /// Number of pages/states discoverable in a PDF (best effort, for metadata).
    pub fn pdf_state_count(pdf_bytes: &[u8]) -> u32 {
        blueprint::Blueprint::from_pdf_bytes(pdf_bytes)
            .ok()
            .and_then(|mut bp| bp.states().ok().map(|s| s.iter().count() as u32))
            .unwrap_or(0)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // The store is keyed to the on-disk history.db; these tests exercise the
        // pure helpers that don't touch it.
        #[test]
        fn ref_id_is_content_hash() {
            let a = compute_ref_id(&[("f.pdf".into(), b"hello".to_vec())]);
            let b = compute_ref_id(&[("f.pdf".into(), b"hello".to_vec())]);
            let c = compute_ref_id(&[("f.pdf".into(), b"world".to_vec())]);
            assert_eq!(a, b);
            assert_ne!(a, c);
            assert!(!a.is_empty());
        }

        #[test]
        fn ref_id_is_order_independent() {
            let a = compute_ref_id(&[
                ("a.pdf".into(), b"one".to_vec()),
                ("b.pdf".into(), b"two".to_vec()),
            ]);
            let b = compute_ref_id(&[
                ("b.pdf".into(), b"two".to_vec()),
                ("a.pdf".into(), b"one".to_vec()),
            ]);
            assert_eq!(a, b);
        }

        /// Pins the exact `ref_id` for a fixed single-PDF input. The
        /// `reference-builder` crate has the identical assertion, so app-added
        /// and builder-built references for the same form share an id. If either
        /// side's hashing changes, one of these tests fails.
        #[test]
        fn ref_id_matches_canonical_hash() {
            assert_eq!(
                compute_ref_id(&[("input.pdf".into(), b"reference-id-test".to_vec())]),
                "c0314ff86abc2dcf5cfd090203e5d24eb456bbff56e290ead6cd302fd16af90a"
            );
        }

        #[test]
        fn snippet_centres_on_match() {
            let s = snippet("Account holder\nName\nNationality", "nationality");
            assert!(s.to_lowercase().contains("nationality"));
            assert!(!s.contains('\n'));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use imp::*;

#[cfg(target_arch = "wasm32")]
mod stub {
    use super::{ReferenceDocInfo, ReferenceInfo, SearchHit};

    pub fn list_references(_profile: &str) -> Vec<ReferenceInfo> {
        Vec::new()
    }
    pub fn compute_doc_id(_content: &str) -> String {
        String::new()
    }
    pub fn add_doc(_profile: &str, _doc_id: &str, _label: &str, _content: &str) -> Result<(), String> {
        Err("References are only available in the desktop app.".to_string())
    }
    pub fn list_docs(_profile: &str) -> Vec<ReferenceDocInfo> {
        Vec::new()
    }
    pub fn delete_doc(_doc_id: &str) {}
    pub fn count(_profile: &str) -> usize {
        0
    }
    pub fn delete_reference(_ref_id: &str) {}
    pub fn list_reference_files(_ref_id: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn read_reference_file(
        _ref_id: &str,
        _path: &str,
        _offset: usize,
        _limit: usize,
    ) -> Result<String, String> {
        Err("References are only available in the desktop app.".to_string())
    }
    pub fn render_reference_page(
        _ref_id: &str,
        _pdf_index: usize,
        _page: usize,
    ) -> Result<Vec<u8>, String> {
        Err("References are only available in the desktop app.".to_string())
    }
    pub fn import_reference_db(_path: &str, _profile: &str) -> Result<(usize, usize), String> {
        Err("References are only available in the desktop app.".to_string())
    }
    pub fn export_references(
        _out_path: &str,
        _profile: Option<&str>,
    ) -> Result<(usize, usize), String> {
        Err("References are only available in the desktop app.".to_string())
    }
    pub fn compute_ref_id(_pdfs: &[(String, Vec<u8>)]) -> String {
        String::new()
    }
    pub fn unzip_package(_zip_bytes: &[u8]) -> Result<Vec<(String, String)>, String> {
        Err("References are only available in the desktop app.".to_string())
    }
    pub fn embed_description(_text: &str) -> Result<Vec<f32>, String> {
        Err("References are only available in the desktop app.".to_string())
    }
    pub fn pdf_state_count(_pdf_bytes: &[u8]) -> u32 {
        0
    }
    #[allow(clippy::too_many_arguments)]
    pub fn add_reference(
        _profile: &str,
        _ref_id: &str,
        _label: &str,
        _description: &str,
        _embedding: &[f32],
        _pdfs: &[(u32, Vec<u8>)],
        _package_files: &[(String, String)],
    ) -> Result<(), String> {
        Err("References are only available in the desktop app.".to_string())
    }
}

#[cfg(target_arch = "wasm32")]
pub use stub::*;
