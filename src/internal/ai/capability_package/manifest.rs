//! Capability package manifest schema (CEX-S2-17, Step 2.7).
//!
//! A *capability package* bundles skills, slash commands, Source Pool sources /
//! MCP servers and sub-agent definitions into one auditable, versioned unit so
//! ecosystem extensions cannot bypass the Source Pool and permission model.
//!
//! This module owns **only** the pure, serde-frozen manifest schema — the
//! data a package ships and an installer reads. The runtime pieces the card
//! also calls for (install-time capability diff, mutating-source / Worker
//! sub-agent gating, checksum recomputation on update, routing bundled
//! sub-agent definitions through `AgentPermissionProfile`) are separate and
//! land later; none of them are performed here. No I/O occurs in this module.
//!
//! `#[serde(deny_unknown_fields)]` on both structs freezes the wire contract so
//! a future field rename or an unrecognised key in a third-party manifest is a
//! hard error rather than a silently-ignored capability. The package identity
//! (`PackageId`) and integrity digest (`Sha256`) reuse the existing
//! [`agent_run`](crate::internal::ai::agent_run) newtypes rather than forking
//! parallel types.
//!
//! See `docs/improvement/agent.md` Step 2.7 (CEX-S2-17).

use serde::{Deserialize, Serialize};

use crate::internal::ai::agent_run::{PackageId, Sha256};

/// The concrete capabilities a package bundles. Each entry is the stable
/// identifier (slug / name) of the bundled artifact; the artifacts themselves
/// live alongside the manifest and are resolved at install time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundledCapabilities {
    /// Bundled Markdown skill names.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Bundled slash-command names.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Bundled Source Pool source / MCP server slugs.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Bundled sub-agent definition names.
    #[serde(default)]
    pub sub_agents: Vec<String>,
}

impl BundledCapabilities {
    /// `true` when the package bundles no capabilities of any kind. An empty
    /// bundle registers nothing on install (the installer should treat it as a
    /// no-op rather than an error).
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
            && self.commands.is_empty()
            && self.sources.is_empty()
            && self.sub_agents.is_empty()
    }
}

/// A local capability package manifest (CEX-S2-17 应该完成的功能 (1)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityPackageManifest {
    /// Stable package identifier.
    pub package_id: PackageId,
    /// Package version string (semver recommended, not enforced here).
    pub version: String,
    /// Publisher identity (free-form; trust evaluation lives elsewhere).
    pub publisher: String,
    /// Integrity digest over the package contents, recomputed on update by the
    /// installer (out of scope for this schema module).
    pub checksum: Sha256,
    /// The capabilities this package bundles.
    #[serde(default)]
    pub bundled: BundledCapabilities,
    /// Permissions the package requests at install time. Surfaced verbatim in
    /// the install-time capability diff (which lives in the installer).
    #[serde(default)]
    pub requested_permissions: Vec<String>,
    /// Warnings to show the user at install time (e.g. "bundles a mutating
    /// source").
    #[serde(default)]
    pub install_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_manifest() -> CapabilityPackageManifest {
        CapabilityPackageManifest {
            package_id: PackageId("acme.toolkit".to_string()),
            version: "1.2.3".to_string(),
            publisher: "acme".to_string(),
            checksum: Sha256("a".repeat(64)),
            bundled: BundledCapabilities {
                skills: vec!["lint".to_string()],
                commands: vec!["/acme".to_string()],
                sources: vec!["acme-mcp".to_string()],
                sub_agents: vec!["acme-reviewer".to_string()],
            },
            requested_permissions: vec!["source:acme-mcp:read".to_string()],
            install_warnings: vec!["bundles an MCP source".to_string()],
        }
    }

    #[test]
    fn full_manifest_round_trips() {
        let manifest = full_manifest();
        let json = serde_json::to_string(&manifest).expect("serialize");
        let back: CapabilityPackageManifest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(manifest, back);
    }

    #[test]
    fn minimal_manifest_defaults_collections_to_empty() {
        // Only the four required fields present; the optional collections must
        // default to empty rather than failing to deserialize.
        let json = r#"{
            "package_id": "acme.toolkit",
            "version": "0.1.0",
            "publisher": "acme",
            "checksum": "0000000000000000000000000000000000000000000000000000000000000000"
        }"#;
        let manifest: CapabilityPackageManifest = serde_json::from_str(json).expect("deserialize");
        assert!(manifest.bundled.is_empty());
        assert!(manifest.requested_permissions.is_empty());
        assert!(manifest.install_warnings.is_empty());
    }

    #[test]
    fn empty_collections_serialize_as_arrays() {
        // No skip_serializing_if: empty bundles/permissions/warnings must appear
        // as `[]` so the wire shape is explicit for the install-time diff.
        let manifest = CapabilityPackageManifest {
            package_id: PackageId("p".to_string()),
            version: "0".to_string(),
            publisher: "x".to_string(),
            checksum: Sha256("0".repeat(64)),
            bundled: BundledCapabilities::default(),
            requested_permissions: Vec::new(),
            install_warnings: Vec::new(),
        };
        let value: serde_json::Value =
            serde_json::to_value(&manifest).expect("serialize to value");
        assert_eq!(value["requested_permissions"], serde_json::json!([]));
        assert_eq!(value["install_warnings"], serde_json::json!([]));
        assert_eq!(value["bundled"]["skills"], serde_json::json!([]));
        // PackageId / Sha256 are transparent newtypes -> bare strings on the wire.
        assert_eq!(value["package_id"], serde_json::json!("p"));
        assert_eq!(value["checksum"], serde_json::json!("0".repeat(64)));
    }

    #[test]
    fn manifest_rejects_unknown_fields() {
        let json = r#"{
            "package_id": "p",
            "version": "0",
            "publisher": "x",
            "checksum": "0000000000000000000000000000000000000000000000000000000000000000",
            "surprise": true
        }"#;
        let parsed: Result<CapabilityPackageManifest, _> = serde_json::from_str(json);
        assert!(parsed.is_err(), "deny_unknown_fields must reject extras");
    }

    #[test]
    fn bundled_rejects_unknown_fields() {
        let json = r#"{ "skills": [], "plugins": [] }"#;
        let parsed: Result<BundledCapabilities, _> = serde_json::from_str(json);
        assert!(parsed.is_err(), "deny_unknown_fields must reject extras");
    }

    #[test]
    fn is_empty_tracks_each_bundle_slot() {
        assert!(BundledCapabilities::default().is_empty());
        for non_empty in [
            BundledCapabilities {
                skills: vec!["s".to_string()],
                ..Default::default()
            },
            BundledCapabilities {
                commands: vec!["c".to_string()],
                ..Default::default()
            },
            BundledCapabilities {
                sources: vec!["src".to_string()],
                ..Default::default()
            },
            BundledCapabilities {
                sub_agents: vec!["a".to_string()],
                ..Default::default()
            },
        ] {
            assert!(!non_empty.is_empty(), "{non_empty:?} should be non-empty");
        }
    }
}
