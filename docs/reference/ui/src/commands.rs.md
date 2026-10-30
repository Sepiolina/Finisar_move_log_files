# `ui/src/commands.rs`

[Reference index](../../README.md) · [Source](../../../../ui/src/commands.rs)

Tauri IPC command layer. `AppState` owns mutex-protected preferences, encrypted-config paths, and an atomic busy flag. `InitialState` returns preferences, decrypted DB settings, saved status, and optional notice; `RunRequest` uses camelCase fields. Long-running commands hold `BusyGuard` for the whole operation and return errors as strings.

## Functions

### `drop`

[Source, line 34](../../../../ui/src/commands.rs#L34)

```rust
fn drop(&mut self)
```

`BusyGuard` destructor: clears the atomic busy flag even when a guarded command returns an error or unwinds.

### `new`

[Source, line 40](../../../../ui/src/commands.rs#L40)

```rust
pub fn new(settings: UiSettings, secure_paths: SecureConfigPaths) -> Self
```

Constructs `AppState` with supplied preferences/config paths and a clear busy flag.

### `begin`

[Source, line 45](../../../../ui/src/commands.rs#L45)

```rust
fn begin(&self) -> Result<BusyGuard<'_>, String>
```

Atomically sets busy and returns a guard. Returns `Another task is already running.` if already set; the existing task remains responsible for clearing it.

### `settings`

[Source, line 52](../../../../ui/src/commands.rs#L52)

```rust
fn settings(&self) -> UiSettings
```

Locks the settings mutex and returns a cloned snapshot. A poisoned mutex panics through `unwrap`.

### `blocking`

[Source, line 58](../../../../ui/src/commands.rs#L58)

```rust
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> archiver_core::Result<T> + Send + 'static,
) -> Result<T, String>
```

Runs a core-result-producing closure on Tauri's blocking pool, awaits it, and converts both task-join and core errors into strings.

### `notify`

[Source, line 67](../../../../ui/src/commands.rs#L67)

```rust
fn notify(title: &str, body: &str)
```

Spawns a detached thread to show an OS notification. Notification failures are ignored; callers do not wait for display completion.

### `db_path`

[Source, line 74](../../../../ui/src/commands.rs#L74)

```rust
pub fn db_path(settings: &UiSettings) -> PathBuf
```

Returns the configured index path or relative `log_archiver_index.sqlite` when absent.

### `load_state`

[Source, line 90](../../../../ui/src/commands.rs#L90)

```rust
pub fn load_state(state: State<AppState>) -> InitialState
```

Loads/decrypts saved DB settings and returns them with a settings snapshot. Absence uses defaults; decryption/config errors also use defaults but add a visible notice. This command does not acquire the busy guard.

### `save_settings`

[Source, line 100](../../../../ui/src/commands.rs#L100)

```rust
pub fn save_settings(settings: UiSettings, state: State<AppState>)
```

Persists supplied preferences and replaces the in-memory mutex value. Disk failures are hidden by `UiSettings::save`, so IPC success does not guarantee persistence.

### `save_db_config`

[Source, line 106](../../../../ui/src/commands.rs#L106)

```rust
pub fn save_db_config(config: DbConfig, state: State<AppState>) -> Result<(), String>
```

Encrypts and saves the supplied `DbConfig`, returning stringified failures. Does not test SQL connectivity or acquire the long-task busy guard.

### `clear_db_config`

[Source, line 111](../../../../ui/src/commands.rs#L111)

```rust
pub fn clear_db_config(state: State<AppState>) -> Result<(), String>
```

Deletes encrypted settings and their key, returning stringified failures. Does not alter other preferences.

### `run_session`

[Source, line 128](../../../../ui/src/commands.rs#L128)

```rust
pub async fn run_session(req: RunRequest, app: AppHandle, state: State<'_, AppState>) -> Result<RunResult, String>
```

Acquires the busy guard, requires saved SQL settings and source/output paths, builds `RunInput` with two-day lookback, automatic workers, and default staging `<output>/.staging`, then awaits the pipeline. A nonempty To date enables the manual window. On real success saves the cached end date; on success/failure dispatches a desktop notification. Email is enabled only with its toggle and nonempty address fields.

### `find_locations`

[Source, line 184](../../../../ui/src/commands.rs#L184)

```rust
fn find_locations(conn: &rusqlite::Connection, query: &str) -> archiver_core::Result<Vec<LogEntryLocation>>
```

Searches exact barcode first and returns those rows when nonempty; otherwise runs filename/path SQL LIKE search. The two search modes are alternatives, not merged.

### `search`

[Source, line 193](../../../../ui/src/commands.rs#L193)

```rust
pub async fn search(query: String, app: AppHandle, state: State<'_, AppState>) -> Result<Vec<LogEntryLocation>, String>
```

Acquires busy, opens/initializes the index on the blocking pool, emits index/search progress, and returns matching locations. Zero matches returns an empty array.

### `extract`

[Source, line 208](../../../../ui/src/commands.rs#L208)

```rust
pub async fn extract(query: String, dest_dir: PathBuf, app: AppHandle, state: State<'_, AppState>) -> Result<usize, String>
```

Acquires busy, searches through the index, and extracts all returned locations to `dest_dir` on the blocking pool. Returns the number extracted and shows a completion notification; earlier written files remain if a later extraction fails.

### `list_volume`

[Source, line 227](../../../../ui/src/commands.rs#L227)

```rust
pub async fn list_volume(volume_path: PathBuf, app: AppHandle, state: State<'_, AppState>) -> Result<Vec<reader::VolumeEntry>, String>
```

Acquires busy and lists current-head files from a supplied MOCHI path without the index. Emits open/read progress and count log messages, returning `VolumeEntry` rows.

### `preview_entry`

[Source, line 247](../../../../ui/src/commands.rs#L247)

```rust
pub async fn preview_entry(volume_path: PathBuf, relative_path: String, app: AppHandle, state: State<'_, AppState>) -> Result<Preview, String>
```

Acquires busy and loads at most 512 KiB from the selected archive entry. Returns lossy UTF-8 `text` plus a `truncated` flag, emitting open/read progress.

### `extract_entry`

[Source, line 260](../../../../ui/src/commands.rs#L260)

```rust
pub async fn extract_entry(
    volume_path: PathBuf,
    relative_path: String,
    dest_dir: PathBuf,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PathBuf, String>
```

Acquires busy, validates/joins the selected archive-relative path below the destination, and extracts one file on the blocking pool. Returns its destination `PathBuf` and logs the write.

### `extract_all`

[Source, line 282](../../../../ui/src/commands.rs#L282)

```rust
pub async fn extract_all(volume_path: PathBuf, dest_dir: PathBuf, app: AppHandle, state: State<'_, AppState>) -> Result<usize, String>
```

Acquires busy and extracts every current-head file under the destination with per-file log messages. Returns the count and shows a completion notification after success.
