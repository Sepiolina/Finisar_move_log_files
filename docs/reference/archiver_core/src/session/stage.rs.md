# `archiver_core/src/session/stage.rs`

[Reference index](../../../README.md) · [Source](../../../../../archiver_core/src/session/stage.rs)

Moves matched files into staging. Each batch of manifest lines is synced before any of its files is renamed, so a renamed file is always listed; an existing staged file is never overwritten.

## Functions

### `stage_entries`

[Source, line 23](../../../../../archiver_core/src/session/stage.rs#L23)

```rust
pub(super) fn stage_entries(
    config: &SessionConfig,
    session_id: i64,
    entries: &[StagedManifestEntry],
    io_workers: usize,
    progress: Progress<'_>,
) -> Result<(StagingManifest, usize)>
```

Creates staging and a new manifest. For each 2,000-entry batch, records and syncs manifest lines before creating parent directories once and renaming entries in parallel (`io_workers` threads). Reports progress; collects only successful moves; counts failures and warns with the first failure. Manifest/write/directory errors propagate.

### `stage_one`

[Source, line 101](../../../../../archiver_core/src/session/stage.rs#L101)

```rust
fn stage_one(
    config: &SessionConfig,
    entry: &StagedManifestEntry,
) -> std::result::Result<(), String>
```

Checks whether the staging destination already exists with `symlink_metadata`, then renames the source there. Returns unit or an operator-facing string error. Uses a separate check and rename rather than an atomic no-replace operation; cross-filesystem moves fail.
