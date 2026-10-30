# `archiver_core/src/session/manifest.rs`

[Reference index](../../../README.md) · [Source](../../../../../archiver_core/src/session/manifest.rs)

The append-only staging manifest `staging/manifest.jsonl`: a header line with the session id, then one line per staged file. Lines are synced in batches; a truncated trailing line is ignored on read.

## Functions

### `path_in`

[Source, line 44](../../../../../archiver_core/src/session/manifest.rs#L44)

```rust
pub(crate) fn path_in(staging_dir: &Path) -> PathBuf
```

Returns `<staging_dir>/manifest.jsonl` without filesystem access.

### `read_if_exists`

[Source, line 51](../../../../../archiver_core/src/session/manifest.rs#L51)

```rust
pub fn read_if_exists(staging_dir: &Path) -> Result<Option<Self>>
```

Reads a JSONL manifest and returns `Some` only with a valid first-line session header. Missing/empty/invalid-header files yield `None`; blank and malformed subsequent lines are skipped, including corrupt middle lines. Read/UTF-8 failures propagate as I/O errors.

### `create`

[Source, line 94](../../../../../archiver_core/src/session/manifest.rs#L94)

```rust
pub fn create(staging_dir: &Path, scan_session_id: i64) -> Result<Self>
```

Creates/truncates the manifest in an existing staging directory, writes the session header, then flushes and syncs it. Returns `StagingManifestWriter` or serialization/I/O errors.

### `record`

[Source, line 107](../../../../../archiver_core/src/session/manifest.rs#L107)

```rust
pub fn record(&mut self, entry: StagedManifestEntry) -> Result<()>
```

Serializes one file entry, increments the pending counter, and automatically flushes at 2,000 entries. Recording alone does not move the file; the orchestration explicitly syncs each batch before renames.

### `write_line`

[Source, line 116](../../../../../archiver_core/src/session/manifest.rs#L116)

```rust
fn write_line(&mut self, line: &ManifestLine) -> Result<()>
```

Serializes a `ManifestLine` as compact JSON and appends a newline to the buffered writer. Returns configuration errors for serialization and I/O errors for writes.

### `flush`

[Source, line 124](../../../../../archiver_core/src/session/manifest.rs#L124)

```rust
pub fn flush(&mut self) -> Result<()>
```

Flushes the buffer, calls the manifest file's `sync_data`, and resets the pending counter. Any failure aborts before resetting the count.

### `finish`

[Source, line 137](../../../../../archiver_core/src/session/manifest.rs#L137)

```rust
pub fn finish(mut self) -> Result<()>
```

Consumes the writer after a final flush/sync. Does not remove the manifest.

### `entry`

[Source, line 146](../../../../../archiver_core/src/session/manifest.rs#L146)

```rust
fn entry(i: usize) -> StagedManifestEntry
```

Test helper: a manifest entry for barcode `BC<i>` (three digits) in `BURNIN`, month 2024-07, size 100 + i.

### `manifest_jsonl_write_then_read_round_trips`

[Source, line 158](../../../../../archiver_core/src/session/manifest.rs#L158)

```rust
fn manifest_jsonl_write_then_read_round_trips()
```

Unit test writing five entries and checking the recovered session ID, count, and endpoint barcodes.

### `manifest_jsonl_skips_truncated_trailing_line`

[Source, line 180](../../../../../archiver_core/src/session/manifest.rs#L180)

```rust
fn manifest_jsonl_skips_truncated_trailing_line()
```

Unit test appending malformed final JSON and checking the previous valid entry survives reading.
