//! Shared text helpers for safe abbreviated display and fuzzy matching.

/// Default short hash width used in human-readable confirmations.
pub const SHORT_HASH_LEN: usize = 7;

/// Return a shortened display form of a hash-like string without assuming ASCII.
pub fn short_display_hash(hash: &str) -> &str {
    if hash.chars().count() <= SHORT_HASH_LEN {
        return hash;
    }

    let byte_idx = hash
        .char_indices()
        .nth(SHORT_HASH_LEN)
        .map(|(idx, _)| idx)
        .unwrap_or(hash.len());

    hash.get(..byte_idx).unwrap_or(hash)
}

/// Compute the Levenshtein edit distance between two strings.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (a, b) = if a.len() > b.len() {
        (&b, &a)
    } else {
        (&a, &b)
    };
    let mut prev: Vec<usize> = (0..=a.len()).collect();
    let mut curr = vec![0; a.len() + 1];
    for (i, cb) in b.iter().enumerate() {
        curr[0] = i + 1;
        for (j, ca) in a.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[a.len()]
}

#[cfg(test)]
mod tests {
    use super::{levenshtein, short_display_hash};

    #[test]
    fn short_display_hash_keeps_ascii_prefix() {
        assert_eq!(short_display_hash("1234567890"), "1234567");
    }

    #[test]
    fn short_display_hash_respects_utf8_boundaries() {
        assert_eq!(short_display_hash("éééééééé"), "ééééééé");
    }

    #[test]
    fn levenshtein_handles_basic_edge_cases() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("main", "maim"), 1);
        assert_eq!(levenshtein("feature", "featur"), 1);
    }
}
