pub fn normalize_commit_anchor(commit: &str) -> Result<String, String> {
    let v = commit.trim();
    if !v.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "Invalid commit hash: contains non-hex characters: {}",
            v
        ));
    }
    let v = v.to_ascii_lowercase();
    if v.len() == 64 {
        return Ok(v);
    }
    if v.len() == 40 {
        let mut out = String::with_capacity(64);
        out.push_str(&v);
        while out.len() < 64 {
            out.push('0');
        }
        return Ok(out);
    }
    Err(format!("Invalid commit hash length: {}", v.len()))
}

pub fn extract_sha1_from_anchor(anchor64: &str) -> Result<String, String> {
    let v = anchor64.trim();
    if v.len() != 64 {
        return Err(format!("Invalid anchor length: {}", v.len()));
    }
    if !v.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "Invalid anchor: contains non-hex characters: {}",
            v
        ));
    }
    Ok(v.chars().take(40).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_accepts_sha256() {
        let v = "a".repeat(64);
        assert_eq!(normalize_commit_anchor(&v).unwrap(), v);
    }

    #[test]
    fn normalize_pads_sha1() {
        let sha1 = "b".repeat(40);
        let normalized = normalize_commit_anchor(&sha1).unwrap();
        assert_eq!(normalized.len(), 64);
        assert!(normalized.starts_with(&sha1));
        assert_eq!(&normalized[40..], "0".repeat(24));
    }

    #[test]
    fn normalize_rejects_other_lengths() {
        assert!(normalize_commit_anchor("abc").is_err());
    }

    #[test]
    fn extract_sha1_from_anchor_returns_prefix() {
        let anchor = format!("{}{}", "c".repeat(40), "0".repeat(24));
        assert_eq!(extract_sha1_from_anchor(&anchor).unwrap(), "c".repeat(40));
    }
}
