pub(super) fn matches_any_ref_pattern(refname: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| matches_ref_pattern(refname, pattern))
}

fn matches_ref_pattern(refname: &str, pattern: &str) -> bool {
    let base_refname = refname.strip_suffix("^{}").unwrap_or(refname);
    base_refname == pattern
        || base_refname
            .strip_suffix(pattern)
            .is_some_and(|prefix| prefix.ends_with('/'))
}

#[cfg(test)]
mod tests {
    use super::matches_ref_pattern;

    #[test]
    fn matches_full_ref_or_path_segment_suffix() {
        assert!(matches_ref_pattern("refs/heads/main", "refs/heads/main"));
        assert!(matches_ref_pattern("refs/heads/main", "heads/main"));
        assert!(matches_ref_pattern("refs/remotes/origin/main", "main"));
    }

    #[test]
    fn rejects_plain_substrings_inside_ref_segments() {
        assert!(!matches_ref_pattern("refs/heads/main-2", "main"));
        assert!(!matches_ref_pattern("refs/heads/domain", "main"));
    }

    #[test]
    fn peeled_refs_match_against_base_refname() {
        assert!(matches_ref_pattern("refs/tags/v1.0^{}", "v1.0"));
        assert!(!matches_ref_pattern("refs/tags/v1.0-rc1^{}", "v1.0"));
    }
}
