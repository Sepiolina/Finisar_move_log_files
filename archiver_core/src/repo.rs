//! Direct SQL queries for managing scan sessions, storage volumes, and log entries.

use crate::error::{ArchiveError, Result};
use crate::writer::{ChunkSpan, FinishedVolume};
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Success,
    Failed,
}

impl SessionStatus {
    fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Running => "running",
            SessionStatus::Success => "success",
            SessionStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarcodeStatus {
    Matched,
    NoFilesFound,
    Error,
}

impl BarcodeStatus {
    fn as_str(self) -> &'static str {
        match self {
            BarcodeStatus::Matched => "matched",
            BarcodeStatus::NoFilesFound => "no_files_found",
            BarcodeStatus::Error => "error",
        }
    }
}

/// Checks if a session ID exists to prevent using stale or invalid IDs.
pub fn session_exists(conn: &Connection, session_id: i64) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM scan_session WHERE id = ?1",
        params![session_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Creates a new scan session with status `running` and returns its ID.
pub fn start_session(conn: &Connection, started_at: &str, query_from_date: &str, query_to_date: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO scan_session (started_at, status, query_from_date, query_to_date)
         VALUES (?1, ?2, ?3, ?4)",
        params![started_at, SessionStatus::Running.as_str(), query_from_date, query_to_date],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Marks a session as complete (success or failed) and updates summary counters.
pub fn complete_session(
    conn: &Connection,
    session_id: i64,
    completed_at: &str,
    status: SessionStatus,
    total_barcodes: i64,
    total_files_moved: i64,
    total_errors: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE scan_session
         SET completed_at = ?1, status = ?2, total_barcodes = ?3,
             total_files_moved = ?4, total_errors = ?5
         WHERE id = ?6",
        params![completed_at, status.as_str(), total_barcodes, total_files_moved, total_errors, session_id],
    )?;
    Ok(())
}

/// Returns the end date of the last successful scan (used to figure out where to resume).
pub fn last_successful_scan_to_date(conn: &Connection) -> Result<Option<String>> {
    conn.query_row(
        "SELECT query_to_date FROM scan_session
         WHERE status = 'success'
         ORDER BY id DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(ArchiveError::from)
}

/// Records a barcode scan. Returns `Ok(None)` if it was already recorded (duplicate).
pub fn record_session_barcode(
    conn: &Connection,
    scan_session_id: i64,
    barcode: &str,
    last_update: &str,
    status: BarcodeStatus,
) -> Result<Option<i64>> {
    let result = conn.execute(
        "INSERT INTO session_barcode (scan_session_id, barcode, last_update, status)
         VALUES (?1, ?2, ?3, ?4)",
        params![scan_session_id, barcode, last_update, status.as_str()],
    );

    match result {
        Ok(_) => Ok(Some(conn.last_insert_rowid())),
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Ok(None)
        }
        Err(e) => Err(ArchiveError::from(e)),
    }
}

/// Checks if a barcode was scanned previously (used to skip unneeded work early).
pub fn is_barcode_already_scanned(conn: &Connection, barcode: &str, last_update: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM session_barcode WHERE barcode = ?1 AND last_update = ?2",
        params![barcode, last_update],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Finds a storage volume ID by name (e.g. "2024-07"), if it exists.
pub fn find_volume_by_name(conn: &Connection, volume_name: &str) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM file_storage_volume WHERE volume_name = ?1",
        params![volume_name],
        |row| row.get(0),
    )
    .optional()
    .map_err(ArchiveError::from)
}

/// Inserts a new volume or updates an existing one with updated size, hash, and totals.
pub fn upsert_volume(
    conn: &Connection,
    volume_name: &str,
    finished: &FinishedVolume,
    cumulative_entry_count: i64,
    now: &str,
) -> Result<i64> {
    let path_str = finished.path.to_string_lossy();

    match find_volume_by_name(conn, volume_name)? {
        Some(existing_id) => {
            conn.execute(
                "UPDATE file_storage_volume
                 SET file_size = ?1, sha256 = ?2, entry_count = ?3, updated_at = ?4
                 WHERE id = ?5",
                params![finished.file_size as i64, finished.sha256, cumulative_entry_count, now, existing_id],
            )?;
            Ok(existing_id)
        }
        None => {
            conn.execute(
                "INSERT INTO file_storage_volume (path, volume_name, file_size, sha256, entry_count, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![path_str, volume_name, finished.file_size as i64, finished.sha256, cumulative_entry_count, now],
            )?;
            Ok(conn.last_insert_rowid())
        }
    }
}

/// Returns the current total entry count recorded for a volume.
pub fn existing_entry_count(conn: &Connection, file_storage_volume_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT entry_count FROM file_storage_volume WHERE id = ?1",
        params![file_storage_volume_id],
        |row| row.get(0),
    )
    .map_err(ArchiveError::from)
}

/// Input payload for creating a single `scanned_log_entry` row.
pub struct LogEntryInsert<'a> {
    pub scan_session_id: i64,
    pub barcode: &'a str,
    pub relative_path: &'a str,
    pub source_subfolder: &'a str,
    pub chunk_start_byte: u64,
    pub chunk_end_byte: u64,
    pub original_size: u64,
    pub file_storage_volume_id: i64,
}

/// Inserts a log entry and returns its ID.
pub fn insert_log_entry(conn: &Connection, entry: &LogEntryInsert) -> Result<i64> {
    conn.execute(
        "INSERT INTO scanned_log_entry
            (scan_session_id, barcode, relative_path, source_subfolder,
             chunk_start_byte, chunk_end_byte, original_size, file_storage_volume_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            entry.scan_session_id,
            entry.barcode,
            entry.relative_path,
            entry.source_subfolder,
            entry.chunk_start_byte as i64,
            entry.chunk_end_byte as i64,
            entry.original_size as i64,
            entry.file_storage_volume_id,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// File location details used during file extraction.
#[derive(Debug, Clone)]
pub struct LogEntryLocation {
    pub relative_path: String,
    pub chunk_start_byte: u64,
    pub chunk_end_byte: u64,
    pub volume_path: String,
}

/// Retrieves all log entry locations matching an exact barcode.
pub fn find_entries_by_barcode(conn: &Connection, barcode: &str) -> Result<Vec<LogEntryLocation>> {
    let mut stmt = conn.prepare(
        "SELECT sle.relative_path, sle.chunk_start_byte, sle.chunk_end_byte, fsv.path
         FROM scanned_log_entry sle
         JOIN file_storage_volume fsv ON fsv.id = sle.file_storage_volume_id
         WHERE sle.barcode = ?1",
    )?;
    let rows = stmt.query_map(params![barcode], |row| {
        Ok(LogEntryLocation {
            relative_path: row.get(0)?,
            chunk_start_byte: row.get::<_, i64>(1)? as u64,
            chunk_end_byte: row.get::<_, i64>(2)? as u64,
            volume_path: row.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(ArchiveError::from)
}

/// Searches log entries by filename/path substring (used in search panels).
pub fn find_entries_by_filename_like(conn: &Connection, needle: &str) -> Result<Vec<LogEntryLocation>> {
    let pattern = format!("%{needle}%");
    let mut stmt = conn.prepare(
        "SELECT sle.relative_path, sle.chunk_start_byte, sle.chunk_end_byte, fsv.path
         FROM scanned_log_entry sle
         JOIN file_storage_volume fsv ON fsv.id = sle.file_storage_volume_id
         WHERE sle.relative_path LIKE ?1",
    )?;
    let rows = stmt.query_map(params![pattern], |row| {
        Ok(LogEntryLocation {
            relative_path: row.get(0)?,
            chunk_start_byte: row.get::<_, i64>(1)? as u64,
            chunk_end_byte: row.get::<_, i64>(2)? as u64,
            volume_path: row.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(ArchiveError::from)
}

/// Saves a completed volume and all its entries inside a single database transaction.
pub fn record_finished_volume(
    conn: &mut Connection,
    scan_session_id: i64,
    volume_name: &str,
    finished: &FinishedVolume,
    now: &str,
    barcode_of: impl Fn(&str) -> Option<String>,
    subfolder_of: impl Fn(&str) -> String,
) -> Result<i64> {
    let tx = conn.transaction()?;

    let prior_count = match find_volume_by_name(&tx, volume_name)? {
        Some(id) => existing_entry_count(&tx, id)?,
        None => 0,
    };
    let cumulative = prior_count + finished.entry_count as i64;

    let volume_id = upsert_volume(&tx, volume_name, finished, cumulative, now)?;

    for entry in &finished.written_entries {
        let Some(barcode) = barcode_of(&entry.relative_path) else {
            continue;
        };
        let subfolder = subfolder_of(&entry.relative_path);
        insert_log_entry(
            &tx,
            &LogEntryInsert {
                scan_session_id,
                barcode: &barcode,
                relative_path: &entry.relative_path,
                source_subfolder: &subfolder,
                chunk_start_byte: entry.chunk_start_byte,
                chunk_end_byte: entry.chunk_end_byte,
                original_size: entry.original_size,
                file_storage_volume_id: volume_id,
            },
        )?;
    }

    tx.commit()?;
    Ok(volume_id)
}

/// Helper function (mainly for unit tests) to verify byte offsets match stored records.
pub fn spans_match(spans: &[ChunkSpan], entries: &[LogEntryLocation]) -> bool {
    entries.iter().all(|e| {
        spans
            .iter()
            .any(|s| s.chunk_start_byte == e.chunk_start_byte && s.chunk_end_byte == e.chunk_end_byte)
    })
}