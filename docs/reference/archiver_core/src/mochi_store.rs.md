# `archiver_core/src/mochi_store.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/mochi_store.rs)

Creates/appends monthly `.mochi` archives and verifies staged content. `StoreFile` pairs an archive-relative name with a staged disk path; `StoredEntry` records size/hash/sequence. `FinishedVolume` includes the final head, reported entries, new commit sequences, creation flag, prior head, and optional degraded directory durability. Defaults are 1 GiB and 20,000 newly written files per commit; a larger file is allowed alone. Identical existing entries are reported for indexing without a new commit.

## Functions

### `default`

[Source, line 94](../../../../archiver_core/src/mochi_store.rs#L94)

```rust
fn default() -> Self
```

Returns `StoreOptions` using the 1 GiB byte budget, 20,000-file budget, and automatic worker count (`0`).

### `volume_path`

[Source, line 149](../../../../archiver_core/src/mochi_store.rs#L149)

```rust
pub fn volume_path(output_dir: &Path, volume_name: &str) -> PathBuf
```

Builds `<output_dir>/<year>/<volume_name>.mochi`, taking year from the text before the first hyphen. Does not validate the month or create directories.

### `hash_file`

[Source, line 157](../../../../archiver_core/src/mochi_store.rs#L157)

```rust
pub fn hash_file(path: &Path) -> Result<String>
```

Streams a disk file through BLAKE3 using a 256 KiB buffered reader and returns lowercase hexadecimal. Open/read failures become path-bearing I/O errors.

### `with_job_ctx`

[Source, line 168](../../../../archiver_core/src/mochi_store.rs#L168)

```rust
pub(crate) fn with_job_ctx<T>(f: impl FnOnce(&JobContext<'_>) -> T) -> T
```

Calls `f` with a fresh MOCHI job context using null progress and a cancellation token that is never externally cancelled. Returns the callback's value unchanged.

### `open_storage`

[Source, line 177](../../../../archiver_core/src/mochi_store.rs#L177)

```rust
fn open_storage(path: &Path) -> Result<OsReadStorage>
```

Opens an `OsReadStorage` without opening its head. Converts storage failures to path-aware archive errors.

### `open_read`

[Source, line 182](../../../../archiver_core/src/mochi_store.rs#L182)

```rust
pub(crate) fn open_read(path: &Path) -> Result<(OsReadStorage, OpenedHead)>
```

Returns read-only storage plus the opened head using default read options. This opens the head; whole-archive content verification is a separate operation.

### `verified_head`

[Source, line 190](../../../../archiver_core/src/mochi_store.rs#L190)

```rust
pub(crate) fn verified_head(head: &OpenedHead) -> VerifiedHead
```

Copies archive ID, sequence, and commit ID from an opened head into `VerifiedHead`. The value identifies a head; this helper does not itself run verification.

### `archive_path_of`

[Source, line 199](../../../../archiver_core/src/mochi_store.rs#L199)

```rust
pub fn archive_path_of(relative_path: &str) -> Result<ArchivePath>
```

Builds a MOCHI `ArchivePath` from slash-separated components. Invalid archive names return mapped MOCHI errors using the relative path as context.

### `mtime_of`

[Source, line 204](../../../../archiver_core/src/mochi_store.rs#L204)

```rust
fn mtime_of(meta: &fs::Metadata) -> Option<Mtime>
```

Converts filesystem modification time to MOCHI seconds/nanoseconds. Returns `None` for unavailable, pre-epoch, or unrepresentable times.

### `stored_file`

[Source, line 218](../../../../archiver_core/src/mochi_store.rs#L218)

```rust
fn stored_file(
    cat: &Catalog,
    snapshot: &Snapshot,
    path: &ArchivePath,
) -> std::result::Result<Option<(String, u64)>, MochiError>
```

Looks up a regular file in a supplied snapshot/catalog and returns its content hash and logical size. Absent entries, non-files, or missing hash metadata yield `None`; catalog errors propagate.

### `new`

[Source, line 265](../../../../archiver_core/src/mochi_store.rs#L265)

```rust
fn new(files: &'a [StoreFile], opts: &StoreOptions) -> Self
```

`Batcher` constructor: stores the input slice and budgets, clamps file budget to at least one, and starts with no cached head or completed directories.

### `next_batch`

[Source, line 279](../../../../archiver_core/src/mochi_store.rs#L279)

```rust
fn next_batch(&mut self, cat: Option<&Catalog>) -> Result<Option<Batch>>
```

Consumes staged files into a transaction until adding another write would exceed byte/file budgets. Missing/non-file inputs are skipped; identical head hashes produce report-only entries. Adds missing ancestor directories and streaming file sources with mtimes. Returns `None` once no reportable entries remain; I/O/catalog/path failures propagate.

### `missing_dirs`

[Source, line 361](../../../../archiver_core/src/mochi_store.rs#L361)

```rust
fn missing_dirs(&self, path: &ArchivePath) -> Vec<ArchivePath>
```

Returns ancestors missing from both the cached head and directories already scheduled, innermost first. The caller reverses them so parent directories are inserted first.

### `restart`

[Source, line 378](../../../../archiver_core/src/mochi_store.rs#L378)

```rust
fn restart(&mut self)
```

Resets the batch cursor and directory set after losing an archive-creation race. Does not modify staged files.

### `staged_metadata`

[Source, line 386](../../../../archiver_core/src/mochi_store.rs#L386)

```rust
fn staged_metadata(file: &StoreFile) -> Result<Option<fs::Metadata>>
```

Returns metadata for an existing regular staged file. Missing paths and non-files yield `None`; other metadata failures return path-bearing I/O errors.

### `store_volume`

[Source, line 397](../../../../archiver_core/src/mochi_store.rs#L397)

```rust
pub fn store_volume(archive_path: &Path, files: &[StoreFile]) -> Result<FinishedVolume>
```

Delegates to `store_volume_with` with default options and returns a `FinishedVolume`. Writes archives but does not delete staging or perform post-write verification.

### `store_volume_with_budget`

[Source, line 403](../../../../archiver_core/src/mochi_store.rs#L403)

```rust
pub fn store_volume_with_budget(
    archive_path: &Path,
    files: &[StoreFile],
    budget: u64,
) -> Result<FinishedVolume>
```

Delegates with an explicit byte budget and default file budget/workers. Used to exercise commit splitting with small fixtures.

### `store_volume_with`

[Source, line 419](../../../../archiver_core/src/mochi_store.rs#L419)

```rust
pub fn store_volume_with(
    archive_path: &Path,
    files: &[StoreFile],
    store: &StoreOptions,
) -> Result<FinishedVolume>
```

Creates parent directories, obtains a single-writer archive handle, commits bounded streaming transactions, reads catalog hashes, closes the writer, and returns final metadata. Refuses interrupted tails; reports lock conflicts; retains staged inputs on all paths. Empty input for a missing archive returns a default result with its path without creating an archive. A failure can occur after some commits already succeeded.

### `writer_options`

[Source, line 510](../../../../archiver_core/src/mochi_store.rs#L510)

```rust
fn writer_options(threads: usize) -> WriterOptions
```

Returns MOCHI writer options with record timestamps enabled and `threads` set to the supplied worker count, leaving other options at defaults.

### `committed`

[Source, line 531](../../../../archiver_core/src/mochi_store.rs#L531)

```rust
fn committed(&mut self, entries: Vec<PendingEntry>, outcome: &CommitOutcome)
```

Appends pending entries with the commit outcome's sequence, records the new sequence, and keeps the first `DirectoryUnconfirmed` reason. Mutates only the in-memory `Commits` accumulator.

### `unchanged`

[Source, line 541](../../../../archiver_core/src/mochi_store.rs#L541)

```rust
fn unchanged(&mut self, entries: Vec<PendingEntry>, seq: u64)
```

Appends report-only entries associated with the current head sequence without recording a new commit. Such sequences mean present by this head, not necessarily introduced there.

### `open_or_create_writer`

[Source, line 557](../../../../archiver_core/src/mochi_store.rs#L557)

```rust
fn open_or_create_writer(
    archive_path: &Path,
    dir: &mut OsDir,
    name: &str,
    threads: usize,
    batcher: &mut Batcher<'_>,
    commits: &mut Commits,
    ctx: &JobContext<'_>,
) -> Result<Option<OpenedWriter>>
```

If missing, creates the archive using the first batch as its initial commit. Returns `None` when no files remain; on `DestinationExists`, restarts batching and opens append. Existing archives use `TailRepair::Refuse`; returns writer, creation flag, and prior head identity.

### `read_back_entries`

[Source, line 610](../../../../archiver_core/src/mochi_store.rs#L610)

```rust
fn read_back_entries(
    archive_path: &Path,
    cat: &Catalog,
    pending: Vec<(PendingEntry, u64)>,
) -> Result<Vec<StoredEntry>>
```

Replays the final catalog head once and converts pending entries to stored records. Reuses known hashes for unchanged entries; otherwise obtains MOCHI-computed hash/length from the catalog. A missing committed file returns `VerificationFailed`; this metadata pass is not a staged-byte read-back check.

### `verification_failed`

[Source, line 643](../../../../archiver_core/src/mochi_store.rs#L643)

```rust
fn verification_failed(volume: &Path, reason: impl Into<String>) -> ArchiveError
```

Constructs a `VerificationFailed` error containing the displayed archive path and supplied reason. Does no I/O.

### `verify_volume`

[Source, line 655](../../../../archiver_core/src/mochi_store.rs#L655)

```rust
pub fn verify_volume(archive_path: &Path, files: &[StoreFile]) -> Result<()>
```

Runs staged-file checks with one worker and full content-integrity archive verification, discarding the returned head. Does not change the archive or staged inputs.

### `verify_volume_with`

[Source, line 669](../../../../archiver_core/src/mochi_store.rs#L669)

```rust
pub fn verify_volume_with(
    archive_path: &Path,
    files: &[StoreFile],
    since: Option<&VerifiedHead>,
    workers: usize,
    mut on_progress: impl FnMut(usize),
) -> Result<VerifiedHead>
```

Checks staged files in parallel slices, limiting worker count using a 2,000-file threshold and requires all workers to observe one head. Then verifies the entire archive when `since` is absent, new commits when a previous head is supplied, or skips structural work for an identical anchor. Reports per-file progress on the caller thread and returns the checked head; mismatched heads or failed checks return errors.

### `verify_files`

[Source, line 760](../../../../archiver_core/src/mochi_store.rs#L760)

```rust
fn verify_files(
    archive_path: &Path,
    files: &[StoreFile],
    checked: &AtomicUsize,
) -> Result<VerifiedHead>
```

Opens an independent read-only handle/snapshot, confirms each path is a file, compares its catalog hash with the current staged-file hash, and verifies streamed archived bytes against that hash. Increments the shared atomic count after each successful file and returns the observed head.

### `verify_whole`

[Source, line 831](../../../../archiver_core/src/mochi_store.rs#L831)

```rust
fn verify_whole(
    archive_path: &Path,
    src: &OsReadStorage,
    ctx: &JobContext<'_>,
) -> Result<Option<VerifiedHead>>
```

Runs MOCHI verification at `ContentIntegrity` level. Operational error, exit code 1, or any failing dimension becomes `VerificationFailed` with up to five findings; otherwise returns the verification's optional head.
