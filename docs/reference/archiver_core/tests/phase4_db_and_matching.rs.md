# `archiver_core/tests/phase4_db_and_matching.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/tests/phase4_db_and_matching.rs)

Pure matcher, query-string, and resume-window tests. No live SQL Server connection is used.

## Functions

### `substring_collision_case_from_spec`

[Source, line 12](../../../../archiver_core/tests/phase4_db_and_matching.rs#L12)

```rust
fn substring_collision_case_from_spec()
```

Checks a barcode missing the final digit of a filename segment is rejected while a shorter prefix followed by a hyphen is accepted as boundary-delimited.

### `correctly_delimited_full_barcode_matches`

[Source, line 49](../../../../archiver_core/tests/phase4_db_and_matching.rs#L49)

```rust
fn correctly_delimited_full_barcode_matches()
```

Checks one correctly delimited full barcode produces exactly one match.

### `match_at_start_and_end_of_filename_without_underscore`

[Source, line 61](../../../../archiver_core/tests/phase4_db_and_matching.rs#L61)

```rust
fn match_at_start_and_end_of_filename_without_underscore()
```

Checks filename-start and extension-dot boundaries, including a barcode forming the whole stem.

### `no_delimiter_on_either_side_does_not_match`

[Source, line 74](../../../../archiver_core/tests/phase4_db_and_matching.rs#L74)

```rust
fn no_delimiter_on_either_side_does_not_match()
```

Checks barcodes attached to letters/digits fail boundary validation.

### `extension_filter_is_case_insensitive_and_requires_a_dot`

[Source, line 84](../../../../archiver_core/tests/phase4_db_and_matching.rs#L84)

```rust
fn extension_filter_is_case_insensitive_and_requires_a_dot()
```

Checks allowed extension case variants, rejection of a disallowed extension, and rejection of filenames with no dot.

### `build_query_first_run_has_no_lower_bound`

[Source, line 94](../../../../archiver_core/tests/phase4_db_and_matching.rs#L94)

```rust
fn build_query_first_run_has_no_lower_bound()
```

Checks first-run SQL has only the exclusive upper bound, expected join tables, and NOLOCK hints.

### `build_query_resume_run_has_both_bounds`

[Source, line 104](../../../../archiver_core/tests/phase4_db_and_matching.rs#L104)

```rust
fn build_query_resume_run_has_both_bounds()
```

Checks resumed SQL contains inclusive lower and exclusive upper date bounds.

### `resume_window_first_run_has_no_from_date`

[Source, line 111](../../../../archiver_core/tests/phase4_db_and_matching.rs#L111)

```rust
fn resume_window_first_run_has_no_from_date()
```

Checks a first run has no lower bound and a date-shaped upper bound; does not assert an exact current date.

### `resume_window_subsequent_run_starts_one_day_before_last_scan`

[Source, line 121](../../../../archiver_core/tests/phase4_db_and_matching.rs#L121)

```rust
fn resume_window_subsequent_run_starts_one_day_before_last_scan()
```

Checks the previous June 15 query end yields June 14 as the next lower bound.

### `resume_window_handles_month_and_year_boundaries`

[Source, line 127](../../../../archiver_core/tests/phase4_db_and_matching.rs#L127)

```rust
fn resume_window_handles_month_and_year_boundaries()
```

Checks one-day overlap across leap-day and year transitions.

## Running this file

```sh
cargo test -p archiver_core --test phase4_db_and_matching
```
