//! Background Worker Thread
//! Handles heavy tasks (DB queries, file archiving) off the main UI thread so 
//! the window remains responsive. Communication happens via message channels (`Command` in, `Progress`/`Outcome` out).

use archiver_core::db::{self, DbConfig};
use archiver_core::email_report::{self, EmailConfig, SessionStats};
use archiver_core::reader::{ExtractedFile, VolumeEntry};
use archiver_core::repo;
use archiver_core::schema::open_and_migrate;
use archiver_core::session::{run_session, SessionConfig};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

/// Instructions sent from the UI to this worker thread.
pub enum Command {
    /// Scans and archives files (or tests without modifying files if `dry_run` is true).
    RunSession {
        db_path: PathBuf,
        db_config: DbConfig,
        session_config: SessionConfig,
        dry_run: bool,
        /// Optional email setup to send run results.
        email_config: Option<EmailConfig>,
    },
    /// Searches and extracts files matching a barcode or filename substring.
    Extract {
        db_path: PathBuf,
        query: String,
        dest_dir: PathBuf,
    },
    /// Searches and lists matching files without extracting them.
    Search { db_path: PathBuf, query: String },

    /// Lists all entries inside a compressed `.tar.zst` volume.
    ListVolume { volume_path: PathBuf },
    /// Extracts a single file from a volume.
    ExtractFromVolume {
        volume_path: PathBuf,
        relative_path: String,
        dest_dir: PathBuf,
    },
    /// Extracts all files from a volume.
    ExtractAllFromVolume { volume_path: PathBuf, dest_dir: PathBuf },
    /// Reads a small portion of a file into memory for inline UI previews.
    PreviewFromVolume { volume_path: PathBuf, relative_path: String },
}

/// Live status updates sent back to update the UI progress bar.
#[derive(Debug, Clone)]
pub enum Progress {
    Status(String),
    /// Current count vs total count (e.g., 10 of 50 files completed).
    Fraction(u32, u32),
}

/// Final results returned to the UI after a command finishes.
#[derive(Debug)]
pub enum Outcome {
    SessionDone(Result<SessionSummary, String>),
    DryRunDone(Result<DryRunSummary, String>),
    ExtractDone(Result<Vec<ExtractedFile>, String>),
    SearchDone(Result<Vec<repo::LogEntryLocation>, String>),

    VolumeListed(Result<Vec<VolumeEntry>, String>),
    VolumeFileExtracted(Result<ExtractedFile, String>),
    VolumeExtractAllDone(Result<Vec<ExtractedFile>, String>),
    VolumePreviewDone(Result<VolumePreview, String>),
}

/// Holds preview bytes loaded into memory for quick viewing.
#[derive(Debug, Clone)]
pub struct VolumePreview {
    pub relative_path: String,
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

/// Limits preview reads to 512 KB to avoid excessive memory usage.
const PREVIEW_MAX_BYTES: u64 = 512 * 1024;

/// Final summary metrics sent to the UI after a real archive session.
#[derive(Debug)]
pub struct SessionSummary {
    pub barcodes_matched: usize,
    pub files_moved: usize,
    pub errors: usize,
    /// Monthly archive volumes touched during this run.
    pub volume_names: Vec<String>,
    pub total_files_scanned: usize,
    pub archive_breakdown: Vec<email_report::ArchiveBreakdownRow>,
    pub unmatched_barcodes: Vec<String>,
    pub files_deleted: usize,
    pub db_records_retrieved: usize,
    pub duration_secs: f64,
    /// Up to date scan timestamp so the UI can update its cache without re-querying the DB.
    pub scan_to_date: String,
}

/// Final summary metrics sent to the UI after a test run.
#[derive(Debug)]
pub struct DryRunSummary {
    pub barcodes_matched: usize,
    pub files_that_would_move: usize,
    pub matched_filenames: Vec<String>,
}

/// Spawns the worker thread and returns channel endpoints for sending commands and receiving status/results.
pub fn spawn() -> (Sender<Command>, Receiver<Progress>, Receiver<Outcome>) {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Command>();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel::<Progress>();
    let (outcome_tx, outcome_rx) = std::sync::mpsc::channel::<Outcome>();

    std::thread::spawn(move || {
        // Single-threaded async engine; processes commands sequentially.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build background tokio runtime");

        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                Command::RunSession { db_path, db_config, session_config, dry_run, email_config } => {
                    let result = rt.block_on(handle_run_session(
                        &db_path,
                        &db_config,
                        &session_config,
                        dry_run,
                        email_config,
                        &progress_tx,
                    ));
                    let _ = outcome_tx.send(if dry_run {
                        Outcome::DryRunDone(result.map(|s| s.into_dry_run()))
                    } else {
                        Outcome::SessionDone(result.map(|s| s.into_session()))
                    });
                }
                Command::Extract { db_path, query, dest_dir } => {
                    let _ = progress_tx.send(Progress::Status(format!("Extracting matches for {query:?}...")));
                    let result = extract(&db_path, &query, &dest_dir, &progress_tx);
                    let _ = outcome_tx.send(Outcome::ExtractDone(result));
                }
                Command::Search { db_path, query } => {
                    let _ = progress_tx.send(Progress::Status(format!("Searching for {query:?}...")));
                    let result = search(&db_path, &query);
                    let _ = outcome_tx.send(Outcome::SearchDone(result));
                }
                Command::ListVolume { volume_path } => {
                    let _ = progress_tx.send(Progress::Status(format!(
                        "Reading {}...",
                        volume_path.display()
                    )));
                    let result = archiver_core::reader::list_volume_entries(&volume_path, |n| {
                        let _ = progress_tx.send(Progress::Status(format!("Found {n} file(s) so far...")));
                    })
                    .map_err(|e| e.to_string());
                    let _ = outcome_tx.send(Outcome::VolumeListed(result));
                }
                Command::ExtractFromVolume { volume_path, relative_path, dest_dir } => {
                    let _ = progress_tx.send(Progress::Status(format!("Extracting {relative_path}...")));
                    let dest_path = dest_dir.join(&relative_path);
                    let result = archiver_core::reader::extract_entry_from_volume(&volume_path, &relative_path, &dest_path)
                        .map(|bytes_written| ExtractedFile {
                            relative_path: relative_path.clone(),
                            dest_path,
                            bytes_written,
                        })
                        .map_err(|e| e.to_string());
                    let _ = outcome_tx.send(Outcome::VolumeFileExtracted(result));
                }
                Command::ExtractAllFromVolume { volume_path, dest_dir } => {
                    let _ = progress_tx.send(Progress::Status("Extracting all files...".to_string()));
                    let result = archiver_core::reader::extract_all_from_volume(&volume_path, &dest_dir, |n| {
                        let _ = progress_tx.send(Progress::Status(format!("Extracted {n} file(s) so far...")));
                    })
                    .map_err(|e| e.to_string());
                    let _ = outcome_tx.send(Outcome::VolumeExtractAllDone(result));
                }
                Command::PreviewFromVolume { volume_path, relative_path } => {
                    let _ = progress_tx.send(Progress::Status(format!("Loading preview of {relative_path}...")));
                    let result = archiver_core::reader::preview_entry_from_volume(&volume_path, &relative_path, PREVIEW_MAX_BYTES)
                        .map(|(bytes, truncated)| VolumePreview {
                            relative_path: relative_path.clone(),
                            bytes,
                            truncated,
                        })
                        .map_err(|e| e.to_string());
                    let _ = outcome_tx.send(Outcome::VolumePreviewDone(result));
                }
            }
        }
    });

    (cmd_tx, progress_rx, outcome_rx)
}

/// Shared internal structure for converting pipeline outputs into real or dry-run summaries.
struct CombinedResult {
    barcodes_matched: usize,
    files_moved_or_matched: usize,
    errors: usize,
    volume_names: Vec<String>,
    matched_filenames: Vec<String>,
    total_files_scanned: usize,
    archive_breakdown: Vec<email_report::ArchiveBreakdownRow>,
    unmatched_barcodes: Vec<String>,
    files_deleted: usize,
    db_records_retrieved: usize,
    duration_secs: f64,
    scan_to_date: String,
}

impl CombinedResult {
    fn into_session(self) -> SessionSummary {
        SessionSummary {
            barcodes_matched: self.barcodes_matched,
            files_moved: self.files_moved_or_matched,
            errors: self.errors,
            volume_names: self.volume_names,
            total_files_scanned: self.total_files_scanned,
            archive_breakdown: self.archive_breakdown,
            unmatched_barcodes: self.unmatched_barcodes,
            files_deleted: self.files_deleted,
            db_records_retrieved: self.db_records_retrieved,
            duration_secs: self.duration_secs,
            scan_to_date: self.scan_to_date,
        }
    }
    fn into_dry_run(self) -> DryRunSummary {
        DryRunSummary {
            barcodes_matched: self.barcodes_matched,
            files_that_would_move: self.files_moved_or_matched,
            matched_filenames: self.matched_filenames,
        }
    }
}

async fn handle_run_session(
    db_path: &PathBuf,
    db_config: &DbConfig,
    session_config: &SessionConfig,
    dry_run: bool,
    email_config: Option<EmailConfig>,
    progress_tx: &Sender<Progress>,
) -> Result<CombinedResult, String> {
    let run_started = std::time::Instant::now();
    let started_label = human_timestamp_now();

    let _ = progress_tx.send(Progress::Status("Loading index database...".into()));
    let mut conn = open_and_migrate(db_path).map_err(|e| e.to_string())?;

    let _ = progress_tx.send(Progress::Status("Querying SQL Server for archival candidates...".into()));
    let (from_date, to_date) = if let Some((override_from, override_to)) = &session_config.date_range_override {
        // Use manual date range if supplied by the UI.
        let from_date = if override_from.trim().is_empty() {
            None
        } else {
            Some(override_from.trim().to_string())
        };
        let _ = progress_tx.send(
            Progress::Status(format!(
                "Using manually overridden scan range ({} to {}).",
                from_date.as_deref().unwrap_or("(no lower bound)"),
                override_to.trim()
            ))
        );
        (from_date, override_to.trim().to_string())
    } else {
        // Compute resume window automatically from last successful scan.
        let last_scan = repo::last_successful_scan_to_date(&conn).map_err(|e| e.to_string())?;
        db::compute_resume_window(last_scan.as_deref(), session_config.lookback_days).map_err(|e| e.to_string())?
    };

    let candidates = db::query_archival_candidates(db_config, from_date.as_deref(), &to_date)
        .await
        .map_err(|e| e.to_string())?;
    let db_records_retrieved = candidates.len();
    let _ = progress_tx.send(Progress::Status(format!("Found {} archival candidates.", db_records_retrieved)));

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let started_at = format!("epoch:{now}");

    // Helper closure to send status text and numerical progress to the UI.
    let progress_cb = |msg: String, done: usize, total: usize| {
        let _ = progress_tx.send(Progress::Status(msg));
        if total > 0 {
            let _ = progress_tx.send(Progress::Fraction(done as u32, total as u32));
        }
    };

    if dry_run {
        // DRY RUN: Scans local files and reports matches without modifying files or archives.
        if candidates.is_empty() {
            progress_cb("No archival candidates found for this window. Nothing to do!".to_string(), 0, 0);
            return Ok(CombinedResult {
                barcodes_matched: 0,
                files_moved_or_matched: 0,
                errors: 0,
                volume_names: Vec::new(),
                matched_filenames: Vec::new(),
                total_files_scanned: 0,
                archive_breakdown: Vec::new(),
                unmatched_barcodes: Vec::new(),
                files_deleted: 0,
                db_records_retrieved,
                duration_secs: run_started.elapsed().as_secs_f64(),
                scan_to_date: to_date.clone(),
            });
        }

        progress_cb(format!("Building search index for {} candidates (dry run)...", candidates.len()), 0, 0);

        let barcodes: Vec<String> = candidates.iter().map(|c| c.barcode.clone()).collect();
        let matcher = archiver_core::matcher::BarcodeMatcher::new(barcodes.clone());

        let mut matched_filenames = Vec::new();
        let mut matched_barcodes = std::collections::HashSet::new();
        let mut total_files_scanned = 0usize;

        for subfolder in &session_config.subfolders {
            let dir = session_config.source_dir.join(subfolder);
            
            if !dir.is_dir() {
                progress_cb(format!("Warning: configured subfolder '{}' not found at {} — skipping.", subfolder, dir.display()), 0, 0);
                continue;
            }

            progress_cb(format!("Scanning subfolder: {}...", subfolder), 0, 0);
            
            let read_dir = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) => {
                    progress_cb(format!("Warning: failed to read subfolder '{}': {} — skipping.", subfolder, e), 0, 0);
                    continue;
                }
            };

            for entry in read_dir.flatten() {
                total_files_scanned += 1;
                if total_files_scanned % 5000 == 0 {
                    progress_cb(format!("Scanned {} files so far (found {} matches)...", total_files_scanned, matched_filenames.len()), 0, 0);
                }

                let path = entry.path();
                if !path.is_file() { continue; }
                
                let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                if !archiver_core::matcher::has_matching_extension(&filename, &session_config.allowed_extensions) { continue; }
                
                let matches = matcher.matching_barcodes(&filename);
                if matches.len() == 1 {
                    matched_barcodes.extend(matches);
                    matched_filenames.push(format!("{subfolder}/{filename}"));
                }
            }
        }

        progress_cb(format!("Finished scanning {} files. Found {} matches.", total_files_scanned, matched_filenames.len()), 0, 0);

        return Ok(CombinedResult {
            barcodes_matched: matched_barcodes.len(),
            files_moved_or_matched: matched_filenames.len(),
            errors: 0,
            volume_names: Vec::new(),
            matched_filenames,
            total_files_scanned,
            archive_breakdown: Vec::new(),
            unmatched_barcodes: Vec::new(),
            files_deleted: 0,
            db_records_retrieved,
            duration_secs: run_started.elapsed().as_secs_f64(),
            scan_to_date: to_date.clone(),
        });
    }

    // REAL RUN: Full execution including scanning, archiving, and cleanup.
    let _ = progress_tx.send(Progress::Status("Starting archive session...".into()));
    let session_run = run_session(
        &mut conn,
        session_config,
        candidates,
        from_date.as_deref().unwrap_or(""),
        &to_date,
        &started_at,
        Some(&progress_cb),
    );

    let duration_secs = run_started.elapsed().as_secs_f64();

    let result = match session_run {
        Ok(result) => result,
        Err(e) => {
            let error_message = e.to_string();
            if let Some(cfg) = &email_config {
                let stats = SessionStats {
                    started_label: started_label.clone(),
                    duration_secs,
                    db_records_retrieved,
                    total_source_files_scanned: 0,
                    total_files_matched: 0,
                    total_files_archived: 0,
                    total_files_deleted: 0,
                    archive_breakdown: Vec::new(),
                    unmatched_barcodes: Vec::new(),
                    errors: 0,
                    failed: Some(error_message.clone()),
                };
                send_stats_email_best_effort(cfg, &stats, progress_tx).await;
            }
            return Err(error_message);
        }
    };

    let _ = progress_tx.send(Progress::Status("Done.".into()));

    if let Some(cfg) = &email_config {
        let stats = SessionStats {
            started_label,
            duration_secs,
            db_records_retrieved,
            total_source_files_scanned: result.total_files_scanned,
            total_files_matched: result.barcodes_matched,
            total_files_archived: result.files_moved,
            total_files_deleted: result.files_deleted,
            archive_breakdown: result
                .archive_breakdown
                .iter()
                .map(|v| email_report::ArchiveBreakdownRow {
                    volume_name: v.volume_name.clone(),
                    files_archived: v.files_archived,
                    total_bytes: v.total_bytes,
                })
                .collect(),
            unmatched_barcodes: result.unmatched_barcodes.clone(),
            errors: result.errors,
            failed: None,
        };
        send_stats_email_best_effort(cfg, &stats, progress_tx).await;
    }

    Ok(CombinedResult {
        barcodes_matched: result.barcodes_matched,
        files_moved_or_matched: result.files_moved,
        errors: result.errors,
        volume_names: result.volume_names,
        matched_filenames: Vec::new(),
        total_files_scanned: result.total_files_scanned,
        archive_breakdown: result
            .archive_breakdown
            .into_iter()
            .map(|v| email_report::ArchiveBreakdownRow {
                volume_name: v.volume_name,
                files_archived: v.files_archived,
                total_bytes: v.total_bytes,
            })
            .collect(),
        unmatched_barcodes: result.unmatched_barcodes,
        files_deleted: result.files_deleted,
        db_records_retrieved,
        duration_secs,
        scan_to_date: to_date.clone(),
    })
}

/// Tries sending an email report; logs errors to the UI without failing the run itself.
async fn send_stats_email_best_effort(config: &EmailConfig, stats: &SessionStats, progress_tx: &Sender<Progress>) {
    let _ = progress_tx.send(Progress::Status("Sending statistics email...".into()));
    let subject = email_report::default_subject(stats);
    match email_report::send_stats_email(config, &subject, stats).await {
        Ok(()) => {
            let _ = progress_tx.send(Progress::Status(format!("Statistics email sent to {}.", config.recipient)));
        }
        Err(e) => {
            let _ = progress_tx.send(Progress::Status(format!("Failed to send statistics email: {e}")));
        }
    }
}

/// Generates a basic UTC epoch timestamp string without needing external date crates.
fn human_timestamp_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("epoch:{secs} UTC")
}

/// Extracts matching files by barcode first; falls back to matching filename substrings.
fn extract(db_path: &PathBuf, query: &str, dest_dir: &PathBuf, progress_tx: &Sender<Progress>) -> Result<Vec<ExtractedFile>, String> {
    let conn = open_and_migrate(db_path).map_err(|e| e.to_string())?;
    let by_barcode = archiver_core::reader::extract_by_barcode(&conn, query, dest_dir).map_err(|e| e.to_string())?;
    if !by_barcode.is_empty() {
        let _ = progress_tx.send(Progress::Fraction(by_barcode.len() as u32, by_barcode.len() as u32));
        return Ok(by_barcode);
    }

    let locations = repo::find_entries_by_filename_like(&conn, query).map_err(|e| e.to_string())?;
    let total = locations.len() as u32;
    let mut extracted = Vec::with_capacity(locations.len());
    for (i, loc) in locations.into_iter().enumerate() {
        let dest_path = dest_dir.join(&loc.relative_path);
        let bytes_written = archiver_core::reader::extract_file_from_chunk(
            std::path::Path::new(&loc.volume_path),
            loc.chunk_start_byte,
            loc.chunk_end_byte,
            &loc.relative_path,
            &dest_path,
        )
        .map_err(|e| e.to_string())?;
        extracted.push(ExtractedFile {
            relative_path: loc.relative_path,
            dest_path,
            bytes_written,
        });
        let _ = progress_tx.send(Progress::Fraction(i as u32 + 1, total));
    }
    Ok(extracted)
}

/// Searches the index DB by exact barcode or partial filename.
fn search(db_path: &PathBuf, query: &str) -> Result<Vec<repo::LogEntryLocation>, String> {
    let conn = open_and_migrate(db_path).map_err(|e| e.to_string())?;
    let by_barcode = repo::find_entries_by_barcode(&conn, query).map_err(|e| e.to_string())?;
    if !by_barcode.is_empty() {
        return Ok(by_barcode);
    }
    repo::find_entries_by_filename_like(&conn, query).map_err(|e| e.to_string())
}