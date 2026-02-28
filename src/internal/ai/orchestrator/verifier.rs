use std::path::Path;

use super::gate;
use super::types::{GateReport, SystemReport};
use crate::internal::ai::intentspec::types::IntentSpec;

/// Execute Phase 3 system-level verification checks.
///
/// Runs integration, security, and release checks sequentially.
/// Each stage blocks the next on failure of required checks.
pub async fn run_system_verification(
    spec: &IntentSpec,
    working_dir: &Path,
) -> SystemReport {
    let plan = &spec.acceptance.verification_plan;

    let integration = gate::run_gates(&plan.integration_checks, working_dir).await;
    let security = if integration.all_required_passed {
        gate::run_gates(&plan.security_checks, working_dir).await
    } else {
        GateReport::empty()
    };
    let release = if integration.all_required_passed && security.all_required_passed {
        gate::run_gates(&plan.release_checks, working_dir).await
    } else {
        GateReport::empty()
    };

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::types::*;
    use std::collections::BTreeMap;

    fn spec_with_checks(
        integration: Vec<Check>,
        security: Vec<Check>,
        release: Vec<Check>,
    ) -> IntentSpec {
        IntentSpec {
            api_version: "intentspec.io/v1alpha1".into(),
            kind: "IntentSpec".into(),
            metadata: Metadata {
                id: "test".into(),
                created_at: "2025-01-01T00:00:00Z".into(),
                created_by: CreatedBy {
                    creator_type: CreatorType::User,
                    id: "test".into(),
                    display_name: None,
                },
                target: Target {
                    repo: RepoTarget {
                        repo_type: RepoType::Local,
                        locator: "/tmp".into(),
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
                objectives: vec![],
                in_scope: vec![],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: Acceptance {
                success_criteria: vec![],
                verification_plan: VerificationPlan {
                    fast_checks: vec![],
                    integration_checks: integration,
                    security_checks: security,
                    release_checks: release,
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
                    data_classes_allowed: vec![],
                    redaction_required: false,
                    retention_days: 90,
                },
                licensing: ConstraintLicensing {
                    allowed_spdx: vec![],
                    forbid_new_licenses: false,
                },
                platform: ConstraintPlatform {
                    language_runtime: String::new(),
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
                trust_tiers: vec![],
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

    fn passing_check(id: &str) -> Check {
        Check {
            id: id.into(),
            kind: CheckKind::Command,
            command: Some("true".into()),
            timeout_seconds: Some(10),
            expected_exit_code: None,
            required: true,
            artifacts_produced: vec![],
        }
    }

    fn failing_check(id: &str) -> Check {
        Check {
            id: id.into(),
            kind: CheckKind::Command,
            command: Some("false".into()),
            timeout_seconds: Some(10),
            expected_exit_code: None,
            required: true,
            artifacts_produced: vec![],
        }
    }

    #[tokio::test]
    async fn test_all_pass() {
        let spec = spec_with_checks(
            vec![passing_check("i1")],
            vec![passing_check("s1")],
            vec![passing_check("r1")],
        );
        let dir = tempfile::tempdir().unwrap();
        let report = run_system_verification(&spec, dir.path()).await;
        assert!(report.overall_passed);
        assert!(report.integration.all_required_passed);
        assert!(report.security.all_required_passed);
        assert!(report.release.all_required_passed);
    }

    #[tokio::test]
    async fn test_integration_fails_skips_rest() {
        let spec = spec_with_checks(
            vec![failing_check("i1")],
            vec![passing_check("s1")],
            vec![passing_check("r1")],
        );
        let dir = tempfile::tempdir().unwrap();
        let report = run_system_verification(&spec, dir.path()).await;
        assert!(!report.overall_passed);
        assert!(!report.integration.all_required_passed);
        // Security and release should be empty (skipped)
        assert!(report.security.results.is_empty());
        assert!(report.release.results.is_empty());
    }

    #[tokio::test]
    async fn test_security_fails_skips_release() {
        let spec = spec_with_checks(
            vec![passing_check("i1")],
            vec![failing_check("s1")],
            vec![passing_check("r1")],
        );
        let dir = tempfile::tempdir().unwrap();
        let report = run_system_verification(&spec, dir.path()).await;
        assert!(!report.overall_passed);
        assert!(report.integration.all_required_passed);
        assert!(!report.security.all_required_passed);
        assert!(report.release.results.is_empty());
    }

    #[tokio::test]
    async fn test_no_checks() {
        let spec = spec_with_checks(vec![], vec![], vec![]);
        let dir = tempfile::tempdir().unwrap();
        let report = run_system_verification(&spec, dir.path()).await;
        assert!(report.overall_passed);
    }

    #[tokio::test]
    async fn test_optional_failure_passes() {
        let optional_fail = Check {
            id: "opt".into(),
            kind: CheckKind::Command,
            command: Some("false".into()),
            timeout_seconds: Some(10),
            expected_exit_code: None,
            required: false,
            artifacts_produced: vec![],
        };
        let spec = spec_with_checks(vec![optional_fail], vec![], vec![]);
        let dir = tempfile::tempdir().unwrap();
        let report = run_system_verification(&spec, dir.path()).await;
        assert!(report.overall_passed);
    }
}
