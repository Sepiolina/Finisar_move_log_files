//! File-viewer support: listing and extracting across *every* chunk in
//! a `.tar.zst` volume by scanning the file directly (no SQLite index
//! involved) — this is what fixes "opening the volume with a native
//! archive tool only shows the first chunk".

use archiver_core::reader;
use archiver_core::ChunkedArchiveWriter;

/// Writes `count` files of `size` bytes each (filled with a repeating,
/// index-derived pattern so corruption/truncation is detectable) into
/// the writer under `subfolder/`.
fn write_dummy_batch(
    writer: &mut ChunkedArchiveWriter,
    subfolder: &str,
    start_index: usize,
    count: usize,
    size: usize,
) -> Vec<(String, Vec<u8>)> {
    let mut written = Vec::new();
    for i in start_index..start_index + count {
        let name = format!("{subfolder}/dummy_{i:04}.txt");
        let mut data = vec![0u8; size];
        for (j, b) in data.iter_mut().enumerate() {
            *b = ((i + j) % 251) as u8;
        }
        writer
            .add_file(&name, data.as_slice(), size as u64)
            .expect("add_file should succeed");
        written.push((name, data));
    }
    written
}

#[test]
fn list_volume_entries_sees_every_chunk_not_just_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let volume_path = dir.path().join("2024-07.tar.zst");

    // Fast compression level — this test cares about chunk count, not ratio.
    let mut writer = ChunkedArchiveWriter::open_or_append(&volume_path, 1).unwrap();
    let written = write_dummy_batch(&mut writer, "JTAG", 0, 20, 5 * 1024 * 1024); // 100MB -> 3+ chunks
    let finished = writer.finish().unwrap();
    assert!(finished.chunk_spans.len() >= 2, "test setup should span multiple chunks");

    let mut progress_calls = 0usize;
    let entries = reader::list_volume_entries(&volume_path, |_| progress_calls += 1).unwrap();

    assert_eq!(entries.len(), written.len(), "listing must find every entry across every chunk");
    for (name, data) in &written {
        let found = entries.iter().find(|e| &e.relative_path == name);
        assert!(found.is_some(), "entry {name} should be listed");
        assert_eq!(found.unwrap().size, data.len() as u64);
    }
}

#[test]
fn extract_entry_from_volume_finds_files_in_later_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let volume_path = dir.path().join("2024-07.tar.zst");

    let mut writer = ChunkedArchiveWriter::open_or_append(&volume_path, 1).unwrap();
    let written = write_dummy_batch(&mut writer, "STM", 0, 20, 5 * 1024 * 1024);
    let finished = writer.finish().unwrap();
    assert!(finished.chunk_spans.len() >= 2);

    // Grab a file that should land in a later chunk (last one written).
    let (name, data) = written.last().unwrap();
    let dest = dir.path().join("out").join(name);
    let bytes_written = reader::extract_entry_from_volume(&volume_path, name, &dest).unwrap();

    assert_eq!(bytes_written, data.len() as u64);
    let on_disk = std::fs::read(&dest).unwrap();
    assert_eq!(&on_disk, data);
}

#[test]
fn extract_all_from_volume_recovers_every_file() {
    let dir = tempfile::tempdir().unwrap();
    let volume_path = dir.path().join("2024-07.tar.zst");

    let mut writer = ChunkedArchiveWriter::open_or_append(&volume_path, 1).unwrap();
    let written = write_dummy_batch(&mut writer, "ESS", 0, 20, 5 * 1024 * 1024);
    writer.finish().unwrap();

    let dest_dir = dir.path().join("extract_all");
    let mut progress_calls = 0usize;
    let extracted = reader::extract_all_from_volume(&volume_path, &dest_dir, |_| progress_calls += 1).unwrap();

    assert_eq!(extracted.len(), written.len());
    assert_eq!(progress_calls, written.len());
    for (name, data) in &written {
        let on_disk = std::fs::read(dest_dir.join(name)).unwrap();
        assert_eq!(&on_disk, data);
    }
}

#[test]
fn preview_entry_from_volume_truncates_to_max_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let volume_path = dir.path().join("2024-07.tar.zst");

    let mut writer = ChunkedArchiveWriter::open_or_append(&volume_path, 1).unwrap();
    let written = write_dummy_batch(&mut writer, "Xray", 0, 1, 10_000);
    writer.finish().unwrap();

    let (name, data) = &written[0];
    let (preview, truncated) = reader::preview_entry_from_volume(&volume_path, name, 100).unwrap();
    assert_eq!(preview.len(), 100);
    assert!(truncated);
    assert_eq!(&preview[..], &data[..100]);

    let (full, truncated) = reader::preview_entry_from_volume(&volume_path, name, 1_000_000).unwrap();
    assert!(!truncated);
    assert_eq!(&full, data);
}
