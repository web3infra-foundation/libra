//! Pickaxe content filters for `libra log`.
//!
//! Two distinct semantics, matching `git log`:
//! - `-S <string>` (pickaxe): a commit matches when the **number of occurrences**
//!   of the literal `string` differs between a changed file's parent-side and
//!   child-side *full blob* (so a change that leaves the count unchanged does
//!   not match, and a change in an unrelated part of the file is also ignored).
//! - `-G <regex>`: a commit matches when **any added or removed diff line**
//!   matches the regex (line-level; occurrence counts are irrelevant).
//!
//! This module holds only the pure predicates; the per-commit blob/diff loading
//! lives in `command::log` so it can reuse that command's object-store helpers.

use regex::Regex;

/// Counts non-overlapping occurrences of the literal `needle` in `haystack`.
/// An empty needle never matches (returns 0), mirroring git's behavior.
pub fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || needle.len() > haystack.len() {
        return 0;
    }
    let mut count = 0;
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            count += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    count
}

/// Returns whether any added (`+`) or removed (`-`) line in the unified
/// `diff_text` matches `re`. The `+++`/`---` file headers and context lines are
/// ignored; the leading `+`/`-` marker is stripped before matching.
pub fn diff_line_matches(diff_text: &str, re: &Regex) -> bool {
    diff_text.lines().any(|line| {
        let is_added = line.starts_with('+') && !line.starts_with("+++");
        let is_removed = line.starts_with('-') && !line.starts_with("---");
        (is_added || is_removed) && re.is_match(&line[1..])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_occurrences_counts_non_overlapping() {
        assert_eq!(count_occurrences(b"aXbXc", b"X"), 2);
        assert_eq!(count_occurrences(b"secret secret", b"secret"), 2);
        assert_eq!(count_occurrences(b"no match here", b"secret"), 0);
        // Non-overlapping: "aaaa" contains 2 non-overlapping "aa".
        assert_eq!(count_occurrences(b"aaaa", b"aa"), 2);
        assert_eq!(count_occurrences(b"anything", b""), 0);
        assert_eq!(count_occurrences(b"x", b"toolong"), 0);
    }

    #[test]
    fn diff_line_matches_only_changed_lines() {
        let re = Regex::new("debug_flag").unwrap();
        let diff = "--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old line\n+added debug_flag here\n unchanged debug_flag context";
        // Matches the `+added debug_flag` line.
        assert!(diff_line_matches(diff, &re));

        // A context line containing the needle does NOT match (no +/- marker).
        let ctx_only = " a debug_flag context line\n unchanged";
        assert!(!diff_line_matches(ctx_only, &re));

        // The `+++`/`---` headers are ignored even if they contain the pattern.
        let headers = "--- a/debug_flag\n+++ b/debug_flag";
        assert!(!diff_line_matches(headers, &re));
    }
}
