//! CEX-S2-17 capability-package install decision.
//!
//! Composes the pure pieces — manifest validation, content-checksum
//! verification, the capability [`diff`](super::diff), and the default-deny
//! re-confirmation rule — into the [`InstallDecision`] the `libra package
//! install` flow renders before it touches the live registries.
//!
//! Computing the decision is pure (plus the caller's read of the installed-package
//! [`store`](super::store)); **applying** it — persisting the package and
//! registering its capabilities against the live source pool / skill loader /
//! agent registry — is the caller's commit step. Keeping the decision pure means
//! the install gate (verify → diff → confirm) is deterministic and unit-testable
//! without any I/O.

use std::{io, path::Path};

use super::{
    checksum::{self, ChecksumError, verify_against_manifest},
    diff::CapabilityDiff,
    manifest::{CapabilityPackageManifest, ManifestValidationError},
    registry::InstalledPackage,
};

/// File name of the JSON manifest at the root of a capability-package directory.
/// JSON (not TOML) so the serde-`transparent` id / digest newtypes round-trip
/// exactly as the in-crate schema tests already pin.
pub const MANIFEST_FILE: &str = "manifest.json";

/// A capability package loaded from a local directory: its parsed manifest plus
/// every bundled content file as a `(relative_path, bytes)` entry, ready for
/// [`prepare_install`]. The manifest file itself is excluded from `entries`
/// because it carries the checksum and so cannot be part of its own digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedPackage {
    /// The parsed `manifest.json`.
    pub manifest: CapabilityPackageManifest,
    /// Bundled content files, sorted by relative path with `/` separators.
    pub entries: Vec<(String, Vec<u8>)>,
}

/// Load a capability package from a local directory: parse `manifest.json` and
/// read every other file as a content entry (paths normalised to `/`-separated
/// and sorted for determinism). The manifest is *not* re-validated or
/// checksum-verified here — that is [`prepare_install`]'s job — so a malformed
/// manifest surfaces as an `InvalidData` error naming the file, and a missing
/// manifest as the underlying `NotFound`.
pub fn load_package_dir(dir: &Path) -> io::Result<LoadedPackage> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let manifest_bytes = std::fs::read(&manifest_path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to read package manifest '{}': {err}",
                manifest_path.display()
            ),
        )
    })?;
    let manifest: CapabilityPackageManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "invalid package manifest '{}': {err}",
                    manifest_path.display()
                ),
            )
        })?;

    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry
            .map_err(|err| io::Error::other(format!("failed to walk package directory: {err}")))?;
        if !entry.file_type().is_file() || entry.path() == manifest_path {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(dir)
            .map_err(|err| io::Error::other(format!("package path escaped its root: {err}")))?
            .to_string_lossy()
            .replace('\\', "/");
        entries.push((rel, std::fs::read(entry.path())?));
    }
    // `WalkDir::sort_by_file_name` orders directory walk, not the flattened
    // relative paths; sort the collected entries so the load is deterministic
    // regardless of directory nesting.
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(LoadedPackage { manifest, entries })
}

/// The vetted outcome of evaluating a package for install/update, ready to
/// render to the user and (on confirmation) commit to the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallDecision {
    /// The capability diff to show the user (new tools / sources / agents /
    /// permissions). For a fresh install this is everything the manifest
    /// bundles; for an update it is the delta from the installed version.
    pub diff: CapabilityDiff,
    /// `true` when a package with the same id is already installed (the update
    /// path) rather than a fresh install.
    pub is_update: bool,
    /// `true` when an update carries a different content checksum than the
    /// installed version — CEX-S2-17 验收 (4) forces a fresh diff + confirmation.
    pub checksum_changed: bool,
    /// `true` when the user must explicitly confirm before anything is
    /// registered: a newly-added mutating capability (source / sub-agent) per
    /// 验收 (2)/(3), or a changed-checksum update per 验收 (4).
    pub requires_confirmation: bool,
    /// Manifest `install_warnings`, surfaced to the user verbatim.
    pub warnings: Vec<String>,
    /// The package to persist once confirmed. Default-deny: `enabled = false`
    /// until the install flow explicitly enables it (验收 (3)).
    pub package: InstalledPackage,
}

/// Why a package cannot be installed. Surfaced to the user verbatim so a
/// malformed or tampered package is rejected with an actionable reason rather
/// than silently trusted — nothing is registered when this is returned.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    /// The manifest's own integrity fields are malformed.
    #[error("capability package manifest is invalid: {0}")]
    Manifest(#[from] ManifestValidationError),
    /// The package content does not match the manifest's declared checksum.
    #[error("capability package content failed verification: {0}")]
    Checksum(#[from] ChecksumError),
}

/// Evaluate a package for install/update against the currently-installed set,
/// returning the [`InstallDecision`] to render and (on confirmation) commit.
///
/// Rejects — registering nothing — when the manifest is invalid or its content
/// digest does not match its declared `checksum`. On success the returned
/// `package` is **disabled** (default-deny); the install flow enables it only
/// after the user accepts the diff. Pure — no I/O.
pub fn prepare_install(
    manifest: CapabilityPackageManifest,
    entries: &[(String, Vec<u8>)],
    installed: &[InstalledPackage],
) -> Result<InstallDecision, InstallError> {
    manifest.validate()?;
    verify_against_manifest(&manifest, entries)?;

    let prior = installed
        .iter()
        .find(|p| p.manifest.package_id == manifest.package_id);
    let is_update = prior.is_some();
    let checksum_changed =
        prior.is_some_and(|p| checksum::checksum_changed(&p.manifest.checksum, &manifest.checksum));
    let diff = match prior {
        Some(p) => CapabilityDiff::for_update(&p.manifest, &manifest),
        None => CapabilityDiff::for_install(&manifest),
    };
    // A newly-added mutating capability always needs confirmation (验收 2/3);
    // a content change on an update also forces re-confirmation (验收 4).
    let requires_confirmation = diff.requires_reconfirmation() || (is_update && checksum_changed);
    let warnings = manifest.install_warnings.clone();

    Ok(InstallDecision {
        diff,
        is_update,
        checksum_changed,
        requires_confirmation,
        warnings,
        package: InstalledPackage {
            manifest,
            enabled: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::{
        agent_run::PackageId, capability_package::manifest::BundledCapabilities,
    };

    fn package_entries(body: &str) -> Vec<(String, Vec<u8>)> {
        vec![("skills/explore.md".to_string(), body.as_bytes().to_vec())]
    }

    fn manifest(
        id: &str,
        entries: &[(String, Vec<u8>)],
        sources: Vec<String>,
    ) -> CapabilityPackageManifest {
        CapabilityPackageManifest {
            package_id: PackageId(id.to_string()),
            version: "1.0.0".to_string(),
            publisher: "acme".to_string(),
            checksum: checksum::compute_package_checksum(entries),
            bundled: BundledCapabilities {
                skills: vec!["explore".to_string()],
                sources,
                ..BundledCapabilities::default()
            },
            requested_permissions: Vec::new(),
            install_warnings: vec!["bundles a network source".to_string()],
        }
    }

    #[test]
    fn fresh_install_of_a_mutating_package_requires_confirmation_and_is_disabled() {
        let entries = package_entries("explore body");
        let m = manifest("acme.toolkit", &entries, vec!["acme-src".to_string()]);
        let decision = prepare_install(m, &entries, &[]).expect("valid fresh install");

        assert!(!decision.is_update);
        assert!(!decision.checksum_changed);
        assert!(
            decision.requires_confirmation,
            "a new mutating source must require confirmation",
        );
        assert!(
            !decision.package.enabled,
            "default-deny: the staged package is disabled until accepted",
        );
        assert!(decision.diff.sources.added.iter().any(|s| s == "acme-src"));
        assert_eq!(
            decision.warnings,
            vec!["bundles a network source".to_string()]
        );
    }

    #[test]
    fn fresh_install_without_mutating_capability_needs_no_confirmation() {
        let entries = package_entries("explore body");
        // No bundled sources / sub-agents -> only skills, which do not force
        // re-confirmation.
        let m = manifest("acme.skillpack", &entries, Vec::new());
        let decision = prepare_install(m, &entries, &[]).expect("valid");
        assert!(!decision.requires_confirmation);
        assert!(decision.diff.skills.added.iter().any(|s| s == "explore"));
    }

    #[test]
    fn update_with_changed_checksum_forces_confirmation() {
        let v1_entries = package_entries("v1 body");
        let installed = vec![InstalledPackage {
            manifest: manifest("acme.toolkit", &v1_entries, Vec::new()),
            enabled: true,
        }];

        // Same id, new content (different checksum), no new mutating capability.
        let v2_entries = package_entries("v2 body");
        let v2 = manifest("acme.toolkit", &v2_entries, Vec::new());
        let decision = prepare_install(v2, &v2_entries, &installed).expect("valid update");

        assert!(decision.is_update);
        assert!(decision.checksum_changed);
        assert!(
            decision.requires_confirmation,
            "a changed-checksum update must re-prompt (验收 4)",
        );
    }

    #[test]
    fn reinstall_identical_content_is_an_update_without_confirmation() {
        let entries = package_entries("same body");
        let m = manifest("acme.skillpack", &entries, Vec::new());
        let installed = vec![InstalledPackage {
            manifest: m.clone(),
            enabled: true,
        }];
        let decision = prepare_install(m, &entries, &installed).expect("valid");
        assert!(decision.is_update);
        assert!(
            !decision.checksum_changed,
            "identical content, same checksum"
        );
        assert!(!decision.requires_confirmation, "no change, no re-prompt");
    }

    #[test]
    fn tampered_content_is_rejected_and_registers_nothing() {
        let entries = package_entries("declared body");
        let m = manifest("acme.toolkit", &entries, Vec::new());
        // The on-disk content differs from what the manifest's checksum claims.
        let tampered = package_entries("TAMPERED body");
        let err = prepare_install(m, &tampered, &[]).expect_err("must reject");
        assert!(matches!(err, InstallError::Checksum(_)));
    }

    #[test]
    fn invalid_manifest_is_rejected() {
        let entries = package_entries("body");
        let mut m = manifest("acme.toolkit", &entries, Vec::new());
        m.publisher = "   ".to_string(); // empty identity field
        let err = prepare_install(m, &entries, &[]).expect_err("must reject");
        assert!(matches!(err, InstallError::Manifest(_)));
    }

    #[test]
    fn load_package_dir_parses_manifest_collects_content_and_passes_the_gate() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::write(dir.join("skills/explore.md"), b"explore body").unwrap();
        std::fs::write(dir.join("commands/build.md"), b"build body").unwrap();

        // The loader excludes manifest.json and sorts entries by relative path.
        let expected_entries = vec![
            ("commands/build.md".to_string(), b"build body".to_vec()),
            ("skills/explore.md".to_string(), b"explore body".to_vec()),
        ];
        let m = CapabilityPackageManifest {
            package_id: PackageId("acme.toolkit".to_string()),
            version: "1.0.0".to_string(),
            publisher: "acme".to_string(),
            checksum: checksum::compute_package_checksum(&expected_entries),
            bundled: BundledCapabilities {
                skills: vec!["explore".to_string()],
                commands: vec!["build".to_string()],
                ..BundledCapabilities::default()
            },
            requested_permissions: Vec::new(),
            install_warnings: Vec::new(),
        };
        std::fs::write(
            dir.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(&m).unwrap(),
        )
        .unwrap();

        let loaded = load_package_dir(dir).expect("load package dir");
        assert_eq!(loaded.manifest.package_id.0, "acme.toolkit");
        assert_eq!(
            loaded.entries, expected_entries,
            "manifest excluded, content collected + sorted",
        );

        // End-to-end: the freshly-loaded package passes the install gate (its
        // on-disk content hashes to exactly the manifest's declared checksum).
        let decision =
            prepare_install(loaded.manifest, &loaded.entries, &[]).expect("install gate");
        assert!(!decision.is_update);
        assert!(
            !decision.requires_confirmation,
            "no mutating capability bundled"
        );
    }

    #[test]
    fn load_package_dir_missing_manifest_is_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let err = load_package_dir(temp.path()).expect_err("no manifest");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
