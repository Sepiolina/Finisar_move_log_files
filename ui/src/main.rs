#![windows_subsystem = "windows"]
//! Log Archiver desktop app: a Tauri shell around `archiver_core`.
//! The UI lives in `frontend/`; the commands it calls live in `commands.rs`.

mod commands;
mod pipeline;
mod settings;

use archiver_core::secure_config::SecureConfigPaths;
use settings::UiSettings;
use std::path::PathBuf;

fn main() {
    let mut settings = UiSettings::load();

    // Refresh the cached "last successful scan" date from the index, if it exists.
    let db = commands::db_path(&settings);
    if db.exists() {
        if let Ok(Ok(last)) = archiver_core::schema::open_and_migrate(&db)
            .map(|conn| archiver_core::repo::last_successful_scan_to_date(&conn))
        {
            settings.last_successful_scan_to_date = last;
        }
    }

    let config_dir = directories_next::ProjectDirs::from("com", "log-archiver", "log-archiver")
        .map(|dirs| dirs.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::AppState::new(settings, SecureConfigPaths::in_dir(&config_dir)))
        .invoke_handler(tauri::generate_handler![
            commands::load_state,
            commands::save_settings,
            commands::save_db_config,
            commands::clear_db_config,
            commands::run_session,
            commands::search,
            commands::extract,
            commands::list_volume,
            commands::preview_entry,
            commands::extract_entry,
            commands::extract_all,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Log Archiver");
}
