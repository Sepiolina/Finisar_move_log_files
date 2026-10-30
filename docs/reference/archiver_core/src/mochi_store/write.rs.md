# `archiver_core/src/mochi_store/write.rs`

[Reference index](../../../README.md) · [Source](../../../../../archiver_core/src/mochi_store/write.rs)

Creates or appends one month's `.mochi` archive in bounded, streamed commits. A missing archive is created with no-replace semantics; an existing one is opened with `TailRepair::Refuse`. Identical existing entries are reported for indexing without a new commit. The head is replayed once per call and once more to read back MOCHI's content hashes.

## Functions

### `store_volume`

[Source, line 49](../../../../../archiver_core/src/mochi_store/write.rs#L49)

```rust
pub fn store_volume(archive_path: &Path, files: &[StoreFile]) -> Result<FinishedVolume>
```

Delegates to `store_volume_with` with default options and returns a `FinishedVolume`. Writes archives but does not delete staging or perform post-write verification.

### `store_volume_with_budget`

[Source, line 55](../../../../../archiver_core/src/mochi_store/write.rs#L55)

```rust
pub fn store_volume_with_budget(
    archive_path: &Path,
    files: &[StoreFile],
    budget: u64,
) -> Result<FinishedVolume>
```

Delegates with an explicit byte budget and default file budget/workers. Used to exercise commit splitting with small fixtures.

### `store_volume_with`

[Source, line 71](../../../../../archiver_core/src/mochi_store/write.rs#L71)

```rust
pub fn store_volume_with(
    archive_path: &Path,
    files: &[StoreFile],
    store: &StoreOptions,
) -> Result<FinishedVolume>
```

Creates parent directories, obtains a single-writer archive handle, commits bounded streaming transactions, reads catalog hashes, closes the writer, and returns final metadata. Refuses interrupted tails; reports lock conflicts; retains staged inputs on all paths. Empty input for a missing archive returns a default result with its path without creating an archive. A failure can occur after some commits already succeeded.

### `writer_options`

[Source, line 162](../../../../../archiver_core/src/mochi_store/write.rs#L162)

```rust
fn writer_options(threads: usize) -> WriterOptions
```

Returns MOCHI writer options with record timestamps enabled and `threads` set to the supplied worker count, leaving other options at defaults.

### `open_or_create_writer`

[Source, line 181](../../../../../archiver_core/src/mochi_store/write.rs#L181)

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

### `new`

[Source, line 264](../../../../../archiver_core/src/mochi_store/write.rs#L264)

```rust
fn new(files: &'a [StoreFile], opts: &StoreOptions) -> Self
```

`Batcher` constructor: stores the input slice and budgets, clamps file budget to at least one, and starts with no cached head or completed directories.

### `next_batch`

[Source, line 278](../../../../../archiver_core/src/mochi_store/write.rs#L278)

```rust
fn next_batch(&mut self, cat: Option<&Catalog>) -> Result<Option<Batch>>
```

Consumes staged files into a transaction until adding another write would exceed the byte or file budget (a batch always takes at least one file). Missing or non-file inputs are skipped; files the head already holds with identical bytes (`already_stored`) become report-only entries. Each written file gets its missing ancestor directories first, then a streaming `OsFile` source with its mtime. Returns `None` once no reportable entries remain; I/O, catalog and path failures propagate.

### `already_stored`

[Source, line 339](../../../../../archiver_core/src/mochi_store/write.rs#L339)

```rust
fn already_stored(
    &self,
    cat: Option<&Catalog>,
    file: &StoreFile,
    path: &ArchivePath,
) -> Result<Option<String>>
```

Resume idempotency check. Only when the archive exists (`cat` and the replayed head are both present): if the head holds a regular file at `path`, hashes the staged file and returns that hash when it equals the stored content hash; otherwise `None`. Catalog failures become path-aware archive errors.

### `missing_dirs`

[Source, line 359](../../../../../archiver_core/src/mochi_store/write.rs#L359)

```rust
fn missing_dirs(&self, path: &ArchivePath) -> Vec<ArchivePath>
```

Returns ancestors missing from both the cached head and directories already scheduled, innermost first. The caller reverses them so parent directories are inserted first.

### `restart`

[Source, line 376](../../../../../archiver_core/src/mochi_store/write.rs#L376)

```rust
fn restart(&mut self)
```

Resets the batch cursor and directory set after losing an archive-creation race. Does not modify staged files.

### `staged_metadata`

[Source, line 384](../../../../../archiver_core/src/mochi_store/write.rs#L384)

```rust
fn staged_metadata(file: &StoreFile) -> Result<Option<fs::Metadata>>
```

Returns metadata for an existing regular staged file. Missing paths and non-files yield `None`; other metadata failures return path-bearing I/O errors.

### `mtime_of`

[Source, line 393](../../../../../archiver_core/src/mochi_store/write.rs#L393)

```rust
fn mtime_of(meta: &fs::Metadata) -> Option<Mtime>
```

Converts filesystem modification time to MOCHI seconds/nanoseconds. Returns `None` for unavailable, pre-epoch, or unrepresentable times.

### `committed`

[Source, line 418](../../../../../archiver_core/src/mochi_store/write.rs#L418)

```rust
fn committed(&mut self, entries: Vec<PendingEntry>, outcome: &CommitOutcome)
```

Appends pending entries with the commit outcome's sequence, records the new sequence, and keeps the first `DirectoryUnconfirmed` reason. Mutates only the in-memory `Commits` accumulator.

### `unchanged`

[Source, line 428](../../../../../archiver_core/src/mochi_store/write.rs#L428)

```rust
fn unchanged(&mut self, entries: Vec<PendingEntry>, seq: u64)
```

Appends report-only entries associated with the current head sequence without recording a new commit. Such sequences mean present by this head, not necessarily introduced there.

### `read_back_entries`

[Source, line 435](../../../../../archiver_core/src/mochi_store/write.rs#L435)

```rust
fn read_back_entries(
    archive_path: &Path,
    cat: &Catalog,
    pending: Vec<(PendingEntry, u64)>,
) -> Result<Vec<StoredEntry>>
```

Replays the final catalog head once and converts pending entries to stored records. Reuses known hashes for unchanged entries; otherwise obtains MOCHI-computed hash/length from the catalog. A missing committed file returns `VerificationFailed`; this metadata pass is not a staged-byte read-back check.
