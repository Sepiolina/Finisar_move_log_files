//! Tauri commands called from `frontend/app.js` via `invoke(...)`.
//!
//! Each long-running command announces its steps through [`Reporter`]
//! so the UI can draw the execution flow live.

use crate::pipeline::{self, Reporter, RunInput, RunResult};
use crate::settings::UiSettings;
use archiver_core::db::DbConfig;
use archiver_core::email_report::EmailConfig;
use archiver_core::reader;
use archiver_core::repo::{self, LogEntryLocation};
use archiver_core::schema::open_and_migrate;
use archiver_core::secure_config::{self, SecureConfigPaths};
use archiver_core::session::SessionConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, State};

/// Preview reads stop at 512 KB.
const PREVIEW_MAX_BYTES: u64 = 512 * 1024;

pub struct AppState {
    settings: Mutex<UiSettings>,
    secure_paths: SecureConfigPaths,
    busy: AtomicBool,
}

/// Clears the busy flag when a command finishes (even on error).
struct BusyGuard<'a>(&'a AtomicBool);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl AppState {
    pub fn new(settings: UiSettings, secure_paths: SecureConfigPaths) -> Self {
        Self { settings: Mutex::new(settings), secure_paths, busy: AtomicBool::new(false) }
    }

    /// Only one task runs at a time.
    fn begin(&self) -> Result<BusyGuard<'_>, String> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err("Another task is already running.".into());
        }
        Ok(BusyGuard(&self.busy))
    }

    fn settings(&self) -> UiSettings {
        self.settings.lock().unwrap().clone()
    }
}

/// Runs blocking archive work off the async runtime.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> archiver_core::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

fn notify(title: &str, body: &str) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new().summary(&title).body(&body).show();
    });
}

pub fn db_path(settings: &UiSettings) -> PathBuf {
    settings.db_path.clone().unwrap_or_else(|| PathBuf::from("log_archiver_index.sqlite"))
}

// ---- Settings ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialState {
    settings: UiSettings,
    db_config: DbConfig,
    db_config_saved: bool,
    notice: Option<String>,
}

#[tauri::command]
pub fn load_state(state: State<AppState>) -> InitialState {
    let (db_config, db_config_saved, notice) = match secure_config::load(&state.secure_paths) {
        Ok(Some(config)) => (config, true, None),
        Ok(None) => (DbConfig::default(), false, None),
        Err(e) => (DbConfig::default(), false, Some(format!("Could not load saved DB settings: {e}"))),
    };
    InitialState { settings: state.settings(), db_config, db_config_saved, notice }
}

#[tauri::command]
pub fn save_settings(settings: UiSettings, state: State<AppState>) {
    settings.save();
    *state.settings.lock().unwrap() = settings;
}

#[tauri::command]
pub fn save_db_config(config: DbConfig, state: State<AppState>) -> Result<(), String> {
    secure_config::save(&state.secure_paths, &config).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_db_config(state: State<AppState>) -> Result<(), String> {
    secure_config::clear(&state.secure_paths).map_err(|e| e.to_string())
}

// ---- Archive run ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    subfolders: Vec<String>,
    extensions: Vec<String>,
    dry_run: bool,
    date_from: Option<String>,
    date_to: Option<String>,
}

#[tauri::command]
pub async fn run_session(req: RunRequest, app: AppHandle, state: State<'_, AppState>) -> Result<RunResult, String> {
    let _busy = state.begin()?;
    let settings = state.settings();
    let db_config = secure_config::load(&state.secure_paths)
        .map_err(|e| e.to_string())?
        .ok_or("SQL Server connection is not saved yet — see Settings.")?;
    let source_dir = settings.source_dir.clone().ok_or("Choose a source folder first.")?;
    let output_dir = settings.output_dir.clone().ok_or("Choose an output folder first.")?;

    let email = (settings.email_stats_enabled
        && !settings.email_api_base_url.trim().is_empty()
        && !settings.email_recipient.trim().is_empty())
    .then(|| EmailConfig {
        api_base_url: settings.email_api_base_url.trim().to_string(),
        recipient: settings.email_recipient.trim().to_string(),
    });

    let input = RunInput {
        db_path: db_path(&settings),
        db_config,
        session: SessionConfig {
            staging_dir: settings.staging_dir.clone().unwrap_or_else(|| output_dir.join(".staging")),
            source_dir,
            output_dir,
            subfolders: req.subfolders,
            allowed_extensions: req.extensions,
            lookback_days: 2,
            date_range_override: req
                .date_to
                .filter(|to| !to.trim().is_empty())
                .map(|to| (req.date_from.unwrap_or_default(), to)),
            workers: 0,
        },
        email,
        dry_run: req.dry_run,
    };

    let result = pipeline::run(input, Reporter(app)).await;
    match &result {
        Ok(RunResult::Archive { result, scan_to_date, .. }) => {
            let mut settings = state.settings.lock().unwrap();
            settings.last_successful_scan_to_date = Some(scan_to_date.clone());
            settings.save();
            notify("Log Archiver: Scan complete", &format!("Archived {} files.", result.files_moved));
        }
        Ok(RunResult::Dryrun { files_that_would_move, .. }) => {
            notify("Log Archiver: Dry run done", &format!("{files_that_would_move} files would be archived."))
        }
        Err(e) => notify("Log Archiver: Error", e),
    }
    result
}

// ---- Search & extract (via the index) ----

/// Exact barcode first, then a filename substring.
fn find_locations(conn: &rusqlite::Connection, query: &str) -> archiver_core::Result<Vec<LogEntryLocation>> {
    let by_barcode = repo::find_entries_by_barcode(conn, query)?;
    if !by_barcode.is_empty() {
        return Ok(by_barcode);
    }
    repo::find_entries_by_filename_like(conn, query)
}

#[tauri::command]
pub async fn search(query: String, app: AppHandle, state: State<'_, AppState>) -> Result<Vec<LogEntryLocation>, String> {
    let _busy = state.begin()?;
    let (r, db) = (Reporter(app), db_path(&state.settings()));
    blocking(move || {
        r.step("index", "Opening index database...");
        let conn = open_and_migrate(&db)?;
        r.step("search", format!("Searching for {query:?}..."));
        let found = find_locations(&conn, &query)?;
        r.log(format!("{} match(es) found.", found.len()));
        Ok(found)
    })
    .await
}

#[tauri::command]
pub async fn extract(query: String, dest_dir: PathBuf, app: AppHandle, state: State<'_, AppState>) -> Result<usize, String> {
    let _busy = state.begin()?;
    let (r, db) = (Reporter(app), db_path(&state.settings()));
    let count = blocking(move || {
        r.step("index", "Opening index database...");
        let conn = open_and_migrate(&db)?;
        r.step("search", format!("Finding files for {query:?}..."));
        let found = find_locations(&conn, &query)?;
        r.step("extract", format!("Extracting {} file(s) to {}...", found.len(), dest_dir.display()));
        Ok(reader::extract_locations(&found, &dest_dir)?.len())
    })
    .await?;
    notify("Log Archiver: Extraction complete", &format!("Extracted {count} file(s)."));
    Ok(count)
}

// ---- Volume viewer (reads a .mochi file directly) ----

#[tauri::command]
pub async fn list_volume(volume_path: PathBuf, app: AppHandle, state: State<'_, AppState>) -> Result<Vec<reader::VolumeEntry>, String> {
    let _busy = state.begin()?;
    let r = Reporter(app);
    blocking(move || {
        r.step("open", format!("Opening {}...", volume_path.display()));
        r.step("read", "Reading entries...");
        let entries = reader::list_volume_entries(&volume_path, |n| r.log(format!("Found {n} file(s) so far...")))?;
        r.log(format!("{} file(s) in this archive.", entries.len()));
        Ok(entries)
    })
    .await
}

#[derive(Serialize)]
pub struct Preview {
    text: String,
    truncated: bool,
}

#[tauri::command]
pub async fn preview_entry(volume_path: PathBuf, relative_path: String, app: AppHandle, state: State<'_, AppState>) -> Result<Preview, String> {
    let _busy = state.begin()?;
    let r = Reporter(app);
    blocking(move || {
        r.step("open", format!("Opening {}...", volume_path.display()));
        r.step("read", format!("Loading preview of {relative_path}..."));
        let (bytes, truncated) = reader::preview_entry_from_volume(&volume_path, &relative_path, PREVIEW_MAX_BYTES)?;
        Ok(Preview { text: String::from_utf8_lossy(&bytes).into_owned(), truncated })
    })
    .await
}

#[tauri::command]
pub async fn extract_entry(
    volume_path: PathBuf,
    relative_path: String,
    dest_dir: PathBuf,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PathBuf, String> {
    let _busy = state.begin()?;
    let r = Reporter(app);
    blocking(move || {
        r.step("open", format!("Opening {}...", volume_path.display()));
        // The archive-supplied name is validated before it becomes a disk path.
        let dest_path = reader::safe_join(&dest_dir, &relative_path)?;
        r.step("extract", format!("Extracting {relative_path}..."));
        reader::extract_entry_from_volume(&volume_path, &relative_path, &dest_path)?;
        r.log(format!("Wrote {}.", dest_path.display()));
        Ok(dest_path)
    })
    .await
}

#[tauri::command]
pub async fn extract_all(volume_path: PathBuf, dest_dir: PathBuf, app: AppHandle, state: State<'_, AppState>) -> Result<usize, String> {
    let _busy = state.begin()?;
    let r = Reporter(app);
    let count = blocking(move || {
        r.step("open", format!("Opening {}...", volume_path.display()));
        r.step("extract", format!("Extracting all files to {}...", dest_dir.display()));
        let files = reader::extract_all_from_volume(&volume_path, &dest_dir, |n| r.log(format!("Extracted {n} file(s)...")))?;
        Ok(files.len())
    })
    .await?;
    notify("Log Archiver: Extraction complete", &format!("Extracted {count} file(s)."));
    Ok(count)
}
