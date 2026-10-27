use aho_corasick::AhoCorasick;
use std::collections::HashSet;

/// True if `c` may sit next to a barcode: start/end of the filename, or
/// one of the delimiters `_`, `.`, `-`.
fn is_boundary(c: Option<char>) -> bool {
    matches!(c, None | Some('_' | '.' | '-'))
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
        Self {
            automaton,
            barcodes,
        }
    }

    /// Returns all boundary-delimited barcode matches found in `filename`.
    pub fn find_matches(&self, filename: &str) -> Vec<FilenameMatch> {
        let mut results = Vec::new();

        for m in self.automaton.find_overlapping_iter(filename) {
            // A match of a non-empty UTF-8 pattern starts and ends on char
            // boundaries, so these are the whole chars around it.
            let before = filename[..m.start()].chars().next_back();
            let after = filename[m.end()..].chars().next();

            if is_boundary(before) && is_boundary(after) {
                results.push(FilenameMatch {
                    barcode: self.barcodes[m.pattern().as_usize()].clone(),
                    start: m.start(),
                    end: m.end(),
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
        self.find_matches(filename)
            .into_iter()
            .map(|m| m.barcode)
            .collect()
    }
}

/// Checks case-insensitively (ASCII) whether the text after the last `.`
/// in `filename` equals one of `allowed_extensions` (a leading `.` on an
/// allowed entry is ignored). A name without a `.` never matches.
pub fn has_matching_extension(filename: &str, allowed_extensions: &[String]) -> bool {
    let Some((_, ext)) = filename.rsplit_once('.') else {
        return false;
    };
    allowed_extensions
        .iter()
        .any(|allowed| allowed.trim_start_matches('.').eq_ignore_ascii_case(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_are_checked_on_whole_chars() {
        let m = BarcodeMatcher::new(["123", "12345"]);
        assert_eq!(m.matching_barcodes("é123ü").len(), 0);
        assert!(m.matches_any("é_123-ü"));
        assert_eq!(m.find_matches("ab_123.log")[0].start, 3);
        let both = m.matching_barcodes("123_12345");
        assert!(both.contains("123") && both.contains("12345"));
        assert!(!m.matches_any("x12345"));
    }

    #[test]
    fn extension_edge_cases() {
        let allowed = vec!["txt".to_string(), ".zip".to_string(), String::new()];
        assert!(has_matching_extension("a.b.TXT", &allowed));
        assert!(has_matching_extension("a.ZIP", &allowed));
        assert!(!has_matching_extension("noext", &allowed));
        // Trailing dot: empty extension, matched only by an empty entry.
        assert!(has_matching_extension("trailing.", &allowed));
        assert!(!has_matching_extension("trailing.", &allowed[..2]));
        // Non-ASCII extensions compare exactly (ASCII-only case folding).
        assert!(!has_matching_extension("f.TXTé", &allowed));
    }
}
