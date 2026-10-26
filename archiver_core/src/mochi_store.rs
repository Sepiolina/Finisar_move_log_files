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
//! * Every file is streamed into the writer (`Transaction::put_file_from`),
//!   so no file is held in memory whole. MOCHI 0.9.11 reads, hashes and
//!   compresses the content on worker threads ahead of the writer
//!   (`WriterOptions::threads`, set from [`StoreOptions::workers`]) and
//!   streams small files as cheaply as in-memory ones (0.9.10 zero-filled a
//!   chunk-sized buffer per file). A month is split into commits of at most
//!   [`COMMIT_BYTE_BUDGET`] bytes and [`COMMIT_FILE_BUDGET`] files; fewer,
//!   larger commits matter with hundreds of thousands of small files, because
//!   every commit is synced and adds catalog records.
//! * The head namespace is replayed **once** per call (not once per commit)
//!   and once more at the end to read back the content hashes MOCHI computed
//!   while it streamed each file, so the cost stays linear in the file count.
//! * After the last commit the writer is closed (`close()`), releasing the
//!   archive's single-writer lock.
//!
//! "Committed" here means `LOCAL_COMMITTED`: one local file was synced. It is
//! not a backup; keep an independent copy of the archive directory.
//!
//! # Verification
//!
//! `verify_volume_with` re-opens the archive **read-only** and, for every
//! staged entry, checks that it is present in the head with a BLAKE3 content
//! hash equal to the hash of the staged file on disk and reads it back
//! end-to-end through MOCHI's verified read path (spread over worker
//! threads, each with its own read-only handle). It then checks the archive
//! structure: when the head this program verified last time is known, only
//! the commits after it (`verify_commits_since`: chain link plus every new
//! object hash-checked and decoded); otherwise the whole archive at the
//! `ContentIntegrity` level. The session only deletes staging after this
//! returns `Ok`.

use crate::error::{ArchiveError, Result};
use crate::par;
use mochi_core::catalog::namespace::{EntryKind, Snapshot};
use mochi_core::catalog::path::ArchivePath;
use mochi_core::catalog::Catalog;
use mochi_core::job::{CancellationToken, JobContext, NullProgress};
use mochi_core::manifest::{Attributes, Mtime};
use mochi_core::object::OsIds;
use mochi_core::publish::{
    open_head, ArchiveWriter, FileSource, PublishDurability, ReadOptions, TailRepair, Transaction,
    WriterOptions,
};
use mochi_core::status::{Status, VerificationLevel};
use mochi_core::storage::os::{OsDir, OsFile, OsReadStorage};
use mochi_core::verify::{self, VerifiedHead, VerifyOptions};
use mochi_core::{ErrorCode, MochiError};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Upper bound on the file bytes in one commit (1 GiB). Content is streamed,
/// so this does not bound memory; it bounds the work a crash throws away. A
/// larger file is committed alone.
pub const COMMIT_BYTE_BUDGET: u64 = 1024 * 1024 * 1024;

/// Upper bound on the files in one commit. Small log files are the norm, so
/// without this a month of 300,000 files would be a single commit.
pub const COMMIT_FILE_BUDGET: usize = 20_000;

/// Fewest files a verification worker gets: each worker opens its own
/// read-only handle and replays the head once, which is not worth it for a
/// handful of files.
const MIN_FILES_PER_VERIFY_WORKER: usize = 2_000;

/// File extension of the monthly archives.
pub const ARCHIVE_EXTENSION: &str = "mochi";

/// Tuning for [`store_volume_with`].
#[derive(Debug, Clone, Copy)]
pub struct StoreOptions {
    pub byte_budget: u64,
    pub file_budget: usize,
    /// Threads MOCHI may use to read, hash and compress (`0`: automatic).
    pub workers: usize,
}

impl Default for StoreOptions {
    fn default() -> Self {
        StoreOptions {
            byte_budget: COMMIT_BYTE_BUDGET,
            file_budget: COMMIT_FILE_BUDGET,
            workers: 0,
        }
    }
}

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
    /// The archive's head before this call's first commit (`None` when this
    /// call created the archive). If that head was verified earlier, only the
    /// commits after it need a structural check.
    pub prior_head: Option<VerifiedHead>,
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

fn mtime_of(meta: &fs::Metadata) -> Option<Mtime> {
    let d = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(Mtime {
        secs: i64::try_from(d.as_secs()).ok()?,
        nanos: d.subsec_nanos(),
    })
}

/// Content hash (hex) and length of the file at `path` in `snapshot`, or
/// `None` if it is absent or not a regular file.
fn stored_file(cat: &Catalog, snapshot: &Snapshot, path: &ArchivePath) -> std::result::Result<Option<(String, u64)>, MochiError> {
    let Some(entry) = snapshot.get(path) else {
        return Ok(None);
    };
    if entry.kind != EntryKind::File {
        return Ok(None);
    }
    Ok(cat
        .file_version(&entry.version)?
        .and_then(|(v, _)| v.content_hash.map(|h| (h.to_hex(), v.logical_len))))
}

struct PendingEntry {
    /// Already in the head with an identical hash: not written again.
    skipped: bool,
    relative_path: String,
    path: ArchivePath,
    size: u64,
    /// Known up front only for skipped entries; read from the catalog after
    /// the commits otherwise.
    content_hash: Option<String>,
}

struct Batch {
    tx: Transaction,
    entries: Vec<PendingEntry>,
}

/// Splits the staged files into bounded commits.
struct Batcher<'a> {
    files: &'a [StoreFile],
    next: usize,
    byte_budget: u64,
    file_budget: usize,
    /// The head namespace when the writer was opened (`None` while the
    /// archive does not exist). This call is the only writer (MOCHI's lock),
    /// so the head later is this plus what this call put.
    head: Option<Snapshot>,
    /// Directories put by an earlier batch of this call.
    dirs_done: HashSet<ArchivePath>,
}

impl<'a> Batcher<'a> {
    fn new(files: &'a [StoreFile], opts: &StoreOptions) -> Self {
        Batcher {
            files,
            next: 0,
            byte_budget: opts.byte_budget,
            file_budget: opts.file_budget.max(1),
            head: None,
            dirs_done: HashSet::new(),
        }
    }

    /// Takes files until the next would exceed a budget (a batch always
    /// takes at least one file). `cat` is the writer's catalog when the
    /// archive exists. Staged files that vanished are skipped, as before.
    fn next_batch(&mut self, cat: Option<&Catalog>) -> Result<Option<Batch>> {
        let mut tx = Transaction::new();
        let mut entries = Vec::new();
        // Files to put: (index into `entries`, staged file, metadata).
        let mut chosen: Vec<(usize, &StoreFile, fs::Metadata)> = Vec::new();
        let mut used: u64 = 0;
        let mut puts = 0usize;
        while self.next < self.files.len() {
            let file = &self.files[self.next];
            let meta = match fs::metadata(&file.staged_path) {
                Ok(m) if m.is_file() => m,
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
            let size_on_disk = meta.len();
            let path = archive_path_of(&file.relative_path)?;

            // Resume idempotency: identical bytes already in the head are not
            // written again (no duplicate version).
            if let (Some(cat), Some(head)) = (cat, self.head.as_ref()) {
                let stored = stored_file(cat, head, &path)
                    .map_err(|e| ArchiveError::from_mochi(&file.staged_path, e))?;
                if let Some((stored, _)) = stored {
                    let hash = hash_file(&file.staged_path)?;
                    if stored == hash {
                        entries.push(PendingEntry {
                            skipped: true,
                            relative_path: file.relative_path.clone(),
                            path,
                            size: size_on_disk,
                            content_hash: Some(hash),
                        });
                        self.next += 1;
                        continue;
                    }
                }
            }

            if puts > 0
                && (puts >= self.file_budget || used.saturating_add(size_on_disk) > self.byte_budget)
            {
                break;
            }

            // Explicit directory entries for every ancestor not yet present.
            let mut missing: Vec<ArchivePath> = Vec::new();
            let mut cur = path.parent();
            while let Some(dir) = cur {
                let in_head = self.head.as_ref().is_some_and(|h| h.get(&dir).is_some());
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

            used = used.saturating_add(size_on_disk);
            puts += 1;
            chosen.push((entries.len(), file, meta));
            entries.push(PendingEntry {
                skipped: false,
                relative_path: file.relative_path.clone(),
                path,
                size: size_on_disk,
                content_hash: None,
            });
            self.next += 1;
        }

        for (i, file, meta) in chosen {
            let attributes = Attributes {
                mtime: mtime_of(&meta),
                ..Attributes::default()
            };
            let source: Arc<dyn FileSource> =
                Arc::new(OsFile::new(file.staged_path.clone(), meta.len()));
            tx.put_file_from(entries[i].path.clone(), source, attributes);
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
    store_volume_with(archive_path, files, &StoreOptions::default())
}

/// [`store_volume`] with an explicit per-commit byte budget (tests use a small
/// one to exercise the splitting).
pub fn store_volume_with_budget(
    archive_path: &Path,
    files: &[StoreFile],
    budget: u64,
) -> Result<FinishedVolume> {
    store_volume_with(
        archive_path,
        files,
        &StoreOptions {
            byte_budget: budget,
            ..StoreOptions::default()
        },
    )
}

/// [`store_volume`] with explicit commit budgets and worker count.
pub fn store_volume_with(archive_path: &Path, files: &[StoreFile], store: &StoreOptions) -> Result<FinishedVolume> {
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
    // MOCHI 0.9.11 compresses on worker threads; give it this month's share
    // of the workers (several months may be written at once).
    let threads = par::worker_count(store.workers);
    let opts = || WriterOptions {
        record_time: true,
        threads: Some(threads),
        ..WriterOptions::default()
    };
    let mut dir = OsDir::containing(archive_path).map_err(|e| map(MochiError::from(e)))?;

    let mut batcher = Batcher::new(files, store);
    // (entry, commit seq), hashes filled in at the end.
    let mut pending: Vec<(PendingEntry, u64)> = Vec::new();
    let mut commit_seqs: Vec<u64> = Vec::new();
    let mut created = false;
    let mut prior_head: Option<VerifiedHead> = None;
    let mut degraded: Option<String> = None;
    let note = |d: &PublishDurability, degraded: &mut Option<String>| {
        if let PublishDurability::DirectoryUnconfirmed(why) = d {
            degraded.get_or_insert_with(|| why.clone());
        }
    };

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
                    prior_head: None,
                });
            }
            Some(first) => {
                let Batch { tx, entries } = first;
                match ArchiveWriter::create_in(&mut dir, name, Box::new(OsIds), opts(), tx, &ctx) {
                    Ok((w, outcome)) => {
                        created = true;
                        note(&outcome.durability, &mut degraded);
                        pending.extend(entries.into_iter().map(|e| (e, outcome.seq)));
                        commit_seqs.push(outcome.seq);
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
        if let (Some(seq), Some(commit_id)) = (w.head_seq(), w.head_commit_id()) {
            prior_head = Some(VerifiedHead {
                archive_id: w.archive_id(),
                seq,
                commit_id,
            });
        }
        writer = Some(w);
    }
    let mut writer = writer.ok_or_else(|| ArchiveError::Config("writer was not opened".into()))?;

    // One replay for the whole call (see the module docs).
    batcher.head = Some(writer.catalog().replay(None).map_err(map)?);
    loop {
        let Some(batch) = batcher.next_batch(Some(writer.catalog()))? else {
            break;
        };
        let Batch { tx, entries } = batch;
        if tx.is_empty() {
            // Everything in this batch is already committed with identical
            // bytes; no new commit. Index it at the current head.
            let seq = writer
                .head_seq()
                .ok_or_else(|| ArchiveError::Config("archive has no head".into()))?;
            pending.extend(entries.into_iter().map(|e| (e, seq)));
            continue;
        }
        let outcome = writer.commit(tx, &ctx).map_err(map)?;
        note(&outcome.durability, &mut degraded);
        pending.extend(entries.into_iter().map(|e| (e, outcome.seq)));
        commit_seqs.push(outcome.seq);
    }

    // The hashes MOCHI computed while it streamed each file, and the lengths
    // it actually read.
    let head = writer.catalog().replay(None).map_err(map)?;
    let mut written: Vec<StoredEntry> = Vec::with_capacity(pending.len());
    for (e, seq) in pending {
        let (content_hash, size) = match e.content_hash {
            Some(h) if e.skipped => (h, e.size),
            _ => stored_file(writer.catalog(), &head, &e.path)
                .map_err(map)?
                .ok_or_else(|| {
                    verification_failed(
                        archive_path,
                        format!("{} is not a file in the head after its commit", e.relative_path),
                    )
                })?,
        };
        written.push(StoredEntry {
            relative_path: e.relative_path,
            original_size: size,
            content_hash,
            commit_seq: seq,
        });
    }
    drop(head);

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
        prior_head,
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
/// * `workers`: threads for the per-file checks (`0`: automatic).
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
    let map = |e: MochiError| ArchiveError::from_mochi(archive_path, e);
    let progress = NullProgress;
    let cancel = CancellationToken::new();
    let ctx = JobContext {
        progress: &progress,
        cancel: &cancel,
    };

    // Per-file checks, in contiguous slices; every worker opens its own
    // read-only handle (a catalog is not shared between threads) and replays
    // the head once.
    let workers = par::worker_count(workers)
        .min(files.len().div_ceil(MIN_FILES_PER_VERIFY_WORKER))
        .max(1);
    let slices: Vec<&[StoreFile]> = files.chunks(files.len().div_ceil(workers).max(1)).collect();
    let checked = std::sync::atomic::AtomicUsize::new(0);
    let mut on_progress = on_progress;
    let results = par::map(
        &slices,
        workers,
        |slice| verify_files(archive_path, slice, &checked),
        |_| on_progress(checked.load(std::sync::atomic::Ordering::Relaxed)),
    );
    let mut head: Option<VerifiedHead> = None;
    for r in results {
        let h = r?;
        if head.is_some_and(|prev| prev != h) {
            return Err(verification_failed(archive_path, "the archive head changed while it was verified"));
        }
        head = Some(h);
    }

    let src = OsReadStorage::open(archive_path).map_err(|e| map(MochiError::from(e)))?;
    let ro = ReadOptions::default();
    // The head the per-file checks saw (opening a big archive's head costs
    // seconds, so it is not opened again just to learn it).
    let current = match head {
        Some(h) => h,
        None => {
            let opened = open_head(&src, &ro).map_err(map)?;
            VerifiedHead {
                archive_id: opened.commit.archive_id,
                seq: opened.seq(),
                commit_id: opened.commit_id,
            }
        }
    };
    let changed = || verification_failed(archive_path, "the archive head changed while it was verified");

    match since {
        Some(prev) if *prev == current => {} // nothing new to check structurally
        Some(prev) => {
            let scoped = verify::verify_commits_since(&src, &ro, prev, &ctx);
            if scoped.scope_status() != Status::Pass {
                let findings: Vec<String> = scoped.findings.iter().take(5).map(|f| format!("{f:?}")).collect();
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
                        findings.join("; ")
                    ),
                ));
            }
            if scoped.head != Some(current) {
                return Err(changed());
            }
        }
        None => {
            if verify_whole(archive_path, &src, &ctx)? != Some(current) {
                return Err(changed());
            }
        }
    }
    Ok(current)
}

/// Present in the head with the staged file's hash, and read back intact.
fn verify_files(
    archive_path: &Path,
    files: &[StoreFile],
    checked: &std::sync::atomic::AtomicUsize,
) -> Result<VerifiedHead> {
    let map = |e: MochiError| ArchiveError::from_mochi(archive_path, e);
    let src = OsReadStorage::open(archive_path).map_err(|e| map(MochiError::from(e)))?;
    let ro = ReadOptions::default();
    let head = open_head(&src, &ro).map_err(map)?;
    let snapshot = head.catalog.replay(None).map_err(map)?;
    let progress = NullProgress;
    let cancel = CancellationToken::new();
    let ctx = JobContext {
        progress: &progress,
        cancel: &cancel,
    };

    for file in files {
        let path = archive_path_of(&file.relative_path)?;
        let entry = snapshot.get(&path).ok_or_else(|| {
            verification_failed(archive_path, format!("{} is missing from the archive head", file.relative_path))
        })?;
        if entry.kind != EntryKind::File {
            return Err(verification_failed(
                archive_path,
                format!("{} is not a regular file in the archive", file.relative_path),
            ));
        }
        let stored_hash = stored_file(&head.catalog, &snapshot, &path).map_err(map)?.map(|(h, _)| h);
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
        // End-to-end verified read of the stored copy (by version: no replay
        // per file).
        let mut sink = HashSink(blake3::Hasher::new());
        mochi_core::read::read_version(&src, &head.catalog, &entry.version, &mut sink, &ro, &ctx)
            .map_err(map)?;
        let read_back = sink.0.finalize().to_hex().to_string();
        if read_back != staged_hash {
            return Err(verification_failed(
                archive_path,
                format!("{}: bytes read back from the archive differ from the staged file", file.relative_path),
            ));
        }
        checked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(VerifiedHead {
        archive_id: head.commit.archive_id,
        seq: head.seq(),
        commit_id: head.commit_id,
    })
}

/// MOCHI's content-level verification of the whole archive. Returns the
/// head it checked.
fn verify_whole(archive_path: &Path, src: &OsReadStorage, ctx: &JobContext<'_>) -> Result<Option<VerifiedHead>> {
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
    Ok(verification.head)
}
