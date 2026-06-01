//! Capability package subsystem (CEX-S2-17, Step 2.7).
//!
//! Bundles skills / commands / sources / sub-agent definitions into an
//! auditable, versioned package so ecosystem extensions stay inside the Source
//! Pool and permission model. This module currently provides the pure,
//! serde-frozen [`manifest`] schema; installer / trust-diff runtime lands
//! separately (see `docs/improvement/agent.md` Step 2.7).

pub mod manifest;

pub use manifest::{BundledCapabilities, CapabilityPackageManifest};
