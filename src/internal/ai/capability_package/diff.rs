//! Capability package install / update diff (CEX-S2-17, Step 2.7).
//!
//! Before a capability package is installed or upgraded the user must be shown
//! exactly which **new** capabilities it would grant — new tools (skills /
//! commands), new sources / MCP servers, new sub-agent definitions and new
//! requested permissions (`docs/development/commands/agent.md` Step 2.7: "安装或启用
//! package 时展示 capability diff"; "package 更新时重新计算 checksum 和
//! permission diff"). When an update adds a *mutating* capability — a new
//! source / MCP server or a new sub-agent definition — re-confirmation is
//! mandatory ("package 更新新增 mutating capability 时必须重新确认").
//!
//! This module owns **only** the pure diff computation. Rendering the diff and
//! driving the confirmation prompt are runtime concerns and live elsewhere. The
//! diff is **additive**: it reports capabilities the new manifest grants that
//! the old one did not. Removed capabilities are deliberately *not* part of the
//! grant surface — uninstall cleanup is a separate concern — but the removed
//! sets are surfaced for display completeness. No I/O occurs here.

use std::collections::BTreeSet;

use super::manifest::{BundledCapabilities, CapabilityPackageManifest};

/// The additive set difference of two capability lists: entries present in
/// `next` but not in `prev`, plus entries present in `prev` but not in `next`.
/// Order-insensitive and de-duplicated (a manifest listing a capability twice
/// grants it once).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StringSetDelta {
    /// Entries newly granted by the update (in `next`, not in `prev`).
    pub added: Vec<String>,
    /// Entries the update drops (in `prev`, not in `next`).
    pub removed: Vec<String>,
}

impl StringSetDelta {
    fn between(prev: &[String], next: &[String]) -> Self {
        let prev_set: BTreeSet<&str> = prev.iter().map(String::as_str).collect();
        let next_set: BTreeSet<&str> = next.iter().map(String::as_str).collect();
        Self {
            added: next_set
                .difference(&prev_set)
                .map(|s| s.to_string())
                .collect(),
            removed: prev_set
                .difference(&next_set)
                .map(|s| s.to_string())
                .collect(),
        }
    }

    /// Delta between two already de-duplicated string sets: `added` = entries in
    /// `next` not in `prev`; `removed` = entries in `prev` not in `next`. Both
    /// lists come out sorted (BTreeSet iteration order) so the result is
    /// deterministic. Used by the effective-capability-set delta where the
    /// inputs are already unioned [`BTreeSet`]s rather than raw manifest lists.
    pub fn between_sets(prev: &BTreeSet<String>, next: &BTreeSet<String>) -> Self {
        Self {
            added: next.difference(prev).cloned().collect(),
            removed: prev.difference(next).cloned().collect(),
        }
    }

    /// `true` when nothing was added or removed.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

/// The computed diff between a previously-installed manifest (or nothing, for a
/// fresh install) and the manifest about to be installed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapabilityDiff {
    /// Skill name delta.
    pub skills: StringSetDelta,
    /// Slash-command delta.
    pub commands: StringSetDelta,
    /// Source / MCP-server delta (mutating capability).
    pub sources: StringSetDelta,
    /// Sub-agent definition delta (mutating capability).
    pub sub_agents: StringSetDelta,
    /// Requested-permission delta.
    pub requested_permissions: StringSetDelta,
}

impl CapabilityDiff {
    /// Compute the diff for a **fresh install**: everything the manifest bundles
    /// and requests is newly granted (the empty-manifest baseline).
    pub fn for_install(manifest: &CapabilityPackageManifest) -> Self {
        Self::between_bundles(
            &BundledCapabilities::default(),
            &[],
            &manifest.bundled,
            &manifest.requested_permissions,
        )
    }

    /// Compute the diff for an **update** from `prev` to `next`.
    pub fn for_update(prev: &CapabilityPackageManifest, next: &CapabilityPackageManifest) -> Self {
        Self::between_bundles(
            &prev.bundled,
            &prev.requested_permissions,
            &next.bundled,
            &next.requested_permissions,
        )
    }

    fn between_bundles(
        prev_bundle: &BundledCapabilities,
        prev_permissions: &[String],
        next_bundle: &BundledCapabilities,
        next_permissions: &[String],
    ) -> Self {
        Self {
            skills: StringSetDelta::between(&prev_bundle.skills, &next_bundle.skills),
            commands: StringSetDelta::between(&prev_bundle.commands, &next_bundle.commands),
            sources: StringSetDelta::between(&prev_bundle.sources, &next_bundle.sources),
            sub_agents: StringSetDelta::between(&prev_bundle.sub_agents, &next_bundle.sub_agents),
            requested_permissions: StringSetDelta::between(prev_permissions, next_permissions),
        }
    }

    /// `true` when the diff grants no new capability of any kind. (Removed-only
    /// diffs are still "empty" for grant purposes but report `false` here only
    /// if something was added; use [`is_empty`](Self::is_empty) for the
    /// strict no-change check.)
    pub fn grants_nothing_new(&self) -> bool {
        self.skills.added.is_empty()
            && self.commands.added.is_empty()
            && self.sources.added.is_empty()
            && self.sub_agents.added.is_empty()
            && self.requested_permissions.added.is_empty()
    }

    /// `true` when the diff has no added or removed entries in any category.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
            && self.commands.is_empty()
            && self.sources.is_empty()
            && self.sub_agents.is_empty()
            && self.requested_permissions.is_empty()
    }

    /// Whether this diff requires explicit user re-confirmation.
    ///
    /// Per CEX-S2-17 验收 (2) a **newly added mutating capability** — a new
    /// source / MCP server or a new sub-agent definition — always requires
    /// re-confirmation. Adding skills / commands / permissions alone does not
    /// force re-confirmation here (the installer may still prompt for those),
    /// but a new mutating capability is non-negotiable.
    pub fn requires_reconfirmation(&self) -> bool {
        !self.sources.added.is_empty() || !self.sub_agents.added.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::manifest::{BundledCapabilities, CapabilityPackageManifest},
        *,
    };
    use crate::internal::ai::agent_run::{PackageId, Sha256};

    fn manifest(
        bundled: BundledCapabilities,
        permissions: Vec<String>,
    ) -> CapabilityPackageManifest {
        CapabilityPackageManifest {
            package_id: PackageId("acme.toolkit".to_string()),
            version: "1.0.0".to_string(),
            publisher: "acme".to_string(),
            checksum: Sha256("0".repeat(64)),
            bundled,
            requested_permissions: permissions,
            install_warnings: Vec::new(),
        }
    }

    fn bundle(
        skills: &[&str],
        commands: &[&str],
        sources: &[&str],
        sub_agents: &[&str],
    ) -> BundledCapabilities {
        let to_vec = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        BundledCapabilities {
            skills: to_vec(skills),
            commands: to_vec(commands),
            sources: to_vec(sources),
            sub_agents: to_vec(sub_agents),
        }
    }

    #[test]
    fn fresh_install_grants_every_bundled_capability() {
        let m = manifest(
            bundle(&["lint"], &["/acme"], &["acme-mcp"], &["reviewer"]),
            vec!["source:acme-mcp:read".to_string()],
        );
        let diff = CapabilityDiff::for_install(&m);
        assert_eq!(diff.skills.added, vec!["lint"]);
        assert_eq!(diff.commands.added, vec!["/acme"]);
        assert_eq!(diff.sources.added, vec!["acme-mcp"]);
        assert_eq!(diff.sub_agents.added, vec!["reviewer"]);
        assert_eq!(
            diff.requested_permissions.added,
            vec!["source:acme-mcp:read"]
        );
        // Nothing removed on a fresh install.
        assert!(diff.skills.removed.is_empty());
        assert!(!diff.grants_nothing_new());
    }

    #[test]
    fn fresh_install_of_a_source_requires_reconfirmation() {
        let m = manifest(bundle(&[], &[], &["acme-mcp"], &[]), Vec::new());
        assert!(CapabilityDiff::for_install(&m).requires_reconfirmation());
    }

    #[test]
    fn fresh_install_of_a_sub_agent_requires_reconfirmation() {
        let m = manifest(bundle(&[], &[], &[], &["reviewer"]), Vec::new());
        assert!(CapabilityDiff::for_install(&m).requires_reconfirmation());
    }

    #[test]
    fn fresh_install_of_only_skills_and_commands_does_not_force_reconfirmation() {
        let m = manifest(
            bundle(&["lint"], &["/acme"], &[], &[]),
            vec!["x".to_string()],
        );
        let diff = CapabilityDiff::for_install(&m);
        assert!(!diff.requires_reconfirmation());
        assert!(!diff.grants_nothing_new());
    }

    #[test]
    fn update_adding_a_source_requires_reconfirmation() {
        let prev = manifest(bundle(&["lint"], &[], &[], &[]), Vec::new());
        let next = manifest(bundle(&["lint"], &[], &["new-mcp"], &[]), Vec::new());
        let diff = CapabilityDiff::for_update(&prev, &next);
        assert_eq!(diff.sources.added, vec!["new-mcp"]);
        assert!(
            diff.skills.is_empty(),
            "unchanged skills must not appear in the diff"
        );
        assert!(diff.requires_reconfirmation());
    }

    #[test]
    fn update_dropping_a_source_reports_removed_and_does_not_force_reconfirmation() {
        let prev = manifest(bundle(&[], &[], &["old-mcp"], &[]), Vec::new());
        let next = manifest(bundle(&[], &[], &[], &[]), Vec::new());
        let diff = CapabilityDiff::for_update(&prev, &next);
        assert_eq!(diff.sources.removed, vec!["old-mcp"]);
        assert!(diff.sources.added.is_empty());
        // A pure removal grants nothing new, so no re-confirmation is forced.
        assert!(!diff.requires_reconfirmation());
        assert!(diff.grants_nothing_new());
    }

    #[test]
    fn identical_manifests_produce_an_empty_diff() {
        let m = manifest(
            bundle(&["lint"], &["/acme"], &["acme-mcp"], &["reviewer"]),
            vec!["p".to_string()],
        );
        let diff = CapabilityDiff::for_update(&m, &m);
        assert!(diff.is_empty());
        assert!(diff.grants_nothing_new());
        assert!(!diff.requires_reconfirmation());
    }

    #[test]
    fn delta_is_order_insensitive_and_deduplicated() {
        // Same set, different order, with a duplicate on each side -> empty delta.
        let delta = StringSetDelta::between(
            &["a".to_string(), "b".to_string(), "a".to_string()],
            &["b".to_string(), "a".to_string()],
        );
        assert!(
            delta.is_empty(),
            "reordered/duplicated identical sets must diff empty"
        );

        // Added entry surfaces exactly once even if listed twice.
        let delta = StringSetDelta::between(
            &["a".to_string()],
            &["a".to_string(), "c".to_string(), "c".to_string()],
        );
        assert_eq!(delta.added, vec!["c"]);
    }

    #[test]
    fn permission_only_update_does_not_force_reconfirmation() {
        let prev = manifest(bundle(&[], &[], &[], &[]), vec!["read".to_string()]);
        let next = manifest(
            bundle(&[], &[], &[], &[]),
            vec!["read".to_string(), "write".to_string()],
        );
        let diff = CapabilityDiff::for_update(&prev, &next);
        assert_eq!(diff.requested_permissions.added, vec!["write"]);
        assert!(!diff.requires_reconfirmation());
        assert!(!diff.grants_nothing_new());
    }
}
