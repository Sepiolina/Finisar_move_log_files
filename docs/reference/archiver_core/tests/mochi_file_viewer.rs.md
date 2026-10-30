# `archiver_core/tests/mochi_file_viewer.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/tests/mochi_file_viewer.rs)

Integration tests for direct archive browsing, extraction, and bounded previews without SQLite. Large fixtures exercise multiple MOCHI chunks.

## Functions

### `dummy`

[Source, line 8](../../../../archiver_core/tests/mochi_file_viewer.rs#L8)

```rust
fn dummy(i: usize, size: usize) -> Vec<u8>
```

Returns `size` deterministic bytes derived from file index `i`, allowing exact byte comparisons.

### `big_volume`

[Source, line 13](../../../../archiver_core/tests/mochi_file_viewer.rs#L13)

```rust
fn big_volume(dir: &std::path::Path, subfolder: &str) -> (std::path::PathBuf, Vec<(String, Vec<u8>)>)
```

Creates twenty 5 MiB fixture files in one named subfolder, stores them in a MOCHI archive, and returns its path plus the expected names/bytes.

### `list_volume_entries_sees_every_file`

[Source, line 31](../../../../archiver_core/tests/mochi_file_viewer.rs#L31)

```rust
fn list_volume_entries_sees_every_file()
```

Checks all twenty files and their sizes appear, directory entries are omitted, and progress is reported.

### `extract_entry_from_volume_returns_exact_bytes_of_a_late_file`

[Source, line 45](../../../../archiver_core/tests/mochi_file_viewer.rs#L45)

```rust
fn extract_entry_from_volume_returns_exact_bytes_of_a_late_file()
```

Extracts the final listed fixture file and checks length, bytes, and absence of leftover partial files.

### `extract_all_from_volume_recovers_every_file_and_overwrites`

[Source, line 59](../../../../archiver_core/tests/mochi_file_viewer.rs#L59)

```rust
fn extract_all_from_volume_recovers_every_file_and_overwrites()
```

Checks all files, one progress call per file, and repeated extraction over a stale existing output on the test host.

### `preview_entry_from_volume_is_bounded`

[Source, line 77](../../../../archiver_core/tests/mochi_file_viewer.rs#L77)

```rust
fn preview_entry_from_volume_is_bounded()
```

Checks a short capped prefix is marked truncated, a larger cap returns all bytes, and an exact size cap is not truncated.

### `preview_of_a_huge_file_reads_only_what_it_needs`

[Source, line 102](../../../../archiver_core/tests/mochi_file_viewer.rs#L102)

```rust
fn preview_of_a_huge_file_reads_only_what_it_needs()
```

Checks a 20 MiB entry preview returns only the requested 64 KiB prefix with `truncated` true. The assertion verifies the result size/content, not direct instrumentation of physical I/O.

## Running this file

```sh
cargo test -p archiver_core --test mochi_file_viewer
```
