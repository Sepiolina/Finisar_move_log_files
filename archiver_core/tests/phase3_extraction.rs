//! Phase 3 test criteria, verbatim from spec:
//! - extract a known file by barcode, diff its bytes against the
//!   original pre-archive copy

use archiver_core::reader::{extract_by_barcode, extract_file_from_chunk_to_bytes};
use archiver_core::repo;
use archiver_core::schema::open_in_memory;
use archiver_core::ChunkedArchiveWriter;

/// Extracts a plausible barcode out of a `<anything>_<BARCODE>_<anything>.ext`
/// style filename for wiring test fixtures — Phase 4 will replace this
/// with the real Aho-Corasick + delimiter-boundary matcher.
fn naive_barcode_from_path(path: &str) -> Option<String> {
    let filename = path.rsplit('/').next()?;
    let stem = filename.rsplit_once('.').map(|(s, _)| s).unwrap_or(filename);
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() >= 3 {
        Some(parts[1].to_string())
    } else {
        None
    }
}

fn naive_subfolder_from_path(path: &str) -> String {
    path.split('/').next().unwrap_or("").to_string()
}

#[test]
fn extract_known_file_by_barcode_matches_original_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let volume_path = dir.path().join("2024-07.tar.zst");

    // Build a small multi-file archive spanning a few "barcodes".
    let mut writer = ChunkedArchiveWriter::open_or_append(&volume_path, 3).unwrap();

    let mut originals: Vec<(String, Vec<u8>)> = Vec::new();
    for (subfolder, barcode, idx) in [
        ("BURNIN", "BC001", 0u8),
        ("ESS", "BC001", 1u8),
        ("BURNIN", "BC002", 2u8),
        ("Xray", "BC003", 3u8),
    ] {
        let rel = format!("{subfolder}/unit_{barcode}_run.txt");
        let data: Vec<u8> = (0..4096u32).map(|i| ((i as u8).wrapping_add(idx))).collect();
        writer.add_file(&rel, data.as_slice(), data.len() as u64).unwrap();
        originals.push((rel, data));
    }
    let finished = writer.finish().unwrap();

    // Index it via Phase 2.
    let mut conn = open_in_memory().unwrap();
    let session_id = repo::start_session(&conn, "t", "2024-06-01", "2024-06-29").unwrap();
    repo::record_finished_volume(
        &mut conn,
        session_id,
        "2024-07",
        &finished,
        "2024-07-01T00:00:00Z",
        |path| naive_barcode_from_path(path),
        |path| naive_subfolder_from_path(path),
    )
    .unwrap();

    // Extract BC001's files (2 of them) to a dest dir and diff against originals.
    let dest_dir = dir.path().join("extracted");
    let extracted = extract_by_barcode(&conn, "BC001", &dest_dir).unwrap();
    assert_eq!(extracted.len(), 2, "BC001 should have exactly 2 matching entries");

    for ext in &extracted {
        let original = originals
            .iter()
            .find(|(rel, _)| *rel == ext.relative_path)
            .expect("extracted path should correspond to a known original");
        let extracted_bytes = std::fs::read(&ext.dest_path).unwrap();
        assert_eq!(&extracted_bytes, &original.1, "extracted bytes for {} must match original", ext.relative_path);
        assert_eq!(ext.bytes_written as usize, original.1.len());
    }

    // BC003 should extract its single file too, via the in-memory variant.
    let bc003_locations = repo::find_entries_by_barcode(&conn, "BC003").unwrap();
    assert_eq!(bc003_locations.len(), 1);
    let loc = &bc003_locations[0];
    let bytes = extract_file_from_chunk_to_bytes(
        std::path::Path::new(&loc.volume_path),
        loc.chunk_start_byte,
        loc.chunk_end_byte,
        &loc.relative_path,
    )
    .unwrap();
    let original = originals.iter().find(|(rel, _)| *rel == loc.relative_path).unwrap();
    assert_eq!(&bytes, &original.1);

    // A barcode with no entries should extract nothing (not error).
    let empty = extract_by_barcode(&conn, "BC999_DOES_NOT_EXIST", &dest_dir).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn extract_nonexistent_relative_path_in_chunk_errors() {
    let dir = tempfile::tempdir().unwrap();
    let volume_path = dir.path().join("2024-07.tar.zst");

    let mut writer = ChunkedArchiveWriter::open_or_append(&volume_path, 3).unwrap();
    writer.add_file("BURNIN/only_file.txt", &b"hello"[..], 5).unwrap();
    let finished = writer.finish().unwrap();
    let span = finished.chunk_spans[0];

    let result = archiver_core::reader::extract_file_from_chunk(
        &volume_path,
        span.chunk_start_byte,
        span.chunk_end_byte,
        "BURNIN/does_not_exist.txt",
        &dir.path().join("out.txt"),
    );
    assert!(result.is_err(), "extracting a path that isn't in the chunk should error, not silently succeed");
}
