# `ui/src/pipeline.rs`

[Reference index](../../README.md) · [Source](../../../../ui/src/pipeline.rs)

Connects async SQL querying, synchronous archive sessions, progress events, dry-run scanning, and optional email. `RunInput` carries index path, DB/session settings, email, and dry-run flag. `RunResult` is a camelCase tagged object with `kind: archive` and flattened `SessionResult`, or `kind: dryrun` with matched counts and filenames. `Reporter` wraps the Tauri app handle.

## Functions

### `emit`

[Source, line 32](../../../../ui/src/pipeline.rs#L32)

```rust
pub fn emit(&self, step: Option<&str>, message: impl Into<String>, done: usize, total: usize)
```

Sends a `progress` event carrying `{step,message,done,total}` to Tauri listeners. `step` may be absent; event-delivery errors are ignored.

### `step`

[Source, line 37](../../../../ui/src/pipeline.rs#L37)

```rust
pub fn step(&self, step: &str, message: impl Into<String>)
```

Emits a named flow step and message with zero counters.

### `log`

[Source, line 41](../../../../ui/src/pipeline.rs#L41)

```rust
pub fn log(&self, message: impl Into<String>)
```

Emits a log message with no step change and zero counters.

### `step_for`

[Source, line 47](../../../../ui/src/pipeline.rs#L47)

```rust
fn step_for(message: &str) -> Option<&'static str>
```

Finds the first matching substring among seven rules to map core messages to scan/stage/archive/cleanup steps. Returns `None` for unmatched text; the UI keeps its current node.

### `run`

[Source, line 85](../../../../ui/src/pipeline.rs#L85)

```rust
pub async fn run(input: RunInput, r: Reporter) -> Result<RunResult, String>
```

Opens/initializes SQLite, computes or accepts the date window, and awaits SQL candidate retrieval. Dry runs then scan on the blocking pool and return without a core session or email. Real runs execute the core on the blocking pool, build statistics, await optional best-effort email, and return an archive result or the core error. Early index/window/query/join failures return before report/email; elapsed archive duration excludes email sending.

### `session_stats`

[Source, line 142](../../../../ui/src/pipeline.rs#L142)

```rust
fn session_stats(
    started_label: String,
    duration_secs: f64,
    db_records_retrieved: usize,
    run: &archiver_core::Result<SessionResult>,
) -> SessionStats
```

Maps a successful `SessionResult` plus duration/query count to email fields. On a core failure, sets session counters/lists to zero/empty and includes the error text, even if some months succeeded. `total_files_matched` currently receives `barcodes_matched`, so it counts distinct barcodes rather than files.

### `dry_run_scan`

[Source, line 166](../../../../ui/src/pipeline.rs#L166)

```rust
fn dry_run_scan(cfg: &SessionConfig, barcodes: Vec<String>, r: &Reporter) -> RunResult
```

Sequentially scans configured subfolders for allowed-extension files matching one distinct barcode, logging unreadable folders and progress every 5,000 entries. Returns counts and all matched filenames without staging. It does not use the real scanner's subfolder validation, date-to-month checks, or path deduplication, and `Path::is_file` follows symlinks. Index creation/querying have already occurred in `run`.

### `send_email`

[Source, line 211](../../../../ui/src/pipeline.rs#L211)

```rust
async fn send_email(config: Option<&EmailConfig>, stats: &SessionStats, r: &Reporter)
```

Returns immediately without configuration; otherwise builds the default subject and awaits relay delivery, logging success or failure. Errors are deliberately swallowed so they do not change the archive result; network time is still awaited.

### `epoch_secs`

[Source, line 221](../../../../ui/src/pipeline.rs#L221)

```rust
fn epoch_secs() -> u64
```

Returns current whole seconds since the Unix epoch, using zero if system time predates it.
