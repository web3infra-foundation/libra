//! Canonical IntentSpec data model shared by drafting, validation, persistence, and
//! orchestration.
//!
//! Boundary: structs here are serialization contracts, so defaults must remain
//! backward compatible with existing JSON fixtures and stored intent objects. Tests in
//! `tests/intent_flow_test.rs` and `tests/ai_validation_decision_flow_test.rs` exercise
//! the edge cases where missing optional fields are filled by defaults.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IntentSpec {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub intent: Intent,
    pub acceptance: Acceptance,
    pub constraints: Constraints,
    pub risk: Risk,
    pub evidence: EvidencePolicy,
    pub security: SecurityPolicy,
    pub execution: ExecutionPolicy,
    pub artifacts: Artifacts,
    pub provenance: ProvenancePolicy,
    pub lifecycle: Lifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libra: Option<LibraBinding>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Metadata {
    pub id: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "createdBy")]
    pub created_by: CreatedBy,
    pub target: Target,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CreatedBy {
    #[serde(rename = "type")]
    pub creator_type: CreatorType,
    pub id: String,
    #[serde(
        rename = "displayName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CreatorType {
    User,
    Agent,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Target {
    pub repo: RepoTarget,
    #[serde(rename = "baseRef")]
    pub base_ref: String,
    #[serde(
        rename = "workspaceId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RepoTarget {
    #[serde(rename = "type")]
    pub repo_type: RepoType,
    pub locator: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RepoType {
    Git,
    Monorepo,
    Local,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Intent {
    pub summary: String,
    #[serde(rename = "problemStatement")]
    pub problem_statement: String,
    #[serde(rename = "changeType")]
    pub change_type: ChangeType,
    pub objectives: Vec<Objective>,
    #[serde(rename = "inScope")]
    pub in_scope: Vec<String>,
    #[serde(rename = "outOfScope", default)]
    pub out_of_scope: Vec<String>,
    #[serde(
        rename = "touchHints",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub touch_hints: Option<TouchHints>,
}

impl Intent {
    pub fn has_implementation_objectives(&self) -> bool {
        self.objectives
            .iter()
            .any(|objective| objective.kind == ObjectiveKind::Implementation)
    }

    pub fn is_analysis_only(&self) -> bool {
        !self.objectives.is_empty() && !self.has_implementation_objectives()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeType {
    Bugfix,
    Feature,
    Test,
    Refactor,
    Performance,
    Security,
    Docs,
    Chore,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Objective {
    pub title: String,
    #[serde(rename = "kind")]
    pub kind: ObjectiveKind,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ObjectiveKind {
    Implementation,
    Analysis,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TouchHints {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub symbols: Vec<String>,
    #[serde(default)]
    pub apis: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Acceptance {
    #[serde(rename = "successCriteria")]
    pub success_criteria: Vec<String>,
    #[serde(rename = "verificationPlan")]
    pub verification_plan: VerificationPlan,
    #[serde(
        rename = "qualityGates",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub quality_gates: Option<QualityGates>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VerificationPlan {
    #[serde(rename = "fastChecks", default)]
    pub fast_checks: Vec<Check>,
    #[serde(rename = "integrationChecks", default)]
    pub integration_checks: Vec<Check>,
    #[serde(rename = "securityChecks", default)]
    pub security_checks: Vec<Check>,
    #[serde(rename = "releaseChecks", default)]
    pub release_checks: Vec<Check>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Check {
    pub id: String,
    pub kind: CheckKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(
        rename = "timeoutSeconds",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_seconds: Option<u64>,
    #[serde(
        rename = "expectedExitCode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub expected_exit_code: Option<i32>,
    pub required: bool,
    #[serde(rename = "artifactsProduced", default)]
    pub artifacts_produced: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum CheckKind {
    Command,
    TestSuite,
    Policy,
}

impl<'de> Deserialize<'de> for CheckKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let normalized = value
            .trim()
            .chars()
            .filter(|ch| !matches!(ch, '-' | '_' | ' '))
            .flat_map(char::to_lowercase)
            .collect::<String>();

        match normalized.as_str() {
            "command" | "cmd" | "shell" | "run" | "build" | "lint" | "fmt" | "format"
            | "clippy" => Ok(Self::Command),
            "testsuite" | "test" | "tests" | "unittest" | "unittests" | "integrationtest"
            | "integrationtests" => Ok(Self::TestSuite),
            "policy" | "security" | "manual" | "review" | "release" => Ok(Self::Policy),
            other => Err(D::Error::unknown_variant(
                other,
                &[
                    "command",
                    "cmd",
                    "run",
                    "build",
                    "lint",
                    "testSuite",
                    "test",
                    "tests",
                    "policy",
                    "security",
                    "release",
                ],
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QualityGates {
    #[serde(
        rename = "requireNewTestsWhenBugfix",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub require_new_tests_when_bugfix: Option<bool>,
    #[serde(
        rename = "maxAllowedRegression",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_allowed_regression: Option<MaxAllowedRegression>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MaxAllowedRegression {
    None,
    Low,
    Medium,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Constraints {
    pub security: ConstraintSecurity,
    pub privacy: ConstraintPrivacy,
    pub licensing: ConstraintLicensing,
    pub platform: ConstraintPlatform,
    pub resources: ConstraintResources,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConstraintSecurity {
    #[serde(rename = "networkPolicy")]
    pub network_policy: NetworkPolicy,
    #[serde(rename = "dependencyPolicy")]
    pub dependency_policy: DependencyPolicy,
    #[serde(rename = "cryptoPolicy", default)]
    pub crypto_policy: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkPolicy {
    Deny,
    Allow,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyPolicy {
    NoNew,
    AllowWithReview,
    Allow,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConstraintPrivacy {
    #[serde(rename = "dataClassesAllowed")]
    pub data_classes_allowed: Vec<DataClass>,
    #[serde(rename = "redactionRequired")]
    pub redaction_required: bool,
    #[serde(rename = "retentionDays")]
    pub retention_days: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DataClass {
    Public,
    Internal,
    Confidential,
    Pii,
    Phi,
    Secrets,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConstraintLicensing {
    #[serde(rename = "allowedSpdx", default)]
    pub allowed_spdx: Vec<String>,
    #[serde(rename = "forbidNewLicenses")]
    pub forbid_new_licenses: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConstraintPlatform {
    #[serde(rename = "languageRuntime", default)]
    pub language_runtime: String,
    #[serde(rename = "supportedOS", default)]
    pub supported_os: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConstraintResources {
    #[serde(rename = "maxWallClockSeconds")]
    pub max_wall_clock_seconds: u32,
    #[serde(rename = "maxCostUnits")]
    pub max_cost_units: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Risk {
    pub level: RiskLevel,
    pub rationale: String,
    #[serde(default)]
    pub factors: Vec<String>,
    #[serde(rename = "humanInLoop")]
    pub human_in_loop: HumanInLoop,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl<'de> Deserialize<'de> for RiskLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            _ => Err(D::Error::custom(format!(
                "unknown risk level `{value}`, expected one of `low`, `medium`, `high`"
            ))),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HumanInLoop {
    pub required: bool,
    #[serde(rename = "minApprovers")]
    pub min_approvers: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EvidencePolicy {
    pub strategy: EvidenceStrategy,
    #[serde(rename = "trustTiers")]
    pub trust_tiers: Vec<TrustTier>,
    #[serde(rename = "domainAllowlistMode")]
    pub domain_allowlist_mode: DomainAllowlistMode,
    #[serde(rename = "allowedDomains", default)]
    pub allowed_domains: Vec<String>,
    #[serde(rename = "blockedDomains", default)]
    pub blocked_domains: Vec<String>,
    #[serde(rename = "minCitationsPerDecision")]
    pub min_citations_per_decision: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceStrategy {
    RepoFirst,
    PinnedOfficial,
    OpenWeb,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum TrustTier {
    Repo,
    VendorDoc,
    Standards,
    Web,
    UserProvided,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum DomainAllowlistMode {
    Disabled,
    AllowlistOnly,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SecurityPolicy {
    #[serde(rename = "toolAcl")]
    pub tool_acl: ToolAcl,
    pub secrets: SecretPolicy,
    #[serde(rename = "promptInjection")]
    pub prompt_injection: PromptInjectionPolicy,
    #[serde(rename = "outputHandling")]
    pub output_handling: OutputHandlingPolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolAcl {
    #[serde(default)]
    pub allow: Vec<ToolRule>,
    #[serde(default)]
    pub deny: Vec<ToolRule>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolRule {
    pub tool: String,
    pub actions: Vec<String>,
    #[serde(default)]
    pub constraints: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SecretPolicy {
    pub policy: SecretAccessPolicy,
    #[serde(rename = "allowedScopes", default)]
    pub allowed_scopes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SecretAccessPolicy {
    DenyAll,
    ReadOnlyScoped,
    AllowScoped,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PromptInjectionPolicy {
    #[serde(rename = "treatRetrievedContentAsUntrusted")]
    pub treat_retrieved_content_as_untrusted: bool,
    #[serde(rename = "enforceOutputSchema")]
    pub enforce_output_schema: bool,
    #[serde(rename = "disallowInstructionFromEvidence")]
    pub disallow_instruction_from_evidence: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OutputHandlingPolicy {
    #[serde(rename = "encodingPolicy")]
    pub encoding_policy: EncodingPolicy,
    #[serde(rename = "noDirectEval")]
    pub no_direct_eval: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum EncodingPolicy {
    ContextualEscape,
    StrictJson,
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecutionPolicy {
    pub retry: RetryPolicy,
    pub replan: ReplanPolicy,
    pub concurrency: ConcurrencyPolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RetryPolicy {
    #[serde(rename = "maxRetries")]
    pub max_retries: u8,
    #[serde(rename = "backoffSeconds")]
    pub backoff_seconds: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReplanPolicy {
    #[serde(default)]
    pub triggers: Vec<ReplanTrigger>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ReplanTrigger {
    EvidenceConflict,
    ScopeCreep,
    RepeatedTestFail,
    SecurityGateFail,
    UnknownApi,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConcurrencyPolicy {
    #[serde(rename = "maxParallelTasks")]
    pub max_parallel_tasks: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Artifacts {
    pub required: Vec<ArtifactReq>,
    #[serde(default)]
    pub retention: ArtifactRetention,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct ArtifactRetention {
    pub days: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ArtifactReq {
    pub name: ArtifactName,
    pub stage: ArtifactStage,
    pub required: bool,
    #[serde(default)]
    pub format: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactName {
    Patchset,
    TestLog,
    BuildLog,
    SastReport,
    ScaReport,
    Sbom,
    ProvenanceAttestation,
    TransparencyProof,
    ReleaseNotes,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactStage {
    PerTask,
    Integration,
    Security,
    Release,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProvenancePolicy {
    #[serde(rename = "requireSlsaProvenance")]
    pub require_slsa_provenance: bool,
    #[serde(rename = "requireSbom")]
    pub require_sbom: bool,
    #[serde(rename = "transparencyLog")]
    pub transparency_log: TransparencyLogPolicy,
    pub bindings: ProvenanceBindings,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TransparencyLogPolicy {
    pub mode: TransparencyMode,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum TransparencyMode {
    None,
    Rekor,
    InternalLedger,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProvenanceBindings {
    #[serde(rename = "embedIntentSpecDigest")]
    pub embed_intent_spec_digest: bool,
    #[serde(rename = "embedEvidenceDigests", default)]
    pub embed_evidence_digests: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Lifecycle {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    pub status: LifecycleStatus,
    #[serde(rename = "changeLog", default)]
    pub change_log: Vec<ChangeLogEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LifecycleStatus {
    Draft,
    Active,
    Deprecated,
    Closed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChangeLogEntry {
    pub at: String,
    pub by: String,
    pub reason: String,
    #[serde(rename = "diffSummary")]
    pub diff_summary: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LibraBinding {
    #[serde(
        rename = "objectStore",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub object_store: Option<ObjectStoreConfig>,
    #[serde(
        rename = "contextPipeline",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub context_pipeline: Option<ContextPipelineConfig>,
    #[serde(
        rename = "planGeneration",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub plan_generation: Option<PlanGenerationConfig>,
    #[serde(rename = "runPolicy", default, skip_serializing_if = "Option::is_none")]
    pub run_policy: Option<RunPolicyConfig>,
    #[serde(
        rename = "actorMapping",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub actor_mapping: Option<ActorMappingConfig>,
    #[serde(
        rename = "decisionPolicy",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub decision_policy: Option<DecisionPolicyConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ObjectStoreConfig {
    #[serde(default = "ObjectStoreConfig::default_backend")]
    pub backend: ObjectStoreBackend,
    #[serde(
        rename = "blobRetentionStrategy",
        default = "ObjectStoreConfig::default_blob_retention"
    )]
    pub blob_retention_strategy: BlobRetentionStrategy,
    #[serde(
        rename = "aiRefPrefix",
        default = "ObjectStoreConfig::default_ai_ref_prefix"
    )]
    pub ai_ref_prefix: String,
}

impl ObjectStoreConfig {
    fn default_backend() -> ObjectStoreBackend {
        ObjectStoreBackend::GitNative
    }
    fn default_blob_retention() -> BlobRetentionStrategy {
        BlobRetentionStrategy::RefAnchoring
    }
    fn default_ai_ref_prefix() -> String {
        "refs/ai/".to_string()
    }
}

impl Default for ObjectStoreConfig {
    fn default() -> Self {
        Self {
            backend: Self::default_backend(),
            blob_retention_strategy: Self::default_blob_retention(),
            ai_ref_prefix: Self::default_ai_ref_prefix(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectStoreBackend {
    GitNative,
    ExternalS3,
    ExternalLocal,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum BlobRetentionStrategy {
    RefAnchoring,
    OrphanCommit,
    KeepPack,
    CustomGc,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContextPipelineConfig {
    #[serde(
        rename = "maxFrames",
        default = "ContextPipelineConfig::default_max_frames"
    )]
    pub max_frames: u32,
    #[serde(
        rename = "seedFrameKind",
        default = "ContextPipelineConfig::default_seed_frame_kind"
    )]
    pub seed_frame_kind: SeedFrameKind,
    #[serde(rename = "checkpointOnReplan", default = "default_true")]
    pub checkpoint_on_replan: bool,
}

impl ContextPipelineConfig {
    fn default_max_frames() -> u32 {
        128
    }
    fn default_seed_frame_kind() -> SeedFrameKind {
        SeedFrameKind::IntentAnalysis
    }
}

impl Default for ContextPipelineConfig {
    fn default() -> Self {
        Self {
            max_frames: Self::default_max_frames(),
            seed_frame_kind: Self::default_seed_frame_kind(),
            checkpoint_on_replan: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SeedFrameKind {
    IntentAnalysis,
    Checkpoint,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlanGenerationConfig {
    #[serde(
        rename = "decompositionMode",
        default = "PlanGenerationConfig::default_decomposition"
    )]
    pub decomposition_mode: DecompositionMode,
    #[serde(
        rename = "conflictResolution",
        default = "PlanGenerationConfig::default_conflict_resolution"
    )]
    pub conflict_resolution: ConflictResolution,
    #[serde(rename = "gateTaskPerStage", default = "default_true")]
    pub gate_task_per_stage: bool,
}

impl PlanGenerationConfig {
    fn default_decomposition() -> DecompositionMode {
        DecompositionMode::PerObjective
    }
    fn default_conflict_resolution() -> ConflictResolution {
        ConflictResolution::ForceSerial
    }
}

impl Default for PlanGenerationConfig {
    fn default() -> Self {
        Self {
            decomposition_mode: Self::default_decomposition(),
            conflict_resolution: Self::default_conflict_resolution(),
            gate_task_per_stage: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum DecompositionMode {
    PerObjective,
    PerFileCluster,
    SingleTask,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictResolution {
    MergeTasks,
    ForceSerial,
    FailFast,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RunPolicyConfig {
    #[serde(
        rename = "patchsetFormat",
        default = "RunPolicyConfig::default_patchset_format"
    )]
    pub patchset_format: PatchsetFormat,
    #[serde(rename = "snapshotOnRunStart", default = "default_true")]
    pub snapshot_on_run_start: bool,
    #[serde(
        rename = "metricsSchema",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub metrics_schema: Option<Value>,
}

impl RunPolicyConfig {
    fn default_patchset_format() -> PatchsetFormat {
        PatchsetFormat::GitDiff
    }
}

impl Default for RunPolicyConfig {
    fn default() -> Self {
        Self {
            patchset_format: Self::default_patchset_format(),
            snapshot_on_run_start: true,
            metrics_schema: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum PatchsetFormat {
    Unified,
    GitDiff,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ActorMappingConfig {
    /// Legacy compatibility field for the Scheduler actor ID.
    #[serde(
        rename = "orchestratorActorId",
        default = "ActorMappingConfig::default_orchestrator"
    )]
    pub orchestrator_actor_id: String,
    #[serde(rename = "coderActorId", default = "ActorMappingConfig::default_coder")]
    pub coder_actor_id: String,
    #[serde(
        rename = "reviewerActorId",
        default = "ActorMappingConfig::default_reviewer"
    )]
    pub reviewer_actor_id: String,
}

impl ActorMappingConfig {
    fn default_orchestrator() -> String {
        "libra-orchestrator".to_string()
    }
    fn default_coder() -> String {
        "libra-coder".to_string()
    }
    fn default_reviewer() -> String {
        "libra-reviewer".to_string()
    }
}

impl Default for ActorMappingConfig {
    fn default() -> Self {
        Self {
            orchestrator_actor_id: Self::default_orchestrator(),
            coder_actor_id: Self::default_coder(),
            reviewer_actor_id: Self::default_reviewer(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DecisionPolicyConfig {
    #[serde(rename = "abandonOnSecurityGateFail", default = "default_true")]
    pub abandon_on_security_gate_fail: bool,
    #[serde(rename = "checkpointBeforeReplan", default = "default_true")]
    pub checkpoint_before_replan: bool,
    #[serde(rename = "rollbackOnProvenanceFail", default = "default_true")]
    pub rollback_on_provenance_fail: bool,
}

impl Default for DecisionPolicyConfig {
    fn default() -> Self {
        Self {
            abandon_on_security_gate_fail: true,
            checkpoint_before_replan: true,
            rollback_on_provenance_fail: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_libra_binding_serde_roundtrip_full() {
        let binding = LibraBinding {
            object_store: Some(ObjectStoreConfig {
                backend: ObjectStoreBackend::ExternalS3,
                blob_retention_strategy: BlobRetentionStrategy::OrphanCommit,
                ai_ref_prefix: "refs/custom/".into(),
            }),
            context_pipeline: Some(ContextPipelineConfig {
                max_frames: 64,
                seed_frame_kind: SeedFrameKind::Checkpoint,
                checkpoint_on_replan: false,
            }),
            plan_generation: Some(PlanGenerationConfig {
                decomposition_mode: DecompositionMode::PerFileCluster,
                conflict_resolution: ConflictResolution::MergeTasks,
                gate_task_per_stage: false,
            }),
            run_policy: Some(RunPolicyConfig {
                patchset_format: PatchsetFormat::Unified,
                snapshot_on_run_start: false,
                metrics_schema: Some(serde_json::json!({"type": "object"})),
            }),
            actor_mapping: Some(ActorMappingConfig {
                orchestrator_actor_id: "custom-orch".into(),
                coder_actor_id: "custom-coder".into(),
                reviewer_actor_id: "custom-reviewer".into(),
            }),
            decision_policy: Some(DecisionPolicyConfig {
                abandon_on_security_gate_fail: false,
                checkpoint_before_replan: false,
                rollback_on_provenance_fail: false,
            }),
        };
        let json = serde_json::to_string(&binding).unwrap();
        let back: LibraBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(binding, back);
    }

    #[test]
    fn test_libra_binding_serde_roundtrip_empty() {
        let binding = LibraBinding {
            object_store: None,
            context_pipeline: None,
            plan_generation: None,
            run_policy: None,
            actor_mapping: None,
            decision_policy: None,
        };
        let json = serde_json::to_string(&binding).unwrap();
        let back: LibraBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(binding, back);
    }

    #[test]
    fn test_libra_binding_deserialize_defaults() {
        // Deserialize from empty JSON object — all fields should get defaults
        let json = r#"{"objectStore": {}, "contextPipeline": {}, "planGeneration": {}, "runPolicy": {}, "actorMapping": {}, "decisionPolicy": {}}"#;
        let binding: LibraBinding = serde_json::from_str(json).unwrap();

        let os = binding.object_store.unwrap();
        assert_eq!(os.backend, ObjectStoreBackend::GitNative);
        assert_eq!(
            os.blob_retention_strategy,
            BlobRetentionStrategy::RefAnchoring
        );
        assert_eq!(os.ai_ref_prefix, "refs/ai/");

        let cp = binding.context_pipeline.unwrap();
        assert_eq!(cp.max_frames, 128);
        assert_eq!(cp.seed_frame_kind, SeedFrameKind::IntentAnalysis);
        assert!(cp.checkpoint_on_replan);

        let pg = binding.plan_generation.unwrap();
        assert_eq!(pg.decomposition_mode, DecompositionMode::PerObjective);
        assert_eq!(pg.conflict_resolution, ConflictResolution::ForceSerial);
        assert!(pg.gate_task_per_stage);

        let rp = binding.run_policy.unwrap();
        assert_eq!(rp.patchset_format, PatchsetFormat::GitDiff);
        assert!(rp.snapshot_on_run_start);
        assert!(rp.metrics_schema.is_none());

        let am = binding.actor_mapping.unwrap();
        assert_eq!(am.orchestrator_actor_id, "libra-orchestrator");
        assert_eq!(am.coder_actor_id, "libra-coder");
        assert_eq!(am.reviewer_actor_id, "libra-reviewer");

        let dp = binding.decision_policy.unwrap();
        assert!(dp.abandon_on_security_gate_fail);
        assert!(dp.checkpoint_before_replan);
        assert!(dp.rollback_on_provenance_fail);
    }

    #[test]
    fn test_object_store_config_serde_roundtrip() {
        let config = ObjectStoreConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let back: ObjectStoreConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn test_decision_policy_config_serde_roundtrip() {
        let config = DecisionPolicyConfig {
            abandon_on_security_gate_fail: false,
            checkpoint_before_replan: true,
            rollback_on_provenance_fail: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: DecisionPolicyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }
}
