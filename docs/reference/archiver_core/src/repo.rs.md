# `archiver_core/src/repo.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/repo.rs)

Typed SQL access for the SQLite index. `SessionStatus` and `BarcodeStatus` mirror schema strings. `LogEntryInsert` carries file/index metadata; serializable `LogEntryLocation` exposes camelCase archive location, optional hash, and commit sequence. Functions use the caller connection; multi-row atomicity is provided explicitly by `record_finished_volume`.

## Functions

### `as_str`

[Source, line 20](../../../../archiver_core/src/repo.rs#L20)

```rust
fn as_str(self) -> &'static str
```

Maps `SessionStatus` to `running`, `success`, or `failed` for storage in SQLite.

### `as_str (definition 2)`

[Source, line 37](../../../../archiver_core/src/repo.rs#L37)

```rust
fn as_str(self) -> &'static str
```

Maps `BarcodeStatus` to `matched`, `no_files_found`, or `error` for storage in SQLite.

### `session_exists`

[Source, line 47](../../../../archiver_core/src/repo.rs#L47)

```rust
pub fn session_exists(conn: &Connection, session_id: i64) -> Result<bool>
```

Returns whether a row with `session_id` exists using `SELECT EXISTS`. Used to recover stale manifest session IDs; SQLite errors propagate.

### `start_session`

[Source, line 57](../../../../archiver_core/src/repo.rs#L57)

```rust
pub fn start_session(
    conn: &Connection,
    started_at: &str,
    query_from_date: &str,
    query_to_date: &str,
) -> Result<i64>
```

Inserts a running session with supplied start label and date bounds, returning `last_insert_rowid`. Does not validate dates or timestamps.

### `complete_session`

[Source, line 77](../../../../archiver_core/src/repo.rs#L77)

```rust
pub fn complete_session(
    conn: &Connection,
    session_id: i64,
    completed_at: &str,
    status: SessionStatus,
    total_barcodes: i64,
    total_files_moved: i64,
    total_errors: i64,
) -> Result<()>
```

Updates the session completion label, status, barcode count, moved-file count, and errors. Returns unit on successful execution, including when no row matches the ID.

### `last_successful_scan_to_date`

[Source, line 104](../../../../archiver_core/src/repo.rs#L104)

```rust
pub fn last_successful_scan_to_date(conn: &Connection) -> Result<Option<String>>
```

Returns the query end of the successful session with the largest ID, or `None` when none exists. Ordering is by ID rather than timestamp.

### `record_session_barcode`

[Source, line 117](../../../../archiver_core/src/repo.rs#L117)

```rust
pub fn record_session_barcode(
    conn: &Connection,
    scan_session_id: i64,
    barcode: &str,
    last_update: &str,
    status: BarcodeStatus,
) -> Result<Option<i64>>
```

Inserts a barcode/update pair with session and match status using a cached statement. Returns `Some(id)` on insertion and `None` for SQLite constraint violations; the code suppresses all `ConstraintViolation` cases, not solely duplicate-key violations.

### `is_barcode_already_scanned`

[Source, line 148](../../../../archiver_core/src/repo.rs#L148)

```rust
pub fn is_barcode_already_scanned(
    conn: &Connection,
    barcode: &str,
    last_update: &str,
) -> Result<bool>
```

Returns whether the exact barcode/update pair exists, independent of its stored status or session. Uses a cached lookup statement.

### `find_volume_by_name`

[Source, line 161](../../../../archiver_core/src/repo.rs#L161)

```rust
pub fn find_volume_by_name(conn: &Connection, volume_name: &str) -> Result<Option<i64>>
```

Looks up a volume ID by month name, returning `None` for no row. The schema does not declare the month name unique; normal callers maintain one row per month.

### `volume_head`

[Source, line 173](../../../../archiver_core/src/repo.rs#L173)

```rust
pub fn volume_head(conn: &Connection, volume_name: &str) -> Result<Option<(String, u64)>>
```

Returns a stored `(commit_id,sequence)` only when both values exist, the ID is nonempty, and the sequence is nonnegative. Otherwise returns `None`; SQLite errors propagate.

### `upsert_volume`

[Source, line 188](../../../../archiver_core/src/repo.rs#L188)

```rust
pub fn upsert_volume(
    conn: &Connection,
    volume_name: &str,
    finished: &FinishedVolume,
    cumulative_entry_count: i64,
    now: &str,
) -> Result<i64>
```

Finds by month name, then inserts or updates path, archive byte size, head ID/sequence, entry count, and update label. Updates preserve the original creation label. Returns the volume ID; callers supply the cumulative count.

### `existing_entry_count`

[Source, line 237](../../../../archiver_core/src/repo.rs#L237)

```rust
pub fn existing_entry_count(conn: &Connection, file_storage_volume_id: i64) -> Result<i64>
```

Returns a volume row's recorded entry count. An absent ID produces a SQLite no-row error.

### `insert_log_entry`

[Source, line 266](../../../../archiver_core/src/repo.rs#L266)

```rust
pub fn insert_log_entry(conn: &Connection, entry: &LogEntryInsert) -> Result<i64>
```

Inserts metadata or updates the existing row on `(relative_path,file_storage_volume_id)` conflict, returning its ID. Repeated indexing is idempotent and newer versions replace the row metadata; prior archive versions remain in MOCHI.

### `location_from_row`

[Source, line 309](../../../../archiver_core/src/repo.rs#L309)

```rust
fn location_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LogEntryLocation>
```

Decodes the selected path, volume path, optional hash, and optional signed SQLite sequence into `LogEntryLocation`. Sequence values are cast to `u64` without validation here.

### `find_locations`

[Source, line 319](../../../../archiver_core/src/repo.rs#L319)

```rust
fn find_locations(
    conn: &Connection,
    where_clause: &str,
    param: &str,
) -> Result<Vec<LogEntryLocation>>
```

Executes the shared join of log entries and storage volumes using an internal WHERE clause and one bound string parameter. Returns decoded locations with no explicit ordering.

### `find_entries_by_barcode`

[Source, line 337](../../../../archiver_core/src/repo.rs#L337)

```rust
pub fn find_entries_by_barcode(conn: &Connection, barcode: &str) -> Result<Vec<LogEntryLocation>>
```

Returns every indexed location with exactly the supplied barcode, or an empty vector. Search values are bound parameters.

### `find_entries_by_filename_like`

[Source, line 342](../../../../archiver_core/src/repo.rs#L342)

```rust
pub fn find_entries_by_filename_like(
    conn: &Connection,
    needle: &str,
) -> Result<Vec<LogEntryLocation>>
```

Returns paths matching SQL `LIKE '%needle%'`. `%` and `_` in `needle` retain wildcard meanings because they are not escaped.

### `record_finished_volume`

[Source, line 350](../../../../archiver_core/src/repo.rs#L350)

```rust
pub fn record_finished_volume(
    conn: &mut Connection,
    scan_session_id: i64,
    volume_name: &str,
    finished: &FinishedVolume,
    now: &str,
    barcode_of: impl Fn(&str) -> Option<String>,
    subfolder_of: impl Fn(&str) -> String,
) -> Result<i64>
```

Uses one SQLite transaction to upsert the volume, map each written entry to barcode/subfolder, upsert mapped entries, and recompute the count from actual rows. Entries whose barcode callback returns `None` are skipped. Commits and returns the volume ID; failure rolls back the transaction. Callers must verify archive content before invoking it.
