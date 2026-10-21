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
use std::path::{Path, PathBuf};

/// A single file moved to the staging area, waiting to be archived.
#[derive(Debug, Clone)]
pub struct StagedFile {
    pub staged_path: PathBuf,
    pub relative_path: String,
    pub source_subfolder: String,
    pub barcode: String,
    pub original_size: u64,
}

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

        Ok(Some(StagingManifest { scan_session_id, files }))
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
    const FLUSH_EVERY: usize = 500;

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
        use std::io::Write as _;
        writeln!(self.file, "{json}").map_err(|e| ArchiveError::io("manifest.jsonl", e))
    }

    /// Forces unsaved lines to be written to disk.
    fn flush(&mut self) -> Result<()> {
        use std::io::Write as _;
        self.file
            .flush()
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
}

/// Summary statistics for a completed archiving session.
#[derive(Debug)]
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
}

/// Statistics for a specific monthly volume. (For reporting only).
#[derive(Debug, Clone)]
pub struct VolumeBreakdown {
    pub volume_name: String,
    pub files_archived: usize,
    pub total_bytes: u64,
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
    // 1. Resume interrupted batch: If a previous run crashed leaving files 
    // in staging, finish archiving them before starting a new scan.
    if let Some(manifest) = StagingManifest::read_if_exists(&config.staging_dir)? {
        if let Some(cb) = progress_callback {
            cb("Found interrupted session. Resuming archive of staged files...".to_string(), 0, 0);
        }
        finish_staged_batch(conn, config, &manifest, progress_callback)?;
    }

    let session_id = repo::start_session(conn, started_at, query_from_date, query_to_date)?;

    if let Some(cb) = progress_callback {
        cb("Checking candidates against SQLite index...".to_string(), 0, 0);
    }

    if candidates.is_empty() {
        if let Some(cb) = progress_callback {
            cb("No archival candidates found for this window. Nothing to do!".to_string(), 0, 0);
        }
        repo::complete_session(conn, session_id, started_at, SessionStatus::Success, 0, 0, 0)?;
        return Ok(SessionResult {
            scan_session_id: session_id,
            barcodes_matched: 0,
            files_moved: 0,
            errors: 0,
            volume_names: Vec::new(),
            total_files_scanned: 0,
            archive_breakdown: Vec::new(),
            unmatched_barcodes: Vec::new(),
            files_deleted: 0,
        });
    }

    if let Some(cb) = progress_callback {
        cb(format!("Building search index for {} candidates (this may take a moment)...", candidates.len()), 0, 0);
    }

    // 2. Build the search index using ALL candidates, even previously scanned ones,
    // so we don't miss secondary files (like a log file that arrived late).
    let barcodes: Vec<String> = candidates.iter().map(|c| c.barcode.clone()).collect();
    let matcher = BarcodeMatcher::new(barcodes.clone());

    let candidate_date: HashMap<&str, &str> = candidates
        .iter()
        .map(|c| (c.barcode.as_str(), c.last_update.as_str()))
        .collect();

    let already_scanned: HashSet<&str> = {
        let mut set = HashSet::new();
        for c in &candidates {
            if repo::is_barcode_already_scanned(conn, &c.barcode, &c.last_update)? {
                set.insert(c.barcode.as_str());
            }
        }
        set
    };
    let fresh_candidates: Vec<&ArchivalCandidate> = candidates
        .iter()
        .filter(|c| !already_scanned.contains(c.barcode.as_str()))
        .collect();

    let mut manifest_entries = Vec::new();
    let mut errors = 0i64;
    let mut total_files_scanned = 0usize;

    for subfolder in &config.subfolders {
        let subfolder_dir = config.source_dir.join(subfolder);
        if !subfolder_dir.is_dir() {
            if let Some(cb) = progress_callback {
                cb(format!("Warning: configured subfolder '{subfolder}' not found — skipping."), 0, 0);
            }
            errors += 1;
            continue;
        }

        if let Some(cb) = progress_callback {
            cb(format!("Scanning subfolder: {}...", subfolder), 0, 0);
        }

        // Catch folder read errors safely so one bad folder doesn't crash the whole run.
        let read_dir = match fs::read_dir(&subfolder_dir) {
            Ok(rd) => rd,
            Err(e) => {
                if let Some(cb) = progress_callback {
                    cb(format!("Warning: failed to read subfolder '{subfolder}': {e} — skipping."), 0, 0);
                }
                errors += 1;
                continue;
            }
        };

        for entry in read_dir {
            total_files_scanned += 1;

            // Log to UI periodically to show the app isn't frozen.
            if total_files_scanned % 5000 == 0 {
                if let Some(cb) = progress_callback {
                    cb(format!("Scanned {} files so far (found {} matches)...", total_files_scanned, manifest_entries.len()), 0, 0);
                }
            }

            let entry = match entry {
                Ok(e) => e,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            if !has_matching_extension(&filename, &config.allowed_extensions) {
                continue;
            }

            let matched_barcodes = matcher.matching_barcodes(&filename);
            if matched_barcodes.len() > 1 {
                errors += 1; // File matched multiple barcodes, which shouldn't happen
                continue;
            }
            let Some(barcode) = matched_barcodes.into_iter().next() else {
                continue;
            };

            let Some(date) = candidate_date.get(barcode.as_str()) else {
                errors += 1;
                continue;
            };
            
            let volume_name = match volume_name_from_date(date) {
                Ok(v) => v,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };

            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            manifest_entries.push(StagedManifestEntry {
                relative_path: format!("{subfolder}/{filename}"),
                source_subfolder: subfolder.clone(),
                barcode,
                original_size: size,
                volume_name,
            });
        }
    }

    if let Some(cb) = progress_callback {
        cb(format!("Finished scanning {} files. Found {} matches.", total_files_scanned, manifest_entries.len()), 0, 0);
    }

    // Skip duplicate files resolving to the exact same path to prevent DB errors later.
    {
        let mut seen: HashSet<String> = HashSet::with_capacity(manifest_entries.len());
        let before = manifest_entries.len();
        manifest_entries.retain(|e| seen.insert(e.relative_path.clone()));
        let duplicates = before - manifest_entries.len();
        if duplicates > 0 {
            errors += duplicates as i64;
            if let Some(cb) = progress_callback {
                cb(format!("Warning: skipped {duplicates} duplicate files."), 0, 0);
            }
        }
    }

    let matched_barcode_set: HashSet<&str> =
        manifest_entries.iter().map(|e| e.barcode.as_str()).collect();

    let unmatched_barcodes: Vec<String> = fresh_candidates
        .iter()
        .filter(|c| !matched_barcode_set.contains(c.barcode.as_str()))
        .map(|c| c.barcode.clone())
        .collect();

    if let Some(cb) = progress_callback {
        cb("Recording barcode match statuses to database (fast insert)...".to_string(), 0, 0);
    }

    // Wrap database inserts in a single transaction for massive performance gains.
    conn.execute_batch("BEGIN TRANSACTION").map_err(|e| ArchiveError::Config(format!("DB error: {e}")))?;

    let total_candidates = fresh_candidates.len();
    for (i, c) in fresh_candidates.iter().enumerate() {
        if i % 50000 == 0 && i > 0 {
            if let Some(cb) = progress_callback {
                cb(format!("Recorded {}/{} barcode statuses to index...", i, total_candidates), i, total_candidates);
            }
        }
        let status = if matched_barcode_set.contains(c.barcode.as_str()) {
            BarcodeStatus::Matched
        } else {
            BarcodeStatus::NoFilesFound
        };
        repo::record_session_barcode(conn, session_id, &c.barcode, &c.last_update, status)?;
    }

    conn.execute_batch("COMMIT").map_err(|e| ArchiveError::Config(format!("DB error: {e}")))?;

    if manifest_entries.is_empty() {
        repo::complete_session(conn, session_id, started_at, SessionStatus::Success, fresh_candidates.len() as i64, 0, errors)?;
        return Ok(SessionResult {
            scan_session_id: session_id,
            barcodes_matched: 0,
            files_moved: 0,
            errors: errors as usize,
            volume_names: Vec::new(),
            total_files_scanned,
            archive_breakdown: Vec::new(),
            unmatched_barcodes,
            files_deleted: 0,
        });
    }

    // 3. Stage Files: Move matched files to the `/staging` directory 
    // and incrementally write their details to the manifest file.
    fs::create_dir_all(&config.staging_dir).map_err(|e| ArchiveError::io(&config.staging_dir, e))?;
    let mut manifest_writer = StagingManifestWriter::create(&config.staging_dir, session_id)?;

    let total_to_stage = manifest_entries.len();
    for (i, entry) in manifest_entries.iter().enumerate() {
        if i % 1000 == 0 || i == total_to_stage - 1 {
            if let Some(cb) = progress_callback {
                cb(format!("Staging file {} of {}...", i + 1, total_to_stage), i, total_to_stage);
            }
        }

        let source_path = config.source_dir.join(&entry.relative_path);
        let staged_path = config.staging_dir.join(&entry.relative_path);
        if let Some(parent) = staged_path.parent() {
            fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
        }

        fs::rename(&source_path, &staged_path).map_err(|e| ArchiveError::io(&staged_path, e))?;
        manifest_writer.record(entry.clone())?;
    }
    manifest_writer.finish()?;

    let manifest = StagingManifest {
        scan_session_id: session_id,
        files: manifest_entries.clone(),
    };

    let result = finish_staged_batch(conn, config, &manifest, progress_callback)?;

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
    })
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
    // If the database was wiped but the staging folder wasn't, the old 
    // session ID won't exist. Create a new dummy session to safely recover.
    let effective_session_id = if repo::session_exists(conn, manifest.scan_session_id)? {
        manifest.scan_session_id
    } else {
        if let Some(cb) = progress_callback {
            cb(format!("Warning: Batch references missing session {}. Creating new recovery session.", manifest.scan_session_id), 0, 0);
        }
        repo::start_session(conn, &started_at_now(), "resumed", "resumed")?
    };

    // Group files by month (e.g. "2024-08")
    let mut by_volume: BTreeMap<String, Vec<&StagedManifestEntry>> = BTreeMap::new();
    for entry in &manifest.files {
        by_volume.entry(entry.volume_name.clone()).or_default().push(entry);
    }

    let total_to_archive = manifest.files.len();
    let mut processed = 0usize;
    let mut all_staged_present: Vec<StagedManifestEntry> = Vec::new();
    let mut touched_volume_names = Vec::new();
    let mut archive_breakdown: Vec<VolumeBreakdown> = Vec::new();

    for (volume_name, entries) in &by_volume {
        let volume_path = mochi_store::volume_path(&config.output_dir, volume_name);

        // Only files still present in staging are archived (resume tolerance).
        let mut staged_present: Vec<StagedManifestEntry> = Vec::new();
        let mut store_files: Vec<StoreFile> = Vec::new();
        for entry in entries {
            processed += 1;
            if processed % 250 == 0 || processed == total_to_archive {
                if let Some(cb) = progress_callback {
                    cb(format!("Preparing file {} of {} ({volume_name})...", processed, total_to_archive), processed, total_to_archive);
                }
            }
            let staged_path = config.staging_dir.join(&entry.relative_path);
            if !staged_path.exists() {
                continue;
            }
            store_files.push(StoreFile {
                relative_path: entry.relative_path.clone(),
                staged_path,
            });
            staged_present.push((*entry).clone());
        }
        if store_files.is_empty() {
            continue;
        }

        if let Some(cb) = progress_callback {
            cb(format!("Committing {} file(s) to {volume_name}.mochi...", store_files.len()), processed, total_to_archive);
        }
        let finished = mochi_store::store_volume(&volume_path, &store_files)?;

        if let Some(cb) = progress_callback {
            cb(format!("Verifying committed archive {volume_name}.mochi (read-only)..."), processed, total_to_archive);
        }
        // Verify before deleting any originals. Any failure aborts here and
        // leaves staging and the manifest untouched.
        mochi_store::verify_volume(&volume_path, &store_files)?;
        if finished.entry_count < staged_present.len() {
            return Err(ArchiveError::VerificationFailed {
                volume: volume_name.clone(),
                reason: format!(
                    "committed {} entries but expected {}. Aborting cleanup.",
                    finished.entry_count,
                    staged_present.len()
                ),
            });
        }

        let now = started_at_now();
        let barcode_lookup: HashMap<&str, &str> = staged_present
            .iter()
            .map(|e| (e.relative_path.as_str(), e.barcode.as_str()))
            .collect();
        let subfolder_lookup: HashMap<&str, &str> = staged_present
            .iter()
            .map(|e| (e.relative_path.as_str(), e.source_subfolder.as_str()))
            .collect();

        repo::record_finished_volume(
            conn,
            effective_session_id,
            volume_name,
            &finished,
            &now,
            |path| barcode_lookup.get(path).map(|s| s.to_string()),
            |path| subfolder_lookup.get(path).map(|s| s.to_string()).unwrap_or_default(),
        )?;

        let volume_bytes: u64 = staged_present.iter().map(|e| e.original_size).sum();
        archive_breakdown.push(VolumeBreakdown {
            volume_name: volume_name.clone(),
            files_archived: staged_present.len(),
            total_bytes: volume_bytes,
        });

        touched_volume_names.push(volume_name.clone());
        all_staged_present.extend(staged_present);
    }

    if let Some(cb) = progress_callback {
        cb("Verification complete across all months. Cleaning up staging area...".to_string(), total_to_archive, total_to_archive);
    }

    // 4. Cleanup: Now that everything is committed, verified, and saved to the DB,
    // we can safely delete the temporary staging folder.
    let files_deleted = all_staged_present.len();
    fs::remove_dir_all(&config.staging_dir).map_err(|e| ArchiveError::io(&config.staging_dir, e))?;

    repo::complete_session(
        conn,
        effective_session_id,
        &started_at_now(),
        SessionStatus::Success,
        all_staged_present
            .iter()
            .map(|e| e.barcode.as_str())
            .collect::<HashSet<_>>()
            .len() as i64,
        all_staged_present.len() as i64,
        0,
    )?;

    Ok(SessionResult {
        scan_session_id: effective_session_id,
        barcodes_matched: all_staged_present
            .iter()
            .map(|e| e.barcode.as_str())
            .collect::<HashSet<_>>()
            .len(),
        files_moved: all_staged_present.len(),
        errors: 0,
        volume_names: touched_volume_names,
        total_files_scanned: 0,
        archive_breakdown,
        unmatched_barcodes: Vec::new(),
        files_deleted,
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
    use std::io::Write as _;

    /// Tests that the jsonl manifest can be written and read back correctly.
    #[test]
    fn manifest_jsonl_write_then_read_round_trips() {
        let staging_dir = std::env::temp_dir().join(format!(
            "archiver_manifest_roundtrip_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&staging_dir).unwrap();

        let mut writer = StagingManifestWriter::create(&staging_dir, 42).unwrap();
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

        let read_back = StagingManifest::read_if_exists(&staging_dir).unwrap().unwrap();
        assert_eq!(read_back.scan_session_id, 42);
        assert_eq!(read_back.files.len(), 5);
        assert_eq!(read_back.files[0].barcode, "BC000");
        assert_eq!(read_back.files[4].barcode, "BC004");

        let _ = fs::remove_dir_all(&staging_dir);
    }

    /// Simulates a crash mid-write. Verifies the reader safely ignores 
    /// incomplete lines without failing the entire operation.
    #[test]
    fn manifest_jsonl_skips_truncated_trailing_line() {
        let staging_dir = std::env::temp_dir().join(format!(
            "archiver_manifest_truncated_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&staging_dir).unwrap();

        let mut writer = StagingManifestWriter::create(&staging_dir, 7).unwrap();
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
        let path = StagingManifest::path_in(&staging_dir);
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "{{\"relative_path\":\"BURNIN/unit_BC002_l").unwrap();
        drop(f);

        let read_back = StagingManifest::read_if_exists(&staging_dir).unwrap().unwrap();
        assert_eq!(read_back.scan_session_id, 7);
        assert_eq!(read_back.files.len(), 1, "truncated trailing entry must be dropped, not fail the read");
        assert_eq!(read_back.files[0].barcode, "BC001");

        let _ = fs::remove_dir_all(&staging_dir);
    }

    /// Tests that the archiver handles an orphaned session ID correctly 
    /// (e.g. if the database gets reset but files are still in staging).
    #[test]
    fn finish_staged_batch_recovers_from_orphaned_scan_session_id() {
        let mut conn = open_in_memory().unwrap();

        let staging_dir = std::env::temp_dir().join(format!(
            "archiver_orphan_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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
        };

        let result = finish_staged_batch::<fn(String, usize, usize)>(&mut conn, &config, &manifest, None);
        assert!(result.is_ok(), "Should successfully recover and not fail.");

        let outcome = result.unwrap();
        assert_ne!(outcome.scan_session_id, 999_999, "Should generate a brand new session ID.");
        assert!(repo::session_exists(&conn, outcome.scan_session_id).unwrap());
        assert_eq!(outcome.files_moved, 1);

        assert!(!staging_dir.join(staged_relative_path).exists());
        let _ = fs::remove_dir_all(&staging_dir);
    }
}