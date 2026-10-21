use crate::settings::UiSettings;
use crate::worker::{ self, Command, Outcome, Progress, VolumePreview };
use archiver_core::db::DbConfig;
use archiver_core::email_report::EmailConfig;
use archiver_core::reader::VolumeEntry;
use archiver_core::repo::LogEntryLocation;
use archiver_core::secure_config::{ self, SecureConfigPaths };
use archiver_core::session::SessionConfig;
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{ Receiver, Sender };

use notify_rust::Notification;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    ScanArchive,
    Extraction,
    FileViewer,
    Settings,
}

fn format_path(path: Option<&PathBuf>, fallback: &str) -> String {
    let full_path = path.map(|p| p.display().to_string()).unwrap_or_else(|| fallback.to_string());
    if full_path.chars().count() > 50 {
        let tail: String = full_path
            .chars()
            .rev()
            .take(47)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("…{tail}")
    } else {
        full_path
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn send_desktop_notification(title: &str, body: &str) {
    let title = title.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        let _ = Notification::new().summary(&title).body(&body).show();
    });
}

// theme
pub fn apply_style(ctx: &egui::Context) {
    const ACCENT: egui::Color32 = egui::Color32::from_rgb(0, 58, 186); // indigo blue
    let rounding = egui::Rounding::same(6.0);

    let mut visuals = egui::Visuals::light();
    visuals.window_fill = egui::Color32::from_rgb(246, 247, 249);
    visuals.panel_fill = egui::Color32::from_rgb(246, 247, 249);
    visuals.faint_bg_color = egui::Color32::from_rgb(240, 241, 244);
    visuals.extreme_bg_color = egui::Color32::from_rgb(255, 255, 255);
    visuals.code_bg_color = egui::Color32::from_rgb(237, 238, 241);

    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(246, 247, 249);
    visuals.widgets.noninteractive.rounding = rounding;
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(255, 255, 255);
    visuals.widgets.inactive.rounding = rounding;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(
        1.0_f32,
        egui::Color32::from_rgb(210, 215, 222)
    );
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.2_f32, ACCENT);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.5_f32, ACCENT);

    visuals.widgets.hovered.rounding = rounding;
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(232, 236, 245);
    visuals.widgets.active.rounding = rounding;
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.open.rounding = rounding;

    visuals.selection.bg_fill = ACCENT.linear_multiply(0.35);
    visuals.selection.stroke.color = ACCENT;
    visuals.hyperlink_color = ACCENT;
    visuals.window_rounding = egui::Rounding::same(8.0);
    visuals.window_shadow = egui::epaint::Shadow {
        extrusion: 12.0,
        color: egui::Color32::from_black_alpha(20),
    };
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.window_margin = egui::style::Margin::same(10.0);
    style.spacing.indent = 16.0;
    ctx.set_style(style);
}

pub struct App {
    settings: UiSettings,
    tab: Tab,

    // Scan/archive tab state
    subfolders_text: String,
    extension_filter_text: String,
    // Manual scan-date-range override (advanced): unchecked by default,
    // so normal runs keep using the automatic resume-from-last-scan
    // window. Not persisted to settings — it's meant as a one-off
    // escape hatch, not a standing configuration.
    override_date_range: bool,
    override_from_date_text: String,
    override_to_date_text: String,
    status_line: String,
    activity_log: Vec<String>,
    progress: Option<(u32, u32)>,
    busy: bool,
    last_session_summary: Option<String>,

    // Extraction tab state
    extraction_query: String,
    extraction_dest: Option<PathBuf>,
    search_results: Vec<LogEntryLocation>,
    extraction_status: String,

    // File Viewer tab state
    viewer_volume_path: Option<PathBuf>,
    viewer_entries: Vec<VolumeEntry>,
    viewer_filter_text: String,
    viewer_status: String,
    viewer_selected_path: Option<String>,
    viewer_extract_dest: Option<PathBuf>,
    viewer_preview: Option<VolumePreview>,

    // Settings tab state
    secure_config_paths: SecureConfigPaths,
    db_config_draft: DbConfig,
    db_config_saved: bool,
    show_password: bool,
    settings_status: String,

    // Worker plumbing
    cmd_tx: Sender<Command>,
    progress_rx: Receiver<Progress>,
    outcome_rx: Receiver<Outcome>,
}

impl App {
    pub fn new() -> Self {
        let mut settings = UiSettings::load();
        let (cmd_tx, progress_rx, outcome_rx) = worker::spawn();
        let startup_db_path = settings.db_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("log_archiver_index.sqlite"));
        if startup_db_path.exists() {
            if let Ok(conn) = archiver_core::schema::open_and_migrate(&startup_db_path) {
                if let Ok(last_scan) = archiver_core::repo::last_successful_scan_to_date(&conn) {
                    settings.last_successful_scan_to_date = last_scan;
                }
            }
        }

        let extension_filter_text = settings.extension_filter.join(", ");

        let config_dir = directories_next::ProjectDirs
            ::from("com", "log-archiver", "log-archiver")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let secure_config_paths = SecureConfigPaths::in_dir(&config_dir);

        let mut startup_settings_error = None;
        let (db_config_draft, db_config_saved) = match secure_config::load(&secure_config_paths) {
            Ok(Some(config)) => (config, true),
            Ok(None) => (DbConfig::default(), false),
            Err(e) => {
                startup_settings_error = Some(format!("Could not load saved DB settings: {e}"));
                (DbConfig::default(), false)
            }
        };

        Self {
            settings,
            tab: Tab::ScanArchive,
            subfolders_text: "BURNIN, ESS, JTAG, STM, Xray".to_string(),
            extension_filter_text,
            override_date_range: false,
            override_from_date_text: String::new(),
            override_to_date_text: String::new(),
            status_line: "Ready.".to_string(),
            activity_log: Vec::new(),
            progress: None,
            busy: false,
            last_session_summary: None,
            extraction_query: String::new(),
            extraction_dest: None,
            search_results: Vec::new(),
            extraction_status: String::new(),
            viewer_volume_path: None,
            viewer_entries: Vec::new(),
            viewer_filter_text: String::new(),
            viewer_status: String::new(),
            viewer_selected_path: None,
            viewer_extract_dest: None,
            viewer_preview: None,
            secure_config_paths,
            db_config_draft,
            db_config_saved,
            show_password: false,
            settings_status: startup_settings_error.unwrap_or_default(),
            cmd_tx,
            progress_rx,
            outcome_rx,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.settings.db_path.clone().unwrap_or_else(|| PathBuf::from("log_archiver_index.sqlite"))
    }

    fn poll_worker(&mut self) {
        while let Ok(p) = self.progress_rx.try_recv() {
            match p {
                Progress::Status(s) => {
                    self.status_line = s.clone();
                    self.activity_log.push(s);
                    if self.activity_log.len() > 100 {
                        self.activity_log.remove(0);
                    }
                }
                Progress::Fraction(done, total) => {
                    self.progress = Some((done, total));
                }
            }
        }
        while let Ok(o) = self.outcome_rx.try_recv() {
            self.busy = false;
            self.progress = None;
            match o {
                Outcome::SessionDone(Ok(s)) => {
                    self.status_line = "Session complete.".to_string();
                    let volumes_label = if s.volume_names.is_empty() {
                        "(none)".to_string()
                    } else {
                        s.volume_names.join(", ")
                    };
                    let mut summary = format!(
                        "Volumes {}: {} barcodes matched, {} files archived, {} errors.\n",
                        volumes_label,
                        s.barcodes_matched,
                        s.files_moved,
                        s.errors
                    );
                    summary.push_str(
                        &format!(
                            "Runtime: {:.2}s — {} DB records retrieved, {} files scanned, {} files deleted from staging.\n",
                            s.duration_secs,
                            s.db_records_retrieved,
                            s.total_files_scanned,
                            s.files_deleted
                        )
                    );
                    if !s.archive_breakdown.is_empty() {
                        summary.push_str("Archives breakdown:\n");
                        for row in &s.archive_breakdown {
                            summary.push_str(
                                &format!("  {}: {} files\n", row.volume_name, row.files_archived)
                            );
                        }
                    }
                    if !s.unmatched_barcodes.is_empty() {
                        let preview: Vec<_> = s.unmatched_barcodes
                            .iter()
                            .take(10)
                            .cloned()
                            .collect();
                        summary.push_str(
                            &format!(
                                "Unmatched barcodes ({}): {}",
                                s.unmatched_barcodes.len(),
                                preview.join(", ")
                            )
                        );
                        if s.unmatched_barcodes.len() > preview.len() {
                            summary.push_str(
                                &format!(
                                    ", ... and {} more",
                                    s.unmatched_barcodes.len() - preview.len()
                                )
                            );
                        }
                        summary.push_str("\n");
                    }
                    self.last_session_summary = Some(summary);
                    self.settings.last_successful_scan_to_date = Some(s.scan_to_date.clone());
                    self.settings.save();

                    send_desktop_notification(
                        "Log Archiver: Scan Complete",
                        &format!("Successfully archived {} files.", s.files_moved)
                    );
                }
                Outcome::SessionDone(Err(e)) => {
                    self.status_line = format!("Session failed: {e}");
                    send_desktop_notification(
                        "Log Archiver: Error",
                        &format!("Session failed: {e}")
                    );
                }
                Outcome::DryRunDone(Ok(s)) => {
                    self.status_line = "Dry run complete — no files were moved.".to_string();
                    let preview: Vec<_> = s.matched_filenames.iter().take(20).cloned().collect();
                    let mut summary = format!(
                        "Dry run: {} barcodes would match, {} files would be archived.\n",
                        s.barcodes_matched,
                        s.files_that_would_move
                    );
                    for f in &preview {
                        summary.push_str(&format!("  {f}\n"));
                    }
                    if s.matched_filenames.len() > preview.len() {
                        summary.push_str(
                            &format!(
                                "  ... and {} more\n",
                                s.matched_filenames.len() - preview.len()
                            )
                        );
                    }
                    self.last_session_summary = Some(summary);

                    send_desktop_notification(
                        "Log Archiver: Dry Run Done",
                        &format!("{} files would be archived.", s.files_that_would_move)
                    );
                }
                Outcome::DryRunDone(Err(e)) => {
                    self.status_line = format!("Dry run failed: {e}");
                    send_desktop_notification(
                        "Log Archiver: Error",
                        &format!("Dry run failed: {e}")
                    );
                }
                Outcome::ExtractDone(Ok(files)) => {
                    self.extraction_status = format!("Extracted {} file(s).", files.len());
                    send_desktop_notification(
                        "Log Archiver: Extraction Complete",
                        &format!("Extracted {} file(s).", files.len())
                    );
                }
                Outcome::ExtractDone(Err(e)) => {
                    self.extraction_status = format!("Extraction failed: {e}");
                    send_desktop_notification(
                        "Log Archiver: Error",
                        &format!("Extraction failed: {e}")
                    );
                }
                Outcome::SearchDone(Ok(results)) => {
                    self.extraction_status = format!("{} match(es) found.", results.len());
                    self.search_results = results;
                }
                Outcome::SearchDone(Err(e)) => {
                    self.extraction_status = format!("Search failed: {e}");
                    self.search_results.clear();
                }
                Outcome::VolumeListed(Ok(entries)) => {
                    self.viewer_status = format!(
                        "{} file(s) found across all chunks.",
                        entries.len()
                    );
                    self.viewer_entries = entries;
                }
                Outcome::VolumeListed(Err(e)) => {
                    self.viewer_status = format!("Failed to read volume: {e}");
                    self.viewer_entries.clear();
                }
                Outcome::VolumeFileExtracted(Ok(f)) => {
                    self.viewer_status = format!(
                        "Extracted {} ({}) to {}.",
                        f.relative_path,
                        format_bytes(f.bytes_written),
                        f.dest_path.display()
                    );
                    send_desktop_notification(
                        "Log Archiver: File Extracted",
                        &format!("Extracted {}.", f.relative_path)
                    );
                }
                Outcome::VolumeFileExtracted(Err(e)) => {
                    self.viewer_status = format!("Extraction failed: {e}");
                }
                Outcome::VolumeExtractAllDone(Ok(files)) => {
                    self.viewer_status = format!("Extracted {} file(s).", files.len());
                    send_desktop_notification(
                        "Log Archiver: Extraction Complete",
                        &format!("Extracted {} file(s).", files.len())
                    );
                }
                Outcome::VolumeExtractAllDone(Err(e)) => {
                    self.viewer_status = format!("Extract all failed: {e}");
                }
                Outcome::VolumePreviewDone(Ok(preview)) => {
                    self.viewer_status = format!("Loaded preview of {}.", preview.relative_path);
                    self.viewer_preview = Some(preview);
                }
                Outcome::VolumePreviewDone(Err(e)) => {
                    self.viewer_status = format!("Preview failed: {e}");
                    self.viewer_preview = None;
                }
            }
        }
    }

    fn parsed_subfolders(&self) -> Vec<String> {
        self.subfolders_text
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn parsed_extensions(&self) -> Vec<String> {
        self.extension_filter_text
            .split(',')
            .map(|s| s.trim().trim_start_matches('.').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn build_email_config(&self) -> Option<EmailConfig> {
        if !self.settings.email_stats_enabled {
            return None;
        }
        let api_base_url = self.settings.email_api_base_url.trim();
        let recipient = self.settings.email_recipient.trim();
        if api_base_url.is_empty() || recipient.is_empty() {
            return None;
        }
        Some(EmailConfig {
            api_base_url: api_base_url.to_string(),
            recipient: recipient.to_string(),
        })
    }

    fn build_session_config(&self) -> Option<SessionConfig> {
        let source_dir = self.settings.source_dir.clone()?;
        let output_dir = self.settings.output_dir.clone()?;
        let staging_dir = self.settings.staging_dir
            .clone()
            .unwrap_or_else(|| output_dir.join(".staging"));

        Some(SessionConfig {
            source_dir,
            output_dir,
            staging_dir,
            subfolders: self.parsed_subfolders(),
            allowed_extensions: self.parsed_extensions(),
            compression_level: self.settings.compression_level,
            lookback_days: 2,
            date_range_override: self.date_range_override(),
        })
    }

    fn date_range_override(&self) -> Option<(String, String)> {
        if !self.override_date_range {
            return None;
        }
        let to = self.override_to_date_text.trim();
        if to.is_empty() {
            return None;
        }
        Some((self.override_from_date_text.trim().to_string(), to.to_string()))
    }

    fn ui_section(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
        egui::Frame
            ::none()
            .fill(egui::Color32::WHITE)
            .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(228, 229, 233)))
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::style::Margin::symmetric(14.0, 12.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(title).strong().size(14.5));
                });
                ui.add_space(6.0);
                add_contents(ui);
            });
        ui.add_space(10.0);
    }

    fn draw_terminal_log(&mut self, ui: &mut egui::Ui) {
        if !self.activity_log.is_empty() || self.last_session_summary.is_some() {
            Self::ui_section(ui, "Output Log", |ui| {
                egui::Frame
                    ::none()
                    .fill(egui::Color32::from_rgb(30, 30, 34))
                    .rounding(6.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());

                        egui::ScrollArea
                            ::vertical()
                            .auto_shrink([false, false])
                            .min_scrolled_height(300.0)
                            .max_height(500.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                ui.style_mut().visuals.override_text_color = Some(
                                    egui::Color32::from_rgb(200, 220, 200)
                                );

                                let font_size = 14.0;

                                for line in &self.activity_log {
                                    ui.label(
                                        egui::RichText
                                            ::new(line)
                                            .family(egui::FontFamily::Monospace)
                                            .size(font_size)
                                    );
                                }

                                if let Some(summary) = &self.last_session_summary {
                                    ui.add_space(15.0);
                                    ui.label(
                                        egui::RichText
                                            ::new("--- SUMMARY ---")
                                            .strong()
                                            .size(font_size + 2.0)
                                            .color(egui::Color32::from_rgb(100, 200, 255))
                                    );
                                    ui.label(
                                        egui::RichText
                                            ::new(summary)
                                            .family(egui::FontFamily::Monospace)
                                            .size(font_size)
                                    );
                                }
                            });
                    });
            });
        }
    }

    fn draw_scan_archive_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            Self::ui_section(ui, "Source & Output", |ui| {
                egui::Grid
                    ::new("scan_paths_grid")
                    .num_columns(2)
                    .spacing([10.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Source folder:");
                        ui.horizontal(|ui| {
                            ui.monospace(
                                format_path(self.settings.source_dir.as_ref(), "(none selected)")
                            );
                            if ui.button("Browse...").clicked() {
                                if
                                    let Ok(Some(path)) = native_dialog::FileDialog
                                        ::new()
                                        .show_open_single_dir()
                                {
                                    self.settings.source_dir = Some(path);
                                    self.settings.save();
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("Output folder:");
                        ui.horizontal(|ui| {
                            ui.monospace(
                                format_path(self.settings.output_dir.as_ref(), "(none selected)")
                            );
                            if ui.button("Browse...").clicked() {
                                if
                                    let Ok(Some(path)) = native_dialog::FileDialog
                                        ::new()
                                        .show_open_single_dir()
                                {
                                    self.settings.output_dir = Some(path);
                                    self.settings.save();
                                }
                            }
                        });
                    });
            });

            Self::ui_section(ui, "Scan Configuration", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Subfolders (comma-separated):");
                    ui.text_edit_singleline(&mut self.subfolders_text);
                });

                ui.horizontal(|ui| {
                    ui.label("Extension filter (comma-separated, no dot needed):");
                    if ui.text_edit_singleline(&mut self.extension_filter_text).changed() {
                        self.settings.extension_filter = self.parsed_extensions();
                        self.settings.save();
                    }
                });

                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label("Compression mode:");
                    let mut fast_mode = self.settings.compression_level < 0;
                    let was_fast = fast_mode;
                    ui.radio_value(&mut fast_mode, false, "Standard (levels 1-22)");
                    ui.radio_value(&mut fast_mode, true, "Fast (zstd --fast=N)");
                    if fast_mode != was_fast {
                        self.settings.compression_level = if fast_mode { -4 } else { 19 };
                        self.settings.save();
                    }
                });

                if self.settings.compression_level < 0 {
                    ui.horizontal(|ui| {
                        ui.label("zstd --fast= level:");
                        let mut fast_n = (-self.settings.compression_level).max(1);
                        if
                            ui
                                .add(egui::DragValue::new(&mut fast_n).clamp_range(1..=1000))
                                .changed()
                        {
                            self.settings.compression_level = -fast_n;
                            self.settings.save();
                        }
                        if ui.button("--fast=1").clicked() {
                            self.settings.compression_level = -1;
                            self.settings.save();
                        }
                        if ui.button("--fast=4").clicked() {
                            self.settings.compression_level = -4;
                            self.settings.save();
                        }
                    });
                    ui.colored_label(
                        egui::Color32::from_rgb(120, 120, 120),
                        format!(
                            "Equivalent to the zstd CLI's --fast={}. Trades compression ratio for much higher throughput good for large, low-value, or time-boxed backlogs.",
                            -self.settings.compression_level
                        )
                    );
                } else {
                    ui.horizontal(|ui| {
                        ui.label("Zstd compression level:");
                        ui.spacing_mut().slider_width = 160.0;
                        ui.visuals_mut().widgets.inactive.bg_fill = egui::Color32::from_rgb(
                            210,
                            215,
                            222
                        );
                        ui.visuals_mut().widgets.hovered.bg_fill = egui::Color32::from_rgb(
                            190,
                            197,
                            207
                        );
                        let mut level = self.settings.compression_level;
                        if ui.add(egui::Slider::new(&mut level, 1..=22)).changed() {
                            self.settings.compression_level = level;
                            self.settings.save();
                        }
                    });
                    if self.settings.compression_level >= 19 {
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 140, 0),
                            "19+ is slow — much better ratio, much longer archive time."
                        );
                    }
                }

                ui.add_space(4.0);
                ui.checkbox(
                    &mut self.settings.dry_run,
                    "Dry run (scan and report matches only — no files will be moved or archived)"
                );

                ui.add_space(4.0);
                if let Some(to_date) = &self.settings.last_successful_scan_to_date {
                    ui.label(
                        egui::RichText
                            ::new(format!("Last successful scan reached: {to_date}"))
                            .weak()
                    );
                } else {
                    ui.label(
                        egui::RichText
                            ::new(
                                "No successful scan on record yet, next run will use the full initial lookback window."
                            )
                            .weak()
                    );
                }

                ui.add_space(4.0);
                ui.checkbox(
                    &mut self.override_date_range,
                    "Override scan date range (advanced)"
                );
                if self.override_date_range {
                    egui::Grid
                        ::new("date_range_override_grid")
                        .num_columns(2)
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("From date (YYYY-MM-DD, optional):");
                            ui.text_edit_singleline(&mut self.override_from_date_text);
                            ui.end_row();

                            ui.label("To date (YYYY-MM-DD, required):");
                            ui.text_edit_singleline(&mut self.override_to_date_text);
                            ui.end_row();
                        });
                    ui.colored_label(
                        egui::Color32::from_rgb(200, 140, 0),
                        "Replaces the automatic resume window for this run only — use for backfills or to recover a missed range. Leave \"From\" blank for no lower bound."
                    );
                }
            });

            Self::ui_section(ui, "Execution", |ui| {
                ui.add_enabled_ui(!self.busy, |ui| {
                    let button_label = if self.settings.dry_run {
                        "▶ Run Dry Scan"
                    } else {
                        "▶ Run Scan & Archive"
                    };

                    let primary_btn = egui::Button
                        ::new(
                            egui::RichText
                                ::new(button_label)
                                .strong()
                                .size(14.0)
                                .color(egui::Color32::WHITE)
                        )
                        .fill(ui.visuals().selection.bg_fill)
                        .min_size(egui::vec2(150.0, 30.0));

                    if ui.add(primary_btn).clicked() {
                        if !self.db_config_saved {
                            self.status_line =
                                "Please save your SQL Server connection settings on the Settings tab first.".to_string();
                        } else if
                            self.override_date_range &&
                            self.override_to_date_text.trim().is_empty()
                        {
                            self.status_line =
                                "Please enter a \"To\" date for the scan range override, or uncheck the override.".to_string();
                        } else if let Some(session_config) = self.build_session_config() {
                            self.busy = true;
                            self.last_session_summary = None;
                            self.activity_log.clear();
                            self.status_line = "Starting...".to_string();
                            let _ = self.cmd_tx.send(Command::RunSession {
                                db_path: self.db_path(),
                                db_config: self.db_config_draft.clone(),
                                session_config,
                                dry_run: self.settings.dry_run,
                                email_config: self.build_email_config(),
                            });
                        } else {
                            self.status_line =
                                "Please select both a source and output folder first.".to_string();
                        }
                    }
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if self.busy {
                        ui.spinner();
                    }
                    ui.label(egui::RichText::new(&self.status_line).strong());
                });

                if let Some((done, total)) = self.progress {
                    if total > 0 {
                        ui.add_space(4.0);
                        ui.add(
                            egui::ProgressBar
                                ::new((done as f32) / (total as f32))
                                .show_percentage()
                                .animate(self.busy)
                        );
                    }
                }
            });

            // Terminal log will only display if there is content
            self.draw_terminal_log(ui);
        });
    }

    fn draw_extraction_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            Self::ui_section(ui, "Extract by Barcode or Log Name", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Search Query:");
                    ui.text_edit_singleline(&mut self.extraction_query);
                    if ui.button("Search").clicked() && !self.extraction_query.trim().is_empty() {
                        let _ = self.cmd_tx.send(Command::Search {
                            db_path: self.db_path(),
                            query: self.extraction_query.trim().to_string(),
                        });
                    }
                });

                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label("Extract to:");
                    ui.monospace(format_path(self.extraction_dest.as_ref(), "(none selected)"));
                    if ui.button("Browse...").clicked() {
                        if
                            let Ok(Some(path)) = native_dialog::FileDialog
                                ::new()
                                .show_open_single_dir()
                        {
                            self.extraction_dest = Some(path);
                        }
                    }
                });

                ui.add_space(8.0);

                ui.add_enabled_ui(
                    self.extraction_dest.is_some() &&
                        !self.extraction_query.trim().is_empty() &&
                        !self.busy,
                    |ui| {
                        let extract_btn = egui::Button
                            ::new(
                                egui::RichText
                                    ::new("Extract Matches")
                                    .strong()
                                    .size(13.0)
                                    .color(egui::Color32::WHITE)
                            )
                            .fill(ui.visuals().selection.bg_fill)
                            .min_size(egui::vec2(120.0, 26.0));

                        if ui.add(extract_btn).clicked() {
                            self.busy = true;
                            let _ = self.cmd_tx.send(Command::Extract {
                                db_path: self.db_path(),
                                query: self.extraction_query.trim().to_string(),
                                dest_dir: self.extraction_dest.clone().unwrap(),
                            });
                        }
                    }
                );

                ui.add_space(8.0);
                ui.label(egui::RichText::new(&self.extraction_status).strong());

                if let Some((done, total)) = self.progress {
                    if total > 0 {
                        ui.add_space(4.0);
                        ui.add(
                            egui::ProgressBar
                                ::new((done as f32) / (total as f32))
                                .show_percentage()
                                .animate(self.busy)
                        );
                    }
                }
            });

            if !self.search_results.is_empty() {
                Self::ui_section(
                    ui,
                    &format!("{} match(es) found:", self.search_results.len()),
                    |ui| {
                        egui::ScrollArea
                            ::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for loc in &self.search_results {
                                    ui.monospace(&loc.relative_path);
                                }
                            });
                    }
                );
            }
        });
    }

    fn draw_file_viewer_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            Self::ui_section(ui, "Open Archive Volume", |ui| {
                ui.label(
                    egui::RichText
                        ::new(
                            "Opening a .tar.zst with a normal archive tool only shows its first internal chunk. \
                         This reads straight through every chunk in the file."
                        )
                        .small()
                        .weak()
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let display_path = format_path(
                        self.viewer_volume_path.as_ref(),
                        "(no file selected)"
                    );
                    ui.monospace(&display_path).on_hover_text(
                        self.viewer_volume_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default()
                    );
                    if ui.button("Browse...").clicked() {
                        if
                            let Ok(Some(path)) = native_dialog::FileDialog
                                ::new()
                                .add_filter("Archive volume", &["zst"])
                                .show_open_single_file()
                        {
                            self.viewer_volume_path = Some(path);
                            self.viewer_entries.clear();
                            self.viewer_selected_path = None;
                            self.viewer_preview = None;
                            self.viewer_status.clear();
                        }
                    }
                    ui.add_enabled_ui(self.viewer_volume_path.is_some() && !self.busy, |ui| {
                        if ui.button("Load Contents").clicked() {
                            if let Some(volume_path) = self.viewer_volume_path.clone() {
                                self.busy = true;
                                self.viewer_entries.clear();
                                self.viewer_selected_path = None;
                                self.viewer_preview = None;
                                self.viewer_status = "Reading volume...".to_string();
                                let _ = self.cmd_tx.send(Command::ListVolume { volume_path });
                            }
                        }
                    });
                });

                if !self.viewer_status.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if self.busy {
                            ui.spinner();
                        }
                        ui.label(egui::RichText::new(&self.viewer_status).strong());
                    });
                }
            });

            if !self.viewer_entries.is_empty() {
                Self::ui_section(
                    ui,
                    &format!("Contents ({} file(s))", self.viewer_entries.len()),
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Filter:");
                            ui.text_edit_singleline(&mut self.viewer_filter_text);
                        });

                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            ui.label("Extract to:");
                            ui.monospace(
                                format_path(self.viewer_extract_dest.as_ref(), "(none selected)")
                            );
                            if ui.button("Browse...").clicked() {
                                if
                                    let Ok(Some(path)) = native_dialog::FileDialog
                                        ::new()
                                        .show_open_single_dir()
                                {
                                    self.viewer_extract_dest = Some(path);
                                }
                            }
                            ui.add_enabled_ui(
                                self.viewer_extract_dest.is_some() && !self.busy,
                                |ui| {
                                    if ui.button("Extract All").clicked() {
                                        if
                                            let (Some(volume_path), Some(dest_dir)) = (
                                                self.viewer_volume_path.clone(),
                                                self.viewer_extract_dest.clone(),
                                            )
                                        {
                                            self.busy = true;
                                            self.viewer_status =
                                                "Extracting all files...".to_string();
                                            let _ = self.cmd_tx.send(Command::ExtractAllFromVolume {
                                                volume_path,
                                                dest_dir,
                                            });
                                        }
                                    }
                                }
                            );
                        });

                        ui.add_space(8.0);

                        let filter = self.viewer_filter_text.to_lowercase();
                        let filtered: Vec<&VolumeEntry> = self.viewer_entries
                            .iter()
                            .filter(
                                |e|
                                    filter.is_empty() ||
                                    e.relative_path.to_lowercase().contains(&filter)
                            )
                            .collect();

                        egui::ScrollArea
                            ::vertical()
                            .max_height(260.0)
                            .id_source("viewer_entry_list")
                            .show(ui, |ui| {
                                for entry in &filtered {
                                    ui.horizontal(|ui| {
                                        let selected =
                                            self.viewer_selected_path.as_deref() ==
                                            Some(entry.relative_path.as_str());
                                        if
                                            ui
                                                .selectable_label(selected, &entry.relative_path)
                                                .clicked()
                                        {
                                            self.viewer_selected_path = Some(
                                                entry.relative_path.clone()
                                            );
                                            self.viewer_preview = None;
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.weak(format_bytes(entry.size));
                                            }
                                        );
                                    });
                                }
                            });

                        if filtered.len() < self.viewer_entries.len() {
                            ui.label(
                                egui::RichText
                                    ::new(
                                        format!(
                                            "Showing {} of {} file(s).",
                                            filtered.len(),
                                            self.viewer_entries.len()
                                        )
                                    )
                                    .small()
                                    .weak()
                            );
                        }
                    }
                );
            }

            if let Some(selected) = self.viewer_selected_path.clone() {
                Self::ui_section(ui, &format!("Selected: {selected}"), |ui| {
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(!self.busy, |ui| {
                            if ui.button("Preview").clicked() {
                                if let Some(volume_path) = self.viewer_volume_path.clone() {
                                    self.busy = true;
                                    self.viewer_status = format!(
                                        "Loading preview of {selected}..."
                                    );
                                    let _ = self.cmd_tx.send(Command::PreviewFromVolume {
                                        volume_path,
                                        relative_path: selected.clone(),
                                    });
                                }
                            }
                        });
                        ui.add_enabled_ui(self.viewer_extract_dest.is_some() && !self.busy, |ui| {
                            if ui.button("Extract This File").clicked() {
                                if
                                    let (Some(volume_path), Some(dest_dir)) = (
                                        self.viewer_volume_path.clone(),
                                        self.viewer_extract_dest.clone(),
                                    )
                                {
                                    self.busy = true;
                                    self.viewer_status = format!("Extracting {selected}...");
                                    let _ = self.cmd_tx.send(Command::ExtractFromVolume {
                                        volume_path,
                                        relative_path: selected.clone(),
                                        dest_dir,
                                    });
                                }
                            }
                        });
                    });

                    if let Some(preview) = &self.viewer_preview {
                        if preview.relative_path == selected {
                            ui.add_space(6.0);
                            match std::str::from_utf8(&preview.bytes) {
                                Ok(text) => {
                                    egui::Frame
                                        ::none()
                                        .fill(egui::Color32::from_rgb(30, 30, 34))
                                        .rounding(6.0)
                                        .inner_margin(12.0)
                                        .show(ui, |ui| {
                                            ui.set_min_width(ui.available_width());
                                            egui::ScrollArea
                                                ::vertical()
                                                .auto_shrink([false, false])
                                                .min_scrolled_height(300.0)
                                                .max_height(500.0)
                                                .stick_to_bottom(true)
                                                .show(ui, |ui| {
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText
                                                                ::new(text)
                                                                .monospace()
                                                                .color(
                                                                    egui::Color32::from_rgb(
                                                                        220,
                                                                        220,
                                                                        225
                                                                    )
                                                                )
                                                        )
                                                    );
                                                });
                                        });
                                    if preview.truncated {
                                        ui.label(
                                            egui::RichText
                                                ::new(
                                                    "Preview truncated — use Extract This File to get the full contents."
                                                )
                                                .small()
                                                .weak()
                                        );
                                    }
                                }
                                Err(_) => {
                                    ui.label(
                                        egui::RichText
                                            ::new(
                                                format!(
                                                    "({} loaded) — doesn't look like text, use Extract This File to save it.",
                                                    format_bytes(preview.bytes.len() as u64)
                                                )
                                            )
                                            .weak()
                                    );
                                }
                            }
                        }
                    }
                });
            }
        });
    }

    fn draw_settings_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            Self::ui_section(ui, "SQL Server Connection", |ui| {
                egui::Grid
                    ::new("db_settings_grid")
                    .num_columns(2)
                    .spacing([10.0, 5.0])
                    .show(ui, |ui| {
                        ui.label("Server / Port");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit
                                    ::singleline(&mut self.db_config_draft.server)
                                    .desired_width(180.0)
                            );
                            ui.label(":");
                            let mut port_text = self.db_config_draft.port.to_string();
                            if
                                ui
                                    .add(
                                        egui::TextEdit
                                            ::singleline(&mut port_text)
                                            .desired_width(50.0)
                                    )
                                    .changed()
                            {
                                if let Ok(p) = port_text.parse::<u16>() {
                                    self.db_config_draft.port = p;
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("Database");
                        ui.text_edit_singleline(&mut self.db_config_draft.database);
                        ui.end_row();

                        ui.label("Username");
                        ui.text_edit_singleline(&mut self.db_config_draft.username);
                        ui.end_row();

                        ui.label("Password");
                        ui.horizontal(|ui| {
                            let field = egui::TextEdit
                                ::singleline(&mut self.db_config_draft.password)
                                .password(!self.show_password);
                            ui.add(field);
                            ui.checkbox(&mut self.show_password, "Show");
                        });
                        ui.end_row();

                        ui.label("Options");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.db_config_draft.encrypt, "Encrypt");
                            ui.checkbox(
                                &mut self.db_config_draft.trust_server_certificate,
                                "Trust cert"
                            );
                        });
                        ui.end_row();

                        ui.label("Timeout / Retries / Backoff (ms)").on_hover_text(
                            "Request timeout, retry attempts, and base retry delay, all in milliseconds."
                        );
                        ui.horizontal(|ui| {
                            let mut timeout_text = self.db_config_draft.request_timeout
                                .as_millis()
                                .to_string();
                            if
                                ui
                                    .add(
                                        egui::TextEdit
                                            ::singleline(&mut timeout_text)
                                            .desired_width(60.0)
                                    )
                                    .changed()
                            {
                                if let Ok(ms) = timeout_text.parse::<u64>() {
                                    self.db_config_draft.request_timeout =
                                        std::time::Duration::from_millis(ms);
                                }
                            }
                            ui.label("/");
                            let mut retry_text = self.db_config_draft.retry_attempts.to_string();
                            if
                                ui
                                    .add(
                                        egui::TextEdit
                                            ::singleline(&mut retry_text)
                                            .desired_width(40.0)
                                    )
                                    .changed()
                            {
                                if let Ok(n) = retry_text.parse::<u32>() {
                                    self.db_config_draft.retry_attempts = n;
                                }
                            }
                            ui.label("/");
                            let mut delay_text = self.db_config_draft.retry_base_delay
                                .as_millis()
                                .to_string();
                            if
                                ui
                                    .add(
                                        egui::TextEdit
                                            ::singleline(&mut delay_text)
                                            .desired_width(60.0)
                                    )
                                    .changed()
                            {
                                if let Ok(ms) = delay_text.parse::<u64>() {
                                    self.db_config_draft.retry_base_delay =
                                        std::time::Duration::from_millis(ms);
                                }
                            }
                        });
                        ui.end_row();
                    });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save Connection").clicked() {
                        match secure_config::save(&self.secure_config_paths, &self.db_config_draft) {
                            Ok(()) => {
                                self.db_config_saved = true;
                                self.settings_status = "Connection settings saved.".to_string();
                            }
                            Err(e) => {
                                self.settings_status = format!("Failed to save: {e}");
                            }
                        }
                    }
                    if ui.button("Clear").clicked() {
                        match secure_config::clear(&self.secure_config_paths) {
                            Ok(()) => {
                                self.db_config_draft = archiver_core::db::DbConfig::default();
                                self.db_config_saved = false;
                                self.settings_status =
                                    "Saved connection settings cleared.".to_string();
                            }
                            Err(e) => {
                                self.settings_status = format!("Failed to clear: {e}");
                            }
                        }
                    }
                    ui.add_space(6.0);
                    if self.db_config_saved {
                        ui.colored_label(egui::Color32::from_rgb(34, 139, 34), "● Saved");
                    } else {
                        ui.colored_label(egui::Color32::from_rgb(217, 119, 6), "● Not saved");
                    }
                    ui.label(
                        egui::RichText::new("Encrypted at rest (AES-256-GCM).").small().weak()
                    ).on_hover_text(
                        "Credentials are stored encrypted on disk, not as plain environment variables."
                    );
                });
                if !self.settings_status.is_empty() {
                    ui.label(egui::RichText::new(&self.settings_status).small().weak());
                }
            });

            Self::ui_section(ui, "Index Database", |ui| {
                ui.horizontal(|ui| {
                    ui.monospace(format_path(Some(&self.db_path()), "(default)")).on_hover_text(
                        self.db_path().display().to_string()
                    );
                    if ui.button("Browse...").clicked() {
                        if
                            let Ok(Some(path)) = native_dialog::FileDialog
                                ::new()
                                .add_filter("SQLite database", &["sqlite", "db"])
                                .show_save_single_file()
                        {
                            self.settings.db_path = Some(path);
                            self.settings.save();
                        }
                    }
                });
            });

            Self::ui_section(ui, "Statistics Email", |ui| {
                if
                    ui
                        .checkbox(
                            &mut self.settings.email_stats_enabled,
                            "Email a summary after each run"
                        )
                        .on_hover_text(
                            "Sends runtime, file counts, an archives breakdown, and any unmatched barcodes to an internal relay after each real (non-dry-run) session."
                        )
                        .changed()
                {
                    self.settings.save();
                }

                ui.add_enabled_ui(self.settings.email_stats_enabled, |ui| {
                    ui.add_space(4.0);
                    egui::Grid
                        ::new("email_settings_grid")
                        .num_columns(2)
                        .spacing([10.0, 5.0])
                        .show(ui, |ui| {
                            ui.label("Relay URL");
                            if
                                ui
                                    .add(
                                        egui::TextEdit
                                            ::singleline(&mut self.settings.email_api_base_url)
                                            .hint_text("http://10.0.0.5:8080")
                                    )
                                    .changed()
                            {
                                self.settings.save();
                            }
                            ui.end_row();

                            ui.label("Recipient");
                            if
                                ui
                                    .add(
                                        egui::TextEdit
                                            ::singleline(&mut self.settings.email_recipient)
                                            .hint_text("logs-team@example.com")
                                    )
                                    .changed()
                            {
                                self.settings.save();
                            }
                            ui.end_row();
                        });
                });

                if self.settings.email_stats_enabled && self.build_email_config().is_none() {
                    ui.colored_label(
                        egui::Color32::from_rgb(217, 119, 6),
                        "Enabled but URL and/or recipient is blank."
                    );
                }
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker();
        if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Ensure buttons have equal height/padding in tab bar
                ui.style_mut().spacing.button_padding = egui::vec2(12.0, 8.0);

                ui.selectable_value(&mut self.tab, Tab::ScanArchive, "Scan & Archive");
                ui.selectable_value(&mut self.tab, Tab::Extraction, "Extraction");
                ui.selectable_value(&mut self.tab, Tab::FileViewer, "File Viewer");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.tab {
                Tab::ScanArchive => self.draw_scan_archive_tab(ui),
                Tab::Extraction => self.draw_extraction_tab(ui),
                Tab::FileViewer => self.draw_file_viewer_tab(ui),
                Tab::Settings => self.draw_settings_tab(ui),
            }
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.settings.save();
    }
}
