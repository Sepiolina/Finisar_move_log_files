# `archiver_core/src/session.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/session.rs)

Coordinates scan → manifest → rename into staging → monthly commit → verification → SQLite indexing → staged-file deletion. Manifest structures persist session ID and each file’s relative path, subfolder, barcode, size, and archive month. `SessionConfig` supplies paths, filters, window settings, and workers. `SessionResult` reports counts, touched volumes, per-volume breakdown, unmatched barcodes, and durability notes. SQL querying and date-window selection are performed by the caller.

## Functions

### `path_in`

[Source, line 60](../../../../archiver_core/src/session.rs#L60)

```rust
fn path_in(staging_dir: &Path) -> PathBuf
```

Returns `<staging_dir>/manifest.jsonl` without filesystem access.

### `read_if_exists`

[Source, line 67](../../../../archiver_core/src/session.rs#L67)

```rust
pub fn read_if_exists(staging_dir: &Path) -> Result<Option<Self>>
```

Reads a JSONL manifest and returns `Some` only with a valid first-line session header. Missing/empty/invalid-header files yield `None`; blank and malformed subsequent lines are skipped, including corrupt middle lines. Read/UTF-8 failures propagate as I/O errors.

### `create`

[Source, line 113](../../../../archiver_core/src/session.rs#L113)

```rust
pub fn create(staging_dir: &Path, scan_session_id: i64) -> Result<Self>
```

Creates/truncates the manifest in an existing staging directory, writes the session header, then flushes and syncs it. Returns `StagingManifestWriter` or serialization/I/O errors.

### `record`

[Source, line 126](../../../../archiver_core/src/session.rs#L126)

```rust
pub fn record(&mut self, entry: StagedManifestEntry) -> Result<()>
```

Serializes one file entry, increments the pending counter, and automatically flushes at 2,000 entries. Recording alone does not move the file; the orchestration explicitly syncs each batch before renames.

### `write_line`

[Source, line 135](../../../../archiver_core/src/session.rs#L135)

```rust
fn write_line(&mut self, line: &ManifestLine) -> Result<()>
```

Serializes a `ManifestLine` as compact JSON and appends a newline to the buffered writer. Returns configuration errors for serialization and I/O errors for writes.

### `flush`

[Source, line 143](../../../../archiver_core/src/session.rs#L143)

```rust
pub fn flush(&mut self) -> Result<()>
```

Flushes the buffer, calls the manifest file's `sync_data`, and resets the pending counter. Any failure aborts before resetting the count.

### `finish`

[Source, line 156](../../../../archiver_core/src/session.rs#L156)

```rust
pub fn finish(mut self) -> Result<()>
```

Consumes the writer after a final flush/sync. Does not remove the manifest.

### `report`

[Source, line 209](../../../../archiver_core/src/session.rs#L209)

```rust
fn report(&self, msg: String, done: usize, total: usize)
```

Calls the optional progress callback with message, done, and total. No callback means no delivery; the message has already been constructed.

### `say`

[Source, line 216](../../../../archiver_core/src/session.rs#L216)

```rust
fn say(&self, msg: impl Into<String>)
```

Sends an uncounted progress message through `report` with `(0,0)`.

### `volume_name_from_date`

[Source, line 222](../../../../archiver_core/src/session.rs#L222)

```rust
fn volume_name_from_date(date: &str) -> Result<String>
```

Checks that the date starts with four digits, a hyphen, and two digits, then returns the first seven bytes as month name. Rejects malformed prefixes with `Config`; does not validate month range or the remaining date text.

### `scan_subfolder`

[Source, line 247](../../../../archiver_core/src/session.rs#L247)

```rust
fn scan_subfolder(
    config: &SessionConfig,
    subfolder: &str,
    matcher: &BarcodeMatcher,
    candidate_date: &HashMap<&str, &str>,
    scanned: &AtomicUsize,
) -> ScanOutcome
```

Nonrecursively lists one source subfolder, counts directory entries, skips symlinks/non-files and disallowed extensions, and keeps filenames matching exactly one candidate barcode with a usable date prefix. Returns matches, error count, and optional folder warning. Metadata failure for a matched entry gives recorded size zero.

### `stage_one`

[Source, line 325](../../../../archiver_core/src/session.rs#L325)

```rust
fn stage_one(
    config: &SessionConfig,
    entry: &StagedManifestEntry,
) -> std::result::Result<(), String>
```

Checks whether the staging destination already exists with `symlink_metadata`, then renames the source there. Returns unit or an operator-facing string error. Uses a separate check and rename rather than an atomic no-replace operation; cross-filesystem moves fail.

### `validate_subfolder`

[Source, line 342](../../../../archiver_core/src/session.rs#L342)

```rust
pub fn validate_subfolder(name: &str) -> Result<()>
```

Requires a single normal, nonempty folder component, rejecting `.`, `..`, separators, colon, NUL, and surrounding whitespace. Returns `Config` on invalid input before a real session mutates files.

### `run_session`

[Source, line 364](../../../../archiver_core/src/session.rs#L364)

```rust
pub fn run_session<F>(
    conn: &mut Connection,
    config: &SessionConfig,
    candidates: Vec<ArchivalCandidate>,
    query_from_date: &str,
    query_to_date: &str,
    started_at: &str,
    progress_callback: Option<&F>,
) -> Result<SessionResult>
where
    F: Fn(String, usize, usize),
```

Validates subfolders, finishes an old manifest first, starts a new SQLite session, scans all supplied candidates, records fresh barcode statuses, stages matches, and finishes the batch. Returns summary counters or an error with recoverable staging retained where applicable. Previously recorded barcodes remain in matching so late files can be found. Fatal paths generally leave the session running; resumed-batch counters are not included in the subsequent fresh session result.

### `already_scanned`

[Source, line 506](../../../../archiver_core/src/session.rs#L506)

```rust
fn already_scanned<'c>(
    conn: &mut Connection,
    candidates: &'c [ArchivalCandidate],
) -> Result<HashSet<&'c str>>
```

Uses one SQLite read transaction and cached pair lookups to return borrowed barcode strings already recorded at their supplied timestamps. The result is keyed by barcode, not by the complete pair.

### `scan_all`

[Source, line 524](../../../../archiver_core/src/session.rs#L524)

```rust
fn scan_all<F>(
    config: &SessionConfig,
    workers: usize,
    matcher: &BarcodeMatcher,
    candidate_date: &HashMap<&str, &str>,
    progress: &Progress<F>,
) -> (Vec<StagedManifestEntry>, usize, i64)
where
    F: Fn(String, usize, usize),
```

Runs subfolder scans through the worker pool, merges entries in configured subfolder order, accumulates entry/error counts, and reports warnings/progress. Recoverable folder errors are counted rather than returned as a fatal result.

### `dedup_by_path`

[Source, line 572](../../../../archiver_core/src/session.rs#L572)

```rust
fn dedup_by_path<F>(entries: &mut Vec<StagedManifestEntry>, progress: &Progress<F>) -> i64
where
    F: Fn(String, usize, usize),
```

Retains only the first entry per relative path, reports the number discarded, and returns that count as additional errors. Mutates the supplied vector.

### `record_barcode_statuses`

[Source, line 587](../../../../archiver_core/src/session.rs#L587)

```rust
fn record_barcode_statuses<F>(
    conn: &mut Connection,
    session_id: i64,
    fresh_candidates: &[&ArchivalCandidate],
    matched_barcode_set: &HashSet<&str>,
    progress: &Progress<F>,
) -> Result<()>
where
    F: Fn(String, usize, usize),
```

Writes matched/no-files-found status for fresh candidates inside raw `BEGIN TRANSACTION`/`COMMIT`, reporting every 50,000 rows. Begin/commit failures become `Config`; an intermediate error does not explicitly roll back the raw transaction.

### `stage_entries`

[Source, line 639](../../../../archiver_core/src/session.rs#L639)

```rust
fn stage_entries<F>(
    config: &SessionConfig,
    session_id: i64,
    entries: &[StagedManifestEntry],
    workers: usize,
    progress: &Progress<F>,
) -> Result<(StagingManifest, usize)>
where
    F: Fn(String, usize, usize),
```

Creates staging and its manifest; for each batch of 2,000, records and syncs entries before parallel renames. Creates destination parents once, collects move failures, and returns an in-memory manifest containing only successful moves plus the failure count. The disk manifest also includes attempted moves; resume skips listed paths absent from staging.

### `finish_staged_batch`

[Source, line 720](../../../../archiver_core/src/session.rs#L720)

```rust
fn finish_staged_batch<F>(
    conn: &mut Connection,
    config: &SessionConfig,
    manifest: &StagingManifest,
    progress_callback: Option<&F>,
) -> Result<SessionResult>
where
    F: Fn(String, usize, usize),
```

Recovers a missing session ID by starting a recovery row, plans month jobs, and commits/verifies/indexes/cleans them. Other months continue when one fails; after jobs finish, the first collected failure is returned and the manifest remains. If all succeed, removes the manifest, prunes empty directories, completes the effective session, and returns batch statistics.

### `plan_jobs`

[Source, line 822](../../../../archiver_core/src/session.rs#L822)

```rust
fn plan_jobs(
    conn: &Connection,
    config: &SessionConfig,
    manifest: &StagingManifest,
) -> Result<Vec<VolumeJob>>
```

Groups manifest entries by month in sorted order, retains paths still present in staging, and loads the previously indexed head per month. Returns jobs only for nonempty months; presence is checked with `exists`, not regular-file validation.

### `run_jobs`

[Source, line 881](../../../../archiver_core/src/session.rs#L881)

```rust
fn run_jobs<F>(
    conn: &mut Connection,
    config: &SessionConfig,
    session_id: i64,
    jobs: &[VolumeJob],
    workers: usize,
    total_to_archive: usize,
    progress: &Progress<F>,
) -> MonthsOutcome
where
    F: Fn(String, usize, usize),
```

Schedules multiple month workers, dividing worker capacity between them. Workers commit/verify and send progress/results over a channel; the calling thread indexes and cleans each successful month, accumulating failures without stopping other jobs. Returns outcomes in completion order.

### `cleanup_staging`

[Source, line 978](../../../../archiver_core/src/session.rs#L978)

```rust
fn cleanup_staging<F>(config: &SessionConfig, progress: &Progress<F>) -> Result<usize>
where
    F: Fn(String, usize, usize),
```

Removes the manifest, prunes empty directories, and counts/reports remaining unlisted files without deleting them. Returns the leftover count; manifest deletion failures propagate.

### `commit_and_verify`

[Source, line 998](../../../../archiver_core/src/session.rs#L998)

```rust
fn commit_and_verify(
    config: &SessionConfig,
    job: &VolumeJob,
    workers: usize,
    say: &dyn Fn(String),
) -> Result<mochi_store::FinishedVolume>
```

Stores one month, selects incremental verification only if the prior archive head matches the indexed head, then verifies staged bytes and archive structure. Reports degraded directory durability and rejects too few reported entries before allowing cleanup. Never writes SQLite or deletes staging.

### `index_and_clean`

[Source, line 1066](../../../../archiver_core/src/session.rs#L1066)

```rust
fn index_and_clean<F>(
    conn: &mut Connection,
    session_id: i64,
    job: &VolumeJob,
    finished: &mochi_store::FinishedVolume,
    workers: usize,
    progress: &Progress<F>,
) -> Result<()>
where
    F: Fn(String, usize, usize),
```

Atomically records the verified volume/entries in SQLite using manifest barcode/subfolder lookup, then deletes that month's staged files in parallel. A deletion error can leave a partially cleaned but fully indexed month for retry; deletion never starts before successful indexing.

### `remove_file_if_present`

[Source, line 1126](../../../../archiver_core/src/session.rs#L1126)

```rust
fn remove_file_if_present(path: &Path) -> Result<()>
```

Removes a file and tolerates `NotFound`; other deletion errors retain path context.

### `prune_empty_dirs`

[Source, line 1136](../../../../archiver_core/src/session.rs#L1136)

```rust
fn prune_empty_dirs(dir: &Path, left: &mut Vec<PathBuf>)
```

Recursively visits directories bottom-up, appends remaining non-directory paths to `left`, and attempts empty-directory removal. Silently ignores unreadable directories, failed directory entries, and removal failures; never intentionally deletes files.

### `started_at_now`

[Source, line 1148](../../../../archiver_core/src/session.rs#L1148)

```rust
fn started_at_now() -> String
```

Returns a current Unix timestamp label `epoch:<seconds>`, falling back to zero for pre-epoch system time.

### `manifest_jsonl_write_then_read_round_trips`

[Source, line 1163](../../../../archiver_core/src/session.rs#L1163)

```rust
fn manifest_jsonl_write_then_read_round_trips()
```

Unit test writing five entries and checking the recovered session ID, count, and endpoint barcodes.

### `manifest_jsonl_skips_truncated_trailing_line`

[Source, line 1193](../../../../archiver_core/src/session.rs#L1193)

```rust
fn manifest_jsonl_skips_truncated_trailing_line()
```

Unit test appending malformed final JSON and checking the previous valid entry survives reading.

### `finish_staged_batch_recovers_from_orphaned_scan_session_id`

[Source, line 1230](../../../../archiver_core/src/session.rs#L1230)

```rust
fn finish_staged_batch_recovers_from_orphaned_scan_session_id()
```

Unit test finishing a staged file whose manifest session ID is absent from SQLite, checking a new session is created and staging is removed after success.
