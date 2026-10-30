# `archiver_core/src/session/scan.rs`

[Reference index](../../../README.md) · [Source](../../../../../archiver_core/src/session/scan.rs)

Finds the files to archive: lists each configured subfolder in parallel and keeps files whose name matches exactly one candidate barcode, with the target month from that barcode's date; then drops duplicate paths.

## Functions

### `validate_subfolder`

[Source, line 23](../../../../../archiver_core/src/session/scan.rs#L23)

```rust
pub fn validate_subfolder(name: &str) -> Result<()>
```

Requires a single normal, nonempty folder component, rejecting `.`, `..`, separators, colon, NUL, and surrounding whitespace. Returns `Config` on invalid input before a real session mutates files.

### `volume_name_from_date`

[Source, line 45](../../../../../archiver_core/src/session/scan.rs#L45)

```rust
fn volume_name_from_date(date: &str) -> Result<String>
```

Checks that the date starts with four digits, a hyphen, and two digits, then returns the first seven bytes as month name. Rejects malformed prefixes with `Config`; does not validate month range or the remaining date text.

### `scan_all`

[Source, line 60](../../../../../archiver_core/src/session/scan.rs#L60)

```rust
pub(super) fn scan_all(
    config: &SessionConfig,
    workers: usize,
    matcher: &BarcodeMatcher,
    candidate_date: &HashMap<&str, &str>,
    progress: Progress<'_>,
) -> Scan
```

Scans subfolders in parallel, accumulating seen-entry counts, errors and warnings, then reports the finish message and drops duplicate paths (`dedup_by_path`, counted as errors). Returns a `Scan` with the unique entries in subfolder order, the number of directory entries seen and the error count.

### `scan_subfolder`

[Source, line 118](../../../../../archiver_core/src/session/scan.rs#L118)

```rust
fn scan_subfolder(
    config: &SessionConfig,
    subfolder: &str,
    matcher: &BarcodeMatcher,
    candidate_date: &HashMap<&str, &str>,
    scanned: &AtomicUsize,
) -> SubfolderScan
```

Lists one configured subfolder non-recursively. Missing or unreadable folders return an error count plus warning. Counts every directory entry, skips non-files and disallowed extensions, rejects ambiguous matches and matches whose candidate date is missing or malformed as errors, and records unique barcode matches with their target month and size. Symlinks are not followed.

### `dedup_by_path`

[Source, line 195](../../../../../archiver_core/src/session/scan.rs#L195)

```rust
fn dedup_by_path(entries: &mut Vec<StagedManifestEntry>, progress: Progress<'_>) -> i64
```

Retains only the first entry per relative path, reports the number discarded, and returns that count as additional errors. Mutates the supplied vector.
