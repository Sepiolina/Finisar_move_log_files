# `archiver_core/src/error.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/error.rs)

Shared error vocabulary and the crate-wide `Result<T>` alias. `ArchiveError` distinguishes filesystem, SQLite, SQL Server, configuration, extraction, email, and MOCHI failures. MOCHI lock and interrupted-tail errors carry archive paths; other MOCHI errors retain stable codes.

## Functions

### `from`

[Source, line 66](../../../../archiver_core/src/error.rs#L66)

```rust
fn from(e: mochi_core::MochiError) -> Self
```

Converts a `MochiError` into `ArchiveError::Mochi`, preserving its stable code and message. This trait conversion does not specialize lock or tail failures; use `from_mochi` when an archive path is available.

### `from_mochi`

[Source, line 77](../../../../archiver_core/src/error.rs#L77)

```rust
pub fn from_mochi(path: &Path, e: mochi_core::MochiError) -> Self
```

Accepts an archive path and MOCHI failure; returns `ArchiveLocked` for `LOCK_CONFLICT`, `UncommittedTail` for `UNCOMMITTED_TAIL`, or the generic conversion otherwise. Performs no repair or filesystem mutation.

### `io`

[Source, line 92](../../../../archiver_core/src/error.rs#L92)

```rust
pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self
```

Wraps an I/O error with `Some(path)` so callers can identify the affected file. Returns an error value without performing I/O.
