use uuid::Uuid;

use super::types::{TaskDAG, TaskNode, TaskNodeStatus};
use crate::internal::ai::intentspec::types::{IntentSpec, NetworkPolicy};

/// Generate a TaskDAG from a resolved IntentSpec.
///
/// Each objective becomes one TaskNode. Dependencies are sequential when
/// `max_parallel_tasks == 1`, otherwise tasks are independent.
pub fn generate_task_dag(spec: &IntentSpec) -> TaskDAG {
    let max_parallel = spec.execution.concurrency.max_parallel_tasks;
    let sequential = max_parallel <= 1;

    let mut constraints = Vec::new();
    if matches!(
        spec.constraints.security.network_policy,
        NetworkPolicy::Deny
    ) {
        constraints.push("network:deny".to_string());
    }
    constraints.push(format!(
        "dependency-policy:{:?}",
        spec.constraints.security.dependency_policy
    ));

    let acceptance_criteria: Vec<String> = spec.acceptance.success_criteria.clone();
    let scope_in = spec.intent.in_scope.clone();
    let scope_out = spec.intent.out_of_scope.clone();

    let mut nodes = Vec::with_capacity(spec.intent.objectives.len());
    let mut prev_id: Option<Uuid> = None;

    for objective in &spec.intent.objectives {
        let id = Uuid::new_v4();
        let dependencies = if sequential {
            prev_id.map(|pid| vec![pid]).unwrap_or_default()
        } else {
            vec![]
        };

        nodes.push(TaskNode {
            id,
            objective: objective.clone(),
            description: None,
            dependencies,
            constraints: constraints.clone(),
            acceptance_criteria: acceptance_criteria.clone(),
            scope_in: scope_in.clone(),
            scope_out: scope_out.clone(),
            status: TaskNodeStatus::Pending,
        });

        prev_id = Some(id);
    }

    TaskDAG {
        nodes,
        intent_spec_id: spec.metadata.id.clone(),
        max_parallel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::types::*;
    use std::collections::BTreeMap;

    fn minimal_spec(objectives: Vec<String>, max_parallel: u8) -> IntentSpec {
        IntentSpec {
            api_version: "intentspec.io/v1alpha1".into(),
            kind: "IntentSpec".into(),
            metadata: Metadata {
                id: "test-id".into(),
                created_at: "2025-01-01T00:00:00Z".into(),
                created_by: CreatedBy {
                    creator_type: CreatorType::User,
                    id: "test".into(),
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
                summary: "test".into(),
                problem_statement: "test".into(),
                change_type: ChangeType::Feature,
                objectives,
                in_scope: vec!["src/".into()],
                out_of_scope: vec!["vendor/".into()],
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
                    encoding_policy: EncodingPolicy::ContextualEscape,
                    no_direct_eval: true,
                },
            },
            execution: ExecutionPolicy {
                retry: RetryPolicy {
                    max_retries: 3,
                    backoff_seconds: 5,
                },
                replan: ReplanPolicy {
                    triggers: vec![],
                },
                concurrency: ConcurrencyPolicy {
                    max_parallel_tasks: max_parallel,
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

    #[test]
    fn test_single_objective() {
        let spec = minimal_spec(vec!["do thing".into()], 1);
        let dag = generate_task_dag(&spec);
        assert_eq!(dag.nodes.len(), 1);
        assert_eq!(dag.nodes[0].objective, "do thing");
        assert!(dag.nodes[0].dependencies.is_empty());
        assert_eq!(dag.intent_spec_id, "test-id");
    }

    #[test]
    fn test_multiple_objectives_sequential() {
        let spec = minimal_spec(vec!["a".into(), "b".into(), "c".into()], 1);
        let dag = generate_task_dag(&spec);
        assert_eq!(dag.nodes.len(), 3);
        assert!(dag.nodes[0].dependencies.is_empty());
        assert_eq!(dag.nodes[1].dependencies, vec![dag.nodes[0].id]);
        assert_eq!(dag.nodes[2].dependencies, vec![dag.nodes[1].id]);
    }

    #[test]
    fn test_multiple_objectives_parallel() {
        let spec = minimal_spec(vec!["a".into(), "b".into(), "c".into()], 4);
        let dag = generate_task_dag(&spec);
        assert_eq!(dag.nodes.len(), 3);
        for node in &dag.nodes {
            assert!(node.dependencies.is_empty());
        }
    }

    #[test]
    fn test_constraint_propagation() {
        let spec = minimal_spec(vec!["do thing".into()], 1);
        let dag = generate_task_dag(&spec);
        assert!(dag.nodes[0].constraints.contains(&"network:deny".to_string()));
    }

    #[test]
    fn test_scope_propagation() {
        let spec = minimal_spec(vec!["do thing".into()], 1);
        let dag = generate_task_dag(&spec);
        assert_eq!(dag.nodes[0].scope_in, vec!["src/".to_string()]);
        assert_eq!(dag.nodes[0].scope_out, vec!["vendor/".to_string()]);
    }

    #[test]
    fn test_empty_objectives() {
        let spec = minimal_spec(vec![], 1);
        let dag = generate_task_dag(&spec);
        assert!(dag.nodes.is_empty());
    }
}
