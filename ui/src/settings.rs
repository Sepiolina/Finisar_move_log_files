//! Phase 6 — persisted UI settings.
//!
//! Per spec: "Persist on close: last source/output paths, extension
//! filter, last-successful-scan date." Stored as a
//! flat JSON file via `directories-next` (already pulled in transitively
//! by `eframe`'s persistence feature, so no new dependency) rather than
//! `eframe`'s own `App::save` mechanism — that ties persistence to
//! `eframe`'s storage backend, which is convenient but harder to inspect
//! or hand-edit if something goes wrong in the field. A plain JSON file
//! next to the DB is easier to reason about for a tool an operator might
//! need to debug by hand.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSettings {
    pub source_dir: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    pub staging_dir: Option<PathBuf>,
    pub db_path: Option<PathBuf>,
    pub extension_filter: Vec<String>,
    /// TEXT-form date (YYYY-MM-DD) of the last successful scan's
    /// query_to_date — read back from the SQLite index at startup
    /// (`repo::last_successful_scan_to_date`) rather than duplicated
    /// here, but kept as a cached display value so the UI can show it
    /// without a DB round trip on every frame.
    pub last_successful_scan_to_date: Option<String>,
    pub dry_run: bool,

    /// Statistics email — sent after each real (non-dry-run) session
    /// completes. `#[serde(default)]` so settings.json files saved
    /// before this feature existed still load instead of falling back
    /// to defaults for everything.
    #[serde(default)]
    pub email_stats_enabled: bool,
    /// Scheme+host+port of the internal email relay, e.g.
    /// `http://10.0.0.5:8080` — no path. `/api/email/send` is appended
    /// by `archiver_core::email_report`.
    #[serde(default)]
    pub email_api_base_url: String,
    #[serde(default)]
    pub email_recipient: String,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            source_dir: None,
            output_dir: None,
            staging_dir: None,
            db_path: None,
            extension_filter: vec!["txt".to_string(), "zip".to_string()],
            last_successful_scan_to_date: None,
            dry_run: false,
            email_stats_enabled: false,
            email_api_base_url: String::new(),
            email_recipient: String::new(),
        }
    }
}

impl UiSettings {
    pub fn settings_path() -> PathBuf {
        directories_next::ProjectDirs::from("com", "log-archiver", "log-archiver")
            .map(|dirs| dirs.config_dir().join("settings.json"))
            .unwrap_or_else(|| PathBuf::from("settings.json"))
    }

    pub fn load() -> Self {
        Self::load_from(&Self::settings_path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        self.save_to(&Self::settings_path());
    }

    pub fn save_to(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Statistics Email fields (and everything else) must survive a
    /// save/load round trip — this is the actual mechanism behind
    /// "settings are remembered" between app launches.
    #[test]
    fn email_settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let mut settings = UiSettings::default();
        settings.email_stats_enabled = true;
        settings.email_api_base_url = "http://10.0.0.5:8080".to_string();
        settings.email_recipient = "ops-team@example.com".to_string();
        settings.save_to(&path);

        let loaded = UiSettings::load_from(&path);
        assert_eq!(loaded.email_stats_enabled, true);
        assert_eq!(loaded.email_api_base_url, "http://10.0.0.5:8080");
        assert_eq!(loaded.email_recipient, "ops-team@example.com");
    }

    /// A settings.json written before this feature existed (no email_*
    /// keys at all) must still load — via `#[serde(default)]` — rather
    /// than falling back to a blank slate for every other setting too.
    #[test]
    fn loads_pre_existing_settings_file_missing_email_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
                "source_dir": "/data/source",
                "output_dir": "/data/output",
                "staging_dir": null,
                "db_path": null,
                "extension_filter": ["txt"],
                "compression_level": 19,
                "last_successful_scan_to_date": null,
                "dry_run": false
            }"#,
        )
        .unwrap();

        let loaded = UiSettings::load_from(&path);
        assert_eq!(loaded.source_dir, Some(PathBuf::from("/data/source")));
        assert_eq!(loaded.email_stats_enabled, false);
        assert_eq!(loaded.email_api_base_url, "");
        assert_eq!(loaded.email_recipient, "");
    }
}
