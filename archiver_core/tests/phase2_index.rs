//! Phase 2 test criteria, verbatim from spec:
//! - insert/query round-trip
//! - verify the unique constraint actually rejects a duplicate
//!   (barcode, last_update) insert

use archiver_core::repo::{self, BarcodeStatus, LogEntryInsert, SessionStatus};
use archiver_core::schema::open_in_memory;
use archiver_core::writer::{ChunkSpan, FinishedVolume, WrittenEntry};

#[test]
fn schema_creates_all_four_tables() {
    let conn = open_in_memory().unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    for expected in [
        "scan_session",
        "session_barcode",
        "file_storage_volume",
        "scanned_log_entry",
    ] {
        assert!(names.iter().any(|n| n == expected), "missing table {expected}, got {names:?}");
    }
}

#[test]
fn scan_session_insert_query_round_trip() {
    let conn = open_in_memory().unwrap();
    let id = repo::start_session(&conn, "2024-07-01T00:00:00Z", "2024-06-01", "2024-06-29").unwrap();
    assert!(id > 0);

    repo::complete_session(&conn, id, "2024-07-01T00:05:00Z", SessionStatus::Success, 10, 42, 0).unwrap();

    let to_date = repo::last_successful_scan_to_date(&conn).unwrap();
    assert_eq!(to_date.as_deref(), Some("2024-06-29"));
}

#[test]
fn session_barcode_unique_constraint_rejects_duplicate() {
    let conn = open_in_memory().unwrap();
    let session_id = repo::start_session(&conn, "2024-07-01T00:00:00Z", "2024-06-01", "2024-06-29").unwrap();

    let first = repo::record_session_barcode(&conn, session_id, "BC001", "2024-06-15T10:00:00Z", BarcodeStatus::Matched)
        .unwrap();
    assert!(first.is_some(), "first insert of a unique (barcode, last_update) pair should succeed");

    // Same barcode + same last_update, even from a different session id,
    // must be rejected by the UNIQUE(barcode, last_update) constraint —
    // this is the mid-session-crash-safe retry guarantee from the spec.
    let session_id_2 = repo::start_session(&conn, "2024-07-02T00:00:00Z", "2024-06-01", "2024-06-30").unwrap();
    let duplicate = repo::record_session_barcode(
        &conn,
        session_id_2,
        "BC001",
        "2024-06-15T10:00:00Z",
        BarcodeStatus::Matched,
    )
    .unwrap();
    assert!(duplicate.is_none(), "duplicate (barcode, last_update) must be rejected, not inserted");

    // A different last_update for the same barcode is a distinct row and
    // must be allowed.
    let different_update = repo::record_session_barcode(
        &conn,
        session_id_2,
        "BC001",
        "2024-06-16T10:00:00Z",
        BarcodeStatus::Matched,
    )
    .unwrap();
    assert!(different_update.is_some());

    assert!(repo::is_barcode_already_scanned(&conn, "BC001", "2024-06-15T10:00:00Z").unwrap());
    assert!(!repo::is_barcode_already_scanned(&conn, "BC001", "2024-06-17T10:00:00Z").unwrap());
}

#[test]
fn scanned_log_entry_unique_constraint_rejects_duplicate_path_in_same_volume() {
    let conn = open_in_memory().unwrap();
    let session_id = repo::start_session(&conn, "t", "2024-06-01", "2024-06-29").unwrap();

    let fake_finished = FinishedVolume {
        path: "/tmp/2024-07.tar.zst".into(),
        file_size: 100,
        sha256: "abc123".into(),
        entry_count: 1,
        chunk_spans: vec![ChunkSpan { chunk_start_byte: 0, chunk_end_byte: 100 }],
        written_entries: vec![],
    };
    let volume_id = repo::upsert_volume(&conn, "2024-07", &fake_finished, 1, "2024-07-01T00:00:00Z").unwrap();

    let entry = LogEntryInsert {
        scan_session_id: session_id,
        barcode: "BC001",
        relative_path: "BURNIN/unit1_BC001_log.txt",
        source_subfolder: "BURNIN",
        chunk_start_byte: 0,
        chunk_end_byte: 100,
        original_size: 50,
        file_storage_volume_id: volume_id,
    };
    repo::insert_log_entry(&conn, &entry).unwrap();

    // Same relative_path + same volume again must violate
    // UNIQUE(relative_path, file_storage_volume_id).
    let result = repo::insert_log_entry(&conn, &entry);
    assert!(result.is_err(), "duplicate (relative_path, file_storage_volume_id) should be rejected by the DB");
}

#[test]
fn upsert_volume_recomputes_metadata_on_append() {
    let conn = open_in_memory().unwrap();

    let first = FinishedVolume {
        path: "/tmp/2024-07.tar.zst".into(),
        file_size: 1000,
        sha256: "hash_after_first_write".into(),
        entry_count: 3,
        chunk_spans: vec![],
        written_entries: vec![],
    };
    let id1 = repo::upsert_volume(&conn, "2024-07", &first, 3, "2024-07-01T00:00:00Z").unwrap();

    // Simulate an append: file grew, hash changed, cumulative entry count
    // increased (3 existing + 2 new = 5), updated_at bumped.
    let second = FinishedVolume {
        path: "/tmp/2024-07.tar.zst".into(),
        file_size: 1800,
        sha256: "hash_after_append".into(),
        entry_count: 2,
        chunk_spans: vec![],
        written_entries: vec![],
    };
    let prior = repo::existing_entry_count(&conn, id1).unwrap();
    let id2 = repo::upsert_volume(&conn, "2024-07", &second, prior + 2, "2024-07-02T00:00:00Z").unwrap();

    assert_eq!(id1, id2, "appending to the same volume_name should update the same row, not insert a new one");

    let conn_check = &conn;
    let (file_size, sha256, entry_count, updated_at): (i64, String, i64, String) = conn_check
        .query_row(
            "SELECT file_size, sha256, entry_count, updated_at FROM file_storage_volume WHERE id = ?1",
            [id1],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();

    assert_eq!(file_size, 1800);
    assert_eq!(sha256, "hash_after_append");
    assert_eq!(entry_count, 5);
    assert_eq!(updated_at, "2024-07-02T00:00:00Z");
}

#[test]
fn record_finished_volume_end_to_end() {
    let mut conn = open_in_memory().unwrap();
    let session_id = repo::start_session(&conn, "t", "2024-06-01", "2024-06-29").unwrap();

    let finished = FinishedVolume {
        path: "/tmp/2024-07.tar.zst".into(),
        file_size: 5000,
        sha256: "deadbeef".into(),
        entry_count: 2,
        chunk_spans: vec![ChunkSpan { chunk_start_byte: 0, chunk_end_byte: 5000 }],
        written_entries: vec![
            WrittenEntry {
                relative_path: "BURNIN/unit1_BC001_log.txt".into(),
                original_size: 2000,
                chunk_start_byte: 0,
                chunk_end_byte: 5000,
            },
            WrittenEntry {
                relative_path: "ESS/unit1_BC001_ess.txt".into(),
                original_size: 1000,
                chunk_start_byte: 0,
                chunk_end_byte: 5000,
            },
        ],
    };

    let volume_id = repo::record_finished_volume(
        &mut conn,
        session_id,
        "2024-07",
        &finished,
        "2024-07-01T00:00:00Z",
        |path| {
            // crude extraction of the barcode segment for the test
            path.split('_').nth(1).map(|s| s.to_string())
        },
        |path| path.split('/').next().unwrap_or("").to_string(),
    )
    .unwrap();

    let locations = repo::find_entries_by_barcode(&conn, "BC001").unwrap();
    assert_eq!(locations.len(), 2);
    assert!(locations.iter().any(|l| l.relative_path == "BURNIN/unit1_BC001_log.txt"));
    assert!(locations.iter().any(|l| l.relative_path == "ESS/unit1_BC001_ess.txt"));
    assert!(locations.iter().all(|l| l.volume_path == "/tmp/2024-07.tar.zst"));

    let by_name = repo::find_entries_by_filename_like(&conn, "unit1_BC001").unwrap();
    assert_eq!(by_name.len(), 2);

    let (entry_count,): (i64,) = conn
        .query_row("SELECT entry_count FROM file_storage_volume WHERE id = ?1", [volume_id], |r| Ok((r.get(0)?,)))
        .unwrap();
    assert_eq!(entry_count, 2);
}
