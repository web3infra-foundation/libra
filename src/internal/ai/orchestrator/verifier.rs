use super::types::{ExecutionPlan, GateReport, GateStage, SystemReport, TaskResult};

/// Build the system verification report from executed gate tasks.
pub fn build_system_report(plan: &ExecutionPlan, task_results: &[TaskResult]) -> SystemReport {
    let integration = gate_report_for_stage(plan, task_results, GateStage::Integration)
        .unwrap_or_else(GateReport::empty);
    let security = gate_report_for_stage(plan, task_results, GateStage::Security)
        .unwrap_or_else(GateReport::empty);
    let release = gate_report_for_stage(plan, task_results, GateStage::Release)
        .unwrap_or_else(GateReport::empty);

    let overall_passed = integration.all_required_passed
        && security.all_required_passed
        && release.all_required_passed;

    SystemReport {
        integration,
        security,
        release,
        overall_passed,
    }
}

fn gate_report_for_stage(
    plan: &ExecutionPlan,
    task_results: &[TaskResult],
    stage: GateStage,
) -> Option<GateReport> {
    let task_id = plan
        .dag
        .nodes
        .iter()
        .find(|node| node.gate_stage == Some(stage.clone()))
        .map(|node| node.id)?;

    task_results
        .iter()
        .find(|result| result.task_id == task_id)
        .and_then(|result| result.gate_report.clone())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::internal::ai::orchestrator::types::{
        ExecutionCheckpoint, TaskContract, TaskDAG, TaskKind, TaskNode, TaskNodeStatus,
    };

    fn plan_with_gates() -> ExecutionPlan {
        let integration_id = Uuid::new_v4();
        let security_id = Uuid::new_v4();
        let release_id = Uuid::new_v4();
        ExecutionPlan {
            intent_spec_id: "test".into(),
            summary: "summary".into(),
            dag: TaskDAG {
                nodes: vec![
                    TaskNode {
                        id: integration_id,
                        title: "Integration".into(),
                        objective: "integration".into(),
                        description: None,
                        kind: TaskKind::Gate,
                        gate_stage: Some(GateStage::Integration),
                        owner_role: Some("verifier".into()),
                        dependencies: vec![],
                        constraints: vec![],
                        acceptance_criteria: vec![],
                        scope_in: vec![],
                        scope_out: vec![],
                        checks: vec![],
                        contract: TaskContract::default(),
                        status: TaskNodeStatus::Pending,
                    },
                    TaskNode {
                        id: security_id,
                        title: "Security".into(),
                        objective: "security".into(),
                        description: None,
                        kind: TaskKind::Gate,
                        gate_stage: Some(GateStage::Security),
                        owner_role: Some("verifier".into()),
                        dependencies: vec![],
                        constraints: vec![],
                        acceptance_criteria: vec![],
                        scope_in: vec![],
                        scope_out: vec![],
                        checks: vec![],
                        contract: TaskContract::default(),
                        status: TaskNodeStatus::Pending,
                    },
                    TaskNode {
                        id: release_id,
                        title: "Release".into(),
                        objective: "release".into(),
                        description: None,
                        kind: TaskKind::Gate,
                        gate_stage: Some(GateStage::Release),
                        owner_role: Some("verifier".into()),
                        dependencies: vec![],
                        constraints: vec![],
                        acceptance_criteria: vec![],
                        scope_in: vec![],
                        scope_out: vec![],
                        checks: vec![],
                        contract: TaskContract::default(),
                        status: TaskNodeStatus::Pending,
                    },
                ],
                intent_spec_id: "test".into(),
                max_parallel: 1,
            },
            parallel_groups: vec![],
            checkpoints: vec![ExecutionCheckpoint {
                label: "after-security".into(),
                after_tasks: vec![security_id],
                reason: "gate".into(),
            }],
        }
    }

    fn gate_result(task_id: Uuid, passed: bool) -> TaskResult {
        TaskResult {
            task_id,
            status: if passed {
                TaskNodeStatus::Completed
            } else {
                TaskNodeStatus::Failed
            },
            gate_report: Some(GateReport {
                results: vec![],
                all_required_passed: passed,
            }),
            agent_output: None,
            retry_count: 0,
            tool_calls: vec![],
            policy_violations: vec![],
        }
    }

    #[test]
    fn test_build_system_report_from_gate_results() {
        let plan = plan_with_gates();
        let results = vec![
            gate_result(plan.dag.nodes[0].id, true),
            gate_result(plan.dag.nodes[1].id, true),
            gate_result(plan.dag.nodes[2].id, false),
        ];

        let report = build_system_report(&plan, &results);
        assert!(report.integration.all_required_passed);
        assert!(report.security.all_required_passed);
        assert!(!report.release.all_required_passed);
        assert!(!report.overall_passed);
    }
}
