//! Capability package subsystem (CEX-S2-17, Step 2.7).
//!
//! Bundles skills / commands / sources / sub-agent definitions into an
//! auditable, versioned package so ecosystem extensions stay inside the Source
//! Pool and permission model. This module currently provides the pure,
//! serde-frozen [`manifest`] schema; installer / trust-diff runtime lands
//! separately (see `docs/development/commands/agent.md` Step 2.7). The pure
//! install/update capability [`diff`] computation also lives here; rendering it
//! and driving the confirmation prompt are runtime concerns elsewhere.

pub mod checksum;
pub mod diff;
pub mod installer;
pub mod manifest;
pub mod registry;
pub mod store;

pub use checksum::{
    ChecksumError, checksum_changed, compute_package_checksum, verify_against_manifest,
};
pub use diff::{CapabilityDiff, StringSetDelta};
pub use installer::{
    InstallDecision, InstallError, LoadedPackage, MANIFEST_FILE, load_package_dir, prepare_install,
};
pub use manifest::{BundledCapabilities, CapabilityPackageManifest, ManifestValidationError};
pub use registry::{
    ActiveCapabilities, ActiveCapabilitiesDelta, InstalledPackage, active_capabilities,
    may_auto_enable,
};
pub use store::InstalledPackageStore;
