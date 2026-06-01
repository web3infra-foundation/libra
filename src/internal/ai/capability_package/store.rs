//! CEX-S2-17 installed-package persistence.
//!
//! Persists the set of installed capability packages to
//! `<dot_libra>/capability_packages.json` so `libra package list` can report
//! them and uninstall can find what to tear down (验收 (5)). The pure
//! effective-capability computation lives in [`super::registry`]; this is the
//! thin on-disk store the installer reads and writes.
//!
//! An absent store loads as an empty list (nothing installed yet); a present
//! but corrupt store surfaces an `io::Error` naming the path so the operator can
//! fix it rather than silently losing the install set.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::registry::InstalledPackage;

/// File name under the repo's `.libra` directory holding the installed-package
/// set as a pretty-printed JSON array.
const STORE_FILE: &str = "capability_packages.json";

/// On-disk store for the installed capability-package set.
pub struct InstalledPackageStore {
    path: PathBuf,
}

impl InstalledPackageStore {
    /// Bind a store to `<dot_libra>/capability_packages.json`.
    pub fn new(dot_libra: impl AsRef<Path>) -> Self {
        Self {
            path: dot_libra.as_ref().join(STORE_FILE),
        }
    }

    /// The backing file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the installed packages. An absent store is an empty list (no error);
    /// a present-but-unparseable store is an `InvalidData` error naming the path.
    pub fn load(&self) -> io::Result<Vec<InstalledPackage>> {
        match fs::read_to_string(&self.path) {
            Ok(content) => serde_json::from_str(&content).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "corrupt capability-package store '{}': {err}",
                        self.path.display()
                    ),
                )
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(io::Error::new(
                err.kind(),
                format!(
                    "failed to read capability-package store '{}': {err}",
                    self.path.display()
                ),
            )),
        }
    }

    /// Persist the installed-package set as pretty JSON, creating the parent
    /// directory if needed.
    pub fn save(&self, packages: &[InstalledPackage]) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(packages).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to serialize capability-package store: {err}"),
            )
        })?;
        fs::write(&self.path, json)
    }

    /// Install or update a package: replace any existing entry with the same
    /// `package_id` (the update path — a new checksum/version supersedes the
    /// prior install) or append it. Returns `true` when an existing entry was
    /// replaced (an update) rather than a fresh install.
    pub fn upsert(&self, package: InstalledPackage) -> io::Result<bool> {
        let mut packages = self.load()?;
        let target = package.manifest.package_id.clone();
        let replaced = if let Some(existing) = packages
            .iter_mut()
            .find(|p| p.manifest.package_id == target)
        {
            *existing = package;
            true
        } else {
            packages.push(package);
            false
        };
        self.save(&packages)?;
        Ok(replaced)
    }

    /// Uninstall a package by id. Returns `true` when an entry was removed,
    /// `false` when no package with that id was installed.
    pub fn remove(&self, package_id: &str) -> io::Result<bool> {
        let mut packages = self.load()?;
        let before = packages.len();
        packages.retain(|p| p.manifest.package_id.0 != package_id);
        let removed = packages.len() != before;
        if removed {
            self.save(&packages)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::{
        agent_run::{PackageId, Sha256},
        capability_package::manifest::{BundledCapabilities, CapabilityPackageManifest},
    };

    fn package(id: &str, enabled: bool) -> InstalledPackage {
        InstalledPackage {
            manifest: CapabilityPackageManifest {
                package_id: PackageId(id.to_string()),
                version: "1.0.0".to_string(),
                publisher: "acme".to_string(),
                checksum: Sha256("a".repeat(64)),
                bundled: BundledCapabilities {
                    sources: vec![format!("{id}-source")],
                    ..BundledCapabilities::default()
                },
                requested_permissions: Vec::new(),
                install_warnings: Vec::new(),
            },
            enabled,
        }
    }

    #[test]
    fn absent_store_loads_as_empty() {
        let temp = tempfile::tempdir().unwrap();
        let store = InstalledPackageStore::new(temp.path());
        assert!(store.load().expect("absent store is empty").is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let temp = tempfile::tempdir().unwrap();
        let store = InstalledPackageStore::new(temp.path());
        let packages = vec![package("acme.a", true), package("acme.b", false)];
        store.save(&packages).expect("save");
        assert_eq!(store.load().expect("load"), packages);
    }

    #[test]
    fn upsert_appends_then_replaces_same_id() {
        let temp = tempfile::tempdir().unwrap();
        let store = InstalledPackageStore::new(temp.path());

        // First install of an id: fresh (not a replacement).
        assert!(!store.upsert(package("acme.a", false)).expect("install"));
        assert_eq!(store.load().unwrap().len(), 1);

        // A different id appends.
        assert!(!store.upsert(package("acme.b", true)).expect("install b"));
        assert_eq!(store.load().unwrap().len(), 2);

        // Re-installing the same id replaces in place (the update path) and is
        // reported as a replacement, keeping the set de-duplicated by id.
        assert!(store.upsert(package("acme.a", true)).expect("update a"));
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 2, "update must not duplicate the id");
        let a = loaded
            .iter()
            .find(|p| p.manifest.package_id.0 == "acme.a")
            .expect("acme.a present");
        assert!(a.enabled, "the replacement value (enabled) wins");
    }

    #[test]
    fn remove_drops_by_id_and_reports_outcome() {
        let temp = tempfile::tempdir().unwrap();
        let store = InstalledPackageStore::new(temp.path());
        store
            .save(&[package("acme.a", true), package("acme.b", true)])
            .unwrap();

        assert!(store.remove("acme.a").expect("remove a"));
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].manifest.package_id.0, "acme.b");

        // Removing an unknown id is a no-op reported as `false`.
        assert!(!store.remove("acme.missing").expect("remove missing"));
        assert_eq!(store.load().unwrap().len(), 1);
    }

    #[test]
    fn corrupt_store_is_an_actionable_error() {
        let temp = tempfile::tempdir().unwrap();
        let store = InstalledPackageStore::new(temp.path());
        fs::write(store.path(), b"not json").unwrap();
        let err = store.load().expect_err("corrupt store must error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("corrupt capability-package store"),
            "error must name the problem: {err}",
        );
    }
}
