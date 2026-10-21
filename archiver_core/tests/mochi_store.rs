//! Storage-layer tests (replaces the old chunking tests): create/append the
//! monthly `.mochi` archive, byte-identical round trip, bounded commits,
//! verification, and the "never truncate / never wait" error behavior.

use archiver_core::mochi_store::{self, StoreFile};
use archiver_core::{reader, ArchiveError};
use mochi_core::object::OsIds;
use mochi_core::publish::{open_head, ArchiveWriter, ReadOptions, TailRepair, WriterOptions};
use mochi_core::storage::os::{OsDir, OsReadStorage};
use std::fs;
use std::path::Path;

/// Deterministic, poorly-compressible bytes.
fn pseudo_random(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Writes `files` under `<staging>/<relative_path>` and returns the store list.
fn stage(staging: &Path, files: &[(&str, Vec<u8>)]) -> Vec<StoreFile> {
    files
        .iter()
        .map(|(rel, data)| {
            let p = staging.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, data).unwrap();
            StoreFile {
                relative_path: rel.to_string(),
                staged_path: p,
            }
        })
        .collect()
}

fn head_seq(archive: &Path) -> u64 {
    let src = OsReadStorage::open(archive).unwrap();
    open_head(&src, &ReadOptions::default()).unwrap().seq()
}

#[test]
fn store_then_list_then_extract_is_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let staging = dir.path().join("staging");
    let big = pseudo_random(1, 9 * 1024 * 1024 + 123); // spans more than one 8 MiB MOCHI chunk
    let files = vec![
        ("BURNIN/unit_BC001_log.txt", b"burnin data".to_vec()),
        ("ESS/unit_BC001_ess.txt", Vec::new()), // empty file
        ("JTAG/unit_BC002_big.zip", big),
        ("Xray/unit_BC003_bin.zip", pseudo_random(2, 70_000)),
    ];
    let store = stage(&staging, &files);
    let archive = mochi_store::volume_path(&dir.path().join("out"), "2024-07");
    assert!(archive.ends_with("2024/2024-07.mochi"));

    let finished = mochi_store::store_volume(&archive, &store).unwrap();
    assert!(finished.created);
    assert_eq!(finished.entry_count, 4);
    assert_eq!(finished.written_entries.len(), 4);
    assert!(finished.file_size > 0);
    assert_eq!(finished.head_commit_id.len(), 64);
    mochi_store::verify_volume(&archive, &store).unwrap();

    // Listing: files only, with the right sizes.
    let mut listed = reader::list_volume_entries(&archive, |_| {}).unwrap();
    listed.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    assert_eq!(listed.len(), 4);
    for (rel, data) in &files {
        let e = listed.iter().find(|e| &e.relative_path == rel).expect(rel);
        assert_eq!(e.size, data.len() as u64);
    }

    // Recorded hashes are plain BLAKE3 of the file bytes.
    for e in &finished.written_entries {
        let data = &files.iter().find(|(r, _)| *r == e.relative_path).unwrap().1;
        assert_eq!(e.content_hash, blake3::hash(data).to_hex().to_string());
        assert_eq!(e.original_size, data.len() as u64);
    }

    // Extract one and all.
    for (rel, data) in &files {
        let dest = dir.path().join("one").join(rel);
        let n = reader::extract_entry_from_volume(&archive, rel, &dest).unwrap();
        assert_eq!(n, data.len() as u64);
        assert_eq!(&fs::read(&dest).unwrap(), data);
    }
    let dest_all = dir.path().join("all");
    let extracted = reader::extract_all_from_volume(&archive, &dest_all, |_| {}).unwrap();
    assert_eq!(extracted.len(), 4);
    for (rel, data) in &files {
        assert_eq!(&fs::read(dest_all.join(rel)).unwrap(), data);
    }
}

#[test]
fn subfolder_directory_entry_exists_and_mtime_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let store = stage(&dir.path().join("s"), &[("BURNIN/a_BC001.txt", b"x".to_vec())]);
    let archive = dir.path().join("2024-07.mochi");
    mochi_store::store_volume(&archive, &store).unwrap();

    let src = OsReadStorage::open(&archive).unwrap();
    let head = open_head(&src, &ReadOptions::default()).unwrap();
    let all = mochi_core::read::list(&head, None).unwrap();
    let kinds: Vec<(String, bool)> = all
        .iter()
        .map(|e| {
            (
                String::from_utf8_lossy(e.path.as_stored()).into_owned(),
                e.content_hash.is_none(),
            )
        })
        .collect();
    assert!(kinds.contains(&("BURNIN".to_string(), true)), "directory entry missing: {kinds:?}");
    assert!(kinds.contains(&("BURNIN/a_BC001.txt".to_string(), false)));
}

#[test]
fn append_to_existing_month_adds_a_new_commit_and_keeps_old_files() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("2024-07.mochi");
    let s1 = stage(&dir.path().join("s1"), &[("BURNIN/a_BC001.txt", b"first".to_vec())]);
    let f1 = mochi_store::store_volume(&archive, &s1).unwrap();
    assert!(f1.created);
    let seq1 = head_seq(&archive);

    let s2 = stage(
        &dir.path().join("s2"),
        &[("BURNIN/b_BC002.txt", b"second".to_vec()), ("ESS/c_BC002.txt", b"third".to_vec())],
    );
    let f2 = mochi_store::store_volume(&archive, &s2).unwrap();
    assert!(!f2.created);
    assert_eq!(head_seq(&archive), seq1 + 1, "an append is exactly one new commit");
    assert_eq!(f2.head_seq, seq1 + 1);
    assert_ne!(f1.head_commit_id, f2.head_commit_id);
    assert!(f2.file_size > f1.file_size);
    assert!(f2.written_entries.iter().all(|e| e.commit_seq == seq1 + 1));

    let listed = reader::list_volume_entries(&archive, |_| {}).unwrap();
    assert_eq!(listed.len(), 3);
    let (bytes, _) = reader::preview_entry_from_volume(&archive, "BURNIN/a_BC001.txt", 100).unwrap();
    assert_eq!(bytes, b"first");
    mochi_store::verify_volume(&archive, &s2).unwrap();
    mochi_store::verify_volume(&archive, &s1).unwrap();
}

#[test]
fn large_volume_is_split_into_bounded_commits() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("2024-07.mochi");
    // Budget 1000 bytes: 4 x 400-byte files -> 2 per commit (400+400 <= 1000, +400 > 1000),
    // then a 5000-byte file that exceeds the budget on its own gets its own commit,
    // then one more small file.
    let files: Vec<(String, Vec<u8>)> = vec![
        ("BURNIN/f1.txt".into(), pseudo_random(1, 400)),
        ("BURNIN/f2.txt".into(), pseudo_random(2, 400)),
        ("BURNIN/f3.txt".into(), pseudo_random(3, 400)),
        ("BURNIN/f4.txt".into(), pseudo_random(4, 400)),
        ("ESS/huge.txt".into(), pseudo_random(5, 5000)),
        ("ESS/tail.txt".into(), pseudo_random(6, 10)),
    ];
    let refs: Vec<(&str, Vec<u8>)> = files.iter().map(|(a, b)| (a.as_str(), b.clone())).collect();
    let store = stage(&dir.path().join("s"), &refs);

    let finished = mochi_store::store_volume_with_budget(&archive, &store, 1000).unwrap();
    assert_eq!(finished.commit_seqs.len(), 4, "got commits {:?}", finished.commit_seqs);
    assert_eq!(finished.entry_count, 6);
    let seq_of = |name: &str| {
        finished
            .written_entries
            .iter()
            .find(|e| e.relative_path == name)
            .unwrap()
            .commit_seq
    };
    assert_eq!(seq_of("BURNIN/f1.txt"), seq_of("BURNIN/f2.txt"));
    assert_eq!(seq_of("BURNIN/f3.txt"), seq_of("BURNIN/f4.txt"));
    assert_ne!(seq_of("BURNIN/f2.txt"), seq_of("BURNIN/f3.txt"));
    // The oversized file is alone in its commit.
    let huge = seq_of("ESS/huge.txt");
    assert_eq!(finished.written_entries.iter().filter(|e| e.commit_seq == huge).count(), 1);
    assert_ne!(seq_of("ESS/tail.txt"), huge);

    mochi_store::verify_volume(&archive, &store).unwrap();
    let listed = reader::list_volume_entries(&archive, |_| {}).unwrap();
    assert_eq!(listed.len(), 6);
    for (rel, data) in &refs {
        let got = reader::extract_entry_to_bytes(&archive, rel).unwrap();
        assert_eq!(&got, data);
    }
}

#[test]
fn restaging_the_same_file_is_idempotent_for_resume() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("2024-07.mochi");
    let store = stage(&dir.path().join("s"), &[("BURNIN/a_BC001.txt", b"same bytes".to_vec())]);
    mochi_store::store_volume(&archive, &store).unwrap();
    // A resumed run re-stores the identical file.
    mochi_store::store_volume(&archive, &store).unwrap();
    assert_eq!(reader::list_volume_entries(&archive, |_| {}).unwrap().len(), 1);
    mochi_store::verify_volume(&archive, &store).unwrap();
}

#[test]
fn verification_fails_when_the_staged_file_differs_from_the_archive() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("2024-07.mochi");
    let store = stage(&dir.path().join("s"), &[("BURNIN/a_BC001.txt", b"original".to_vec())]);
    mochi_store::store_volume(&archive, &store).unwrap();
    fs::write(&store[0].staged_path, b"tampered").unwrap();
    let err = mochi_store::verify_volume(&archive, &store).unwrap_err();
    assert!(matches!(err, ArchiveError::VerificationFailed { .. }), "{err}");

    // And a file that was never stored is reported missing.
    let other = stage(&dir.path().join("s2"), &[("ESS/never_stored.txt", b"zzz".to_vec())]);
    assert!(matches!(
        mochi_store::verify_volume(&archive, &other).unwrap_err(),
        ArchiveError::VerificationFailed { .. }
    ));
}

#[test]
fn bit_flip_inside_the_archive_fails_verification() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("2024-07.mochi");
    let store = stage(&dir.path().join("s"), &[("BURNIN/a_BC001.txt", pseudo_random(9, 50_000))]);
    mochi_store::store_volume(&archive, &store).unwrap();
    let mut bytes = fs::read(&archive).unwrap();
    // Corrupt the middle of the file (inside the stored chunk).
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    fs::write(&archive, &bytes).unwrap();
    assert!(mochi_store::verify_volume(&archive, &store).is_err());
}

#[test]
fn uncommitted_tail_is_refused_and_never_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("2024-07.mochi");
    let s1 = stage(&dir.path().join("s1"), &[("BURNIN/a_BC001.txt", b"one".to_vec())]);
    mochi_store::store_volume(&archive, &s1).unwrap();

    // Simulate an interrupted write: bytes after the last commit.
    let mut f = fs::OpenOptions::new().append(true).open(&archive).unwrap();
    use std::io::Write as _;
    f.write_all(&pseudo_random(7, 4096)).unwrap();
    drop(f);
    let len_before = fs::metadata(&archive).unwrap().len();

    let s2 = stage(&dir.path().join("s2"), &[("BURNIN/b_BC002.txt", b"two".to_vec())]);
    let err = mochi_store::store_volume(&archive, &s2).unwrap_err();
    eprintln!("tail error: {err}");
    assert!(
        matches!(err, ArchiveError::UncommittedTail { .. } | ArchiveError::Mochi { .. }),
        "unexpected error: {err}"
    );
    assert_eq!(fs::metadata(&archive).unwrap().len(), len_before, "the archive must not be modified");
}

#[test]
fn interrupted_commit_surfaces_uncommitted_tail_and_is_not_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("2024-07.mochi");
    let s1 = stage(&dir.path().join("s1"), &[("BURNIN/a_BC001.txt", pseudo_random(1, 20_000))]);
    mochi_store::store_volume(&archive, &s1).unwrap();
    let s2 = stage(&dir.path().join("s2"), &[("BURNIN/b_BC002.txt", pseudo_random(2, 20_000))]);
    mochi_store::store_volume(&archive, &s2).unwrap();

    // Cut the second commit short: its data is there, its footer is not.
    let len = fs::metadata(&archive).unwrap().len();
    let f = fs::OpenOptions::new().write(true).open(&archive).unwrap();
    f.set_len(len - 50).unwrap();
    drop(f);

    let s3 = stage(&dir.path().join("s3"), &[("ESS/c_BC003.txt", b"three".to_vec())]);
    let err = mochi_store::store_volume(&archive, &s3).unwrap_err();
    assert!(matches!(err, ArchiveError::UncommittedTail { .. }), "{err}");
    assert!(err.to_string().contains("UNCOMMITTED_TAIL"));
    assert_eq!(fs::metadata(&archive).unwrap().len(), len - 50, "no automatic truncation");
}

#[test]
fn second_writer_gets_a_clear_lock_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("2024-07.mochi");
    let s1 = stage(&dir.path().join("s1"), &[("BURNIN/a_BC001.txt", b"one".to_vec())]);
    mochi_store::store_volume(&archive, &s1).unwrap();

    // Another writer holds the archive.
    let mut odir = OsDir::containing(&archive).unwrap();
    let (held, _) = ArchiveWriter::open_append_in(
        &mut odir,
        "2024-07.mochi",
        Box::new(OsIds),
        WriterOptions::default(),
        TailRepair::Refuse,
    )
    .unwrap();

    let s2 = stage(&dir.path().join("s2"), &[("BURNIN/b_BC002.txt", b"two".to_vec())]);
    let err = mochi_store::store_volume(&archive, &s2).unwrap_err();
    assert!(matches!(err, ArchiveError::ArchiveLocked { .. }), "{err}");
    assert!(err.to_string().contains("LOCK_CONFLICT"));

    held.close().unwrap();
    // After release the append goes through.
    mochi_store::store_volume(&archive, &s2).unwrap();
}

#[test]
fn mochi_errors_carry_their_stable_code() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.mochi");
    let err = reader::list_volume_entries(&missing, |_| {}).unwrap_err();
    match err {
        ArchiveError::Mochi { code, .. } => assert_eq!(code, "IO_ERROR"),
        other => panic!("unexpected {other}"),
    }
}
