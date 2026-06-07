//! Shared blob line-similarity used by rename/copy detection.
//!
//! The `diff` command's `-M`/`-C` rename and copy detection and `merge`'s
//! rename tracking both need the same notion of "how similar are these two
//! blobs". This module holds the single canonical implementation so the two
//! commands cannot drift apart.

use std::collections::HashMap;

/// Multiset Sørensen–Dice coefficient over text lines:
/// `2 * common / (|A| + |B|)`, where `common` counts lines present in both
/// inputs respecting multiplicity (a line repeated twice in each side counts
/// twice).
///
/// Returns `1.0` for byte-identical inputs (and for two empty inputs), and
/// `0.0` when either side is binary (contains a NUL byte). This is exactly the
/// algorithm `merge` uses for rename detection, kept here so `diff` and `merge`
/// share one implementation.
pub fn blob_line_similarity(a: &[u8], b: &[u8]) -> f64 {
    if a == b {
        return 1.0;
    }
    if a.contains(&0) || b.contains(&0) {
        return 0.0;
    }
    let a_lines = bytes_to_lines(a);
    let b_lines = bytes_to_lines(b);
    if a_lines.is_empty() && b_lines.is_empty() {
        return 1.0;
    }

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for line in &a_lines {
        *counts.entry(line.as_str()).or_default() += 1;
    }
    let mut common = 0usize;
    for line in &b_lines {
        if let Some(count) = counts.get_mut(line.as_str())
            && *count > 0
        {
            *count -= 1;
            common += 1;
        }
    }

    (2.0 * common as f64) / (a_lines.len() + b_lines.len()) as f64
}

fn bytes_to_lines(data: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(data)
        .lines()
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_blobs_are_fully_similar() {
        assert_eq!(blob_line_similarity(b"a\nb\nc\n", b"a\nb\nc\n"), 1.0);
    }

    #[test]
    fn two_empty_blobs_are_fully_similar() {
        assert_eq!(blob_line_similarity(b"", b""), 1.0);
    }

    #[test]
    fn binary_blob_is_never_similar() {
        assert_eq!(blob_line_similarity(b"a\nb\n", &[0u8, 1, 2]), 0.0);
        assert_eq!(blob_line_similarity(&[0u8, 1, 2], b"a\nb\n"), 0.0);
    }

    #[test]
    fn disjoint_blobs_have_zero_similarity() {
        assert_eq!(blob_line_similarity(b"a\nb\n", b"x\ny\n"), 0.0);
    }

    #[test]
    fn small_edit_keeps_high_similarity() {
        // Three of four lines shared: 2*3 / (4 + 4) = 0.75.
        let a = b"l1\nl2\nl3\nl4\n";
        let b = b"l1\nl2\nl3\nCHANGED\n";
        assert!((blob_line_similarity(a, b) - 0.75).abs() < 1e-9);
    }

    #[test]
    fn multiset_counts_respect_multiplicity() {
        // A has "x" twice; B has "x" once. common = 1 (one match), not 2.
        // 2*1 / (2 + 1) = 0.6666...
        let a = b"x\nx\n";
        let b = b"x\n";
        assert!((blob_line_similarity(a, b) - (2.0 / 3.0)).abs() < 1e-9);
    }
}
