# `archiver_core/src/par.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/par.rs)

Small scoped worker pool used for scanning and per-file operations. `TICK_EVERY` is 250 ms; progress callbacks execute on the calling thread.

## Functions

### `worker_count`

[Source, line 13](../../../../archiver_core/src/par.rs#L13)

```rust
pub fn worker_count(requested: usize) -> usize
```

Returns any explicit positive request unchanged. For zero, uses available parallelism with fallback 4 and clamps the automatic result to 1–8 workers.

### `map`

[Source, line 27](../../../../archiver_core/src/par.rs#L27)

```rust
pub fn map<T, R>(
    items: &[T],
    workers: usize,
    work: impl Fn(&T) -> R + Sync,
    mut tick: impl FnMut(usize),
) -> Vec<R>
where
    T: Sync,
    R: Send,
```

Runs `work` over a borrowed slice with bounded scoped threads and returns results in input order. Calls `tick(done)` roughly every 250 ms and once at completion, including empty input. Worker panics propagate; returned error values are collected rather than short-circuiting work.

### `map_keeps_input_order_and_reports_completion`

[Source, line 97](../../../../archiver_core/src/par.rs#L97)

```rust
fn map_keeps_input_order_and_reports_completion()
```

Unit test checking ordered transformed results with four workers, final completion count, and empty-input behavior.
