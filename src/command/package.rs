//! `libra package` — capability-package install / list / diff (CEX-S2-17).
//!
//! A capability package bundles skills / commands / sources / sub-agent
//! definitions into an auditable, checksum-verified unit. `install` vets a local
//! package directory (validating the manifest, verifying the content checksum,
//! computing the capability diff, and applying default-deny confirmation) and
//! records it in the per-repo store; `list` shows the installed set; `diff`
//! previews what a package would grant without installing it.
//!
//! Registering the bundled capabilities into a live session happens at session
//! startup from the persisted store — `install` here only **vets and records**
//! (default-deny: installed-but-disabled unless `--enable`).

use std::path::{Path, PathBuf};

use clap::Subcommand;

use crate::{
    internal::ai::capability_package::{
        CapabilityDiff, InstalledPackage, InstalledPackageStore, active_capabilities,
        load_package_dir, prepare_install,
    },
    utils::{
        error::{CliError, CliResult},
        output::OutputConfig,
        util,
    },
};

/// `--help` EXAMPLES block (compat: every visible command renders one).
pub const PACKAGE_EXAMPLES: &str = "\
EXAMPLES:
    libra package list                          List installed capability packages
    libra package diff ./my-package             Preview the capabilities a package would grant
    libra package install ./my-package          Vet + record a package (prints the capability diff)
    libra package install ./my-package --yes    Accept the capability diff without prompting
    libra package install ./my-package --enable Record AND enable (default-deny installs disabled)
    libra package uninstall acme.toolkit        Remove a recorded package by id";

#[derive(Subcommand, Debug)]
pub enum PackageCmds {
    /// List installed capability packages and their enabled state.
    List,
    /// Preview the capability diff a local package would grant, without installing.
    Diff {
        /// Path to the package directory (containing `manifest.json`).
        path: PathBuf,
    },
    /// Vet and record a local capability package (default-deny: disabled unless `--enable`).
    Install {
        /// Path to the package directory (containing `manifest.json`).
        path: PathBuf,
        /// Accept the capability diff without an interactive confirmation.
        #[arg(long)]
        yes: bool,
        /// Enable the package immediately rather than leaving it installed-but-disabled.
        #[arg(long)]
        enable: bool,
    },
    /// Uninstall a recorded package by id (its capabilities drop out at next session start).
    Uninstall {
        /// The `package_id` to remove (see `libra package list`).
        package_id: String,
    },
}

/// CLI entry point: print errors to stderr and exit non-zero on failure.
pub async fn execute(command: PackageCmds) {
    if let Err(error) = execute_safe(command, &OutputConfig::default()).await {
        error.print_stderr();
    }
}

/// Structured entry point returning [`CliResult`] instead of exiting.
pub async fn execute_safe(command: PackageCmds, _output: &OutputConfig) -> CliResult<()> {
    let dot_libra = util::try_get_storage_path(None).map_err(|err| {
        CliError::fatal(format!(
            "`libra package` must run inside a Libra repository: {err}"
        ))
    })?;
    let store = InstalledPackageStore::new(&dot_libra);

    match command {
        PackageCmds::List => run_list(&store),
        PackageCmds::Diff { path } => run_diff(&path),
        PackageCmds::Install { path, yes, enable } => run_install(&store, &path, yes, enable),
        PackageCmds::Uninstall { package_id } => run_uninstall(&store, &package_id),
    }
}

fn run_uninstall(store: &InstalledPackageStore, package_id: &str) -> CliResult<()> {
    let removed = store
        .remove(package_id)
        .map_err(|err| CliError::fatal(format!("failed to update installed packages: {err}")))?;
    if removed {
        println!(
            "Uninstalled capability package `{package_id}`. \
             Its bundled capabilities drop out at the next session start."
        );
    } else {
        println!("No installed capability package matches `{package_id}`.");
    }
    Ok(())
}

fn run_list(store: &InstalledPackageStore) -> CliResult<()> {
    let installed = store
        .load()
        .map_err(|err| CliError::fatal(format!("failed to read installed packages: {err}")))?;
    if installed.is_empty() {
        println!("No capability packages installed.");
        return Ok(());
    }
    println!("Installed capability packages:");
    for package in &installed {
        let manifest = &package.manifest;
        let state = if package.enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!(
            "  {:<28} {:<10} {:<9} (publisher: {})",
            manifest.package_id.0, manifest.version, state, manifest.publisher,
        );
    }
    // Show the *effective* capability set — the union contributed by ENABLED
    // packages only, which is what a session registers (CEX-S2-17: "未启用的
    // package 不会注册"). A disabled package above contributes nothing here.
    if let Some(summary) = effective_capability_summary(&installed) {
        print!("{summary}");
    }
    Ok(())
}

/// Render the effective active capability set — the de-duplicated union of every
/// **enabled** package's bundles ([`active_capabilities`]) — as an indented
/// `category: a, b, c` block. Returns `None` when nothing is enabled (so the
/// caller prints no "Active capabilities" header for a disabled-only install
/// set). Pure — no I/O.
fn effective_capability_summary(installed: &[InstalledPackage]) -> Option<String> {
    let active = active_capabilities(installed);
    if active.is_empty() {
        return None;
    }
    let mut out = String::from("Effective active capabilities (enabled packages only):\n");
    let categories = [
        ("skills", &active.skills),
        ("commands", &active.commands),
        ("sources", &active.sources),
        ("sub-agents", &active.sub_agents),
    ];
    for (label, set) in categories {
        if !set.is_empty() {
            let joined = set.iter().cloned().collect::<Vec<_>>().join(", ");
            out.push_str(&format!("  {label}: {joined}\n"));
        }
    }
    Some(out)
}

fn run_diff(path: &Path) -> CliResult<()> {
    let loaded = load_package_dir(path).map_err(|err| {
        CliError::fatal(format!(
            "failed to load package '{}': {err}",
            path.display()
        ))
    })?;
    let diff = CapabilityDiff::for_install(&loaded.manifest);
    println!(
        "Package `{}` v{} (publisher: {}) would grant:",
        loaded.manifest.package_id.0, loaded.manifest.version, loaded.manifest.publisher,
    );
    print!("{}", render_diff(&diff));
    if diff.requires_reconfirmation() {
        println!(
            "  ! bundles a new mutating capability (source / sub-agent) — install requires confirmation"
        );
    }
    for warning in &loaded.manifest.install_warnings {
        println!("  ! warning: {warning}");
    }
    Ok(())
}

fn run_install(
    store: &InstalledPackageStore,
    path: &Path,
    yes: bool,
    enable: bool,
) -> CliResult<()> {
    let loaded = load_package_dir(path).map_err(|err| {
        CliError::fatal(format!(
            "failed to load package '{}': {err}",
            path.display()
        ))
    })?;
    let installed = store
        .load()
        .map_err(|err| CliError::fatal(format!("failed to read installed packages: {err}")))?;

    let decision = prepare_install(loaded.manifest, &loaded.entries, &installed)
        .map_err(|err| CliError::fatal(err.to_string()))?;

    let id = decision.package.manifest.package_id.0.clone();
    let verb = if decision.is_update {
        "Update"
    } else {
        "Install"
    };
    println!(
        "{verb} `{}` v{} would grant:",
        id, decision.package.manifest.version,
    );
    print!("{}", render_diff(&decision.diff));
    for warning in &decision.warnings {
        println!("  ! warning: {warning}");
    }

    // Default-deny: a new mutating capability (or a changed-checksum update)
    // must be explicitly accepted before anything is recorded.
    if decision.requires_confirmation && !yes {
        println!(
            "This package requires confirmation (new mutating capability or changed content). \
             Re-run with --yes to accept."
        );
        return Ok(());
    }

    let mut package = decision.package;
    package.enabled = enable;
    let replaced = store
        .upsert(package)
        .map_err(|err| CliError::fatal(format!("failed to record installed package: {err}")))?;

    let state = if enable {
        "enabled"
    } else {
        "installed (disabled — enable with `libra package install … --enable`)"
    };
    let action = if replaced { "Updated" } else { "Recorded" };
    println!("{action} capability package `{id}`: {state}.");
    Ok(())
}

/// Render a [`CapabilityDiff`] as an indented per-category added/removed list.
/// Empty categories are omitted; a wholly-empty diff renders a single line.
fn render_diff(diff: &CapabilityDiff) -> String {
    let mut out = String::new();
    let categories = [
        ("skills", &diff.skills),
        ("commands", &diff.commands),
        ("sources", &diff.sources),
        ("sub-agents", &diff.sub_agents),
        ("permissions", &diff.requested_permissions),
    ];
    for (label, delta) in categories {
        for added in &delta.added {
            out.push_str(&format!("  + {label}: {added}\n"));
        }
        for removed in &delta.removed {
            out.push_str(&format!("  - {label}: {removed}\n"));
        }
    }
    if out.is_empty() {
        out.push_str("  (no capability changes)\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::{
        agent_run::{PackageId, Sha256},
        capability_package::{BundledCapabilities, CapabilityPackageManifest},
    };

    fn package(id: &str, skill: &str, source: &str, enabled: bool) -> InstalledPackage {
        InstalledPackage {
            manifest: CapabilityPackageManifest {
                package_id: PackageId(id.to_string()),
                version: "1.0.0".to_string(),
                publisher: "acme".to_string(),
                checksum: Sha256("a".repeat(64)),
                bundled: BundledCapabilities {
                    skills: vec![skill.to_string()],
                    sources: vec![source.to_string()],
                    ..BundledCapabilities::default()
                },
                requested_permissions: Vec::new(),
                install_warnings: Vec::new(),
            },
            enabled,
        }
    }

    #[test]
    fn effective_summary_is_none_when_nothing_enabled() {
        // An empty set, and an installed-but-disabled-only set, both register
        // nothing — so the summary is omitted entirely.
        assert!(effective_capability_summary(&[]).is_none());
        let disabled = vec![package("acme.a", "explore", "src-a", false)];
        assert!(
            effective_capability_summary(&disabled).is_none(),
            "a disabled package must contribute no effective capability",
        );
    }

    #[test]
    fn effective_summary_unions_only_enabled_packages() {
        let installed = vec![
            package("acme.a", "explore", "src-a", true),
            package("acme.b", "review", "src-b", false), // disabled: excluded
            package("acme.c", "build", "src-c", true),
        ];
        let summary = effective_capability_summary(&installed).expect("some enabled");
        // Enabled packages' capabilities appear...
        assert!(summary.contains("explore"), "{summary}");
        assert!(summary.contains("build"), "{summary}");
        assert!(
            summary.contains("src-a") && summary.contains("src-c"),
            "{summary}"
        );
        // ...the disabled package's do NOT (CEX-S2-17: disabled registers nothing).
        assert!(
            !summary.contains("review"),
            "disabled skill leaked: {summary}"
        );
        assert!(
            !summary.contains("src-b"),
            "disabled source leaked: {summary}"
        );
    }
}
