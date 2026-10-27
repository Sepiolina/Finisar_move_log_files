use crate::error::{ArchiveError, Result};
use rusqlite::Connection;

pub const SCHEMA_SQL: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS scan_session (
    id INTEGER PRIMARY KEY,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    status TEXT CHECK(status IN ('running','success','failed')) NOT NULL,
    query_from_date TEXT NOT NULL,
    query_to_date TEXT NOT NULL,
    total_barcodes INTEGER DEFAULT 0,
    total_files_moved INTEGER DEFAULT 0,
    total_errors INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS session_barcode (
    id INTEGER PRIMARY KEY,
    scan_session_id INTEGER NOT NULL REFERENCES scan_session(id),
    barcode TEXT NOT NULL,
    last_update TEXT NOT NULL,
    status TEXT CHECK(status IN ('matched','no_files_found','error')) NOT NULL,
    UNIQUE(barcode, last_update)
);

CREATE TABLE IF NOT EXISTS file_storage_volume (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    volume_name TEXT NOT NULL,      -- e.g. "2024-07"
    file_size INTEGER NOT NULL,
    head_commit_id TEXT,            -- MOCHI commit id (hex) of the archive head after the last append
    head_seq INTEGER,               -- MOCHI commit sequence of that head
    entry_count INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL        -- bump on every append
);

CREATE TABLE IF NOT EXISTS scanned_log_entry (
    id INTEGER PRIMARY KEY,
    scan_session_id INTEGER NOT NULL REFERENCES scan_session(id),
    barcode TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    source_subfolder TEXT NOT NULL,   -- BURNIN/ESS/JTAG/STM/Xray
    content_hash TEXT,                -- BLAKE3 (hex) of the file's bytes, as stored in MOCHI
    commit_seq INTEGER,               -- MOCHI commit sequence that added this file
    original_size INTEGER NOT NULL,
    file_storage_volume_id INTEGER NOT NULL REFERENCES file_storage_volume(id),
    UNIQUE(relative_path, file_storage_volume_id)
);

CREATE INDEX IF NOT EXISTS idx_scanned_log_entry_barcode ON scanned_log_entry(barcode);
CREATE INDEX IF NOT EXISTS idx_session_barcode_session ON session_barcode(scan_session_id);
"#;

/// Version of the index schema, stored in SQLite's `PRAGMA user_version`.
///
/// * `0` — either a brand-new database or the legacy `.tar.zst`/chunk-offset
///   format (which never set a version). Told apart by inspecting the tables.
/// * `2` — the MOCHI format (`head_commit_id`/`head_seq`, `content_hash`/`commit_seq`).
///
/// Clean break: a legacy database is **not** migrated and **not** modified.
/// `open_and_migrate` refuses it with [`ArchiveError::SchemaMismatch`] and tells
/// the operator to move it aside (a fresh index is then created). The legacy
/// `.tar.zst` volumes remain on disk untouched.
pub const SCHEMA_VERSION: i64 = 2;

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    // Column 1 of `PRAGMA table_info` is the column name.
    for name in stmt.query_map([], |row| row.get::<_, String>(1))? {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn apply_schema(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == 0 {
        // Unversioned: legacy tar.zst index, or empty.
        let legacy = table_has_column(conn, "file_storage_volume", "sha256")?
            || table_has_column(conn, "scanned_log_entry", "chunk_start_byte")?;
        if legacy {
            return Err(ArchiveError::SchemaMismatch {
                found: 0,
                expected: SCHEMA_VERSION,
                hint: "this index was written by the old tar.zst/chunk-offset format and is not migrated. \
                       Move or rename the old database file (keep it for reference) and start again with a new one; \
                       a fresh index is created automatically."
                    .to_string(),
            });
        }
    } else if version != SCHEMA_VERSION {
        return Err(ArchiveError::SchemaMismatch {
            found: version,
            expected: SCHEMA_VERSION,
            hint: "this database was written by a different version of the program".to_string(),
        });
    }
    conn.execute_batch(SCHEMA_SQL)?;
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    Ok(())
}

/// Opens (creating if needed) the SQLite index DB at `path` and ensures
/// the schema exists. Safe to call on every startup. A legacy-format
/// database is rejected with a clear error (see [`SCHEMA_VERSION`]).
pub fn open_and_migrate(path: impl AsRef<std::path::Path>) -> Result<Connection> {
    let conn = Connection::open(path)?;
    apply_schema(&conn)?;
    Ok(conn)
}

/// In-memory DB for tests.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    apply_schema(&conn)?;
    Ok(conn)
}
