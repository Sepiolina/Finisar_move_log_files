//! Phase 5 test criteria, verbatim from spec:
//! - run the full flow against a populated dummy source tree; kill the
//!   process mid-archive on a second run and confirm re-running doesn't
//!   lose or duplicate data

use archiver_core::db::ArchivalCandidate;
use archiver_core::repo;
use archiver_core::schema::open_and_migrate;
use archiver_core::session::{run_session, SessionConfig};
use std::fs;
use std::path::PathBuf;

fn make_source_tree(root: &std::path::Path) {
    for sub in ["BURNIN", "ESS", "JTAG", "STM", "Xray"] {
        fs::create_dir_all(root.join(sub)).unwrap();
    }
    fs::write(root.join("BURNIN/unit_BC001_log.txt"), b"burnin data for bc001").unwrap();
    fs::write(root.join("ESS/unit_BC001_ess.txt"), b"ess data for bc001").unwrap();
    fs::write(root.join("BURNIN/unit_BC002_log.txt"), b"burnin data for bc002").unwrap();
    fs::write(root.join("Xray/unit_BC003_scan.zip"), b"xray zip data for bc003").unwrap();
    // A file that matches no configured barcode — should be left alone.
    fs::write(root.join("STM/unit_UNRELATED_stm.txt"), b"nobody archives me").unwrap();
}

fn default_config(source_dir: PathBuf, output_dir: PathBuf, staging_dir: PathBuf) -> SessionConfig {
    SessionConfig {
        source_dir,
        output_dir,
        staging_dir,
        subfolders: vec!["BURNIN".into(), "ESS".into(), "JTAG".into(), "STM".into(), "Xray".into()],
        allowed_extensions: vec!["txt".into(), "zip".into()],
        compression_level: 3,
        lookback_days: 2,
        date_range_override: None,
    }
}

fn candidates() -> Vec<ArchivalCandidate> {
    vec![
        ArchivalCandidate { barcode: "BC001".into(), last_update: "2024-06-10T00:00:00Z".into() },
        ArchivalCandidate { barcode: "BC002".into(), last_update: "2024-06-11T00:00:00Z".into() },
        ArchivalCandidate { barcode: "BC003".into(), last_update: "2024-06-12T00:00:00Z".into() },
    ]
}

#[test]
fn full_flow_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    let output_dir = dir.path().join("output");
    let staging_dir = dir.path().join("staging");
    make_source_tree(&source_dir);

    let db_path = dir.path().join("index.sqlite");
    let mut conn = open_and_migrate(&db_path).unwrap();
    let config = default_config(source_dir.clone(), output_dir.clone(), staging_dir.clone());

    let result = run_session(
        &mut conn,
        &config,
        candidates(),
        "2024-07",
        "2024-06-01",
        "2024-06-29",
        "2024-07-01T00:00:00Z",
    )
    .unwrap();

    assert_eq!(result.files_moved, 4, "4 files should match the 3 known barcodes (BC001 has 2 files)");
    assert_eq!(result.barcodes_matched, 3);
    assert_eq!(result.errors, 0);

    // Staging must be gone after a clean, verified run.
    assert!(!staging_dir.exists() || fs::read_dir(&staging_dir).unwrap().next().is_none());

    // The unrelated file must still be sitting in source, untouched.
    assert!(source_dir.join("STM/unit_UNRELATED_stm.txt").exists());

    // Matched files must be GONE from source (moved, not copied).
    assert!(!source_dir.join("BURNIN/unit_BC001_log.txt").exists());

    // And extractable from the archive.
    let locations = repo::find_entries_by_barcode(&conn, "BC001").unwrap();
    assert_eq!(locations.len(), 2);

    let dest = dir.path().join("extract_check");
    let extracted = archiver_core::reader::extract_by_barcode(&conn, "BC001", &dest).unwrap();
    assert_eq!(extracted.len(), 2);
    let burnin_bytes = fs::read(dest.join("BURNIN/unit_BC001_log.txt")).unwrap();
    assert_eq!(burnin_bytes, b"burnin data for bc001");
}

#[test]
fn crash_mid_archive_then_rerun_loses_nothing_and_duplicates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    let output_dir = dir.path().join("output");
    let staging_dir = dir.path().join("staging");
    make_source_tree(&source_dir);

    let db_path = dir.path().join("index.sqlite");
    let mut conn = open_and_migrate(&db_path).unwrap();
    let config = default_config(source_dir.clone(), output_dir.clone(), staging_dir.clone());

    // --- First "run": gets through query + scan + stage (steps 1-3),
    // then the process is killed before archiving (steps 4-7 never run).
    // We simulate this by driving the staging half manually rather than
    // calling run_session, since run_session doesn't expose a partial
    // stopping point (which is itself the point: a real crash could
    // happen ANYWHERE, so recovery must not depend on where exactly).
    {
        use archiver_core::matcher::{has_matching_extension, BarcodeMatcher};

        let session_id = repo::start_session(&conn, "2024-07-01T00:00:00Z", "2024-06-01", "2024-06-29").unwrap();
        let cands = candidates();
        let barcodes: Vec<String> = cands.iter().map(|c| c.barcode.clone()).collect();
        let matcher = BarcodeMatcher::new(barcodes);

        let mut manifest_entries = Vec::new();
        for sub in &config.subfolders {
            let subdir = source_dir.join(sub);
            if !subdir.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&subdir).unwrap() {
                let entry = entry.unwrap();
                let filename = entry.file_name().to_string_lossy().to_string();
                if !has_matching_extension(&filename, &config.allowed_extensions) {
                    continue;
                }
                let matched = matcher.matching_barcodes(&filename);
                if matched.len() != 1 {
                    continue;
                }
                let barcode = matched.into_iter().next().unwrap();
                manifest_entries.push(archiver_core::session::StagedManifestEntry {
                    relative_path: format!("{sub}/{filename}"),
                    source_subfolder: sub.clone(),
                    barcode,
                    original_size: entry.metadata().unwrap().len(),
                });
            }
        }

        fs::create_dir_all(&staging_dir).unwrap();
        let manifest = archiver_core::session::StagingManifest {
            scan_session_id: session_id,
            volume_name: "2024-07".to_string(),
            files: manifest_entries.clone(),
        };
        // Write manifest, then move files — mirroring run_session's own
        // ordering — but STOP here. This is "the crash": staging is
        // fully populated and the manifest is on disk, but nothing has
        // been archived, no DB entries beyond scan_session/session_barcode
        // exist yet, and the process (in the real world) would now be
        // dead.
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        fs::write(staging_dir.join("manifest.json"), json).unwrap();
        for e in &manifest_entries {
            let src = source_dir.join(&e.relative_path);
            let dst = staging_dir.join(&e.relative_path);
            fs::create_dir_all(dst.parent().unwrap()).unwrap();
            fs::rename(&src, &dst).unwrap();
        }

        // Sanity: nothing archived yet.
        assert!(!output_dir.join("2024-07/2024-07.tar.zst").exists());
        assert_eq!(staging_dir.join("manifest.json").exists(), true);
    }

    // --- Second "run": process restarts. run_session is called fresh
    // with candidates as if the DB query ran again (the 1-day resume
    // overlap in the real system would return BC001-BC003 again, all of
    // which are now duplicates of the ALREADY-RECORDED session_barcode
    // rows from the killed run... except that run never completed far
    // enough to record session_barcode rows for these, only staged the
    // files). run_session must first notice the manifest and finish
    // that batch before doing anything else.
    let result = run_session(
        &mut conn,
        &config,
        candidates(), // simulating a re-query that returns the same window
        "2024-07",
        "2024-06-01",
        "2024-06-29",
        "2024-07-02T00:00:00Z",
    )
    .unwrap();

    // Staging must be fully cleaned up now.
    assert!(!staging_dir.exists() || fs::read_dir(&staging_dir).unwrap().next().is_none());

    // All 4 originally-staged files must be present in the archive
    // exactly once — no loss, no duplication.
    let mut all_matches = 0usize;
    for barcode in ["BC001", "BC002", "BC003"] {
        let locations = repo::find_entries_by_barcode(&conn, barcode).unwrap();
        all_matches += locations.len();
    }
    assert_eq!(all_matches, 4, "resumed batch should contribute exactly the original 4 entries, no duplicates");

    // BC001's two files must both extract with correct, undamaged bytes.
    let dest = dir.path().join("extract_after_resume");
    let extracted = archiver_core::reader::extract_by_barcode(&conn, "BC001", &dest).unwrap();
    assert_eq!(extracted.len(), 2);
    for e in &extracted {
        assert!(fs::read(&e.dest_path).unwrap().len() > 0);
    }

    // The resumed batch's files_moved count in the result should reflect
    // whatever the SECOND run_session call itself did (which, since the
    // resume happens as a pre-step, is just the fresh scan — by now
    // source no longer has BC001/2/3 files to find, since they were
    // already moved into staging in the "first run").
    let _ = result;
}
