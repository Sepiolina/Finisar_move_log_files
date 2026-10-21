//! Monthly MOCHI archives: create-or-append, bounded commits, verification.
//!
//! One archive per month, `<output_dir>/<YYYY>/<YYYY-MM>.mochi`. Inside, every
//! file lives at `<SUBFOLDER>/<filename>` (the same `relative_path` the
//! matcher produces), with an explicit directory entry for the subfolder.
//!
//! # Commit model
//!
//! * `store_volume` writes the staged files of one month. A missing archive is
//!   created (`ArchiveWriter::create_in`, which never replaces anything); an
//!   existing one is opened for append with `TailRepair::Refuse`, so an
//!   interrupted earlier write surfaces as `UNCOMMITTED_TAIL` and is never
//!   truncated automatically.
//! * MOCHI holds a commit's content in memory, so the month's files are split
//!   into several commits of at most [`COMMIT_BYTE_BUDGET`] bytes each. A
//!   single file larger than the budget gets a commit of its own.
//! * After the last commit the writer is closed (`close()`), releasing the
//!   archive's single-writer lock.
//!
//! "Committed" here means `LOCAL_COMMITTED`: one local file was synced. It is
//! not a backup; keep an independent copy of the archive directory.
//!
//! # Verification
//!
//! `verify_volume` re-opens the archive **read-only** and, for every staged
//! entry, checks that it is present in the head with a BLAKE3 content hash equal
//! to the hash of the staged file on disk, reads it back end-to-end through
//! MOCHI's verified read path, and finally runs `verify::verify` at the
//! `ContentIntegrity` level over the whole archive. The session only deletes
//! staging after this returns `Ok`.

use crate::error::{ArchiveError, Result};
use mochi_core::catalog::namespace::{EntryKind, Snapshot};
use mochi_core::catalog::Catalog;
use mochi_core::catalog::path::ArchivePath;
use mochi_core::job::{CancellationToken, JobContext, NullProgress};
use mochi_core::manifest::{Attributes, Mtime};
use mochi_core::object::OsIds;
use mochi_core::publish::{
    open_head, ArchiveWriter, PublishDurability, ReadOptions, TailRepair, Transaction, WriterOptions,
};
use mochi_core::status::{Status, VerificationLevel};
use mochi_core::storage::os::{OsDir, OsReadStorage};
use mochi_core::verify::{self, VerifyOptions};
use mochi_core::{ErrorCode, MochiError};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Upper bound on the file bytes held in memory for one commit (64 MiB).
/// MOCHI buffers a commit's content, so a month with more data than this is
/// written as several commits. A file bigger than the budget is committed
/// alone (it must fit in memory by itself).
pub const COMMIT_BYTE_BUDGET: u64 = 64 * 1024 * 1024;

/// File extension of the monthly archives.
pub const ARCHIVE_EXTENSION: &str = "mochi";

/// A staged file waiting to be committed into the month's archive.
#[derive(Debug, Clone)]
pub struct StoreFile {
    /// Path inside the archive, `<SUBFOLDER>/<filename>`.
    pub relative_path: String,
    /// Where the file currently sits (the staging area).
    pub staged_path: PathBuf,
}

/// One file as committed.
#[derive(Debug, Clone)]
pub struct StoredEntry {
    pub relative_path: String,
    pub original_size: u64,
    /// BLAKE3 of the file's bytes, lowercase hex (what `b3sum` prints).
    pub content_hash: String,
    /// The MOCHI commit sequence that added this file.
    pub commit_seq: u64,
}

/// Result of writing one month's files.
#[derive(Debug, Clone)]
pub struct FinishedVolume {
    pub path: PathBuf,
    /// Size of the archive file after the last commit.
    pub file_size: u64,
    /// Files committed by this call (the number of entries added now).
    pub entry_count: usize,
    pub head_commit_id: String,
    pub head_seq: u64,
    pub written_entries: Vec<StoredEntry>,
    /// Sequence numbers of the commits made by this call, in order.
    pub commit_seqs: Vec<u64>,
    /// `true` if this call created the archive.
    pub created: bool,
    /// Set when a commit was `LOCAL_COMMITTED` but its directory entry is not
    /// confirmed durable (always the case for a new archive on Windows). This
    /// is a degraded status, not an error.
    pub degraded_durability: Option<String>,
}

/// `<output_dir>/<YYYY>/<volume_name>.mochi` for a `YYYY-MM` volume name.
pub fn volume_path(output_dir: &Path, volume_name: &str) -> PathBuf {
    let year = volume_name.split('-').next().unwrap_or(volume_name);
    output_dir
        .join(year)
        .join(format!("{volume_name}.{ARCHIVE_EXTENSION}"))
}

/// Streaming BLAKE3 of a file on disk, lowercase hex.
pub fn hash_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path).map_err(|e| ArchiveError::io(path, e))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| ArchiveError::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Builds an [`ArchivePath`] from a `/`-separated relative path.
pub fn archive_path_of(relative_path: &str) -> Result<ArchivePath> {
    ArchivePath::from_components(relative_path.split('/'))
        .map_err(|e| ArchiveError::from_mochi(Path::new(relative_path), e))
}

fn mtime_of(path: &Path) -> Option<Mtime> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let d = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(Mtime {
        secs: i64::try_from(d.as_secs()).ok()?,
        nanos: d.subsec_nanos(),
    })
}

struct PendingEntry {
    /// Already in the head with an identical hash: not written again.
    skipped: bool,
    relative_path: String,
    size: u64,
    content_hash: String,
}

struct Batch {
    tx: Transaction,
    entries: Vec<PendingEntry>,
}

/// Splits the staged files into byte-bounded commits.
struct Batcher<'a> {
    files: &'a [StoreFile],
    next: usize,
    budget: u64,
    /// Directory entries already put by an earlier batch of this call.
    dirs_done: HashSet<ArchivePath>,
}

impl<'a> Batcher<'a> {
    fn new(files: &'a [StoreFile], budget: u64) -> Self {
        Batcher {
            files,
            next: 0,
            budget,
            dirs_done: HashSet::new(),
        }
    }

    /// Reads files until adding the next would exceed the budget (a batch
    /// always takes at least one file). `head` is the writer's current
    /// namespace, or `None` while the archive is not created yet. Staged
    /// files that vanished are skipped, as before.
    fn next_batch(&mut self, head: Option<(&Catalog, &Snapshot)>) -> Result<Option<Batch>> {
        let head_snapshot = head.map(|(_, s)| s);
        let mut tx = Transaction::new();
        let mut entries = Vec::new();
        let mut used: u64 = 0;
        while self.next < self.files.len() {
            let file = &self.files[self.next];
            let size_on_disk = match fs::metadata(&file.staged_path) {
                Ok(m) if m.is_file() => m.len(),
                Ok(_) => {
                    self.next += 1;
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    self.next += 1;
                    continue;
                }
                Err(e) => return Err(ArchiveError::io(&file.staged_path, e)),
            };
            let path = archive_path_of(&file.relative_path)?;

            // Resume idempotency: identical bytes already in the head are not
            // written again (no duplicate version, nothing held in memory).
            if let Some((cat, snap)) = head {
                if let Some(entry) = snap.get(&path) {
                    if entry.kind == EntryKind::File {
                        let stored = cat
                            .file_version(&entry.version)
                            .map_err(|e| ArchiveError::from_mochi(&file.staged_path, e))?
                            .and_then(|(v, _)| v.content_hash)
                            .map(|h| h.to_hex());
                        if let Some(stored) = stored {
                            let hash = hash_file(&file.staged_path)?;
                            if stored == hash {
                                entries.push(PendingEntry {
                                    skipped: true,
                                    relative_path: file.relative_path.clone(),
                                    size: size_on_disk,
                                    content_hash: hash,
                                });
                                self.next += 1;
                                continue;
                            }
                        }
                    }
                }
            }

            let puts = entries.iter().filter(|e| !e.skipped).count();
            if puts > 0 && used.saturating_add(size_on_disk) > self.budget {
                break;
            }
            let content =
                fs::read(&file.staged_path).map_err(|e| ArchiveError::io(&file.staged_path, e))?;

            // Explicit directory entries for every ancestor not yet present.
            let mut missing: Vec<ArchivePath> = Vec::new();
            let mut cur = path.parent();
            while let Some(dir) = cur {
                let in_head = head_snapshot.is_some_and(|h| h.get(&dir).is_some());
                if in_head || self.dirs_done.contains(&dir) {
                    break;
                }
                cur = dir.parent();
                missing.push(dir);
            }
            for dir in missing.into_iter().rev() {
                tx.put_dir(dir.clone(), Attributes::default());
                self.dirs_done.insert(dir);
            }

            let hash = blake3::hash(&content).to_hex().to_string();
            let size = content.len() as u64;
            used = used.saturating_add(size);
            tx.put_file(
                path,
                content,
                Attributes {
                    mtime: mtime_of(&file.staged_path),
                    ..Attributes::default()
                },
            );
            entries.push(PendingEntry {
                skipped: false,
                relative_path: file.relative_path.clone(),
                size,
                content_hash: hash,
            });
            self.next += 1;
        }
        if entries.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Batch { tx, entries }))
        }
    }
}

/// Writes `files` into the month's archive at `archive_path`, creating it if
/// missing. See the module docs for the commit model.
pub fn store_volume(archive_path: &Path, files: &[StoreFile]) -> Result<FinishedVolume> {
    store_volume_with_budget(archive_path, files, COMMIT_BYTE_BUDGET)
}

/// [`store_volume`] with an explicit per-commit byte budget (tests use a small
/// one to exercise the splitting).
pub fn store_volume_with_budget(
    archive_path: &Path,
    files: &[StoreFile],
    budget: u64,
) -> Result<FinishedVolume> {
    let map = |e: MochiError| ArchiveError::from_mochi(archive_path, e);

    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
    }
    let name = archive_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            ArchiveError::Config(format!("archive path {archive_path:?} has no usable file name"))
        })?;

    let progress = NullProgress;
    let cancel = CancellationToken::new();
    let ctx = JobContext {
        progress: &progress,
        cancel: &cancel,
    };
    let opts = || WriterOptions {
        record_time: true,
        ..WriterOptions::default()
    };
    let mut dir = OsDir::containing(archive_path)
        .map_err(|e| map(MochiError::from(e)))?;

    let mut batcher = Batcher::new(files, budget);
    let mut written: Vec<StoredEntry> = Vec::new();
    let mut commit_seqs: Vec<u64> = Vec::new();
    let mut created = false;
    let mut degraded: Option<String> = None;
    let note = |d: &PublishDurability, degraded: &mut Option<String>| {
        if let PublishDurability::DirectoryUnconfirmed(why) = d {
            degraded.get_or_insert_with(|| why.clone());
        }
    };

    let mut record = |batch_entries: Vec<PendingEntry>, seq: u64| {
        for e in batch_entries {
            written.push(StoredEntry {
                relative_path: e.relative_path,
                original_size: e.size,
                content_hash: e.content_hash,
                commit_seq: seq,
            });
        }
    };
    let mut record_commit = |seq: u64| commit_seqs.push(seq);

    // Open (or create) the writer, committing the first batch on the way.
    let mut writer = if archive_path.exists() {
        None
    } else {
        match batcher.next_batch(None)? {
            None => {
                // Nothing to store and nothing to create.
                return Ok(FinishedVolume {
                    path: archive_path.to_path_buf(),
                    file_size: 0,
                    entry_count: 0,
                    head_commit_id: String::new(),
                    head_seq: 0,
                    written_entries: Vec::new(),
                    commit_seqs: Vec::new(),
                    created: false,
                    degraded_durability: None,
                });
            }
            Some(first) => {
                let Batch { tx, entries } = first;
                match ArchiveWriter::create_in(&mut dir, name, Box::new(OsIds), opts(), tx, &ctx) {
                    Ok((w, outcome)) => {
                        created = true;
                        note(&outcome.durability, &mut degraded);
                        record(entries, outcome.seq);
                        record_commit(outcome.seq);
                        Some(w)
                    }
                    // Lost a race with another creator: the archive exists now.
                    // The consumed batch's dirs must be re-put when re-batched.
                    Err(e) if e.code == ErrorCode::DestinationExists => {
                        batcher.next = 0;
                        batcher.dirs_done.clear();
                        None
                    }
                    Err(e) => return Err(map(e)),
                }
            }
        }
    };
    if writer.is_none() {
        let (w, _) = ArchiveWriter::open_append_in(&mut dir, name, Box::new(OsIds), opts(), TailRepair::Refuse)
            .map_err(map)?;
        writer = Some(w);
    }
    let mut writer = writer.ok_or_else(|| ArchiveError::Config("writer was not opened".into()))?;

    loop {
        let head = writer.catalog().replay(None).map_err(map)?;
        let Some(batch) = batcher.next_batch(Some((writer.catalog(), &head)))? else {
            break;
        };
        drop(head);
        let Batch { tx, entries } = batch;
        if tx.is_empty() {
            // Everything in this batch is already committed with identical
            // bytes; no new commit. Index it at the current head.
            let seq = writer
                .head_seq()
                .ok_or_else(|| ArchiveError::Config("archive has no head".into()))?;
            record(entries, seq);
            continue;
        }
        let outcome = writer.commit(tx, &ctx).map_err(map)?;
        note(&outcome.durability, &mut degraded);
        record(entries, outcome.seq);
        record_commit(outcome.seq);
    }

    let head_seq = writer
        .head_seq()
        .ok_or_else(|| ArchiveError::Config("archive has no head after commit".into()))?;
    let head_commit_id = writer
        .head_commit_id()
        .map(|c| c.to_hex())
        .unwrap_or_default();
    writer.close().map_err(map)?;

    let file_size = fs::metadata(archive_path)
        .map_err(|e| ArchiveError::io(archive_path, e))?
        .len();
    Ok(FinishedVolume {
        path: archive_path.to_path_buf(),
        file_size,
        entry_count: written.len(),
        head_commit_id,
        head_seq,
        written_entries: written,
        commit_seqs,
        created,
        degraded_durability: degraded,
    })
}

/// A `Write` sink that BLAKE3-hashes what it receives.
struct HashSink(blake3::Hasher);

impl std::io::Write for HashSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn verification_failed(volume: &Path, reason: impl Into<String>) -> ArchiveError {
    ArchiveError::VerificationFailed {
        volume: volume.display().to_string(),
        reason: reason.into(),
    }
}

/// Verifies that every staged file is in the committed archive, byte for byte.
///
/// `files` are the staged files that were stored. See the module docs. Any
/// failure returns `Err`; the caller must keep the staging area.
pub fn verify_volume(archive_path: &Path, files: &[StoreFile]) -> Result<()> {
    let map = |e: MochiError| ArchiveError::from_mochi(archive_path, e);
    let src = OsReadStorage::open(archive_path)
        .map_err(|e| map(MochiError::from(e)))?;
    let ro = ReadOptions::default();
    let head = open_head(&src, &ro).map_err(map)?;
    let listing: HashMap<ArchivePath, (EntryKind, Option<String>)> =
        mochi_core::read::list(&head, None)
            .map_err(map)?
            .into_iter()
            .map(|e| (e.path, (e.kind, e.content_hash.map(|h| h.to_hex()))))
            .collect();

    let progress = NullProgress;
    let cancel = CancellationToken::new();
    let ctx = JobContext {
        progress: &progress,
        cancel: &cancel,
    };

    for file in files {
        let path = archive_path_of(&file.relative_path)?;
        let (kind, stored_hash) = listing.get(&path).ok_or_else(|| {
            verification_failed(archive_path, format!("{} is missing from the archive head", file.relative_path))
        })?;
        if *kind != EntryKind::File {
            return Err(verification_failed(
                archive_path,
                format!("{} is not a regular file in the archive", file.relative_path),
            ));
        }
        let staged_hash = hash_file(&file.staged_path)?;
        if stored_hash.as_deref() != Some(staged_hash.as_str()) {
            return Err(verification_failed(
                archive_path,
                format!(
                    "{}: archive content hash {} does not match staged file hash {staged_hash}",
                    file.relative_path,
                    stored_hash.as_deref().unwrap_or("(none)")
                ),
            ));
        }
        // End-to-end verified read of the stored copy.
        let mut sink = HashSink(blake3::Hasher::new());
        mochi_core::read::read_file(&src, &head, &path, &mut sink, &ro, &ctx).map_err(map)?;
        let read_back = sink.0.finalize().to_hex().to_string();
        if read_back != staged_hash {
            return Err(verification_failed(
                archive_path,
                format!("{}: bytes read back from the archive differ from the staged file", file.relative_path),
            ));
        }
    }

    let verification = verify::verify(
        &src,
        &VerifyOptions {
            level: VerificationLevel::ContentIntegrity,
            ..VerifyOptions::default()
        },
        &ctx,
    );
    let report = &verification.report;
    let failed: Vec<String> = report
        .dimensions
        .iter()
        .filter(|(_, s)| **s == Status::Fail)
        .map(|(d, _)| format!("{d:?}"))
        .collect();
    if report.operational_error || report.exit_code == 1 || !failed.is_empty() {
        let findings: Vec<String> = report
            .findings
            .iter()
            .take(5)
            .map(|f| format!("{f:?}"))
            .collect();
        return Err(verification_failed(
            archive_path,
            format!(
                "content-level verification did not pass (exit code {}, failed: {failed:?}, operational_error: {}): {}",
                report.exit_code,
                report.operational_error,
                findings.join("; ")
            ),
        ));
    }
    Ok(())
}
