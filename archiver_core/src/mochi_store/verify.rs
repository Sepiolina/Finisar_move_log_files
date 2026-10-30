//! Proving a month's staged files are in its archive before staging is
//! deleted.
//!
//! [`verify_volume_with`] reopens the archive **read-only** and runs two
//! checks **at the same time**, each with its own read-only handle:
//!
//! * **Structure** (MOCHI): when the head this program verified last time is
//!   known, only the commits after it (`verify_commits_since`: chain link plus
//!   every new object hash-checked and decoded); otherwise the whole archive
//!   at the `ContentIntegrity` level.
//! * **Per file** (this program): every staged file is in the head as a
//!   regular file, its recorded BLAKE3 content hash equals the hash of the
//!   staged file on disk, and a full verified read of the stored copy hashes
//!   to the same value.
//!
//! Both must pass and must have seen the same head. Any failure returns
//! `Err`, and the caller keeps the staging area.
//!
//! Opening the head of a large archive costs seconds, and several heads
//! opened at once slow each other down far more than linearly, so the
//! per-file check opens **one** head. Hashing the staged files needs no
//! head: it runs on worker threads while that head is being opened.

use super::{
    archive_path_of, hash_file, open_read, open_storage, stored_file, verification_failed,
    verified_head, with_job_ctx, StoreFile,
};
use crate::error::{ArchiveError, Result};
use crate::par;
use mochi_core::catalog::namespace::EntryKind;
use mochi_core::job::JobContext;
use mochi_core::publish::ReadOptions;
use mochi_core::status::{Status, VerificationLevel};
use mochi_core::storage::os::OsReadStorage;
use mochi_core::verify::{self, VerifiedHead, VerifyOptions};
use mochi_core::MochiError;
use std::path::Path;
use std::time::{Duration, Instant};

/// How often the per-file check reports progress.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// How many findings an error message quotes.
const FINDINGS_QUOTED: usize = 5;

/// Verifies that every staged file is in the committed archive, byte for
/// byte, and checks the whole archive at the content-integrity level.
///
/// `files` are the staged files that were stored. Any failure returns `Err`;
/// the caller must keep the staging area.
pub fn verify_volume(archive_path: &Path, files: &[StoreFile]) -> Result<()> {
    verify_volume_with(archive_path, files, None, 1, |_| {}).map(|_| ())
}

/// [`verify_volume`] for large batches.
///
/// * `since`: a head of this archive that was verified earlier (for example
///   the head this program recorded after its previous run). The structural
///   check then covers only the commits after it; `None` checks the whole
///   archive.
/// * `workers`: threads for hashing the staged files (`0`: automatic).
/// * `on_progress(files_checked)` runs on the calling thread.
///
/// Returns the head that was verified.
pub fn verify_volume_with(
    archive_path: &Path,
    files: &[StoreFile],
    since: Option<&VerifiedHead>,
    workers: usize,
    on_progress: impl FnMut(usize),
) -> Result<VerifiedHead> {
    let (per_file, structure) = std::thread::scope(|s| {
        let structure = s.spawn(|| check_structure(archive_path, since));
        let per_file = check_files(archive_path, files, workers, on_progress);
        let structure = structure
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p));
        (per_file, structure)
    });
    let structure_head = structure?;
    if per_file?.is_some_and(|head| head != structure_head) {
        return Err(verification_failed(
            archive_path,
            "the archive head changed while it was verified",
        ));
    }
    Ok(structure_head)
}

/// Every staged file is in the head with its own hash, and reads back
/// intact. Returns the head it checked (`None` when there were no files).
fn check_files(
    archive_path: &Path,
    files: &[StoreFile],
    workers: usize,
    mut on_progress: impl FnMut(usize),
) -> Result<Option<VerifiedHead>> {
    if files.is_empty() {
        return Ok(None);
    }
    let map = |e: MochiError| ArchiveError::from_mochi(archive_path, e);
    let fail =
        |path: &str, why: String| verification_failed(archive_path, format!("{path}: {why}"));

    // Hash the staged files while the head is opened and replayed.
    let (opened, staged_hashes) = std::thread::scope(|s| {
        let opened = s.spawn(|| {
            let (src, head) = open_read(archive_path)?;
            let snapshot = head.catalog.replay(None).map_err(map)?;
            Ok::<_, ArchiveError>((src, head, snapshot))
        });
        let hashes = par::map(
            files,
            par::worker_count(workers),
            |f| hash_file(&f.staged_path),
            |_| {},
        );
        let opened = opened
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p));
        (opened, hashes)
    });
    let (src, head, snapshot) = opened?;
    let ro = ReadOptions::default();

    with_job_ctx(|ctx| {
        let mut last_report = Instant::now();
        for (done, (file, staged_hash)) in files.iter().zip(staged_hashes).enumerate() {
            let staged_hash = staged_hash?;
            let rel = file.relative_path.as_str();
            let path = archive_path_of(rel)?;
            let entry = snapshot
                .get(&path)
                .ok_or_else(|| fail(rel, "missing from the archive head".into()))?;
            if entry.kind != EntryKind::File {
                return Err(fail(rel, "not a regular file in the archive".into()));
            }

            // 1. The recorded hash is the hash of the file on disk.
            let stored_hash = stored_file(&head.catalog, &snapshot, &path)
                .map_err(map)?
                .map(|(h, _)| h);
            if stored_hash.as_deref() != Some(staged_hash.as_str()) {
                return Err(fail(
                    rel,
                    format!(
                        "archive content hash {} does not match staged file hash {staged_hash}",
                        stored_hash.as_deref().unwrap_or("(none)")
                    ),
                ));
            }

            // 2. The stored bytes read back, verified, to the same hash (by
            // version: no replay per file).
            let mut hasher = blake3::Hasher::new();
            mochi_core::read::read_version(
                &src,
                &head.catalog,
                &entry.version,
                &mut hasher,
                &ro,
                ctx,
            )
            .map_err(map)?;
            if hasher.finalize().to_hex().as_str() != staged_hash {
                return Err(fail(
                    rel,
                    "bytes read back from the archive differ from the staged file".into(),
                ));
            }

            if last_report.elapsed() >= PROGRESS_EVERY {
                on_progress(done + 1);
                last_report = Instant::now();
            }
        }
        on_progress(files.len());
        Ok(Some(verified_head(&head)))
    })
}

/// MOCHI's structural check: the commits after `since`, or the whole archive.
/// Returns the head it checked.
fn check_structure(archive_path: &Path, since: Option<&VerifiedHead>) -> Result<VerifiedHead> {
    let src = open_storage(archive_path)?;
    with_job_ctx(|ctx| match since {
        Some(since) => check_new_commits(archive_path, &src, since, ctx),
        None => check_whole_archive(archive_path, &src, ctx),
    })
}

/// `verify_commits_since`: the chain link to `since` and every object the
/// newer commits introduce.
fn check_new_commits(
    archive_path: &Path,
    src: &OsReadStorage,
    since: &VerifiedHead,
    ctx: &JobContext<'_>,
) -> Result<VerifiedHead> {
    let scoped = verify::verify_commits_since(src, &ReadOptions::default(), since, ctx);
    let head = scoped
        .head
        .ok_or_else(|| verification_failed(archive_path, "the archive head could not be opened"))?;
    if head == *since {
        return Ok(head); // no new commits: nothing new to check
    }
    if scoped.scope_status() != Status::Pass {
        return Err(verification_failed(
            archive_path,
            format!(
                "verification of commits {}..{:?} did not pass (chain {:?}, new objects {:?}, \
                 {}/{} objects checked, operational_error: {}): {}",
                scoped.from_seq,
                scoped.to_seq,
                scoped.chain,
                scoped.new_objects,
                scoped.objects_checked,
                scoped.objects_expected,
                scoped.operational_error,
                quote_findings(&scoped.findings)
            ),
        ));
    }
    Ok(head)
}

/// MOCHI's content-level verification of the whole archive.
fn check_whole_archive(
    archive_path: &Path,
    src: &OsReadStorage,
    ctx: &JobContext<'_>,
) -> Result<VerifiedHead> {
    let verification = verify::verify(
        src,
        &VerifyOptions {
            level: VerificationLevel::ContentIntegrity,
            ..VerifyOptions::default()
        },
        ctx,
    );
    let report = &verification.report;
    let failed: Vec<String> = report
        .dimensions
        .iter()
        .filter(|(_, s)| **s == Status::Fail)
        .map(|(d, _)| format!("{d:?}"))
        .collect();
    // Exit code 2 is the normal "degraded" result in MOCHI 1.0.
    if report.operational_error || report.exit_code == 1 || !failed.is_empty() {
        return Err(verification_failed(
            archive_path,
            format!(
                "content-level verification did not pass (exit code {}, failed: {failed:?}, operational_error: {}): {}",
                report.exit_code,
                report.operational_error,
                quote_findings(&report.findings)
            ),
        ));
    }
    verification
        .head
        .ok_or_else(|| verification_failed(archive_path, "verification found no head"))
}

fn quote_findings(findings: &[impl std::fmt::Debug]) -> String {
    findings
        .iter()
        .take(FINDINGS_QUOTED)
        .map(|f| format!("{f:?}"))
        .collect::<Vec<_>>()
        .join("; ")
}
