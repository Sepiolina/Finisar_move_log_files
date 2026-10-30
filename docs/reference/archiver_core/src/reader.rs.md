# `archiver_core/src/reader.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/reader.rs)

Read-only MOCHI browsing and extraction. `OpenVolume` caches the head snapshot once, avoiding replay per file. `VolumeEntry` contains path/size; `ExtractedFile` contains archive path, destination, and bytes written. Extraction reads the current archive head, not the historical `commit_seq` or hash stored in an index location.

## Functions

### `open`

[Source, line 33](../../../../archiver_core/src/reader.rs#L33)

```rust
fn open(volume_path: &Path) -> Result<Self>
```

Opens a volume read-only, opens its head, and replays its namespace once into an `OpenVolume`. Returns path-aware archive errors on failure.

### `map`

[Source, line 48](../../../../archiver_core/src/reader.rs#L48)

```rust
fn map(&self, e: MochiError) -> ArchiveError
```

Adds this opened volume's disk path to a MOCHI error through `ArchiveError::from_mochi`.

### `read_file`

[Source, line 54](../../../../archiver_core/src/reader.rs#L54)

```rust
fn read_file(&self, relative_path: &str, out: &mut dyn Write) -> Result<u64>
```

Resolves an archive-relative path in the cached snapshot and streams the selected version into a `Write` sink. Returns logical length after verified completion; missing paths produce `EntryNotFound` and directories produce a MOCHI invalid-argument error. The sink may receive bytes before verification fails.

### `file_paths`

[Source, line 82](../../../../archiver_core/src/reader.rs#L82)

```rust
fn file_paths(&self) -> Result<impl Iterator<Item = (String, u64)>>
```

Lists head entries, filters out directories, and returns an iterator over path strings and logical sizes. Does not read file contents; paths are decoded with lossy UTF-8 conversion.

### `file_size`

[Source, line 96](../../../../archiver_core/src/reader.rs#L96)

```rust
fn file_size(&self, relative_path: &str) -> Result<Option<u64>>
```

Resolves a file path and reads logical length from its catalog version, returning `None` for absent/non-file entries or missing version metadata. Archive path and catalog errors propagate.

### `safe_join`

[Source, line 117](../../../../archiver_core/src/reader.rs#L117)

```rust
pub fn safe_join(root: &Path, relative_path: &str) -> Result<PathBuf>
```

Validates slash-separated components and joins them below `root`. Rejects empty paths/components, `.`, `..`, and components interpreted as non-normal by the host platform. This is lexical validation, not symlink containment; Windows drive/backslash interpretation depends on the host OS.

### `write_entry_to`

[Source, line 139](../../../../archiver_core/src/reader.rs#L139)

```rust
fn write_entry_to(vol: &OpenVolume, relative_path: &str, dest_path: &Path) -> Result<u64>
```

Creates parent directories, streams a verified file into the fixed sibling `.<filename>.partial`, flushes it, and renames it to the destination. Attempts to remove the partial file on error. Existing-destination rename behavior depends on the OS; the helper does not explicitly sync the file or directory.

### `list_volume_entries`

[Source, line 174](../../../../archiver_core/src/reader.rs#L174)

```rust
pub fn list_volume_entries(
    volume_path: &Path,
    mut on_progress: impl FnMut(usize),
) -> Result<Vec<VolumeEntry>>
```

Returns files and logical sizes without reading their content. Calls `on_progress` every 500 files and at the end, including a possible repeated final count at an exact multiple of 500.

### `extract_entry_from_volume`

[Source, line 194](../../../../archiver_core/src/reader.rs#L194)

```rust
pub fn extract_entry_from_volume(
    volume_path: &Path,
    relative_path: &str,
    dest_path: &Path,
) -> Result<u64>
```

Opens the archive and writes one named entry to the exact caller-supplied destination via `write_entry_to`. Returns bytes written; callers deriving a destination from archive names must call `safe_join` themselves.

### `extract_all_from_volume`

[Source, line 214](../../../../archiver_core/src/reader.rs#L214)

```rust
pub fn extract_all_from_volume(
    volume_path: &Path,
    dest_dir: &Path,
    mut on_progress: impl FnMut(usize),
) -> Result<Vec<ExtractedFile>>
```

Extracts each current-head file under `dest_dir` through `safe_join`, preserving relative layout and returning extraction records. Reports each completed file; aborts on the first error with earlier successful outputs retained.

### `write`

[Source, line 242](../../../../archiver_core/src/reader.rs#L242)

```rust
fn write(&mut self, data: &[u8]) -> std::io::Result<usize>
```

`BoundedSink` implementation: appends input up to remaining capacity. When a write exceeds the limit, saves the available prefix, sets `full`, and returns an I/O error to stop the archive read.

### `flush`

[Source, line 252](../../../../archiver_core/src/reader.rs#L252)

```rust
fn flush(&mut self) -> std::io::Result<()>
```

`BoundedSink` implementation: returns success without work because the sink is in memory.

### `preview_entry_from_volume`

[Source, line 261](../../../../archiver_core/src/reader.rs#L261)

```rust
pub fn preview_entry_from_volume(
    volume_path: &Path,
    relative_path: &str,
    max_bytes: u64,
) -> Result<(Vec<u8>, bool)>
```

Returns `(prefix_bytes,truncated)` with at most `max_bytes` retained. Stops reading when the bounded sink fills; chunks read remain verified, but truncated previews do not complete whole-file verification. Missing entries and errors before the intentional limit abort propagate.

### `extract_entry_to_bytes`

[Source, line 284](../../../../archiver_core/src/reader.rs#L284)

```rust
pub fn extract_entry_to_bytes(volume_path: &Path, relative_path: &str) -> Result<Vec<u8>>
```

Reads and verifies one complete current-head entry into a `Vec<u8>`. Returns all bytes; memory grows with the file size.

### `extract_by_barcode`

[Source, line 293](../../../../archiver_core/src/reader.rs#L293)

```rust
pub fn extract_by_barcode(
    conn: &rusqlite::Connection,
    barcode: &str,
    dest_dir: &Path,
) -> Result<Vec<ExtractedFile>>
```

Queries exact barcode locations from SQLite and delegates to `extract_locations`. No matches yields an empty vector rather than `EntryNotFound`.

### `extract_locations`

[Source, line 303](../../../../archiver_core/src/reader.rs#L303)

```rust
pub fn extract_locations(
    locations: &[crate::repo::LogEntryLocation],
    dest_dir: &Path,
) -> Result<Vec<ExtractedFile>>
```

Extracts the supplied index locations below `dest_dir`, caching one `OpenVolume` per distinct volume-path string. Validates destinations with `safe_join` and returns completed records; stops on first failure without rolling back earlier files. Locations resolving to the same destination are processed in input order.
