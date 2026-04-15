//! Phase 0 contract tests for selected plan set and task dependency rules.

use libra::internal::ai::runtime::contracts::{
    FinalDecisionVerdict, PlanRevisionSource, PlanSetWriteInput, ProjectionFreshness,
    SelectedPlanSet, validate_same_plan_dependencies,
};
use uuid::Uuid;

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

#[test]
fn task_dependencies_are_same_plan_only_in_v1() {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    validate_same_plan_dependencies(&[(first, vec![]), (second, vec![first])]).unwrap();

    let external = Uuid::new_v4();
    assert!(validate_same_plan_dependencies(&[(first, vec![external])]).is_err());
}

#[test]
fn stale_projection_blocks_phase4_auto_decision() {
    assert!(ProjectionFreshness::Fresh.allows_final_decision_write());
    assert!(!ProjectionFreshness::StaleReadOnly.allows_final_decision_write());
}

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
