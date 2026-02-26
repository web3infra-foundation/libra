//! Utility functions for the AI module.
//!
//! This module provides conversion utilities for commit hashes between different formats,
//! primarily handling compatibility between SHA-1 (40-char) and SHA-256 (64-char) hash formats.
//!
//! # Core Concepts
//!
//! - **Anchor**: A unified 64-character hexadecimal string used as the internal standard storage format.
//! - **SHA-1 hash**: The traditional 40-character hex hash used by Git.
//! - **SHA-256 hash**: The newer 64-character hex hash supported by Git.
//!
//! # Conversion Rules
//!
//! - SHA-256 hashes (64 chars) are used directly as anchors without conversion.
//! - SHA-1 hashes (40 chars) are zero-padded at the end to 64 characters to form an anchor.
//! - To extract a SHA-1 from an anchor, simply take the first 40 characters.

/// Normalizes a commit hash into a 64-character anchor format.
///
/// This function accepts a commit hash in either SHA-1 (40-char) or SHA-256 (64-char) format
/// and converts it into a unified 64-character lowercase hexadecimal string (anchor format).
///
/// # Processing Logic
///
/// 1. Trims leading and trailing whitespace from the input.
/// 2. Validates that all characters are valid hexadecimal digits (0-9, a-f, A-F).
/// 3. Converts all letters to lowercase.
/// 4. Processes based on length:
///    - 64 characters (SHA-256): returned as-is.
///    - 40 characters (SHA-1): zero-padded with 24 '0's at the end to reach 64 characters.
///    - Other lengths: returns an error.
///
/// # Arguments
///
/// - `commit`: The commit hash string to normalize.
///
/// # Returns
///
/// - `Ok(String)`: The normalized 64-character lowercase hexadecimal string.
/// - `Err(String)`: An error description if the input contains invalid characters or has an unsupported length.
///
/// # Examples
///
/// ```
/// use libra::internal::ai::util::normalize_commit_anchor;
///
/// // SHA-256 hash is returned directly (lowercased)
/// let sha256 = "a".repeat(64);
/// assert_eq!(normalize_commit_anchor(&sha256).unwrap(), sha256);
///
/// // SHA-1 hash is zero-padded to 64 characters
/// let sha1 = "b".repeat(40);
/// let result = normalize_commit_anchor(&sha1).unwrap();
/// assert_eq!(result.len(), 64);
/// ```
pub fn normalize_commit_anchor(commit: &str) -> Result<String, String> {
    // Trim leading and trailing whitespace
    let v = commit.trim();

    // Validate that all characters are hexadecimal digits (0-9, a-f, A-F)
    if !v.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "Invalid commit hash: contains non-hex characters: {}",
            v
        ));
    }

    // Convert to lowercase for consistent formatting
    let v = v.to_ascii_lowercase();

    // SHA-256 hash: exactly 64 characters, return as-is
    if v.len() == 64 {
        return Ok(v);
    }

    // SHA-1 hash: 40 characters, zero-pad to 64 characters
    if v.len() == 40 {
        let mut out = String::with_capacity(64);
        out.push_str(&v);
        // Append 24 '0's to reach a total length of 64
        while out.len() < 64 {
            out.push('0');
        }
        return Ok(out);
    }

    // Unsupported length, return an error
    Err(format!("Invalid commit hash length: {}", v.len()))
}

/// Extracts the original SHA-1 hash from a 64-character anchor format.
///
/// This function is the inverse of [`normalize_commit_anchor`] (for the SHA-1 case).
/// It extracts the first 40 characters from a 64-character anchor string,
/// which represents the original SHA-1 hash value.
///
/// # Processing Logic
///
/// 1. Trims leading and trailing whitespace from the input.
/// 2. Validates that the length is exactly 64 characters.
/// 3. Validates that all characters are valid hexadecimal digits.
/// 4. Returns the first 40 characters as the SHA-1 hash.
///
/// # Arguments
///
/// - `anchor64`: A 64-character anchor format string.
///
/// # Returns
///
/// - `Ok(String)`: The extracted 40-character SHA-1 hash.
/// - `Err(String)`: An error description if the length is incorrect or the input contains invalid characters.
///
/// # Examples
///
/// ```
/// use libra::internal::ai::util::extract_sha1_from_anchor;
///
/// let anchor = format!("{}{}", "c".repeat(40), "0".repeat(24));
/// assert_eq!(extract_sha1_from_anchor(&anchor).unwrap(), "c".repeat(40));
/// ```
pub fn extract_sha1_from_anchor(anchor64: &str) -> Result<String, String> {
    // Trim leading and trailing whitespace
    let v = anchor64.trim();

    // Validate that the length is exactly 64 characters
    if v.len() != 64 {
        return Err(format!("Invalid anchor length: {}", v.len()));
    }

    // Validate that all characters are hexadecimal digits
    if !v.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "Invalid anchor: contains non-hex characters: {}",
            v
        ));
    }

    // Take the first 40 characters as the SHA-1 hash
    Ok(v.chars().take(40).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test: a SHA-256 hash (64 chars) should be accepted and returned as-is.
    #[test]
    fn normalize_accepts_sha256() {
        let v = "a".repeat(64);
        assert_eq!(normalize_commit_anchor(&v).unwrap(), v);
    }

    /// Test: a SHA-1 hash (40 chars) should be zero-padded to 64 characters.
    #[test]
    fn normalize_pads_sha1() {
        let sha1 = "b".repeat(40);
        let normalized = normalize_commit_anchor(&sha1).unwrap();
        assert_eq!(normalized.len(), 64);
        assert!(normalized.starts_with(&sha1));
        assert_eq!(&normalized[40..], "0".repeat(24));
    }

    /// Test: hashes with unsupported lengths should be rejected with an error.
    #[test]
    fn normalize_rejects_other_lengths() {
        assert!(normalize_commit_anchor("abc").is_err());
    }

    /// Test: the first 40 characters should be correctly extracted as the SHA-1 hash from an anchor.
    #[test]
    fn extract_sha1_from_anchor_returns_prefix() {
        let anchor = format!("{}{}", "c".repeat(40), "0".repeat(24));
        assert_eq!(extract_sha1_from_anchor(&anchor).unwrap(), "c".repeat(40));
    }
}
