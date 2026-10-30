# `ui/src/main.rs`

[Reference index](../../README.md) · [Source](../../../../ui/src/main.rs)

Desktop entry point. Declares command, pipeline, and settings modules and hides the console window on Windows. Connects the embedded frontend to the Rust backend.

## Functions

### `main`

[Source, line 13](../../../../ui/src/main.rs#L13)

```rust
fn main()
```

Loads UI settings and, when the index exists and opens successfully, refreshes the cached last-successful date. Resolves encrypted-config paths, installs the dialog plugin, manages `AppState`, registers all eleven commands, and runs Tauri. Startup index refresh failures are ignored; failure to run the app panics.
