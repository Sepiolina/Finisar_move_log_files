//! Archiving a staged batch month by month: commit, verify, index, and only
//! then delete the staged copies.
//!
//! Months are separate archives with separate writer locks, so several are
//! committed and verified at once on worker threads. Workers never touch
//! SQLite or delete anything: they report back to the calling thread, which
//! indexes a month and then removes its staged copies.

use super::{Progress, SessionConfig, StagedManifestEntry, StagingManifest, VolumeBreakdown};
use crate::error::{ArchiveError, Result};
use crate::mochi_store::{self, FinishedVolume, StoreFile};
use crate::repo;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// One month's share of a staged batch.
pub(super) struct VolumeJob {
    pub volume_name: String,
    /// The staged files still present, in manifest order.
    pub entries: Vec<StagedManifestEntry>,
    pub store_files: Vec<StoreFile>,
    /// The head the index recorded after the month's last verified write.
    pub indexed_head: Option<(String, u64)>,
}

/// Groups the manifest by month (in month order). Only files still present in
/// staging are archived (resume tolerance); months with none are dropped.
pub(super) fn plan_jobs(
    conn: &Connection,
    config: &SessionConfig,
    manifest: &StagingManifest,
) -> Result<Vec<VolumeJob>> {
    let mut by_volume: BTreeMap<&str, Vec<&StagedManifestEntry>> = BTreeMap::new();
    for entry in &manifest.files {
        by_volume.entry(&entry.volume_name).or_default().push(entry);
    }

    let mut jobs = Vec::new();
    for (volume_name, entries) in by_volume {
        let mut job = VolumeJob {
            volume_name: volume_name.to_string(),
            entries: Vec::new(),
            store_files: Vec::new(),
            indexed_head: repo::volume_head(conn, volume_name)?,
        };
        for entry in entries {
            let staged_path = config.staging_dir.join(&entry.relative_path);
            if staged_path.exists() {
                job.store_files.push(StoreFile {
                    relative_path: entry.relative_path.clone(),
                    staged_path,
                });
                job.entries.push(entry.clone());
            }
        }
        if !job.store_files.is_empty() {
            jobs.push(job);
        }
    }
    Ok(jobs)
}

/// What [`run_jobs`] collected.
#[derive(Default)]
pub(super) struct MonthsOutcome {
    /// Entries of the months that were committed, verified, indexed and cleaned.
    pub archived: Vec<StagedManifestEntry>,
    /// Sorted by volume name.
    pub breakdown: Vec<VolumeBreakdown>,
    pub durability_notes: Vec<String>,
    /// A month that fails does not stop the others: its staged files and the
    /// manifest stay for the next run, which retries only what is left.
    pub failures: Vec<ArchiveError>,
}

/// Thread budget for one batch.
#[derive(Debug, Clone, Copy)]
pub(super) struct Threads {
    /// CPU-bound work (MOCHI compression, verification).
    pub cpu: usize,
    /// File-system metadata work (deleting staged files), which waits on the
    /// disk more than it uses the CPU.
    pub io: usize,
}

/// From a month worker to the thread that owns the index.
enum MonthMsg {
    Progress(String),
    Done(usize, Box<Result<FinishedVolume>>),
}

/// Commits and verifies the months in parallel, indexing and cleaning each
/// one on this thread as it finishes.
pub(super) fn run_jobs(
    conn: &mut Connection,
    config: &SessionConfig,
    session_id: i64,
    jobs: &[VolumeJob],
    threads: Threads,
    progress: Progress<'_>,
) -> MonthsOutcome {
    let total: usize = jobs.iter().map(|j| j.store_files.len()).sum();
    // Every month at once (up to one per core): much of a commit runs on one
    // thread, so months side by side keep the cores busy.
    let parallel_months = threads.cpu.clamp(1, jobs.len().max(1));
    let per_month = (threads.cpu / parallel_months).max(1);

    let mut out = MonthsOutcome::default();
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::channel::<MonthMsg>();
        for _ in 0..parallel_months {
            let sender = sender.clone();
            let next = &next;
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.get(i) else { break };
                let say = |m: String| {
                    let _ = sender.send(MonthMsg::Progress(m));
                };
                let outcome = commit_and_verify(config, job, per_month, &say);
                let _ = sender.send(MonthMsg::Done(i, Box::new(outcome)));
            });
        }
        drop(sender);

        // Files of the months finished so far (only this thread counts).
        let mut done_files = 0usize;
        for msg in receiver {
            match msg {
                MonthMsg::Progress(m) => progress.report(m, done_files, total),
                MonthMsg::Done(i, outcome) => {
                    let job = &jobs[i];
                    let indexed = (*outcome).and_then(|finished| {
                        index_and_clean(conn, session_id, job, &finished, threads.io, progress)
                            .map(|()| finished)
                    });
                    match indexed {
                        Ok(finished) => out.record_success(job, &finished),
                        Err(e) => {
                            progress.report(
                                format!(
                                    "ERROR: {}.mochi: {e}. Its {} staged file(s) are kept in staging for the next run.",
                                    job.volume_name,
                                    job.store_files.len()
                                ),
                                done_files,
                                total,
                            );
                            out.failures.push(e);
                        }
                    }
                    done_files += job.store_files.len();
                }
            }
        }
    });
    out.breakdown
        .sort_by(|a, b| a.volume_name.cmp(&b.volume_name));
    out
}

impl MonthsOutcome {
    fn record_success(&mut self, job: &VolumeJob, finished: &FinishedVolume) {
        if let Some(why) = &finished.degraded_durability {
            self.durability_notes
                .push(format!("{}: {why}", job.volume_name));
        }
        self.breakdown.push(VolumeBreakdown {
            volume_name: job.volume_name.clone(),
            files_archived: job.entries.len(),
            total_bytes: job.entries.iter().map(|e| e.original_size).sum(),
        });
        self.archived.extend(job.entries.iter().cloned());
    }
}

/// Commits one month's staged files and verifies them (no index writes, no
/// deletes: safe on a worker thread).
fn commit_and_verify(
    config: &SessionConfig,
    job: &VolumeJob,
    workers: usize,
    say: &dyn Fn(String),
) -> Result<FinishedVolume> {
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
    let scope = if since.is_some() {
        "new commits"
    } else {
        "whole archive"
    };
    say(format!(
        "Verifying {n} file(s) in {volume_name}.mochi (read-only, {scope})..."
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
    if finished.entry_count < job.entries.len() {
        return Err(ArchiveError::VerificationFailed {
            volume: volume_name.clone(),
            reason: format!(
                "committed {} entries but expected {}. Aborting cleanup.",
                finished.entry_count,
                job.entries.len()
            ),
        });
    }
    Ok(finished)
}

/// Indexes a committed and verified month, then deletes its staged copies.
fn index_and_clean(
    conn: &mut Connection,
    session_id: i64,
    job: &VolumeJob,
    finished: &FinishedVolume,
    io_workers: usize,
    progress: Progress<'_>,
) -> Result<()> {
    let volume_name = &job.volume_name;
    let n = job.store_files.len();
    let by_path: HashMap<&str, &StagedManifestEntry> = job
        .entries
        .iter()
        .map(|e| (e.relative_path.as_str(), e))
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
        &super::started_at_now(),
        |path| by_path.get(path).map(|e| e.barcode.clone()),
        |path| {
            by_path
                .get(path)
                .map(|e| e.source_subfolder.clone())
                .unwrap_or_default()
        },
    )?;

    // Committed, verified and indexed: these staged copies may go. The
    // manifest still lists them; a resume skips listed files that are gone.
    crate::par::map(
        &job.store_files,
        io_workers,
        |f| remove_file_if_present(&f.staged_path),
        |done| {
            progress.say(format!(
                "Removed {done} of {n} staged file(s) of {volume_name}..."
            ))
        },
    )
    .into_iter()
    .collect()
}

/// Final cleanup after every month succeeded. Each month's verified files
/// were already deleted right after it was indexed; this removes the manifest,
/// then directories that are now empty. Anything else found in staging (e.g.
/// a file renamed in by a crashed run whose manifest line was lost) is never
/// deleted: it stays, is counted as an error and reported. Returns that count.
pub(super) fn cleanup_staging(config: &SessionConfig, progress: Progress<'_>) -> Result<usize> {
    remove_file_if_present(&StagingManifest::path_in(&config.staging_dir))?;
    let mut unlisted: Vec<PathBuf> = Vec::new();
    prune_empty_dirs(&config.staging_dir, &mut unlisted);
    if let Some(first) = unlisted.first() {
        progress.say(format!(
            "Warning: {} file(s) in {} were not listed in the staging manifest and were NOT archived or deleted (first: {}). They are left in place; review them manually.",
            unlisted.len(),
            config.staging_dir.display(),
            first.display()
        ));
    }
    Ok(unlisted.len())
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
