//! Reads MOCHI volumes back: list, extract, preview.
//!
//! Everything goes through `mochi-core`'s verified read path (`read::list`,
//! `read::read_file`). Archive-supplied names are untrusted: whenever a file
//! is written to disk its destination is built with [`safe_join`], which
//! accepts only plain path components (no `..`, no root, no drive prefix).
//! Extraction writes to a temporary sibling file and renames it into place only
//! after `read_file` returned `Ok`, so a damaged file is never published.

use crate::error::{ArchiveError, Result};
use crate::mochi_store::{archive_path_of, open_read, with_job_ctx};
use mochi_core::catalog::namespace::{EntryKind, Snapshot};
use mochi_core::publish::{OpenedHead, ReadOptions};
use mochi_core::storage::os::OsReadStorage;
use mochi_core::MochiError;
use std::collections::hash_map::{Entry, HashMap};
use std::fs::File;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// An opened, read-only volume.
struct OpenVolume {
    src: OsReadStorage,
    head: OpenedHead,
    /// The head namespace, replayed once at open: reading by version from it
    /// keeps extracting N files linear (`read::read_file` replays per call).
    snapshot: Snapshot,
    ro: ReadOptions,
    path: PathBuf,
}

impl OpenVolume {
    fn open(volume_path: &Path) -> Result<Self> {
        let (src, head) = open_read(volume_path)?;
        let snapshot = head
            .catalog
            .replay(None)
            .map_err(|e| ArchiveError::from_mochi(volume_path, e))?;
        Ok(OpenVolume {
            src,
            head,
            snapshot,
            ro: ReadOptions::default(),
            path: volume_path.to_path_buf(),
        })
    }

    fn map(&self, e: MochiError) -> ArchiveError {
        ArchiveError::from_mochi(&self.path, e)
    }

    /// Streams the file at `relative_path` into `out` (verified only once
    /// this returns `Ok`). Returns the logical length.
    fn read_file(&self, relative_path: &str, out: &mut dyn Write) -> Result<u64> {
        let path = archive_path_of(relative_path)?;
        let entry = self
            .snapshot
            .get(&path)
            .ok_or_else(|| ArchiveError::EntryNotFound(relative_path.to_string()))?;
        if entry.kind == EntryKind::Directory {
            return Err(self.map(MochiError::new(
                mochi_core::ErrorCode::InvalidArgument,
                format!("{relative_path:?} is a directory"),
            )));
        }
        with_job_ctx(|ctx| {
            mochi_core::read::read_version(
                &self.src,
                &self.head.catalog,
                &entry.version,
                out,
                &self.ro,
                ctx,
            )
        })
        .map(|r| r.logical_len)
        .map_err(|e| self.map(e))
    }

    /// Every file in the head (directories are skipped): archive-relative
    /// path and logical length.
    fn file_paths(&self) -> Result<impl Iterator<Item = (String, u64)>> {
        let listing = mochi_core::read::list(&self.head, None).map_err(|e| self.map(e))?;
        Ok(listing
            .into_iter()
            .filter(|e| e.kind == EntryKind::File)
            .map(|e| {
                (
                    String::from_utf8_lossy(e.path.as_stored()).into_owned(),
                    e.logical_len,
                )
            }))
    }

    /// Logical size of a file entry, if present.
    fn file_size(&self, relative_path: &str) -> Result<Option<u64>> {
        let path = archive_path_of(relative_path)?;
        let Some(entry) = self
            .snapshot
            .get(&path)
            .filter(|e| e.kind == EntryKind::File)
        else {
            return Ok(None);
        };
        Ok(self
            .head
            .catalog
            .file_version(&entry.version)
            .map_err(|e| self.map(e))?
            .map(|(v, _)| v.logical_len))
    }
}

/// Joins an archive-relative path (`/`-separated) onto `root`, accepting only
/// plain components. Rejects `..`, `.`, empty components, absolute paths and
/// Windows drive/UNC prefixes.
pub fn safe_join(root: &Path, relative_path: &str) -> Result<PathBuf> {
    if relative_path.is_empty() {
        return Err(ArchiveError::UnsafePath(relative_path.to_string()));
    }
    let mut out = root.to_path_buf();
    for part in relative_path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(ArchiveError::UnsafePath(relative_path.to_string()));
        }
        // On Windows `\` and `C:` would be re-interpreted by Path; Components
        // exposes that as non-Normal.
        let mut comps = Path::new(part).components();
        match (comps.next(), comps.next()) {
            (Some(Component::Normal(_)), None) => out.push(part),
            _ => return Err(ArchiveError::UnsafePath(relative_path.to_string())),
        }
    }
    Ok(out)
}

/// Reads one entry into `dest_path` via a temporary sibling file that is
/// renamed into place only after a verified read.
fn write_entry_to(vol: &OpenVolume, relative_path: &str, dest_path: &Path) -> Result<u64> {
    if let Some(parent) = dest_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
        }
    }
    let file_name = dest_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "extracted".to_string());
    let tmp = dest_path.with_file_name(format!(".{file_name}.partial"));
    let result = (|| -> Result<u64> {
        let mut out = File::create(&tmp).map_err(|e| ArchiveError::io(&tmp, e))?;
        let n = vol.read_file(relative_path, &mut out)?;
        out.flush().map_err(|e| ArchiveError::io(&tmp, e))?;
        drop(out);
        std::fs::rename(&tmp, dest_path).map_err(|e| ArchiveError::io(dest_path, e))?;
        Ok(n)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// A simple record of a file's path and size when browsing the full volume.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeEntry {
    pub relative_path: String,
    pub size: u64,
}

/// Lists every file in the volume (directories are skipped) without reading
/// any file content. `on_progress` receives the running count.
pub fn list_volume_entries(
    volume_path: &Path,
    mut on_progress: impl FnMut(usize),
) -> Result<Vec<VolumeEntry>> {
    let vol = OpenVolume::open(volume_path)?;
    let mut out = Vec::new();
    for (relative_path, size) in vol.file_paths()? {
        out.push(VolumeEntry {
            relative_path,
            size,
        });
        if out.len() % 500 == 0 {
            on_progress(out.len());
        }
    }
    on_progress(out.len());
    Ok(out)
}

/// Extracts one file to `dest_path` (overwriting). Returns bytes written.
pub fn extract_entry_from_volume(
    volume_path: &Path,
    relative_path: &str,
    dest_path: &Path,
) -> Result<u64> {
    let vol = OpenVolume::open(volume_path)?;
    write_entry_to(&vol, relative_path, dest_path)
}

/// Keeps track of a file's original name, where it was saved, and how many bytes were written.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedFile {
    pub relative_path: String,
    pub dest_path: PathBuf,
    pub bytes_written: u64,
}

/// Extracts every file of the volume into `dest_dir`, keeping the
/// `<SUBFOLDER>/<filename>` layout. Existing files are overwritten.
pub fn extract_all_from_volume(
    volume_path: &Path,
    dest_dir: &Path,
    mut on_progress: impl FnMut(usize),
) -> Result<Vec<ExtractedFile>> {
    let vol = OpenVolume::open(volume_path)?;
    let mut results = Vec::new();
    for (relative_path, _) in vol.file_paths()? {
        let dest_path = safe_join(dest_dir, &relative_path)?;
        let bytes_written = write_entry_to(&vol, &relative_path, &dest_path)?;
        results.push(ExtractedFile {
            relative_path,
            dest_path,
            bytes_written,
        });
        on_progress(results.len());
    }
    Ok(results)
}

/// A `Write` sink that keeps at most `limit` bytes and then aborts the read.
struct BoundedSink {
    buf: Vec<u8>,
    limit: usize,
    full: bool,
}

impl Write for BoundedSink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let room = self.limit - self.buf.len();
        if data.len() > room {
            self.buf.extend_from_slice(&data[..room]);
            self.full = true;
            return Err(std::io::Error::other("preview limit reached"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Loads at most `max_bytes` from the start of a file into memory. The
/// bool is `true` if the file is longer. The read is stopped early, so the
/// whole-file hash is not checked for a truncated preview (each chunk read
/// is still hash-verified).
pub fn preview_entry_from_volume(
    volume_path: &Path,
    relative_path: &str,
    max_bytes: u64,
) -> Result<(Vec<u8>, bool)> {
    let vol = OpenVolume::open(volume_path)?;
    let full_size = vol
        .file_size(relative_path)?
        .ok_or_else(|| ArchiveError::EntryNotFound(relative_path.to_string()))?;
    let limit = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let mut sink = BoundedSink {
        buf: Vec::new(),
        limit,
        full: false,
    };
    match vol.read_file(relative_path, &mut sink) {
        Ok(_) => Ok((sink.buf, false)),
        Err(_) if sink.full => Ok((sink.buf, full_size > max_bytes)),
        Err(e) => Err(e),
    }
}

/// Extracts one entry of a volume to memory (whole file).
pub fn extract_entry_to_bytes(volume_path: &Path, relative_path: &str) -> Result<Vec<u8>> {
    let vol = OpenVolume::open(volume_path)?;
    let mut buf = Vec::new();
    vol.read_file(relative_path, &mut buf)?;
    Ok(buf)
}

/// Uses the index to find all files for a barcode and extracts them into
/// `dest_dir/<SUBFOLDER>/<filename>`.
pub fn extract_by_barcode(
    conn: &rusqlite::Connection,
    barcode: &str,
    dest_dir: &Path,
) -> Result<Vec<ExtractedFile>> {
    let locations = crate::repo::find_entries_by_barcode(conn, barcode)?;
    extract_locations(&locations, dest_dir)
}

/// Extracts a set of indexed entries, opening each volume once.
pub fn extract_locations(
    locations: &[crate::repo::LogEntryLocation],
    dest_dir: &Path,
) -> Result<Vec<ExtractedFile>> {
    let mut volumes: HashMap<&str, OpenVolume> = HashMap::new();
    let mut results = Vec::with_capacity(locations.len());
    for loc in locations {
        let dest_path = safe_join(dest_dir, &loc.relative_path)?;
        let vol = match volumes.entry(loc.volume_path.as_str()) {
            Entry::Occupied(open) => open.into_mut(),
            Entry::Vacant(slot) => slot.insert(OpenVolume::open(Path::new(&loc.volume_path))?),
        };
        let bytes_written = write_entry_to(vol, &loc.relative_path, &dest_path)?;
        results.push(ExtractedFile {
            relative_path: loc.relative_path.clone(),
            dest_path,
            bytes_written,
        });
    }
    Ok(results)
}
