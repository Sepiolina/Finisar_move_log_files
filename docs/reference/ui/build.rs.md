# `ui/build.rs`

[Reference index](../README.md) · [Source](../../../ui/build.rs)

Cargo build script for generating the Tauri application build context.

## Functions

### `main`

[Source, line 1](../../../ui/build.rs#L1)

```rust
fn main()
```

Calls `tauri_build::build()` during Cargo's build-script phase to process Tauri configuration/resources. No application runtime behavior is implemented here.
