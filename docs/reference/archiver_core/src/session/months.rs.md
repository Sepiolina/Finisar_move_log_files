# `archiver_core/src/session/months.rs`

[Reference index](../../../README.md) · [Source](../../../../../archiver_core/src/session/months.rs)

Archives a staged batch month by month: commit and verify on worker threads (every month at once), then index and delete the staged copies on the calling thread, and finally prune staging.

## Functions

### `plan_jobs`

[Source, line 31](../../../../../archiver_core/src/session/months.rs#L31)

```rust
pub(super) fn plan_jobs(
    conn: &Connection,
    config: &SessionConfig,
    manifest: &StagingManifest,
) -> Result<Vec<VolumeJob>>
```

Groups manifest entries by month in sorted order, reads each month's indexed head, keeps only entries whose staged path exists (`entries` and matching `store_files`, in manifest order), and omits empty months.

### `run_jobs`

[Source, line 97](../../../../../archiver_core/src/session/months.rs#L97)

```rust
pub(super) fn run_jobs(
    conn: &mut Connection,
    config: &SessionConfig,
    session_id: i64,
    jobs: &[VolumeJob],
    threads: Threads,
    progress: Progress<'_>,
) -> MonthsOutcome
```

Runs every month at once (up to `threads.cpu`; much of a MOCHI commit runs on one thread, so months side by side keep the cores busy), splitting the CPU threads between them. Worker threads commit and verify; the calling thread receives progress and completion messages, indexes and cleans each successful month, and records failures with staging retained. Returns the archived entries, the breakdown sorted by month, durability notes and failures.

### `record_success`

[Source, line 167](../../../../../archiver_core/src/session/months.rs#L167)

```rust
fn record_success(&mut self, job: &VolumeJob, finished: &FinishedVolume)
```

Adds a finished month to the outcome: its durability note if any, its breakdown row (files and original bytes) and its archived entries.

### `commit_and_verify`

[Source, line 183](../../../../../archiver_core/src/session/months.rs#L183)

```rust
fn commit_and_verify(
    config: &SessionConfig,
    job: &VolumeJob,
    workers: usize,
    say: &dyn Fn(String),
) -> Result<FinishedVolume>
```

Stores one month's files using the per-month thread budget, chooses scoped verification only when the archive's prior head matches the indexed head, verifies before any deletion, reports degraded durability, and rejects a result whose committed entry count is below the expected staged count.

### `index_and_clean`

[Source, line 251](../../../../../archiver_core/src/session/months.rs#L251)

```rust
fn index_and_clean(
    conn: &mut Connection,
    session_id: i64,
    job: &VolumeJob,
    finished: &FinishedVolume,
    io_workers: usize,
    progress: Progress<'_>,
) -> Result<()>
```

Builds a path lookup from the month's entries, writes the finished volume and entries to SQLite in one transaction, then removes every staged copy in parallel (`io_workers` threads). Progress is reported throughout. Index or deletion errors are returned.

### `cleanup_staging`

[Source, line 306](../../../../../archiver_core/src/session/months.rs#L306)

```rust
pub(super) fn cleanup_staging(config: &SessionConfig, progress: Progress<'_>) -> Result<usize>
```

Removes the manifest, prunes empty staging directories, and returns the number of remaining unlisted paths. Remaining files are not deleted; a warning names the first one.

### `remove_file_if_present`

[Source, line 322](../../../../../archiver_core/src/session/months.rs#L322)

```rust
fn remove_file_if_present(path: &Path) -> Result<()>
```

Removes a file and tolerates `NotFound`; other deletion errors retain path context.

### `prune_empty_dirs`

[Source, line 332](../../../../../archiver_core/src/session/months.rs#L332)

```rust
fn prune_empty_dirs(dir: &Path, left: &mut Vec<PathBuf>)
```

Recursively visits directories bottom-up, appends remaining non-directory paths to `left`, and attempts empty-directory removal. Silently ignores unreadable directories, failed directory entries, and removal failures; never intentionally deletes files.
