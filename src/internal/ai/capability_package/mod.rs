//! Capability package subsystem (CEX-S2-17, Step 2.7).
//!
//! Bundles skills / commands / sources / sub-agent definitions into an
//! auditable, versioned package so ecosystem extensions stay inside the Source
//! Pool and permission model. This module currently provides the pure,
//! serde-frozen [`manifest`] schema; installer / trust-diff runtime lands
//! separately (see `docs/improvement/agent.md` Step 2.7). The pure
//! install/update capability [`diff`] computation also lives here; rendering it
//! and driving the confirmation prompt are runtime concerns elsewhere.

pub mod diff;
pub mod manifest;
pub mod registry;

pub use diff::{CapabilityDiff, StringSetDelta};
pub use manifest::{BundledCapabilities, CapabilityPackageManifest, ManifestValidationError};
pub use registry::{ActiveCapabilities, InstalledPackage, active_capabilities};
