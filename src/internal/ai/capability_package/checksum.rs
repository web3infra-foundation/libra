//! CEX-S2-17 capability-package content checksum.
//!
//! A package's `manifest.checksum` is a SHA-256 digest over the package's
//! bundled content. The installer recomputes it on install and on update
//! (CEX-S2-17 验收 (4): a changed checksum forces a fresh permission diff +
//! reconfirmation) and refuses to register a package whose on-disk content does
//! not match the digest its manifest claims — a tampered or truncated download
//! is rejected before any capability is granted.
//!
//! This module is the **pure digest core**: given the package's `(path, bytes)`
//! entries it produces a canonical [`Sha256`], and [`verify_against_manifest`]
//! compares it against the manifest's claimed digest. Reading the files off
//! disk is the installer's job; keeping the digest pure makes it deterministic
//! and trivially testable.

use ring::digest;

use super::manifest::CapabilityPackageManifest;
use crate::internal::ai::agent_run::Sha256;

/// Compute the canonical SHA-256 content digest for a capability package from
/// its bundled `(path, bytes)` entries.
///
/// Entries are hashed in a deterministic order (sorted by path) with
/// length-prefixed framing of both the path and the content, so two packages
/// whose bytes differ only in layout (e.g. a path boundary shifted into file
/// content) can never collide on a flat concatenation. The result is the
/// 64-character lowercase-hex form the manifest stores. Pure — no I/O.
pub fn compute_package_checksum(entries: &[(String, Vec<u8>)]) -> Sha256 {
    let mut ordered: Vec<&(String, Vec<u8>)> = entries.iter().collect();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));

    let mut ctx = digest::Context::new(&digest::SHA256);
    for (path, bytes) in ordered {
        // Length-prefix each field so `("a", "bc")` and `("ab", "c")` — or any
        // other re-split of the same concatenation — hash differently.
        ctx.update(&(path.len() as u64).to_le_bytes());
        ctx.update(path.as_bytes());
        ctx.update(&(bytes.len() as u64).to_le_bytes());
        ctx.update(bytes);
    }
    Sha256(hex::encode(ctx.finish().as_ref()))
}

/// Why a capability package failed content-integrity verification.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChecksumError {
    /// The recomputed content digest does not match the manifest's `checksum`;
    /// the installer must refuse to register the package.
    #[error(
        "capability package checksum mismatch: manifest claims {claimed}, content hashes to {actual}"
    )]
    Mismatch {
        /// The digest the manifest declares.
        claimed: String,
        /// The digest computed from the on-disk content.
        actual: String,
    },
}

/// Recompute the digest over `entries` and confirm it matches the manifest's
/// declared `checksum`. Returns the verified [`Sha256`] on success (so the
/// caller can record it as the installed digest); returns
/// [`ChecksumError::Mismatch`] — carrying both digests — when the content has
/// been tampered with or truncated. Pure — no I/O.
pub fn verify_against_manifest(
    manifest: &CapabilityPackageManifest,
    entries: &[(String, Vec<u8>)],
) -> Result<Sha256, ChecksumError> {
    let actual = compute_package_checksum(entries);
    if actual == manifest.checksum {
        Ok(actual)
    } else {
        Err(ChecksumError::Mismatch {
            claimed: manifest.checksum.0.clone(),
            actual: actual.0,
        })
    }
}

/// `true` when a package's freshly-computed content digest differs from the
/// digest recorded at its previous install — the update trigger CEX-S2-17 验收
/// (4) requires to recompute the permission diff and re-prompt for
/// confirmation. Pure comparison.
pub fn checksum_changed(previous: &Sha256, current: &Sha256) -> bool {
    previous != current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::agent_run::PackageId;

    fn entries() -> Vec<(String, Vec<u8>)> {
        vec![
            ("skills/explore.md".to_string(), b"explore body".to_vec()),
            ("commands/build.md".to_string(), b"build body".to_vec()),
        ]
    }

    fn manifest_with(checksum: Sha256) -> CapabilityPackageManifest {
        CapabilityPackageManifest {
            package_id: PackageId("acme.toolkit".to_string()),
            version: "1.0.0".to_string(),
            publisher: "acme".to_string(),
            checksum,
            bundled: Default::default(),
            requested_permissions: Vec::new(),
            install_warnings: Vec::new(),
        }
    }

    #[test]
    fn checksum_is_64_char_lowercase_hex_and_deterministic() {
        let digest = compute_package_checksum(&entries());
        assert_eq!(digest.0.len(), 64, "SHA-256 hex is 64 chars");
        assert!(
            digest
                .0
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "digest must be lowercase hex: {}",
            digest.0,
        );
        // Recomputing the same content yields the identical digest.
        assert_eq!(compute_package_checksum(&entries()), digest);
        // Validation accepts the produced digest as a canonical checksum.
        assert!(manifest_with(digest).validate().is_ok());
    }

    #[test]
    fn checksum_is_order_independent() {
        let mut reordered = entries();
        reordered.reverse();
        assert_eq!(
            compute_package_checksum(&entries()),
            compute_package_checksum(&reordered),
            "entry input order must not change the canonical digest",
        );
    }

    #[test]
    fn checksum_changes_when_content_changes() {
        let base = compute_package_checksum(&entries());

        // Mutated file content.
        let mut tampered = entries();
        tampered[0].1 = b"explore body TAMPERED".to_vec();
        assert_ne!(base, compute_package_checksum(&tampered));

        // The length-prefix framing defeats a path/content re-split collision.
        let split_a = vec![("ab".to_string(), b"c".to_vec())];
        let split_b = vec![("a".to_string(), b"bc".to_vec())];
        assert_ne!(
            compute_package_checksum(&split_a),
            compute_package_checksum(&split_b),
            "re-splitting path/content boundaries must not collide",
        );
    }

    #[test]
    fn verify_accepts_matching_and_rejects_tampered_content() {
        let entries = entries();
        let manifest = manifest_with(compute_package_checksum(&entries));

        // Matching content verifies and returns the digest.
        let verified = verify_against_manifest(&manifest, &entries).expect("matching content");
        assert_eq!(verified, manifest.checksum);

        // Tampered content is rejected with both digests surfaced.
        let mut tampered = entries.clone();
        tampered[0].1.push(b'!');
        let err = verify_against_manifest(&manifest, &tampered).expect_err("tampered content");
        let ChecksumError::Mismatch { claimed, actual } = err;
        assert_eq!(claimed, manifest.checksum.0);
        assert_ne!(
            actual, claimed,
            "the actual digest must differ from the claim"
        );
    }

    #[test]
    fn checksum_changed_detects_update() {
        let v1 = compute_package_checksum(&entries());
        let mut updated = entries();
        updated.push(("sources/new.json".to_string(), b"{}".to_vec()));
        let v2 = compute_package_checksum(&updated);
        assert!(
            checksum_changed(&v1, &v2),
            "added content is a changed checksum"
        );
        assert!(
            !checksum_changed(&v1, &v1),
            "an unchanged package is not a change"
        );
    }
}
