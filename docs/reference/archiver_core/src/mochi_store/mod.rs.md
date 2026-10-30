# `archiver_core/src/mochi_store/mod.rs`

[Reference index](../../../README.md) · [Source](../../../../../archiver_core/src/mochi_store/mod.rs)

Shared types and helpers of the monthly MOCHI archives: `StoreOptions` (1 GiB / 20,000-file commit budgets), `StoreFile`, `StoredEntry`, `FinishedVolume`, archive paths, BLAKE3 of staged files, read-only opening and catalog lookups. Writing lives in `write.rs`, verification in `verify.rs`; both are re-exported here, so callers keep using `mochi_store::store_volume*` and `mochi_store::verify_volume*`.

## Functions

### `default`

[Source, line 61](../../../../../archiver_core/src/mochi_store/mod.rs#L61)

```rust
fn default() -> Self
```

Returns `StoreOptions` using the 1 GiB byte budget, 20,000-file budget, and automatic worker count (`0`).

### `volume_path`

[Source, line 116](../../../../../archiver_core/src/mochi_store/mod.rs#L116)

```rust
pub fn volume_path(output_dir: &Path, volume_name: &str) -> PathBuf
```

Builds `<output_dir>/<year>/<volume_name>.mochi`, taking year from the text before the first hyphen. Does not validate the month or create directories.

### `hash_file`

[Source, line 124](../../../../../archiver_core/src/mochi_store/mod.rs#L124)

```rust
pub fn hash_file(path: &Path) -> Result<String>
```

Streams a disk file through BLAKE3 using a 256 KiB buffered reader and returns lowercase hexadecimal. Open/read failures become path-bearing I/O errors.

### `with_job_ctx`

[Source, line 135](../../../../../archiver_core/src/mochi_store/mod.rs#L135)

```rust
pub(crate) fn with_job_ctx<T>(f: impl FnOnce(&JobContext<'_>) -> T) -> T
```

Calls `f` with a fresh MOCHI job context using null progress and a cancellation token that is never externally cancelled. Returns the callback's value unchanged.

### `open_storage`

[Source, line 144](../../../../../archiver_core/src/mochi_store/mod.rs#L144)

```rust
fn open_storage(path: &Path) -> Result<OsReadStorage>
```

Opens an `OsReadStorage` without opening its head. Converts storage failures to path-aware archive errors.

### `open_read`

[Source, line 149](../../../../../archiver_core/src/mochi_store/mod.rs#L149)

```rust
pub(crate) fn open_read(path: &Path) -> Result<(OsReadStorage, OpenedHead)>
```

Returns read-only storage plus the opened head using default read options. This opens the head; whole-archive content verification is a separate operation.

### `verified_head`

[Source, line 157](../../../../../archiver_core/src/mochi_store/mod.rs#L157)

```rust
pub(crate) fn verified_head(head: &OpenedHead) -> VerifiedHead
```

Copies archive ID, sequence, and commit ID from an opened head into `VerifiedHead`. The value identifies a head; this helper does not itself run verification.

### `archive_path_of`

[Source, line 166](../../../../../archiver_core/src/mochi_store/mod.rs#L166)

```rust
pub fn archive_path_of(relative_path: &str) -> Result<ArchivePath>
```

Builds a MOCHI `ArchivePath` from slash-separated components. Invalid archive names return mapped MOCHI errors using the relative path as context.

### `stored_file`

[Source, line 173](../../../../../archiver_core/src/mochi_store/mod.rs#L173)

```rust
fn stored_file(
    cat: &Catalog,
    snapshot: &Snapshot,
    path: &ArchivePath,
) -> std::result::Result<Option<(String, u64)>, MochiError>
```

Looks up a regular file in a supplied snapshot/catalog and returns its content hash and logical size. Absent entries, non-files, or missing hash metadata yield `None`; catalog errors propagate.

### `verification_failed`

[Source, line 189](../../../../../archiver_core/src/mochi_store/mod.rs#L189)

```rust
fn verification_failed(volume: &Path, reason: impl Into<String>) -> ArchiveError
```

Constructs a `VerificationFailed` error containing the displayed archive path and supplied reason. Does no I/O.
