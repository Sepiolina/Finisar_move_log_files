//! Phase 4 test criteria, verbatim from spec:
//! - feed a mixed filename list including a deliberate substring-collision
//!   case; confirm only the correctly-delimited match hits
//!
//! Plus coverage for the query builder and resume-window date math, which
//! don't require a live SQL Server connection to test.

use archiver_core::db::{build_query, compute_resume_window};
use archiver_core::matcher::{has_matching_extension, BarcodeMatcher};

#[test]
fn substring_collision_case_from_spec() {
    // The exact case called out in the spec: barcode list contains both
    // "1233012-A00" and a filename containing "1233012-A00-FHE0000989"
    // where the REAL target barcode is "1233012-A00-FHE000098" (one
    // digit shorter). A naive substring match would find "1233012-A00"
    // sitting inside "1233012-A00-FHE0000989" — but the character right
    // after that match ('-' is fine, but then 'F' follows with no
    // delimiter before the next segment) needs the boundary check.
    //
    // More precisely: "1233012-A00-FHE000098" is itself a PREFIX of
    // "1233012-A00-FHE0000989" (extra trailing '9'). A naive substring
    // search finds "1233012-A00-FHE000098" starting at position 0, but
    // the character immediately after the match is '9' — not a
    // delimiter — so this must be rejected.
    let barcodes = vec!["1233012-A00-FHE000098".to_string(), "1233012-A00".to_string()];
    let matcher = BarcodeMatcher::new(barcodes);

    let filename = "unit_1233012-A00-FHE0000989_run.txt";
    let matches = matcher.find_matches(filename);

    // "1233012-A00-FHE000098" must NOT match here — it's immediately
    // followed by '9', which is not a boundary character.
    assert!(
        !matches.iter().any(|m| m.barcode == "1233012-A00-FHE000098"),
        "shorter barcode that is a prefix of a longer run must not match without a trailing delimiter, got {matches:?}"
    );

    // "1233012-A00" DOES sit at a valid boundary here: preceded by '_'
    // and followed by '-', both delimiters — so it's a legitimate match
    // even though it's also a prefix of the longer string.
    assert!(
        matches.iter().any(|m| m.barcode == "1233012-A00"),
        "1233012-A00 is flanked by valid delimiters ('_' before, '-' after) and should match, got {matches:?}"
    );
}

#[test]
fn correctly_delimited_full_barcode_matches() {
    let barcodes = vec!["1233012-A00-FHE000098".to_string()];
    let matcher = BarcodeMatcher::new(barcodes);

    // Properly delimited on both sides.
    let filename = "unit_1233012-A00-FHE000098_run.txt";
    let matches = matcher.find_matches(filename);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].barcode, "1233012-A00-FHE000098");
}

#[test]
fn match_at_start_and_end_of_filename_without_underscore() {
    let barcodes = vec!["BC001".to_string()];
    let matcher = BarcodeMatcher::new(barcodes);

    // Barcode at the very start of the filename (no character before it).
    assert!(matcher.matches_any("BC001_log.txt"));
    // Barcode at the very end before the extension dot.
    assert!(matcher.matches_any("prefix_BC001.txt"));
    // Barcode is the entire stem.
    assert!(matcher.matches_any("BC001.txt"));
}

#[test]
fn no_delimiter_on_either_side_does_not_match() {
    let barcodes = vec!["BC001".to_string()];
    let matcher = BarcodeMatcher::new(barcodes);

    // "BC001" appears but is glued to other alphanumerics on both sides.
    assert!(!matcher.matches_any("xBC001x.txt"));
    assert!(!matcher.matches_any("prefixBC0012.txt"));
}

#[test]
fn extension_filter_is_case_insensitive_and_requires_a_dot() {
    let allowed = vec!["txt".to_string(), "zip".to_string()];
    assert!(has_matching_extension("foo.txt", &allowed));
    assert!(has_matching_extension("foo.TXT", &allowed));
    assert!(has_matching_extension("foo.Zip", &allowed));
    assert!(!has_matching_extension("foo.log", &allowed));
    assert!(!has_matching_extension("foo_no_extension", &allowed));
}

#[test]
fn build_query_first_run_has_no_lower_bound() {
    let sql = build_query(None, "2024-06-29");
    assert!(sql.contains("WHERE sft.last_update < '2024-06-29'"));
    assert!(!sql.contains(">="));
    assert!(sql.contains("WITH (NOLOCK)"));
    assert!(sql.contains("ShipmentFinisarSerial"));
    assert!(sql.contains("ShipmentFinisarTrace"));
}

#[test]
fn build_query_resume_run_has_both_bounds() {
    let sql = build_query(Some("2024-06-01"), "2024-06-29");
    assert!(sql.contains("sft.last_update >= '2024-06-01'"));
    assert!(sql.contains("sft.last_update < '2024-06-29'"));
}

#[test]
fn resume_window_first_run_has_no_from_date() {
    let (from, to) = compute_resume_window(None, 2).unwrap();
    assert!(from.is_none());
    // `to` should be a well-formed YYYY-MM-DD string.
    assert_eq!(to.len(), 10);
    assert_eq!(to.chars().nth(4), Some('-'));
    assert_eq!(to.chars().nth(7), Some('-'));
}

#[test]
fn resume_window_subsequent_run_starts_one_day_before_last_scan() {
    let (from, _to) = compute_resume_window(Some("2024-06-15"), 2).unwrap();
    assert_eq!(from.as_deref(), Some("2024-06-14"));
}

#[test]
fn resume_window_handles_month_and_year_boundaries() {
    let (from, _) = compute_resume_window(Some("2024-03-01"), 2).unwrap();
    assert_eq!(from.as_deref(), Some("2024-02-29")); // 2024 is a leap year

    let (from, _) = compute_resume_window(Some("2024-01-01"), 2).unwrap();
    assert_eq!(from.as_deref(), Some("2023-12-31"));
}
