//! Extraction through the SQLite index (by barcode / by filename) and
//! path-safety of everything that writes to disk.

use archiver_core::mochi_store::{self, StoreFile};
use archiver_core::reader::{self, safe_join};
use archiver_core::repo;
use archiver_core::schema::open_in_memory;
use archiver_core::ArchiveError;
use std::fs;
use std::path::Path;

fn stage(staging: &Path, files: &[(&str, &[u8])]) -> Vec<StoreFile> {
    files
        .iter()
        .map(|(rel, data)| {
            let p = staging.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, data).unwrap();
            StoreFile { relative_path: rel.to_string(), staged_path: p }
        })
        .collect()
}

fn barcode_of(p: &str) -> Option<String> {
    p.split('_').find(|s| s.starts_with("BC")).map(String::from)
}

fn indexed_volume(dir: &Path) -> (rusqlite::Connection, std::path::PathBuf) {
    let archive = mochi_store::volume_path(&dir.join("out"), "2024-07");
    let store = stage(
        &dir.join("staging"),
        &[
            ("BURNIN/unit_BC001_log.txt", b"burnin one"),
            ("ESS/unit_BC001_ess.txt", b"ess one"),
            ("BURNIN/unit_BC002_log.txt", b"burnin two"),
        ],
    );
    let finished = mochi_store::store_volume(&archive, &store).unwrap();
    let mut conn = open_in_memory().unwrap();
    let sid = repo::start_session(&conn, "t", "a", "b").unwrap();
    repo::record_finished_volume(&mut conn, sid, "2024-07", &finished, "t", barcode_of, |p| {
        p.split('/').next().unwrap().to_string()
    })
    .unwrap();
    (conn, archive)
}

#[test]
fn extract_by_barcode_uses_the_index_and_restores_exact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _archive) = indexed_volume(dir.path());
    let dest = dir.path().join("dest");
    let out = reader::extract_by_barcode(&conn, "BC001", &dest).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(fs::read(dest.join("BURNIN/unit_BC001_log.txt")).unwrap(), b"burnin one");
    assert_eq!(fs::read(dest.join("ESS/unit_BC001_ess.txt")).unwrap(), b"ess one");
    assert!(!dest.join("BURNIN/unit_BC002_log.txt").exists());
    assert!(out.iter().all(|e| e.bytes_written > 0 && e.dest_path.starts_with(&dest)));
    assert!(reader::extract_by_barcode(&conn, "NOPE", &dest).unwrap().is_empty());
}

#[test]
fn extract_locations_from_filename_search() {
    let dir = tempfile::tempdir().unwrap();
    let (conn, _) = indexed_volume(dir.path());
    let locs = repo::find_entries_by_filename_like(&conn, "BC002").unwrap();
    assert_eq!(locs.len(), 1);
    let dest = dir.path().join("dest");
    let out = reader::extract_locations(&locs, &dest).unwrap();
    assert_eq!(fs::read(&out[0].dest_path).unwrap(), b"burnin two");
}

#[test]
fn extracting_a_missing_entry_reports_entry_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (_, archive) = indexed_volume(dir.path());
    let err = reader::extract_entry_from_volume(&archive, "BURNIN/absent.txt", &dir.path().join("x")).unwrap_err();
    assert!(matches!(err, ArchiveError::EntryNotFound(_)), "{err}");
    assert!(!dir.path().join("x").exists());
}

#[test]
fn safe_join_rejects_traversal_and_absolute_paths() {
    let root = Path::new("/tmp/dest");
    assert_eq!(safe_join(root, "BURNIN/a.txt").unwrap(), root.join("BURNIN").join("a.txt"));
    for bad in ["../evil.txt", "BURNIN/../../evil", "/etc/passwd", "", "a//b", "./a", "BURNIN/.."] {
        assert!(matches!(safe_join(root, bad), Err(ArchiveError::UnsafePath(_))), "{bad:?} must be rejected");
    }
}

#[test]
fn index_rows_with_hostile_paths_cannot_escape_the_destination() {
    let dir = tempfile::tempdir().unwrap();
    let (_, archive) = indexed_volume(dir.path());
    let evil = repo::LogEntryLocation {
        relative_path: "../escaped.txt".into(),
        volume_path: archive.to_string_lossy().into_owned(),
        content_hash: None,
        commit_seq: None,
    };
    let dest = dir.path().join("dest");
    assert!(reader::extract_locations(&[evil], &dest).is_err());
    assert!(!dir.path().join("escaped.txt").exists());
}
