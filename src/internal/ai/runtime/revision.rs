//! Cross-phase revision-chain helper (schema-only landing).
//!
//! Phase 0 (Intent), Phase 1 (Plan) and Phase 2 (Execution) all participate
//! in **revision chains**: every modification produces a new immutable
//! revision rather than overwriting the previous one, so the formal history
//! stays append-only and downstream verifiers / observers can reconstruct
//! the decision path.
//!
//! This module hosts the shared helpers for that chain — currently
//! schema-only — so the eventual implementations can sit beside the data
//! types instead of being scattered across `orchestrator/`, `intentspec/`
//! and `runtime/phase{0,1,2}.rs`.
//!
//! # Schema vs. wiring
//!
//! The current revision-chain logic is implicit:
//!
//! - `intentspec::resolve_intentspec` is invoked on every draft to produce
//!   a fresh `IntentSpec`; downstream code passes the new spec into
//!   [`super::phase0::write_intent`] and a new persisted Intent revision
//!   is created.
//! - `orchestrator::persistence::ExecutionAuditSession::record_plan_compiled`
//!   either reuses an existing preview plan id (when revision 1) or calls
//!   `create_plan_set_revision`, threading `parent_execution_plan_id` /
//!   `parent_test_plan_id` to keep the chain explicit.
//!
//! What's missing is a **shared** helper that captures the rules below
//! (per [`docs/improvement/agent.md`](../../../../../docs/improvement/agent.md)
//! Part B revision chain section):
//!
//! 1. `Modify Plan` requests must not edit `Plan` / `Task` in place; they
//!    must derive a new revision skeleton from the previous one.
//! 2. `step_id` values are stable across plan revisions when the step's
//!    intent is unchanged, so observers can correlate metrics across
//!    revisions.
//! 3. `plan` and `test-plan` always rev together — the chain must enforce
//!    that the (n)-th execution-plan revision pairs with the (n)-th
//!    test-plan revision, never (n−1) or (n+1).
//!
//! Once these rules graduate from prose to code (`handle_modify_request()`
//! + `derive_next_revision_skeleton()`), they will land in this module.

use uuid::Uuid;

/// Identifies the kind of revision chain a modify request walks.
///
/// Each variant maps to a distinct AI object family in the formal history:
/// Intent ↔ `git-internal Intent`, ExecutionPlan ↔ persisted plan
/// revisions with `role = "execution"`, TestPlan ↔ same family with
/// `role = "test"`. Keeping the discriminator on the entry point means
/// downstream helpers can switch on a single value rather than re-deriving
/// the chain kind from request shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevisionKind {
    Intent,
    ExecutionPlan,
    TestPlan,
}

impl RevisionKind {
    /// Stable label used in audit / log lines so a future grep pipeline can
    /// correlate revision events across phases.
    pub fn label(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::ExecutionPlan => "execution_plan",
            Self::TestPlan => "test_plan",
        }
    }
}

/// The parent reference and ordinal of a new revision in a chain.
///
/// `previous_id` is the persisted id of the immediately-preceding revision
/// (or `None` for the first link in a chain); `revision` is the 1-based
/// ordinal so the (n)-th plan revision can be paired with the (n)-th
/// test-plan revision per the cross-phase rule.
///
/// **Stability contract:** field names are part of the public Runtime
/// surface; once `handle_modify_request()` ships, downstream observers will
/// key off `previous_id` / `revision`. New fields may be added as
/// `Option<...>`; existing fields cannot be renamed or removed without a
/// parallel deprecation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionChainEntry {
    pub kind: RevisionKind,
    pub previous_id: Option<String>,
    pub revision: u32,
    /// Logical entity id (e.g. `task_id` for plan / test-plan revisions).
    /// Stable across revisions of the same chain — observers correlate
    /// time-series metrics by this value.
    pub logical_id: Uuid,
}

impl RevisionChainEntry {
    /// `true` for the first link in a chain (no `previous_id`).
    pub fn is_first(&self) -> bool {
        self.previous_id.is_none()
    }

    /// `true` when this entry is a continuation (rev > 1) of an existing
    /// chain. Helpers like `handle_modify_request` will branch on this to
    /// either create the first revision or derive a skeleton from the
    /// `previous_id` link.
    pub fn is_continuation(&self) -> bool {
        self.revision > 1 && self.previous_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Labels must be stable so audit consumers can grep across phases.
    #[test]
    fn revision_kind_labels_are_stable() {
        assert_eq!(RevisionKind::Intent.label(), "intent");
        assert_eq!(RevisionKind::ExecutionPlan.label(), "execution_plan");
        assert_eq!(RevisionKind::TestPlan.label(), "test_plan");
    }

    /// `is_first()` and `is_continuation()` are mutually exclusive on
    /// well-formed chains: revision 1 + no parent is "first", revision >=2
    /// + parent set is "continuation". Tests both directions plus the
    ///   degenerate case (revision 1 with a parent — represents a forced
    ///   re-derive and is NOT continuation by our convention).
    #[test]
    fn first_and_continuation_flag_chain_position_correctly() {
        let logical_id = Uuid::new_v4();

        let first = RevisionChainEntry {
            kind: RevisionKind::Intent,
            previous_id: None,
            revision: 1,
            logical_id,
        };
        assert!(first.is_first());
        assert!(!first.is_continuation());

        let continuation = RevisionChainEntry {
            kind: RevisionKind::ExecutionPlan,
            previous_id: Some("plan-prev".to_string()),
            revision: 2,
            logical_id,
        };
        assert!(!continuation.is_first());
        assert!(continuation.is_continuation());

        // Degenerate: revision 1 with a parent set — neither flag fires
        // continuation, so the caller can branch into a "first link of a
        // forked chain" code path.
        let forked = RevisionChainEntry {
            kind: RevisionKind::TestPlan,
            previous_id: Some("plan-prev".to_string()),
            revision: 1,
            logical_id,
        };
        assert!(!forked.is_first());
        assert!(!forked.is_continuation());
    }

    /// `RevisionChainEntry` must derive `Clone` so observer / audit
    /// handlers can keep a snapshot while the caller continues mutating
    /// the chain head.
    #[test]
    fn entry_is_clone() {
        let entry = RevisionChainEntry {
            kind: RevisionKind::Intent,
            previous_id: Some("intent-prev".to_string()),
            revision: 3,
            logical_id: Uuid::new_v4(),
        };
        let cloned = entry.clone();
        assert_eq!(cloned, entry);
    }
}
