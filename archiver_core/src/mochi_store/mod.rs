//! Monthly MOCHI archives: write staged files in, then prove they are there.
//!
//! One archive per month, `<output_dir>/<YYYY>/<YYYY-MM>.mochi`. Inside, every
//! file lives at `<SUBFOLDER>/<filename>` (the same `relative_path` the
//! matcher produces), with an explicit directory entry for the subfolder.
//!
//! The module has two halves:
//!
//! * [`write`]: [`store_volume`] creates the archive if it is missing (never
//!   replacing anything) or appends to it with `TailRepair::Refuse`, streams
//!   every file into bounded commits and closes the writer.
//! * [`verify`]: [`verify_volume_with`] reopens the archive read-only and
//!   checks every staged file against it, byte for byte, while MOCHI checks
//!   the archive's structure at the same time.
//!
//! The session deletes a staged file only after both halves returned `Ok`.
//!
//! "Committed" here means `LOCAL_COMMITTED`: one local file was synced. It is
//! not a backup; keep an independent copy of the archive directory.

mod verify;
mod write;

pub use verify::{verify_volume, verify_volume_with};
pub use write::{store_volume, store_volume_with, store_volume_with_budget};

use crate::error::{ArchiveError, Result};
use mochi_core::catalog::namespace::{EntryKind, Snapshot};
use mochi_core::catalog::path::ArchivePath;
use mochi_core::catalog::Catalog;
use mochi_core::job::{CancellationToken, JobContext, NullProgress};
use mochi_core::publish::{open_head, OpenedHead, ReadOptions};
use mochi_core::storage::os::OsReadStorage;
use mochi_core::verify::VerifiedHead;
use mochi_core::MochiError;
use std::fs;
use std::path::{Path, PathBuf};

/// Upper bound on the file bytes in one commit (1 GiB). Content is streamed,
/// so this does not bound memory; it bounds the work a crash throws away. A
/// larger file is committed alone.
pub const COMMIT_BYTE_BUDGET: u64 = 1024 * 1024 * 1024;

/// Upper bound on the files in one commit. Small log files are the norm, so
/// without this a month of 300,000 files would be a single commit.
pub const COMMIT_FILE_BUDGET: usize = 20_000;

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
#[derive(Debug, Clone, Default)]
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
    let f = fs::File::open(path).map_err(|e| ArchiveError::io(path, e))?;
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(std::io::BufReader::with_capacity(256 * 1024, f))
        .map_err(|e| ArchiveError::io(path, e))?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Runs `f` with a job context that reports no progress and is never
/// cancelled.
pub(crate) fn with_job_ctx<T>(f: impl FnOnce(&JobContext<'_>) -> T) -> T {
    let cancel = CancellationToken::new();
    f(&JobContext {
        progress: &NullProgress,
        cancel: &cancel,
    })
}

/// Opens the archive at `path` read-only, without opening its head.
fn open_storage(path: &Path) -> Result<OsReadStorage> {
    OsReadStorage::open(path).map_err(|e| ArchiveError::from_mochi(path, MochiError::from(e)))
}

/// Opens the archive at `path` read-only, together with its verified head.
pub(crate) fn open_read(path: &Path) -> Result<(OsReadStorage, OpenedHead)> {
    let src = open_storage(path)?;
    let head =
        open_head(&src, &ReadOptions::default()).map_err(|e| ArchiveError::from_mochi(path, e))?;
    Ok((src, head))
}

/// The identity of an opened head.
pub(crate) fn verified_head(head: &OpenedHead) -> VerifiedHead {
    VerifiedHead {
        archive_id: head.commit.archive_id,
        seq: head.seq(),
        commit_id: head.commit_id,
    }
}

/// Builds an [`ArchivePath`] from a `/`-separated relative path.
pub fn archive_path_of(relative_path: &str) -> Result<ArchivePath> {
    ArchivePath::from_components(relative_path.split('/'))
        .map_err(|e| ArchiveError::from_mochi(Path::new(relative_path), e))
}

/// Content hash (hex) and length of the file at `path` in `snapshot`, or
/// `None` if it is absent or not a regular file.
fn stored_file(
    cat: &Catalog,
    snapshot: &Snapshot,
    path: &ArchivePath,
) -> std::result::Result<Option<(String, u64)>, MochiError> {
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

fn verification_failed(volume: &Path, reason: impl Into<String>) -> ArchiveError {
    ArchiveError::VerificationFailed {
        volume: volume.display().to_string(),
        reason: reason.into(),
    }
}
