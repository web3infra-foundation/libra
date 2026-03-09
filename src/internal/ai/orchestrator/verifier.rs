use std::collections::BTreeSet;

use super::{
    run_state::RunStateSnapshot,
    types::{ExecutionPlanSpec, GateReport, GateStage, SystemReport, TaskKind},
};
use crate::internal::ai::intentspec::types::{ArtifactName, ArtifactStage, IntentSpec};

/// Build the system verification report from executed gate tasks, review results,
/// and required artifact contracts.
pub fn build_system_report(
    spec: &IntentSpec,
    plan: &ExecutionPlanSpec,
    run_state: &RunStateSnapshot,
) -> SystemReport {
    let integration = gate_report_for_stage(plan, run_state, GateStage::Integration)
        .unwrap_or_else(GateReport::empty);
    let security = gate_report_for_stage(plan, run_state, GateStage::Security)
        .unwrap_or_else(GateReport::empty);
    let release = gate_report_for_stage(plan, run_state, GateStage::Release)
        .unwrap_or_else(GateReport::empty);
    let (review_passed, review_findings) = review_report(plan, run_state);
    let (artifacts_complete, missing_artifacts) = artifact_report(spec, plan, run_state);

    let overall_passed = integration.all_required_passed
        && security.all_required_passed
        && release.all_required_passed
        && review_passed
        && artifacts_complete;

    SystemReport {
        integration,
        security,
        release,
        review_passed,
        review_findings,
        artifacts_complete,
        missing_artifacts,
        overall_passed,
    }
}

fn gate_report_for_stage(
    plan: &ExecutionPlanSpec,
    run_state: &RunStateSnapshot,
    stage: GateStage,
) -> Option<GateReport> {
    let task_id = plan
        .tasks
        .iter()
        .find(|task| task.gate_stage == Some(stage.clone()))
        .map(|task| task.id())?;

    run_state
        .result_for(task_id)
        .and_then(|result| result.gate_report.clone())
}

fn review_report(plan: &ExecutionPlanSpec, run_state: &RunStateSnapshot) -> (bool, Vec<String>) {
    let implementation_ids = plan
        .tasks
        .iter()
        .filter(|task| task.kind == TaskKind::Implementation)
        .map(|task| task.id())
        .collect::<BTreeSet<_>>();

    let findings = run_state
        .ordered_task_results()
        .iter()
        .filter(|result| implementation_ids.contains(&result.task_id))
        .filter_map(|result| result.review.as_ref())
        .filter(|review| !review.approved)
        .map(|review| {
            if review.issues.is_empty() {
                review.summary.clone()
            } else {
                format!("{} [{}]", review.summary, review.issues.join("; "))
            }
        })
        .collect::<Vec<_>>();

    (findings.is_empty(), findings)
}

fn artifact_report(
    spec: &IntentSpec,
    plan: &ExecutionPlanSpec,
    run_state: &RunStateSnapshot,
) -> (bool, Vec<String>) {
    let produced = produced_artifacts(plan, run_state);
    let missing = spec
        .artifacts
        .required
        .iter()
        .filter(|artifact| artifact.required)
        .filter(|artifact| !produced.contains(&artifact_key(&artifact.name, &artifact.stage)))
        .map(|artifact| {
            format!(
                "{}@{}",
                artifact_name_label(&artifact.name),
                artifact_stage_label(&artifact.stage)
            )
        })
        .collect::<Vec<_>>();

    (missing.is_empty(), missing)
}

fn produced_artifacts(plan: &ExecutionPlanSpec, run_state: &RunStateSnapshot) -> BTreeSet<String> {
    let mut produced = BTreeSet::new();

    for task in &plan.tasks {
        let Some(result) = run_state.result_for(task.id()) else {
            continue;
        };

        if task.kind == TaskKind::Implementation
            && result.status == super::types::TaskNodeStatus::Completed
            && result
                .tool_calls
                .iter()
                .any(|call| !call.paths_written.is_empty() || !call.diffs.is_empty())
        {
            produced.insert(artifact_key(
                &ArtifactName::Patchset,
                &ArtifactStage::PerTask,
            ));
        }

        if let Some(report) = &result.gate_report {
            for check in &task.checks {
                if report
                    .results
                    .iter()
                    .any(|gate| gate.check_id == check.id && gate.passed)
                {
                    for artifact in &check.artifacts_produced {
                        if let Some(name) = parse_artifact_name(artifact) {
                            produced.insert(artifact_key(
                                &name,
                                &stage_for_gate(task.gate_stage.as_ref()),
                            ));
                        }
                    }
                }
            }
        }
    }

    produced
}

fn stage_for_gate(stage: Option<&GateStage>) -> ArtifactStage {
    match stage {
        Some(GateStage::Integration) => ArtifactStage::Integration,
        Some(GateStage::Security) => ArtifactStage::Security,
        Some(GateStage::Release) => ArtifactStage::Release,
        Some(GateStage::Fast) | None => ArtifactStage::PerTask,
    }
}

fn parse_artifact_name(raw: &str) -> Option<ArtifactName> {
    Some(match raw {
        "patchset" => ArtifactName::Patchset,
        "test-log" => ArtifactName::TestLog,
        "build-log" => ArtifactName::BuildLog,
        "sast-report" => ArtifactName::SastReport,
        "sca-report" => ArtifactName::ScaReport,
        "sbom" => ArtifactName::Sbom,
        "provenance-attestation" => ArtifactName::ProvenanceAttestation,
        "transparency-proof" => ArtifactName::TransparencyProof,
        "release-notes" => ArtifactName::ReleaseNotes,
        _ => return None,
    })
}

fn artifact_key(name: &ArtifactName, stage: &ArtifactStage) -> String {
    format!(
        "{}@{}",
        artifact_name_label(name),
        artifact_stage_label(stage)
    )
}

fn artifact_name_label(name: &ArtifactName) -> &'static str {
    match name {
        ArtifactName::Patchset => "patchset",
        ArtifactName::TestLog => "test-log",
        ArtifactName::BuildLog => "build-log",
        ArtifactName::SastReport => "sast-report",
        ArtifactName::ScaReport => "sca-report",
        ArtifactName::Sbom => "sbom",
        ArtifactName::ProvenanceAttestation => "provenance-attestation",
        ArtifactName::TransparencyProof => "transparency-proof",
        ArtifactName::ReleaseNotes => "release-notes",
    }
}

fn artifact_stage_label(stage: &ArtifactStage) -> &'static str {
    match stage {
        ArtifactStage::PerTask => "per-task",
        ArtifactStage::Integration => "integration",
        ArtifactStage::Security => "security",
        ArtifactStage::Release => "release",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_internal::internal::object::{task::Task as GitTask, types::ActorRef};

    use crate::internal::ai::{
        intentspec::types::{
            Acceptance, Artifacts, ChangeLogEntry, ChangeType, Check, CheckKind, ConcurrencyPolicy,
            ConstraintLicensing, ConstraintPlatform, ConstraintPrivacy, ConstraintResources,
            ConstraintSecurity, Constraints, CreatedBy, CreatorType, DataClass, DependencyPolicy,
            DomainAllowlistMode, EncodingPolicy, EvidencePolicy, EvidenceStrategy, ExecutionPolicy,
            HumanInLoop, Intent, Lifecycle, LifecycleStatus, Metadata, NetworkPolicy,
            OutputHandlingPolicy, PromptInjectionPolicy, ProvenanceBindings, ProvenancePolicy,
            RepoTarget, RepoType, RetryPolicy, Risk, RiskLevel, SecretAccessPolicy, SecretPolicy,
            SecurityPolicy, Target, ToolAcl, TransparencyLogPolicy, TransparencyMode, TrustTier,
            VerificationPlan,
        },
        orchestrator::{
            run_state::{RunStateSnapshot, TaskStatusSnapshot},
            types::{
                ExecutionCheckpoint, ExecutionPlanSpec, GateResult, ReviewOutcome, TaskContract,
                TaskKind, TaskNodeStatus, TaskResult, TaskSpec,
            },
        },
    };

    fn spec_with_required_artifacts() -> IntentSpec {
        IntentSpec {
            api_version: "intentspec.io/v1alpha1".into(),
            kind: "IntentSpec".into(),
            metadata: Metadata {
                id: "test".into(),
                created_at: "2025-01-01T00:00:00Z".into(),
                created_by: CreatedBy {
                    creator_type: CreatorType::User,
                    id: "tester".into(),
                    display_name: None,
                },
                target: Target {
                    repo: RepoTarget {
                        repo_type: RepoType::Local,
                        locator: ".".into(),
                    },
                    base_ref: "HEAD".into(),
                    workspace_id: None,
                    labels: Default::default(),
                },
            },
            intent: Intent {
                summary: "summary".into(),
                problem_statement: "problem".into(),
                change_type: ChangeType::Feature,
                objectives: vec!["ship".into()],
                in_scope: vec!["src/".into()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: Acceptance {
                success_criteria: vec!["tests pass".into()],
                verification_plan: VerificationPlan {
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                quality_gates: None,
            },
            constraints: Constraints {
                security: ConstraintSecurity {
                    network_policy: NetworkPolicy::Deny,
                    dependency_policy: DependencyPolicy::NoNew,
                    crypto_policy: String::new(),
                },
                privacy: ConstraintPrivacy {
                    data_classes_allowed: vec![DataClass::Public],
                    redaction_required: false,
                    retention_days: 30,
                },
                licensing: ConstraintLicensing {
                    allowed_spdx: vec![],
                    forbid_new_licenses: false,
                },
                platform: ConstraintPlatform {
                    language_runtime: "rust".into(),
                    supported_os: vec![],
                },
                resources: ConstraintResources {
                    max_wall_clock_seconds: 300,
                    max_cost_units: 0,
                },
            },
            risk: Risk {
                level: RiskLevel::Low,
                rationale: "test".into(),
                factors: vec![],
                human_in_loop: HumanInLoop {
                    required: false,
                    min_approvers: 0,
                },
            },
            evidence: EvidencePolicy {
                strategy: EvidenceStrategy::RepoFirst,
                trust_tiers: vec![TrustTier::Repo],
                domain_allowlist_mode: DomainAllowlistMode::Disabled,
                allowed_domains: vec![],
                blocked_domains: vec![],
                min_citations_per_decision: 1,
            },
            security: SecurityPolicy {
                tool_acl: ToolAcl {
                    allow: vec![],
                    deny: vec![],
                },
                secrets: SecretPolicy {
                    policy: SecretAccessPolicy::DenyAll,
                    allowed_scopes: vec![],
                },
                prompt_injection: PromptInjectionPolicy {
                    treat_retrieved_content_as_untrusted: true,
                    enforce_output_schema: true,
                    disallow_instruction_from_evidence: true,
                },
                output_handling: OutputHandlingPolicy {
                    encoding_policy: EncodingPolicy::StrictJson,
                    no_direct_eval: true,
                },
            },
            execution: ExecutionPolicy {
                retry: RetryPolicy {
                    max_retries: 1,
                    backoff_seconds: 0,
                },
                replan: crate::internal::ai::intentspec::types::ReplanPolicy { triggers: vec![] },
                concurrency: ConcurrencyPolicy {
                    max_parallel_tasks: 1,
                },
            },
            artifacts: Artifacts {
                required: vec![
                    crate::internal::ai::intentspec::types::ArtifactReq {
                        name: ArtifactName::Patchset,
                        stage: ArtifactStage::PerTask,
                        required: true,
                        format: "git-diff".into(),
                    },
                    crate::internal::ai::intentspec::types::ArtifactReq {
                        name: ArtifactName::TestLog,
                        stage: ArtifactStage::Release,
                        required: true,
                        format: "text".into(),
                    },
                ],
                retention: Default::default(),
            },
            provenance: ProvenancePolicy {
                require_slsa_provenance: false,
                require_sbom: false,
                transparency_log: TransparencyLogPolicy {
                    mode: TransparencyMode::None,
                },
                bindings: ProvenanceBindings {
                    embed_intent_spec_digest: true,
                    embed_evidence_digests: false,
                },
            },
            lifecycle: Lifecycle {
                schema_version: "1".into(),
                status: LifecycleStatus::Active,
                change_log: Vec::<ChangeLogEntry>::new(),
            },
            libra: None,
            extensions: Default::default(),
        }
    }

    fn plan_with_gates() -> ExecutionPlanSpec {
        let impl_task = {
            let actor = ActorRef::agent("test-verifier").unwrap();
            GitTask::new(actor, "Implementation", None).unwrap()
        };
        let impl_id = impl_task.header().object_id();
        let release_task = {
            let actor = ActorRef::agent("test-verifier").unwrap();
            let mut task = GitTask::new(actor, "Release", None).unwrap();
            task.add_dependency(impl_id);
            task
        };
        let release_id = release_task.header().object_id();
        ExecutionPlanSpec {
            intent_spec_id: "test".into(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![
                TaskSpec {
                    step: git_internal::internal::object::plan::PlanStep::new("Implementation"),
                    task: impl_task,
                    objective: "implementation".into(),
                    kind: TaskKind::Implementation,
                    gate_stage: None,
                    owner_role: Some("coder".into()),
                    scope_in: vec![],
                    scope_out: vec![],
                    checks: vec![],
                    contract: TaskContract::default(),
                },
                TaskSpec {
                    step: git_internal::internal::object::plan::PlanStep::new("Release"),
                    task: release_task,
                    objective: "release".into(),
                    kind: TaskKind::Gate,
                    gate_stage: Some(GateStage::Release),
                    owner_role: Some("verifier".into()),
                    scope_in: vec![],
                    scope_out: vec![],
                    checks: vec![Check {
                        id: "release-test".into(),
                        kind: CheckKind::Command,
                        command: Some("cargo test".into()),
                        timeout_seconds: None,
                        expected_exit_code: None,
                        required: true,
                        artifacts_produced: vec!["test-log".into()],
                    }],
                    contract: TaskContract::default(),
                },
            ],
            max_parallel: 1,
            checkpoints: vec![ExecutionCheckpoint {
                label: "after-release".into(),
                after_tasks: vec![release_id],
                reason: "gate".into(),
            }],
        }
    }

    #[test]
    fn test_build_system_report_tracks_review_and_artifacts() {
        let spec = spec_with_required_artifacts();
        let plan = plan_with_gates();
        let results = vec![
            TaskResult {
                task_id: plan.tasks[0].id(),
                status: TaskNodeStatus::Completed,
                gate_report: None,
                agent_output: Some("done".into()),
                retry_count: 0,
                tool_calls: vec![crate::internal::ai::orchestrator::types::ToolCallRecord {
                    tool_name: "apply_patch".into(),
                    action: "write".into(),
                    arguments_json: None,
                    paths_read: vec![],
                    paths_written: vec!["src/lib.rs".into()],
                    success: true,
                    summary: None,
                    diffs: vec![],
                }],
                policy_violations: vec![],
                review: Some(ReviewOutcome {
                    approved: true,
                    summary: "looks good".into(),
                    issues: vec![],
                }),
            },
            TaskResult {
                task_id: plan.tasks[1].id(),
                status: TaskNodeStatus::Completed,
                gate_report: Some(GateReport {
                    results: vec![GateResult {
                        check_id: "release-test".into(),
                        kind: "test".into(),
                        passed: true,
                        exit_code: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                        duration_ms: 1,
                        timed_out: false,
                    }],
                    all_required_passed: true,
                }),
                agent_output: None,
                retry_count: 0,
                tool_calls: vec![],
                policy_violations: vec![],
                review: None,
            },
        ];
        let run_state = RunStateSnapshot {
            intent_spec_id: plan.intent_spec_id.clone(),
            revision: plan.revision,
            task_statuses: results
                .iter()
                .map(|result| TaskStatusSnapshot {
                    task_id: result.task_id,
                    status: result.status.clone(),
                })
                .collect(),
            task_results: results,
            dagrs_runtime: Default::default(),
        };

        let report = build_system_report(&spec, &plan, &run_state);
        assert!(report.review_passed);
        assert!(report.artifacts_complete);
        assert!(report.overall_passed);
    }

    #[test]
    fn test_failed_gate_check_does_not_produce_artifact() {
        let spec = spec_with_required_artifacts();
        let plan = plan_with_gates();
        let results = vec![
            TaskResult {
                task_id: plan.tasks[0].id(),
                status: TaskNodeStatus::Completed,
                gate_report: None,
                agent_output: Some("done".into()),
                retry_count: 0,
                tool_calls: vec![crate::internal::ai::orchestrator::types::ToolCallRecord {
                    tool_name: "apply_patch".into(),
                    action: "write".into(),
                    arguments_json: None,
                    paths_read: vec![],
                    paths_written: vec!["src/lib.rs".into()],
                    success: true,
                    summary: None,
                    diffs: vec![],
                }],
                policy_violations: vec![],
                review: Some(ReviewOutcome {
                    approved: true,
                    summary: "looks good".into(),
                    issues: vec![],
                }),
            },
            TaskResult {
                task_id: plan.tasks[1].id(),
                status: TaskNodeStatus::Failed,
                gate_report: Some(GateReport {
                    results: vec![GateResult {
                        check_id: "release-test".into(),
                        kind: "test".into(),
                        passed: false,
                        exit_code: 1,
                        stdout: String::new(),
                        stderr: "failed".into(),
                        duration_ms: 1,
                        timed_out: false,
                    }],
                    all_required_passed: false,
                }),
                agent_output: None,
                retry_count: 0,
                tool_calls: vec![],
                policy_violations: vec![],
                review: None,
            },
        ];
        let run_state = RunStateSnapshot {
            intent_spec_id: plan.intent_spec_id.clone(),
            revision: plan.revision,
            task_statuses: results
                .iter()
                .map(|result| TaskStatusSnapshot {
                    task_id: result.task_id,
                    status: result.status.clone(),
                })
                .collect(),
            task_results: results,
            dagrs_runtime: Default::default(),
        };

        let report = build_system_report(&spec, &plan, &run_state);
        assert!(!report.artifacts_complete);
        assert!(
            report
                .missing_artifacts
                .iter()
                .any(|name| name == "test-log@release")
        );
    }
}
