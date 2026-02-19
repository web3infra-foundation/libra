//! Fuzzy sequence matching for patch application.
//!
//! Attempt to find the sequence of `pattern` lines within `lines` beginning at or after `start`.
//! Returns the starting index of the match or `None` if not found. Matches are attempted with
//! decreasing strictness: exact match, then ignoring trailing whitespace, then ignoring leading
//! and trailing whitespace. When `eof` is true, we first try starting at the end-of-file (so that
//! patterns intended to match file endings are applied at the end), and fall back to searching
//! from `start` if needed.
//!
//! Special cases handled defensively:
//! • Empty `pattern` → returns `Some(start)` (no-op match)
//! • `pattern.len() > lines.len()` → returns `None` (cannot match, avoids
//!   out‑of‑bounds panic that occurred pre‑2025‑04‑12)

pub(crate) fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }

    // When the pattern is longer than the available input there is no possible
    // match. Early‑return to avoid the out‑of‑bounds slice that would occur in
    // the search loops below (previously caused a panic when
    // `pattern.len() > lines.len()`).
    if pattern.len() > lines.len() {
        return None;
    }
    let search_start = if eof && lines.len() >= pattern.len() {
        lines.len() - pattern.len()
    } else {
        start
    };
    // Exact match first.
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }
    // Then rstrip match.
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if lines[i + p_idx].trim_end() != pat.trim_end() {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }
    // Finally, trim both sides to allow more lenience.
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if lines[i + p_idx].trim() != pat.trim() {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    // ------------------------------------------------------------------
    // Final, most permissive pass – attempt to match after *normalising*
    // common Unicode punctuation to their ASCII equivalents so that diffs
    // authored with plain ASCII characters can still be applied to source
    // files that contain typographic dashes / quotes, etc.  This mirrors the
    // fuzzy behaviour of `git apply` which ignores minor byte-level
    // differences when locating context lines.
    // ------------------------------------------------------------------

    fn normalise(s: &str) -> String {
        s.trim()
            .chars()
            .map(|c| match c {
                // Various dash / hyphen code-points → ASCII '-'
                '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
                | '\u{2212}' => '-',
                // Fancy single quotes → '\''
                '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
                // Fancy double quotes → '"'
                '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
                // Non-breaking space and other odd spaces → normal space
                '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
                | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
                | '\u{3000}' => ' ',
                other => other,
            })
            .collect::<String>()
    }

    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if normalise(&lines[i + p_idx]) != normalise(pat) {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    // ------------------------------------------------------------------
    // Fifth pass: strip the read_file "L{n}: " line-number prefix from
    // pattern lines before comparing. This handles the common model error
    // of copying context lines verbatim from `read_file` output (which
    // prefixes every line with "L{n}: ") directly into the patch. Only the
    // prefix is stripped — the actual file content is matched as-is.
    // ------------------------------------------------------------------

    fn strip_line_number_prefix(s: &str) -> Option<&str> {
        // Matches `L` + digits + `:` and an optional single space.
        let bytes = s.as_bytes();
        if bytes.first() != Some(&b'L') {
            return None;
        }
        let mut i = 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i <= 1 || bytes.get(i) != Some(&b':') {
            return None;
        }
        let mut j = i + 1;
        if bytes.get(j) == Some(&b' ') {
            j += 1;
        }
        Some(&s[j..])
    }

    fn strip_line_number_prefix_optional_space(s: &str) -> &str {
        if let Some(stripped) = strip_line_number_prefix(s) {
            return stripped;
        }
        if let Some(rest) = s.strip_prefix(' ')
            && let Some(stripped) = strip_line_number_prefix(rest)
        {
            return stripped;
        }
        s
    }

    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            let stripped = strip_line_number_prefix_optional_space(pat).trim();
            if lines[i + p_idx].trim() != stripped {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use std::string::ToString;

    use super::seek_sequence;

    fn to_vec(strings: &[&str]) -> Vec<String> {
        strings.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn test_exact_match_finds_sequence() {
        let lines = to_vec(&["foo", "bar", "baz"]);
        let pattern = to_vec(&["bar", "baz"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
    }

    #[test]
    fn test_rstrip_match_ignores_trailing_whitespace() {
        let lines = to_vec(&["foo   ", "bar\t\t"]);
        // Pattern omits trailing whitespace.
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn test_trim_match_ignores_leading_and_trailing_whitespace() {
        let lines = to_vec(&["    foo   ", "   bar\t"]);
        // Pattern omits any additional whitespace.
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn test_pattern_longer_than_input_returns_none() {
        let lines = to_vec(&["just one line"]);
        let pattern = to_vec(&["too", "many", "lines"]);
        // Should not panic – must return None when pattern cannot possibly fit.
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), None);
    }

    #[test]
    fn test_line_number_prefix_stripped_single_line() {
        // Pattern line includes the L{n}: prefix from read_file output.
        let lines = to_vec(&["## Libra"]);
        let pattern = to_vec(&["L1: ## Libra"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn test_line_number_prefix_stripped_multi_digit() {
        let lines = to_vec(&["fn main() {"]);
        let pattern = to_vec(&["L123: fn main() {"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn test_line_number_prefix_stripped_sequence() {
        // Multi-line pattern where every line has an L{n}: prefix.
        let lines = to_vec(&["## Libra", "", "`Libra` is a partial implementation"]);
        let pattern = to_vec(&[
            "L1: ## Libra",
            "L2: ",
            "L3: `Libra` is a partial implementation",
        ]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn test_no_false_positive_for_literal_l_prefix() {
        // Lines that genuinely contain "L1:" as content should still match via
        // the earlier exact-match pass; the stripping pass should not corrupt them.
        let lines = to_vec(&["L1: some log entry"]);
        let pattern = to_vec(&["L1: some log entry"]);
        // Exact match succeeds on the first pass — no stripping involved.
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }
}
