# `archiver_core/src/session/mod.rs`

[Reference index](../../../README.md) · [Source](../../../../../archiver_core/src/session/mod.rs)

One archiving run: resume, scan, stage, archive, record. Defines `SessionConfig`, `SessionResult`, `VolumeBreakdown` and the non-generic `Progress` callback wrapper, and re-exports the manifest types and `validate_subfolder` so `session::*` paths are unchanged.

## Functions

### `report`

[Source, line 89](../../../../../archiver_core/src/session/mod.rs#L89)

```rust
fn report(&self, msg: String, done: usize, total: usize)
```

Invokes the optional progress callback with a message, done count and total; does nothing when no callback is set.

### `say`

[Source, line 96](../../../../../archiver_core/src/session/mod.rs#L96)

```rust
fn say(&self, msg: impl Into<String>)
```

Sends an uncounted progress message through `report` with `(0,0)`.

### `run_session`

[Source, line 102](../../../../../archiver_core/src/session/mod.rs#L102)

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

Validates subfolders, resumes an existing staging manifest first, then starts a running session. Empty candidates complete successfully. Otherwise builds a matcher from all candidates, filters fresh candidates, scans (`scan::scan_all`), records barcode statuses, and either completes with no matches or stages files (`stage::stage_entries`, with the I/O thread count) and archives them (`finish_staged_batch`). Session counters combine scan, staging and archive errors. The generic callback is converted once to the non-generic `Progress` used everywhere inside.

### `io_worker_count`

[Source, line 214](../../../../../archiver_core/src/session/mod.rs#L214)

```rust
fn io_worker_count(requested: usize) -> usize
```

Threads for file-system metadata work (renames into staging, deleting staged copies): twice the CPU worker count, at most 16. Such work waits on the disk far more than it uses a core (measured: deleting 50,000 synced files took 6.3 s on 1 thread, 3.5 s on 4, 2.9 s on 16).

### `already_scanned`

[Source, line 219](../../../../../archiver_core/src/session/mod.rs#L219)

```rust
fn already_scanned<'c>(
    conn: &mut Connection,
    candidates: &'c [ArchivalCandidate],
) -> Result<HashSet<&'c str>>
```

Uses one SQLite read transaction and cached pair lookups to return borrowed barcode strings already recorded at their supplied timestamps. The result is keyed by barcode, not by the complete pair.

### `record_barcode_statuses`

[Source, line 236](../../../../../archiver_core/src/session/mod.rs#L236)

```rust
fn record_barcode_statuses(
    conn: &mut Connection,
    session_id: i64,
    fresh_candidates: &[&ArchivalCandidate],
    matched: &HashSet<&str>,
    progress: Progress<'_>,
) -> Result<()>
```

Writes matched/no-files-found status for fresh candidates inside raw `BEGIN TRANSACTION`/`COMMIT`, reporting every 50,000 rows. Begin/commit failures become `Config`; an intermediate error does not explicitly roll back the raw transaction.

### `finish_staged_batch`

[Source, line 273](../../../../../archiver_core/src/session/mod.rs#L273)

```rust
fn finish_staged_batch(
    conn: &mut Connection,
    config: &SessionConfig,
    manifest: &StagingManifest,
    progress: Progress<'_>,
) -> Result<SessionResult>
```

Uses the manifest's session or creates a recovery session if it was deleted. Plans jobs (`months::plan_jobs`), runs them (`months::run_jobs`) with CPU and I/O thread budgets, returns the first month failure while preserving the manifest, otherwise cleans staging (`months::cleanup_staging`), counts archived files and unique barcodes, completes the session and returns sorted month data and durability notes.

### `started_at_now`

[Source, line 347](../../../../../archiver_core/src/session/mod.rs#L347)

```rust
fn started_at_now() -> String
```

Returns a current Unix timestamp label `epoch:<seconds>`, falling back to zero for pre-epoch system time.

### `finish_staged_batch_recovers_from_orphaned_scan_session_id`

[Source, line 364](../../../../../archiver_core/src/session/mod.rs#L364)

```rust
fn finish_staged_batch_recovers_from_orphaned_scan_session_id()
```

Test: a manifest naming a missing session is archived under a new recovery session (`Progress::NONE`), counts one file and removes the staged copy.
