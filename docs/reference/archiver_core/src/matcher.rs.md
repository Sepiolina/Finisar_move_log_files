# `archiver_core/src/matcher.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/matcher.rs)

Matches candidate barcodes in filenames using case-sensitive, overlapping Aho–Corasick search. `FilenameMatch` carries the barcode and UTF-8 byte offsets. Matching requires a filename edge or `_`, `.`, or `-` on each side.

## Functions

### `is_boundary`

[Source, line 6](../../../../archiver_core/src/matcher.rs#L6)

```rust
fn is_boundary(c: Option<char>) -> bool
```

Returns true for a missing neighboring character or one of `_`, `.`, and `-`; all other characters fail the boundary check.

### `new`

[Source, line 28](../../../../archiver_core/src/matcher.rs#L28)

```rust
pub fn new(barcodes: impl IntoIterator<Item = impl Into<String>>) -> Self
```

Collects barcode patterns and builds a Standard Aho–Corasick automaton supporting overlapping matches. Returns a reusable `BarcodeMatcher`; automaton construction failure panics through `expect`.

### `find_matches`

[Source, line 41](../../../../archiver_core/src/matcher.rs#L41)

```rust
pub fn find_matches(&self, filename: &str) -> Vec<FilenameMatch>
```

Searches every overlapping occurrence in `filename`, checks whole neighboring Unicode characters, and returns boundary-valid `FilenameMatch` records with byte offsets. Does not deduplicate barcode strings.

### `matches_any`

[Source, line 63](../../../../archiver_core/src/matcher.rs#L63)

```rust
pub fn matches_any(&self, filename: &str) -> bool
```

Returns whether `find_matches(filename)` is nonempty. It currently allocates and computes the full match vector rather than stopping after the first match.

### `matching_barcodes`

[Source, line 68](../../../../archiver_core/src/matcher.rs#L68)

```rust
pub fn matching_barcodes(&self, filename: &str) -> HashSet<String>
```

Collects valid matches into a unique `HashSet<String>`. Used by scanning to reject filenames matching more than one distinct barcode.

### `has_matching_extension`

[Source, line 79](../../../../archiver_core/src/matcher.rs#L79)

```rust
pub fn has_matching_extension(filename: &str, allowed_extensions: &[String]) -> bool
```

Checks text after the final dot against allowed extensions using ASCII case-insensitive comparison, ignoring leading dots in allowed entries. A filename without a dot fails; a trailing dot matches only an allowed empty extension.

### `boundaries_are_checked_on_whole_chars`

[Source, line 93](../../../../archiver_core/src/matcher.rs#L93)

```rust
fn boundaries_are_checked_on_whole_chars()
```

Unit test covering Unicode neighbors, byte offsets, overlapping prefix patterns, and rejection of barcodes attached to other letters.

### `extension_edge_cases`

[Source, line 104](../../../../archiver_core/src/matcher.rs#L104)

```rust
fn extension_edge_cases()
```

Unit test covering multiple dots, leading dots in configured extensions, trailing empty extensions, no-dot names, and ASCII-only case folding.
