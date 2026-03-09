pub mod acl;
mod checkpoint_policy;
pub mod decider;
pub mod executor;
pub mod gate;
pub mod persistence;
pub mod planner;
pub mod policy;
pub mod replan;
pub mod run_state;
pub mod types;
pub mod verifier;

use std::sync::Arc;

use types::{OrchestratorConfig, OrchestratorError, OrchestratorResult};

use crate::internal::ai::{
    completion::CompletionModel,
    intentspec::{repair_intentspec, types::IntentSpec, validate_intentspec},
    tools::registry::ToolRegistry,
};

/// The main orchestrator that drives IntentSpec execution through all phases.
///
/// Phases:
/// 0. Validate + repair IntentSpec
/// 1. Compile an execution plan
/// 2. Execute plan tasks with retry and policy enforcement
/// 3. Build system verification report from gate tasks
/// 4. Make decision
pub struct Orchestrator<M: CompletionModel> {
    model: M,
    registry: Arc<ToolRegistry>,
    config: OrchestratorConfig,
}

impl<M: CompletionModel + 'static> Orchestrator<M> {
    pub fn new(model: M, registry: Arc<ToolRegistry>, config: OrchestratorConfig) -> Self {
        Self {
            model,
            registry,
            config,
        }
    }

    /// Run the full orchestration pipeline.
    pub async fn run(&self, mut spec: IntentSpec) -> Result<OrchestratorResult, OrchestratorError> {
        // Phase 0: Validate and repair
        let issues = validate_intentspec(&spec);
        if !issues.is_empty() {
            repair_intentspec(&mut spec, &issues);
            let remaining = validate_intentspec(&spec);
            if !remaining.is_empty() {
                return Err(OrchestratorError::ValidationFailed(
                    remaining
                        .iter()
                        .map(|e| e.message.clone())
                        .collect::<Vec<_>>()
                        .join("; "),
                ));
            }
        }

        let tool_loop_config = crate::internal::ai::agent::ToolLoopConfig {
            preamble: self.config.coder_preamble.clone(),
            ..Default::default()
        };
        let max_replans = replan::max_replans(&spec);
        let mut replan_count = 0_u32;
        let mut plan_revision_specs = Vec::new();
        let observer = self.config.observer.clone();
        let (execution_plan_spec, run_state, system_report, decision) = loop {
            // Phase 1: Compile execution plan
            let mut plan_spec = planner::compile_execution_plan_spec(&spec)?;
            plan_spec.revision = replan_count + 1;
            plan_spec.parent_revision = (replan_count > 0).then_some(replan_count);
            plan_spec.replan_reason = spec
                .lifecycle
                .change_log
                .last()
                .map(|entry| entry.reason.clone());
            if let Some(observer) = &observer {
                observer.on_plan_compiled(&plan_spec);
            }

            // Phase 2: Execute tasks
            let executor_config = executor::ExecutorConfig {
                tool_loop_config: tool_loop_config.clone(),
                max_retries: spec.execution.retry.max_retries,
                backoff_seconds: spec.execution.retry.backoff_seconds,
                working_dir: self.config.working_dir.clone(),
                spec: Arc::new(spec.clone()),
                reviewer_preamble: self.config.reviewer_preamble.clone(),
                dagrs_resume_checkpoint_id: self.config.dagrs_resume_checkpoint_id.clone(),
                observer: observer.clone(),
            };

            let run_state =
                executor::execute_dag(&plan_spec, &self.model, &self.registry, &executor_config)
                    .await?;

            // Phase 3: System verification
            let system_report = verifier::build_system_report(&spec, &plan_spec, &run_state);

            if replan_count < max_replans
                && let Some(directive) =
                    replan::detect_replan(&spec, &plan_spec, &run_state, &system_report)
            {
                plan_revision_specs.push(plan_spec.clone());
                replan_count += 1;
                if let Some(observer) = &observer {
                    observer.on_replan(
                        plan_spec.revision,
                        replan_count + 1,
                        &directive.reason,
                        &directive.diff_summary,
                    );
                }
                replan::apply_replan(&mut spec, replan_count + 1, &directive);
                continue;
            }

            // Phase 4: Decision
            let decision = decider::make_decision(
                &run_state,
                &system_report,
                &spec.risk.level,
                spec.risk.human_in_loop.required,
            );
            plan_revision_specs.push(plan_spec.clone());
            break (plan_spec, run_state, system_report, decision);
        };
        let task_results = run_state.task_results.clone();

        let persistence = if let Some(ref mcp_server) = self.config.mcp_server {
            let persisted =
                persistence::persist_execution(persistence::ExecutionPersistenceRequest {
                    mcp_server,
                    spec: &spec,
                    execution_plan_spec: &execution_plan_spec,
                    plan_revision_specs: &plan_revision_specs,
                    run_state: &run_state,
                    system_report: &system_report,
                    decision: &decision,
                    working_dir: &self.config.working_dir,
                    base_commit: self.config.base_commit.as_deref(),
                    model_name: std::any::type_name::<M>(),
                })
                .await?;
            if let Some(observer) = &observer {
                observer.on_persistence_complete(&persisted);
            }
            Some(persisted)
        } else {
            None
        };

        Ok(OrchestratorResult {
            decision,
            execution_plan_spec,
            plan_revision_specs,
            run_state,
            task_results,
            system_report,
            intent_spec_id: spec.metadata.id.clone(),
            lifecycle_change_log: spec.lifecycle.change_log.clone(),
            replan_count,
            persistence,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use super::*;
    use crate::internal::ai::{
        completion::{
            CompletionError, CompletionRequest, CompletionResponse,
            message::{AssistantContent, Text},
        },
        intentspec::types::*,
    };

    #[derive(Clone)]
    struct MockOrchestratorModel;

    struct RecordingObserver {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl types::OrchestratorObserver for RecordingObserver {
        fn on_plan_compiled(&self, plan: &types::ExecutionPlanSpec) {
            self.events
                .lock()
                .unwrap()
                .push(format!("plan:{}", plan.revision));
        }

        fn on_task_started(&self, task: &types::TaskSpec) {
            self.events
                .lock()
                .unwrap()
                .push(format!("start:{}", task.title()));
        }

        fn on_task_completed(&self, task: &types::TaskSpec, _result: &types::TaskResult) {
            self.events
                .lock()
                .unwrap()
                .push(format!("done:{}", task.title()));
        }

        fn on_graph_progress(&self, completed: usize, total: usize) {
            self.events
                .lock()
                .unwrap()
                .push(format!("graph:{completed}/{total}"));
        }
    }

    impl CompletionModel for MockOrchestratorModel {
        type Response = ();

        #[allow(clippy::manual_async_fn)]
        fn completion(
            &self,
            _request: CompletionRequest,
        ) -> impl std::future::Future<
            Output = Result<CompletionResponse<Self::Response>, CompletionError>,
        > + Send {
            async {
                Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "task complete".into(),
                    })],
                    raw_response: (),
                })
            }
        }
    }

    fn test_spec() -> IntentSpec {
        IntentSpec {
            api_version: "intentspec.io/v1alpha1".into(),
            kind: "IntentSpec".into(),
            metadata: Metadata {
                id: "orch-test".into(),
                created_at: "2025-01-01T00:00:00Z".into(),
                created_by: CreatedBy {
                    creator_type: CreatorType::User,
                    id: "tester".into(),
                    display_name: None,
                },
                target: Target {
                    repo: RepoTarget {
                        repo_type: RepoType::Local,
                        locator: "/tmp/test".into(),
                    },
                    base_ref: "HEAD".into(),
                    workspace_id: None,
                    labels: BTreeMap::new(),
                },
            },
            intent: Intent {
                summary: "test orchestration".into(),
                problem_statement: "test".into(),
                change_type: ChangeType::Feature,
                objectives: vec!["implement feature".into()],
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
                    retention_days: 90,
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
                    max_wall_clock_seconds: 3600,
                    max_cost_units: 100,
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
                    allow: vec![ToolRule {
                        tool: "workspace.fs".into(),
                        actions: vec!["read".into(), "write".into()],
                        constraints: BTreeMap::new(),
                    }],
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
                    encoding_policy: EncodingPolicy::ContextualEscape,
                    no_direct_eval: true,
                },
            },
            execution: ExecutionPolicy {
                retry: RetryPolicy {
                    max_retries: 2,
                    backoff_seconds: 0,
                },
                replan: ReplanPolicy { triggers: vec![] },
                concurrency: ConcurrencyPolicy {
                    max_parallel_tasks: 1,
                },
            },
            artifacts: Artifacts {
                required: vec![],
                retention: ArtifactRetention { days: 90 },
            },
            provenance: ProvenancePolicy {
                require_slsa_provenance: false,
                require_sbom: false,
                transparency_log: TransparencyLogPolicy {
                    mode: TransparencyMode::None,
                },
                bindings: ProvenanceBindings {
                    embed_intent_spec_digest: false,
                    embed_evidence_digests: false,
                },
            },
            lifecycle: Lifecycle {
                schema_version: "1.0.0".into(),
                status: LifecycleStatus::Active,
                change_log: vec![],
            },
            libra: None,
            extensions: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn test_orchestrator_full_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let model = MockOrchestratorModel;
        let registry = Arc::new(ToolRegistry::new());
        let config = OrchestratorConfig {
            working_dir: dir.path().to_path_buf(),
            base_commit: None,
            dagrs_resume_checkpoint_id: None,
            coder_preamble: None,
            reviewer_preamble: None,
            mcp_server: None,
            observer: None,
        };
        let orchestrator = Orchestrator::new(model, registry, config);
        let spec = test_spec();
        let result = orchestrator.run(spec).await.unwrap();
        assert_eq!(result.decision, types::DecisionOutcome::Commit);
        assert_eq!(result.task_results.len(), 5);
        assert_eq!(result.execution_plan_spec.tasks.len(), 5);
        assert!(result.system_report.overall_passed);
    }

    #[tokio::test]
    async fn test_orchestrator_emits_progress_events() {
        let dir = tempfile::tempdir().unwrap();
        let model = MockOrchestratorModel;
        let registry = Arc::new(ToolRegistry::new());
        let events = Arc::new(Mutex::new(Vec::new()));
        let observer: Arc<dyn types::OrchestratorObserver> = Arc::new(RecordingObserver {
            events: Arc::clone(&events),
        });
        let config = OrchestratorConfig {
            working_dir: dir.path().to_path_buf(),
            base_commit: None,
            dagrs_resume_checkpoint_id: None,
            coder_preamble: None,
            reviewer_preamble: None,
            mcp_server: None,
            observer: Some(observer),
        };
        let orchestrator = Orchestrator::new(model, registry, config);
        let result = orchestrator.run(test_spec()).await.unwrap();
        let events = events.lock().unwrap().clone();
        assert!(!events.is_empty());
        assert!(events.iter().any(|event| event.starts_with("plan:")));
        assert!(events.iter().any(|event| event.starts_with("start:")));
        assert!(events.iter().any(|event| event.starts_with("done:")));
        assert!(events.iter().any(|event| event.starts_with("graph:")));
        assert_eq!(result.decision, types::DecisionOutcome::Commit);
    }

    #[tokio::test]
    async fn test_orchestrator_high_risk_human_review() {
        let dir = tempfile::tempdir().unwrap();
        let model = MockOrchestratorModel;
        let registry = Arc::new(ToolRegistry::new());
        let config = OrchestratorConfig {
            working_dir: dir.path().to_path_buf(),
            base_commit: None,
            dagrs_resume_checkpoint_id: None,
            coder_preamble: None,
            reviewer_preamble: None,
            mcp_server: None,
            observer: None,
        };
        let orchestrator = Orchestrator::new(model, registry, config);
        let mut spec = test_spec();
        spec.risk.level = RiskLevel::High;
        spec.risk.human_in_loop.required = true;
        spec.risk.human_in_loop.min_approvers = 2;
        let result = orchestrator.run(spec).await.unwrap();
        assert_eq!(result.decision, types::DecisionOutcome::HumanReviewRequired);
    }

    #[tokio::test]
    async fn test_orchestrator_validation_failure() {
        let dir = tempfile::tempdir().unwrap();
        let model = MockOrchestratorModel;
        let registry = Arc::new(ToolRegistry::new());
        let config = OrchestratorConfig {
            working_dir: dir.path().to_path_buf(),
            base_commit: None,
            dagrs_resume_checkpoint_id: None,
            coder_preamble: None,
            reviewer_preamble: None,
            mcp_server: None,
            observer: None,
        };
        let orchestrator = Orchestrator::new(model, registry, config);
        let mut spec = test_spec();
        // Empty required fields that repair cannot fix
        spec.metadata.id = String::new();
        spec.intent.summary = String::new();
        let err = orchestrator.run(spec).await.unwrap_err();
        assert!(matches!(err, OrchestratorError::ValidationFailed(_)));
    }

    #[tokio::test]
    async fn test_orchestrator_replans_after_security_gate_failure() {
        let dir = tempfile::tempdir().unwrap();
        let model = MockOrchestratorModel;
        let registry = Arc::new(ToolRegistry::new());
        let config = OrchestratorConfig {
            working_dir: dir.path().to_path_buf(),
            base_commit: None,
            dagrs_resume_checkpoint_id: None,
            coder_preamble: None,
            reviewer_preamble: None,
            mcp_server: None,
            observer: None,
        };
        let orchestrator = Orchestrator::new(model, registry, config);
        let mut spec = test_spec();
        spec.execution.replan.triggers = vec![ReplanTrigger::SecurityGateFail];
        spec.acceptance.verification_plan.security_checks = vec![Check {
            id: "security-fail".into(),
            kind: CheckKind::Command,
            command: Some("false".into()),
            timeout_seconds: Some(1),
            expected_exit_code: None,
            required: true,
            artifacts_produced: vec![],
        }];

        let result = orchestrator.run(spec).await.unwrap();
        assert_eq!(result.replan_count, 1);
        assert_eq!(result.lifecycle_change_log.len(), 1);
        assert_eq!(result.plan_revision_specs.len(), 2);
        assert_eq!(result.execution_plan_spec.revision, 2);
        assert_eq!(result.decision, types::DecisionOutcome::Abandon);
    }
}
