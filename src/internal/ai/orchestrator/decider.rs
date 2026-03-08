use super::{
    run_state::RunStateSnapshot,
    types::{DecisionOutcome, SystemReport, TaskNodeStatus},
};
use crate::internal::ai::intentspec::types::RiskLevel;

/// Make a decision based on task results, system verification, and risk level.
///
/// Decision logic:
/// - Any task failed → Abandon
/// - System verification failed → Abandon
/// - human_in_loop required → HumanReviewRequired
/// - All pass + low/medium risk → Commit
/// - High risk always → HumanReviewRequired
pub fn make_decision(
    run_state: &RunStateSnapshot,
    system_report: &SystemReport,
    risk: &RiskLevel,
    human_in_loop_required: bool,
) -> DecisionOutcome {
    // Any failed task → abandon
    let has_failed = run_state
        .ordered_task_results()
        .iter()
        .any(|r| r.status == TaskNodeStatus::Failed);
    if has_failed {
        return DecisionOutcome::Abandon;
    }

    // System verification failed → abandon
    if !system_report.overall_passed {
        return DecisionOutcome::Abandon;
    }

    // Human-in-loop required → human review
    if human_in_loop_required {
        return DecisionOutcome::HumanReviewRequired;
    }

    // High risk always requires human review
    if matches!(risk, RiskLevel::High) {
        return DecisionOutcome::HumanReviewRequired;
    }

    DecisionOutcome::Commit
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::internal::ai::orchestrator::{
        run_state::RunStateSnapshot,
        types::{GateReport, TaskResult},
    };

    fn passing_system_report() -> SystemReport {
        SystemReport {
            integration: GateReport::empty(),
            security: GateReport::empty(),
            release: GateReport::empty(),
            review_passed: true,
            review_findings: vec![],
            artifacts_complete: true,
            missing_artifacts: vec![],
            overall_passed: true,
        }
    }

    fn failing_system_report() -> SystemReport {
        SystemReport {
            integration: GateReport::empty(),
            security: GateReport::empty(),
            release: GateReport::empty(),
            review_passed: true,
            review_findings: vec![],
            artifacts_complete: true,
            missing_artifacts: vec![],
            overall_passed: false,
        }
    }

    fn task_result(status: TaskNodeStatus) -> TaskResult {
        TaskResult {
            task_id: Uuid::new_v4(),
            status,
            gate_report: None,
            agent_output: None,
            retry_count: 0,
            tool_calls: vec![],
            policy_violations: vec![],
            review: None,
        }
    }

    fn run_state(results: Vec<TaskResult>) -> RunStateSnapshot {
        RunStateSnapshot {
            intent_spec_id: "spec-1".into(),
            revision: 1,
            task_statuses: results
                .iter()
                .map(|result| super::super::run_state::TaskStatusSnapshot {
                    task_id: result.task_id,
                    status: result.status.clone(),
                })
                .collect(),
            task_results: results,
        }
    }

    #[test]
    fn test_all_pass_low_risk() {
        let results = run_state(vec![task_result(TaskNodeStatus::Completed)]);
        let report = passing_system_report();
        let decision = make_decision(&results, &report, &RiskLevel::Low, false);
        assert_eq!(decision, DecisionOutcome::Commit);
    }

    #[test]
    fn test_all_pass_medium_risk() {
        let results = run_state(vec![task_result(TaskNodeStatus::Completed)]);
        let report = passing_system_report();
        let decision = make_decision(&results, &report, &RiskLevel::Medium, false);
        assert_eq!(decision, DecisionOutcome::Commit);
    }

    #[test]
    fn test_high_risk_requires_human_review() {
        let results = run_state(vec![task_result(TaskNodeStatus::Completed)]);
        let report = passing_system_report();
        let decision = make_decision(&results, &report, &RiskLevel::High, false);
        assert_eq!(decision, DecisionOutcome::HumanReviewRequired);
    }

    #[test]
    fn test_human_in_loop_required() {
        let results = run_state(vec![task_result(TaskNodeStatus::Completed)]);
        let report = passing_system_report();
        let decision = make_decision(&results, &report, &RiskLevel::Low, true);
        assert_eq!(decision, DecisionOutcome::HumanReviewRequired);
    }

    #[test]
    fn test_task_failed() {
        let results = run_state(vec![
            task_result(TaskNodeStatus::Completed),
            task_result(TaskNodeStatus::Failed),
        ]);
        let report = passing_system_report();
        let decision = make_decision(&results, &report, &RiskLevel::Low, false);
        assert_eq!(decision, DecisionOutcome::Abandon);
    }

    #[test]
    fn test_verification_failed() {
        let results = run_state(vec![task_result(TaskNodeStatus::Completed)]);
        let report = failing_system_report();
        let decision = make_decision(&results, &report, &RiskLevel::Low, false);
        assert_eq!(decision, DecisionOutcome::Abandon);
    }

    #[test]
    fn test_empty_results_commit() {
        let results = run_state(vec![]);
        let report = passing_system_report();
        let decision = make_decision(&results, &report, &RiskLevel::Low, false);
        assert_eq!(decision, DecisionOutcome::Commit);
    }

    #[test]
    fn test_task_failed_takes_priority_over_human_review() {
        let results = run_state(vec![task_result(TaskNodeStatus::Failed)]);
        let report = passing_system_report();
        let decision = make_decision(&results, &report, &RiskLevel::High, true);
        assert_eq!(decision, DecisionOutcome::Abandon);
    }

    #[test]
    fn test_verification_failed_takes_priority_over_human_review() {
        let results = run_state(vec![task_result(TaskNodeStatus::Completed)]);
        let report = failing_system_report();
        let decision = make_decision(&results, &report, &RiskLevel::High, true);
        assert_eq!(decision, DecisionOutcome::Abandon);
    }
}
