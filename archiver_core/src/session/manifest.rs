//! The staging manifest, `staging/manifest.jsonl`: which files a run moved
//! into staging, so that a crashed run can be finished by the next one.
//!
//! One JSON object per line: a header with the session id, then one line per
//! staged file. The file is append-only, so a crash can lose at most the
//! last unsynced lines, and a truncated trailing line is ignored on read.

use crate::error::{ArchiveError, Result};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const MANIFEST_FILENAME: &str = "manifest.jsonl";

/// The files a run listed in staging.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StagingManifest {
    pub scan_session_id: i64,
    pub files: Vec<StagedManifestEntry>,
}

/// Details about a single file in the staging manifest.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StagedManifestEntry {
    pub relative_path: String,
    pub source_subfolder: String,
    pub barcode: String,
    pub original_size: u64,
    /// The "YYYY-MM" archive file this entry belongs in.
    /// This is based on the barcode's original date, not today's date.
    pub volume_name: String,
}

/// A line in the `manifest.jsonl` file.
/// The first line is always the Header, and the rest are File Entries.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum ManifestLine {
    Header { scan_session_id: i64 },
    Entry(StagedManifestEntry),
}

impl StagingManifest {
    pub(crate) fn path_in(staging_dir: &Path) -> PathBuf {
        staging_dir.join(MANIFEST_FILENAME)
    }

    /// Reads the manifest from disk to resume a previous interrupted run.
    /// Safely ignores incomplete lines (caused by crashes mid-write)
    /// and treats them as un-staged.
    pub fn read_if_exists(staging_dir: &Path) -> Result<Option<Self>> {
        let path = Self::path_in(staging_dir);
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path).map_err(|e| ArchiveError::io(&path, e))?;
        let mut lines = content.lines();

        let Some(header_line) = lines.next() else {
            return Ok(None);
        };
        let scan_session_id = match serde_json::from_str::<ManifestLine>(header_line) {
            Ok(ManifestLine::Header { scan_session_id }) => scan_session_id,
            _ => return Ok(None),
        };

        let files = lines
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| match serde_json::from_str::<ManifestLine>(line) {
                Ok(ManifestLine::Entry(entry)) => Some(entry),
                _ => None, // a corrupted (e.g. truncated) line
            })
            .collect();

        Ok(Some(StagingManifest {
            scan_session_id,
            files,
        }))
    }
}

/// Writes entries to `manifest.jsonl` as files are moved into staging.
/// Flushes to disk periodically to balance write speed and crash safety.
pub struct StagingManifestWriter {
    file: std::io::BufWriter<fs::File>,
    pending_since_flush: usize,
}

impl StagingManifestWriter {
    /// How many files to stage before forcing a save to the disk.
    pub const FLUSH_EVERY: usize = 2000;

    /// Creates the manifest file and writes the initial session header.
    pub fn create(staging_dir: &Path, scan_session_id: i64) -> Result<Self> {
        let path = StagingManifest::path_in(staging_dir);
        let file = fs::File::create(&path).map_err(|e| ArchiveError::io(&path, e))?;
        let mut writer = StagingManifestWriter {
            file: std::io::BufWriter::new(file),
            pending_since_flush: 0,
        };
        writer.write_line(&ManifestLine::Header { scan_session_id })?;
        writer.flush()?;
        Ok(writer)
    }

    /// Records a newly staged file into the manifest.
    pub fn record(&mut self, entry: StagedManifestEntry) -> Result<()> {
        self.write_line(&ManifestLine::Entry(entry))?;
        self.pending_since_flush += 1;
        if self.pending_since_flush >= Self::FLUSH_EVERY {
            self.flush()?;
        }
        Ok(())
    }

    fn write_line(&mut self, line: &ManifestLine) -> Result<()> {
        let json = serde_json::to_string(line)
            .map_err(|e| ArchiveError::Config(format!("failed to serialize manifest line: {e}")))?;
        writeln!(self.file, "{json}").map_err(|e| ArchiveError::io(MANIFEST_FILENAME, e))
    }

    /// Forces unsaved lines to be written to disk and synced (`fsync`), so a
    /// power cut cannot keep a rename while losing the line that lists it.
    pub fn flush(&mut self) -> Result<()> {
        self.file
            .flush()
            .map_err(|e| ArchiveError::io(MANIFEST_FILENAME, e))?;
        self.file
            .get_ref()
            .sync_data()
            .map_err(|e| ArchiveError::io(MANIFEST_FILENAME, e))?;
        self.pending_since_flush = 0;
        Ok(())
    }

    /// Finalizes the writer, flushing any leftover lines.
    pub fn finish(mut self) -> Result<()> {
        self.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(i: usize) -> StagedManifestEntry {
        StagedManifestEntry {
            relative_path: format!("BURNIN/unit_BC{i:03}_log.txt"),
            source_subfolder: "BURNIN".to_string(),
            barcode: format!("BC{i:03}"),
            original_size: 100 + i as u64,
            volume_name: "2024-07".to_string(),
        }
    }

    /// Tests that the jsonl manifest can be written and read back correctly.
    #[test]
    fn manifest_jsonl_write_then_read_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let staging_dir = tmp.path();

        let mut writer = StagingManifestWriter::create(staging_dir, 42).unwrap();
        for i in 0..5 {
            writer.record(entry(i)).unwrap();
        }
        writer.finish().unwrap();

        let read_back = StagingManifest::read_if_exists(staging_dir)
            .unwrap()
            .unwrap();
        assert_eq!(read_back.scan_session_id, 42);
        assert_eq!(read_back.files.len(), 5);
        assert_eq!(read_back.files[0].barcode, "BC000");
        assert_eq!(read_back.files[4].barcode, "BC004");
    }

    /// Simulates a crash mid-write. Verifies the reader safely ignores
    /// incomplete lines without failing the entire operation.
    #[test]
    fn manifest_jsonl_skips_truncated_trailing_line() {
        let tmp = tempfile::tempdir().unwrap();
        let staging_dir = tmp.path();

        let mut writer = StagingManifestWriter::create(staging_dir, 7).unwrap();
        writer.record(entry(1)).unwrap();
        writer.finish().unwrap();

        // Write a malformed/partial entry to simulate a crash mid-save.
        let path = StagingManifest::path_in(staging_dir);
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "{{\"relative_path\":\"BURNIN/unit_BC002_l").unwrap();
        drop(f);

        let read_back = StagingManifest::read_if_exists(staging_dir)
            .unwrap()
            .unwrap();
        assert_eq!(read_back.scan_session_id, 7);
        assert_eq!(
            read_back.files.len(),
            1,
            "truncated trailing entry must be dropped, not fail the read"
        );
        assert_eq!(read_back.files[0].barcode, "BC001");
    }
}
