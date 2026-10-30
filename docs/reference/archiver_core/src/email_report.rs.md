# `archiver_core/src/email_report.rs`

[Reference index](../../README.md) · [Source](../../../../archiver_core/src/email_report.rs)

Builds a session statistics email and posts JSON to `/api/email/send`. `SessionStats` combines core counters, timing, query count, optional failure text, per-volume rows, and unmatched barcodes. `EmailConfig` contains relay address and recipient. The transport is raw plaintext HTTP over TCP with separate 20-second connect/send/read timeouts.

## Functions

### `default_subject`

[Source, line 59](../../../../archiver_core/src/email_report.rs#L59)

```rust
pub fn default_subject(stats: &SessionStats) -> String
```

Returns a failure subject when `stats.failed` is present; otherwise includes archived files, unmatched count, errors, and start label. Does not send mail.

### `send_stats_email`

[Source, line 79](../../../../archiver_core/src/email_report.rs#L79)

```rust
pub async fn send_stats_email(
    config: &EmailConfig,
    subject: &str,
    stats: &SessionStats,
) -> Result<()>
```

Renders HTML, parses host/port, serializes `{to,subject,body}`, and sends one HTTP/1.1 POST with `Connection: close`. Reads to EOF and accepts only a parsed 2xx status; timeout, transport, serialization, and non-2xx responses return `Email`. Sending is awaited by the UI pipeline, though failure does not fail the archive session.

### `parse_host_port`

[Source, line 147](../../../../archiver_core/src/email_report.rs#L147)

```rust
fn parse_host_port(base: &str) -> Result<(String, u16)>
```

Trims whitespace/trailing slashes, strips `http://` or `https://`, splits at the last colon, and parses a `u16` port. Missing host/port or invalid port returns `Email`. Stripping `https://` does not enable TLS; this is a simple parser rather than full URL validation.

### `render_html`

[Source, line 177](../../../../archiver_core/src/email_report.rs#L177)

```rust
fn render_html(stats: &SessionStats) -> String
```

Renders with full list caps, then repeatedly halves the larger archive-row/barcode cap until the body fits the 95,000-byte target. As a fallback, truncates at a UTF-8 boundary and appends closing markup; that suffix can exceed the target slightly and truncation may cut an HTML element.

### `render_with_caps`

[Source, line 207](../../../../archiver_core/src/email_report.rs#L207)

```rust
fn render_with_caps(stats: &SessionStats, row_cap: usize, unmatched_cap: usize) -> String
```

Returns report HTML with explicit row and barcode caps using a preallocated string. Uses `expect` because formatting into a `String` is treated as infallible.

### `write_report`

[Source, line 216](../../../../archiver_core/src/email_report.rs#L216)

```rust
fn write_report(
    s: &mut String,
    stats: &SessionStats,
    row_cap: usize,
    unmatched_cap: usize,
) -> std::fmt::Result
```

Appends the failure banner, runtime counters, archive table, and unmatched list to a supplied string. Displays omission counts when caps hide rows and escapes dynamic labels/content. Returns the formatting result.

### `format_bytes`

[Source, line 339](../../../../archiver_core/src/email_report.rs#L339)

```rust
fn format_bytes(bytes: u64) -> String
```

Formats bytes with binary 1,024 scaling across B/KB/MB/GB/TB. Bytes remain integral; scaled units use two decimal places.

### `escape_html`

[Source, line 354](../../../../archiver_core/src/email_report.rs#L354)

```rust
fn escape_html(s: &str) -> String
```

Returns a string escaping ampersand, angle brackets, double quote, and apostrophe in that order. Used for dynamic report content.

### `base_stats`

[Source, line 366](../../../../archiver_core/src/email_report.rs#L366)

```rust
fn base_stats() -> SessionStats
```

Test fixture returning representative counters and timing with empty archive/unmatched lists and no failure.

### `renders_under_size_budget_with_huge_unmatched_list`

[Source, line 383](../../../../archiver_core/src/email_report.rs#L383)

```rust
fn renders_under_size_budget_with_huge_unmatched_list()
```

Unit test rendering 500,000 unmatched barcodes and 5,000 archive rows, checking the resulting body is near the configured budget and starts with HTML.

### `small_report_stays_intact`

[Source, line 404](../../../../archiver_core/src/email_report.rs#L404)

```rust
fn small_report_stays_intact()
```

Unit test verifying a small report retains its barcode, volume name, and formatted duration.

### `escapes_html_in_barcodes`

[Source, line 419](../../../../archiver_core/src/email_report.rs#L419)

```rust
fn escapes_html_in_barcodes()
```

Unit test ensuring script-like barcode content is escaped instead of emitted as markup.
