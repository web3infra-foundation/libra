//! `AgentPermissionProfile[S]` snapshot.
//!
//! Per S2-INV-05 sub-agent tool policy is **default deny**; every Worker /
//! Explorer / Reviewer agent must declare its allowed tools, sources, and
//! approval mode explicitly. The exact `ApprovalKey` shape is owned by Step
//! 1.6 (Approval TTL) — until that lands, this profile uses a forward-stable
//! placeholder.

#![cfg(feature = "subagent-scaffold")]

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Approval policy scope for a sub-agent.
///
/// The full `ApprovalKey` (with `sensitivity_tier` / `scope` / `blast_radius`
/// fields) is frozen by Step 1.6. The variants here only express *who* the
/// approval prompts go to; field-level scope plumbing arrives later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRouting {
    /// All sub-agent approvals route to Layer 1 / human reviewer.
    /// Default for all agent types per S2-INV-06.
    Layer1Human,
    /// Pre-approved for the duration of this `AgentRun` (used for read-only
    /// agents like Explorer).
    SessionPreApproved,
}

/// Permission profile attached to an `AgentRun`. Default deny: an empty
/// profile authorizes nothing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPermissionProfile {
    /// Tool names the sub-agent may call (subset of registered tools).
    /// Default empty = deny everything.
    #[serde(default)]
    pub allowed_tools: BTreeSet<String>,

    /// Tool names the sub-agent must NOT call even if `allowed_tools` would
    /// transitively cover them (used for hard-coded denies like `spawn_*`).
    #[serde(default)]
    pub denied_tools: BTreeSet<String>,

    /// MCP / Source Pool slugs the sub-agent may read from. Per CEX-S2-10,
    /// the slug namespace itself is owned by Step 1.10.
    #[serde(default)]
    pub allowed_source_slugs: BTreeSet<String>,

    /// Where approval prompts route.
    pub approval_routing: ApprovalRouting,

    /// Whether this sub-agent may spawn further sub-agents. Per S2-INV-09
    /// this is `false` by default; Layer 1 is the only legitimate spawner.
    #[serde(default)]
    pub may_spawn_sub_agents: bool,
}

impl Default for AgentPermissionProfile {
    fn default() -> Self {
        Self {
            allowed_tools: BTreeSet::new(),
            denied_tools: BTreeSet::new(),
            allowed_source_slugs: BTreeSet::new(),
            approval_routing: ApprovalRouting::Layer1Human,
            may_spawn_sub_agents: false,
        }
    }
}
