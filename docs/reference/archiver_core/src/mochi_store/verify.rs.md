# `archiver_core/src/mochi_store/verify.rs`

[Reference index](../../../README.md) · [Source](../../../../../archiver_core/src/mochi_store/verify.rs)

Proves a month's staged files are in its archive before staging is deleted. The structural check (MOCHI: new commits since a known head, or the whole archive at `ContentIntegrity`) and the per-file check (present, hash equal to the staged file, full verified read-back) run at the same time, each on its own read-only handle. The per-file check opens one head and hashes staged files in parallel while it opens.

## Functions

### `verify_volume`

[Source, line 51](../../../../../archiver_core/src/mochi_store/verify.rs#L51)

```rust
pub fn verify_volume(archive_path: &Path, files: &[StoreFile]) -> Result<()>
```

Convenience wrapper over `verify_volume_with` with no earlier head (whole-archive structural check), one hashing thread and no progress. Discards the returned head.

### `verify_volume_with`

[Source, line 65](../../../../../archiver_core/src/mochi_store/verify.rs#L65)

```rust
pub fn verify_volume_with(
    archive_path: &Path,
    files: &[StoreFile],
    since: Option<&VerifiedHead>,
    workers: usize,
    on_progress: impl FnMut(usize),
) -> Result<VerifiedHead>
```

Runs the structural check (`check_structure`) on a scoped thread while the per-file check (`check_files`) runs on the calling thread, each with its own read-only handle. Both must pass and, when there are files, must have seen the same head; otherwise `VerificationFailed` ("the archive head changed while it was verified"). Progress callbacks run on the calling thread. Returns the head the structural check verified.

### `check_files`

[Source, line 92](../../../../../archiver_core/src/mochi_store/verify.rs#L92)

```rust
fn check_files(
    archive_path: &Path,
    files: &[StoreFile],
    workers: usize,
    mut on_progress: impl FnMut(usize),
) -> Result<Option<VerifiedHead>>
```

Returns `Ok(None)` for no files. Otherwise opens and replays **one** read-only head on a scoped thread while the staged files are hashed in parallel (`par::map`, `workers` threads); then, on the calling thread, requires for every file in order: present in the head, a regular file, catalog content hash equal to the staged hash, and a full `read::read_version` of the stored copy hashing to the same value. Reports progress about every 250 ms and once at the end; returns the head it checked. One head per check because several heads opened at once slow each other down far more than linearly.

### `check_structure`

[Source, line 184](../../../../../archiver_core/src/mochi_store/verify.rs#L184)

```rust
fn check_structure(archive_path: &Path, since: Option<&VerifiedHead>) -> Result<VerifiedHead>
```

Opens a read-only storage handle and dispatches to `check_new_commits` when `since` is given, otherwise to `check_whole_archive`. Returns the head that check verified.

### `check_new_commits`

[Source, line 194](../../../../../archiver_core/src/mochi_store/verify.rs#L194)

```rust
fn check_new_commits(
    archive_path: &Path,
    src: &OsReadStorage,
    since: &VerifiedHead,
    ctx: &JobContext<'_>,
) -> Result<VerifiedHead>
```

Runs `verify::verify_commits_since` from `since`. A head that could not be opened is an error; a head equal to `since` passes at once (nothing new). Otherwise the scope status must be `Pass`; any other result becomes `VerificationFailed` with the commit range, chain and new-object statuses, object counts, operational-error flag and up to five findings.

### `check_whole_archive`

[Source, line 228](../../../../../archiver_core/src/mochi_store/verify.rs#L228)

```rust
fn check_whole_archive(
    archive_path: &Path,
    src: &OsReadStorage,
    ctx: &JobContext<'_>,
) -> Result<VerifiedHead>
```

Runs MOCHI verification of the whole archive at `ContentIntegrity` level. Operational error, exit code 1, or any failing dimension becomes `VerificationFailed` with up to five findings (exit code 2 is the normal degraded result in MOCHI 1.0). Returns the verified head, or an error if verification reported none.

### `quote_findings`

[Source, line 265](../../../../../archiver_core/src/mochi_store/verify.rs#L265)

```rust
fn quote_findings(findings: &[impl std::fmt::Debug]) -> String
```

Formats at most five findings with `Debug`, joined by `; `, for error messages.
