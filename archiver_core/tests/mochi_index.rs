//! SQLite index tests for the MOCHI schema: round trips, uniqueness,
//! per-volume head tracking, and legacy-schema detection.

use archiver_core::mochi_store::{FinishedVolume, StoredEntry};
use archiver_core::repo::{self, BarcodeStatus, LogEntryInsert, SessionStatus};
use archiver_core::schema::{open_and_migrate, open_in_memory, SCHEMA_VERSION};
use archiver_core::ArchiveError;

fn finished(path: &str, size: u64, head: &str, seq: u64, entries: Vec<StoredEntry>) -> FinishedVolume {
    FinishedVolume {
        path: path.into(),
        file_size: size,
        entry_count: entries.len(),
        head_commit_id: head.into(),
        head_seq: seq,
        written_entries: entries,
        commit_seqs: vec![seq],
        created: false,
        degraded_durability: None,
    }
}

fn entry(rel: &str, size: u64, hash: &str, seq: u64) -> StoredEntry {
    StoredEntry {
        relative_path: rel.into(),
        original_size: size,
        content_hash: hash.into(),
        commit_seq: seq,
    }
}

fn columns(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1)).unwrap().map(|r| r.unwrap()).collect()
}

#[test]
fn schema_creates_all_four_tables_with_mochi_columns() {
    let conn = open_in_memory().unwrap();
    let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").unwrap();
    let names: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    for expected in ["scan_session", "session_barcode", "file_storage_volume", "scanned_log_entry"] {
        assert!(names.iter().any(|n| n == expected), "missing table {expected}, got {names:?}");
    }

    let vol = columns(&conn, "file_storage_volume");
    for c in ["path", "volume_name", "file_size", "entry_count", "created_at", "updated_at", "head_commit_id", "head_seq"] {
        assert!(vol.iter().any(|x| x == c), "file_storage_volume lacks {c}: {vol:?}");
    }
    assert!(!vol.iter().any(|x| x == "sha256"));

    let sle = columns(&conn, "scanned_log_entry");
    for c in ["content_hash", "commit_seq", "relative_path", "original_size"] {
        assert!(sle.iter().any(|x| x == c), "scanned_log_entry lacks {c}: {sle:?}");
    }
    assert!(!sle.iter().any(|x| x == "chunk_start_byte" || x == "chunk_end_byte"));

    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, SCHEMA_VERSION);
}

#[test]
fn reopening_a_current_database_is_fine_and_keeps_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("index.sqlite");
    {
        let conn = open_and_migrate(&path).unwrap();
        repo::start_session(&conn, "t", "2024-06-01", "2024-06-29").unwrap();
    }
    let conn = open_and_migrate(&path).unwrap();
    assert!(repo::session_exists(&conn, 1).unwrap());
}

#[test]
fn legacy_tar_zst_database_is_detected_and_not_touched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.sqlite");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE file_storage_volume (id INTEGER PRIMARY KEY, path TEXT NOT NULL, volume_name TEXT NOT NULL,
                file_size INTEGER NOT NULL, sha256 TEXT NOT NULL, entry_count INTEGER NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
             INSERT INTO file_storage_volume VALUES (1,'/x/2024-07.tar.zst','2024-07',1,'abc',1,'t','t');",
        )
        .unwrap();
    }
    let err = open_and_migrate(&path).unwrap_err();
    assert!(matches!(err, ArchiveError::SchemaMismatch { found: 0, .. }), "{err}");
    assert!(err.to_string().contains("tar.zst"));

    // Left exactly as it was.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM file_storage_volume", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 0);
}

#[test]
fn newer_schema_version_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.sqlite");
    rusqlite::Connection::open(&path).unwrap().execute_batch("PRAGMA user_version = 99").unwrap();
    assert!(matches!(open_and_migrate(&path).unwrap_err(), ArchiveError::SchemaMismatch { found: 99, .. }));
}

#[test]
fn scan_session_insert_query_round_trip() {
    let conn = open_in_memory().unwrap();
    let id = repo::start_session(&conn, "2024-07-01T00:00:00Z", "2024-06-01", "2024-06-29").unwrap();
    assert!(id > 0);
    repo::complete_session(&conn, id, "2024-07-01T00:05:00Z", SessionStatus::Success, 10, 42, 0).unwrap();
    assert_eq!(repo::last_successful_scan_to_date(&conn).unwrap().as_deref(), Some("2024-06-29"));
}

#[test]
fn session_barcode_unique_constraint_rejects_duplicate() {
    let conn = open_in_memory().unwrap();
    let s1 = repo::start_session(&conn, "t", "2024-06-01", "2024-06-29").unwrap();
    let first = repo::record_session_barcode(&conn, s1, "BC001", "2024-06-15T10:00:00Z", BarcodeStatus::Matched).unwrap();
    assert!(first.is_some());
    let s2 = repo::start_session(&conn, "t2", "2024-06-01", "2024-06-30").unwrap();
    assert!(repo::record_session_barcode(&conn, s2, "BC001", "2024-06-15T10:00:00Z", BarcodeStatus::Matched).unwrap().is_none());
    assert!(repo::record_session_barcode(&conn, s2, "BC001", "2024-06-16T10:00:00Z", BarcodeStatus::Matched).unwrap().is_some());
    assert!(repo::is_barcode_already_scanned(&conn, "BC001", "2024-06-15T10:00:00Z").unwrap());
    assert!(!repo::is_barcode_already_scanned(&conn, "BC001", "2024-06-17T10:00:00Z").unwrap());
}

#[test]
fn scanned_log_entry_unique_constraint_is_enforced_in_the_schema() {
    let conn = open_in_memory().unwrap();
    let session_id = repo::start_session(&conn, "t", "2024-06-01", "2024-06-29").unwrap();
    let f = finished("/tmp/2024-07.mochi", 100, "aa", 0, vec![]);
    let volume_id = repo::upsert_volume(&conn, "2024-07", &f, 1, "2024-07-01T00:00:00Z").unwrap();
    let insert = |c: &rusqlite::Connection| {
        c.execute(
            "INSERT INTO scanned_log_entry (scan_session_id, barcode, relative_path, source_subfolder,
                content_hash, commit_seq, original_size, file_storage_volume_id)
             VALUES (?1,'BC001','BURNIN/x.txt','BURNIN','h',0,5,?2)",
            rusqlite::params![session_id, volume_id],
        )
    };
    insert(&conn).unwrap();
    assert!(insert(&conn).is_err(), "UNIQUE(relative_path, file_storage_volume_id) must reject a raw duplicate");
}

#[test]
fn insert_log_entry_is_idempotent_for_resume() {
    let conn = open_in_memory().unwrap();
    let session_id = repo::start_session(&conn, "t", "2024-06-01", "2024-06-29").unwrap();
    let f = finished("/tmp/2024-07.mochi", 100, "aa", 0, vec![]);
    let volume_id = repo::upsert_volume(&conn, "2024-07", &f, 1, "t").unwrap();
    let mk = |hash: &'static str, seq: u64| LogEntryInsert {
        scan_session_id: session_id,
        barcode: "BC001",
        relative_path: "BURNIN/x.txt",
        source_subfolder: "BURNIN",
        content_hash: hash,
        commit_seq: seq,
        original_size: 50,
        file_storage_volume_id: volume_id,
    };
    let a = repo::insert_log_entry(&conn, &mk("h1", 0)).unwrap();
    let b = repo::insert_log_entry(&conn, &mk("h2", 3)).unwrap();
    assert_eq!(a, b);
    let (n, hash, seq): (i64, String, i64) = conn
        .query_row("SELECT COUNT(*), content_hash, commit_seq FROM scanned_log_entry", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!((n, hash.as_str(), seq), (1, "h2", 3), "re-recording updates the single row to the latest commit");
}

#[test]
fn upsert_volume_tracks_head_on_append() {
    let conn = open_in_memory().unwrap();
    let id1 = repo::upsert_volume(&conn, "2024-07", &finished("/o/2024-07.mochi", 1000, "head0", 0, vec![]), 3, "2024-07-01T00:00:00Z").unwrap();
    let prior = repo::existing_entry_count(&conn, id1).unwrap();
    let id2 = repo::upsert_volume(&conn, "2024-07", &finished("/o/2024-07.mochi", 1800, "head1", 1, vec![]), prior + 2, "2024-07-02T00:00:00Z").unwrap();
    assert_eq!(id1, id2);
    let (size, head, seq, count, created, updated): (i64, String, i64, i64, String, String) = conn
        .query_row(
            "SELECT file_size, head_commit_id, head_seq, entry_count, created_at, updated_at FROM file_storage_volume WHERE id = ?1",
            [id1],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!((size, head.as_str(), seq, count), (1800, "head1", 1, 5));
    assert_eq!(created, "2024-07-01T00:00:00Z");
    assert_eq!(updated, "2024-07-02T00:00:00Z");
}

#[test]
fn record_finished_volume_end_to_end_across_two_appends() {
    let mut conn = open_in_memory().unwrap();
    let session_id = repo::start_session(&conn, "t", "2024-06-01", "2024-06-29").unwrap();
    let barcode_of = |p: &str| p.split('_').nth(1).map(|s| s.to_string());
    let sub_of = |p: &str| p.split('/').next().unwrap_or("").to_string();

    let f1 = finished(
        "/o/2024/2024-07.mochi",
        5000,
        "c0",
        0,
        vec![entry("BURNIN/unit1_BC001_log.txt", 2000, "h1", 0), entry("ESS/unit1_BC001_ess.txt", 1000, "h2", 0)],
    );
    let volume_id = repo::record_finished_volume(&mut conn, session_id, "2024-07", &f1, "t1", barcode_of, sub_of).unwrap();

    let f2 = finished("/o/2024/2024-07.mochi", 6000, "c1", 1, vec![entry("JTAG/unit2_BC002_j.txt", 10, "h3", 1)]);
    let v2 = repo::record_finished_volume(&mut conn, session_id, "2024-07", &f2, "t2", barcode_of, sub_of).unwrap();
    assert_eq!(volume_id, v2);

    let locs = repo::find_entries_by_barcode(&conn, "BC001").unwrap();
    assert_eq!(locs.len(), 2);
    assert!(locs.iter().all(|l| l.volume_path == "/o/2024/2024-07.mochi"));
    let by_hash: Vec<_> = locs.iter().filter_map(|l| l.content_hash.clone()).collect();
    assert!(by_hash.contains(&"h1".to_string()) && by_hash.contains(&"h2".to_string()));
    assert!(locs.iter().all(|l| l.commit_seq == Some(0)));
    assert_eq!(repo::find_entries_by_barcode(&conn, "BC002").unwrap()[0].commit_seq, Some(1));
    assert_eq!(repo::find_entries_by_filename_like(&conn, "unit1_BC001").unwrap().len(), 2);

    let (count, head, seq): (i64, String, i64) = conn
        .query_row("SELECT entry_count, head_commit_id, head_seq FROM file_storage_volume WHERE id = ?1", [volume_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!((count, head.as_str(), seq), (3, "c1", 1));

    // Recording the same batch again (resume after a crash before staging cleanup) is harmless.
    repo::record_finished_volume(&mut conn, session_id, "2024-07", &f2, "t3", barcode_of, sub_of).unwrap();
    let count: i64 = conn.query_row("SELECT entry_count FROM file_storage_volume", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 3);
}
