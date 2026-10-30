//! Writing one month: create-or-append, bounded commits.
//!
//! * A missing archive is created (`ArchiveWriter::create_in`, which never
//!   replaces anything); an existing one is opened for append with
//!   `TailRepair::Refuse`, so an interrupted earlier write surfaces as
//!   `UNCOMMITTED_TAIL` and is never truncated automatically.
//! * Every file is streamed into the writer (`Transaction::put_file_from`),
//!   so no file is held in memory whole. MOCHI reads, hashes and compresses
//!   the content on worker threads ahead of the writer
//!   (`WriterOptions::threads`, set from [`StoreOptions::workers`]).
//! * A month is split into commits of at most [`COMMIT_BYTE_BUDGET`] bytes
//!   and [`COMMIT_FILE_BUDGET`] files; fewer, larger commits matter with
//!   hundreds of thousands of small files, because every commit is synced
//!   and adds catalog records.
//! * The head namespace is replayed **once** per call (not once per commit)
//!   and once more at the end to read back the content hashes MOCHI computed
//!   while it streamed each file, so the cost stays linear in the file count.
//! * After the last commit the writer is closed (`close()`), releasing the
//!   archive's single-writer lock.

use super::{
    archive_path_of, hash_file, stored_file, verification_failed, with_job_ctx, FinishedVolume,
    StoreFile, StoreOptions, StoredEntry,
};
#[cfg(doc)]
use super::{COMMIT_BYTE_BUDGET, COMMIT_FILE_BUDGET};
use crate::error::{ArchiveError, Result};
use crate::par;
use mochi_core::catalog::namespace::Snapshot;
use mochi_core::catalog::path::ArchivePath;
use mochi_core::catalog::Catalog;
use mochi_core::job::JobContext;
use mochi_core::manifest::{Attributes, Mtime};
use mochi_core::object::OsIds;
use mochi_core::publish::{
    ArchiveWriter, CommitOutcome, FileSource, PublishDurability, TailRepair, Transaction,
    WriterOptions,
};
use mochi_core::storage::os::{OsDir, OsFile, OsStorage};
use mochi_core::verify::VerifiedHead;
use mochi_core::{ErrorCode, MochiError};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

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
pub fn store_volume_with(
    archive_path: &Path,
    files: &[StoreFile],
    store: &StoreOptions,
) -> Result<FinishedVolume> {
    let map = |e: MochiError| ArchiveError::from_mochi(archive_path, e);

    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
    }
    let name = archive_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            ArchiveError::Config(format!(
                "archive path {archive_path:?} has no usable file name"
            ))
        })?;
    // Several months may be written at once; MOCHI gets this month's share.
    let threads = par::worker_count(store.workers);
    let mut dir = OsDir::containing(archive_path).map_err(|e| map(MochiError::from(e)))?;
    let mut batcher = Batcher::new(files, store);
    let mut commits = Commits::default();

    with_job_ctx(|ctx| {
        let opened = open_or_create_writer(
            archive_path,
            &mut dir,
            name,
            threads,
            &mut batcher,
            &mut commits,
            ctx,
        )?;
        let Some(OpenedWriter {
            mut writer,
            created,
            prior_head,
        }) = opened
        else {
            // The archive is missing and there is nothing to store.
            return Ok(FinishedVolume {
                path: archive_path.to_path_buf(),
                ..FinishedVolume::default()
            });
        };

        // One replay for the whole call (see the module docs).
        batcher.head = Some(writer.catalog().replay(None).map_err(map)?);
        while let Some(Batch { tx, entries }) = batcher.next_batch(Some(writer.catalog()))? {
            if tx.is_empty() {
                // Everything in this batch is already committed with identical
                // bytes: no new commit. Index it at the current head.
                let seq = writer
                    .head_seq()
                    .ok_or_else(|| ArchiveError::Config("archive has no head".into()))?;
                commits.unchanged(entries, seq);
            } else {
                let outcome = writer.commit(tx, ctx).map_err(map)?;
                commits.committed(entries, &outcome);
            }
        }

        let written = read_back_entries(archive_path, writer.catalog(), commits.pending)?;
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
            commit_seqs: commits.seqs,
            created,
            degraded_durability: commits.degraded,
            prior_head,
        })
    })
}

fn writer_options(threads: usize) -> WriterOptions {
    WriterOptions {
        record_time: true,
        threads: Some(threads),
        ..WriterOptions::default()
    }
}

struct OpenedWriter {
    writer: ArchiveWriter<OsStorage>,
    /// `true` if this call created the archive.
    created: bool,
    /// The head before this call's first commit (`None` if created now).
    prior_head: Option<VerifiedHead>,
}

/// Opens the archive for append or, if it is missing, creates it with the
/// first batch as its first commit. `None`: the archive is missing and there
/// is nothing to store, so nothing was created.
fn open_or_create_writer(
    archive_path: &Path,
    dir: &mut OsDir,
    name: &str,
    threads: usize,
    batcher: &mut Batcher<'_>,
    commits: &mut Commits,
    ctx: &JobContext<'_>,
) -> Result<Option<OpenedWriter>> {
    let map = |e: MochiError| ArchiveError::from_mochi(archive_path, e);

    if !archive_path.exists() {
        let Some(Batch { tx, entries }) = batcher.next_batch(None)? else {
            return Ok(None);
        };
        // `create_in` never replaces an existing file.
        let options = writer_options(threads);
        match ArchiveWriter::create_in(dir, name, Box::new(OsIds), options, tx, ctx) {
            Ok((writer, outcome)) => {
                commits.committed(entries, &outcome);
                return Ok(Some(OpenedWriter {
                    writer,
                    created: true,
                    prior_head: None,
                }));
            }
            // Lost a race with another creator: the archive exists now; append.
            Err(e) if e.code == ErrorCode::DestinationExists => batcher.restart(),
            Err(e) => return Err(map(e)),
        }
    }

    let options = writer_options(threads);
    let (writer, _) =
        ArchiveWriter::open_append_in(dir, name, Box::new(OsIds), options, TailRepair::Refuse)
            .map_err(map)?;
    let prior_head = match (writer.head_seq(), writer.head_commit_id()) {
        (Some(seq), Some(commit_id)) => Some(VerifiedHead {
            archive_id: writer.archive_id(),
            seq,
            commit_id,
        }),
        _ => None,
    };
    Ok(Some(OpenedWriter {
        writer,
        created: false,
        prior_head,
    }))
}

/// A staged file that went into (or was already in) a commit.
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
    /// archive exists. Staged files that vanished are skipped.
    fn next_batch(&mut self, cat: Option<&Catalog>) -> Result<Option<Batch>> {
        let mut tx = Transaction::new();
        let mut entries = Vec::new();
        let mut used: u64 = 0;
        let mut puts = 0usize;
        while let Some(file) = self.files.get(self.next) {
            let Some(meta) = staged_metadata(file)? else {
                self.next += 1;
                continue;
            };
            let size = meta.len();
            let path = archive_path_of(&file.relative_path)?;

            // Resume idempotency: identical bytes already in the head are not
            // written again (no duplicate version).
            if let Some(hash) = self.already_stored(cat, file, &path)? {
                entries.push(PendingEntry {
                    skipped: true,
                    relative_path: file.relative_path.clone(),
                    path,
                    size,
                    content_hash: Some(hash),
                });
                self.next += 1;
                continue;
            }

            let full = puts >= self.file_budget || used.saturating_add(size) > self.byte_budget;
            if puts > 0 && full {
                break;
            }

            // Explicit directory entries for every ancestor not yet present,
            // outermost first.
            for dir in self.missing_dirs(&path).into_iter().rev() {
                tx.put_dir(dir.clone(), Attributes::default());
                self.dirs_done.insert(dir);
            }
            let attributes = Attributes {
                mtime: mtime_of(&meta),
                ..Attributes::default()
            };
            let source: Arc<dyn FileSource> = Arc::new(OsFile::new(file.staged_path.clone(), size));
            tx.put_file_from(path.clone(), source, attributes);

            used = used.saturating_add(size);
            puts += 1;
            entries.push(PendingEntry {
                skipped: false,
                relative_path: file.relative_path.clone(),
                path,
                size,
                content_hash: None,
            });
            self.next += 1;
        }
        Ok((!entries.is_empty()).then_some(Batch { tx, entries }))
    }

    /// The staged file's hash if the head already holds exactly these bytes
    /// at `path`.
    fn already_stored(
        &self,
        cat: Option<&Catalog>,
        file: &StoreFile,
        path: &ArchivePath,
    ) -> Result<Option<String>> {
        let (Some(cat), Some(head)) = (cat, self.head.as_ref()) else {
            return Ok(None);
        };
        let stored = stored_file(cat, head, path)
            .map_err(|e| ArchiveError::from_mochi(&file.staged_path, e))?;
        let Some((stored_hash, _)) = stored else {
            return Ok(None);
        };
        let hash = hash_file(&file.staged_path)?;
        Ok((stored_hash == hash).then_some(hash))
    }

    /// The ancestors of `path` that are neither in the head nor put by an
    /// earlier batch, innermost first.
    fn missing_dirs(&self, path: &ArchivePath) -> Vec<ArchivePath> {
        let mut missing = Vec::new();
        let mut cur = path.parent();
        while let Some(dir) = cur {
            let in_head = self.head.as_ref().is_some_and(|h| h.get(&dir).is_some());
            if in_head || self.dirs_done.contains(&dir) {
                break;
            }
            cur = dir.parent();
            missing.push(dir);
        }
        missing
    }

    /// Starts over from the first file, e.g. after the batch that would have
    /// created the archive was consumed by a lost creation race: its
    /// directories must be put again.
    fn restart(&mut self) {
        self.next = 0;
        self.dirs_done.clear();
    }
}

/// Metadata of a staged file, or `None` if it vanished or is not a regular
/// file (such files are skipped).
fn staged_metadata(file: &StoreFile) -> Result<Option<fs::Metadata>> {
    match fs::metadata(&file.staged_path) {
        Ok(m) if m.is_file() => Ok(Some(m)),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ArchiveError::io(&file.staged_path, e)),
    }
}

fn mtime_of(meta: &fs::Metadata) -> Option<Mtime> {
    let d = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(Mtime {
        secs: i64::try_from(d.as_secs()).ok()?,
        nanos: d.subsec_nanos(),
    })
}

/// What this call's batches produced so far.
#[derive(Default)]
struct Commits {
    /// (entry, commit seq); the hashes are read back after the last commit.
    pending: Vec<(PendingEntry, u64)>,
    /// Sequence numbers of the commits made, in order.
    seqs: Vec<u64>,
    /// The first reason a commit's directory entry was not confirmed durable.
    degraded: Option<String>,
}

impl Commits {
    /// `entries` were committed as `outcome`.
    fn committed(&mut self, entries: Vec<PendingEntry>, outcome: &CommitOutcome) {
        if let PublishDurability::DirectoryUnconfirmed(why) = &outcome.durability {
            self.degraded.get_or_insert_with(|| why.clone());
        }
        self.pending
            .extend(entries.into_iter().map(|e| (e, outcome.seq)));
        self.seqs.push(outcome.seq);
    }

    /// `entries` are already in the head at `seq`; no commit was made.
    fn unchanged(&mut self, entries: Vec<PendingEntry>, seq: u64) {
        self.pending.extend(entries.into_iter().map(|e| (e, seq)));
    }
}

/// The committed entries with the hashes MOCHI computed while it streamed
/// each file, and the lengths it actually read (one replay of the head).
fn read_back_entries(
    archive_path: &Path,
    cat: &Catalog,
    pending: Vec<(PendingEntry, u64)>,
) -> Result<Vec<StoredEntry>> {
    let map = |e: MochiError| ArchiveError::from_mochi(archive_path, e);
    let head = cat.replay(None).map_err(map)?;
    let mut written = Vec::with_capacity(pending.len());
    for (e, seq) in pending {
        let (content_hash, size) = match e.content_hash {
            Some(h) if e.skipped => (h, e.size),
            _ => stored_file(cat, &head, &e.path)
                .map_err(map)?
                .ok_or_else(|| {
                    verification_failed(
                        archive_path,
                        format!(
                            "{} is not a file in the head after its commit",
                            e.relative_path
                        ),
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
    Ok(written)
}
