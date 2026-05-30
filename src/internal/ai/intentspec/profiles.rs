//! IntentSpec profile templates that convert common request classes into default
//! checks, artifacts, and risk metadata.
//!
//! Boundary: profiles provide defaults only; user-supplied constraints and acceptance
//! criteria remain authoritative. Intent draft tests cover profile selection and ensure
//! generated defaults do not remove explicit user fields.

use std::collections::BTreeMap;

use super::types::{
    ArtifactName, ArtifactReq, ArtifactRetention, ArtifactStage, Artifacts, ConcurrencyPolicy,
    ConstraintLicensing, ConstraintPlatform, ConstraintPrivacy, ConstraintResources,
    ConstraintSecurity, Constraints, DataClass, DependencyPolicy, DomainAllowlistMode,
    EncodingPolicy, EvidencePolicy, EvidenceStrategy, ExecutionPolicy, HumanInLoop, NetworkPolicy,
    OutputHandlingPolicy, PromptInjectionPolicy, ProvenanceBindings, ProvenancePolicy,
    ReplanPolicy, ReplanTrigger, RetryPolicy, Risk, RiskLevel, SecretAccessPolicy, SecretPolicy,
    SecurityPolicy, ToolAcl, ToolRule, TransparencyLogPolicy, TransparencyMode, TrustTier,
};

pub fn default_risk(risk_level: RiskLevel, rationale: String, factors: Vec<String>) -> Risk {
    let human_in_loop = match risk_level {
        RiskLevel::Low => HumanInLoop {
            required: false,
            min_approvers: 0,
        },
        RiskLevel::Medium => HumanInLoop {
            required: true,
            min_approvers: 1,
        },
        RiskLevel::High => HumanInLoop {
            required: true,
            min_approvers: 2,
        },
    };

    Risk {
        level: risk_level,
        rationale,
        factors,
        human_in_loop,
    }
}

pub fn default_constraints(risk_level: RiskLevel) -> Constraints {
    let dependency_policy = match risk_level {
        RiskLevel::Low | RiskLevel::High => DependencyPolicy::NoNew,
        RiskLevel::Medium => DependencyPolicy::AllowWithReview,
    };

    Constraints {
        security: ConstraintSecurity {
            network_policy: NetworkPolicy::Deny,
            dependency_policy,
            crypto_policy: String::new(),
        },
        privacy: ConstraintPrivacy {
            data_classes_allowed: vec![DataClass::Public],
            redaction_required: true,
            retention_days: match risk_level {
                RiskLevel::Low => 7,
                RiskLevel::Medium => 30,
                RiskLevel::High => 14,
            },
        },
        licensing: ConstraintLicensing {
            allowed_spdx: Vec::new(),
            forbid_new_licenses: false,
        },
        platform: ConstraintPlatform {
            language_runtime: "rust-2024".to_string(),
            supported_os: vec!["linux".to_string(), "darwin".to_string()],
        },
        resources: ConstraintResources {
            max_wall_clock_seconds: match risk_level {
                RiskLevel::Low => 3600,
                RiskLevel::Medium => 14400,
                RiskLevel::High => 28800,
            },
            max_cost_units: 0,
        },
    }
}

pub fn default_evidence(risk_level: RiskLevel) -> EvidencePolicy {
    let min_citations = match risk_level {
        RiskLevel::Low => 0,
        RiskLevel::Medium => 2,
        RiskLevel::High => 3,
    };
    EvidencePolicy {
        strategy: match risk_level {
            RiskLevel::Medium => EvidenceStrategy::PinnedOfficial,
            RiskLevel::Low | RiskLevel::High => EvidenceStrategy::RepoFirst,
        },
        trust_tiers: vec![TrustTier::Repo, TrustTier::Standards, TrustTier::VendorDoc],
        domain_allowlist_mode: DomainAllowlistMode::AllowlistOnly,
        allowed_domains: Vec::new(),
        blocked_domains: Vec::new(),
        min_citations_per_decision: min_citations,
    }
}

pub fn default_security() -> SecurityPolicy {
    SecurityPolicy {
        tool_acl: ToolAcl {
            allow: vec![
                ToolRule {
                    tool: "workspace.fs".to_string(),
                    actions: vec!["read".to_string(), "write".to_string()],
                    constraints: BTreeMap::new(),
                },
                ToolRule {
                    tool: "libra.vcs".to_string(),
                    actions: vec!["read".to_string(), "write".to_string()],
                    constraints: BTreeMap::new(),
                },
                ToolRule {
                    tool: "web.search".to_string(),
                    actions: vec!["query".to_string()],
                    constraints: BTreeMap::new(),
                },
            ],
            deny: Vec::new(),
        },
        secrets: SecretPolicy {
            policy: SecretAccessPolicy::DenyAll,
            allowed_scopes: Vec::new(),
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
    }
}

pub fn default_execution(risk_level: RiskLevel) -> ExecutionPolicy {
    ExecutionPolicy {
        retry: RetryPolicy {
            max_retries: if matches!(risk_level, RiskLevel::High) {
                2
            } else {
                3
            },
            backoff_seconds: 10,
        },
        replan: ReplanPolicy {
            triggers: vec![
                ReplanTrigger::RepeatedTestFail,
                ReplanTrigger::SecurityGateFail,
                ReplanTrigger::EvidenceConflict,
            ],
        },
        concurrency: ConcurrencyPolicy {
            max_parallel_tasks: match risk_level {
                RiskLevel::Low => 2,
                RiskLevel::Medium => 4,
                RiskLevel::High => 1,
            },
        },
    }
}

pub fn default_artifacts(risk_level: RiskLevel, has_implementation_work: bool) -> Artifacts {
    let mut required = Vec::new();

    if has_implementation_work {
        required.push(ArtifactReq {
            name: ArtifactName::Patchset,
            stage: ArtifactStage::PerTask,
            required: true,
            format: "git-diff".to_string(),
        });
    }

    if has_implementation_work && matches!(risk_level, RiskLevel::Medium | RiskLevel::High) {
        required.push(ArtifactReq {
            name: ArtifactName::SastReport,
            stage: ArtifactStage::Security,
            required: true,
            format: "sarif".to_string(),
        });
        required.push(ArtifactReq {
            name: ArtifactName::ScaReport,
            stage: ArtifactStage::Security,
            required: true,
            format: "json".to_string(),
        });
        required.push(ArtifactReq {
            name: ArtifactName::Sbom,
            stage: ArtifactStage::Security,
            required: true,
            format: "spdx-json".to_string(),
        });
    }

    if has_implementation_work && matches!(risk_level, RiskLevel::High) {
        required.push(ArtifactReq {
            name: ArtifactName::ProvenanceAttestation,
            stage: ArtifactStage::Release,
            required: true,
            format: "in-toto+json".to_string(),
        });
        required.push(ArtifactReq {
            name: ArtifactName::TransparencyProof,
            stage: ArtifactStage::Release,
            required: true,
            format: "rekor-inclusion-proof".to_string(),
        });
    }

    Artifacts {
        required,
        retention: ArtifactRetention {
            days: match risk_level {
                RiskLevel::Low => 7,
                RiskLevel::Medium => 180,
                RiskLevel::High => 365,
            },
        },
    }
}

pub fn default_provenance(
    risk_level: RiskLevel,
    has_implementation_work: bool,
) -> ProvenancePolicy {
    let require_provenance = has_implementation_work && !matches!(risk_level, RiskLevel::Low);
    let require_sbom =
        has_implementation_work && matches!(risk_level, RiskLevel::Medium | RiskLevel::High);
    let mode = if !has_implementation_work || matches!(risk_level, RiskLevel::Low) {
        TransparencyMode::None
    } else {
        TransparencyMode::Rekor
    };
    ProvenancePolicy {
        require_slsa_provenance: require_provenance,
        require_sbom,
        transparency_log: TransparencyLogPolicy { mode },
        bindings: ProvenanceBindings {
            embed_intent_spec_digest: true,
            embed_evidence_digests: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_only_defaults_do_not_require_patchset() {
        let artifacts = default_artifacts(RiskLevel::Low, false);
        assert!(
            !artifacts
                .required
                .iter()
                .any(|req| req.name == ArtifactName::Patchset),
            "{:?}",
            artifacts.required
        );
    }

    #[test]
    fn implementation_defaults_require_patchset() {
        let artifacts = default_artifacts(RiskLevel::Low, true);
        assert!(
            artifacts
                .required
                .iter()
                .any(|req| req.name == ArtifactName::Patchset),
            "{:?}",
            artifacts.required
        );
    }

    #[test]
    fn implementation_defaults_do_not_require_test_log_without_explicit_checks() {
        let artifacts = default_artifacts(RiskLevel::Low, true);
        assert!(
            !artifacts
                .required
                .iter()
                .any(|req| req.name == ArtifactName::TestLog),
            "{:?}",
            artifacts.required
        );
    }

    #[test]
    fn analysis_only_medium_risk_defaults_do_not_require_security_artifacts() {
        let artifacts = default_artifacts(RiskLevel::Medium, false);
        assert!(artifacts.required.is_empty(), "{:?}", artifacts.required);
    }

    #[test]
    fn analysis_only_defaults_disable_provenance_requirements() {
        let provenance = default_provenance(RiskLevel::High, false);
        assert!(!provenance.require_slsa_provenance);
        assert!(!provenance.require_sbom);
        assert_eq!(provenance.transparency_log.mode, TransparencyMode::None);
    }

    /// `default_risk` HumanInLoop escalation table per risk level:
    /// Low → not required / 0 approvers
    /// Medium → required / 1 approver
    /// High → required / 2 approvers
    /// Pin the table so a re-tune is detected here, not at validation
    /// time.
    #[test]
    fn default_risk_human_in_loop_escalates_per_risk_level() {
        let low = default_risk(RiskLevel::Low, "low risk".into(), vec![]);
        assert!(!low.human_in_loop.required);
        assert_eq!(low.human_in_loop.min_approvers, 0);

        let medium = default_risk(RiskLevel::Medium, "medium risk".into(), vec![]);
        assert!(medium.human_in_loop.required);
        assert_eq!(medium.human_in_loop.min_approvers, 1);

        let high = default_risk(RiskLevel::High, "high risk".into(), vec![]);
        assert!(high.human_in_loop.required);
        assert_eq!(high.human_in_loop.min_approvers, 2);
    }

    /// `default_constraints` retention_days table:
    /// Low → 7, Medium → 30, High → 14 (note: High's retention is
    /// lower than Medium's, which is intentional — see the privacy
    /// constraint that bounds High-risk data exposure).
    #[test]
    fn default_constraints_retention_days_per_risk_level() {
        assert_eq!(
            default_constraints(RiskLevel::Low).privacy.retention_days,
            7
        );
        assert_eq!(
            default_constraints(RiskLevel::Medium)
                .privacy
                .retention_days,
            30
        );
        // Pin the High = 14 anomaly so a future "monotonic" refactor
        // doesn't silently raise it.
        assert_eq!(
            default_constraints(RiskLevel::High).privacy.retention_days,
            14
        );
    }

    /// `default_constraints` max_wall_clock_seconds table:
    /// Low → 3600 (1h), Medium → 14400 (4h), High → 28800 (8h).
    #[test]
    fn default_constraints_max_wall_clock_seconds_per_risk_level() {
        assert_eq!(
            default_constraints(RiskLevel::Low)
                .resources
                .max_wall_clock_seconds,
            3600,
        );
        assert_eq!(
            default_constraints(RiskLevel::Medium)
                .resources
                .max_wall_clock_seconds,
            14400,
        );
        assert_eq!(
            default_constraints(RiskLevel::High)
                .resources
                .max_wall_clock_seconds,
            28800,
        );
    }

    /// `default_constraints` dependency_policy table: Low/High both
    /// use NoNew; Medium uses AllowWithReview. Pin the asymmetric
    /// pattern so a future "linear" refactor doesn't silently allow
    /// new dependencies at High risk.
    #[test]
    fn default_constraints_dependency_policy_low_and_high_match() {
        assert_eq!(
            default_constraints(RiskLevel::Low)
                .security
                .dependency_policy,
            DependencyPolicy::NoNew,
        );
        assert_eq!(
            default_constraints(RiskLevel::High)
                .security
                .dependency_policy,
            DependencyPolicy::NoNew,
        );
        assert_eq!(
            default_constraints(RiskLevel::Medium)
                .security
                .dependency_policy,
            DependencyPolicy::AllowWithReview,
        );
    }

    /// `default_evidence` min_citations escalates with risk:
    /// Low → 0, Medium → 2, High → 3.
    #[test]
    fn default_evidence_min_citations_escalates_per_risk_level() {
        assert_eq!(
            default_evidence(RiskLevel::Low).min_citations_per_decision,
            0,
        );
        assert_eq!(
            default_evidence(RiskLevel::Medium).min_citations_per_decision,
            2,
        );
        assert_eq!(
            default_evidence(RiskLevel::High).min_citations_per_decision,
            3,
        );
    }

    /// `default_evidence` strategy table: Medium uses PinnedOfficial;
    /// Low and High both use RepoFirst. Pin the asymmetric pattern.
    #[test]
    fn default_evidence_strategy_medium_vs_low_and_high() {
        assert_eq!(
            default_evidence(RiskLevel::Low).strategy,
            EvidenceStrategy::RepoFirst,
        );
        assert_eq!(
            default_evidence(RiskLevel::Medium).strategy,
            EvidenceStrategy::PinnedOfficial,
        );
        assert_eq!(
            default_evidence(RiskLevel::High).strategy,
            EvidenceStrategy::RepoFirst,
        );
    }

    /// `default_execution` parallelism table:
    /// Low → 2 parallel, Medium → 4, High → 1 (serial for safety).
    /// max_retries table: Low/Medium → 3, High → 2 (less retry budget
    /// to fail fast).
    #[test]
    fn default_execution_concurrency_and_retry_per_risk_level() {
        let low = default_execution(RiskLevel::Low);
        assert_eq!(low.concurrency.max_parallel_tasks, 2);
        assert_eq!(low.retry.max_retries, 3);

        let medium = default_execution(RiskLevel::Medium);
        assert_eq!(medium.concurrency.max_parallel_tasks, 4);
        assert_eq!(medium.retry.max_retries, 3);

        let high = default_execution(RiskLevel::High);
        assert_eq!(high.concurrency.max_parallel_tasks, 1);
        assert_eq!(high.retry.max_retries, 2);
    }

    /// Medium-risk + implementation-work must add SAST/SCA/SBOM but
    /// NOT provenance/transparency artifacts. Pins the "Medium = some
    /// security artifacts, no release artifacts" boundary.
    #[test]
    fn default_artifacts_medium_risk_impl_adds_security_no_release() {
        let artifacts = default_artifacts(RiskLevel::Medium, true);
        let names: Vec<&ArtifactName> = artifacts.required.iter().map(|a| &a.name).collect();

        assert!(names.contains(&&ArtifactName::Patchset));
        assert!(names.contains(&&ArtifactName::SastReport));
        assert!(names.contains(&&ArtifactName::ScaReport));
        assert!(names.contains(&&ArtifactName::Sbom));

        assert!(!names.contains(&&ArtifactName::ProvenanceAttestation));
        assert!(!names.contains(&&ArtifactName::TransparencyProof));
    }

    /// High-risk + implementation-work must add SAST/SCA/SBOM AND
    /// provenance/transparency. Pins the High-risk artifact set is
    /// a strict superset of Medium-risk.
    #[test]
    fn default_artifacts_high_risk_impl_adds_security_and_release() {
        let artifacts = default_artifacts(RiskLevel::High, true);
        let names: Vec<&ArtifactName> = artifacts.required.iter().map(|a| &a.name).collect();

        assert!(names.contains(&&ArtifactName::Patchset));
        assert!(names.contains(&&ArtifactName::SastReport));
        assert!(names.contains(&&ArtifactName::ScaReport));
        assert!(names.contains(&&ArtifactName::Sbom));
        assert!(names.contains(&&ArtifactName::ProvenanceAttestation));
        assert!(names.contains(&&ArtifactName::TransparencyProof));
    }

    /// `default_artifacts` retention.days table:
    /// Low → 7 days, Medium → 180 days, High → 365 days.
    /// Note: this is the **artifact** retention, distinct from the
    /// privacy retention in `default_constraints` (Low=7, Medium=30,
    /// High=14). The validator clamps these to the minimum.
    #[test]
    fn default_artifacts_retention_days_per_risk_level() {
        assert_eq!(default_artifacts(RiskLevel::Low, true).retention.days, 7);
        assert_eq!(
            default_artifacts(RiskLevel::Medium, true).retention.days,
            180
        );
        assert_eq!(default_artifacts(RiskLevel::High, true).retention.days, 365);
    }

    /// `default_provenance` transparency_log: only `High` + impl-work
    /// uses `Rekor`; everything else uses `None`. Pin the lookup table.
    #[test]
    fn default_provenance_transparency_log_mode_per_risk_level() {
        assert_eq!(
            default_provenance(RiskLevel::Low, true)
                .transparency_log
                .mode,
            TransparencyMode::None,
        );
        assert_eq!(
            default_provenance(RiskLevel::Medium, true)
                .transparency_log
                .mode,
            TransparencyMode::Rekor,
        );
        assert_eq!(
            default_provenance(RiskLevel::High, true)
                .transparency_log
                .mode,
            TransparencyMode::Rekor,
        );

        // Analysis-only suppresses Rekor at every level.
        assert_eq!(
            default_provenance(RiskLevel::High, false)
                .transparency_log
                .mode,
            TransparencyMode::None,
        );
    }
}
