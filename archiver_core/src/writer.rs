use crate::error::{ArchiveError, Result};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Clamps a compression level to what the linked zstd library supports.
/// Positive = normal levels (1-22). Negative = zstd's "fast" mode
/// (`--fast=4` CLI flag == level `-4` here).
pub fn clamp_compression_level(level: i32) -> i32 {
    let range = zstd::compression_level_range();
    level.clamp(*range.start(), *range.end())
}

/// A chunk closes once it holds this many uncompressed bytes.
pub const CHUNK_SIZE_THRESHOLD: u64 = 50 * 1024 * 1024; // 50MB

/// Byte range of one finished chunk within the output file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkSpan {
    pub chunk_start_byte: u64,
    pub chunk_end_byte: u64,
}

/// Info about one written file, for Phase 2's `scanned_log_entry` table.
#[derive(Debug, Clone)]
pub struct WrittenEntry {
    pub relative_path: String,
    pub original_size: u64,
    pub chunk_start_byte: u64,
    pub chunk_end_byte: u64,
}

/// A buffered file handle, wrapped so it can be shared (see `SharedSink`).
struct CountingFileSink {
    file: BufWriter<File>,
}

impl Write for CountingFileSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Cloneable handle to the same file — lets both the zstd encoder and our
/// own position-checking code use it, without `unsafe`.
#[derive(Clone)]
struct SharedSink(Rc<RefCell<CountingFileSink>>);

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

/// The chunk currently being written to.
struct ActiveChunk {
    tar_builder: tar::Builder<zstd::stream::Encoder<'static, SharedSink>>,
    start_byte: u64,
    uncompressed_bytes: u64,
}

/// Writes files into a chunked `.tar.zst` archive, rolling over to a new
/// chunk past 50MB uncompressed. Not thread-safe (uses `Rc`) — drive from
/// one thread, send progress elsewhere via channel/callback.
pub struct ChunkedArchiveWriter {
    volume_path: PathBuf,
    sink: Rc<RefCell<CountingFileSink>>,
    compression_level: i32,
    active: Option<ActiveChunk>,
    pending_entries: Vec<(String, u64)>,
    pub written_entries: Vec<WrittenEntry>,
    pub chunk_spans: Vec<ChunkSpan>,
}

impl ChunkedArchiveWriter {
    /// Opens the archive, creating it if needed. Existing chunks are left
    /// alone; new chunks are appended after them. See
    /// `clamp_compression_level` for what `compression_level` accepts.
    pub fn open_or_append(volume_path: impl AsRef<Path>, compression_level: i32) -> Result<Self> {
        let volume_path = volume_path.as_ref().to_path_buf();
        if let Some(parent) = volume_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&volume_path)
            .map_err(|e| ArchiveError::io(&volume_path, e))?;

        Ok(Self {
            volume_path,
            sink: Rc::new(RefCell::new(CountingFileSink {
                file: BufWriter::new(file),
            })),
            compression_level,
            active: None,
            pending_entries: Vec::new(),
            written_entries: Vec::new(),
            chunk_spans: Vec::new(),
        })
    }

    /// Current file size on disk (flushes first for an accurate read).
    fn current_file_end(&self) -> Result<u64> {
        let mut sink = self.sink.borrow_mut();
        sink.file
            .flush()
            .map_err(|e| ArchiveError::io(&self.volume_path, e))?;
        sink.file
            .get_mut()
            .seek(SeekFrom::End(0))
            .map_err(|e| ArchiveError::io(&self.volume_path, e))
    }

    /// Starts a new chunk if one isn't already open.
    fn ensure_chunk_open(&mut self) -> Result<()> {
        if self.active.is_some() {
            return Ok(());
        }
        let start_byte = self.current_file_end()?;

        let level = clamp_compression_level(self.compression_level);
        let encoder = zstd::stream::Encoder::new(SharedSink(Rc::clone(&self.sink)), level)
            .map_err(ArchiveError::Zstd)?;
        let tar_builder = tar::Builder::new(encoder);

        self.active = Some(ActiveChunk {
            tar_builder,
            start_byte,
            uncompressed_bytes: 0,
        });
        Ok(())
    }

    /// Adds one file into the current (or a new) chunk. `relative_path`
    /// is its path inside the tar. Rollover is checked *after* writing,
    /// so a chunk may end up slightly over 50MB rather than split mid-file.
    pub fn add_file(&mut self, relative_path: &str, mut reader: impl Read, size_hint: u64) -> Result<()> {
        self.ensure_chunk_open()?;
        let active = self.active.as_mut().expect("just ensured open");

        let mut header = tar::Header::new_gnu();
        header.set_size(size_hint);
        header.set_mode(0o644);
        header.set_mtime(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
        header.set_cksum();

        active
            .tar_builder
            .append_data(&mut header, relative_path, &mut reader)
            .map_err(ArchiveError::Tar)?;

        // Tar pads data to 512-byte blocks and adds a 512-byte header —
        // track that padded size so the threshold check is accurate.
        let padded_data = ((size_hint + 511) / 512) * 512;
        active.uncompressed_bytes += 512 + padded_data; // header block + padded data

        self.pending_entries.push((relative_path.to_string(), size_hint));

        if active.uncompressed_bytes >= CHUNK_SIZE_THRESHOLD {
            self.close_chunk()?;
        }

        Ok(())
    }

    /// Closes the current chunk: writes tar's end-of-archive marker,
    /// finishes the zstd frame, and records the chunk's byte span. Every
    /// chunk gets its own marker, which is what makes it independently
    /// readable. No-op if no chunk is open.
    pub fn close_chunk(&mut self) -> Result<()> {
        let Some(active) = self.active.take() else {
            return Ok(());
        };

        // Writes the tar end-of-archive marker, returns the zstd encoder.
        let encoder = active.tar_builder.into_inner().map_err(ArchiveError::Tar)?;

        // Closes the zstd frame — required for the chunk to be
        // independently decompressible.
        encoder.finish().map_err(ArchiveError::Zstd)?;

        let end_byte = self.current_file_end()?;

        let span = ChunkSpan {
            chunk_start_byte: active.start_byte,
            chunk_end_byte: end_byte,
        };
        self.chunk_spans.push(span);

        for (relative_path, original_size) in self.pending_entries.drain(..) {
            self.written_entries.push(WrittenEntry {
                relative_path,
                original_size,
                chunk_start_byte: span.chunk_start_byte,
                chunk_end_byte: span.chunk_end_byte,
            });
        }

        Ok(())
    }

    /// Call once all files are added: closes any open chunk, flushes to
    /// disk, and hashes the whole file.
    pub fn finish(mut self) -> Result<FinishedVolume> {
        self.close_chunk()?;
        self.sink
            .borrow_mut()
            .file
            .flush()
            .map_err(|e| ArchiveError::io(&self.volume_path, e))?;
        drop(self.sink);

        let sha256 = hash_file(&self.volume_path)?;
        let file_size = std::fs::metadata(&self.volume_path)
            .map_err(|e| ArchiveError::io(&self.volume_path, e))?
            .len();

        Ok(FinishedVolume {
            path: self.volume_path,
            file_size,
            sha256,
            entry_count: self.written_entries.len(),
            chunk_spans: self.chunk_spans,
            written_entries: self.written_entries,
        })
    }
}

/// Hashes an entire file. Used after `finish()` and after every Phase 2
/// append — there's no incremental shortcut, so it re-hashes fully.
pub fn hash_file(path: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut f = File::open(path).map_err(|e| ArchiveError::io(path, e))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| ArchiveError::io(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Everything Phase 2 needs to save results into `file_storage_volume`
/// and `scanned_log_entry`.
#[derive(Debug)]
pub struct FinishedVolume {
    pub path: PathBuf,
    pub file_size: u64,
    pub sha256: String,
    pub entry_count: usize,
    pub chunk_spans: Vec<ChunkSpan>,
    pub written_entries: Vec<WrittenEntry>,
}