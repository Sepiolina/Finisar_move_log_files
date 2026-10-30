# `archiver_core/tests/mochi_session_flow.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/tests/mochi_session_flow.rs)

End-to-end session fixtures and crash/recovery simulations over five source subfolders and two archive months. `NoProgress` is a function-pointer alias used when passing no callback.

## Functions

### `make_source_tree`

[Source, line 18](../../../../archiver_core/tests/mochi_session_flow.rs#L18)

```rust
fn make_source_tree(root: &Path)
```

Creates five source folders, four matching fixture files, and one unrelated file for preservation checks.

### `config`

[Source, line 29](../../../../archiver_core/tests/mochi_session_flow.rs#L29)

```rust
fn config(source_dir: PathBuf, output_dir: PathBuf, staging_dir: PathBuf) -> SessionConfig
```

Builds a session configuration using supplied source/output/staging paths, five subfolders, txt/zip filters, and two workers.

### `candidates`

[Source, line 42](../../../../archiver_core/tests/mochi_session_flow.rs#L42)

```rust
fn candidates() -> Vec<ArchivalCandidate>
```

Returns three fixed candidates spanning June and July 2024.

### `run`

[Source, line 51](../../../../archiver_core/tests/mochi_session_flow.rs#L51)

```rust
fn run(conn: &mut rusqlite::Connection, cfg: &SessionConfig, started: &str) -> archiver_core::session::SessionResult
```

Runs a complete session with standard candidates, query bounds, and no progress callback; unwraps and returns the result.

### `stage_like_run_session`

[Source, line 55](../../../../archiver_core/tests/mochi_session_flow.rs#L55)

```rust
fn stage_like_run_session(cfg: &SessionConfig, session_id: i64) -> Vec<StagedManifestEntry>
```

Test-only crash-state builder: finds fixture matches, moves them to staging, records a manifest, and returns entries. Unlike production staging, this helper renames before recording; it simulates an already-completed staged state and is not the production durability algorithm.

### `staging_is_gone`

[Source, line 90](../../../../archiver_core/tests/mochi_session_flow.rs#L90)

```rust
fn staging_is_gone(staging: &Path) -> bool
```

Returns true when the staging path is absent or its directory is empty; errors reading an existing directory panic.

### `full_flow_end_to_end`

[Source, line 95](../../../../archiver_core/tests/mochi_session_flow.rs#L95)

```rust
fn full_flow_end_to_end()
```

Checks four moved files, three matched barcodes, clean staging, unrelated-file preservation, two monthly archives, hash/sequence metadata, and exact indexed extraction.

### `a_second_session_appends_to_the_same_month_as_a_new_commit`

[Source, line 142](../../../../archiver_core/tests/mochi_session_flow.rs#L142)

```rust
fn a_second_session_appends_to_the_same_month_as_a_new_commit()
```

Creates a late file for an already recorded barcode and checks a second session archives it in commit 1 with updated cumulative counts.

### `crash_after_staging_then_rerun_loses_nothing_and_duplicates_nothing`

[Source, line 167](../../../../archiver_core/tests/mochi_session_flow.rs#L167)

```rust
fn crash_after_staging_then_rerun_loses_nothing_and_duplicates_nothing()
```

Builds a manifest/staged state without archives, reruns, and checks cleanup plus exactly four index entries and correct restored bytes.

### `crash_after_commit_but_before_index_is_recovered_without_duplicates`

[Source, line 195](../../../../archiver_core/tests/mochi_session_flow.rs#L195)

```rust
fn crash_after_commit_but_before_index_is_recovered_without_duplicates()
```

Commits June without indexing it, reruns from staging, and checks one namespace/index row per file and correct volume count.

### `a_failed_verification_keeps_staging_and_the_session_open`

[Source, line 230](../../../../archiver_core/tests/mochi_session_flow.rs#L230)

```rust
fn a_failed_verification_keeps_staging_and_the_session_open()
```

Places garbage at June's archive path before resume, expects an error, and checks staged files/manifest and garbage bytes remain. The test does not explicitly query the session status despite its name.

### `cleanup_never_deletes_unlisted_files_left_in_staging`

[Source, line 254](../../../../archiver_core/tests/mochi_session_flow.rs#L254)

```rust
fn cleanup_never_deletes_unlisted_files_left_in_staging()
```

Adds an unlisted orphan, resumes, and checks it survives while listed files and empty directories are removed; verifies cleanup error count and indexed data.

### `resume_with_a_listed_file_that_was_never_renamed_loses_nothing`

[Source, line 282](../../../../archiver_core/tests/mochi_session_flow.rs#L282)

```rust
fn resume_with_a_listed_file_that_was_never_renamed_loses_nothing()
```

Writes a manifest entry while leaving its file in source, then checks resume skips its absent staged path and the fresh scan archives it.

### `invalid_subfolders_are_rejected_at_session_start`

[Source, line 311](../../../../archiver_core/tests/mochi_session_flow.rs#L311)

```rust
fn invalid_subfolders_are_rejected_at_session_start()
```

Checks malformed folder names fail validation, ordinary names pass, and a session with traversal-like configuration aborts before moving a source file.

## Running this file

```sh
cargo test -p archiver_core --test mochi_session_flow
```
