# File-by-file and function-by-function reference

This reference covers every tracked application, test, example, configuration, and supporting file in Log Archiver. Function pages follow source order and include public APIs, private methods, nested named helpers, trait methods, and test functions. Signatures show inputs and return types; each entry explains behavior, side effects, and relevant error handling. Anonymous callbacks are explained with their enclosing function, with frontend event callbacks described separately. Macro-generated and dependency functions are outside this repository's reference.

Use the existing [project README](../../README.md) for architecture and operations. Paths in the table below identify the exact documented source file; each page links back to it. Source line links describe this documentation snapshot and can shift after later edits.

## Execution and recovery

1. `ui/src/main.rs` loads settings and registers IPC commands.
2. `commands::run_session` validates required UI state and builds pipeline input.
3. `pipeline::run` opens SQLite, chooses a date window, and queries SQL Server.
4. `session::run_session` resumes an interrupted batch before scanning new candidates, then records/syncs manifest lines before staging renames.
5. Monthly workers write and verify archives. The index-owning thread records each verified month transactionally before removing its staged copies.
6. Successful completion removes the manifest and empty directories; failures preserve recoverable staged data. Optional email is awaited and its failure is logged without changing the core result.

A committed local archive is not an independent backup. Missing/corrupt manifest entries, external file changes, filesystem behavior, and OS-dependent path/rename rules must be assessed using the specific function contracts; the reference does not promise recovery beyond those implementations.

## File index

| Source file | Reference | Named functions |
| --- | --- | ---: |
| `.claude/skills/mochi/SKILL.md` | [Read](.claude/skills/mochi/SKILL.md.md) | 0 |
| `.github/workflows/build.yml` | [Read](.github/workflows/build.yml.md) | 0 |
| `.gitignore` | [Read](.gitignore.md) | 0 |
| `Cargo.lock` | [Read](Cargo.lock.md) | 0 |
| `Cargo.toml` | [Read](Cargo.toml.md) | 0 |
| `LICENSE` | [Read](LICENSE.md) | 0 |
| `README.md` | [Read](README.md.md) | 0 |
| `archiver_core/Cargo.toml` | [Read](archiver_core/Cargo.toml.md) | 0 |
| `archiver_core/examples/bench_session.rs` | [Read](archiver_core/examples/bench_session.rs.md) | 1 |
| `archiver_core/src/db.rs` | [Read](archiver_core/src/db.rs.md) | 17 |
| `archiver_core/src/email_report.rs` | [Read](archiver_core/src/email_report.rs.md) | 12 |
| `archiver_core/src/error.rs` | [Read](archiver_core/src/error.rs.md) | 3 |
| `archiver_core/src/lib.rs` | [Read](archiver_core/src/lib.rs.md) | 0 |
| `archiver_core/src/matcher.rs` | [Read](archiver_core/src/matcher.rs.md) | 8 |
| `archiver_core/src/mochi_store/mod.rs` | [Read](archiver_core/src/mochi_store/mod.rs.md) | 10 |
| `archiver_core/src/mochi_store/verify.rs` | [Read](archiver_core/src/mochi_store/verify.rs.md) | 7 |
| `archiver_core/src/mochi_store/write.rs` | [Read](archiver_core/src/mochi_store/write.rs.md) | 15 |
| `archiver_core/src/par.rs` | [Read](archiver_core/src/par.rs.md) | 3 |
| `archiver_core/src/reader.rs` | [Read](archiver_core/src/reader.rs.md) | 16 |
| `archiver_core/src/repo.rs` | [Read](archiver_core/src/repo.rs.md) | 18 |
| `archiver_core/src/schema.rs` | [Read](archiver_core/src/schema.rs.md) | 4 |
| `archiver_core/src/secure_config.rs` | [Read](archiver_core/src/secure_config.rs.md) | 8 |
| `archiver_core/src/session/manifest.rs` | [Read](archiver_core/src/session/manifest.rs.md) | 10 |
| `archiver_core/src/session/mod.rs` | [Read](archiver_core/src/session/mod.rs.md) | 9 |
| `archiver_core/src/session/months.rs` | [Read](archiver_core/src/session/months.rs.md) | 8 |
| `archiver_core/src/session/scan.rs` | [Read](archiver_core/src/session/scan.rs.md) | 5 |
| `archiver_core/src/session/stage.rs` | [Read](archiver_core/src/session/stage.rs.md) | 2 |
| `archiver_core/tests/large_batch_integrity.rs` | [Read](archiver_core/tests/large_batch_integrity.rs.md) | 7 |
| `archiver_core/tests/mochi_extraction.rs` | [Read](archiver_core/tests/mochi_extraction.rs.md) | 8 |
| `archiver_core/tests/mochi_file_viewer.rs` | [Read](archiver_core/tests/mochi_file_viewer.rs.md) | 7 |
| `archiver_core/tests/mochi_index.rs` | [Read](archiver_core/tests/mochi_index.rs.md) | 13 |
| `archiver_core/tests/mochi_session_flow.rs` | [Read](archiver_core/tests/mochi_session_flow.rs.md) | 14 |
| `archiver_core/tests/mochi_store.rs` | [Read](archiver_core/tests/mochi_store.rs.md) | 15 |
| `archiver_core/tests/phase4_db_and_matching.rs` | [Read](archiver_core/tests/phase4_db_and_matching.rs.md) | 10 |
| `archiver_core/tests/secure_config.rs` | [Read](archiver_core/tests/secure_config.rs.md) | 9 |
| `ui/Cargo.toml` | [Read](ui/Cargo.toml.md) | 0 |
| `ui/build.rs` | [Read](ui/build.rs.md) | 1 |
| `ui/capabilities/default.json` | [Read](ui/capabilities/default.json.md) | 0 |
| `ui/frontend/app.js` | [Read](ui/frontend/app.js.md) | 43 |
| `ui/frontend/index.html` | [Read](ui/frontend/index.html.md) | 0 |
| `ui/frontend/styles.css` | [Read](ui/frontend/styles.css.md) | 0 |
| `ui/icons/icon.ico` | [Read](ui/icons/icon.ico.md) | 0 |
| `ui/icons/icon.png` | [Read](ui/icons/icon.png.md) | 0 |
| `ui/src/commands.rs` | [Read](ui/src/commands.rs.md) | 19 |
| `ui/src/main.rs` | [Read](ui/src/main.rs.md) | 1 |
| `ui/src/pipeline.rs` | [Read](ui/src/pipeline.rs.md) | 9 |
| `ui/src/settings.rs` | [Read](ui/src/settings.rs.md) | 9 |
| `ui/tauri.conf.json` | [Read](ui/tauri.conf.json.md) | 0 |

This snapshot documents **42 source/supporting files** and **314 named function definitions**, including tests and named frontend helpers.

## Validation and maintenance

For documentation changes, check file/function coverage and local Markdown links. No runtime code changes are required to use this reference. For application changes, the repository CI runs:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

SQL query construction is covered by tests, while live SQL Server behavior requires an accessible configured server. Linux UI builds require Tauri/WebKit system dependencies. Avoid using the destructive synthetic benchmark against any directory containing real data.

When adding or changing a function, update its file page and retain source order. For new tracked files, add an index entry and either function contracts or an explanation that the file contains no functions. Do not edit Cargo.lock merely to update documentation.
