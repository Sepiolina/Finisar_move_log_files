//! This module runs the main archiving process:
//! 1. Query the database for files that need to be archived.
//! 2. Scan source folders for these files (using fast text search).
//! 3. Safely move matched files into a temporary `/staging` folder.
//! 4. Commit files from `/staging` into monthly MOCHI archives (`YYYY-MM.mochi`).
//! 5. Verify each archive read-only (content hashes vs the staged files, plus
//!    MOCHI's content-level verification) before touching staging.
//! 6. Delete the `/staging` files and save the completion status to the DB.
//!
//! **Crash Safety:** If the program crashes, no data is lost. The staging
//! manifest tracks progress, so the next run picks up exactly where it left off.

use crate::db::ArchivalCandidate;
use crate::error::{ArchiveError, Result};
use crate::matcher::{has_matching_extension, BarcodeMatcher};
use crate::mochi_store::{self, StoreFile};
use crate::repo::{self, BarcodeStatus, SessionStatus};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A record of files currently sitting in the `/staging` folder.
///
/// Saved as `manifest.jsonl` (one JSON object per line). We use an append-only
/// text file so if the app crashes, we only lose the very last un-flushed lines.
/// On restart, the app reads this file to resume archiving safely.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StagingManifest {
    pub scan_session_id: i64,
    pub files: Vec<StagedManifestEntry>,
}

/// Details about a single file in the staging manifest.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StagedManifestEntry {
    pub relative_path: String,
    pub source_subfolder: String,
    pub barcode: String,
    pub original_size: u64,
    /// The "YYYY-MM" archive file this entry belongs in.
    /// This is based on the barcode's original date, not today's date.
    pub volume_name: String,
}

/// A line in the `manifest.jsonl` file.
/// The first line is always the Header, and the rest are File Entries.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum ManifestLine {
    Header { scan_session_id: i64 },
    Entry(StagedManifestEntry),
}

const MANIFEST_FILENAME: &str = "manifest.jsonl";

impl StagingManifest {
    fn path_in(staging_dir: &Path) -> PathBuf {
        staging_dir.join(MANIFEST_FILENAME)
    }

    /// Reads the manifest from disk to resume a previous interrupted run.
    /// Safely ignores incomplete lines (caused by crashes mid-write)
    /// and treats them as un-staged.
    pub fn read_if_exists(staging_dir: &Path) -> Result<Option<Self>> {
        let path = Self::path_in(staging_dir);
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path).map_err(|e| ArchiveError::io(&path, e))?;
        let mut lines = content.lines();

        let Some(header_line) = lines.next() else {
            return Ok(None);
        };
        let scan_session_id = match serde_json::from_str::<ManifestLine>(header_line) {
            Ok(ManifestLine::Header { scan_session_id }) => scan_session_id,
            _ => return Ok(None),
        };

        let mut files = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ManifestLine>(line) {
                Ok(ManifestLine::Entry(entry)) => files.push(entry),
                _ => continue, // Skip corrupted lines safely
            }
        }

        Ok(Some(StagingManifest {
            scan_session_id,
            files,
        }))
    }
}

/// Writes entries to `manifest.jsonl` as files are moved into staging.
/// Flushes to disk periodically to balance write speed and crash safety.
pub struct StagingManifestWriter {
    file: std::io::BufWriter<fs::File>,
    pending_since_flush: usize,
}

impl StagingManifestWriter {
    /// How many files to stage before forcing a save to the disk.
    pub const FLUSH_EVERY: usize = 2000;

    /// Creates the manifest file and writes the initial session header.
    pub fn create(staging_dir: &Path, scan_session_id: i64) -> Result<Self> {
        let path = StagingManifest::path_in(staging_dir);
        let file = fs::File::create(&path).map_err(|e| ArchiveError::io(&path, e))?;
        let mut writer = StagingManifestWriter {
            file: std::io::BufWriter::new(file),
            pending_since_flush: 0,
        };
        writer.write_line(&ManifestLine::Header { scan_session_id })?;
        writer.flush()?;
        Ok(writer)
    }

    /// Records a newly staged file into the manifest.
    pub fn record(&mut self, entry: StagedManifestEntry) -> Result<()> {
        self.write_line(&ManifestLine::Entry(entry))?;
        self.pending_since_flush += 1;
        if self.pending_since_flush >= Self::FLUSH_EVERY {
            self.flush()?;
        }
        Ok(())
    }

    fn write_line(&mut self, line: &ManifestLine) -> Result<()> {
        let json = serde_json::to_string(line)
            .map_err(|e| ArchiveError::Config(format!("failed to serialize manifest line: {e}")))?;
        writeln!(self.file, "{json}").map_err(|e| ArchiveError::io("manifest.jsonl", e))
    }

    /// Forces unsaved lines to be written to disk and synced (`fsync`), so a
    /// power cut cannot keep a rename while losing the line that lists it.
    pub fn flush(&mut self) -> Result<()> {
        self.file
            .flush()
            .map_err(|e| ArchiveError::io("manifest.jsonl", e))?;
        self.file
            .get_ref()
            .sync_data()
            .map_err(|e| ArchiveError::io("manifest.jsonl", e))?;
        self.pending_since_flush = 0;
        Ok(())
    }

    /// Finalizes the writer, flushing any leftover lines.
    pub fn finish(mut self) -> Result<()> {
        self.flush()
    }
}

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
    /// Worker threads for scanning, staging, verification and cleanup
    /// (`0`: automatic, the CPU count capped at 8).
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

/// The optional progress callback: `(message, done, total)`. Messages are
/// built even when no callback is set.
struct Progress<'a, F>(Option<&'a F>);

impl<F: Fn(String, usize, usize)> Progress<'_, F> {
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

/// Extracts the "YYYY-MM" volume name from an ISO date string (YYYY-MM-DD).
fn volume_name_from_date(date: &str) -> Result<String> {
    let bytes = date.as_bytes();
    let well_formed = date.len() >= 7
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit);
    if !well_formed {
        return Err(ArchiveError::Config(format!(
            "cannot derive archive month from malformed date {date:?} (expected YYYY-MM-DD)"
        )));
    }
    Ok(date[0..7].to_string())
}

/// What scanning one subfolder found.
struct ScanOutcome {
    entries: Vec<StagedManifestEntry>,
    errors: i64,
    warning: Option<String>,
}

/// Lists one configured subfolder (not recursive) and keeps the files whose
/// name matches exactly one candidate barcode. Only matches are `stat`ed:
/// the entry type comes from the directory listing itself, and symlinks are
/// not followed (moving a link would not move the log it points to).
fn scan_subfolder(
    config: &SessionConfig,
    subfolder: &str,
    matcher: &BarcodeMatcher,
    candidate_date: &HashMap<&str, &str>,
    scanned: &AtomicUsize,
) -> ScanOutcome {
    let mut out = ScanOutcome {
        entries: Vec::new(),
        errors: 0,
        warning: None,
    };
    let subfolder_dir = config.source_dir.join(subfolder);
    if !subfolder_dir.is_dir() {
        out.errors += 1;
        out.warning = Some(format!(
            "Warning: configured subfolder '{subfolder}' not found — skipping."
        ));
        return out;
    }
    // One unreadable folder must not fail the whole run.
    let read_dir = match fs::read_dir(&subfolder_dir) {
        Ok(rd) => rd,
        Err(e) => {
            out.errors += 1;
            out.warning = Some(format!(
                "Warning: failed to read subfolder '{subfolder}': {e} — skipping."
            ));
            return out;
        }
    };

    for entry in read_dir {
        scanned.fetch_add(1, Ordering::Relaxed);
        let Ok(entry) = entry else {
            out.errors += 1;
            continue;
        };
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let file_name = entry.file_name();
        let filename = file_name.to_string_lossy();
        if !has_matching_extension(&filename, &config.allowed_extensions) {
            continue;
        }

        let matched_barcodes = matcher.matching_barcodes(&filename);
        if matched_barcodes.len() > 1 {
            out.errors += 1; // Ambiguous: matches several barcodes.
            continue;
        }
        let Some(barcode) = matched_barcodes.into_iter().next() else {
            continue;
        };
        let Some(date) = candidate_date.get(barcode.as_str()) else {
            out.errors += 1;
            continue;
        };
        let Ok(volume_name) = volume_name_from_date(date) else {
            out.errors += 1;
            continue;
        };

        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.entries.push(StagedManifestEntry {
            relative_path: format!("{subfolder}/{filename}"),
            source_subfolder: subfolder.to_string(),
            barcode,
            original_size: size,
            volume_name,
        });
    }
    out
}

/// Moves one listed file from the source tree into staging. Never replaces
/// a file already in staging. The error is a message for the operator.
fn stage_one(
    config: &SessionConfig,
    entry: &StagedManifestEntry,
) -> std::result::Result<(), String> {
    let source_path = config.source_dir.join(&entry.relative_path);
    let staged_path = config.staging_dir.join(&entry.relative_path);
    if fs::symlink_metadata(&staged_path).is_ok() {
        return Err(format!(
            "{}: already in staging, not overwritten",
            entry.relative_path
        ));
    }
    fs::rename(&source_path, &staged_path).map_err(|e| format!("{}: {e}", entry.relative_path))
}

/// Checks that a configured subfolder is a single plain path component: not
/// empty, no `/` or `\\`, not `.`/`..`, no drive prefix (`:`) and no NUL.
pub fn validate_subfolder(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':', '\0'])
        || name.trim() != name
        || {
            let mut c = Path::new(name).components();
            !matches!(
                (c.next(), c.next()),
                (Some(std::path::Component::Normal(_)), None)
            )
        };
    if bad {
        return Err(ArchiveError::Config(format!(
            "invalid subfolder {name:?}: must be a single plain folder name (no '/', '\\', '..', ':' or empty)"
        )));
    }
    Ok(())
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
    let progress = Progress(progress_callback);
    for sub in &config.subfolders {
        validate_subfolder(sub)?;
    }

    // 1. A previous run left files in staging: finish archiving them first.
    if let Some(manifest) = StagingManifest::read_if_exists(&config.staging_dir)? {
        progress.say("Found interrupted session. Resuming archive of staged files...");
        finish_staged_batch(conn, config, &manifest, progress_callback)?;
    }

    let session_id = repo::start_session(conn, started_at, query_from_date, query_to_date)?;

    progress.say("Checking candidates against SQLite index...");

    if candidates.is_empty() {
        progress.say("No archival candidates found for this window. Nothing to do!");
        repo::complete_session(
            conn,
            session_id,
            started_at,
            SessionStatus::Success,
            0,
            0,
            0,
        )?;
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
    let barcodes: Vec<String> = candidates.iter().map(|c| c.barcode.clone()).collect();
    let matcher = BarcodeMatcher::new(barcodes);

    let candidate_date: HashMap<&str, &str> = candidates
        .iter()
        .map(|c| (c.barcode.as_str(), c.last_update.as_str()))
        .collect();

    let already_scanned = already_scanned(conn, &candidates)?;
    let fresh_candidates: Vec<&ArchivalCandidate> = candidates
        .iter()
        .filter(|c| !already_scanned.contains(c.barcode.as_str()))
        .collect();

    let workers = crate::par::worker_count(config.workers);

    let (mut manifest_entries, total_files_scanned, mut errors) =
        scan_all(config, workers, &matcher, &candidate_date, &progress);
    errors += dedup_by_path(&mut manifest_entries, &progress);

    let matched_barcode_set: HashSet<&str> = manifest_entries
        .iter()
        .map(|e| e.barcode.as_str())
        .collect();

    let unmatched_barcodes: Vec<String> = fresh_candidates
        .iter()
        .filter(|c| !matched_barcode_set.contains(c.barcode.as_str()))
        .map(|c| c.barcode.clone())
        .collect();

    record_barcode_statuses(
        conn,
        session_id,
        &fresh_candidates,
        &matched_barcode_set,
        &progress,
    )?;

    if manifest_entries.is_empty() {
        repo::complete_session(
            conn,
            session_id,
            started_at,
            SessionStatus::Success,
            fresh_candidates.len() as i64,
            0,
            errors,
        )?;
        return Ok(SessionResult {
            scan_session_id: session_id,
            errors: errors as usize,
            total_files_scanned,
            unmatched_barcodes,
            ..SessionResult::default()
        });
    }

    // 3. Move matched files into staging, listing them in the manifest first.
    let (manifest, stage_failures) =
        stage_entries(config, session_id, &manifest_entries, workers, &progress)?;
    errors += stage_failures as i64;

    let result = finish_staged_batch(conn, config, &manifest, progress_callback)?;
    let errors = errors + result.errors as i64;

    repo::complete_session(
        conn,
        session_id,
        started_at,
        SessionStatus::Success,
        fresh_candidates.len() as i64,
        result.files_moved as i64,
        errors,
    )?;

    Ok(SessionResult {
        scan_session_id: session_id,
        barcodes_matched: result.barcodes_matched,
        files_moved: result.files_moved,
        errors: errors as usize,
        volume_names: result.volume_names,
        total_files_scanned,
        archive_breakdown: result.archive_breakdown,
        unmatched_barcodes,
        files_deleted: result.files_deleted,
        durability_notes: result.durability_notes,
    })
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

/// Scans all subfolders in parallel. Returns the matches (in subfolder
/// order), the number of directory entries seen, and the error count.
fn scan_all<F>(
    config: &SessionConfig,
    workers: usize,
    matcher: &BarcodeMatcher,
    candidate_date: &HashMap<&str, &str>,
    progress: &Progress<F>,
) -> (Vec<StagedManifestEntry>, usize, i64)
where
    F: Fn(String, usize, usize),
{
    progress.say(format!(
        "Scanning {} subfolder(s)...",
        config.subfolders.len()
    ));
    let scanned = AtomicUsize::new(0);
    let outcomes = crate::par::map(
        &config.subfolders,
        workers,
        |subfolder| scan_subfolder(config, subfolder, matcher, candidate_date, &scanned),
        |_| {
            progress.say(format!(
                "Scanned {} files so far...",
                scanned.load(Ordering::Relaxed)
            ))
        },
    );
    let total_files_scanned = scanned.load(Ordering::Relaxed);

    let mut entries = Vec::new();
    let mut errors = 0i64;
    for outcome in outcomes {
        errors += outcome.errors;
        if let Some(w) = outcome.warning {
            progress.say(w);
        }
        entries.extend(outcome.entries);
    }

    progress.say(format!(
        "Finished scanning {} files. Found {} matches.",
        total_files_scanned,
        entries.len()
    ));
    (entries, total_files_scanned, errors)
}

/// Keeps the first entry per relative path (a duplicate would fail later at
/// the DB). Returns the number dropped, which count as errors.
fn dedup_by_path<F>(entries: &mut Vec<StagedManifestEntry>, progress: &Progress<F>) -> i64
where
    F: Fn(String, usize, usize),
{
    let mut seen: HashSet<String> = HashSet::with_capacity(entries.len());
    let before = entries.len();
    entries.retain(|e| seen.insert(e.relative_path.clone()));
    let duplicates = before - entries.len();
    if duplicates > 0 {
        progress.say(format!("Warning: skipped {duplicates} duplicate files."));
    }
    duplicates as i64
}

/// Records Matched / NoFilesFound for every fresh candidate in one transaction.
fn record_barcode_statuses<F>(
    conn: &mut Connection,
    session_id: i64,
    fresh_candidates: &[&ArchivalCandidate],
    matched_barcode_set: &HashSet<&str>,
    progress: &Progress<F>,
) -> Result<()>
where
    F: Fn(String, usize, usize),
{
    progress.say("Recording barcode match statuses to database (fast insert)...");

    // Raw BEGIN/COMMIT (not `conn.transaction()`): keeps the error variant and
    // the no-rollback-on-error behaviour of the original code.
    conn.execute_batch("BEGIN TRANSACTION")
        .map_err(|e| ArchiveError::Config(format!("DB error: {e}")))?;

    let total = fresh_candidates.len();
    for (i, c) in fresh_candidates.iter().enumerate() {
        if i % 50000 == 0 && i > 0 {
            progress.report(
                format!("Recorded {i}/{total} barcode statuses to index..."),
                i,
                total,
            );
        }
        let status = if matched_barcode_set.contains(c.barcode.as_str()) {
            BarcodeStatus::Matched
        } else {
            BarcodeStatus::NoFilesFound
        };
        repo::record_session_barcode(conn, session_id, &c.barcode, &c.last_update, status)?;
    }

    conn.execute_batch("COMMIT")
        .map_err(|e| ArchiveError::Config(format!("DB error: {e}")))?;
    Ok(())
}

/// Lists `entries` in a new staging manifest and moves them into staging.
/// Returns the manifest of the files actually moved, and the failure count.
///
/// Crash-correct ordering: per batch of FLUSH_EVERY, the manifest lines are
/// written and SYNCED before any file of that batch is renamed, so a renamed
/// file can never be missing from the manifest. A listed file that was not
/// renamed (yet, or because the rename failed) is simply still in the source
/// tree; resume skips listed files that are absent from staging.
///
/// Renames run in parallel. A file that cannot be moved (locked by the
/// tester, permissions) is counted as an error and left in the source; it
/// is picked up again by a later run. A file whose staging path is already
/// taken is never overwritten (that file could be the only copy).
fn stage_entries<F>(
    config: &SessionConfig,
    session_id: i64,
    entries: &[StagedManifestEntry],
    workers: usize,
    progress: &Progress<F>,
) -> Result<(StagingManifest, usize)>
where
    F: Fn(String, usize, usize),
{
    fs::create_dir_all(&config.staging_dir)
        .map_err(|e| ArchiveError::io(&config.staging_dir, e))?;
    let mut manifest_writer = StagingManifestWriter::create(&config.staging_dir, session_id)?;

    let total_to_stage = entries.len();
    let mut dirs_made: HashSet<PathBuf> = HashSet::new();
    // Only moved files go into the returned manifest. One that was not moved
    // is not archived by this run (a resume after a crash may still pick up a
    // same-named file in staging; never lost).
    let mut staged: Vec<StagedManifestEntry> = Vec::new();
    let mut stage_failures: Vec<String> = Vec::new();
    for (batch_no, batch) in entries
        .chunks(StagingManifestWriter::FLUSH_EVERY)
        .enumerate()
    {
        for entry in batch {
            manifest_writer.record(entry.clone())?;
        }
        manifest_writer.flush()?;

        for entry in batch {
            let staged_path = config.staging_dir.join(&entry.relative_path);
            if let Some(parent) = staged_path.parent() {
                if dirs_made.insert(parent.to_path_buf()) {
                    fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
                }
            }
        }
        let offset = batch_no * StagingManifestWriter::FLUSH_EVERY;
        let results = crate::par::map(
            batch,
            workers,
            |entry| stage_one(config, entry),
            |done| {
                progress.report(
                    format!("Staging file {} of {}...", offset + done, total_to_stage),
                    offset + done,
                    total_to_stage,
                )
            },
        );
        for (entry, result) in batch.iter().zip(results) {
            match result {
                Ok(()) => staged.push(entry.clone()),
                Err(msg) => stage_failures.push(msg),
            }
        }
    }
    manifest_writer.finish()?;
    if !stage_failures.is_empty() {
        progress.report(
            format!(
                "Warning: {} of {} file(s) could not be moved into staging and were left in the source folder (first: {}).",
                stage_failures.len(),
                total_to_stage,
                stage_failures[0]
            ),
            staged.len(),
            total_to_stage,
        );
    }

    let manifest = StagingManifest {
        scan_session_id: session_id,
        files: staged,
    };
    Ok((manifest, stage_failures.len()))
}

/// Commits files from `/staging` into monthly `.mochi` archives, verifies them,
/// and cleans up the `/staging` folder on success.
fn finish_staged_batch<F>(
    conn: &mut Connection,
    config: &SessionConfig,
    manifest: &StagingManifest,
    progress_callback: Option<&F>,
) -> Result<SessionResult>
where
    F: Fn(String, usize, usize),
{
    let progress = Progress(progress_callback);

    // If the database was wiped but the staging folder wasn't, the old
    // session ID won't exist. Create a new session to recover into.
    let effective_session_id = if repo::session_exists(conn, manifest.scan_session_id)? {
        manifest.scan_session_id
    } else {
        progress.say(format!(
            "Warning: Batch references missing session {}. Creating new recovery session.",
            manifest.scan_session_id
        ));
        repo::start_session(conn, &started_at_now(), "resumed", "resumed")?
    };

    let jobs = plan_jobs(conn, config, manifest)?;
    let workers = crate::par::worker_count(config.workers);
    let total_to_archive = manifest.files.len();
    let mut months = run_jobs(
        conn,
        config,
        effective_session_id,
        &jobs,
        workers,
        total_to_archive,
        &progress,
    );
    months
        .archive_breakdown
        .sort_by(|a, b| a.volume_name.cmp(&b.volume_name));
    months.volume_names.sort();

    if let Some(first) = months.failures.into_iter().next() {
        // The manifest stays: the next run resumes the failed months.
        return Err(first);
    }

    progress.report(
        "Verification complete across all months. Cleaning up staging area...".to_string(),
        total_to_archive,
        total_to_archive,
    );

    let cleanup_errors = cleanup_staging(config, &progress)?;

    let files_moved = months.staged_present.len();
    let barcodes_matched = months
        .staged_present
        .iter()
        .map(|e| e.barcode.as_str())
        .collect::<HashSet<_>>()
        .len();

    repo::complete_session(
        conn,
        effective_session_id,
        &started_at_now(),
        SessionStatus::Success,
        barcodes_matched as i64,
        files_moved as i64,
        cleanup_errors as i64,
    )?;

    Ok(SessionResult {
        scan_session_id: effective_session_id,
        barcodes_matched,
        files_moved,
        errors: cleanup_errors,
        volume_names: months.volume_names,
        total_files_scanned: 0,
        archive_breakdown: months.archive_breakdown,
        unmatched_barcodes: Vec::new(),
        // Every archived file's staged copy was deleted in `index_and_clean`.
        files_deleted: files_moved,
        durability_notes: months.durability_notes,
    })
}

/// One month's share of a staged batch.
struct VolumeJob {
    volume_name: String,
    store_files: Vec<StoreFile>,
    staged_present: Vec<StagedManifestEntry>,
    indexed_head: Option<(String, u64)>,
}

/// From a month worker to the thread that owns the index.
enum VolumeMsg {
    Progress(String),
    Done(usize, Box<Result<mochi_store::FinishedVolume>>),
}

/// Groups the manifest by month (in month order). Only files still present in
/// staging are archived (resume tolerance); months with none are dropped.
fn plan_jobs(
    conn: &Connection,
    config: &SessionConfig,
    manifest: &StagingManifest,
) -> Result<Vec<VolumeJob>> {
    let mut by_volume: BTreeMap<String, Vec<&StagedManifestEntry>> = BTreeMap::new();
    for entry in &manifest.files {
        by_volume
            .entry(entry.volume_name.clone())
            .or_default()
            .push(entry);
    }

    let mut jobs: Vec<VolumeJob> = Vec::new();
    for (volume_name, entries) in &by_volume {
        let mut job = VolumeJob {
            volume_name: volume_name.clone(),
            store_files: Vec::new(),
            staged_present: Vec::new(),
            // The head the index recorded after the last verified write.
            indexed_head: repo::volume_head(conn, volume_name)?,
        };
        for entry in entries {
            let staged_path = config.staging_dir.join(&entry.relative_path);
            if !staged_path.exists() {
                continue;
            }
            job.store_files.push(StoreFile {
                relative_path: entry.relative_path.clone(),
                staged_path,
            });
            job.staged_present.push((*entry).clone());
        }
        if !job.store_files.is_empty() {
            jobs.push(job);
        }
    }
    Ok(jobs)
}

/// What `run_jobs` collected, in completion order (unsorted).
struct MonthsOutcome {
    /// Entries of the months that were committed, verified, indexed and cleaned.
    staged_present: Vec<StagedManifestEntry>,
    volume_names: Vec<String>,
    archive_breakdown: Vec<VolumeBreakdown>,
    durability_notes: Vec<String>,
    /// A month that fails does not stop the others: its staged files and the
    /// manifest stay for the next run, which retries only what is left.
    failures: Vec<ArchiveError>,
}

/// Commits and verifies the months in parallel, indexing and cleaning each
/// one on this thread as it finishes.
///
/// Months are separate archives with separate writer locks, so several are
/// committed and verified at once. Workers never touch SQLite or delete
/// anything: they report back here, and this thread indexes a month and
/// then removes its staged copies.
fn run_jobs<F>(
    conn: &mut Connection,
    config: &SessionConfig,
    session_id: i64,
    jobs: &[VolumeJob],
    workers: usize,
    total_to_archive: usize,
    progress: &Progress<F>,
) -> MonthsOutcome
where
    F: Fn(String, usize, usize),
{
    let mut out = MonthsOutcome {
        staged_present: Vec::new(),
        volume_names: Vec::new(),
        archive_breakdown: Vec::new(),
        durability_notes: Vec::new(),
        failures: Vec::new(),
    };
    let parallel_months = (workers / 2).clamp(1, jobs.len().max(1));
    let per_month_workers = (workers / parallel_months).max(1);
    let clean_workers = per_month_workers.max(workers / 2);
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::channel::<VolumeMsg>();
        for _ in 0..parallel_months {
            let sender = sender.clone();
            let next = &next;
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.get(i) else { break };
                let say = |m: String| {
                    let _ = sender.send(VolumeMsg::Progress(m));
                };
                let outcome = commit_and_verify(config, job, per_month_workers, &say);
                let _ = sender.send(VolumeMsg::Done(i, Box::new(outcome)));
            });
        }
        drop(sender);

        // Files of the months finished so far (only this thread counts).
        let mut done_files = 0usize;
        for msg in receiver {
            let processed = done_files;
            match msg {
                VolumeMsg::Progress(m) => progress.report(m, processed, total_to_archive),
                VolumeMsg::Done(i, outcome) => {
                    let job = &jobs[i];
                    done_files += job.store_files.len();
                    let indexed = (*outcome).and_then(|finished| {
                        index_and_clean(conn, session_id, job, &finished, clean_workers, progress)
                            .map(|()| finished)
                    });
                    match indexed {
                        Ok(finished) => {
                            if let Some(why) = &finished.degraded_durability {
                                out.durability_notes
                                    .push(format!("{}: {why}", job.volume_name));
                            }
                            out.archive_breakdown.push(VolumeBreakdown {
                                volume_name: job.volume_name.clone(),
                                files_archived: job.staged_present.len(),
                                total_bytes: job
                                    .staged_present
                                    .iter()
                                    .map(|e| e.original_size)
                                    .sum(),
                            });
                            out.volume_names.push(job.volume_name.clone());
                            out.staged_present
                                .extend(job.staged_present.iter().cloned());
                        }
                        Err(e) => {
                            progress.report(
                                format!(
                                    "ERROR: {}.mochi: {e}. Its {} staged file(s) are kept in staging for the next run.",
                                    job.volume_name,
                                    job.store_files.len()
                                ),
                                processed,
                                total_to_archive,
                            );
                            out.failures.push(e);
                        }
                    }
                }
            }
        }
    });
    out
}

/// Final cleanup after every month succeeded. Each month's verified files
/// were already deleted right after it was indexed; this removes the manifest,
/// then directories that are now empty. Anything else found in staging (e.g.
/// a file renamed in by a crashed run whose manifest line was lost) is never
/// deleted: it stays, is counted as an error and reported. Returns that count.
fn cleanup_staging<F>(config: &SessionConfig, progress: &Progress<F>) -> Result<usize>
where
    F: Fn(String, usize, usize),
{
    remove_file_if_present(&StagingManifest::path_in(&config.staging_dir))?;
    let mut unlisted: Vec<PathBuf> = Vec::new();
    prune_empty_dirs(&config.staging_dir, &mut unlisted);
    let cleanup_errors = unlisted.len();
    if cleanup_errors > 0 {
        progress.say(format!(
            "Warning: {cleanup_errors} file(s) in {} were not listed in the staging manifest and were NOT archived or deleted (first: {}). They are left in place; review them manually.",
            config.staging_dir.display(),
            unlisted[0].display()
        ));
    }
    Ok(cleanup_errors)
}

/// Commits one month's staged files and verifies them (no index writes, no
/// deletes: safe on a worker thread).
fn commit_and_verify(
    config: &SessionConfig,
    job: &VolumeJob,
    workers: usize,
    say: &dyn Fn(String),
) -> Result<mochi_store::FinishedVolume> {
    let volume_name = &job.volume_name;
    let volume_path = mochi_store::volume_path(&config.output_dir, volume_name);
    let n = job.store_files.len();

    say(format!("Committing {n} file(s) to {volume_name}.mochi..."));
    let finished = mochi_store::store_volume_with(
        &volume_path,
        &job.store_files,
        &mochi_store::StoreOptions {
            workers,
            ..mochi_store::StoreOptions::default()
        },
    )?;

    // The head before this run's commits was verified by an earlier run if
    // it is the head the index recorded (the index is written only after a
    // successful verification). Then only the new commits need the
    // structural check; otherwise the whole archive is checked.
    let since = finished.prior_head.filter(|prior| {
        job.indexed_head
            .as_ref()
            .is_some_and(|(id, seq)| *id == prior.commit_id.to_hex() && *seq == prior.seq)
    });
    say(format!(
        "Verifying {n} file(s) in {volume_name}.mochi (read-only, {})...",
        if since.is_some() {
            "new commits"
        } else {
            "whole archive"
        }
    ));
    // Verify before deleting any originals. Any failure aborts here and
    // leaves this month's staged files and the manifest untouched.
    mochi_store::verify_volume_with(
        &volume_path,
        &job.store_files,
        since.as_ref(),
        workers,
        |done| {
            say(format!(
                "Verified {done} of {n} file(s) in {volume_name}.mochi..."
            ))
        },
    )?;
    if let Some(why) = &finished.degraded_durability {
        // Committed (LOCAL_COMMITTED) but the directory entry is not confirmed durable.
        say(format!("DEGRADED (not an error): {volume_name}.mochi is committed (LOCAL_COMMITTED), but durability of its directory entry is unconfirmed: {why}. Keep an independent copy."));
    }
    if finished.entry_count < job.staged_present.len() {
        return Err(ArchiveError::VerificationFailed {
            volume: volume_name.clone(),
            reason: format!(
                "committed {} entries but expected {}. Aborting cleanup.",
                finished.entry_count,
                job.staged_present.len()
            ),
        });
    }
    Ok(finished)
}

/// Indexes a committed and verified month, then deletes its staged copies.
fn index_and_clean<F>(
    conn: &mut Connection,
    session_id: i64,
    job: &VolumeJob,
    finished: &mochi_store::FinishedVolume,
    workers: usize,
    progress: &Progress<F>,
) -> Result<()>
where
    F: Fn(String, usize, usize),
{
    let volume_name = &job.volume_name;
    let n = job.store_files.len();
    let now = started_at_now();
    let lookup: HashMap<&str, (&str, &str)> = job
        .staged_present
        .iter()
        .map(|e| {
            (
                e.relative_path.as_str(),
                (e.barcode.as_str(), e.source_subfolder.as_str()),
            )
        })
        .collect();
    progress.say(format!(
        "Indexing {} file(s) of {volume_name}.mochi...",
        finished.written_entries.len()
    ));
    repo::record_finished_volume(
        conn,
        session_id,
        volume_name,
        finished,
        &now,
        |path| lookup.get(path).map(|(b, _)| b.to_string()),
        |path| {
            lookup
                .get(path)
                .map(|(_, s)| s.to_string())
                .unwrap_or_default()
        },
    )?;

    // Committed, verified and indexed: these staged copies may go. The
    // manifest still lists them; a resume skips listed files that are gone.
    let results = crate::par::map(
        &job.store_files,
        workers,
        |f| remove_file_if_present(&f.staged_path),
        |done| {
            progress.say(format!(
                "Removed {done} of {n} staged file(s) of {volume_name}..."
            ))
        },
    );
    results.into_iter().collect::<Result<Vec<()>>>()?;
    Ok(())
}

/// Deletes a file; one that is already gone is not an error.
fn remove_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(ArchiveError::io(path, err)),
    }
}

/// Removes empty directories under (and including) `dir`, bottom-up. Files that
/// remain are appended to `left`; they are never touched.
fn prune_empty_dirs(dir: &Path, left: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => prune_empty_dirs(&p, left),
            _ => left.push(p),
        }
    }
    let _ = fs::remove_dir(dir); // fails (harmlessly) if not empty
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

    /// Tests that the jsonl manifest can be written and read back correctly.
    #[test]
    fn manifest_jsonl_write_then_read_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let staging_dir = tmp.path();

        let mut writer = StagingManifestWriter::create(staging_dir, 42).unwrap();
        for i in 0..5 {
            writer
                .record(StagedManifestEntry {
                    relative_path: format!("BURNIN/unit_BC{i:03}_log.txt"),
                    source_subfolder: "BURNIN".to_string(),
                    barcode: format!("BC{i:03}"),
                    original_size: 100 + i as u64,
                    volume_name: "2024-07".to_string(),
                })
                .unwrap();
        }
        writer.finish().unwrap();

        let read_back = StagingManifest::read_if_exists(staging_dir)
            .unwrap()
            .unwrap();
        assert_eq!(read_back.scan_session_id, 42);
        assert_eq!(read_back.files.len(), 5);
        assert_eq!(read_back.files[0].barcode, "BC000");
        assert_eq!(read_back.files[4].barcode, "BC004");
    }

    /// Simulates a crash mid-write. Verifies the reader safely ignores
    /// incomplete lines without failing the entire operation.
    #[test]
    fn manifest_jsonl_skips_truncated_trailing_line() {
        let tmp = tempfile::tempdir().unwrap();
        let staging_dir = tmp.path();

        let mut writer = StagingManifestWriter::create(staging_dir, 7).unwrap();
        writer
            .record(StagedManifestEntry {
                relative_path: "BURNIN/unit_BC001_log.txt".to_string(),
                source_subfolder: "BURNIN".to_string(),
                barcode: "BC001".to_string(),
                original_size: 100,
                volume_name: "2024-07".to_string(),
            })
            .unwrap();
        writer.finish().unwrap();

        // Write a malformed/partial entry to simulate a crash mid-save.
        let path = StagingManifest::path_in(staging_dir);
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "{{\"relative_path\":\"BURNIN/unit_BC002_l").unwrap();
        drop(f);

        let read_back = StagingManifest::read_if_exists(staging_dir)
            .unwrap()
            .unwrap();
        assert_eq!(read_back.scan_session_id, 7);
        assert_eq!(
            read_back.files.len(),
            1,
            "truncated trailing entry must be dropped, not fail the read"
        );
        assert_eq!(read_back.files[0].barcode, "BC001");
    }

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
        let mut f = fs::File::create(staging_dir.join(staged_relative_path)).unwrap();
        f.write_all(b"orphan test data").unwrap();
        drop(f);

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

        let result =
            finish_staged_batch::<fn(String, usize, usize)>(&mut conn, &config, &manifest, None);
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
