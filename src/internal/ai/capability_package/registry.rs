//! Installed capability-package registry model (CEX-S2-17, Step 2.7).
//!
//! Tracks which capability packages are installed and which are enabled, and
//! computes the **effective** capability set — the union of the bundles of every
//! *enabled* package. This is the pure core of two CEX-S2-17 acceptance
//! criteria:
//!
//! - "未启用的 package 不会注册 tools / sources / skills / agents" — a disabled
//!   (or uninstalled) package contributes nothing to the effective set.
//! - "package 卸载后相关 tool definitions / source connections / agent
//!   definitions 消失" — removing a package from the registry drops its
//!   contributions from the effective set.
//!
//! Performing the actual tool/source/agent registration and teardown against the
//! live runtime is the installer's job; this module decides *what* should be
//! registered, deterministically and without I/O, so the installer has one
//! authority to diff the runtime against.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    diff::StringSetDelta,
    manifest::{BundledCapabilities, CapabilityPackageManifest},
};

/// One installed package plus whether it is currently enabled. A package can be
/// installed-but-disabled; only enabled packages register their capabilities.
///
/// `Serialize` / `Deserialize` so the installer can persist the installed set
/// via [`super::store::InstalledPackageStore`]; `enabled` defaults to `false`
/// (default-deny) when a hand-edited or older store omits it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledPackage {
    /// The package's manifest (carries id / version / bundles).
    pub manifest: CapabilityPackageManifest,
    /// Whether the package is enabled. Default-deny: an installed package that
    /// is not explicitly enabled registers nothing.
    #[serde(default)]
    pub enabled: bool,
}

/// The de-duplicated, sorted capability set contributed by the enabled
/// packages — what the runtime should have registered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActiveCapabilities {
    /// Active skill names.
    pub skills: BTreeSet<String>,
    /// Active slash-command names.
    pub commands: BTreeSet<String>,
    /// Active Source Pool source / MCP slugs.
    pub sources: BTreeSet<String>,
    /// Active sub-agent definition names.
    pub sub_agents: BTreeSet<String>,
}

impl ActiveCapabilities {
    /// `true` when nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
            && self.commands.is_empty()
            && self.sources.is_empty()
            && self.sub_agents.is_empty()
    }

    fn merge(&mut self, bundle: &BundledCapabilities) {
        self.skills.extend(bundle.skills.iter().cloned());
        self.commands.extend(bundle.commands.iter().cloned());
        self.sources.extend(bundle.sources.iter().cloned());
        self.sub_agents.extend(bundle.sub_agents.iter().cloned());
    }

    /// Compute the per-category register / teardown plan for moving the live
    /// runtime FROM the `previous` effective set TO `self`. For each category,
    /// `added` capabilities must be registered and `removed` capabilities torn
    /// down (CEX-S2-17 应该完成的功能: "package 卸载后相关 tool definitions /
    /// source connections / agent definitions 消失").
    ///
    /// Because both operands are the **effective** (unioned, de-duplicated) sets
    /// produced by [`active_capabilities`], the teardown is **overlap-safe**: a
    /// capability still provided by another enabled package after one package is
    /// uninstalled / disabled stays in the effective set and is therefore *not*
    /// reported as `removed`. A naive per-package teardown would incorrectly drop
    /// such shared capabilities; diffing effective sets is the correct authority.
    ///
    /// Pure; no I/O. The installer applies the returned plan against the live
    /// tool / source / skill / agent registries.
    pub fn delta_since(&self, previous: &ActiveCapabilities) -> ActiveCapabilitiesDelta {
        ActiveCapabilitiesDelta {
            skills: StringSetDelta::between_sets(&previous.skills, &self.skills),
            commands: StringSetDelta::between_sets(&previous.commands, &self.commands),
            sources: StringSetDelta::between_sets(&previous.sources, &self.sources),
            sub_agents: StringSetDelta::between_sets(&previous.sub_agents, &self.sub_agents),
        }
    }
}

/// The per-category change in the **effective** active capability set when the
/// installed / enabled package set changes. For each category, `added` entries
/// must be registered against the live runtime and `removed` entries torn down.
/// Produced by [`ActiveCapabilities::delta_since`]; the teardown side is
/// overlap-safe (see that method).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActiveCapabilitiesDelta {
    /// Skill-name register/teardown delta.
    pub skills: StringSetDelta,
    /// Slash-command register/teardown delta.
    pub commands: StringSetDelta,
    /// Source / MCP-slug register/teardown delta.
    pub sources: StringSetDelta,
    /// Sub-agent-definition register/teardown delta.
    pub sub_agents: StringSetDelta,
}

impl ActiveCapabilitiesDelta {
    /// `true` when nothing needs to be registered or torn down in any category.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
            && self.commands.is_empty()
            && self.sources.is_empty()
            && self.sub_agents.is_empty()
    }
}

/// Compute the [`ActiveCapabilities`] contributed by the **enabled** packages in
/// `installed`. Disabled (or absent) packages contribute nothing.
///
/// Pure and order-insensitive: the result depends only on the set of enabled
/// packages' bundles, de-duplicated. Removing a package from `installed` (or
/// flipping it to `enabled = false`) drops its contributions — the "uninstall /
/// disable makes capabilities disappear" criterion.
pub fn active_capabilities(installed: &[InstalledPackage]) -> ActiveCapabilities {
    let mut active = ActiveCapabilities::default();
    for package in installed.iter().filter(|p| p.enabled) {
        active.merge(&package.manifest.bundled);
    }
    active
}

/// Whether a package may be **auto-enabled** at install time, or must instead
/// wait for explicit project-config / user confirmation (CEX-S2-17 应该完成的
///功能: "package 默认不能启用 mutating source 或 Worker sub-agent；必须由项目
/// 配置或用户确认").
///
/// A package that bundles a mutating capability — a Source Pool source / MCP
/// server, or a sub-agent definition — must **not** be auto-enabled: enabling it
/// would register a mutating source or a spawnable agent without a human in the
/// loop. A package bundling only skills / commands carries no mutating
/// capability and may be auto-enabled. Pure; no I/O.
pub fn may_auto_enable(manifest: &CapabilityPackageManifest) -> bool {
    let bundle = &manifest.bundled;
    bundle.sources.is_empty() && bundle.sub_agents.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::agent_run::{PackageId, Sha256};

    fn manifest(id: &str, bundled: BundledCapabilities) -> CapabilityPackageManifest {
        CapabilityPackageManifest {
            package_id: PackageId(id.to_string()),
            version: "1.0.0".to_string(),
            publisher: "acme".to_string(),
            checksum: Sha256("0".repeat(64)),
            bundled,
            requested_permissions: Vec::new(),
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

    fn installed(id: &str, bundled: BundledCapabilities, enabled: bool) -> InstalledPackage {
        InstalledPackage {
            manifest: manifest(id, bundled),
            enabled,
        }
    }

    #[test]
    fn no_packages_registers_nothing() {
        assert!(active_capabilities(&[]).is_empty());
    }

    #[test]
    fn disabled_package_registers_nothing() {
        // S2-INV / CEX-S2-17 验收 (1): an installed-but-disabled package
        // contributes no capabilities.
        let pkgs = vec![installed(
            "acme",
            bundle(&["lint"], &["/acme"], &["acme-mcp"], &["reviewer"]),
            false,
        )];
        assert!(
            active_capabilities(&pkgs).is_empty(),
            "a disabled package must register nothing",
        );
    }

    #[test]
    fn enabled_package_registers_its_bundle() {
        let pkgs = vec![installed(
            "acme",
            bundle(&["lint"], &["/acme"], &["acme-mcp"], &["reviewer"]),
            true,
        )];
        let active = active_capabilities(&pkgs);
        assert!(active.skills.contains("lint"));
        assert!(active.commands.contains("/acme"));
        assert!(active.sources.contains("acme-mcp"));
        assert!(active.sub_agents.contains("reviewer"));
    }

    #[test]
    fn only_enabled_packages_contribute() {
        let pkgs = vec![
            installed("on", bundle(&["a"], &[], &[], &[]), true),
            installed("off", bundle(&["b"], &[], &[], &[]), false),
        ];
        let active = active_capabilities(&pkgs);
        assert!(active.skills.contains("a"));
        assert!(
            !active.skills.contains("b"),
            "the disabled package's skill must not be registered",
        );
    }

    #[test]
    fn enabled_packages_union_and_dedup() {
        // Two enabled packages bundling an overlapping source slug -> one entry.
        let pkgs = vec![
            installed("p1", bundle(&["x"], &[], &["shared-mcp"], &[]), true),
            installed("p2", bundle(&["y"], &[], &["shared-mcp"], &[]), true),
        ];
        let active = active_capabilities(&pkgs);
        assert_eq!(
            active.skills,
            ["x", "y"].iter().map(|s| s.to_string()).collect(),
        );
        assert_eq!(
            active.sources,
            ["shared-mcp"].iter().map(|s| s.to_string()).collect(),
            "a slug bundled by two packages registers once",
        );
    }

    #[test]
    fn disabling_a_package_drops_its_capabilities() {
        // Models uninstall/disable: the same registry with the package flipped
        // off no longer registers its source.
        let enabled = vec![installed(
            "acme",
            bundle(&[], &[], &["acme-mcp"], &[]),
            true,
        )];
        assert!(active_capabilities(&enabled).sources.contains("acme-mcp"));

        let disabled = vec![installed(
            "acme",
            bundle(&[], &[], &["acme-mcp"], &[]),
            false,
        )];
        assert!(
            !active_capabilities(&disabled).sources.contains("acme-mcp"),
            "disabling the package must drop its source registration",
        );
    }

    #[test]
    fn delta_since_empty_reports_everything_as_added() {
        // Fresh install: moving from no active capabilities to a full set must
        // mark every entry as a registration (`added`), nothing torn down.
        let pkgs = vec![installed(
            "acme",
            bundle(&["lint"], &["/acme"], &["acme-mcp"], &["reviewer"]),
            true,
        )];
        let after = active_capabilities(&pkgs);
        let delta = after.delta_since(&ActiveCapabilities::default());
        assert_eq!(delta.skills.added, vec!["lint".to_string()]);
        assert_eq!(delta.commands.added, vec!["/acme".to_string()]);
        assert_eq!(delta.sources.added, vec!["acme-mcp".to_string()]);
        assert_eq!(delta.sub_agents.added, vec!["reviewer".to_string()]);
        assert!(delta.skills.removed.is_empty());
        assert!(delta.sources.removed.is_empty());
    }

    #[test]
    fn delta_since_to_empty_reports_everything_as_removed() {
        // Full uninstall: moving from a populated set to nothing must mark every
        // entry as a teardown (`removed`), nothing added.
        let before = active_capabilities(&[installed(
            "acme",
            bundle(&["lint"], &[], &["acme-mcp"], &["reviewer"]),
            true,
        )]);
        let delta = ActiveCapabilities::default().delta_since(&before);
        assert_eq!(delta.skills.removed, vec!["lint".to_string()]);
        assert_eq!(delta.sources.removed, vec!["acme-mcp".to_string()]);
        assert_eq!(delta.sub_agents.removed, vec!["reviewer".to_string()]);
        assert!(delta.skills.added.is_empty());
        assert!(delta.sources.added.is_empty());
    }

    #[test]
    fn delta_since_is_overlap_safe_for_shared_capabilities() {
        // Two enabled packages both provide `shared-mcp` and `common`. Disabling
        // ONE must NOT tear down those shared capabilities (the other still
        // provides them) — only the uninstalled package's *exclusive* `solo`
        // source is removed. This is the property a naive per-package teardown
        // gets wrong; diffing the effective (unioned) sets gets it right.
        let before = active_capabilities(&[
            installed(
                "p1",
                bundle(&["common"], &[], &["shared-mcp", "solo"], &[]),
                true,
            ),
            installed("p2", bundle(&["common"], &[], &["shared-mcp"], &[]), true),
        ]);
        // p1 uninstalled / disabled; p2 stays enabled.
        let after = active_capabilities(&[
            installed(
                "p1",
                bundle(&["common"], &[], &["shared-mcp", "solo"], &[]),
                false,
            ),
            installed("p2", bundle(&["common"], &[], &["shared-mcp"], &[]), true),
        ]);
        let delta = after.delta_since(&before);
        assert_eq!(
            delta.sources.removed,
            vec!["solo".to_string()],
            "only the exclusively-p1 source must be torn down",
        );
        assert!(
            !delta.sources.removed.contains(&"shared-mcp".to_string()),
            "a source still provided by an enabled package must NOT be torn down",
        );
        assert!(
            delta.skills.is_empty(),
            "`common` is still provided by p2, so the skill set is unchanged",
        );
        assert!(delta.sources.added.is_empty());
    }

    #[test]
    fn delta_since_identical_state_is_empty() {
        let active = active_capabilities(&[installed(
            "acme",
            bundle(&["lint"], &["/acme"], &[], &[]),
            true,
        )]);
        assert!(
            active.delta_since(&active).is_empty(),
            "no package-set change means no register/teardown work",
        );
    }

    #[test]
    fn skills_and_commands_only_may_auto_enable() {
        // No mutating capability -> safe to auto-enable.
        let m = manifest("safe", bundle(&["lint"], &["/acme"], &[], &[]));
        assert!(may_auto_enable(&m));
    }

    #[test]
    fn bundling_a_source_blocks_auto_enable() {
        // CEX-S2-17: a mutating source must not auto-enable.
        let m = manifest("mcp", bundle(&[], &[], &["acme-mcp"], &[]));
        assert!(
            !may_auto_enable(&m),
            "a package bundling a source must require confirmation",
        );
    }

    #[test]
    fn bundling_a_sub_agent_blocks_auto_enable() {
        let m = manifest("agent", bundle(&[], &[], &[], &["worker"]));
        assert!(
            !may_auto_enable(&m),
            "a package bundling a sub-agent must require confirmation",
        );
    }

    #[test]
    fn empty_package_may_auto_enable() {
        let m = manifest("empty", BundledCapabilities::default());
        assert!(may_auto_enable(&m));
    }
}
