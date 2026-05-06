//! `AgentPatchSet[S]` snapshot — sub-agent output staged in the isolated
//! workspace; never applied to the main worktree without a `MergeDecision`.
//!
//! Per S2-INV-03, sub-agent patches must NOT touch the main worktree until
//! Layer 1 issues an `accept` decision. This wrapper carries the `PatchSet`
//! id reference plus sub-agent provenance fields (the actual diff bytes stay
//! in the upstream `git_internal::internal::object::patchset::PatchSet`).

#![cfg(feature = "subagent-scaffold")]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AgentPatchSetId, AgentRunId};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPatchSet {
    pub id: AgentPatchSetId,

    /// Owning sub-agent run.
    pub agent_run_id: AgentRunId,

    /// Underlying persistent `PatchSet` snapshot id (held in the AI orphan
    /// branch; loadable via `git_internal::internal::object::patchset::PatchSet`).
    /// We reference rather than copy so we do not fork the patch schema.
    pub patchset_id: Uuid,

    /// Whether this patch is restricted to the workspace materialization
    /// scope (CEX-S2-11). `true` when sparse / blocked path was used; merge
    /// review must double-check write scope.
    #[serde(default)]
    pub workspace_scope_constrained: bool,
}
