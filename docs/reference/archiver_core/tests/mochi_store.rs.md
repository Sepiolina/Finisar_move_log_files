# `archiver_core/tests/mochi_store.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/tests/mochi_store.rs)

Storage integration tests for create/append, byte and file metadata, verification, identical-content resume, locks, and interrupted writes. Uses temporary real MOCHI archives.

## Functions

### `pseudo_random`

[Source, line 14](../../../../archiver_core/tests/mochi_store.rs#L14)

```rust
fn pseudo_random(seed: u64, len: usize) -> Vec<u8>
```

Generates deterministic xorshift-style bytes from a seed and length for poorly compressible fixtures.

### `stage`

[Source, line 27](../../../../archiver_core/tests/mochi_store.rs#L27)

```rust
fn stage(staging: &Path, files: &[(&str, Vec<u8>)]) -> Vec<StoreFile>
```

Writes fixture vectors below a staging directory and returns the corresponding `StoreFile` list.

### `head_seq`

[Source, line 42](../../../../archiver_core/tests/mochi_store.rs#L42)

```rust
fn head_seq(archive: &Path) -> u64
```

Opens a read-only archive head and returns its sequence; fixture failures panic.

### `store_then_list_then_extract_is_byte_identical`

[Source, line 48](../../../../archiver_core/tests/mochi_store.rs#L48)

```rust
fn store_then_list_then_extract_is_byte_identical()
```

Checks creation, sizes, commit ID length, verification, listing, BLAKE3 metadata, and single/all extraction across small, empty, and multi-chunk files.

### `subfolder_directory_entry_exists_and_mtime_is_recorded`

[Source, line 102](../../../../archiver_core/tests/mochi_store.rs#L102)

```rust
fn subfolder_directory_entry_exists_and_mtime_is_recorded()
```

Stores one file and asserts the explicit subfolder directory and file are listed. Despite its name, this test does not assert a modification-time value.

### `append_to_existing_month_adds_a_new_commit_and_keeps_old_files`

[Source, line 125](../../../../archiver_core/tests/mochi_store.rs#L125)

```rust
fn append_to_existing_month_adds_a_new_commit_and_keeps_old_files()
```

Checks append increases sequence and archive size, changes head ID, retains old files, and verifies both batches.

### `large_volume_is_split_into_bounded_commits`

[Source, line 154](../../../../archiver_core/tests/mochi_store.rs#L154)

```rust
fn large_volume_is_split_into_bounded_commits()
```

Uses a 1,000-byte budget to check two-small-file groups, a 5,000-byte file committed alone, a tail file, and exact extraction after four commits.

### `restaging_the_same_file_is_idempotent_for_resume`

[Source, line 200](../../../../archiver_core/tests/mochi_store.rs#L200)

```rust
fn restaging_the_same_file_is_idempotent_for_resume()
```

Stores identical input twice and checks one namespace file remains and verification succeeds.

### `verification_fails_when_the_staged_file_differs_from_the_archive`

[Source, line 212](../../../../archiver_core/tests/mochi_store.rs#L212)

```rust
fn verification_fails_when_the_staged_file_differs_from_the_archive()
```

Changes staged content after storing and checks verification failure; separately checks an unstored path fails verification.

### `bit_flip_inside_the_archive_fails_verification`

[Source, line 230](../../../../archiver_core/tests/mochi_store.rs#L230)

```rust
fn bit_flip_inside_the_archive_fails_verification()
```

Flips a byte near the archive midpoint and checks content verification fails.

### `uncommitted_tail_is_refused_and_never_truncated`

[Source, line 244](../../../../archiver_core/tests/mochi_store.rs#L244)

```rust
fn uncommitted_tail_is_refused_and_never_truncated()
```

Appends junk after a valid commit, checks storing fails, and confirms the archive length is unchanged. Accepts either dedicated tail or generic MOCHI error for arbitrary junk.

### `interrupted_commit_surfaces_uncommitted_tail_and_is_not_truncated`

[Source, line 268](../../../../archiver_core/tests/mochi_store.rs#L268)

```rust
fn interrupted_commit_surfaces_uncommitted_tail_and_is_not_truncated()
```

Cuts the footer of a second commit, expects dedicated `UncommittedTail`, and confirms no automatic truncation.

### `second_writer_gets_a_clear_lock_conflict`

[Source, line 290](../../../../archiver_core/tests/mochi_store.rs#L290)

```rust
fn second_writer_gets_a_clear_lock_conflict()
```

Holds an append writer open, checks a second writer returns `ArchiveLocked`, then closes the first writer and verifies append succeeds.

### `mochi_errors_carry_their_stable_code`

[Source, line 318](../../../../archiver_core/tests/mochi_store.rs#L318)

```rust
fn mochi_errors_carry_their_stable_code()
```

Lists a nonexistent archive and asserts the generic MOCHI error code is `IO_ERROR`.

### `identical_file_already_in_head_is_skipped_but_still_reported_and_verified`

[Source, line 329](../../../../archiver_core/tests/mochi_store.rs#L329)

```rust
fn identical_file_already_in_head_is_skipped_but_still_reported_and_verified()
```

Checks identical re-store makes no commit or byte growth but reports both entries, then changes one file and checks one new commit with updated bytes.

## Running this file

```sh
cargo test -p archiver_core --test mochi_store
```
