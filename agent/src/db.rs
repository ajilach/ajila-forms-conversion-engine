//! Local SQLite store for application settings and edit history.
//!
//! The data lives in `<config_dir>/blueprint/history.db`.
//!
//! Schema:
//! - `settings(key TEXT PRIMARY KEY, value TEXT)` — key/value store (the whole
//!   `AppSettings` is stored as JSON under the `app` key).
//! - `documents(doc_hash TEXT PRIMARY KEY, label TEXT, created_at TEXT)` — one
//!   row per uploaded document set, keyed by an order-independent content hash.
//! - `sessions(session_id TEXT PRIMARY KEY, doc_hash TEXT, profile TEXT,
//!   label TEXT, created_at TEXT)` — one row per pipeline run / continued edit.
//! - `edits(id INTEGER PRIMARY KEY, session_id TEXT, seq INTEGER, created_at
//!   TEXT, action_label TEXT, structure_json TEXT)` — one snapshot per edit.

/// Metadata about a previous editing session, for the "continue editing" list.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub session_id: String,
    pub label: String,
    pub profile: Option<String>,
    /// RFC3339 timestamp of when the session was created.
    pub created_at: String,
    /// Number of edits recorded in the session (including the initial snapshot).
    pub edit_count: usize,
}

/// A single entry in a session's edit history, for the history sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct EditInfo {
    pub seq: usize,
    /// RFC3339 timestamp of when the edit was recorded.
    pub created_at: String,
    pub action_label: String,
}

/// Format an RFC3339 timestamp into a compact `YYYY-MM-DD HH:MM` form.
pub fn format_timestamp(ts: &str) -> String {
    if ts.len() >= 16 {
        ts[..16].replace('T', " ")
    } else {
        ts.to_string()
    }
}

mod imp {
    use super::{EditInfo, SessionInfo};
    use rusqlite::{Connection, OptionalExtension};
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;

    fn db_path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        });
        base.join("blueprint").join("history.db")
    }

    /// Open the database connection, creating the file and schema if needed.
    ///
    /// Public so the reference store ([`crate::references`]) can share the same
    /// single `history.db` connection rather than opening a second database.
    pub fn open() -> rusqlite::Result<Connection> {
        let path = db_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        ensure_schema(&conn)?;
        Ok(conn)
    }

    fn ensure_schema(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS documents (
                doc_hash   TEXT PRIMARY KEY,
                label      TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                doc_hash   TEXT NOT NULL,
                profile    TEXT,
                label      TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS edits (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id     TEXT NOT NULL,
                seq            INTEGER NOT NULL,
                created_at     TEXT NOT NULL,
                action_label   TEXT NOT NULL,
                structure_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_edits_session ON edits(session_id, seq);
            CREATE INDEX IF NOT EXISTS idx_sessions_doc ON sessions(doc_hash, created_at);",
        )?;
        // Reference-form tables (shared schema with the `reference-builder`
        // crate, so dataset exports import without drift). Stored in the same
        // `history.db`; only these tables are written by reference import/export.
        conn.execute_batch(blueprint::reference_db::SCHEMA_SQL)?;
        Ok(())
    }

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    // ── Settings ────────────────────────────────────────────────────────────

    pub fn get_setting(key: &str) -> Option<String> {
        let conn = open().ok()?;
        conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .ok()
        .flatten()
    }

    pub fn set_setting(key: &str, value: &str) {
        if let Ok(conn) = open() {
            let _ = conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            );
        }
    }

    // ── Documents & sessions ────────────────────────────────────────────────

    pub fn document_hash(files: &[(String, Vec<u8>)]) -> String {
        // Order-independent: hash each file, sort the digests, hash the result.
        let mut digests: Vec<String> = files.iter().map(|(_, bytes)| sha256_hex(bytes)).collect();
        digests.sort();
        sha256_hex(digests.join("").as_bytes())
    }

    pub fn upsert_document(doc_hash: &str, label: &str) {
        if let Ok(conn) = open() {
            let _ = conn.execute(
                "INSERT INTO documents (doc_hash, label, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(doc_hash) DO UPDATE SET label = excluded.label",
                rusqlite::params![doc_hash, label, now()],
            );
        }
    }

    pub fn create_session(doc_hash: &str, profile: Option<&str>, label: &str) -> Option<String> {
        let conn = open().ok()?;
        let session_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO sessions (session_id, doc_hash, profile, label, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![session_id, doc_hash, profile, label, now()],
        )
        .ok()?;
        Some(session_id)
    }

    /// Look up the conversion profile stored for a session, if any.
    pub fn session_profile(session_id: &str) -> Option<String> {
        let conn = open().ok()?;
        conn.query_row(
            "SELECT profile FROM sessions WHERE session_id = ?1",
            [session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten()
    }

    pub fn list_sessions(doc_hash: &str) -> Vec<SessionInfo> {
        let Ok(conn) = open() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT s.session_id, s.label, s.profile, s.created_at,
                    (SELECT COUNT(*) FROM edits e WHERE e.session_id = s.session_id)
             FROM sessions s
             WHERE s.doc_hash = ?1
             ORDER BY s.created_at DESC",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map([doc_hash], |row| {
            Ok(SessionInfo {
                session_id: row.get(0)?,
                label: row.get(1)?,
                profile: row.get(2)?,
                created_at: row.get(3)?,
                edit_count: row.get::<_, i64>(4)? as usize,
            })
        });
        match rows {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// List every editing session across all documents, newest first. Used by
    /// the "load previous session" browser shown before an upload.
    pub fn list_all_sessions() -> Vec<SessionInfo> {
        let Ok(conn) = open() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT s.session_id, s.label, s.profile, s.created_at,
                    (SELECT COUNT(*) FROM edits e WHERE e.session_id = s.session_id)
             FROM sessions s
             ORDER BY s.created_at DESC",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map([], |row| {
            Ok(SessionInfo {
                session_id: row.get(0)?,
                label: row.get(1)?,
                profile: row.get(2)?,
                created_at: row.get(3)?,
                edit_count: row.get::<_, i64>(4)? as usize,
            })
        });
        match rows {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Permanently delete an editing session and all of its edits.
    pub fn delete_session(session_id: &str) {
        let Ok(conn) = open() else {
            return;
        };
        let _ = conn.execute("DELETE FROM edits WHERE session_id = ?1", [session_id]);
        let _ = conn.execute("DELETE FROM sessions WHERE session_id = ?1", [session_id]);
    }

    // ── Edits ───────────────────────────────────────────────────────────────

    fn next_seq(conn: &Connection, session_id: &str) -> usize {
        conn.query_row(
            "SELECT COALESCE(MAX(seq) + 1, 0) FROM edits WHERE session_id = ?1",
            [session_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n as usize)
        .unwrap_or(0)
    }

    fn insert_edit_conn(
        conn: &Connection,
        session_id: &str,
        action_label: &str,
        structure_json: &str,
    ) -> Option<usize> {
        let seq = next_seq(conn, session_id);
        conn.execute(
            "INSERT INTO edits (session_id, seq, created_at, action_label, structure_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![session_id, seq as i64, now(), action_label, structure_json],
        )
        .ok()?;
        Some(seq)
    }

    fn record_edit_conn(
        conn: &Connection,
        session_id: &str,
        after_seq: usize,
        action_label: &str,
        structure_json: &str,
    ) -> Option<usize> {
        conn.execute(
            "DELETE FROM edits WHERE session_id = ?1 AND seq > ?2",
            rusqlite::params![session_id, after_seq as i64],
        )
        .ok()?;
        insert_edit_conn(conn, session_id, action_label, structure_json)
    }

    fn snapshot_at_conn(conn: &Connection, session_id: &str, seq: usize) -> Option<String> {
        conn.query_row(
            "SELECT structure_json FROM edits WHERE session_id = ?1 AND seq = ?2",
            rusqlite::params![session_id, seq as i64],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    fn latest_seq_conn(conn: &Connection, session_id: &str) -> Option<usize> {
        conn.query_row(
            "SELECT MAX(seq) FROM edits WHERE session_id = ?1",
            [session_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()
        .ok()
        .flatten()
        .flatten()
        .map(|n| n as usize)
    }

    fn list_edits_conn(conn: &Connection, session_id: &str) -> Vec<EditInfo> {
        let Ok(mut stmt) = conn.prepare(
            "SELECT seq, created_at, action_label FROM edits
             WHERE session_id = ?1 ORDER BY seq ASC",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map([session_id], |row| {
            Ok(EditInfo {
                seq: row.get::<_, i64>(0)? as usize,
                created_at: row.get(1)?,
                action_label: row.get(2)?,
            })
        });
        match rows {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Append a snapshot at the next sequence number. Returns the new seq.
    pub fn insert_edit(
        session_id: &str,
        action_label: &str,
        structure_json: &str,
    ) -> Option<usize> {
        let conn = open().ok()?;
        insert_edit_conn(&conn, session_id, action_label, structure_json)
    }

    /// Record an edit after `after_seq`, discarding any redo tail (seq > after_seq).
    /// Returns the new seq.
    pub fn record_edit(
        session_id: &str,
        after_seq: usize,
        action_label: &str,
        structure_json: &str,
    ) -> Option<usize> {
        let conn = open().ok()?;
        record_edit_conn(&conn, session_id, after_seq, action_label, structure_json)
    }

    pub fn snapshot_at(session_id: &str, seq: usize) -> Option<String> {
        let conn = open().ok()?;
        snapshot_at_conn(&conn, session_id, seq)
    }

    pub fn latest_seq(session_id: &str) -> Option<usize> {
        let conn = open().ok()?;
        latest_seq_conn(&conn, session_id)
    }

    pub fn list_edits(session_id: &str) -> Vec<EditInfo> {
        let Ok(conn) = open() else {
            return Vec::new();
        };
        list_edits_conn(&conn, session_id)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn mem() -> Connection {
            let conn = Connection::open_in_memory().unwrap();
            ensure_schema(&conn).unwrap();
            conn
        }

        #[test]
        fn document_hash_is_order_independent() {
            let a = ("a.xml".to_string(), b"alpha".to_vec());
            let b = ("b.xml".to_string(), b"beta".to_vec());
            let forward = document_hash(&[a.clone(), b.clone()]);
            let reversed = document_hash(&[b, a]);
            assert_eq!(forward, reversed);
        }

        #[test]
        fn document_hash_differs_on_content() {
            let one = document_hash(&[("f".into(), b"one".to_vec())]);
            let two = document_hash(&[("f".into(), b"two".to_vec())]);
            assert_ne!(one, two);
        }

        #[test]
        fn edits_append_with_increasing_seq() {
            let conn = mem();
            assert_eq!(insert_edit_conn(&conn, "s", "first", "{}"), Some(0));
            assert_eq!(insert_edit_conn(&conn, "s", "second", "{}"), Some(1));
            assert_eq!(latest_seq_conn(&conn, "s"), Some(1));
        }

        #[test]
        fn snapshot_round_trips() {
            let conn = mem();
            insert_edit_conn(&conn, "s", "first", "{\"v\":1}");
            insert_edit_conn(&conn, "s", "second", "{\"v\":2}");
            assert_eq!(
                snapshot_at_conn(&conn, "s", 0).as_deref(),
                Some("{\"v\":1}")
            );
            assert_eq!(
                snapshot_at_conn(&conn, "s", 1).as_deref(),
                Some("{\"v\":2}")
            );
        }

        #[test]
        fn record_edit_truncates_redo_tail() {
            let conn = mem();
            insert_edit_conn(&conn, "s", "a", "0");
            insert_edit_conn(&conn, "s", "b", "1");
            insert_edit_conn(&conn, "s", "c", "2");
            // User undid back to seq 0, then makes a new edit: seq 1 and 2 dropped.
            assert_eq!(record_edit_conn(&conn, "s", 0, "d", "3"), Some(1));
            assert_eq!(latest_seq_conn(&conn, "s"), Some(1));
            assert_eq!(snapshot_at_conn(&conn, "s", 1).as_deref(), Some("3"));
            assert_eq!(snapshot_at_conn(&conn, "s", 2), None);
        }

        #[test]
        fn list_edits_is_ordered_by_seq() {
            let conn = mem();
            insert_edit_conn(&conn, "s", "a", "0");
            insert_edit_conn(&conn, "s", "b", "1");
            let edits = list_edits_conn(&conn, "s");
            assert_eq!(edits.len(), 2);
            assert_eq!(edits[0].seq, 0);
            assert_eq!(edits[0].action_label, "a");
            assert_eq!(edits[1].seq, 1);
            assert_eq!(edits[1].action_label, "b");
        }

        #[test]
        fn sessions_listed_newest_first() {
            let conn = mem();
            // created_at is generated via now(); insert with explicit ordering.
            conn.execute(
                "INSERT INTO sessions (session_id, doc_hash, profile, label, created_at)
                 VALUES ('old', 'h', NULL, 'old', '2020-01-01T00:00:00Z'),
                        ('new', 'h', NULL, 'new', '2024-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT session_id FROM sessions WHERE doc_hash = ?1 ORDER BY created_at DESC",
                )
                .unwrap();
            let ids: Vec<String> = stmt
                .query_map(["h"], |r| r.get::<_, String>(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert_eq!(ids, vec!["new".to_string(), "old".to_string()]);
        }
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

pub use imp::*;
