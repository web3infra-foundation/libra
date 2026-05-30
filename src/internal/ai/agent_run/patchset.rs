//! `AgentPatchSet[S]` snapshot — sub-agent output staged in the isolated
//! workspace; never applied to the main worktree without a `MergeDecision`.
//!
//! Per S2-INV-03, sub-agent patches must NOT touch the main worktree until
//! Layer 1 issues an `accept` decision. This wrapper carries the `PatchSet`
//! id reference plus sub-agent provenance fields (the actual diff bytes stay
//! in the upstream `git_internal::internal::object::patchset::PatchSet`).

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

#[cfg(test)]
mod tests {
    use super::*;

    /// CEX-S2-10 freezes the `AgentPatchSet` wire contract
    /// (`#[serde(deny_unknown_fields)]`). Pin the required field set, the
    /// `#[serde(default)]` behaviour of `workspace_scope_constrained` (an
    /// omitted bool reads back `false`), and the `deny_unknown_fields`
    /// rejection — so silent schema drift in persisted patch records is
    /// caught here.
    #[test]
    fn agent_patchset_wire_contract_is_frozen() {
        let patchset = AgentPatchSet {
            id: AgentPatchSetId::new(),
            agent_run_id: AgentRunId::new(),
            patchset_id: Uuid::new_v4(),
            workspace_scope_constrained: true,
        };
        let json = serde_json::to_value(&patchset).expect("serialize AgentPatchSet");
        let obj = json
            .as_object()
            .expect("AgentPatchSet serializes to an object");
        for key in [
            "id",
            "agent_run_id",
            "patchset_id",
            "workspace_scope_constrained",
        ] {
            assert!(
                obj.contains_key(key),
                "AgentPatchSet must serialize `{key}`"
            );
        }
        assert_eq!(json["workspace_scope_constrained"], true);

        // `#[serde(default)]`: an omitted `workspace_scope_constrained`
        // reads back `false` rather than failing.
        let mut without_default = obj.clone();
        without_default.remove("workspace_scope_constrained");
        let parsed: AgentPatchSet =
            serde_json::from_value(serde_json::Value::Object(without_default))
                .expect("deserialize without the defaulted bool");
        assert!(
            !parsed.workspace_scope_constrained,
            "an omitted #[serde(default)] bool must default to false",
        );

        // deny_unknown_fields: an unknown field is rejected on read.
        let mut with_extra = obj.clone();
        with_extra.insert("bogus".to_string(), serde_json::Value::Bool(true));
        assert!(
            serde_json::from_value::<AgentPatchSet>(serde_json::Value::Object(with_extra)).is_err(),
            "deny_unknown_fields must reject an unknown field",
        );

        // Round-trip: the full wire shape deserializes and re-serializes
        // intact (parity with the AgentTask pin).
        let back: AgentPatchSet =
            serde_json::from_value(json.clone()).expect("deserialize AgentPatchSet");
        assert_eq!(
            serde_json::to_value(&back).expect("re-serialize"),
            json,
            "AgentPatchSet must round-trip its wire shape",
        );
    }
}
