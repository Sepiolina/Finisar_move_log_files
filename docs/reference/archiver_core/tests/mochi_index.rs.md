# `archiver_core/tests/mochi_index.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/tests/mochi_index.rs)

SQLite schema and repository integration tests. `FinishedVolume` and `StoredEntry` fixtures are synthetic metadata; these tests do not need actual archive content.

## Functions

### `finished`

[Source, line 9](../../../../archiver_core/tests/mochi_index.rs#L9)

```rust
fn finished(path: &str, size: u64, head: &str, seq: u64, entries: Vec<StoredEntry>) -> FinishedVolume
```

Builds a `FinishedVolume` fixture with supplied path, size, head, sequence, and entries; sets a single commit sequence and no durability/prior-head state.

### `entry`

[Source, line 24](../../../../archiver_core/tests/mochi_index.rs#L24)

```rust
fn entry(rel: &str, size: u64, hash: &str, seq: u64) -> StoredEntry
```

Builds a `StoredEntry` fixture from relative path, size, hash, and sequence.

### `columns`

[Source, line 33](../../../../archiver_core/tests/mochi_index.rs#L33)

```rust
fn columns(conn: &rusqlite::Connection, table: &str) -> Vec<String>
```

Reads `PRAGMA table_info` and returns all column names for test assertions.

### `schema_creates_all_four_tables_with_mochi_columns`

[Source, line 39](../../../../archiver_core/tests/mochi_index.rs#L39)

```rust
fn schema_creates_all_four_tables_with_mochi_columns()
```

Checks table existence, current MOCHI metadata fields, removal of legacy offset/hash fields, and `user_version` 2.

### `reopening_a_current_database_is_fine_and_keeps_data`

[Source, line 64](../../../../archiver_core/tests/mochi_index.rs#L64)

```rust
fn reopening_a_current_database_is_fine_and_keeps_data()
```

Creates a session in a disk-backed database, reopens it through schema setup, and checks the row remains.

### `legacy_tar_zst_database_is_detected_and_not_touched`

[Source, line 76](../../../../archiver_core/tests/mochi_index.rs#L76)

```rust
fn legacy_tar_zst_database_is_detected_and_not_touched()
```

Builds an unversioned legacy table, expects `SchemaMismatch`, and confirms its row and zero version remain unchanged.

### `newer_schema_version_is_rejected`

[Source, line 102](../../../../archiver_core/tests/mochi_index.rs#L102)

```rust
fn newer_schema_version_is_rejected()
```

Sets schema version 99 and checks reopening rejects it with the reported version.

### `scan_session_insert_query_round_trip`

[Source, line 110](../../../../archiver_core/tests/mochi_index.rs#L110)

```rust
fn scan_session_insert_query_round_trip()
```

Creates/completes a session and checks the last-successful query end is returned.

### `session_barcode_unique_constraint_rejects_duplicate`

[Source, line 119](../../../../archiver_core/tests/mochi_index.rs#L119)

```rust
fn session_barcode_unique_constraint_rejects_duplicate()
```

Checks a repeated barcode/update pair is suppressed across sessions, a new update can be inserted, and scanned lookups reflect both cases.

### `scanned_log_entry_unique_constraint_is_enforced_in_the_schema`

[Source, line 132](../../../../archiver_core/tests/mochi_index.rs#L132)

```rust
fn scanned_log_entry_unique_constraint_is_enforced_in_the_schema()
```

Uses raw inserts to check the schema rejects duplicate `(relative_path,volume)` rows independently of repository upsert behavior.

### `insert_log_entry_is_idempotent_for_resume`

[Source, line 150](../../../../archiver_core/tests/mochi_index.rs#L150)

```rust
fn insert_log_entry_is_idempotent_for_resume()
```

Upserts one path twice and checks the same row ID, one row total, and updated hash/sequence metadata.

### `upsert_volume_tracks_head_on_append`

[Source, line 175](../../../../archiver_core/tests/mochi_index.rs#L175)

```rust
fn upsert_volume_tracks_head_on_append()
```

Checks volume updates retain ID/creation label while changing byte size, head, sequence, count, and update label.

### `record_finished_volume_end_to_end_across_two_appends`

[Source, line 194](../../../../archiver_core/tests/mochi_index.rs#L194)

```rust
fn record_finished_volume_end_to_end_across_two_appends()
```

Records two batches, checks searchable file hashes/sequences and cumulative volume count/head, then records the second batch again without count inflation.

## Running this file

```sh
cargo test -p archiver_core --test mochi_index
```
