//! Integrity rules added for large batches: staging never overwrites, one
//! failing month does not block the others (and loses nothing), appends are
//! verified against the previously verified head, and commits are bounded by
//! file count as well as bytes.

use archiver_core::db::ArchivalCandidate;
use archiver_core::mochi_store::{self, StoreFile, StoreOptions, STREAM_THRESHOLD};
use archiver_core::reader;
use archiver_core::repo;
use archiver_core::schema::open_and_migrate;
use archiver_core::session::{run_session, SessionConfig};
use std::fs;
use std::path::Path;

type NoProgress = fn(String, usize, usize);

fn config(root: &Path) -> SessionConfig {
    SessionConfig {
        source_dir: root.join("src"),
        output_dir: root.join("out"),
        staging_dir: root.join("staging"),
        subfolders: vec!["BURNIN".into(), "ESS".into()],
        allowed_extensions: vec!["log".into()],
        lookback_days: 2,
        date_range_override: None,
        workers: 3,
    }
}

fn candidate(barcode: &str, date: &str) -> ArchivalCandidate {
    ArchivalCandidate {
        barcode: barcode.into(),
        last_update: date.into(),
    }
}

fn staged(dir: &Path, files: &[(&str, Vec<u8>)]) -> Vec<StoreFile> {
    files
        .iter()
        .map(|(rel, data)| {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, data).unwrap();
            StoreFile {
                relative_path: rel.to_string(),
                staged_path: p,
            }
        })
        .collect()
}

#[test]
fn staging_never_overwrites_a_file_already_in_staging() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    for sub in &cfg.subfolders {
        fs::create_dir_all(cfg.source_dir.join(sub)).unwrap();
    }
    fs::write(cfg.source_dir.join("BURNIN/u_BC001_a.log"), b"new").unwrap();
    fs::write(cfg.source_dir.join("BURNIN/u_BC001_b.log"), b"other").unwrap();
    // A leftover of a crashed run whose manifest line was lost.
    fs::create_dir_all(cfg.staging_dir.join("BURNIN")).unwrap();
    fs::write(cfg.staging_dir.join("BURNIN/u_BC001_a.log"), b"only copy").unwrap();

    let mut conn = open_and_migrate(tmp.path().join("idx.sqlite")).unwrap();
    let r = run_session(&mut conn, &cfg, vec![candidate("BC001", "2024-06-01")], "a", "b", "t", None::<&NoProgress>)
        .unwrap();

    assert_eq!(fs::read(cfg.staging_dir.join("BURNIN/u_BC001_a.log")).unwrap(), b"only copy");
    assert_eq!(fs::read(cfg.source_dir.join("BURNIN/u_BC001_a.log")).unwrap(), b"new");
    assert_eq!(r.files_moved, 1, "the other file is archived");
    assert!(r.errors >= 2, "the refused move and the unlisted leftover are both reported");
    assert!(!cfg.source_dir.join("BURNIN/u_BC001_b.log").exists());
}

#[test]
fn a_failing_month_keeps_its_files_and_does_not_block_the_others() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    for sub in &cfg.subfolders {
        fs::create_dir_all(cfg.source_dir.join(sub)).unwrap();
    }
    for i in 0..20 {
        fs::write(cfg.source_dir.join(format!("BURNIN/u_BC001_{i}.log")), format!("june {i}")).unwrap();
        fs::write(cfg.source_dir.join(format!("ESS/u_BC002_{i}.log")), format!("july {i}")).unwrap();
    }
    // June's archive path is taken by a directory: that month cannot be written.
    let june = mochi_store::volume_path(&cfg.output_dir, "2024-06");
    fs::create_dir_all(&june).unwrap();

    let candidates = || vec![candidate("BC001", "2024-06-10"), candidate("BC002", "2024-07-10")];
    let mut conn = open_and_migrate(tmp.path().join("idx.sqlite")).unwrap();
    let err = run_session(&mut conn, &cfg, candidates(), "a", "b", "t1", None::<&NoProgress>);
    assert!(err.is_err());

    // July: committed, indexed, and removed from staging.
    assert_eq!(repo::find_entries_by_barcode(&conn, "BC002").unwrap().len(), 20);
    assert!(!cfg.staging_dir.join("ESS").exists() || fs::read_dir(cfg.staging_dir.join("ESS")).unwrap().count() == 0);
    // June: every file still in staging, nothing indexed, manifest kept.
    assert_eq!(fs::read_dir(cfg.staging_dir.join("BURNIN")).unwrap().count(), 20);
    assert!(repo::find_entries_by_barcode(&conn, "BC001").unwrap().is_empty());
    assert!(cfg.staging_dir.join("manifest.jsonl").exists());

    // Once the problem is fixed the next run resumes June first.
    fs::remove_dir(&june).unwrap();
    run_session(&mut conn, &cfg, Vec::new(), "a", "b", "t2", None::<&NoProgress>).unwrap();
    assert!(!cfg.staging_dir.exists());
    let locs = repo::find_entries_by_barcode(&conn, "BC001").unwrap();
    assert_eq!(locs.len(), 20);
    for i in 0..20 {
        let bytes = reader::extract_entry_to_bytes(&june, &format!("BURNIN/u_BC001_{i}.log")).unwrap();
        assert_eq!(bytes, format!("june {i}").into_bytes());
    }
}

#[test]
fn append_is_verified_against_the_previously_verified_head() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("2024/2024-06.mochi");
    let s1 = staged(&tmp.path().join("s1"), &[("BURNIN/a.log", b"a".to_vec())]);
    let first = mochi_store::store_volume(&archive, &s1).unwrap();
    assert!(first.prior_head.is_none(), "a new archive has no prior head");
    let head1 = mochi_store::verify_volume_with(&archive, &s1, None, 2, |_| {}).unwrap();

    let s2 = staged(&tmp.path().join("s2"), &[("BURNIN/b.log", b"b".to_vec()), ("ESS/c.log", b"c".to_vec())]);
    let second = mochi_store::store_volume(&archive, &s2).unwrap();
    assert_eq!(second.prior_head, Some(head1));
    let head2 = mochi_store::verify_volume_with(&archive, &s2, Some(&head1), 2, |_| {}).unwrap();
    assert_eq!(head2.seq, second.head_seq);
    // Nothing new since head2: still checks every listed file.
    mochi_store::verify_volume_with(&archive, &s2, Some(&head2), 2, |_| {}).unwrap();

    // An anchor from a different archive is not a continuation.
    let other = tmp.path().join("other.mochi");
    let s3 = staged(&tmp.path().join("s3"), &[("X/z.log", b"z".to_vec())]);
    mochi_store::store_volume(&other, &s3).unwrap();
    let foreign = mochi_store::verify_volume_with(&other, &s3, None, 1, |_| {}).unwrap();
    assert!(mochi_store::verify_volume_with(&archive, &s2, Some(&foreign), 2, |_| {}).is_err());

    // A staged file that differs from what was committed fails.
    fs::write(&s2[0].staged_path, b"changed").unwrap();
    assert!(mochi_store::verify_volume_with(&archive, &s2, Some(&head1), 2, |_| {}).is_err());
}

#[test]
fn commits_are_bounded_by_file_count_and_big_files_are_streamed() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("2024/2024-06.mochi");
    let mut files: Vec<(String, Vec<u8>)> = (0..10).map(|i| (format!("BURNIN/f{i}.log"), format!("small {i}").into_bytes())).collect();
    let big: Vec<u8> = (0..STREAM_THRESHOLD as usize + 12_345).map(|i| (i % 251) as u8).collect();
    files.push(("Xray/big.bin".into(), big.clone()));
    let refs: Vec<(&str, Vec<u8>)> = files.iter().map(|(p, d)| (p.as_str(), d.clone())).collect();
    let store = staged(&tmp.path().join("s"), &refs);

    let finished = mochi_store::store_volume_with(
        &archive,
        &store,
        &StoreOptions {
            file_budget: 3,
            workers: 2,
            ..StoreOptions::default()
        },
    )
    .unwrap();
    assert_eq!(finished.commit_seqs.len(), 4, "11 files, at most 3 per commit");
    assert_eq!(finished.written_entries.len(), 11);
    for (e, (_, data)) in finished.written_entries.iter().zip(&files) {
        assert_eq!(e.content_hash, blake3::hash(data).to_hex().to_string(), "{}", e.relative_path);
        assert_eq!(e.original_size, data.len() as u64);
    }
    mochi_store::verify_volume_with(&archive, &store, None, 3, |_| {}).unwrap();
    assert_eq!(reader::extract_entry_to_bytes(&archive, "Xray/big.bin").unwrap(), big);
}
