//! Finding the files to archive: list each configured subfolder and keep the
//! files whose name matches exactly one candidate barcode.

use super::{Progress, SessionConfig, StagedManifestEntry};
use crate::error::{ArchiveError, Result};
use crate::matcher::{has_matching_extension, BarcodeMatcher};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Everything the scan found.
pub(super) struct Scan {
    /// One entry per matched file, in subfolder order, unique by path.
    pub entries: Vec<StagedManifestEntry>,
    /// Directory entries seen.
    pub files_seen: usize,
    pub errors: i64,
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

/// Scans all subfolders in parallel, then drops duplicate paths.
pub(super) fn scan_all(
    config: &SessionConfig,
    workers: usize,
    matcher: &BarcodeMatcher,
    candidate_date: &HashMap<&str, &str>,
    progress: Progress<'_>,
) -> Scan {
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
    let files_seen = scanned.load(Ordering::Relaxed);

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
        "Finished scanning {files_seen} files. Found {} matches.",
        entries.len()
    ));
    errors += dedup_by_path(&mut entries, progress);

    Scan {
        entries,
        files_seen,
        errors,
    }
}

/// What scanning one subfolder found.
struct SubfolderScan {
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
) -> SubfolderScan {
    let mut out = SubfolderScan {
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
        let Some(volume_name) = candidate_date
            .get(barcode.as_str())
            .and_then(|date| volume_name_from_date(date).ok())
        else {
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

/// Keeps the first entry per relative path (a duplicate would fail later at
/// the DB). Returns the number dropped, which count as errors.
fn dedup_by_path(entries: &mut Vec<StagedManifestEntry>, progress: Progress<'_>) -> i64 {
    let mut seen: HashSet<String> = HashSet::with_capacity(entries.len());
    let before = entries.len();
    entries.retain(|e| seen.insert(e.relative_path.clone()));
    let duplicates = before - entries.len();
    if duplicates > 0 {
        progress.say(format!("Warning: skipped {duplicates} duplicate files."));
    }
    duplicates as i64
}
