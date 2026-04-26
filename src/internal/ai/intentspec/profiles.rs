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
}
