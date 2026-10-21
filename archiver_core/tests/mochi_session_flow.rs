//! Full session flow against a dummy source tree, including a simulated crash
//! between staging and archiving, and a crash after the archive commit but
//! before the index was written.

use archiver_core::db::ArchivalCandidate;
use archiver_core::mochi_store::{self, StoreFile};
use archiver_core::reader;
use archiver_core::repo;
use archiver_core::schema::open_and_migrate;
use archiver_core::session::{
    run_session, SessionConfig, StagedManifestEntry, StagingManifestWriter,
};
use std::fs;
use std::path::{Path, PathBuf};

type NoProgress = fn(String, usize, usize);

fn make_source_tree(root: &Path) {
    for sub in ["BURNIN", "ESS", "JTAG", "STM", "Xray"] {
        fs::create_dir_all(root.join(sub)).unwrap();
    }
    fs::write(root.join("BURNIN/unit_BC001_log.txt"), b"burnin data for bc001").unwrap();
    fs::write(root.join("ESS/unit_BC001_ess.txt"), b"ess data for bc001").unwrap();
    fs::write(root.join("BURNIN/unit_BC002_log.txt"), b"burnin data for bc002").unwrap();
    fs::write(root.join("Xray/unit_BC003_scan.zip"), b"xray zip data for bc003").unwrap();
    fs::write(root.join("STM/unit_UNRELATED_stm.txt"), b"nobody archives me").unwrap();
}

fn config(source_dir: PathBuf, output_dir: PathBuf, staging_dir: PathBuf) -> SessionConfig {
    SessionConfig {
        source_dir,
        output_dir,
        staging_dir,
        subfolders: vec!["BURNIN".into(), "ESS".into(), "JTAG".into(), "STM".into(), "Xray".into()],
        allowed_extensions: vec!["txt".into(), "zip".into()],
        lookback_days: 2,
        date_range_override: None,
    }
}

fn candidates() -> Vec<ArchivalCandidate> {
    vec![
        ArchivalCandidate { barcode: "BC001".into(), last_update: "2024-06-10T00:00:00Z".into() },
        ArchivalCandidate { barcode: "BC002".into(), last_update: "2024-06-11T00:00:00Z".into() },
        // A different month, to get a second archive.
        ArchivalCandidate { barcode: "BC003".into(), last_update: "2024-07-12T00:00:00Z".into() },
    ]
}

fn run(conn: &mut rusqlite::Connection, cfg: &SessionConfig, started: &str) -> archiver_core::session::SessionResult {
    run_session(conn, cfg, candidates(), "2024-06-01", "2024-07-29", started, None::<&NoProgress>).unwrap()
}

fn stage_like_run_session(cfg: &SessionConfig, session_id: i64) -> Vec<StagedManifestEntry> {
    let matcher = archiver_core::matcher::BarcodeMatcher::new(candidates().into_iter().map(|c| c.barcode).collect::<Vec<String>>());
    let mut entries = Vec::new();
    fs::create_dir_all(&cfg.staging_dir).unwrap();
    for sub in &cfg.subfolders {
        let subdir = cfg.source_dir.join(sub);
        for e in fs::read_dir(&subdir).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().to_string();
            let m = matcher.matching_barcodes(&name);
            if m.len() != 1 {
                continue;
            }
            let barcode = m.into_iter().next().unwrap();
            let volume = if barcode == "BC003" { "2024-07" } else { "2024-06" };
            entries.push(StagedManifestEntry {
                relative_path: format!("{sub}/{name}"),
                source_subfolder: sub.clone(),
                barcode,
                original_size: e.metadata().unwrap().len(),
                volume_name: volume.into(),
            });
        }
    }
    let mut w = StagingManifestWriter::create(&cfg.staging_dir, session_id).unwrap();
    for e in &entries {
        let dst = cfg.staging_dir.join(&e.relative_path);
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::rename(cfg.source_dir.join(&e.relative_path), dst).unwrap();
        w.record(e.clone()).unwrap();
    }
    w.finish().unwrap();
    entries
}

fn staging_is_gone(staging: &Path) -> bool {
    !staging.exists() || fs::read_dir(staging).unwrap().next().is_none()
}

#[test]
fn full_flow_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let (source, output, staging) = (dir.path().join("source"), dir.path().join("output"), dir.path().join("staging"));
    make_source_tree(&source);
    let mut conn = open_and_migrate(dir.path().join("index.sqlite")).unwrap();
    let cfg = config(source.clone(), output.clone(), staging.clone());

    let result = run(&mut conn, &cfg, "2024-08-01T00:00:00Z");
    assert_eq!(result.files_moved, 4);
    assert_eq!(result.barcodes_matched, 3);
    assert_eq!(result.errors, 0);
    assert_eq!(result.files_deleted, 4);
    assert!(staging_is_gone(&staging));
    assert!(source.join("STM/unit_UNRELATED_stm.txt").exists(), "unrelated file untouched");
    assert!(!source.join("BURNIN/unit_BC001_log.txt").exists(), "matched files are moved, not copied");

    // One archive per month, named from the barcode's date.
    assert!(output.join("2024/2024-06.mochi").is_file());
    assert!(output.join("2024/2024-07.mochi").is_file());
    assert!(!output.join("2024/2024-06.tar.zst").exists());
    let mut vols = result.volume_names.clone();
    vols.sort();
    assert_eq!(vols, vec!["2024-06", "2024-07"]);

    // Index rows.
    let locs = repo::find_entries_by_barcode(&conn, "BC001").unwrap();
    assert_eq!(locs.len(), 2);
    assert!(locs.iter().all(|l| l.content_hash.as_deref().is_some_and(|h| h.len() == 64)));
    assert!(locs.iter().all(|l| l.commit_seq == Some(0)));
    let (head_id, head_seq, count): (String, i64, i64) = conn
        .query_row(
            "SELECT head_commit_id, head_seq, entry_count FROM file_storage_volume WHERE volume_name = '2024-06'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((head_id.len(), head_seq, count), (64, 0, 3));

    // Extractable, byte-identical.
    let dest = dir.path().join("extract_check");
    let extracted = reader::extract_by_barcode(&conn, "BC001", &dest).unwrap();
    assert_eq!(extracted.len(), 2);
    assert_eq!(fs::read(dest.join("BURNIN/unit_BC001_log.txt")).unwrap(), b"burnin data for bc001");
    assert_eq!(fs::read(dest.join("ESS/unit_BC001_ess.txt")).unwrap(), b"ess data for bc001");
}

#[test]
fn a_second_session_appends_to_the_same_month_as_a_new_commit() {
    let dir = tempfile::tempdir().unwrap();
    let (source, output, staging) = (dir.path().join("source"), dir.path().join("output"), dir.path().join("staging"));
    make_source_tree(&source);
    let mut conn = open_and_migrate(dir.path().join("index.sqlite")).unwrap();
    let cfg = config(source.clone(), output.clone(), staging.clone());
    run(&mut conn, &cfg, "2024-08-01T00:00:00Z");

    // A late log file for BC001 shows up (the barcode itself is already scanned).
    fs::write(source.join("JTAG/unit_BC001_late.txt"), b"late arrival").unwrap();
    let r = run(&mut conn, &cfg, "2024-08-02T00:00:00Z");
    assert_eq!(r.files_moved, 1);

    let locs = repo::find_entries_by_barcode(&conn, "BC001").unwrap();
    assert_eq!(locs.len(), 3);
    let late = locs.iter().find(|l| l.relative_path.contains("late")).unwrap();
    assert_eq!(late.commit_seq, Some(1), "the second session is commit 1 of the month");
    let (head_seq, count): (i64, i64) = conn
        .query_row("SELECT head_seq, entry_count FROM file_storage_volume WHERE volume_name = '2024-06'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!((head_seq, count), (1, 4));
    assert_eq!(reader::list_volume_entries(&output.join("2024/2024-06.mochi"), |_| {}).unwrap().len(), 4);
}

#[test]
fn crash_after_staging_then_rerun_loses_nothing_and_duplicates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (source, output, staging) = (dir.path().join("source"), dir.path().join("output"), dir.path().join("staging"));
    make_source_tree(&source);
    let mut conn = open_and_migrate(dir.path().join("index.sqlite")).unwrap();
    let cfg = config(source.clone(), output.clone(), staging.clone());

    // "First run" dies after staging, before any archive exists.
    let sid = repo::start_session(&conn, "2024-08-01T00:00:00Z", "2024-06-01", "2024-07-29").unwrap();
    let staged = stage_like_run_session(&cfg, sid);
    assert_eq!(staged.len(), 4);
    assert!(!output.join("2024/2024-06.mochi").exists());

    // Second run resumes the batch first.
    run(&mut conn, &cfg, "2024-08-02T00:00:00Z");
    assert!(staging_is_gone(&staging));

    let total: usize = ["BC001", "BC002", "BC003"].iter().map(|b| repo::find_entries_by_barcode(&conn, b).unwrap().len()).sum();
    assert_eq!(total, 4, "exactly the original 4 entries");
    let june = reader::list_volume_entries(&output.join("2024/2024-06.mochi"), |_| {}).unwrap();
    assert_eq!(june.len(), 3);
    let dest = dir.path().join("after_resume");
    let ex = reader::extract_by_barcode(&conn, "BC001", &dest).unwrap();
    assert_eq!(ex.len(), 2);
    assert_eq!(fs::read(dest.join("ESS/unit_BC001_ess.txt")).unwrap(), b"ess data for bc001");
}

#[test]
fn crash_after_commit_but_before_index_is_recovered_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let (source, output, staging) = (dir.path().join("source"), dir.path().join("output"), dir.path().join("staging"));
    make_source_tree(&source);
    let mut conn = open_and_migrate(dir.path().join("index.sqlite")).unwrap();
    let cfg = config(source.clone(), output.clone(), staging.clone());

    let sid = repo::start_session(&conn, "2024-08-01T00:00:00Z", "2024-06-01", "2024-07-29").unwrap();
    let staged = stage_like_run_session(&cfg, sid);

    // The crash happens right after the June archive was committed (and even
    // verified), but before the index rows were written and staging deleted.
    let june: Vec<StoreFile> = staged
        .iter()
        .filter(|e| e.volume_name == "2024-06")
        .map(|e| StoreFile { relative_path: e.relative_path.clone(), staged_path: staging.join(&e.relative_path) })
        .collect();
    let june_path = mochi_store::volume_path(&output, "2024-06");
    mochi_store::store_volume(&june_path, &june).unwrap();
    assert_eq!(repo::find_entries_by_barcode(&conn, "BC001").unwrap().len(), 0);

    run(&mut conn, &cfg, "2024-08-02T00:00:00Z");
    assert!(staging_is_gone(&staging));

    // Namespace holds each file once; the index holds each file once.
    assert_eq!(reader::list_volume_entries(&june_path, |_| {}).unwrap().len(), 3);
    assert_eq!(repo::find_entries_by_barcode(&conn, "BC001").unwrap().len(), 2);
    assert_eq!(repo::find_entries_by_barcode(&conn, "BC002").unwrap().len(), 1);
    let count: i64 = conn
        .query_row("SELECT entry_count FROM file_storage_volume WHERE volume_name='2024-06'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn a_failed_verification_keeps_staging_and_the_session_open() {
    let dir = tempfile::tempdir().unwrap();
    let (source, output, staging) = (dir.path().join("source"), dir.path().join("output"), dir.path().join("staging"));
    make_source_tree(&source);
    let mut conn = open_and_migrate(dir.path().join("index.sqlite")).unwrap();
    let cfg = config(source.clone(), output.clone(), staging.clone());

    let sid = repo::start_session(&conn, "t", "2024-06-01", "2024-07-29").unwrap();
    stage_like_run_session(&cfg, sid);

    // The June archive exists but is unreadable garbage -> must abort, not delete.
    let june_path = mochi_store::volume_path(&output, "2024-06");
    fs::create_dir_all(june_path.parent().unwrap()).unwrap();
    fs::write(&june_path, b"this is not a mochi archive at all").unwrap();

    let err = run_session(&mut conn, &cfg, candidates(), "2024-06-01", "2024-07-29", "2024-08-02T00:00:00Z", None::<&NoProgress>)
        .unwrap_err();
    eprintln!("expected failure: {err}");
    assert!(staging.join("BURNIN/unit_BC001_log.txt").exists(), "staging must be kept");
    assert!(staging.join("manifest.jsonl").exists());
    assert_eq!(fs::read(&june_path).unwrap(), b"this is not a mochi archive at all", "garbage file is not modified");
}

#[test]
fn cleanup_never_deletes_unlisted_files_left_in_staging() {
    let dir = tempfile::tempdir().unwrap();
    let (source, output, staging) = (dir.path().join("source"), dir.path().join("output"), dir.path().join("staging"));
    make_source_tree(&source);
    let mut conn = open_and_migrate(dir.path().join("index.sqlite")).unwrap();
    let cfg = config(source.clone(), output.clone(), staging.clone());

    let sid = repo::start_session(&conn, "2024-08-01T00:00:00Z", "2024-06-01", "2024-07-29").unwrap();
    stage_like_run_session(&cfg, sid);
    // A file that was renamed into staging by a crashed run but never made it into the manifest.
    let orphan = staging.join("JTAG/unit_BC002_orphan.txt");
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::write(&orphan, b"renamed but unlisted").unwrap();

    run(&mut conn, &cfg, "2024-08-02T00:00:00Z");

    assert_eq!(fs::read(&orphan).unwrap(), b"renamed but unlisted", "unlisted file must survive");
    assert!(!staging.join("manifest.jsonl").exists());
    assert!(!staging.join("BURNIN/unit_BC001_log.txt").exists(), "listed, verified files are removed");
    assert!(!staging.join("BURNIN").exists(), "now-empty directories are pruned");
    // Counted as an error on the resumed session.
    let errs: i64 = conn.query_row("SELECT total_errors FROM scan_session WHERE id = ?1", [sid], |r| r.get(0)).unwrap();
    assert_eq!(errs, 1);
    // The listed files were archived.
    assert_eq!(repo::find_entries_by_barcode(&conn, "BC001").unwrap().len(), 2);
}

#[test]
fn resume_with_a_listed_file_that_was_never_renamed_loses_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (source, output, staging) = (dir.path().join("source"), dir.path().join("output"), dir.path().join("staging"));
    make_source_tree(&source);
    let mut conn = open_and_migrate(dir.path().join("index.sqlite")).unwrap();
    let cfg = config(source.clone(), output.clone(), staging.clone());

    // Manifest line written (flushed) before the rename, then the crash: file still in source.
    let sid = repo::start_session(&conn, "t", "2024-06-01", "2024-07-29").unwrap();
    fs::create_dir_all(&staging).unwrap();
    let mut w = StagingManifestWriter::create(&staging, sid).unwrap();
    w.record(StagedManifestEntry {
        relative_path: "BURNIN/unit_BC001_log.txt".into(),
        source_subfolder: "BURNIN".into(),
        barcode: "BC001".into(),
        original_size: 21,
        volume_name: "2024-06".into(),
    })
    .unwrap();
    w.finish().unwrap();

    run(&mut conn, &cfg, "2024-08-02T00:00:00Z");
    // The never-renamed file was neither lost nor deleted by the cleanup; the
    // fresh scan in the same run picked it up and archived it.
    assert_eq!(repo::find_entries_by_barcode(&conn, "BC001").unwrap().len(), 2);
    assert!(!source.join("BURNIN/unit_BC001_log.txt").exists());
}

#[test]
fn invalid_subfolders_are_rejected_at_session_start() {
    for bad in ["", ".", "..", "../x", "a/b", "a\\b", "C:", "C:evil", "/abs", " sp "] {
        assert!(archiver_core::session::validate_subfolder(bad).is_err(), "{bad:?} must be rejected");
    }
    for ok in ["BURNIN", "Xray", "my folder", "a.b"] {
        archiver_core::session::validate_subfolder(ok).unwrap();
    }

    let dir = tempfile::tempdir().unwrap();
    let (source, output, staging) = (dir.path().join("source"), dir.path().join("output"), dir.path().join("staging"));
    make_source_tree(&source);
    let mut conn = open_and_migrate(dir.path().join("index.sqlite")).unwrap();
    let mut cfg = config(source.clone(), output, staging);
    cfg.subfolders = vec!["BURNIN".into(), "../BURNIN".into()];
    let err = run_session(&mut conn, &cfg, candidates(), "a", "b", "t", None::<&NoProgress>).unwrap_err();
    assert!(matches!(err, archiver_core::ArchiveError::Config(_)), "{err}");
    assert!(source.join("BURNIN/unit_BC001_log.txt").exists(), "nothing was touched");
}
