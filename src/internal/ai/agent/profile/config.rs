//! Declarative TOML configuration for the multi-agent runtime
//! (OC-Phase 5 P5.1).
//!
//! `AgentsConfig` parses the user's `agents.toml` (or the
//! `[code.*]` sections of a wider `libra.toml`) into a typed,
//! validated tree. The schema covers:
//!
//! | Section                                | Purpose                                                        |
//! |----------------------------------------|----------------------------------------------------------------|
//! | `[code.multi_agent]`                   | Feature flag + depth / concurrency caps                         |
//! | `[code.goal]`                          | Goal-mode flag + continuation policy                            |
//! | `[code.agents.<name>]`                 | Per-agent execution spec (mode, model, tools, permission, steps)|
//! | `[code.compaction]`                    | Override compaction model + budget knobs                        |
//! | `[code.budget]`                        | Session-wide cost / token thresholds                            |
//! | `[code.budget.goal]`                   | Goal-loop cost / wall-clock thresholds                          |
//! | `[code.budget.per_agent.<name>]`       | Per-agent cost / step caps                                      |
//!
//! ## Scope
//!
//! P5.1 only delivers the schema, the loader, and validation. Budget
//! enforcement (P5.3), the usage-stats migration (P5.2), and TUI
//! command surfaces (P5.4) consume this struct without modifying it.
//!
//! ## Validation contract
//!
//! `AgentsConfig::validate` runs after deserialisation and returns a
//! list of every problem it finds (rather than a single first-error)
//! so an operator hand-editing the TOML sees the complete punch-list
//! in one pass. The rules:
//!
//! - Every `[code.agents.<name>]` model string parses through
//!   [`ModelBinding::parse`].
//! - Every `mode` string is a valid [`AgentMode`].
//! - Every `tool` list entry is a non-empty trimmed string.
//! - Every `warn_*` threshold is strictly less than its `max_*` peer
//!   (so warnings fire before the hard cap).
//! - `max_concurrent_subagents` and `max_subagent_depth` are at
//!   least 1 when `multi_agent.enabled = true`.
//! - `subagent_timeout_ms` is greater than 0 when
//!   `multi_agent.enabled = true`.
//! - `auto_continue_on_resume` is one of `"ask"`, `"auto"`, `"never"`.
//! - Per-agent `[code.budget.per_agent.<name>]` references an agent
//!   declared under `[code.agents.<name>]`.
//!
//! Rules that depend on runtime state (e.g. whether the catalogued
//! provider exists) are NOT enforced here; the factory layer
//! (OC-Phase 1) owns those.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::spec::{
    AgentExecutionSpec, AgentMode, AgentPermissionSpec, ModelBinding, ToolSelection,
};

/// Top-level config. Every subsection is optional so a partial
/// `agents.toml` (e.g. only `[code.multi_agent]`) parses cleanly and
/// the rest fall back to documented defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentsConfig {
    #[serde(default)]
    pub multi_agent: MultiAgentConfig,

    #[serde(default)]
    pub goal: GoalConfig,

    /// `[code.sub_agents]` — the CEX-S2-12 flag-gated sub-agent
    /// runtime. Defaults to `enabled = false` so existing
    /// single-Agent installs are byte-equivalent pre/post upgrade.
    /// Schema-only landing today: the runtime that consumes this
    /// flag (single sub-agent behind flag + hook dispatch) is the
    /// substantive CEX-S2-12 work and stays in its own card. Adding
    /// the flag now lets future runtime code branch on
    /// `cfg.sub_agents.enabled` without an extra schema patch.
    ///
    /// Distinct from [`multi_agent`](Self::multi_agent): that flag is
    /// the OC-Phase 5 user-facing multi-agent mode (`/agents` slash
    /// command, model bindings, etc.); `sub_agents` is the
    /// CEX-S2-12 runtime gate for explorer/worker/reviewer sub-agent
    /// types. Both keys can coexist and both default to disabled.
    #[serde(default)]
    pub sub_agents: SubAgentsConfig,

    /// Per-agent declarations keyed by agent name. The TOML form is
    /// `[code.agents.<name>]`; deserialisation maps the table key
    /// straight into the BTreeMap key so iteration order is stable
    /// and deterministic across runs.
    #[serde(default)]
    pub agents: BTreeMap<String, AgentConfigEntry>,

    #[serde(default)]
    pub compaction: Option<CompactionConfig>,

    #[serde(default)]
    pub budget: BudgetConfig,
}

/// `[code.multi_agent]`. Defaults match `code.multi_agent.enabled =
/// false` so an existing single-agent install is byte-equivalent
/// pre/post upgrade.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiAgentConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_max_subagent_depth")]
    pub max_subagent_depth: u32,

    #[serde(default = "default_max_concurrent_subagents")]
    pub max_concurrent_subagents: u32,

    #[serde(default = "default_subagent_timeout_ms")]
    pub subagent_timeout_ms: u64,

    /// CEX-S2-12 / S2-INV-03: permit the expensive full-copy fallback
    /// when a sub-agent's isolated workspace cannot be materialized via
    /// the preferred (size-selected) strategy. Defaults to `false` so an
    /// operator opts in to duplicating the whole worktree per run.
    #[serde(default)]
    pub allow_full_copy: bool,

    /// CEX-S2-14: per-slug Source Pool call throttle for sub-agent runs.
    /// `0` (the default) disables the throttle, leaving flag-off
    /// source-call behaviour unbounded exactly as the Step 1 baseline —
    /// so a fresh install observes byte-equivalent behaviour. A positive
    /// value caps the number of concurrent in-flight calls the scheduler
    /// will issue against any single source slug.
    #[serde(default)]
    pub source_concurrency_limit: u32,
}

impl Default for MultiAgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_subagent_depth: default_max_subagent_depth(),
            max_concurrent_subagents: default_max_concurrent_subagents(),
            subagent_timeout_ms: default_subagent_timeout_ms(),
            allow_full_copy: false,
            source_concurrency_limit: 0,
        }
    }
}

fn default_max_subagent_depth() -> u32 {
    1
}
fn default_max_concurrent_subagents() -> u32 {
    1
}
fn default_subagent_timeout_ms() -> u64 {
    600_000
}

/// `[code.sub_agents]` — CEX-S2-12 sub-agent runtime gate.
///
/// Defaults to disabled per S2-INV-08 (Step 2 default off + flag-off
/// rollback): a fresh install must observe byte-equivalent behaviour
/// to Step 1, and any sub-agent attempt with `enabled = false` is a
/// programmer error that fails closed at the dispatcher layer (the
/// dispatcher landed in v0.17.737+ and is only built behind the
/// `enabled` gate).
///
/// `max_parallel = 2` matches the CEX-S2-14 scheduler-side observer
/// budget; CEX-S2-12 enforces a single concurrent sub-agent regardless
/// (single sub-agent behind flag) — the runtime build path applies
/// `cex_s2_12_subagent_concurrency_cap` (see `src/command/code.rs`) so
/// the effective dispatcher concurrency is `1` until CEX-S2-14 unlocks
/// the configured value. The field is parsed here so a later CEX-S2-14
/// patch doesn't need to touch the config schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubAgentsConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_sub_agents_max_parallel")]
    pub max_parallel: u32,

    #[serde(default)]
    pub auto_merge: AutoMergeConfig,
}

impl Default for SubAgentsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_parallel: default_sub_agents_max_parallel(),
            auto_merge: AutoMergeConfig::default(),
        }
    }
}

fn default_sub_agents_max_parallel() -> u32 {
    2
}

/// `[code.sub_agents.auto_merge]` — CEX-S2-15 auto-merge feature flag.
///
/// Defaults to disabled per CEX-S2-15 acceptance criterion (4):
/// auto-merge of human-gated `MergeCandidate` instances may only be
/// enabled after a 30-day operator-collected fixture demonstrates
/// `conflict_rate < 5%` and `rollback_rate < 1%`. This card lands only
/// the schema gate — the CEX-S2-15 ValidatorEngine + risk-score
/// pipeline that consumes the flag does not exist yet.
///
/// Modelled as a structured subsection rather than a bare
/// `auto_merge: bool` on [`SubAgentsConfig`] because CEX-S2-15 will
/// need companion knobs (window length, minimum sample size, …) and
/// adding fields inside an existing table is additive — widening a
/// scalar field to a table later would be a breaking schema change.
/// Landing the table shape now keeps the future CEX-S2-15 patch
/// confined to validator wiring.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoMergeConfig {
    #[serde(default)]
    pub enabled: bool,
}

/// `[code.goal]`. Mirrors the doc's defaults: feature off, ask before
/// auto-continuing on resume, 50 continuation loops, require
/// completion evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_auto_continue_on_resume")]
    pub auto_continue_on_resume: String,

    #[serde(default = "default_max_continuation_loops")]
    pub max_continuation_loops: u32,

    #[serde(default = "default_require_completion_evidence")]
    pub require_completion_evidence: bool,
}

impl Default for GoalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_continue_on_resume: default_auto_continue_on_resume(),
            max_continuation_loops: default_max_continuation_loops(),
            require_completion_evidence: default_require_completion_evidence(),
        }
    }
}

fn default_auto_continue_on_resume() -> String {
    "ask".to_string()
}
fn default_max_continuation_loops() -> u32 {
    50
}
fn default_require_completion_evidence() -> bool {
    true
}

/// Allowed values for `goal.auto_continue_on_resume`. Anything else
/// is a validation error.
const ALLOWED_AUTO_CONTINUE_VALUES: &[&str] = &["ask", "auto", "never"];

/// Policy attached to a per-agent permission category in
/// `[code.agents.<name>].permission`. Mirrors the opencode tri-state
/// (`allow` / `deny` / `ask`) the doc's permission ruleset uses
/// elsewhere; serialised as the bare lowercase string the TOML form
/// expects (`permission = { write = "deny", shell = "deny" }`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionPolicy {
    Allow,
    Deny,
    Ask,
}

/// `[code.agents.<name>]`. Mirrors the runtime
/// [`AgentExecutionSpec`](super::spec::AgentExecutionSpec) shape but
/// keeps the model id as a raw string (validated separately) so
/// validation can collect every parse error in one pass instead of
/// blowing up at deserialisation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigEntry {
    /// Raw `provider/model[@variant]` string. Validation parses it via
    /// [`ModelBinding::parse`] and reports a structured error if the
    /// string is malformed; serde keeps the raw string here so a
    /// future loader that wants to re-render the original config can
    /// do so losslessly.
    pub model: String,

    /// Optional mode override (`primary` / `subagent` / `all`).
    /// Validated against [`AgentMode`] (case-insensitive snake_case).
    #[serde(default = "default_agent_mode_string")]
    pub mode: String,

    /// Tool allow-list. Empty means "inherit runtime default" — the
    /// dispatcher decides per-mode (primary = inherit session list,
    /// subagent = empty / deny). Use [`AgentConfigEntry::tool_selection`]
    /// at the runtime boundary to reify this into a [`ToolSelection`].
    #[serde(default)]
    pub tools: Vec<String>,

    /// Per-agent permission overrides keyed by permission *category*
    /// name (`write`, `shell`, `edit`, `read`, `webfetch`, …) — NOT by
    /// tool name. Doc form is `{ write = "deny", shell = "deny" }`
    /// (opencode.md:1381). The OC-Phase 3 dispatcher translates each
    /// category into the runtime `AgentPermissionSpec` ruleset; the
    /// configuration layer keeps the category map opaque so a future
    /// category addition does not require a schema change here.
    /// Validation rejects policy values that are not in
    /// [`PermissionPolicy`].
    #[serde(default)]
    pub permission: BTreeMap<String, PermissionPolicy>,

    /// Override for the per-agent step cap. `None` means "inherit
    /// runtime default for this agent's mode".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
}

impl AgentConfigEntry {
    /// Reify the validated `tools` list into the runtime
    /// [`ToolSelection`]. An empty list maps to
    /// [`ToolSelection::Inherit`]; otherwise to
    /// [`ToolSelection::Allow`] with the entries trimmed.
    ///
    /// Callers should run validation first; this function does NOT
    /// re-check empty entries.
    pub fn tool_selection(&self) -> ToolSelection {
        if self.tools.is_empty() {
            ToolSelection::Inherit
        } else {
            ToolSelection::Allow(self.tools.iter().map(|t| t.trim().to_string()).collect())
        }
    }

    /// Convert a validated TOML entry into the runtime
    /// [`AgentExecutionSpec`] shape.
    ///
    /// Returns a validation-style error instead of panicking so callers
    /// can surface one user-friendly config error path whether the
    /// failure was found during `validate()` or during conversion.
    pub fn to_execution_spec(
        &self,
        name: &str,
    ) -> Result<AgentExecutionSpec, AgentsConfigValidationError> {
        let model = ModelBinding::parse(&self.model).ok_or_else(|| {
            AgentsConfigValidationError::InvalidModelBinding {
                name: name.to_string(),
                value: self.model.clone(),
            }
        })?;
        let mode = parse_agent_mode(&self.mode).ok_or_else(|| {
            AgentsConfigValidationError::InvalidAgentMode {
                name: name.to_string(),
                value: self.mode.clone(),
            }
        })?;
        Ok(AgentExecutionSpec {
            name: name.to_string(),
            mode,
            model: Some(model),
            tools: self.tool_selection(),
            permission: permission_spec_from_config(&self.permission),
            max_steps: self.steps,
            ..AgentExecutionSpec::default()
        })
    }
}

fn permission_spec_from_config(
    permissions: &BTreeMap<String, PermissionPolicy>,
) -> AgentPermissionSpec {
    let mut allowed_tools = BTreeSet::new();
    let mut denied_tools = BTreeSet::new();
    for (permission, policy) in permissions {
        let key = normalize_permission_key(permission);
        match policy {
            PermissionPolicy::Allow | PermissionPolicy::Ask => {
                allowed_tools.insert(key);
            }
            PermissionPolicy::Deny => {
                denied_tools.insert(key);
            }
        }
    }
    AgentPermissionSpec {
        allowed_tools,
        denied_tools,
        ..AgentPermissionSpec::default()
    }
}

fn normalize_permission_key(permission: &str) -> String {
    match permission.trim() {
        "write" => "edit".to_string(),
        "bash" => "shell".to_string(),
        other => other.to_string(),
    }
}

fn default_agent_mode_string() -> String {
    "primary".to_string()
}

/// `[code.compaction]`. Optional override for the embedded compaction
/// agent's model + tail-turn / preserve-recent-tokens knobs. Absent
/// means "use the embedded defaults from
/// `src/internal/ai/context_budget/compaction.rs`".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionConfig {
    pub model: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_turns: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve_recent_tokens: Option<u64>,
}

impl CompactionConfig {
    /// Parse `self.model` (`provider/model[@variant]`) into a
    /// [`ModelBinding`] suitable for `ProviderFactory::build`.
    /// `validate()` already rejects malformed strings, so a
    /// post-validate caller can safely `.expect()` — but the
    /// fallible signature here lets the sub-agent dispatcher
    /// integration log a diagnostic on the unlikely "validate
    /// returned Ok but we lost the binding" path instead of
    /// panicking inside the dispatch tail.
    pub fn model_binding(&self) -> Option<ModelBinding> {
        ModelBinding::parse(&self.model)
    }
}

impl AgentsConfig {
    /// Resolve the operator's compaction model binding, if
    /// `[code.compaction]` is present in `agents.toml`. Returns
    /// `None` when the section is absent (use the embedded
    /// compaction defaults) or when the model string fails to
    /// parse (validate() should have caught this; a None return
    /// is the safe degrade for callers that load
    /// late-binding profiles).
    ///
    /// Production wire-up: the OC-Phase 4 P4.4 dispatcher
    /// integration calls this from `build_subagent_runtime_for_session`
    /// (or its compaction-aware successor) to decide whether to
    /// build a compaction `CompletionModel` and route the parent
    /// frame through `run_compaction(...)` before feeding the
    /// child via `ContextHandoff::to_handoff_messages` (v0.17.781).
    pub fn compaction_model_binding(&self) -> Option<ModelBinding> {
        self.compaction
            .as_ref()
            .and_then(CompactionConfig::model_binding)
    }
}

/// `[code.budget]` and its nested goal / per-agent subsections. Every
/// threshold is optional so an operator can opt into per-axis caps
/// (e.g. only cost; only wall-clock) independently.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_session_cost_usd: Option<f64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_session_cost_usd: Option<f64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_session_tokens: Option<u64>,

    #[serde(default)]
    pub goal: GoalBudgetConfig,

    /// Per-agent budget overrides keyed by agent name. Validation
    /// requires every key to match a `[code.agents.<name>]` entry.
    #[serde(default)]
    pub per_agent: BTreeMap<String, PerAgentBudgetConfig>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalBudgetConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_wall_clock_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_clock_minutes: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerAgentBudgetConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
}

/// Validation outcome aggregating every problem found in a single
/// `AgentsConfig`. The first-error pattern would surface bugs one at
/// a time across multiple loader runs; collecting every issue lets an
/// operator fix the whole file in one pass.
#[derive(Clone, Debug, PartialEq, Error)]
pub struct AgentsConfigValidationErrors {
    pub errors: Vec<AgentsConfigValidationError>,
}

/// Failure modes from [`AgentsConfig::from_path`] /
/// [`AgentsConfig::load_or_default`]. Distinct from
/// [`AgentsConfigValidationErrors`] (post-parse rule violations) so
/// the surface can render the right hint ("file missing" vs "rules
/// not satisfied"). Path is captured by-value (lossy string) so
/// the error survives the file handle going out of scope.
#[derive(Debug, Error)]
pub enum AgentsConfigLoadError {
    /// `std::fs::read_to_string` failed. Usually surfaces
    /// permission errors or "no such file" when the operator
    /// passed an explicit path.
    #[error("failed to read agents config at '{path}': {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// File read succeeded but TOML parsing rejected the contents.
    /// Includes the path so the diagnostic doesn't lose the
    /// "which file" context after the source is consumed.
    #[error("failed to parse agents config at '{path}': {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
}

impl std::fmt::Display for AgentsConfigValidationErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "agents.toml has {} validation issue(s):",
            self.errors.len()
        )?;
        for err in &self.errors {
            write!(f, "\n  - {err}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Error)]
pub enum AgentsConfigValidationError {
    #[error("agent '{name}': model string '{value}' is not a valid `provider/model[@variant]`")]
    InvalidModelBinding { name: String, value: String },

    #[error("agent '{name}': mode '{value}' is not one of primary/subagent/all")]
    InvalidAgentMode { name: String, value: String },

    #[error("agent '{name}': tool entry at index {index} is empty")]
    EmptyToolEntry { name: String, index: usize },

    #[error("compaction.model '{value}' is not a valid `provider/model[@variant]`")]
    InvalidCompactionModel { value: String },

    #[error("goal.auto_continue_on_resume '{value}' is not one of: ask, auto, never")]
    InvalidAutoContinueOnResume { value: String },

    #[error(
        "multi_agent.{field} must be at least 1 when multi_agent.enabled is true (got {value})"
    )]
    MultiAgentMustBePositive { field: &'static str, value: u32 },

    #[error(
        "multi_agent.subagent_timeout_ms must be greater than 0 when multi_agent.enabled is true"
    )]
    MultiAgentTimeoutMustBePositive,

    #[error(
        "sub_agents.max_parallel must be at least 1 when sub_agents.enabled is true (got {value})"
    )]
    SubAgentsMaxParallelMustBePositive { value: u32 },

    #[error(
        "sub_agents.auto_merge.enabled requires sub_agents.enabled = true (cannot auto-merge when the sub-agent runtime gate is off)"
    )]
    AutoMergeRequiresSubAgentsEnabled,

    #[error(
        "budget.warn_session_cost_usd ({warn}) must be strictly less than max_session_cost_usd ({max})"
    )]
    SessionCostWarnNotBelowMax { warn: f64, max: f64 },

    #[error(
        "budget.goal.warn_cost_usd ({warn}) must be strictly less than goal.max_cost_usd ({max})"
    )]
    GoalCostWarnNotBelowMax { warn: f64, max: f64 },

    #[error(
        "budget.goal.warn_wall_clock_minutes ({warn}) must be strictly less than goal.max_wall_clock_minutes ({max})"
    )]
    GoalWallClockWarnNotBelowMax { warn: u32, max: u32 },

    #[error(
        "budget.per_agent.{name} references an agent that is not declared under [code.agents.{name}]"
    )]
    PerAgentBudgetUndefinedAgent { name: String },
}

impl AgentsConfig {
    /// Parse a TOML document under the top-level `code` table. Returns
    /// `Err(toml::de::Error)` on syntax errors and unknown fields
    /// (the schema is `deny_unknown_fields` so a typo'd key fails
    /// loud at load time rather than silently being ignored).
    ///
    /// The wrapping `[code]` table is required so the TOML matches
    /// the doc's `[code.multi_agent]` / `[code.agents.<name>]` paths
    /// without any extra rewriting at the call site.
    pub fn from_toml_str(toml_str: &str) -> Result<Self, toml::de::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wrapper {
            #[serde(default)]
            code: AgentsConfig,
        }
        let wrapper: Wrapper = toml::from_str(toml_str)?;
        Ok(wrapper.code)
    }

    /// Load an `AgentsConfig` from a TOML file on disk. Convenience
    /// wrapper around [`Self::from_toml_str`] that reads the file
    /// first and surfaces IO errors next to parse errors in the
    /// returned `AgentsConfigLoadError`. Production code (libra
    /// code's session bootstrap) calls this when reading
    /// `.libra/agents.toml`; tests prefer [`Self::from_toml_str`]
    /// directly with inline TOML strings.
    pub fn from_path(path: &std::path::Path) -> Result<Self, AgentsConfigLoadError> {
        let text = std::fs::read_to_string(path).map_err(|source| AgentsConfigLoadError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_toml_str(&text).map_err(|source| AgentsConfigLoadError::Parse {
            path: path.display().to_string(),
            source,
        })
    }

    /// Load an `AgentsConfig` from `path` if the file exists,
    /// otherwise return `Default::default()`. Used by libra code's
    /// session bootstrap to make "no `agents.toml`" a silent
    /// fallback to the conservative defaults (multi_agent disabled,
    /// no agents declared) rather than an error every operator
    /// without an explicit config sees.
    pub fn load_or_default(path: &std::path::Path) -> Result<Self, AgentsConfigLoadError> {
        if path.is_file() {
            Self::from_path(path)
        } else {
            Ok(Self::default())
        }
    }

    /// Render back to the canonical TOML form (with the leading
    /// `[code]` wrapper). Used by tests + future "show effective
    /// config" surfaces; lossy fields (defaulted-away nones) are
    /// dropped via `skip_serializing_if`.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        #[derive(Serialize)]
        struct Wrapper<'a> {
            code: &'a AgentsConfig,
        }
        toml::to_string_pretty(&Wrapper { code: self })
    }

    /// Convert every `[code.agents.<name>]` entry into executable
    /// specs after running the same validation pass used at load time.
    pub fn execution_specs(
        &self,
    ) -> Result<BTreeMap<String, AgentExecutionSpec>, AgentsConfigValidationErrors> {
        self.validate()?;
        let mut specs = BTreeMap::new();
        let mut errors = Vec::new();
        for (name, agent) in &self.agents {
            match agent.to_execution_spec(name) {
                Ok(spec) => {
                    specs.insert(name.clone(), spec);
                }
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Ok(specs)
        } else {
            Err(AgentsConfigValidationErrors { errors })
        }
    }

    /// Build an [`crate::internal::ai::agent::runtime::AgentSpecRegistry`]
    /// from this config's `[code.agents.*]` entries, suitable for
    /// handing to
    /// [`crate::internal::ai::agent::runtime::DefaultSubAgentDispatcher::new`].
    ///
    /// This is the production wire-up the libra-code session
    /// bootstrap calls when `code.sub_agents.enabled = true`: a
    /// fresh `BTreeMap<String, AgentExecutionSpec>` is captured
    /// via [`execution_specs`](Self::execution_specs) and wrapped
    /// in an `Arc<dyn AgentSpecRegistry>` that the dispatcher's
    /// `lookup(name)` / `registered_names()` calls read from.
    ///
    /// `Err` is the same `AgentsConfigValidationErrors` shape the
    /// callers already surface — if validation passes once at
    /// load time, this method can be expected to succeed on a
    /// subsequent call as long as no fields were mutated.
    pub fn build_agent_registry(
        &self,
    ) -> Result<
        std::sync::Arc<dyn crate::internal::ai::agent::runtime::AgentSpecRegistry>,
        AgentsConfigValidationErrors,
    > {
        let specs = self.execution_specs()?;
        Ok(std::sync::Arc::new(StaticAgentSpecRegistry { specs }))
    }

    /// Run the full validation pass. See module-level docs for the
    /// rules. Returns `Ok(())` only when the punch list is empty.
    pub fn validate(&self) -> Result<(), AgentsConfigValidationErrors> {
        let mut errors = Vec::new();

        // multi_agent caps must be >= 1 when the feature is on.
        if self.multi_agent.enabled {
            if self.multi_agent.max_subagent_depth == 0 {
                errors.push(AgentsConfigValidationError::MultiAgentMustBePositive {
                    field: "max_subagent_depth",
                    value: 0,
                });
            }
            if self.multi_agent.max_concurrent_subagents == 0 {
                errors.push(AgentsConfigValidationError::MultiAgentMustBePositive {
                    field: "max_concurrent_subagents",
                    value: 0,
                });
            }
            if self.multi_agent.subagent_timeout_ms == 0 {
                errors.push(AgentsConfigValidationError::MultiAgentTimeoutMustBePositive);
            }
        }

        // sub_agents.max_parallel must be >= 1 when the flag is on.
        // Schema-only landing: the runtime that consumes this flag is
        // CEX-S2-12 (single sub-agent behind flag); the validation
        // here is the public contract surface so a future patch that
        // wires the dispatcher cannot ship with the flag enabled but
        // capped at zero.
        if self.sub_agents.enabled && self.sub_agents.max_parallel == 0 {
            errors
                .push(AgentsConfigValidationError::SubAgentsMaxParallelMustBePositive { value: 0 });
        }

        // auto_merge.enabled cannot be true while the parent sub_agents
        // gate is off — that would be a programmer error declaring a
        // dormant sub-feature enabled on a dormant runtime. Schema-only
        // landing: the CEX-S2-15 ValidatorEngine + risk-score pipeline
        // that actually consumes `auto_merge.enabled` does not exist
        // yet, but the contract surface must reject the misconfigured
        // combination at config load.
        if self.sub_agents.auto_merge.enabled && !self.sub_agents.enabled {
            errors.push(AgentsConfigValidationError::AutoMergeRequiresSubAgentsEnabled);
        }

        // goal.auto_continue_on_resume value enum.
        if !ALLOWED_AUTO_CONTINUE_VALUES.contains(&self.goal.auto_continue_on_resume.as_str()) {
            errors.push(AgentsConfigValidationError::InvalidAutoContinueOnResume {
                value: self.goal.auto_continue_on_resume.clone(),
            });
        }

        // Per-agent rules.
        for (name, agent) in &self.agents {
            if ModelBinding::parse(&agent.model).is_none() {
                errors.push(AgentsConfigValidationError::InvalidModelBinding {
                    name: name.clone(),
                    value: agent.model.clone(),
                });
            }
            if parse_agent_mode(&agent.mode).is_none() {
                errors.push(AgentsConfigValidationError::InvalidAgentMode {
                    name: name.clone(),
                    value: agent.mode.clone(),
                });
            }
            for (idx, tool) in agent.tools.iter().enumerate() {
                if tool.trim().is_empty() {
                    errors.push(AgentsConfigValidationError::EmptyToolEntry {
                        name: name.clone(),
                        index: idx,
                    });
                }
            }
        }

        // Compaction model must be parseable when set.
        if let Some(c) = &self.compaction
            && ModelBinding::parse(&c.model).is_none()
        {
            errors.push(AgentsConfigValidationError::InvalidCompactionModel {
                value: c.model.clone(),
            });
        }

        // Budget warn-vs-max ordering: only enforce when both halves
        // are set (an operator can opt into a hard cap without a
        // warn threshold and vice versa, but mixing them must keep
        // warn < max so warnings actually fire first).
        if let (Some(warn), Some(max)) = (
            self.budget.warn_session_cost_usd,
            self.budget.max_session_cost_usd,
        ) && warn >= max
        {
            errors.push(AgentsConfigValidationError::SessionCostWarnNotBelowMax { warn, max });
        }
        if let (Some(warn), Some(max)) = (
            self.budget.goal.warn_cost_usd,
            self.budget.goal.max_cost_usd,
        ) && warn >= max
        {
            errors.push(AgentsConfigValidationError::GoalCostWarnNotBelowMax { warn, max });
        }
        if let (Some(warn), Some(max)) = (
            self.budget.goal.warn_wall_clock_minutes,
            self.budget.goal.max_wall_clock_minutes,
        ) && warn >= max
        {
            errors.push(AgentsConfigValidationError::GoalWallClockWarnNotBelowMax { warn, max });
        }

        // Per-agent budget keys must reference declared agents.
        for name in self.budget.per_agent.keys() {
            if !self.agents.contains_key(name) {
                errors.push(AgentsConfigValidationError::PerAgentBudgetUndefinedAgent {
                    name: name.clone(),
                });
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(AgentsConfigValidationErrors { errors })
        }
    }
}

/// Snapshot-based [`AgentSpecRegistry`] backed by an immutable
/// `BTreeMap<String, AgentExecutionSpec>` extracted from the
/// loaded [`AgentsConfig`].
///
/// Each `lookup(name)` clones a stored spec; this matches the
/// trait signature (`fn lookup(&self, name: &str) ->
/// Option<AgentExecutionSpec>`) without forcing the dispatcher to
/// hold a long-lived borrow on the config. Use
/// [`AgentsConfig::build_agent_registry`] to construct one.
///
/// [`AgentSpecRegistry`]: crate::internal::ai::agent::runtime::AgentSpecRegistry
#[derive(Clone, Debug)]
struct StaticAgentSpecRegistry {
    specs: BTreeMap<String, AgentExecutionSpec>,
}

impl crate::internal::ai::agent::runtime::AgentSpecRegistry for StaticAgentSpecRegistry {
    fn lookup(&self, name: &str) -> Option<AgentExecutionSpec> {
        self.specs.get(name).cloned()
    }

    fn registered_names(&self) -> Vec<String> {
        self.specs.keys().cloned().collect()
    }
}

/// Parse an `AgentMode` from the TOML string form. Accepts the same
/// snake_case variants serde_json's `rename_all = "snake_case"` produces
/// for [`AgentMode`].
pub fn parse_agent_mode(s: &str) -> Option<AgentMode> {
    match s.trim() {
        "primary" => Some(AgentMode::Primary),
        "subagent" => Some(AgentMode::Subagent),
        "all" => Some(AgentMode::All),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical sample TOML mirroring the doc snippet at
    /// docs/development/commands/_general.md:1363-1407. Used as the
    /// happy-path round-trip fixture.
    const CANONICAL_SAMPLE_TOML: &str = r#"
[code.multi_agent]
enabled = false
max_subagent_depth = 1
max_concurrent_subagents = 1
subagent_timeout_ms = 600000

[code.goal]
enabled = false
auto_continue_on_resume = "ask"
max_continuation_loops = 50
require_completion_evidence = true

[code.agents.planner]
mode = "primary"
model = "anthropic/claude-3-5-sonnet-latest"
tools = ["read_file", "list_dir", "grep_files", "task"]
steps = 30

[code.agents.explorer]
mode = "subagent"
model = "deepseek/deepseek-chat"
tools = ["read_file", "list_dir", "grep_files"]
permission = { write = "deny", shell = "deny" }
steps = 20

[code.compaction]
model = "deepseek/deepseek-chat"
tail_turns = 3
preserve_recent_tokens = 4000

[code.budget]
max_session_cost_usd = 5.0
warn_session_cost_usd = 2.0
max_session_tokens = 1000000

[code.budget.goal]
warn_cost_usd = 2.0
max_cost_usd = 5.0
warn_wall_clock_minutes = 30
max_wall_clock_minutes = 120

[code.budget.per_agent.explorer]
max_cost_usd = 1.0
max_steps = 20
"#;

    #[test]
    fn from_toml_parses_canonical_sample_and_validates() {
        let cfg = AgentsConfig::from_toml_str(CANONICAL_SAMPLE_TOML).expect("parse must succeed");
        cfg.validate().expect("canonical sample must validate");
        assert_eq!(cfg.agents.len(), 2);
        assert!(cfg.agents.contains_key("planner"));
        assert!(cfg.agents.contains_key("explorer"));
        assert_eq!(
            cfg.agents["planner"].model,
            "anthropic/claude-3-5-sonnet-latest"
        );
        assert_eq!(cfg.agents["explorer"].mode, "subagent");
        // Doc fixture line 1385: permission categories carry tri-state policy.
        let explorer_perm = &cfg.agents["explorer"].permission;
        assert_eq!(explorer_perm.get("write"), Some(&PermissionPolicy::Deny));
        assert_eq!(explorer_perm.get("shell"), Some(&PermissionPolicy::Deny));
        assert_eq!(cfg.budget.max_session_cost_usd, Some(5.0));
        assert_eq!(cfg.budget.per_agent["explorer"].max_steps, Some(20));
        assert_eq!(
            cfg.compaction.as_ref().unwrap().model,
            "deepseek/deepseek-chat"
        );
        assert_eq!(cfg.compaction.as_ref().unwrap().tail_turns, Some(3));
    }

    #[test]
    fn empty_toml_yields_default_config_with_documented_defaults() {
        let cfg = AgentsConfig::from_toml_str("").unwrap();
        cfg.validate().expect("empty config must validate");
        assert!(!cfg.multi_agent.enabled);
        assert_eq!(cfg.multi_agent.max_subagent_depth, 1);
        assert_eq!(cfg.multi_agent.max_concurrent_subagents, 1);
        assert_eq!(cfg.multi_agent.subagent_timeout_ms, 600_000);
        // CEX-S2-12 sub_agents flag — default off, max_parallel = 2 per
        // the scheduler-side observer budget in CEX-S2-14.
        assert!(!cfg.sub_agents.enabled);
        assert_eq!(cfg.sub_agents.max_parallel, 2);
        // CEX-S2-15 auto_merge subsection — default off until the 30-day
        // conflict_rate/rollback_rate fixture is collected.
        assert!(!cfg.sub_agents.auto_merge.enabled);
        assert!(!cfg.goal.enabled);
        assert_eq!(cfg.goal.auto_continue_on_resume, "ask");
        assert_eq!(cfg.goal.max_continuation_loops, 50);
        assert!(cfg.goal.require_completion_evidence);
        assert!(cfg.agents.is_empty());
        assert!(cfg.compaction.is_none());
        assert!(cfg.budget.max_session_cost_usd.is_none());
    }

    /// `[code.sub_agents]` parses from TOML with both fields explicit
    /// + the default empty form. Round-trips through `to_toml_string`.
    #[test]
    fn sub_agents_section_parses_and_round_trips() {
        let toml_str = r#"
[code.sub_agents]
enabled = true
max_parallel = 3
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).expect("parses");
        assert!(cfg.sub_agents.enabled);
        assert_eq!(cfg.sub_agents.max_parallel, 3);
        cfg.validate()
            .expect("enabled + max_parallel >= 1 must validate");

        // Round-trip: serialise then re-parse.
        let serialised = cfg.to_toml_string().expect("to_toml");
        let reparsed = AgentsConfig::from_toml_str(&serialised).expect("re-parse");
        assert_eq!(reparsed.sub_agents, cfg.sub_agents);
    }

    /// `sub_agents.enabled = true` with `max_parallel = 0` must fail
    /// validation per the schema contract — a flag-gated runtime
    /// capped at zero would be a programmer error that should be
    /// caught at config load, not after the dispatcher tries to spawn
    /// the first sub-agent.
    #[test]
    fn sub_agents_max_parallel_must_be_positive_when_enabled() {
        let toml_str = r#"
[code.sub_agents]
enabled = true
max_parallel = 0
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).expect("parses");
        let err = cfg
            .validate()
            .expect_err("max_parallel = 0 with enabled = true must fail");
        assert!(
            err.errors.iter().any(|e| matches!(
                e,
                AgentsConfigValidationError::SubAgentsMaxParallelMustBePositive { value: 0 }
            )),
            "expected SubAgentsMaxParallelMustBePositive, got {:?}",
            err.errors,
        );
    }

    /// `sub_agents.enabled = false` with `max_parallel = 0` must NOT
    /// fail validation — the gate is off, so the cap is dormant and
    /// the user shouldn't be forced to fix a stale leftover value
    /// just to keep the flag turned off.
    #[test]
    fn sub_agents_max_parallel_zero_is_allowed_when_disabled() {
        let toml_str = r#"
[code.sub_agents]
enabled = false
max_parallel = 0
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).expect("parses");
        cfg.validate()
            .expect("disabled + max_parallel = 0 must validate (gate is off)");
    }

    /// `deny_unknown_fields` on `SubAgentsConfig` catches typo'd keys
    /// (`enable` vs `enabled`, `max_parallels` vs `max_parallel`) at
    /// load time rather than silently accepting them.
    #[test]
    fn sub_agents_section_rejects_unknown_keys() {
        let toml_str = r#"
[code.sub_agents]
enabled = true
max_parallels = 3
"#;
        let err = AgentsConfig::from_toml_str(toml_str).expect_err("typo'd key must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("max_parallels") || message.contains("unknown field"),
            "expected error to mention the unknown field, got: {message}",
        );
    }

    /// `[code.sub_agents.auto_merge]` parses with `enabled = true` when
    /// the parent `sub_agents` gate is also on, and round-trips through
    /// `to_toml_string`. Mirrors `sub_agents_section_parses_and_round_trips`
    /// to lock the auto_merge subsection as part of the public schema.
    #[test]
    fn sub_agents_auto_merge_parses_and_round_trips() {
        let toml_str = r#"
[code.sub_agents]
enabled = true

[code.sub_agents.auto_merge]
enabled = true
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).expect("parses");
        assert!(cfg.sub_agents.enabled);
        assert!(cfg.sub_agents.auto_merge.enabled);
        cfg.validate()
            .expect("auto_merge enabled with parent enabled must validate");

        // Round-trip: serialise then re-parse — the nested table shape
        // must survive without flattening into a scalar.
        let serialised = cfg.to_toml_string().expect("to_toml");
        let reparsed = AgentsConfig::from_toml_str(&serialised).expect("re-parse");
        assert_eq!(reparsed.sub_agents, cfg.sub_agents);
    }

    /// `auto_merge.enabled = true` while `sub_agents.enabled = false`
    /// is a programmer error: enabling a sub-feature gate on a dormant
    /// runtime never executes anything, so it must fail at config load
    /// rather than silently no-op.
    #[test]
    fn sub_agents_auto_merge_requires_parent_enabled() {
        let toml_str = r#"
[code.sub_agents]
enabled = false

[code.sub_agents.auto_merge]
enabled = true
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).expect("parses");
        let err = cfg
            .validate()
            .expect_err("auto_merge enabled with parent disabled must fail");
        assert!(
            err.errors.iter().any(|e| matches!(
                e,
                AgentsConfigValidationError::AutoMergeRequiresSubAgentsEnabled
            )),
            "expected AutoMergeRequiresSubAgentsEnabled, got {:?}",
            err.errors,
        );
    }

    /// `auto_merge.enabled = false` (default) with `sub_agents.enabled
    /// = false` is the dormant baseline — both gates off must validate
    /// cleanly so a fresh install never has to touch the sub_agents
    /// table just to keep everything off.
    #[test]
    fn sub_agents_auto_merge_default_is_allowed_when_parent_disabled() {
        let toml_str = r#"
[code.sub_agents]
enabled = false

[code.sub_agents.auto_merge]
enabled = false
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).expect("parses");
        cfg.validate()
            .expect("disabled + auto_merge disabled must validate (both gates off)");
    }

    /// `deny_unknown_fields` on `AutoMergeConfig` catches typo'd keys
    /// (`enable` vs `enabled`) at load time rather than silently
    /// accepting them — important because the auto-merge gate must
    /// fail loud rather than appearing on by accident.
    #[test]
    fn sub_agents_auto_merge_rejects_unknown_keys() {
        let toml_str = r#"
[code.sub_agents.auto_merge]
enable = true
"#;
        let err = AgentsConfig::from_toml_str(toml_str).expect_err("typo'd key must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("enable") || message.contains("unknown field"),
            "expected error to mention the unknown field, got: {message}",
        );
    }

    #[test]
    fn unknown_top_level_key_under_code_fails_load() {
        // `deny_unknown_fields` on AgentsConfig must reject typos.
        let toml_str = r#"
[code]
typo_section = true
"#;
        let err = AgentsConfig::from_toml_str(toml_str).expect_err("unknown key must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("typo_section"),
            "error must name the offending key, got: {msg}"
        );
    }

    #[test]
    fn invalid_model_binding_collected_per_agent() {
        let toml_str = r#"
[code.agents.bad1]
model = "no-slash"
mode = "primary"

[code.agents.bad2]
model = "anthropic/claude@"
mode = "primary"

[code.agents.good]
model = "openai/gpt-4o"
mode = "primary"
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        let err = cfg.validate().expect_err("bad models must fail");
        let bad_names: Vec<&str> = err
            .errors
            .iter()
            .filter_map(|e| match e {
                AgentsConfigValidationError::InvalidModelBinding { name, .. } => {
                    Some(name.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(bad_names, vec!["bad1", "bad2"]);
    }

    #[test]
    fn invalid_agent_mode_string_collected() {
        let toml_str = r#"
[code.agents.weird]
model = "openai/gpt-4o"
mode = "moderator"
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        let err = cfg.validate().expect_err("bad mode must fail");
        assert!(err.errors.iter().any(|e| matches!(
            e,
            AgentsConfigValidationError::InvalidAgentMode { name, value }
                if name == "weird" && value == "moderator"
        )));
    }

    #[test]
    fn empty_tool_entry_caught_at_specific_index() {
        let toml_str = r#"
[code.agents.x]
model = "openai/gpt-4o"
mode = "primary"
tools = ["read_file", "  ", "grep_files"]
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        let err = cfg.validate().expect_err("empty tool entry must fail");
        assert!(err.errors.iter().any(|e| matches!(
            e,
            AgentsConfigValidationError::EmptyToolEntry { name, index }
                if name == "x" && *index == 1
        )));
    }

    #[test]
    fn warn_must_be_strictly_below_max_session_cost() {
        let toml_str = r#"
[code.budget]
warn_session_cost_usd = 5.0
max_session_cost_usd = 5.0
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        let err = cfg.validate().expect_err("warn==max must fail");
        assert!(err.errors.iter().any(|e| matches!(
            e,
            AgentsConfigValidationError::SessionCostWarnNotBelowMax { warn, max }
                if (*warn - 5.0).abs() < f64::EPSILON && (*max - 5.0).abs() < f64::EPSILON
        )));
    }

    #[test]
    fn goal_warn_must_be_strictly_below_max() {
        let toml_str = r#"
[code.budget.goal]
warn_cost_usd = 6.0
max_cost_usd = 5.0
warn_wall_clock_minutes = 60
max_wall_clock_minutes = 30
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        let err = cfg.validate().expect_err("warn>max must fail");
        let kinds: Vec<&str> = err
            .errors
            .iter()
            .map(|e| match e {
                AgentsConfigValidationError::GoalCostWarnNotBelowMax { .. } => "cost",
                AgentsConfigValidationError::GoalWallClockWarnNotBelowMax { .. } => "wall",
                _ => "other",
            })
            .collect();
        assert!(kinds.contains(&"cost"));
        assert!(kinds.contains(&"wall"));
    }

    #[test]
    fn warn_with_no_max_does_not_trigger_validation_error() {
        // Operator opts into a warn-only threshold; should pass.
        let toml_str = r#"
[code.budget]
warn_session_cost_usd = 2.0
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        cfg.validate().expect("warn-only must validate");
    }

    #[test]
    fn auto_continue_on_resume_enum_validated() {
        let toml_str = r#"
[code.goal]
auto_continue_on_resume = "yolo"
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        let err = cfg.validate().expect_err("bad enum must fail");
        assert!(err.errors.iter().any(|e| matches!(
            e,
            AgentsConfigValidationError::InvalidAutoContinueOnResume { value } if value == "yolo"
        )));
    }

    #[test]
    fn multi_agent_caps_must_be_at_least_one_when_enabled() {
        let toml_str = r#"
[code.multi_agent]
enabled = true
max_subagent_depth = 0
max_concurrent_subagents = 0
subagent_timeout_ms = 0
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        let err = cfg.validate().expect_err("zero caps must fail");
        let fields: Vec<&'static str> = err
            .errors
            .iter()
            .filter_map(|e| match e {
                AgentsConfigValidationError::MultiAgentMustBePositive { field, .. } => Some(*field),
                _ => None,
            })
            .collect();
        assert_eq!(
            fields,
            vec!["max_subagent_depth", "max_concurrent_subagents"]
        );
        assert!(err.errors.iter().any(|e| matches!(
            e,
            AgentsConfigValidationError::MultiAgentTimeoutMustBePositive
        )));
    }

    #[test]
    fn multi_agent_zero_caps_when_disabled_does_not_trigger_validation() {
        // The feature flag is off; the operator can leave the caps at
        // 0 to make the off state explicit.
        let toml_str = r#"
[code.multi_agent]
enabled = false
max_subagent_depth = 0
max_concurrent_subagents = 0
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        cfg.validate()
            .expect("disabled feature with 0 caps must validate");
    }

    #[test]
    fn per_agent_budget_must_reference_declared_agent() {
        let toml_str = r#"
[code.agents.planner]
model = "anthropic/claude-3-5-sonnet-latest"
mode = "primary"

[code.budget.per_agent.explorer]
max_cost_usd = 1.0
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        let err = cfg.validate().expect_err("undefined agent must fail");
        assert!(err.errors.iter().any(|e| matches!(
            e,
            AgentsConfigValidationError::PerAgentBudgetUndefinedAgent { name } if name == "explorer"
        )));
    }

    #[test]
    fn validation_aggregates_all_issues_in_one_pass() {
        let toml_str = r#"
[code.multi_agent]
enabled = true
max_subagent_depth = 0

[code.goal]
auto_continue_on_resume = "yolo"

[code.agents.bad]
model = "no-slash"
mode = "moderator"

[code.budget]
warn_session_cost_usd = 5.0
max_session_cost_usd = 5.0
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        let err = cfg.validate().expect_err("multiple issues must surface");
        assert!(
            err.errors.len() >= 5,
            "expected 5+ issues collected in one pass, got {}: {err:?}",
            err.errors.len()
        );
    }

    #[test]
    fn tool_selection_inherit_when_empty_allow_when_listed() {
        let inherit = AgentConfigEntry {
            model: "openai/gpt-4o".to_string(),
            mode: "primary".to_string(),
            tools: Vec::new(),
            permission: BTreeMap::new(),
            steps: None,
        };
        assert!(matches!(inherit.tool_selection(), ToolSelection::Inherit));

        let allow = AgentConfigEntry {
            model: "openai/gpt-4o".to_string(),
            mode: "primary".to_string(),
            tools: vec!["read_file".to_string(), "  list_dir  ".to_string()],
            permission: BTreeMap::new(),
            steps: None,
        };
        match allow.tool_selection() {
            ToolSelection::Allow(tools) => {
                assert_eq!(tools, vec!["read_file".to_string(), "list_dir".to_string()]);
            }
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[test]
    fn parse_agent_mode_accepts_canonical_strings_only() {
        assert_eq!(parse_agent_mode("primary"), Some(AgentMode::Primary));
        assert_eq!(parse_agent_mode("subagent"), Some(AgentMode::Subagent));
        assert_eq!(parse_agent_mode("all"), Some(AgentMode::All));
        // Surrounding whitespace tolerated; mixed case is not.
        assert_eq!(parse_agent_mode("  primary  "), Some(AgentMode::Primary));
        assert!(parse_agent_mode("Primary").is_none());
        assert!(parse_agent_mode("moderator").is_none());
    }

    /// Round-trip: the canonical sample serialises back to a string
    /// that re-parses to the same struct. Pins `to_toml_string` →
    /// `from_toml_str` lossless behaviour for fields we surface.
    #[test]
    fn round_trip_canonical_sample_through_toml() {
        let cfg = AgentsConfig::from_toml_str(CANONICAL_SAMPLE_TOML).unwrap();
        let serialised = cfg.to_toml_string().expect("serialise must succeed");
        let cfg2 = AgentsConfig::from_toml_str(&serialised).expect("re-parse must succeed");
        assert_eq!(cfg, cfg2);
    }

    /// Permission policies that fall outside the `allow|deny|ask`
    /// tri-state must fail at deserialisation (serde enum check) so a
    /// typo'd policy is loud at load time, not a silent fallback.
    #[test]
    fn invalid_permission_policy_string_fails_load() {
        let toml_str = r#"
[code.agents.x]
model = "openai/gpt-4o"
mode = "primary"
permission = { write = "yolo" }
"#;
        let err = AgentsConfig::from_toml_str(toml_str).expect_err("bad policy must fail");
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("yolo") || msg.contains("variant"),
            "error must surface the offending policy or its rejected variant, got: {msg}"
        );
    }

    /// Empty permission map (the common case for a default-allow
    /// agent) parses cleanly and validates.
    #[test]
    fn permission_map_defaults_to_empty_when_omitted() {
        let toml_str = r#"
[code.agents.x]
model = "openai/gpt-4o"
mode = "primary"
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        cfg.validate().expect("default permission must validate");
        assert!(cfg.agents["x"].permission.is_empty());
    }

    /// Mixed permission categories — `allow` / `deny` / `ask` — all
    /// round-trip through serde so the runtime translator (OC-Phase 3)
    /// receives the operator's exact intent.
    #[test]
    fn permission_map_carries_each_tri_state_value() {
        let toml_str = r#"
[code.agents.x]
model = "openai/gpt-4o"
mode = "primary"
permission = { write = "deny", read = "allow", shell = "ask" }
"#;
        let cfg = AgentsConfig::from_toml_str(toml_str).unwrap();
        cfg.validate().expect("tri-state permission must validate");
        let perm = &cfg.agents["x"].permission;
        assert_eq!(perm.get("write"), Some(&PermissionPolicy::Deny));
        assert_eq!(perm.get("read"), Some(&PermissionPolicy::Allow));
        assert_eq!(perm.get("shell"), Some(&PermissionPolicy::Ask));
    }

    #[test]
    fn execution_specs_convert_toml_agents_for_runtime_dispatch() {
        let cfg = AgentsConfig::from_toml_str(
            r#"
[code.agents.explorer]
model = "deepseek/deepseek-chat"
mode = "subagent"
tools = ["read_file", "grep_files"]
permission = { write = "deny", bash = "ask" }
steps = 12
"#,
        )
        .unwrap();

        let specs = cfg.execution_specs().expect("valid config converts");
        let spec = specs.get("explorer").expect("explorer spec");
        assert_eq!(spec.name, "explorer");
        assert_eq!(spec.mode, AgentMode::Subagent);
        assert_eq!(spec.model.as_ref().unwrap().provider_id, "deepseek");
        assert_eq!(spec.max_steps, Some(12));
        assert!(matches!(spec.tools, ToolSelection::Allow(_)));
        assert!(spec.permission.denied_tools.contains("edit"));
        assert!(spec.permission.allowed_tools.contains("shell"));
    }

    /// OC-Phase 4 P4.4 prerequisite (v0.17.782):
    /// `AgentsConfig::compaction_model_binding()` returns the
    /// parsed binding when `[code.compaction]` is present, or
    /// `None` for "use embedded defaults". Pins the round trip
    /// through `ModelBinding::parse` so a future
    /// dispatcher-side integration can resolve the binding
    /// once at session bootstrap without reparsing.
    #[test]
    fn compaction_model_binding_resolves_provider_model_from_toml() {
        let cfg = AgentsConfig::from_toml_str(
            r#"
[code.compaction]
model = "deepseek/deepseek-chat"
tail_turns = 4
"#,
        )
        .expect("compaction section must parse");
        let binding = cfg
            .compaction_model_binding()
            .expect("present [code.compaction] must resolve a binding");
        assert_eq!(binding.provider_id, "deepseek");
        assert_eq!(binding.model_id, "deepseek-chat");

        let empty = AgentsConfig::from_toml_str("").expect("empty TOML must parse");
        assert!(
            empty.compaction_model_binding().is_none(),
            "absent [code.compaction] must resolve None (use embedded defaults)",
        );
    }

    /// OC-Phase 3 P3.4 production wire-up prerequisite (v0.17.772):
    /// `AgentsConfig::build_agent_registry()` materialises every
    /// validated `[code.agents.*]` entry into the
    /// `AgentSpecRegistry` shape the dispatcher consumes via
    /// `DefaultSubAgentDispatcher::new(registry, config)`.
    /// `lookup(name)` returns clones; `registered_names()` returns
    /// the BTreeMap key order (sorted).
    /// `load_or_default` returns the default `AgentsConfig` when
    /// no file exists at `path` (the common case for operators
    /// without an explicit `.libra/agents.toml`). A regression that
    /// surfaced the missing-file IO error would force every libra
    /// code session bootstrap to handle the absent-config case
    /// itself.
    #[test]
    fn load_or_default_returns_default_when_path_does_not_exist() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nonexistent = temp.path().join("agents.toml");
        let cfg =
            AgentsConfig::load_or_default(&nonexistent).expect("absent file must not be an error");
        let defaults = AgentsConfig::default();
        assert_eq!(cfg.multi_agent.enabled, defaults.multi_agent.enabled);
        assert_eq!(cfg.sub_agents.enabled, defaults.sub_agents.enabled);
        assert!(cfg.agents.is_empty());
    }

    /// `load_or_default` reads the file when it exists and parses
    /// it with the same wrapper-required `[code]` table shape that
    /// `from_toml_str` accepts. A round trip via disk pins the
    /// TOML-on-disk path against `from_toml_str` for the inline
    /// path.
    #[test]
    fn load_or_default_reads_existing_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("agents.toml");
        std::fs::write(
            &path,
            r#"
[code.agents.explorer]
model = "deepseek/deepseek-chat"
mode = "subagent"
tools = ["read_file"]
"#,
        )
        .expect("write fixture");
        let cfg = AgentsConfig::load_or_default(&path).expect("file must parse");
        assert!(
            cfg.agents.contains_key("explorer"),
            "explorer agent must round-trip through the on-disk loader",
        );
    }

    /// `from_path` on a malformed file surfaces the parse error
    /// path (not the Read variant) and includes the path so the
    /// diagnostic does not lose the file context after the source
    /// is consumed.
    #[test]
    fn from_path_surfaces_parse_errors_with_path_context() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("malformed.toml");
        std::fs::write(&path, "this is not valid toml = [\n").expect("write fixture");
        let err =
            AgentsConfig::from_path(&path).expect_err("malformed TOML must surface as an error");
        match err {
            AgentsConfigLoadError::Parse {
                path: reported_path,
                ..
            } => {
                assert!(
                    reported_path.contains("malformed.toml"),
                    "Parse error must carry the path; got: {reported_path}",
                );
            }
            other => panic!("expected Parse error, got: {other:?}"),
        }
    }

    #[test]
    fn build_agent_registry_exposes_validated_specs_via_registry_trait() {
        let cfg = AgentsConfig::from_toml_str(
            r#"
[code.agents.explorer]
model = "deepseek/deepseek-chat"
mode = "subagent"
tools = ["read_file"]

[code.agents.reviewer]
model = "anthropic/claude-3-5-sonnet-latest"
mode = "subagent"
tools = ["read_file", "grep_files"]
"#,
        )
        .expect("two-agent config must parse");

        let registry = cfg
            .build_agent_registry()
            .expect("validated config must build a registry");

        let names = registry.registered_names();
        assert_eq!(
            names,
            vec!["explorer".to_string(), "reviewer".to_string()],
            "registered_names should return BTreeMap key order (sorted)",
        );

        let explorer = registry.lookup("explorer").expect("explorer must resolve");
        assert_eq!(explorer.name, "explorer");
        assert_eq!(
            explorer.model.as_ref().unwrap().provider_id,
            "deepseek",
            "explorer's model binding survives the conversion",
        );

        let reviewer = registry.lookup("reviewer").expect("reviewer must resolve");
        assert_eq!(reviewer.model.as_ref().unwrap().provider_id, "anthropic");

        assert!(
            registry.lookup("ghost").is_none(),
            "unknown names must return None, not a placeholder spec",
        );
    }

    #[test]
    fn agents_config_validation_error_display_pins_each_variant() {
        assert_eq!(
            AgentsConfigValidationError::InvalidModelBinding {
                name: "research".to_string(),
                value: "openai".to_string(),
            }
            .to_string(),
            "agent 'research': model string 'openai' is not a valid `provider/model[@variant]`",
        );
        assert_eq!(
            AgentsConfigValidationError::InvalidAgentMode {
                name: "research".to_string(),
                value: "expert".to_string(),
            }
            .to_string(),
            "agent 'research': mode 'expert' is not one of primary/subagent/all",
        );
        assert_eq!(
            AgentsConfigValidationError::EmptyToolEntry {
                name: "research".to_string(),
                index: 3,
            }
            .to_string(),
            "agent 'research': tool entry at index 3 is empty",
        );
        assert_eq!(
            AgentsConfigValidationError::InvalidCompactionModel {
                value: "raw".to_string(),
            }
            .to_string(),
            "compaction.model 'raw' is not a valid `provider/model[@variant]`",
        );
        assert_eq!(
            AgentsConfigValidationError::InvalidAutoContinueOnResume {
                value: "maybe".to_string(),
            }
            .to_string(),
            "goal.auto_continue_on_resume 'maybe' is not one of: ask, auto, never",
        );
        assert_eq!(
            AgentsConfigValidationError::MultiAgentMustBePositive {
                field: "max_subagent_depth",
                value: 0,
            }
            .to_string(),
            "multi_agent.max_subagent_depth must be at least 1 when multi_agent.enabled is true \
             (got 0)",
        );
        assert_eq!(
            AgentsConfigValidationError::MultiAgentTimeoutMustBePositive.to_string(),
            "multi_agent.subagent_timeout_ms must be greater than 0 when multi_agent.enabled is \
             true",
        );
        assert_eq!(
            AgentsConfigValidationError::SubAgentsMaxParallelMustBePositive { value: 0 }
                .to_string(),
            "sub_agents.max_parallel must be at least 1 when sub_agents.enabled is true (got 0)",
        );
        assert_eq!(
            AgentsConfigValidationError::SessionCostWarnNotBelowMax {
                warn: 9.0,
                max: 5.0,
            }
            .to_string(),
            "budget.warn_session_cost_usd (9) must be strictly less than max_session_cost_usd (5)",
        );
        assert_eq!(
            AgentsConfigValidationError::GoalCostWarnNotBelowMax {
                warn: 12.0,
                max: 8.0,
            }
            .to_string(),
            "budget.goal.warn_cost_usd (12) must be strictly less than goal.max_cost_usd (8)",
        );
        assert_eq!(
            AgentsConfigValidationError::GoalWallClockWarnNotBelowMax { warn: 30, max: 20 }
                .to_string(),
            "budget.goal.warn_wall_clock_minutes (30) must be strictly less than \
             goal.max_wall_clock_minutes (20)",
        );
        assert_eq!(
            AgentsConfigValidationError::PerAgentBudgetUndefinedAgent {
                name: "unknown".to_string(),
            }
            .to_string(),
            "budget.per_agent.unknown references an agent that is not declared under \
             [code.agents.unknown]",
        );
        assert_eq!(
            AgentsConfigValidationError::AutoMergeRequiresSubAgentsEnabled.to_string(),
            "sub_agents.auto_merge.enabled requires sub_agents.enabled = true (cannot auto-merge \
             when the sub-agent runtime gate is off)",
        );
    }
}
