use crate::error::Result;
use rusqlite::Connection;

pub const SCHEMA_SQL: &str =
    r#"
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
    sha256 TEXT NOT NULL,
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
    chunk_start_byte INTEGER NOT NULL,
    chunk_end_byte INTEGER NOT NULL,
    original_size INTEGER NOT NULL,
    file_storage_volume_id INTEGER NOT NULL REFERENCES file_storage_volume(id),
    UNIQUE(relative_path, file_storage_volume_id)
);

CREATE INDEX IF NOT EXISTS idx_scanned_log_entry_barcode ON scanned_log_entry(barcode);
CREATE INDEX IF NOT EXISTS idx_session_barcode_session ON session_barcode(scan_session_id);
"#;

/// Opens (creating if needed) the SQLite index DB at `path` and ensures
/// the schema exists. Safe to call on every startup — all statements are
/// `IF NOT EXISTS`.
pub fn open_and_migrate(path: impl AsRef<std::path::Path>) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(conn)
}

/// In-memory DB for tests.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(conn)
}
