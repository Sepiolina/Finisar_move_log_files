# `archiver_core/tests/large_batch_integrity.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/tests/large_batch_integrity.rs)

Integration tests for staging collisions, independent monthly recovery, incremental verification anchors, and file-count/streaming commit limits. Uses real temporary archive files and indexes.

## Functions

### `config`

[Source, line 17](../../../../archiver_core/tests/large_batch_integrity.rs#L17)

```rust
fn config(root: &Path) -> SessionConfig
```

Returns a two-subfolder `.log` session configuration under the supplied root with three requested workers.

### `candidate`

[Source, line 30](../../../../archiver_core/tests/large_batch_integrity.rs#L30)

```rust
fn candidate(barcode: &str, date: &str) -> ArchivalCandidate
```

Builds one `ArchivalCandidate` from a barcode/date pair.

### `staged`

[Source, line 37](../../../../archiver_core/tests/large_batch_integrity.rs#L37)

```rust
fn staged(dir: &Path, files: &[(&str, Vec<u8>)]) -> Vec<StoreFile>
```

Creates parent directories, writes supplied fixture bytes, and returns their archive-relative/staged-path inputs.

### `staging_never_overwrites_a_file_already_in_staging`

[Source, line 53](../../../../archiver_core/tests/large_batch_integrity.rs#L53)

```rust
fn staging_never_overwrites_a_file_already_in_staging()
```

Checks an existing staged copy and same-named source both survive while another file archives successfully; collision and unlisted-file counts are reported.

### `a_failing_month_keeps_its_files_and_does_not_block_the_others`

[Source, line 77](../../../../archiver_core/tests/large_batch_integrity.rs#L77)

```rust
fn a_failing_month_keeps_its_files_and_does_not_block_the_others()
```

Blocks June's archive with a directory, checks July indexes/cleans while June retains staging/manifest, then removes the obstruction and verifies resume restores all June bytes.

### `append_is_verified_against_the_previously_verified_head`

[Source, line 117](../../../../archiver_core/tests/large_batch_integrity.rs#L117)

```rust
fn append_is_verified_against_the_previously_verified_head()
```

Checks full verification returns an anchor, append preserves it as prior head, incremental verification succeeds, a foreign archive anchor fails, and a changed staged copy fails.

### `commits_are_bounded_by_file_count_and_a_multi_chunk_file_round_trips`

[Source, line 146](../../../../archiver_core/tests/large_batch_integrity.rs#L146)

```rust
fn commits_are_bounded_by_file_count_and_a_multi_chunk_file_round_trips()
```

Stores ten small files plus a multi-chunk file with a three-file budget, asserts four commits and exact metadata hashes/sizes, then verifies and extracts the large file.

## Running this file

```sh
cargo test -p archiver_core --test large_batch_integrity
```
