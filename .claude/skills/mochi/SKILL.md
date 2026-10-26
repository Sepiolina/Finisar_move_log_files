---
name: mochi
description: Build programs on top of the MOCHI archiver, either the `mochi` CLI (scripts, log shippers, cron jobs) or the `mochi-core` Rust library. Use it for log storage or archival on MOCHI, for code that creates, appends to, lists, reads, searches, verifies, retains, compacts or repairs `.mochi` archives, and for parsing `mochi --json` output or exit codes. Covers the commit model, the single-writer lock, the log-segment layout, retention by rotation, verification policy, wording rules, and what 1.0 does not do.
---

# Building on MOCHI (CLI or `mochi-core`)

MOCHI 1.0 is an append-only archive format. A `.mochi` file holds a chain of
**commits**, and each commit is a full **snapshot** of a namespace of files and
directories. Everything is hash-anchored with BLAKE3. You can drive it two ways:

| Use | When |
|---|---|
| **`mochi` CLI with `--json`** | Shell scripts, cron jobs, programs in any language, log shippers. Stable JSON schemas and exit codes. |
| **`mochi-core` (Rust)** | Rust programs that build content in memory (no staging files), need progress or cancellation, or embed reading and verifying. |

The CLI is a thin layer over `mochi-core`, so both give the same results.

Authoritative references in the MOCHI repo (read them when this file is not
enough): `docs/c14-cli.md` (every command, flag and exit code),
`docs/report-schema-v1.md` and `docs/schemas/json/*.schema.json` (JSON
output), `docs/user/` (user guide), `docs/spec.md` (normative). A working
library example is in `examples/log_segments.rs` next to this file.

## The mental model (get this right first)

1. **One commit per write call.** `create` writes commit 0. Each `append` or
   `ArchiveWriter::commit` adds exactly one commit. Old snapshots stay
   readable (`--snapshot SEQ`) until they are expired *and* the archive is
   rewritten.
2. **A commit replaces whole files.** No call appends bytes to a stored file.
   Putting `a.log` again stores a new version of it. Store logs as
   **immutable, rotated segments**, never as one growing file.
3. **Deleting removes an entry from the new snapshot, not from history.** The
   bytes stay in the archive until a `gc apply` or `compact` rewrite.
4. **Directories are explicit entries.** In the library a file's parent must
   already exist in the head or be put earlier in the same transaction.
   Otherwise validation fails.
5. **CLI inputs become top-level entries** (named after their last path
   component). `mochi append a.mochi /var/log/app/x.log` stores `x.log` at the
   root. To place files under `logs/2026-10-21/`, see the staging-tree recipe
   below.
6. **One writer at a time, fail-fast.** A second writer gets `LOCK_CONFLICT`
   (exit 3) right away; it never waits. Serialize writers yourself (one
   shipper process, a queue, or retry with backoff).
7. **A commit's content is held in memory.** The CLI caps it with
   `--max-memory` (default 2 GiB) and fails with `LIMIT_EXCEEDED` above that,
   so keep each commit well under it. `Transaction::put_file` takes a
   `Vec<u8>`.
8. **`LOCAL_COMMITTED` is the only success status.** It means one local file
   was synced. It is **not** a backup. Keep independent copies.
9. **Paths are bytes.** Components are joined by `/` and are never normalized
   or case-folded. Windows names are WTF-8.
10. **The profile is fixed at creation:** default, `--tar-compatible`,
    `--encrypted`, `--recovery-record N`. You cannot add one later.

## Designing log storage on MOCHI

Recommended layout and policy:

```
logs/<YYYY-MM-DD>/<host>-<app>-<rotation-seq>.log[.ndjson]
```

- **Commit on rotation, not per line.** Each commit costs metadata (about
  0.5 KiB for an empty commit, about 2 KiB with a growing catalog; measured in
  `docs/benchmarks/t32-scaling.md`). Writer time per commit also grows with
  the catalog: 10,000 commits took about 9 minutes in total. Batch by time
  (every 5–60 min) or by size (for example 64 MiB), and put many segments in
  one commit when you can.
- **Rotate archives**, for example one archive per month:
  `logs-2026-10.mochi`. This keeps each catalog small and opening fast, and
  it makes retention trivial: delete a whole old archive file (after you have
  copied it elsewhere if needed). Retention inside one archive (expire, then
  `gc plan`/`gc apply`) works, but it **writes a new archive file** and never
  shrinks in place.
- **Don't compress the logs yourself.** MOCHI stores each chunk as a Zstandard
  frame (chunk size 8 MiB by default). Same-content chunks within an archive
  are deduplicated against the head in the default profile.
- **Search is by path, not content.** `mochi search` matches stored path bytes,
  content hashes and version IDs. Full-text search is not built (exit 4). Put
  what you need to find into the file name (date, host, app), or keep your own
  index outside the archive.
- **Pick the profile at creation.**
  - Default: dedup on. This is usually right for logs.
  - `--recovery-record 5..20`: Reed–Solomon parity. `repair` can rebuild
    damaged bytes from it. It is a repair aid, not a backup.
  - `--encrypted`: names, content and sizes are sealed with a passphrase.
    Every read and write then needs the passphrase. There is no recovery if
    every passphrase is lost. It conflicts with `--tar-compatible`.
  - `--tar-compatible`: every commit also writes a TAR stream that generic
    tools can read. There is no dedup.
- **Verify on a schedule, and record it.** Run `mochi verify ARCHIVE --json`
  (read-only) after closing an archive and periodically after that. Use
  `mochi health ARCHIVE --json` to judge the recorded evidence against a
  policy (by default, verify evidence at most 30 days old).

### Staging-tree recipe (CLI, place files under a directory)

Appending a directory whose name already exists **merges** into it: new files
are added, and existing siblings are kept. So build a staging tree that
mirrors the archive layout and holds only the new segments:

```bash
stage=$(mktemp -d)
mkdir -p "$stage/logs/2026-10-21"
mv /var/log/app/app.log.1 "$stage/logs/2026-10-21/web1-app-000123.log"
mochi --json append logs-2026-10.mochi "$stage/logs"   # adds logs/2026-10-21/web1-app-000123.log
# Remove $stage only after exit code 0 (or 2 with "durability":"directory_unconfirmed").
```

Create the archive the same way: `mochi --json create logs-2026-10.mochi "$stage/logs"`.
`create` refuses an existing file (`DESTINATION_EXISTS`, exit 3) and never
replaces anything, so "create if missing, else append" is safe to race.

## CLI quick reference

Global options go before the subcommand: `--json`, `--state-dir DIR`,
`--no-local-history`, `--passphrase-file PATH`,
`--passphrase-env-for-automation` (reads `MOCHI_PASSPHRASE`), and
`--limit NAME=VALUE`. A passphrase is **never** a command-line argument.

```bash
mochi --json create  A.mochi INPUT...            # commit 0. --tar-compatible | --encrypted | --recovery-record PCT
mochi --json append  A.mochi [INPUT...] [--delete PATH]...   # one commit; deletes first, recursive
mochi --json list    A.mochi [PATH] [--snapshot SEQ]
mochi        get     A.mochi PATH --stdout       # one file's bytes; exit 1 = bytes already written are unverified
mochi --json get     A.mochi [PATH...] -C DIR [--snapshot SEQ]   # verified before each file is published
mochi --json search  A.mochi [PATTERN] [-i] [--under P] [--snapshot HEAD|retained|all|SEQ] [--content-hash B3] [--kind file|dir]
mochi --json snapshot list A.mochi
mochi --json snapshot retain  A.mochi SEQ --label L          # legal hold
mochi --json snapshot expire  A.mochi SEQ... --confirm       # irreversible
mochi --json snapshot release A.mochi --label L --confirm
mochi --json verify  A.mochi [--level structural|referential|stored|content|restoration] [--expected-head ID] [--require-freshness]
mochi --json fsck    A.mochi                     # verify + every commit cross-checked
mochi --json health  A.mochi [--policy P.json]   # judges recorded evidence; checks nothing itself
mochi --json gc plan A.mochi --output plan.json  # read-only
mochi --json gc apply A.mochi --plan plan.json --output NEW.mochi   # writes NEW; source kept
mochi --json compact A.mochi --output NEW.mochi
mochi --json restore-test A.mochi -C NEWDIR
mochi --json repair plan A.mochi [--copy OTHER]... [--output plan.json]
mochi --json repair apply A.mochi --plan plan.json --output NEW.mochi
mochi --json dump-index A.mochi [--table NAME]   # inspection only, never SQL
```

### JSON output

Each `--json` command prints one JSON object on stdout with a `"schema"` tag
(`mochi-cli-commit-v1`, `mochi-cli-list-v1`, `mochi-cli-search-v1`,
`mochi-cli-snapshot-list-v1`, `mochi-cli-restore-v1`, …). `verify`, `fsck`
and `health` print the report schema v1 (`"schema_version": 1`). JSON Schemas
are in `docs/schemas/json/`. Real output:

```json
{"schema":"mochi-cli-commit-v1","command":"append","status":"LOCAL_COMMITTED",
 "archive_id":"b9d9…","seq":1,"commit_id":"074e…","durability":"durable","durability_note":null,
 "added":{"files":1,"directories":0,"bytes":6},"deleted":[],"skipped":[],"truncation":null,
 "encrypted":false,"exit_code":0}

{"schema":"mochi-cli-list-v1","archive_id":"…","seq":1,"commit_id":"…",
 "entries":[{"path":"logs/2026-10-21/app-0001.log","kind":"file","size":6,"content_hash":"b43d…"},
            {"path":"logs","kind":"directory","size":0,"content_hash":null}]}
```

- `content_hash` is plain BLAKE3 of the file, the same value `b3sum` prints.
  Use it for your own integrity cross-checks.
- A name that is not UTF-8 also carries `path_hex` with the exact bytes. Use
  `path_hex` for identity when it is present.
- Errors in `--json` mode are one line on stdout:
  `{"error":{"code":"LOCK_CONFLICT","message":"…"}}`. Branch on `code`,
  never on `message`. Argument-parsing errors are plain text on stderr,
  exit 3.
- In verify reports, **read `policy_result` and `exit_code`**.
  `overall_status` is never `PASS` in 1.0, because searchability and
  retention compliance are `UNSUPPORTED` and durability is `UNKNOWN`.

### Exit codes (all commands)

| Code | Meaning | What your program should do |
|---|---|---|
| 0 | OK | proceed |
| 1 | Integrity or verification failure (`*_INTEGRITY_FAILED`, `NO_VALID_HEAD`, `TAIL_UNRESOLVED`, `FRESHNESS_FAILED`, …) | alert; do not trust or delete sources; plan a repair |
| 2 | Degraded or partial: skipped inputs (symlinks, devices), `directory_unconfirmed` durability (**always on Windows for a new archive**), partial search coverage, partial repair, `UNKNOWN`/`OVERDUE` required evidence | committed data is there; inspect `skipped` / `durability`; "0 hits" from partial coverage proves nothing |
| 3 | Operational error: `LOCK_CONFLICT`, `UNCOMMITTED_TAIL`, `DESTINATION_EXISTS`, `INVALID_ARGUMENT`, `KEY_UNAVAILABLE`, `LIMIT_EXCEEDED`, `IO_ERROR`, `WRITER_POISONED` | retry `LOCK_CONFLICT` with backoff; fix the others; nothing was committed |
| 4 | Unsupported in 1.0 (`UNSUPPORTED_FEATURE`; draft archives, content search, post-1.0 commands) | do not retry |

### Freshness and local state

The CLI keeps the last head it saw per archive in `heads.json` and an evidence
log under the state directory. The default is `$MOCHI_STATE_DIR`, else
`$XDG_STATE_HOME/mochi`, `~/.local/state/mochi` or `%LOCALAPPDATA%\mochi`.
Services should pass `--state-dir` explicitly. Pass `--no-local-history` for
throwaway runs (CI, tests), and then freshness is `UNKNOWN`. To detect a
rolled-back or replaced archive, store the `commit_id` of your last commit and
run `verify --expected-head ID`.

### Interrupted writes

If a process dies mid-commit, nothing is published. The next `append` exits 3
with `UNCOMMITTED_TAIL`. **Never automate truncation.** Alert a human, who
runs `mochi verify`, then `mochi append A.mochi --truncate-tail`. That
command quarantines the tail to a verified `.mochiq` sidecar first. Only an
eligible tail is ever truncated, and refusing is always safe.

## Library (`mochi-core`)

The crates are not published to crates.io. Depend on git or a path:

```toml
[dependencies]
mochi-core = { git = "https://github.com/immerh8/MOCHI", rev = "e790260" }  # 0.9.11
```

The workspace pins Rust 1.91 (minimum 1.89). SQLite is bundled through
`rusqlite`.

### Key API

| Need | Call |
|---|---|
| Path | `catalog::path::ArchivePath::from_components(["logs","2026-10-21","a.log"])`, `from_stored(b"logs/a.log")` |
| Build a commit | `publish::Transaction::new()` then `.put_dir(p, Attributes::default())`, `.put_file(p, Vec<u8>, attrs)`, `.delete(p)` (non-recursive), `.rename(from, to)`, `.at(Mtime)`, `.hold(label, seq)`, `.expire(seq)`, `.release(label)` |
| Create (no-replace) | `ArchiveWriter::create_in(&mut OsDir::containing(path)?, name, Box::new(OsIds), WriterOptions{..}, first_tx, &ctx)` returns `(writer, CommitOutcome)` |
| Append | `ArchiveWriter::open_append_in(&mut dir, name, Box::new(OsIds), opts, TailRepair::Refuse)` then `w.commit(tx, &ctx)` |
| Current namespace | `w.catalog().replay(None)?.get(&path)` (writer) or `read::list(&head, under)` (reader) |
| Release lock | `w.close()?` (dropping also unlocks, but can't report errors) |
| Open for reading | `storage::os::OsReadStorage::open(path)?` then `publish::open_head(&src, &ReadOptions::default())?` |
| A past snapshot | `publish::commit_history(&src, &ro)?` then `open_at_footer(&src, entry.footer_offset, &ro)` |
| Read a file | `read::read_file(&src, &head, &path, &mut impl Write, &ro, &ctx)` (output is unverified until it returns `Ok`) |
| Find | `search::search(&src, &head, SnapshotScope::…, &Query{name: b"web1".to_vec(), ..Default::default()}, &ro, &ctx)`; check `coverage.complete()` |
| Verify | `verify::verify(&src, &VerifyOptions{level: VerificationLevel::Restoration, ..Default::default()}, &ctx)` returns a `Verification{report, head}`; read `report.policy_result`, `report.exit_code` |
| Restore to disk | `restore::restore_selected` (the engine `mochi get` uses) |
| Retention / GC / compaction | `gc::plan`, `compact` (writes a new archive; the source is kept) |
| Encryption | `ReadOptions{keys: Some(keys::KeySession::new(vec![Passphrase::new(text)?])?), ..}` (`Passphrase` is in `mochi_format::secret`); `WriterOptions{read, profile: Some(Profile{encrypted: true, ..}), ..}` |
| Jobs | `job::JobContext{progress: &impl ProgressSink, cancel: &CancellationToken}`; implement `ProgressSink::report(&ProgressEvent)` for progress and call `cancel.cancel()` to stop |
| Errors | `MochiError{code: ErrorCode, message}`; `code.as_str()` is the stable name (`LOCK_CONFLICT`, …); `mochi_cli::exit_code_for(code)` is the CLI's exit mapping |

`CommitOutcome` has `status` (`LOCAL_COMMITTED`), `durability` (`Durable` or
`DirectoryUnconfirmed(why)`, which you report as degraded), `seq`,
`commit_id`, `checkpoint`, `objects_written` and `dedup`.

### Minimal write path (verified against 0.9.6)

```rust
let progress = NullProgress;
let cancel = CancellationToken::new();
let ctx = JobContext { progress: &progress, cancel: &cancel };
let mut dir = OsDir::containing(archive)?;
let opts = WriterOptions { record_time: true, ..WriterOptions::default() };

let (mut w, _) = ArchiveWriter::open_append_in(&mut dir, "logs-2026-10.mochi",
    Box::new(OsIds), opts, TailRepair::Refuse)?;
let head = w.catalog().replay(None)?;
let day = ArchivePath::from_components(["logs", "2026-10-21"])?;
let mut tx = Transaction::new();
if head.get(&day).is_none() { tx.put_dir(day, Attributes::default()); } // "logs" must exist too
tx.put_file(ArchivePath::from_components(["logs", "2026-10-21", "web1-000123.log"])?,
            segment_bytes, Attributes { mtime: Some(Mtime { secs, nanos: 0 }), ..Default::default() });
let outcome = w.commit(tx, &ctx)?;   // one commit, LOCAL_COMMITTED
w.close()?;
```

For the full program (create-or-append, list, read back, verify), see
`examples/log_segments.rs`.

### Library rules

- Use `OsIds` for IDs in production. Never derive IDs from content.
- After any failed sync the writer is poisoned (`WRITER_POISONED`). Drop it
  and reopen; never retry and report success.
- Open storage read-only (`OsReadStorage`) for list, read, search and verify.
- Do not pass `TailRepair::Truncate` automatically (see Interrupted writes).
- Treat archive-supplied names as untrusted. Escape them before printing to
  a terminal or HTML, and never use them as HTML, `href`, `src` or shell
  input. For restore, use `restore::restore_selected`; do not join archive
  paths onto disk paths yourself.
- `WriterOptions.chunk_size`, `zstd_level` and `profile` are fixed at creation.
  Passing a different value on append is refused.

## Wording rules for anything user-facing

- After a commit say **"committed (LOCAL_COMMITTED)"**. Never say "backed up",
  "safe", "secure" or "preserved". Remind users to keep an independent copy.
- After a delete, say the entry was "removed from this snapshot (still in
  history)".
- Never show success styling for `UNKNOWN`, `OVERDUE`, `UNSUPPORTED` or
  `DEGRADED`. A check that was skipped is never "passed".
- A partial repair or partial search is labelled **partial**.
- Removing a passphrase envelope is not revocation. Don't call it that.

## Not available in MOCHI 1.0 (answer UNSUPPORTED, don't build workarounds into the format)

Streaming `put_file` (content is in memory per commit), progress output and
Ctrl-C handling in the CLI, `--into`/`--prefix` for CLI inputs (use the
staging tree), symlinks (skipped, exit 2), full-text content search,
multi-writer or concurrent preparation, split volumes, mount, remote or S3
storage, key files or public-key recipients, signatures, SFX, and the
Preservation profile. Nothing in 1.0 reports `PRESERVED`. Archives from a
pre-1.0 build (draft 2) must go through `mochi migrate A --output NEW` first;
every other command refuses them with exit 4.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `LOCK_CONFLICT` (3) | Another writer is active. Serialize writers or retry with backoff. |
| `UNCOMMITTED_TAIL` (3) | An earlier write was interrupted. Verify, then have a human run `append --truncate-tail`. |
| `DESTINATION_EXISTS` (3) | `create` / `--output` never replaces a file. Pick a new name or append instead. |
| Exit 2 on every Windows `create` | The directory entry durability is unconfirmed (policy in 1.0). The data is committed; report it as degraded. |
| Exit 2 with `skipped` entries | Symlinks, devices, FIFOs or sockets in the input were skipped. |
| `LIMIT_EXCEEDED` on append | The commit is larger than `--max-memory`. Split it into several commits. |
| File landed at the archive root | CLI inputs are top-level. Use the staging-tree recipe. |
| `UNSUPPORTED_FEATURE` (4) on open | A draft archive. Run `mochi migrate`. |
| `KEY_UNAVAILABLE` (3) | Wrong or missing passphrase for an Encrypted archive. This is not a verification result. |
