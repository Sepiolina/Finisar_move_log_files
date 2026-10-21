use aho_corasick::AhoCorasick;
use std::collections::HashSet;

/// Delimiters allowed around a valid barcode.
const BOUNDARY_CHARS: [char; 3] = ['_', '.', '-'];

fn is_boundary(c: Option<char>) -> bool {
    match c {
        None => true, // Start or end of string
        Some(c) => BOUNDARY_CHARS.contains(&c),
    }
}

/// A confirmed, boundary-delimited barcode match inside a filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilenameMatch {
    pub barcode: String,
    pub start: usize,
    pub end: usize,
}

/// Reusable Aho-Corasick matcher for searching barcodes across filenames.
pub struct BarcodeMatcher {
    automaton: AhoCorasick,
    barcodes: Vec<String>,
}

impl BarcodeMatcher {
    /// Builds the automaton using standard overlapping matching.
    /// Overlapping search preserves prefix collisions (e.g., `123` vs `12345`)
    /// so the boundary filter can evaluate every candidate independently.
    pub fn new(barcodes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let barcodes: Vec<String> = barcodes.into_iter().map(Into::into).collect();
        let automaton = AhoCorasick::builder()
            .match_kind(aho_corasick::MatchKind::Standard)
            .build(&barcodes)
            .expect("Invalid barcode patterns");
        Self { automaton, barcodes }
    }

    /// Returns all boundary-delimited barcode matches found in `filename`.
    pub fn find_matches(&self, filename: &str) -> Vec<FilenameMatch> {
        let chars: Vec<char> = filename.chars().collect();
        let mut results = Vec::new();

        for m in self.automaton.find_overlapping_iter(filename) {
            let barcode = &self.barcodes[m.pattern().as_usize()];
            let start_byte = m.start();
            let end_byte = m.end();

            // Convert byte offsets to character indices for UTF-8 safety
            let start_char = filename[..start_byte].chars().count();
            let end_char = filename[..end_byte].chars().count();

            let before = if start_char == 0 { None } else { chars.get(start_char - 1).copied() };
            let after = chars.get(end_char).copied();

            if is_boundary(before) && is_boundary(after) {
                results.push(FilenameMatch {
                    barcode: barcode.clone(),
                    start: start_byte,
                    end: end_byte,
                });
            }
        }

        results
    }

    /// Returns true if `filename` contains at least one valid barcode match.
    pub fn matches_any(&self, filename: &str) -> bool {
        !self.find_matches(filename).is_empty()
    }

    /// Returns the unique set of matched barcodes in `filename`.
    pub fn matching_barcodes(&self, filename: &str) -> HashSet<String> {
        self.find_matches(filename).into_iter().map(|m| m.barcode).collect()
    }
}

/// Checks case-insensitively if `filename` ends with an allowed extension.
pub fn has_matching_extension(filename: &str, allowed_extensions: &[String]) -> bool {
    if !filename.contains('.') {
        return false;
    }
    let Some(ext) = filename.rsplit('.').next() else {
        return false;
    };
    allowed_extensions.iter().any(|allowed| {
        allowed.trim_start_matches('.').eq_ignore_ascii_case(ext)
    })
}