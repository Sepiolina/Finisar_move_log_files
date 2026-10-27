use crate::error::{ArchiveError, Result};
use serde::Serialize;
use std::fmt::Write as _;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// The relay rejects bodies of 100 KB or more; keep a safety margin.
const MAX_BODY_BYTES: usize = 95_000;

/// Timeout for each network step (connect, send, read response).
const IO_TIMEOUT: Duration = Duration::from_secs(20);

/// One row of the "Archives Breakdown" table: the same shape as the
/// session's per-volume breakdown.
pub use crate::session::VolumeBreakdown as ArchiveBreakdownRow;

/// Everything the email report needs about one session run. The UI builds
/// it from `session::SessionResult` plus what only it knows (wall-clock
/// duration, raw DB record count).
#[derive(Debug, Clone)]
pub struct SessionStats {
    /// When the run started, for a human reader (local time is fine).
    pub started_label: String,
    pub duration_secs: f64,
    pub db_records_retrieved: usize,
    pub total_source_files_scanned: usize,
    pub total_files_matched: usize,
    pub total_files_archived: usize,
    pub total_files_deleted: usize,
    pub archive_breakdown: Vec<ArchiveBreakdownRow>,
    /// DB candidates with no matching file in any configured subfolder.
    pub unmatched_barcodes: Vec<String>,
    pub errors: usize,
    /// `Some(message)` if the session failed outright rather than
    /// completing (successfully or with recoverable per-file errors).
    pub failed: Option<String>,
}

/// Where to send the report. `api_base_url` is scheme+host+port only
/// (e.g. `http://10.0.0.5:8080`); the `/api/email/send` path is fixed and
/// appended here so a config typo can't send requests to the wrong route.
#[derive(Debug, Clone)]
pub struct EmailConfig {
    pub api_base_url: String,
    pub recipient: String,
}

#[derive(Serialize)]
struct EmailRequest<'a> {
    to: &'a str,
    subject: &'a str,
    body: String,
}

/// Default subject line, carrying the headline numbers so it is useful in
/// an inbox list without opening the email.
pub fn default_subject(stats: &SessionStats) -> String {
    match &stats.failed {
        Some(_) => format!("Log Archiver — Session FAILED ({})", stats.started_label),
        None => format!(
            "Log Archiver — {} archived, {} unmatched, {} errors ({})",
            stats.total_files_archived,
            stats.unmatched_barcodes.len(),
            stats.errors,
            stats.started_label
        ),
    }
}

/// POSTs the statistics report to the configured relay. Returns an error on
/// any transport failure or non-2xx status; callers should treat that as
/// non-fatal to the session being reported on.
///
/// Speaks minimal HTTP/1.1 over plain TCP instead of pulling in an HTTP
/// client: the relay is an internal, unauthenticated `http://host:port`
/// endpoint (no TLS, no path prefix).
pub async fn send_stats_email(
    config: &EmailConfig,
    subject: &str,
    stats: &SessionStats,
) -> Result<()> {
    let body = render_html(stats);
    let (host, port) = parse_host_port(&config.api_base_url)?;
    let addr = format!("{host}:{port}");

    let payload = EmailRequest {
        to: &config.recipient,
        subject,
        body,
    };
    let json_body = serde_json::to_vec(&payload)
        .map_err(|e| ArchiveError::Email(format!("failed to serialize email payload: {e}")))?;

    let mut stream = timeout(IO_TIMEOUT, TcpStream::connect(&addr))
        .await
        .map_err(|_| ArchiveError::Email(format!("timed out connecting to {addr}")))?
        .map_err(|e| ArchiveError::Email(format!("failed to connect to {addr}: {e}")))?;

    let request_head = format!(
        "POST /api/email/send HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n",
        len = json_body.len()
    );

    timeout(IO_TIMEOUT, async {
        stream.write_all(request_head.as_bytes()).await?;
        stream.write_all(&json_body).await?;
        stream.flush().await
    })
    .await
    .map_err(|_| ArchiveError::Email(format!("timed out sending request to {addr}")))?
    .map_err(|e| ArchiveError::Email(format!("failed to send request to {addr}: {e}")))?;

    let mut response = Vec::new();
    timeout(IO_TIMEOUT, stream.read_to_end(&mut response))
        .await
        .map_err(|_| ArchiveError::Email(format!("timed out waiting for response from {addr}")))?
        .map_err(|e| ArchiveError::Email(format!("failed to read response from {addr}: {e}")))?;

    let response_text = String::from_utf8_lossy(&response);
    let status_line = response_text.lines().next().unwrap_or("").to_string();
    let status_code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if !(200..300).contains(&status_code) {
        let snippet: String = response_text.chars().take(500).collect();
        return Err(ArchiveError::Email(format!(
            "email API at {addr}/api/email/send returned {status_line:?}: {snippet}"
        )));
    }

    Ok(())
}

/// Splits an `api_base_url` like `http://10.0.0.5:8080` (or
/// `10.0.0.5:8080` without a scheme) into `(host, port)`. Only IPv4
/// addresses and plain hostnames are supported (no bracketed IPv6, no
/// scheme other than `http://`), matching the relay's own contract.
fn parse_host_port(base: &str) -> Result<(String, u16)> {
    let without_scheme = base
        .trim()
        .trim_end_matches('/')
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    let (host, port_str) = without_scheme.rsplit_once(':').ok_or_else(|| {
        ArchiveError::Email(format!(
            "invalid email API URL {base:?}: expected host:port"
        ))
    })?;

    if host.is_empty() {
        return Err(ArchiveError::Email(format!(
            "invalid email API URL {base:?}: missing host"
        )));
    }
    let port: u16 = port_str.parse().map_err(|_| {
        ArchiveError::Email(format!(
            "invalid email API URL {base:?}: bad port {port_str:?}"
        ))
    })?;

    Ok((host.to_string(), port))
}

/// Renders the HTML report, shrinking the two unbounded sections
/// (archive breakdown, unmatched barcodes) as needed to stay under
/// `MAX_BODY_BYTES`. Everything else in the report is fixed-size.
fn render_html(stats: &SessionStats) -> String {
    let mut row_cap = stats.archive_breakdown.len();
    let mut unmatched_cap = stats.unmatched_barcodes.len();

    let mut html = render_with_caps(stats, row_cap, unmatched_cap);
    while html.len() > MAX_BODY_BYTES && (row_cap > 0 || unmatched_cap > 0) {
        // Halve the larger list first: it is the likeliest cause.
        if unmatched_cap >= row_cap {
            unmatched_cap /= 2;
        } else {
            row_cap /= 2;
        }
        html = render_with_caps(stats, row_cap, unmatched_cap);
    }

    // Last resort (should be unreachable: with both lists collapsed the
    // body is small): hard-truncate on a char boundary rather than ever
    // POSTing an oversized body.
    if html.len() > MAX_BODY_BYTES {
        let mut cut = MAX_BODY_BYTES;
        while !html.is_char_boundary(cut) {
            cut -= 1;
        }
        html.truncate(cut);
        html.push_str("...</body></html>");
    }

    html
}

fn render_with_caps(stats: &SessionStats, row_cap: usize, unmatched_cap: usize) -> String {
    let mut s = String::with_capacity(4096);
    write_report(&mut s, stats, row_cap, unmatched_cap)
        .expect("formatting into a String cannot fail");
    s
}

/// Writes the report, showing at most `row_cap` breakdown rows and
/// `unmatched_cap` unmatched barcodes.
fn write_report(
    s: &mut String,
    stats: &SessionStats,
    row_cap: usize,
    unmatched_cap: usize,
) -> std::fmt::Result {
    s.push_str(
        "<html><body style=\"font-family:Segoe UI,Arial,sans-serif;font-size:13px;color:#222;\">",
    );

    if let Some(err) = &stats.failed {
        write!(
            s,
            "<p style=\"color:#b00020;font-weight:bold;\">Session FAILED: {}</p>",
            escape_html(err)
        )?;
    }

    s.push_str("<h2 style=\"margin-bottom:4px;\">Runtime Statistics</h2>");
    write!(
        s,
        "<p style=\"margin-top:0;color:#555;\">Started: {}</p>",
        escape_html(&stats.started_label)
    )?;

    s.push_str("<table cellspacing=\"0\" cellpadding=\"4\" style=\"border-collapse:collapse;\">");
    let rows: &[(&str, String)] = &[
        (
            "Total Duration",
            format!("{:.2} seconds", stats.duration_secs),
        ),
        (
            "DB Records Retrieved",
            stats.db_records_retrieved.to_string(),
        ),
        (
            "Total Source Files Scanned",
            stats.total_source_files_scanned.to_string(),
        ),
        ("Total Files Matched", stats.total_files_matched.to_string()),
        (
            "Total Files Archived",
            stats.total_files_archived.to_string(),
        ),
        ("Total Files Deleted", stats.total_files_deleted.to_string()),
        ("Errors", stats.errors.to_string()),
    ];
    for (label, value) in rows {
        write!(
            s,
            "<tr><td style=\"color:#555;padding-right:16px;\">{}</td><td style=\"font-weight:bold;\">{}</td></tr>",
            escape_html(label),
            escape_html(value)
        )?;
    }
    s.push_str("</table>");

    s.push_str("<h3 style=\"margin-bottom:4px;\">Archives Breakdown</h3>");
    if stats.archive_breakdown.is_empty() {
        s.push_str("<p style=\"color:#555;\">No volumes were touched this run.</p>");
    } else {
        s.push_str(
            "<table cellspacing=\"0\" cellpadding=\"4\" style=\"border-collapse:collapse;border:1px solid #ddd;\">\
             <tr style=\"background:#f2f2f2;\">\
             <th style=\"text-align:left;border:1px solid #ddd;\">Volume</th>\
             <th style=\"text-align:right;border:1px solid #ddd;\">Files Archived</th>\
             <th style=\"text-align:right;border:1px solid #ddd;\">Size</th>\
             </tr>",
        );
        for row in stats.archive_breakdown.iter().take(row_cap) {
            write!(
                s,
                "<tr><td style=\"border:1px solid #ddd;\">{}</td><td style=\"text-align:right;border:1px solid #ddd;\">{}</td><td style=\"text-align:right;border:1px solid #ddd;\">{}</td></tr>",
                escape_html(&row.volume_name),
                row.files_archived,
                format_bytes(row.total_bytes)
            )?;
        }
        s.push_str("</table>");
        if stats.archive_breakdown.len() > row_cap {
            write!(
                s,
                "<p style=\"color:#555;\">... and {} more volume(s) not shown (see the app for the full list).</p>",
                stats.archive_breakdown.len() - row_cap
            )?;
        }
    }

    s.push_str("<h3 style=\"margin-bottom:4px;\">Unmatched Barcodes</h3>");
    if stats.unmatched_barcodes.is_empty() {
        s.push_str(
            "<p style=\"color:#555;\">None — every DB candidate had at least one matching file.</p>",
        );
    } else {
        write!(
            s,
            "<p style=\"color:#555;margin-bottom:4px;\">{} barcode(s) had no matching file in any configured subfolder this run.</p>",
            stats.unmatched_barcodes.len()
        )?;
        if unmatched_cap > 0 {
            s.push_str("<p style=\"font-family:Consolas,monospace;word-break:break-all;\">");
            let shown: Vec<&str> = stats
                .unmatched_barcodes
                .iter()
                .take(unmatched_cap)
                .map(String::as_str)
                .collect();
            s.push_str(&escape_html(&shown.join(", ")));
            s.push_str("</p>");
        }
        if stats.unmatched_barcodes.len() > unmatched_cap {
            write!(
                s,
                "<p style=\"color:#555;\">... and {} more not shown (see the app for the full list).</p>",
                stats.unmatched_barcodes.len() - unmatched_cap
            )?;
        }
    }

    s.push_str("</body></html>");
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_stats() -> SessionStats {
        SessionStats {
            started_label: "2026-08-13 10:00:00".to_string(),
            duration_secs: 782.70,
            db_records_retrieved: 217_447,
            total_source_files_scanned: 187_600,
            total_files_matched: 186_029,
            total_files_archived: 186_029,
            total_files_deleted: 186_029,
            archive_breakdown: Vec::new(),
            unmatched_barcodes: Vec::new(),
            errors: 0,
            failed: None,
        }
    }

    #[test]
    fn renders_under_size_budget_with_huge_unmatched_list() {
        let mut stats = base_stats();
        stats.unmatched_barcodes = (0..500_000).map(|i| format!("BC{i:08}")).collect();
        stats.archive_breakdown = (0..5000)
            .map(|i| ArchiveBreakdownRow {
                volume_name: format!("2026-{:02}", (i % 12) + 1),
                files_archived: i,
                total_bytes: i as u64 * 1024,
            })
            .collect();

        let html = render_html(&stats);
        assert!(
            html.len() < MAX_BODY_BYTES + 200,
            "body was {} bytes",
            html.len()
        );
        assert!(html.starts_with("<html>"));
    }

    #[test]
    fn small_report_stays_intact() {
        let mut stats = base_stats();
        stats.unmatched_barcodes = vec!["BC00000001".to_string()];
        stats.archive_breakdown = vec![ArchiveBreakdownRow {
            volume_name: "2026-08".to_string(),
            files_archived: 186_029,
            total_bytes: 12_345_678,
        }];
        let html = render_html(&stats);
        assert!(html.contains("BC00000001"));
        assert!(html.contains("2026-08"));
        assert!(html.contains("782.70 seconds"));
    }

    #[test]
    fn escapes_html_in_barcodes() {
        let mut stats = base_stats();
        stats.unmatched_barcodes = vec!["<script>alert(1)</script>".to_string()];
        let html = render_html(&stats);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
