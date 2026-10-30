# `archiver_core/tests/mochi_extraction.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/tests/mochi_extraction.rs)

Integration tests for SQLite-directed extraction and lexical destination validation. Fixtures use temporary disk archives and an in-memory index; assertions panic on failure.

## Functions

### `stage`

[Source, line 12](../../../../archiver_core/tests/mochi_extraction.rs#L12)

```rust
fn stage(staging: &Path, files: &[(&str, &[u8])]) -> Vec<StoreFile>
```

Writes fixture byte slices under a staging root, creates parents, and returns `StoreFile` inputs.

### `barcode_of`

[Source, line 24](../../../../archiver_core/tests/mochi_extraction.rs#L24)

```rust
fn barcode_of(p: &str) -> Option<String>
```

Returns the first underscore-separated component beginning with `BC`, or `None`; fixture mapping helper for indexing.

### `indexed_volume`

[Source, line 28](../../../../archiver_core/tests/mochi_extraction.rs#L28)

```rust
fn indexed_volume(dir: &Path) -> (rusqlite::Connection, std::path::PathBuf)
```

Creates three files for two barcodes, commits one monthly archive, starts a session, and indexes it. Returns the in-memory connection and archive path; fixture setup failures panic.

### `extract_by_barcode_uses_the_index_and_restores_exact_bytes`

[Source, line 49](../../../../archiver_core/tests/mochi_extraction.rs#L49)

```rust
fn extract_by_barcode_uses_the_index_and_restores_exact_bytes()
```

Checks extraction of both BC001 files, exact restored bytes, exclusion of BC002, destination containment, and empty output for an absent barcode.

### `extract_locations_from_filename_search`

[Source, line 63](../../../../archiver_core/tests/mochi_extraction.rs#L63)

```rust
fn extract_locations_from_filename_search()
```

Finds BC002 through filename search, extracts its location, and asserts exact restored content.

### `extracting_a_missing_entry_reports_entry_not_found`

[Source, line 74](../../../../archiver_core/tests/mochi_extraction.rs#L74)

```rust
fn extracting_a_missing_entry_reports_entry_not_found()
```

Checks a missing archive entry returns `EntryNotFound` and does not publish the requested output.

### `safe_join_rejects_traversal_and_absolute_paths`

[Source, line 83](../../../../archiver_core/tests/mochi_extraction.rs#L83)

```rust
fn safe_join_rejects_traversal_and_absolute_paths()
```

Checks a valid relative join and rejects parent traversal, rooted names, empty components, dot components, and empty paths.

### `index_rows_with_hostile_paths_cannot_escape_the_destination`

[Source, line 92](../../../../archiver_core/tests/mochi_extraction.rs#L92)

```rust
fn index_rows_with_hostile_paths_cannot_escape_the_destination()
```

Supplies an index location containing `../` and checks extraction errors without creating a file outside the destination.

## Running this file

```sh
cargo test -p archiver_core --test mochi_extraction
```
