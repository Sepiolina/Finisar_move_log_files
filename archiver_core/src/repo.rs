//! Direct SQL queries for managing scan sessions, storage volumes, and log entries.

use crate::error::{ArchiveError, Result};
use crate::mochi_store::FinishedVolume;
use rusqlite::{params, Connection, OptionalExtension};

// `SessionStatus` and `BarcodeStatus` mirror the `CHECK(status IN (...))`
// constraints in `schema::SCHEMA_SQL`; some variants (`Failed`, `Error`) are
// not written by the current code but are kept so the enums stay in sync with
// the schema.

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
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM scan_session WHERE id = ?1)",
        params![session_id],
        |row| row.get(0),
    )
    .map_err(ArchiveError::from)
}

/// Creates a new scan session with status `running` and returns its ID.
pub fn start_session(
    conn: &Connection,
    started_at: &str,
    query_from_date: &str,
    query_to_date: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO scan_session (started_at, status, query_from_date, query_to_date)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            started_at,
            SessionStatus::Running.as_str(),
            query_from_date,
            query_to_date
        ],
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
        params![
            completed_at,
            status.as_str(),
            total_barcodes,
            total_files_moved,
            total_errors,
            session_id
        ],
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
    let result = conn
        .prepare_cached(
            "INSERT INTO session_barcode (scan_session_id, barcode, last_update, status)
             VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![
            scan_session_id,
            barcode,
            last_update,
            status.as_str()
        ]);

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
pub fn is_barcode_already_scanned(
    conn: &Connection,
    barcode: &str,
    last_update: &str,
) -> Result<bool> {
    conn.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM session_barcode WHERE barcode = ?1 AND last_update = ?2)",
    )?
    .query_row(params![barcode, last_update], |row| row.get(0))
    .map_err(ArchiveError::from)
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

/// The MOCHI head (commit id hex, sequence) recorded for a volume after its
/// last verified write, if any.
pub fn volume_head(conn: &Connection, volume_name: &str) -> Result<Option<(String, u64)>> {
    let row: Option<(Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT head_commit_id, head_seq FROM file_storage_volume WHERE volume_name = ?1",
            params![volume_name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((Some(id), Some(seq))) if !id.is_empty() && seq >= 0 => Some((id, seq as u64)),
        _ => None,
    })
}

/// Inserts a new volume or updates an existing one with the new size, MOCHI head and totals.
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
                 SET path = ?1, file_size = ?2, head_commit_id = ?3, head_seq = ?4,
                     entry_count = ?5, updated_at = ?6
                 WHERE id = ?7",
                params![
                    path_str,
                    finished.file_size as i64,
                    finished.head_commit_id,
                    finished.head_seq as i64,
                    cumulative_entry_count,
                    now,
                    existing_id
                ],
            )?;
            Ok(existing_id)
        }
        None => {
            conn.execute(
                "INSERT INTO file_storage_volume
                    (path, volume_name, file_size, head_commit_id, head_seq, entry_count, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    path_str,
                    volume_name,
                    finished.file_size as i64,
                    finished.head_commit_id,
                    finished.head_seq as i64,
                    cumulative_entry_count,
                    now
                ],
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
    /// BLAKE3 (hex) of the stored file's bytes.
    pub content_hash: &'a str,
    /// MOCHI commit sequence that added the file.
    pub commit_seq: u64,
    pub original_size: u64,
    pub file_storage_volume_id: i64,
}

/// Inserts a log entry and returns its ID.
///
/// Idempotent per `(relative_path, volume)`: if the row already exists (a
/// resumed batch that was recorded but whose staging cleanup was interrupted,
/// or a newer version of the same file name in the same month) it is updated
/// in place to describe the latest committed version.
pub fn insert_log_entry(conn: &Connection, entry: &LogEntryInsert) -> Result<i64> {
    conn.prepare_cached(
        "INSERT INTO scanned_log_entry
            (scan_session_id, barcode, relative_path, source_subfolder,
             content_hash, commit_seq, original_size, file_storage_volume_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(relative_path, file_storage_volume_id) DO UPDATE SET
             scan_session_id = excluded.scan_session_id,
             barcode = excluded.barcode,
             source_subfolder = excluded.source_subfolder,
             content_hash = excluded.content_hash,
             commit_seq = excluded.commit_seq,
             original_size = excluded.original_size
         RETURNING id",
    )?
    .query_row(
        params![
            entry.scan_session_id,
            entry.barcode,
            entry.relative_path,
            entry.source_subfolder,
            entry.content_hash,
            entry.commit_seq as i64,
            entry.original_size as i64,
            entry.file_storage_volume_id,
        ],
        |r| r.get(0),
    )
    .map_err(ArchiveError::from)
}

/// Where an indexed file lives (used for extraction and search panels).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntryLocation {
    pub relative_path: String,
    pub volume_path: String,
    /// BLAKE3 (hex) of the file as committed.
    pub content_hash: Option<String>,
    /// MOCHI commit sequence that added the file.
    pub commit_seq: Option<u64>,
}

fn location_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LogEntryLocation> {
    Ok(LogEntryLocation {
        relative_path: row.get(0)?,
        volume_path: row.get(1)?,
        content_hash: row.get(2)?,
        commit_seq: row.get::<_, Option<i64>>(3)?.map(|v| v as u64),
    })
}

/// Runs the location query with the given `WHERE` clause (one `?1` parameter).
fn find_locations(
    conn: &Connection,
    where_clause: &str,
    param: &str,
) -> Result<Vec<LogEntryLocation>> {
    let sql = format!(
        "SELECT sle.relative_path, fsv.path, sle.content_hash, sle.commit_seq
         FROM scanned_log_entry sle
         JOIN file_storage_volume fsv ON fsv.id = sle.file_storage_volume_id
         WHERE {where_clause}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![param], location_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(ArchiveError::from)
}

/// Retrieves all log entry locations matching an exact barcode.
pub fn find_entries_by_barcode(conn: &Connection, barcode: &str) -> Result<Vec<LogEntryLocation>> {
    find_locations(conn, "sle.barcode = ?1", barcode)
}

/// Searches log entries by filename/path substring (used in search panels).
pub fn find_entries_by_filename_like(
    conn: &Connection,
    needle: &str,
) -> Result<Vec<LogEntryLocation>> {
    find_locations(conn, "sle.relative_path LIKE ?1", &format!("%{needle}%"))
}

/// Saves a committed volume and all its entries inside a single database transaction.
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

    // Upsert first so entry rows have a volume id. The entry count written
    // here is a placeholder: nothing reads it inside this transaction and it
    // is overwritten below from the actual rows before commit.
    let volume_id = upsert_volume(&tx, volume_name, finished, 0, now)?;

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
                content_hash: &entry.content_hash,
                commit_seq: entry.commit_seq,
                original_size: entry.original_size,
                file_storage_volume_id: volume_id,
            },
        )?;
    }

    let total: i64 = tx.query_row(
        "SELECT COUNT(*) FROM scanned_log_entry WHERE file_storage_volume_id = ?1",
        params![volume_id],
        |r| r.get(0),
    )?;
    tx.execute(
        "UPDATE file_storage_volume SET entry_count = ?1 WHERE id = ?2",
        params![total, volume_id],
    )?;

    tx.commit()?;
    Ok(volume_id)
}
