//! Reads parts (chunks) of a volume file back out.
//! Used to verify freshly written chunks in memory, or to extract specific files by jumping straight to their location.

use crate::error::{ ArchiveError, Result };
use std::fs::File;
use std::io::{ Read, Seek, SeekFrom, Take };
use std::path::Path;

/// Opens a specific section of a file as its own standalone archive.
pub fn open_chunk_as_tar(
    volume_path: &Path,
    chunk_start_byte: u64,
    chunk_end_byte: u64
) -> Result<tar::Archive<zstd::stream::Decoder<'static, std::io::BufReader<Take<File>>>>> {
    let mut file = File::open(volume_path).map_err(|e| ArchiveError::io(volume_path, e))?;
    file.seek(SeekFrom::Start(chunk_start_byte)).map_err(|e| ArchiveError::io(volume_path, e))?;
    let bounded = file.take(chunk_end_byte - chunk_start_byte);
    let decoder = zstd::stream::Decoder::new(bounded).map_err(ArchiveError::Zstd)?;
    Ok(tar::Archive::new(decoder))
}

/// Tests a chunk in memory right after saving to make sure the data is intact.
/// Returns the total number of files found inside.
pub fn verify_chunk_in_ram(
    volume_path: &Path,
    chunk_start_byte: u64,
    chunk_end_byte: u64
) -> Result<usize> {
    let mut archive = open_chunk_as_tar(volume_path, chunk_start_byte, chunk_end_byte)?;
    let entries = archive.entries().map_err(ArchiveError::Tar)?;

    let mut count = 0usize;
    for entry in entries {
        let mut entry = entry.map_err(ArchiveError::Tar)?;
        let mut sink = std::io::sink();
        std::io::copy(&mut entry, &mut sink).map_err(|e| ArchiveError::io(volume_path, e))?;
        count += 1;
    }
    Ok(count)
}

/// Quickly jumps directly to a chunk, then searches inside it to extract a specific file to disk.
/// Note: Jumping to the chunk is instant, but scanning inside it takes a bit of time.
pub fn extract_file_from_chunk(
    volume_path: &Path,
    chunk_start_byte: u64,
    chunk_end_byte: u64,
    relative_path: &str,
    dest_path: &Path
) -> Result<u64> {
    let mut archive = open_chunk_as_tar(volume_path, chunk_start_byte, chunk_end_byte)?;
    let entries = archive.entries().map_err(ArchiveError::Tar)?;

    for entry in entries {
        let mut entry = entry.map_err(ArchiveError::Tar)?;
        let entry_path = entry.path().map_err(ArchiveError::Tar)?.to_string_lossy().to_string();
        if entry_path != relative_path {
            continue;
        }

        if let Some(parent) = dest_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
            }
        }
        let mut out = File::create(dest_path).map_err(|e| ArchiveError::io(dest_path, e))?;
        let bytes = std::io
            ::copy(&mut entry, &mut out)
            .map_err(|e| ArchiveError::io(dest_path, e))?;
        return Ok(bytes);
    }

    Err(ArchiveError::EntryNotFound(relative_path.to_string()))
}

/// Opens the entire volume file as one giant stream, reading all chunks back-to-back.
pub fn open_volume_as_tar(
    volume_path: &Path
) -> Result<tar::Archive<zstd::stream::Decoder<'static, std::io::BufReader<File>>>> {
    let file = File::open(volume_path).map_err(|e| ArchiveError::io(volume_path, e))?;
    let decoder = zstd::stream::Decoder::new(file).map_err(ArchiveError::Zstd)?;
    let mut archive = tar::Archive::new(decoder);
    archive.set_ignore_zeros(true);
    Ok(archive)
}

/// A simple record of a file's path and size when browsing the full volume.
#[derive(Debug, Clone)]
pub struct VolumeEntry {
    pub relative_path: String,
    pub size: u64,
}

/// Lists every file in the volume without loading their contents.
/// This keeps it fast and memory-efficient while updating a progress counter for the UI.
pub fn list_volume_entries(
    volume_path: &Path,
    mut on_progress: impl FnMut(usize)
) -> Result<Vec<VolumeEntry>> {
    let mut archive = open_volume_as_tar(volume_path)?;
    let entries = archive.entries().map_err(ArchiveError::Tar)?;

    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(ArchiveError::Tar)?;
        let relative_path = entry.path().map_err(ArchiveError::Tar)?.to_string_lossy().to_string();
        let size = entry.header().size().map_err(ArchiveError::Tar)?;
        out.push(VolumeEntry { relative_path, size });
        if out.len() % 500 == 0 {
            on_progress(out.len());
        }
    }
    on_progress(out.len());
    Ok(out)
}

/// Searches the whole volume from the beginning to extract a specific file.
/// This is slower than `extract_file_from_chunk` because it doesn't know where to jump to first.
pub fn extract_entry_from_volume(
    volume_path: &Path,
    relative_path: &str,
    dest_path: &Path
) -> Result<u64> {
    let mut archive = open_volume_as_tar(volume_path)?;
    let entries = archive.entries().map_err(ArchiveError::Tar)?;

    for entry in entries {
        let mut entry = entry.map_err(ArchiveError::Tar)?;
        let entry_path = entry.path().map_err(ArchiveError::Tar)?.to_string_lossy().to_string();
        if entry_path != relative_path {
            continue;
        }

        if let Some(parent) = dest_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
            }
        }
        let mut out = File::create(dest_path).map_err(|e| ArchiveError::io(dest_path, e))?;
        let bytes = std::io
            ::copy(&mut entry, &mut out)
            .map_err(|e| ArchiveError::io(dest_path, e))?;
        return Ok(bytes);
    }

    Err(ArchiveError::EntryNotFound(relative_path.to_string()))
}

/// Extracts every single file in the volume into a target folder, keeping the original folder structure.
pub fn extract_all_from_volume(
    volume_path: &Path,
    dest_dir: &Path,
    mut on_progress: impl FnMut(usize)
) -> Result<Vec<ExtractedFile>> {
    let mut archive = open_volume_as_tar(volume_path)?;
    let entries = archive.entries().map_err(ArchiveError::Tar)?;

    let mut results = Vec::new();
    for entry in entries {
        let mut entry = entry.map_err(ArchiveError::Tar)?;
        let relative_path = entry.path().map_err(ArchiveError::Tar)?.to_string_lossy().to_string();
        let dest_path = dest_dir.join(&relative_path);
        if let Some(parent) = dest_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| ArchiveError::io(parent, e))?;
            }
        }
        let mut out = File::create(&dest_path).map_err(|e| ArchiveError::io(&dest_path, e))?;
        let bytes_written = std::io
            ::copy(&mut entry, &mut out)
            .map_err(|e| ArchiveError::io(&dest_path, e))?;
        results.push(ExtractedFile {
            relative_path,
            dest_path,
            bytes_written,
        });
        on_progress(results.len());
    }
    Ok(results)
}

/// Loads a small piece of a file into memory so you can preview it without saving it to disk.
pub fn preview_entry_from_volume(
    volume_path: &Path,
    relative_path: &str,
    max_bytes: u64
) -> Result<(Vec<u8>, bool)> {
    let mut archive = open_volume_as_tar(volume_path)?;
    let entries = archive.entries().map_err(ArchiveError::Tar)?;

    for entry in entries {
        let entry = entry.map_err(ArchiveError::Tar)?;
        let entry_path = entry.path().map_err(ArchiveError::Tar)?.to_string_lossy().to_string();
        if entry_path != relative_path {
            continue;
        }
        let full_size = entry.header().size().map_err(ArchiveError::Tar)?;
        let mut limited = entry.take(max_bytes);
        let mut buf = Vec::new();
        limited.read_to_end(&mut buf).map_err(|e| ArchiveError::io(volume_path, e))?;
        return Ok((buf, full_size > max_bytes));
    }

    Err(ArchiveError::EntryNotFound(relative_path.to_string()))
}

/// Quickly jumps to a chunk to find a file, but keeps it in memory instead of saving it.
/// Useful for small files or quick previews.
pub fn extract_file_from_chunk_to_bytes(
    volume_path: &Path,
    chunk_start_byte: u64,
    chunk_end_byte: u64,
    relative_path: &str
) -> Result<Vec<u8>> {
    let mut archive = open_chunk_as_tar(volume_path, chunk_start_byte, chunk_end_byte)?;
    let entries = archive.entries().map_err(ArchiveError::Tar)?;

    for entry in entries {
        let mut entry = entry.map_err(ArchiveError::Tar)?;
        let entry_path = entry.path().map_err(ArchiveError::Tar)?.to_string_lossy().to_string();
        if entry_path != relative_path {
            continue;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| ArchiveError::io(volume_path, e))?;
        return Ok(buf);
    }

    Err(ArchiveError::EntryNotFound(relative_path.to_string()))
}

/// Keeps track of a file's original name, where it was saved, and how many bytes were written.
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    pub relative_path: String,
    pub dest_path: std::path::PathBuf,
    pub bytes_written: u64,
}

/// Uses the database to find all files related to a barcode, then quickly extracts them straight from their exact chunks.
pub fn extract_by_barcode(
    conn: &rusqlite::Connection,
    barcode: &str,
    dest_dir: &Path
) -> Result<Vec<ExtractedFile>> {
    let locations = crate::repo::find_entries_by_barcode(conn, barcode)?;
    let mut results = Vec::with_capacity(locations.len());

    for loc in locations {
        let volume_path = Path::new(&loc.volume_path);
        let dest_path = dest_dir.join(&loc.relative_path);
        let bytes_written = extract_file_from_chunk(
            volume_path,
            loc.chunk_start_byte,
            loc.chunk_end_byte,
            &loc.relative_path,
            &dest_path
        )?;
        results.push(ExtractedFile {
            relative_path: loc.relative_path,
            dest_path,
            bytes_written,
        });
    }

    Ok(results)
}
