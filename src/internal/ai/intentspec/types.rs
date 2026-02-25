use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
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
    pub objectives: Vec<String>,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeType {
    Bugfix,
    Feature,
    Refactor,
    Performance,
    Security,
    Docs,
    Chore,
    Unknown,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum CheckKind {
    Command,
    TestSuite,
    Policy,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
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
    pub object_store: Option<Value>,
    #[serde(
        rename = "contextPipeline",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub context_pipeline: Option<Value>,
    #[serde(
        rename = "planGeneration",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub plan_generation: Option<Value>,
    #[serde(rename = "runPolicy", default, skip_serializing_if = "Option::is_none")]
    pub run_policy: Option<Value>,
    #[serde(
        rename = "actorMapping",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub actor_mapping: Option<Value>,
    #[serde(
        rename = "decisionPolicy",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub decision_policy: Option<Value>,
}
