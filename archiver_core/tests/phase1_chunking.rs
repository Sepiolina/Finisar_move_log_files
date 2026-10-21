//! - Archive 150MB of dummy files → confirm 3+ chunks were created.
//! - Decompress chunk 2 alone with real `zstd` + `tar` (no custom code) → confirm it extracts cleanly.
//! - Decompress the whole file with `zstd -d | tar --ignore-zeros -xf -` → confirm all files across all chunks extract.
//! - Append a second batch to the same volume → confirm the first batch's files are still extractable afterward.
//!
//! These tests shell out to the real `zstd` and `tar` binaries to avoid
//! validating our own reader against our own writer only.

use archiver_core::ChunkedArchiveWriter;
use std::io::Write;
use std::process::Command;

fn has_binary(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Writes `count` files of `size` bytes each (filled with a repeating
/// pattern derived from index, so corruption/truncation is detectable)
/// into the writer under `subfolder/`.
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
            *b = ((i + j) % 251) as u8; // 251 is prime, avoids trivial periodicity aliasing with block size
        }
        writer
            .add_file(&name, data.as_slice(), size as u64)
            .expect("add_file should succeed");
        written.push((name, data));
    }
    written
}

#[test]
fn archiving_150mb_creates_3_or_more_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let volume_path = dir.path().join("2024-07.tar.zst");

    let mut writer = ChunkedArchiveWriter::open_or_append(&volume_path, 3).unwrap();
    // 30 files * 5MB = 150MB uncompressed payload.
    write_dummy_batch(&mut writer, "BURNIN", 0, 30, 5 * 1024 * 1024);
    let finished = writer.finish().unwrap();

    assert!(
        finished.chunk_spans.len() >= 3,
        "expected 3+ chunks from 150MB of input, got {}",
        finished.chunk_spans.len()
    );
    assert_eq!(finished.entry_count, 30);

    // Spans should be contiguous and non-overlapping, each ending where
    // the file's length is by the last span.
    let mut prev_end = 0u64;
    for span in &finished.chunk_spans {
        assert!(span.chunk_start_byte >= prev_end);
        assert!(span.chunk_end_byte > span.chunk_start_byte);
        prev_end = span.chunk_end_byte;
    }
    let actual_len = std::fs::metadata(&volume_path).unwrap().len();
    assert_eq!(prev_end, actual_len, "last chunk should end at EOF");
}

/// Negative compression levels are zstd's "fast" mode — exactly what the
/// `zstd` CLI's `--fast=N` flag maps to (`N = -level`). Verifies this
/// through the real writer + real `zstd`/`tar` binaries (not just the
/// Rust zstd crate directly), for both `--fast=1` and `--fast=4`.
#[test]
fn negative_compression_level_is_fast_mode_and_extracts_with_real_zstd() {
    if !has_binary("zstd") || !has_binary("tar") {
        eprintln!("skipping: zstd/tar not available in this environment");
        return;
    }

    for fast_n in [1i32, 4i32] {
        let dir = tempfile::tempdir().unwrap();
        let volume_path = dir.path().join("2024-07.tar.zst");

        // --fast=N is level -N.
        let mut writer = ChunkedArchiveWriter::open_or_append(&volume_path, -fast_n).unwrap();
        let written = write_dummy_batch(&mut writer, "BURNIN", 0, 5, 4096);
        let finished = writer.finish().unwrap();
        assert_eq!(finished.entry_count, 5);

        let extract_dir = dir.path().join("extracted");
        std::fs::create_dir_all(&extract_dir).unwrap();

        // Real zstd -d | real tar --ignore-zeros -x, exactly like an
        // operator recovering an archive by hand — no custom code.
        let zstd_out = Command::new("zstd")
            .args(["-d", "--stdout"])
            .arg(&volume_path)
            .output()
            .expect("zstd -d should run");
        assert!(zstd_out.status.success(), "zstd -d failed: {:?}", zstd_out);

        let mut tar_child = Command::new("tar")
            .args(["--ignore-zeros", "-xf", "-", "-C"])
            .arg(&extract_dir)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("tar should spawn");
        tar_child
            .stdin
            .take()
            .unwrap()
            .write_all(&zstd_out.stdout)
            .expect("write to tar stdin");
        let tar_status = tar_child.wait().expect("tar should run");
        assert!(tar_status.success(), "tar extraction failed for --fast={fast_n}");

        for (name, data) in &written {
            let extracted = std::fs::read(extract_dir.join(name))
                .unwrap_or_else(|e| panic!("missing {name} for --fast={fast_n}: {e}"));
            assert_eq!(&extracted, data, "content mismatch for {name} at --fast={fast_n}");
        }
    }
}

#[test]
fn chunk_2_extracts_standalone_with_real_zstd_and_tar() {
    if !has_binary("zstd") || !has_binary("tar") {
        eprintln!("skipping: real zstd/tar binaries not available in this environment");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let volume_path = dir.path().join("2024-07.tar.zst");

    let mut writer = ChunkedArchiveWriter::open_or_append(&volume_path, 3).unwrap();
    let written = write_dummy_batch(&mut writer, "ESS", 0, 30, 5 * 1024 * 1024);
    let finished = writer.finish().unwrap();
    assert!(finished.chunk_spans.len() >= 3);

    let span = finished.chunk_spans[1]; // "chunk 2"

    // Extract just that byte range into its own .zst file, then let the
    // real binaries decompress + untar it with zero flags beyond the
    // default (no --ignore-zeros needed for a single self-terminating chunk).
    let mut vol = std::fs::File::open(&volume_path).unwrap();
    use std::io::{Read, Seek, SeekFrom};
    vol.seek(SeekFrom::Start(span.chunk_start_byte)).unwrap();
    let mut chunk_bytes = vec![0u8; (span.chunk_end_byte - span.chunk_start_byte) as usize];
    vol.read_exact(&mut chunk_bytes).unwrap();

    let chunk_zst_path = dir.path().join("chunk2.zst");
    std::fs::File::create(&chunk_zst_path)
        .unwrap()
        .write_all(&chunk_bytes)
        .unwrap();

    let extract_dir = dir.path().join("extract_chunk2");
    std::fs::create_dir_all(&extract_dir).unwrap();

    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "zstd -d -c {} | tar -xf - -C {}",
            chunk_zst_path.display(),
            extract_dir.display()
        ))
        .status()
        .unwrap();
    assert!(status.success(), "real zstd|tar extraction of a single chunk must succeed with no special flags");

    // Whichever files landed in chunk 2 should extract byte-identical.
    let mut found_any = false;
    for (name, data) in &written {
        let path = extract_dir.join(name);
        if path.exists() {
            found_any = true;
            let extracted = std::fs::read(&path).unwrap();
            assert_eq!(&extracted, data, "extracted file {name} must match original bytes");
        }
    }
    assert!(found_any, "chunk 2 should contain at least one of the written files");
}

#[test]
fn whole_file_extracts_with_ignore_zeros() {
    if !has_binary("zstd") || !has_binary("tar") {
        eprintln!("skipping: real zstd/tar binaries not available in this environment");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let volume_path = dir.path().join("2024-07.tar.zst");

    let mut writer = ChunkedArchiveWriter::open_or_append(&volume_path, 3).unwrap();
    let written = write_dummy_batch(&mut writer, "JTAG", 0, 30, 5 * 1024 * 1024);
    let finished = writer.finish().unwrap();
    assert!(finished.chunk_spans.len() >= 3);

    let extract_dir = dir.path().join("extract_whole");
    std::fs::create_dir_all(&extract_dir).unwrap();

    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "zstd -d -c {} | tar --ignore-zeros -xf - -C {}",
            volume_path.display(),
            extract_dir.display()
        ))
        .status()
        .unwrap();
    assert!(status.success(), "whole-file extraction with --ignore-zeros must succeed");

    for (name, data) in &written {
        let path = extract_dir.join(name);
        assert!(path.exists(), "file {name} should be present in whole-file extraction");
        let extracted = std::fs::read(&path).unwrap();
        assert_eq!(&extracted, data, "extracted file {name} must match original bytes");
    }

    // Sanity: WITHOUT --ignore-zeros, plain tar should stop after the
    // first chunk's footer and NOT recover every file — demonstrates why
    // the flag is required, matching the spec's documentation requirement.
    let extract_dir_no_flag = dir.path().join("extract_whole_no_flag");
    std::fs::create_dir_all(&extract_dir_no_flag).unwrap();
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "zstd -d -c {} | tar -xf - -C {}",
            volume_path.display(),
            extract_dir_no_flag.display()
        ))
        .status()
        .unwrap();
    let missing_without_flag = written
        .iter()
        .any(|(name, _)| !extract_dir_no_flag.join(name).exists());
    assert!(
        missing_without_flag,
        "without --ignore-zeros, extraction should NOT recover every file — \
         if this fails, the footer-per-chunk behavior may have regressed"
    );
}

#[test]
fn append_second_batch_keeps_first_batch_extractable() {
    let dir = tempfile::tempdir().unwrap();
    let volume_path = dir.path().join("2024-07.tar.zst");

    let mut writer1 = ChunkedArchiveWriter::open_or_append(&volume_path, 3).unwrap();
    let batch1 = write_dummy_batch(&mut writer1, "STM", 0, 5, 1024 * 1024);
    let finished1 = writer1.finish().unwrap();
    let batch1_spans = finished1.chunk_spans.clone();

    let mut writer2 = ChunkedArchiveWriter::open_or_append(&volume_path, 3).unwrap();
    let batch2 = write_dummy_batch(&mut writer2, "Xray", 100, 5, 1024 * 1024);
    let finished2 = writer2.finish().unwrap();

    // Batch 2's chunks must start where batch 1's ended — no seek-back,
    // no rewriting.
    let batch1_end = batch1_spans.last().unwrap().chunk_end_byte;
    assert_eq!(
        finished2.chunk_spans.first().unwrap().chunk_start_byte,
        batch1_end,
        "second session's first chunk should start exactly where the first session's last chunk ended"
    );

    // Verify batch 1's files are still extractable from their original spans.
    for span in &batch1_spans {
        let count = archiver_core::reader::verify_chunk_in_ram(
            &volume_path,
            span.chunk_start_byte,
            span.chunk_end_byte,
        )
        .unwrap();
        assert!(count > 0);
    }

    // And batch 2's files extract too.
    for span in &finished2.chunk_spans {
        let count = archiver_core::reader::verify_chunk_in_ram(
            &volume_path,
            span.chunk_start_byte,
            span.chunk_end_byte,
        )
        .unwrap();
        assert!(count > 0);
    }

    let _ = (batch1, batch2); // silence unused warnings if names go unread above
}
