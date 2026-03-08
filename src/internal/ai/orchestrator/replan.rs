use chrono::Utc;

use super::{
    run_state::RunStateSnapshot,
    types::{ExecutionPlanSpec, SystemReport, TaskNodeStatus},
};
use crate::internal::ai::intentspec::types::{
    ChangeLogEntry, ConflictResolution, DecompositionMode, IntentSpec, LibraBinding,
    PlanGenerationConfig, ReplanTrigger,
};

#[derive(Clone, Debug)]
pub struct ReplanDirective {
    pub trigger: ReplanTrigger,
    pub reason: String,
    pub diff_summary: String,
}

pub fn max_replans(spec: &IntentSpec) -> u32 {
    if spec.execution.replan.triggers.is_empty() {
        0
    } else {
        spec.execution.replan.triggers.len() as u32
    }
}

pub fn detect_replan(
    spec: &IntentSpec,
    _plan: &ExecutionPlanSpec,
    run_state: &RunStateSnapshot,
    system_report: &SystemReport,
) -> Option<ReplanDirective> {
    if trigger_enabled(spec, ReplanTrigger::ScopeCreep)
        && run_state.ordered_task_results().iter().any(|result| {
            result
                .policy_violations
                .iter()
                .any(|violation| violation.code == "scope-creep")
        })
    {
        return Some(ReplanDirective {
            trigger: ReplanTrigger::ScopeCreep,
            reason: "scope creep detected during task execution".to_string(),
            diff_summary:
                "Tighten execution to a single serial task and preserve current checkpoints."
                    .to_string(),
        });
    }

    if trigger_enabled(spec, ReplanTrigger::SecurityGateFail)
        && !system_report.security.all_required_passed
    {
        return Some(ReplanDirective {
            trigger: ReplanTrigger::SecurityGateFail,
            reason: "security gate failed".to_string(),
            diff_summary: "Recompile the plan in serial mode to focus on security remediation."
                .to_string(),
        });
    }

    if trigger_enabled(spec, ReplanTrigger::RepeatedTestFail)
        && run_state.ordered_task_results().iter().any(|result| {
            result.status == TaskNodeStatus::Failed
                && result.retry_count >= spec.execution.retry.max_retries
        })
    {
        return Some(ReplanDirective {
            trigger: ReplanTrigger::RepeatedTestFail,
            reason: "task kept failing after retries".to_string(),
            diff_summary:
                "Collapse execution into a single serial repair task for the next revision."
                    .to_string(),
        });
    }

    if trigger_enabled(spec, ReplanTrigger::EvidenceConflict) && !system_report.artifacts_complete {
        return Some(ReplanDirective {
            trigger: ReplanTrigger::EvidenceConflict,
            reason: "required artifacts were not produced".to_string(),
            diff_summary: "Keep gates enabled and recompile with stricter artifact expectations."
                .to_string(),
        });
    }

    if trigger_enabled(spec, ReplanTrigger::UnknownApi)
        && run_state.ordered_task_results().iter().any(|result| {
            result
                .policy_violations
                .iter()
                .any(|violation| violation.code == "invalid-tool-arguments")
        })
    {
        return Some(ReplanDirective {
            trigger: ReplanTrigger::UnknownApi,
            reason: "tool or API usage drifted from the compiled contract".to_string(),
            diff_summary:
                "Recompile to a single-task plan and reduce concurrency to prevent further drift."
                    .to_string(),
        });
    }

    None
}

pub fn apply_replan(spec: &mut IntentSpec, revision: u32, directive: &ReplanDirective) {
    let plan_config = spec
        .libra
        .get_or_insert_with(empty_libra_binding)
        .plan_generation
        .get_or_insert_with(PlanGenerationConfig::default);

    plan_config.conflict_resolution = ConflictResolution::ForceSerial;
    plan_config.gate_task_per_stage = true;
    if matches!(
        directive.trigger,
        ReplanTrigger::ScopeCreep | ReplanTrigger::RepeatedTestFail | ReplanTrigger::UnknownApi
    ) {
        plan_config.decomposition_mode = DecompositionMode::SingleTask;
    }

    spec.execution.concurrency.max_parallel_tasks = 1;
    spec.lifecycle.change_log.push(ChangeLogEntry {
        at: Utc::now().to_rfc3339(),
        by: "libra-orchestrator".to_string(),
        reason: directive.reason.clone(),
        diff_summary: format!("revision {revision}: {}", directive.diff_summary),
    });
}

fn trigger_enabled(spec: &IntentSpec, trigger: ReplanTrigger) -> bool {
    spec.execution.replan.triggers.contains(&trigger)
}

fn empty_libra_binding() -> LibraBinding {
    LibraBinding {
        object_store: None,
        context_pipeline: None,
        plan_generation: None,
        run_policy: None,
        actor_mapping: None,
        decision_policy: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use uuid::Uuid;

    use super::*;
    use crate::internal::ai::{
        intentspec::types::*,
        orchestrator::{
            run_state::{RunStateSnapshot, TaskStatusSnapshot},
            types::{ExecutionPlanSpec, GateReport, TaskResult},
        },
    };

    fn spec_with_triggers() -> IntentSpec {
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
                    labels: BTreeMap::new(),
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
                success_criteria: vec![],
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
                    retention_days: 1,
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
                    max_wall_clock_seconds: 30,
                    max_cost_units: 0,
                },
            },
            risk: Risk {
                level: RiskLevel::Low,
                rationale: String::new(),
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
                replan: ReplanPolicy {
                    triggers: vec![ReplanTrigger::SecurityGateFail],
                },
                concurrency: ConcurrencyPolicy {
                    max_parallel_tasks: 4,
                },
            },
            artifacts: Artifacts {
                required: vec![],
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
                change_log: vec![],
            },
            libra: None,
            extensions: BTreeMap::new(),
        }
    }

    fn run_state(results: Vec<TaskResult>) -> RunStateSnapshot {
        RunStateSnapshot {
            intent_spec_id: "test".into(),
            revision: 1,
            task_statuses: results
                .iter()
                .map(|result| TaskStatusSnapshot {
                    task_id: result.task_id,
                    status: result.status.clone(),
                })
                .collect(),
            task_results: results,
        }
    }

    #[test]
    fn test_apply_replan_reduces_parallelism_and_logs_change() {
        let mut spec = spec_with_triggers();
        apply_replan(
            &mut spec,
            2,
            &ReplanDirective {
                trigger: ReplanTrigger::SecurityGateFail,
                reason: "security gate failed".into(),
                diff_summary: "serialise execution".into(),
            },
        );
        assert_eq!(spec.execution.concurrency.max_parallel_tasks, 1);
        assert_eq!(spec.lifecycle.change_log.len(), 1);
        assert!(
            spec.libra
                .unwrap()
                .plan_generation
                .unwrap()
                .gate_task_per_stage
        );
    }

    #[test]
    fn test_detect_replan_from_security_failure() {
        let spec = spec_with_triggers();
        let plan = ExecutionPlanSpec {
            intent_spec_id: "test".into(),
            summary: "summary".into(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![],
            max_parallel: 1,
            parallel_groups: vec![],
            checkpoints: vec![],
        };
        let directive = detect_replan(
            &spec,
            &plan,
            &run_state(vec![TaskResult {
                task_id: Uuid::new_v4(),
                status: TaskNodeStatus::Completed,
                gate_report: Some(GateReport::empty()),
                agent_output: None,
                retry_count: 0,
                tool_calls: vec![],
                policy_violations: vec![],
                review: None,
            }]),
            &SystemReport {
                integration: GateReport::empty(),
                security: GateReport {
                    results: vec![],
                    all_required_passed: false,
                },
                release: GateReport::empty(),
                review_passed: true,
                review_findings: vec![],
                artifacts_complete: true,
                missing_artifacts: vec![],
                overall_passed: false,
            },
        );
        assert!(directive.is_some());
    }
}
