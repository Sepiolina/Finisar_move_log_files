//! One archiving run, start to finish:
//!
//! 1. **Resume** ([`run_session`]): a staging manifest left by a crashed run
//!    is finished first.
//! 2. **Scan** ([`scan`]): list the configured subfolders and keep the files
//!    whose name matches exactly one candidate barcode.
//! 3. **Stage** ([`stage`]): list each match in the manifest, durably, then
//!    move it into `/staging`.
//! 4. **Archive** ([`months`]): per month, commit the staged files into
//!    `YYYY-MM.mochi`, verify them read-only, index them in SQLite, and only
//!    then delete their staged copies.
//! 5. **Record**: clean up `/staging` and save the session's counters.
//!
//! **Crash safety:** a file is always in exactly one of three places: the
//! source tree, `/staging` (listed in the manifest), or a committed and
//! verified archive. A run killed at any point is finished by the next one.

mod manifest;
mod months;
mod scan;
mod stage;

pub use manifest::{StagedManifestEntry, StagingManifest, StagingManifestWriter};
pub use scan::validate_subfolder;

use crate::db::ArchivalCandidate;
use crate::error::{ArchiveError, Result};
use crate::matcher::BarcodeMatcher;
use crate::par;
use crate::repo::{self, BarcodeStatus, SessionStatus};
use months::Threads;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// User-configurable settings for the current scan run.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub source_dir: PathBuf,
    pub output_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub subfolders: Vec<String>,
    pub allowed_extensions: Vec<String>,
    pub lookback_days: i64,
    /// Manual override to scan a specific date range (e.g., historical backfill)
    /// instead of automatically calculating from the last run.
    pub date_range_override: Option<(String, String)>,
    /// Worker threads for scanning, compression and verification (`0`:
    /// automatic, the CPU count capped at 8). Renames and deletes, which
    /// mostly wait on the disk, use twice as many.
    pub workers: usize,
}

/// Summary statistics for a completed archiving session.
#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResult {
    pub scan_session_id: i64,
    pub barcodes_matched: usize,
    pub files_moved: usize,
    pub errors: usize,
    pub volume_names: Vec<String>,
    pub total_files_scanned: usize,
    pub archive_breakdown: Vec<VolumeBreakdown>,
    pub unmatched_barcodes: Vec<String>,
    pub files_deleted: usize,
    /// Volumes committed with unconfirmed directory durability (degraded, not an error).
    pub durability_notes: Vec<String>,
}

/// Statistics for a specific monthly volume. (For reporting only).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeBreakdown {
    pub volume_name: String,
    pub files_archived: usize,
    pub total_bytes: u64,
}

/// The optional progress callback: `(message, done, total)`.
#[derive(Clone, Copy)]
pub(crate) struct Progress<'a>(Option<&'a dyn Fn(String, usize, usize)>);

impl Progress<'_> {
    /// No callback.
    #[cfg(test)]
    const NONE: Progress<'static> = Progress(None);

    fn report(&self, msg: String, done: usize, total: usize) {
        if let Some(cb) = self.0 {
            cb(msg, done, total);
        }
    }

    /// Reports a message without a count (`0, 0`).
    fn say(&self, msg: impl Into<String>) {
        self.report(msg.into(), 0, 0);
    }
}

/// Runs one complete session: scan, stage, archive, verify, clean up, and record.
pub fn run_session<F>(
    conn: &mut Connection,
    config: &SessionConfig,
    candidates: Vec<ArchivalCandidate>,
    query_from_date: &str,
    query_to_date: &str,
    started_at: &str,
    progress_callback: Option<&F>,
) -> Result<SessionResult>
where
    F: Fn(String, usize, usize),
{
    let progress = Progress(progress_callback.map(|f| f as &dyn Fn(String, usize, usize)));
    for sub in &config.subfolders {
        validate_subfolder(sub)?;
    }

    // 1. A previous run left files in staging: finish archiving them first.
    if let Some(manifest) = StagingManifest::read_if_exists(&config.staging_dir)? {
        progress.say("Found interrupted session. Resuming archive of staged files...");
        finish_staged_batch(conn, config, &manifest, progress)?;
    }

    let session_id = repo::start_session(conn, started_at, query_from_date, query_to_date)?;
    let complete = |conn: &Connection, barcodes: usize, moved: usize, errors: i64| {
        repo::complete_session(
            conn,
            session_id,
            started_at,
            SessionStatus::Success,
            barcodes as i64,
            moved as i64,
            errors,
        )
    };

    progress.say("Checking candidates against SQLite index...");
    if candidates.is_empty() {
        progress.say("No archival candidates found for this window. Nothing to do!");
        complete(conn, 0, 0, 0)?;
        return Ok(SessionResult {
            scan_session_id: session_id,
            ..SessionResult::default()
        });
    }

    progress.say(format!(
        "Building search index for {} candidates (this may take a moment)...",
        candidates.len()
    ));
    // 2. The search index uses ALL candidates, even previously scanned ones,
    // so secondary files (like a log file that arrived late) are not missed.
    let matcher = BarcodeMatcher::new(candidates.iter().map(|c| c.barcode.as_str()));
    let candidate_date: HashMap<&str, &str> = candidates
        .iter()
        .map(|c| (c.barcode.as_str(), c.last_update.as_str()))
        .collect();
    let already_scanned = already_scanned(conn, &candidates)?;
    let fresh_candidates: Vec<&ArchivalCandidate> = candidates
        .iter()
        .filter(|c| !already_scanned.contains(c.barcode.as_str()))
        .collect();

    let workers = par::worker_count(config.workers);
    let scan = scan::scan_all(config, workers, &matcher, &candidate_date, progress);
    let mut errors = scan.errors;

    let matched: HashSet<&str> = scan.entries.iter().map(|e| e.barcode.as_str()).collect();
    let unmatched_barcodes: Vec<String> = fresh_candidates
        .iter()
        .filter(|c| !matched.contains(c.barcode.as_str()))
        .map(|c| c.barcode.clone())
        .collect();
    record_barcode_statuses(conn, session_id, &fresh_candidates, &matched, progress)?;

    if scan.entries.is_empty() {
        complete(conn, fresh_candidates.len(), 0, errors)?;
        return Ok(SessionResult {
            scan_session_id: session_id,
            errors: errors as usize,
            total_files_scanned: scan.files_seen,
            unmatched_barcodes,
            ..SessionResult::default()
        });
    }

    // 3. Move matched files into staging, listing them in the manifest first.
    let (manifest, stage_failures) = stage::stage_entries(
        config,
        session_id,
        &scan.entries,
        io_worker_count(config.workers),
        progress,
    )?;
    errors += stage_failures as i64;

    // 4. Archive, verify, index and clean up.
    let archived = finish_staged_batch(conn, config, &manifest, progress)?;
    errors += archived.errors as i64;
    complete(conn, fresh_candidates.len(), archived.files_moved, errors)?;

    Ok(SessionResult {
        scan_session_id: session_id,
        errors: errors as usize,
        total_files_scanned: scan.files_seen,
        unmatched_barcodes,
        ..archived
    })
}

/// Threads for file-system metadata work (renames, deletes): it waits on the
/// disk far more than it uses a core, so more threads than cores still help.
fn io_worker_count(requested: usize) -> usize {
    (par::worker_count(requested) * 2).min(16)
}

/// Barcodes already recorded as scanned for their `last_update`.
fn already_scanned<'c>(
    conn: &mut Connection,
    candidates: &'c [ArchivalCandidate],
) -> Result<HashSet<&'c str>> {
    let mut set = HashSet::new();
    // One read transaction for all lookups (hundreds of thousands).
    let tx = conn.transaction()?;
    for c in candidates {
        if repo::is_barcode_already_scanned(&tx, &c.barcode, &c.last_update)? {
            set.insert(c.barcode.as_str());
        }
    }
    tx.commit()?;
    Ok(set)
}

/// Records Matched / NoFilesFound for every fresh candidate in one transaction.
fn record_barcode_statuses(
    conn: &mut Connection,
    session_id: i64,
    fresh_candidates: &[&ArchivalCandidate],
    matched: &HashSet<&str>,
    progress: Progress<'_>,
) -> Result<()> {
    progress.say("Recording barcode match statuses to database (fast insert)...");

    // Raw BEGIN/COMMIT (not `conn.transaction()`): keeps the error variant and
    // the no-rollback-on-error behaviour of the original code.
    let db_error = |e: rusqlite::Error| ArchiveError::Config(format!("DB error: {e}"));
    conn.execute_batch("BEGIN TRANSACTION").map_err(db_error)?;

    let total = fresh_candidates.len();
    for (i, c) in fresh_candidates.iter().enumerate() {
        if i % 50000 == 0 && i > 0 {
            progress.report(
                format!("Recorded {i}/{total} barcode statuses to index..."),
                i,
                total,
            );
        }
        let status = if matched.contains(c.barcode.as_str()) {
            BarcodeStatus::Matched
        } else {
            BarcodeStatus::NoFilesFound
        };
        repo::record_session_barcode(conn, session_id, &c.barcode, &c.last_update, status)?;
    }

    conn.execute_batch("COMMIT").map_err(db_error)?;
    Ok(())
}

/// Commits files from `/staging` into monthly `.mochi` archives, verifies them,
/// and cleans up the `/staging` folder on success.
fn finish_staged_batch(
    conn: &mut Connection,
    config: &SessionConfig,
    manifest: &StagingManifest,
    progress: Progress<'_>,
) -> Result<SessionResult> {
    // If the database was wiped but the staging folder wasn't, the old
    // session ID won't exist. Create a new session to recover into.
    let session_id = if repo::session_exists(conn, manifest.scan_session_id)? {
        manifest.scan_session_id
    } else {
        progress.say(format!(
            "Warning: Batch references missing session {}. Creating new recovery session.",
            manifest.scan_session_id
        ));
        repo::start_session(conn, &started_at_now(), "resumed", "resumed")?
    };

    let jobs = months::plan_jobs(conn, config, manifest)?;
    let threads = Threads {
        cpu: par::worker_count(config.workers),
        io: io_worker_count(config.workers),
    };
    let months = months::run_jobs(conn, config, session_id, &jobs, threads, progress);
    if let Some(first) = months.failures.into_iter().next() {
        // The manifest stays: the next run resumes the failed months.
        return Err(first);
    }

    let total = manifest.files.len();
    progress.report(
        "Verification complete across all months. Cleaning up staging area...".to_string(),
        total,
        total,
    );
    let cleanup_errors = months::cleanup_staging(config, progress)?;

    let files_moved = months.archived.len();
    let barcodes_matched = months
        .archived
        .iter()
        .map(|e| e.barcode.as_str())
        .collect::<HashSet<_>>()
        .len();
    repo::complete_session(
        conn,
        session_id,
        &started_at_now(),
        SessionStatus::Success,
        barcodes_matched as i64,
        files_moved as i64,
        cleanup_errors as i64,
    )?;

    Ok(SessionResult {
        scan_session_id: session_id,
        barcodes_matched,
        files_moved,
        errors: cleanup_errors,
        volume_names: months
            .breakdown
            .iter()
            .map(|b| b.volume_name.clone())
            .collect(),
        total_files_scanned: 0,
        archive_breakdown: months.breakdown,
        unmatched_barcodes: Vec::new(),
        // Every archived file's staged copy was deleted right after its month
        // was indexed.
        files_deleted: files_moved,
        durability_notes: months.durability_notes,
    })
}

fn started_at_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("epoch:{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::open_in_memory;
    use std::fs;

    /// Tests that the archiver handles an orphaned session ID correctly
    /// (e.g. if the database gets reset but files are still in staging).
    #[test]
    fn finish_staged_batch_recovers_from_orphaned_scan_session_id() {
        let mut conn = open_in_memory().unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let staging_dir = tmp.path().to_path_buf();
        let output_dir = staging_dir.join("output");
        fs::create_dir_all(staging_dir.join("BURNIN")).unwrap();
        fs::create_dir_all(&output_dir).unwrap();

        let staged_relative_path = "BURNIN/unit_BC001_log.txt";
        fs::write(staging_dir.join(staged_relative_path), b"orphan test data").unwrap();

        // This session ID does not exist in the new dummy database.
        let manifest = StagingManifest {
            scan_session_id: 999_999,
            files: vec![StagedManifestEntry {
                relative_path: staged_relative_path.to_string(),
                source_subfolder: "BURNIN".to_string(),
                barcode: "BC001".to_string(),
                original_size: b"orphan test data".len() as u64,
                volume_name: "2024-07".to_string(),
            }],
        };

        let config = SessionConfig {
            source_dir: staging_dir.clone(),
            output_dir,
            staging_dir: staging_dir.clone(),
            subfolders: vec!["BURNIN".into()],
            allowed_extensions: vec!["txt".into()],
            lookback_days: 2,
            date_range_override: None,
            workers: 0,
        };

        let result = finish_staged_batch(&mut conn, &config, &manifest, Progress::NONE);
        assert!(result.is_ok(), "Should successfully recover and not fail.");

        let outcome = result.unwrap();
        assert_ne!(
            outcome.scan_session_id, 999_999,
            "Should generate a brand new session ID."
        );
        assert!(repo::session_exists(&conn, outcome.scan_session_id).unwrap());
        assert_eq!(outcome.files_moved, 1);

        assert!(!staging_dir.join(staged_relative_path).exists());
    }
}
