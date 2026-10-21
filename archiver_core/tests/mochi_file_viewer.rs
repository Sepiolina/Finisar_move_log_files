//! File-viewer support: list / extract / preview straight from a `.mochi`
//! volume (no SQLite index involved).

use archiver_core::mochi_store::{self, StoreFile};
use archiver_core::reader;
use std::fs;

fn dummy(i: usize, size: usize) -> Vec<u8> {
    (0..size).map(|j| ((i * 7 + j) % 251) as u8).collect()
}

/// 20 files x 5 MiB spread over the volume (100 MiB, many MOCHI chunks).
fn big_volume(dir: &std::path::Path, subfolder: &str) -> (std::path::PathBuf, Vec<(String, Vec<u8>)>) {
    let archive = dir.join("2024-07.mochi");
    let mut files = Vec::new();
    let mut store = Vec::new();
    for i in 0..20 {
        let rel = format!("{subfolder}/dummy_{i:04}.txt");
        let data = dummy(i, 5 * 1024 * 1024);
        let p = dir.join("staging").join(&rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, &data).unwrap();
        store.push(StoreFile { relative_path: rel.clone(), staged_path: p });
        files.push((rel, data));
    }
    mochi_store::store_volume(&archive, &store).unwrap();
    (archive, files)
}

#[test]
fn list_volume_entries_sees_every_file() {
    let dir = tempfile::tempdir().unwrap();
    let (archive, written) = big_volume(dir.path(), "JTAG");
    let mut progress_calls = 0usize;
    let entries = reader::list_volume_entries(&archive, |_| progress_calls += 1).unwrap();
    assert_eq!(entries.len(), written.len(), "directories must not be listed as files");
    assert!(progress_calls >= 1);
    for (name, data) in &written {
        let e = entries.iter().find(|e| &e.relative_path == name).unwrap();
        assert_eq!(e.size, data.len() as u64);
    }
}

#[test]
fn extract_entry_from_volume_returns_exact_bytes_of_a_late_file() {
    let dir = tempfile::tempdir().unwrap();
    let (archive, written) = big_volume(dir.path(), "STM");
    let (name, data) = written.last().unwrap();
    let dest = dir.path().join("out").join(name);
    let n = reader::extract_entry_from_volume(&archive, name, &dest).unwrap();
    assert_eq!(n, data.len() as u64);
    assert_eq!(&fs::read(&dest).unwrap(), data);
    // No leftover temp file.
    let leftovers: Vec<_> = fs::read_dir(dest.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");
}

#[test]
fn extract_all_from_volume_recovers_every_file_and_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let (archive, written) = big_volume(dir.path(), "ESS");
    let dest_dir = dir.path().join("extract_all");
    let mut progress_calls = 0usize;
    let extracted = reader::extract_all_from_volume(&archive, &dest_dir, |_| progress_calls += 1).unwrap();
    assert_eq!(extracted.len(), written.len());
    assert_eq!(progress_calls, written.len());
    for (name, data) in &written {
        assert_eq!(&fs::read(dest_dir.join(name)).unwrap(), data);
    }
    // Running it again over the same folder succeeds (overwrites).
    fs::write(dest_dir.join(&written[0].0), b"stale").unwrap();
    reader::extract_all_from_volume(&archive, &dest_dir, |_| {}).unwrap();
    assert_eq!(fs::read(dest_dir.join(&written[0].0)).unwrap(), written[0].1);
}

#[test]
fn preview_entry_from_volume_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("v.mochi");
    let data = dummy(3, 10_000);
    let p = dir.path().join("s/Xray/one.txt");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, &data).unwrap();
    mochi_store::store_volume(&archive, &[StoreFile { relative_path: "Xray/one.txt".into(), staged_path: p }]).unwrap();

    let (preview, truncated) = reader::preview_entry_from_volume(&archive, "Xray/one.txt", 100).unwrap();
    assert_eq!(preview.len(), 100);
    assert!(truncated);
    assert_eq!(&preview[..], &data[..100]);

    let (full, truncated) = reader::preview_entry_from_volume(&archive, "Xray/one.txt", 1_000_000).unwrap();
    assert!(!truncated);
    assert_eq!(full, data);

    // Exactly at the limit is not truncated.
    let (exact, truncated) = reader::preview_entry_from_volume(&archive, "Xray/one.txt", 10_000).unwrap();
    assert!(!truncated);
    assert_eq!(exact.len(), 10_000);
}

#[test]
fn preview_of_a_huge_file_reads_only_what_it_needs() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("v.mochi");
    let data = dummy(5, 20 * 1024 * 1024);
    let p = dir.path().join("s/Xray/huge.bin");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, &data).unwrap();
    mochi_store::store_volume(&archive, &[StoreFile { relative_path: "Xray/huge.bin".into(), staged_path: p }]).unwrap();
    let (preview, truncated) = reader::preview_entry_from_volume(&archive, "Xray/huge.bin", 64 * 1024).unwrap();
    assert_eq!(preview.len(), 64 * 1024);
    assert!(truncated);
    assert_eq!(&preview[..], &data[..64 * 1024]);
}
