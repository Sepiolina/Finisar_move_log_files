# `archiver_core/examples/bench_session.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/examples/bench_session.rs)

Synthetic throughput benchmark using an in-memory index and temporary source/archive trees. Invocation: `cargo run --release -p archiver_core --example bench_session -- <files> [months] [dir] [append]`. Defaults: 20,000 files, one month, temp `archiver_bench`, no append. Use a disposable directory: the benchmark recursively deletes its selected root before generating data.

## Functions

### `main`

[Source, line 19](../../../../archiver_core/examples/bench_session.rs#L19)

```rust
fn main()
```

Parses positional arguments, removes/recreates the benchmark root, generates roughly 2 KiB files over five subfolders with four files per barcode, and times a full session. Asserts moved/indexed counts and empty source; optionally adds another batch and asserts its moved count. Prints throughput/progress and removes the root unless `ARCHIVER_BENCH_KEEP` is set. Uses `unwrap`/`expect` and assertions, so setup or integrity failures panic. The month count is only clamped to at least one; use 1–12 for calendar month names.
