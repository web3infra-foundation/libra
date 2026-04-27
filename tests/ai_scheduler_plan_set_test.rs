//! Phase 0 contract tests for selected plan set and task dependency rules.
//!
//! Pin the runtime invariants the scheduler relies on:
//! - A `SelectedPlanSet` always carries a fixed (execution_plan, test_plan) pair, in
//!   that order.
//! - `PlanSetWriteInput` round-trips through serde with both plan IDs preserved.
//! - `validate_same_plan_dependencies` accepts intra-plan deps and rejects
//!   cross-plan deps in the v1 contract.
//! - Stale projections (`StaleReadOnly`) cannot trigger Phase 4 auto-decisions.
//! - `FinalDecisionVerdict` discriminator strings are stable on the wire.
//!
//! **Layer:** L1 — pure unit tests, no I/O.

use libra::internal::ai::runtime::contracts::{
    FinalDecisionVerdict, PlanRevisionSource, PlanSetWriteInput, ProjectionFreshness,
    SelectedPlanSet, validate_same_plan_dependencies,
};
use uuid::Uuid;

/// Scenario: a `SelectedPlanSet` always exposes its plans in (execution, test)
/// order via `ordered_ids`. Pinning this is what lets downstream consumers index
/// without bookkeeping which plan is which.
#[test]
fn selected_plan_set_is_a_fixed_execution_test_pair() {
    let execution_plan_id = Uuid::new_v4();
    let test_plan_id = Uuid::new_v4();
    let selected = SelectedPlanSet {
        execution_plan_id,
        test_plan_id,
    };

    assert_eq!(selected.ordered_ids(), [execution_plan_id, test_plan_id]);
}

/// Scenario: serializing a `PlanSetWriteInput` whose execution and test sources are
/// both `Existing` produces a JSON shape carrying the right `plan_id` under each key.
/// Acts as a regression guard for the over-the-wire contract MCP clients depend on.
#[test]
fn plan_set_write_input_can_pass_through_one_existing_head() {
    let execution_plan_id = Uuid::new_v4();
    let test_plan_id = Uuid::new_v4();
    let input = PlanSetWriteInput {
        execution: PlanRevisionSource::Existing {
            plan_id: execution_plan_id,
        },
        test: PlanRevisionSource::Existing {
            plan_id: test_plan_id,
        },
    };

    let encoded = serde_json::to_value(&input).unwrap();
    assert_eq!(
        encoded["execution"]["plan_id"],
        serde_json::json!(execution_plan_id)
    );
    assert_eq!(encoded["test"]["plan_id"], serde_json::json!(test_plan_id));
}

/// Scenario: the v1 contract forbids cross-plan dependencies. The validator accepts
/// `[(first, []), (second, [first])]` (intra-plan dep) but rejects
/// `[(first, [external])]` where `external` is not in the same plan. Guards the rule
/// that lets the scheduler reason about a single plan's DAG in isolation.
#[test]
fn task_dependencies_are_same_plan_only_in_v1() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    validate_same_plan_dependencies(&[(first, vec![]), (second, vec![first])]).unwrap();

    let external = Uuid::new_v4();
    assert!(validate_same_plan_dependencies(&[(first, vec![external])]).is_err());
}

/// Scenario: `ProjectionFreshness::Fresh` permits final-decision writes;
/// `StaleReadOnly` blocks them. This is the gate that prevents the runtime from
/// auto-deciding against an out-of-date snapshot.
#[test]
fn stale_projection_blocks_phase4_auto_decision() {
    assert!(ProjectionFreshness::Fresh.allows_final_decision_write());
    assert!(!ProjectionFreshness::StaleReadOnly.allows_final_decision_write());
}

/// Scenario: the on-the-wire string for `Cancelled` and `Abandon` must remain
/// distinct (`"cancelled"` vs `"abandon"`) so storage and audit tools can tell them
/// apart unambiguously. Pinning the discriminators guards against a serde rename
/// regression.
#[test]
fn decision_verdict_terms_are_not_ambiguous() {
    assert_eq!(
        serde_json::to_string(&FinalDecisionVerdict::Cancelled).unwrap(),
        "\"cancelled\""
    );
    assert_eq!(
        serde_json::to_string(&FinalDecisionVerdict::Abandon).unwrap(),
        "\"abandon\""
    );
}
