//! The archive run: SQL Server query → scan → stage → MOCHI commit → cleanup → report.
//!
//! Every stage is announced through [`Reporter`] so the frontend can light up
//! the matching node of its execution-flow diagram.

use archiver_core::db::{self, DbConfig};
use archiver_core::email_report::{self, ArchiveBreakdownRow, EmailConfig, SessionStats};
use archiver_core::matcher::{has_matching_extension, BarcodeMatcher};
use archiver_core::repo;
use archiver_core::schema::open_and_migrate;
use archiver_core::session::{run_session, SessionConfig, SessionResult};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;
use tauri::{AppHandle, Emitter};

/// One `progress` event: an optional flow step plus a log line.
#[derive(Clone, Serialize)]
struct ProgressEvent<'a> {
    step: Option<&'a str>,
    message: String,
    done: usize,
    total: usize,
}

/// Sends `progress` events to the window.
#[derive(Clone)]
pub struct Reporter(pub AppHandle);

impl Reporter {
    pub fn emit(&self, step: Option<&str>, message: impl Into<String>, done: usize, total: usize) {
        let event = ProgressEvent { step, message: message.into(), done, total };
        let _ = self.0.emit("progress", event);
    }
    /// Moves the flow diagram to `step`.
    pub fn step(&self, step: &str, message: impl Into<String>) {
        self.emit(Some(step), message, 0, 0);
    }
    /// Logs a line without changing the active step.
    pub fn log(&self, message: impl Into<String>) {
        self.emit(None, message, 0, 0);
    }
}

/// Maps `archiver_core` progress messages onto flow steps.
fn step_for(message: &str) -> Option<&'static str> {
    const RULES: [(&str, &str); 7] = [
        ("Checking candidates", "scan"),
        ("Building search index", "scan"),
        ("Recorded ", "stage"),
        ("Staging file", "stage"),
        ("Committing", "archive"),
        ("Verifying", "archive"),
        ("Cleaning up staging", "cleanup"),
    ];
    RULES.iter().find(|(key, _)| message.contains(key)).map(|(_, step)| *step)
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RunResult {
    Archive {
        #[serde(flatten)]
        result: SessionResult,
        db_records_retrieved: usize,
        duration_secs: f64,
        scan_to_date: String,
    },
    Dryrun {
        barcodes_matched: usize,
        files_that_would_move: usize,
        matched_filenames: Vec<String>,
    },
}

pub struct RunInput {
    pub db_path: PathBuf,
    pub db_config: DbConfig,
    pub session: SessionConfig,
    pub email: Option<EmailConfig>,
    pub dry_run: bool,
}

pub async fn run(input: RunInput, r: Reporter) -> Result<RunResult, String> {
    let started = Instant::now();
    let started_label = format!("epoch:{} UTC", epoch_secs());

    r.step("index", "Opening index database...");
    let conn = open_and_migrate(&input.db_path).map_err(|e| e.to_string())?;

    r.step("query", "Querying SQL Server for archival candidates...");
    let (from_date, to_date) = match &input.session.date_range_override {
        Some((from, to)) => {
            let from = Some(from.trim().to_string()).filter(|f| !f.is_empty());
            r.log(format!("Using manual scan range ({} to {to}).", from.as_deref().unwrap_or("no lower bound")));
            (from, to.trim().to_string())
        }
        None => {
            let last_scan = repo::last_successful_scan_to_date(&conn).map_err(|e| e.to_string())?;
            db::compute_resume_window(last_scan.as_deref(), input.session.lookback_days).map_err(|e| e.to_string())?
        }
    };
    let candidates = db::query_archival_candidates(&input.db_config, from_date.as_deref(), &to_date)
        .await
        .map_err(|e| e.to_string())?;
    let db_records_retrieved = candidates.len();
    r.log(format!("Found {db_records_retrieved} archival candidates."));

    if input.dry_run {
        let barcodes = candidates.into_iter().map(|c| c.barcode).collect();
        let result = tauri::async_runtime::spawn_blocking(move || dry_run_scan(&input.session, barcodes, &r))
            .await
            .map_err(|e| e.to_string())?;
        return Ok(result);
    }

    // The core session is synchronous; keep it off the async runtime.
    let session = input.session.clone();
    let r2 = r.clone();
    let from = from_date.clone().unwrap_or_default();
    let to = to_date.clone();
    let session_run = tauri::async_runtime::spawn_blocking(move || {
        let mut conn = conn;
        let progress = |msg: String, done: usize, total: usize| r2.emit(step_for(&msg), msg, done, total);
        run_session(&mut conn, &session, candidates, &from, &to, &format!("epoch:{}", epoch_secs()), Some(&progress))
    })
    .await
    .map_err(|e| e.to_string())?;
    let duration_secs = started.elapsed().as_secs_f64();

    r.step("report", "Preparing report...");
    let stats = session_stats(started_label, duration_secs, db_records_retrieved, &session_run);
    send_email(input.email.as_ref(), &stats, &r).await;
    let result = session_run.map_err(|e| e.to_string())?;
    r.log("Done.");

    Ok(RunResult::Archive { result, db_records_retrieved, duration_secs, scan_to_date: to_date })
}

/// Email statistics for a finished (or failed) session; counts are zero on failure.
fn session_stats(
    started_label: String,
    duration_secs: f64,
    db_records_retrieved: usize,
    run: &archiver_core::Result<SessionResult>,
) -> SessionStats {
    let ok = run.as_ref().ok();
    let count = |f: fn(&SessionResult) -> usize| ok.map_or(0, f);
    SessionStats {
        started_label,
        duration_secs,
        db_records_retrieved,
        total_source_files_scanned: count(|r| r.total_files_scanned),
        total_files_matched: count(|r| r.barcodes_matched),
        total_files_archived: count(|r| r.files_moved),
        total_files_deleted: count(|r| r.files_deleted),
        archive_breakdown: ok
            .map(|r| {
                r.archive_breakdown
                    .iter()
                    .map(|v| ArchiveBreakdownRow {
                        volume_name: v.volume_name.clone(),
                        files_archived: v.files_archived,
                        total_bytes: v.total_bytes,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        unmatched_barcodes: ok.map(|r| r.unmatched_barcodes.clone()).unwrap_or_default(),
        errors: count(|r| r.errors),
        failed: run.as_ref().err().map(|e| e.to_string()),
    }
}

/// Dry run: reports which files would be archived without touching anything.
fn dry_run_scan(cfg: &SessionConfig, barcodes: Vec<String>, r: &Reporter) -> RunResult {
    if barcodes.is_empty() {
        r.step("report", "No archival candidates in this window. Nothing to do!");
        return RunResult::Dryrun { barcodes_matched: 0, files_that_would_move: 0, matched_filenames: Vec::new() };
    }
    r.step("scan", format!("Building search index for {} candidates (dry run)...", barcodes.len()));
    let matcher = BarcodeMatcher::new(barcodes);
    let mut matched_filenames = Vec::new();
    let mut matched_barcodes = HashSet::new();
    let mut scanned = 0usize;

    for subfolder in &cfg.subfolders {
        let dir = cfg.source_dir.join(subfolder);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            r.log(format!("Warning: subfolder '{subfolder}' not readable at {} — skipping.", dir.display()));
            continue;
        };
        r.log(format!("Scanning subfolder: {subfolder}..."));
        for entry in entries.flatten() {
            scanned += 1;
            if scanned.is_multiple_of(5000) {
                r.log(format!("Scanned {scanned} files so far ({} matches)...", matched_filenames.len()));
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if !path.is_file() || !has_matching_extension(&name, &cfg.allowed_extensions) {
                continue;
            }
            let matches = matcher.matching_barcodes(&name);
            if matches.len() == 1 {
                matched_barcodes.extend(matches);
                matched_filenames.push(format!("{subfolder}/{name}"));
            }
        }
    }

    r.step("report", format!("Scanned {scanned} files. {} would be archived.", matched_filenames.len()));
    RunResult::Dryrun {
        barcodes_matched: matched_barcodes.len(),
        files_that_would_move: matched_filenames.len(),
        matched_filenames,
    }
}

/// Best effort: an email failure is logged, never fails the run.
async fn send_email(config: Option<&EmailConfig>, stats: &SessionStats, r: &Reporter) {
    let Some(config) = config else { return };
    r.log("Sending statistics email...");
    let subject = email_report::default_subject(stats);
    match email_report::send_stats_email(config, &subject, stats).await {
        Ok(()) => r.log(format!("Statistics email sent to {}.", config.recipient)),
        Err(e) => r.log(format!("Failed to send statistics email: {e}")),
    }
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
