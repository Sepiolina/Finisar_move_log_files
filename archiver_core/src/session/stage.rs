//! Moving matched files from the source tree into staging, listing each one
//! in the manifest first.

use super::{Progress, SessionConfig, StagedManifestEntry, StagingManifest, StagingManifestWriter};
use crate::error::{ArchiveError, Result};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

/// Lists `entries` in a new staging manifest and moves them into staging.
/// Returns the manifest of the files actually moved, and the failure count.
///
/// Crash-correct ordering: per batch of `FLUSH_EVERY`, the manifest lines
/// are written and SYNCED before any file of that batch is renamed, so a
/// renamed file can never be missing from the manifest. A listed file that
/// was not renamed (yet, or because the rename failed) is simply still in the
/// source tree; resume skips listed files that are absent from staging.
///
/// Renames run in parallel. A file that cannot be moved (locked by the
/// tester, permissions) is counted as an error and left in the source; it
/// is picked up again by a later run. A file whose staging path is already
/// taken is never overwritten (that file could be the only copy).
pub(super) fn stage_entries(
    config: &SessionConfig,
    session_id: i64,
    entries: &[StagedManifestEntry],
    io_workers: usize,
    progress: Progress<'_>,
) -> Result<(StagingManifest, usize)> {
    fs::create_dir_all(&config.staging_dir)
        .map_err(|e| ArchiveError::io(&config.staging_dir, e))?;
    let mut manifest_writer = StagingManifestWriter::create(&config.staging_dir, session_id)?;

    let total = entries.len();
    let mut dirs_made: HashSet<PathBuf> = HashSet::new();
    // Only moved files go into the returned manifest. One that was not moved
    // is not archived by this run (a resume after a crash may still pick up a
    // same-named file in staging; never lost).
    let mut staged: Vec<StagedManifestEntry> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for (batch_no, batch) in entries
        .chunks(StagingManifestWriter::FLUSH_EVERY)
        .enumerate()
    {
        // 1. List the batch, durably.
        for entry in batch {
            manifest_writer.record(entry.clone())?;
        }
        manifest_writer.flush()?;

        // 2. Then move it.
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
            io_workers,
            |entry| stage_one(config, entry),
            |done| {
                progress.report(
                    format!("Staging file {} of {total}...", offset + done),
                    offset + done,
                    total,
                )
            },
        );
        for (entry, result) in batch.iter().zip(results) {
            match result {
                Ok(()) => staged.push(entry.clone()),
                Err(msg) => failures.push(msg),
            }
        }
    }
    manifest_writer.finish()?;
    if let Some(first) = failures.first() {
        progress.report(
            format!(
                "Warning: {} of {total} file(s) could not be moved into staging and were left in the source folder (first: {first}).",
                failures.len(),
            ),
            staged.len(),
            total,
        );
    }

    let manifest = StagingManifest {
        scan_session_id: session_id,
        files: staged,
    };
    Ok((manifest, failures.len()))
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
