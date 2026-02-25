//! Deprecated compatibility shim for the legacy `ai::agents` module.
//!
//! This module is kept to reduce breakage while migrating callers to
//! `ai::agent::profile`.
#[allow(deprecated)]
pub mod parser;

#[allow(deprecated)]
pub mod router;

pub use crate::internal::ai::agent::profile::{
    AgentProfile as AgentDefinition,
    AgentProfileRouter as AgentRouter,
    load_embedded_profiles as load_embedded_agents,
    load_profiles as load_agents,
    parse_agent_profile,
    AgentProfile,
};

