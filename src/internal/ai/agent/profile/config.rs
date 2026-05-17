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
//! - `auto_continue_on_resume` is one of `"ask"`, `"auto"`, `"never"`.
//! - Per-agent `[code.budget.per_agent.<name>]` references an agent
//!   declared under `[code.agents.<name>]`.
//!
//! Rules that depend on runtime state (e.g. whether the catalogued
//! provider exists) are NOT enforced here; the factory layer
//! (OC-Phase 1) owns those.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::spec::{AgentMode, ModelBinding, ToolSelection};

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
}

impl Default for MultiAgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_subagent_depth: default_max_subagent_depth(),
            max_concurrent_subagents: default_max_concurrent_subagents(),
        }
    }
}

fn default_max_subagent_depth() -> u32 {
    1
}
fn default_max_concurrent_subagents() -> u32 {
    1
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
    /// docs/improvement/opencode.md:1363-1407. Used as the
    /// happy-path round-trip fixture.
    const CANONICAL_SAMPLE_TOML: &str = r#"
[code.multi_agent]
enabled = false
max_subagent_depth = 1
max_concurrent_subagents = 1

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
        assert!(!cfg.goal.enabled);
        assert_eq!(cfg.goal.auto_continue_on_resume, "ask");
        assert_eq!(cfg.goal.max_continuation_loops, 50);
        assert!(cfg.goal.require_completion_evidence);
        assert!(cfg.agents.is_empty());
        assert!(cfg.compaction.is_none());
        assert!(cfg.budget.max_session_cost_usd.is_none());
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
    }

    /// Pins the manual `Display` impl on the [`AgentsConfigValidationErrors`]
    /// aggregator. The wrapper renders a header line (`agents.toml has N
    /// validation issue(s):`) followed by one `\n  - {err}` line per child.
    /// `agents_config_validation_error_display_pins_each_variant` already
    /// pins the per-variant rendering above; this test pins the aggregator's
    /// header / separator / empty-list behaviour so a future refactor of
    /// the wrapper cannot silently change the multi-issue rendering shown
    /// to operators when `AgentsConfig::validate` fails.
    #[test]
    fn agents_config_validation_errors_display_pins_aggregator_format() {
        assert_eq!(
            AgentsConfigValidationErrors { errors: vec![] }.to_string(),
            "agents.toml has 0 validation issue(s):",
        );

        let single = AgentsConfigValidationErrors {
            errors: vec![AgentsConfigValidationError::InvalidModelBinding {
                name: "research".to_string(),
                value: "openai".to_string(),
            }],
        };
        assert_eq!(
            single.to_string(),
            "agents.toml has 1 validation issue(s):\n  - \
             agent 'research': model string 'openai' is not a valid \
             `provider/model[@variant]`",
        );

        let multi = AgentsConfigValidationErrors {
            errors: vec![
                AgentsConfigValidationError::InvalidAgentMode {
                    name: "research".to_string(),
                    value: "expert".to_string(),
                },
                AgentsConfigValidationError::EmptyToolEntry {
                    name: "research".to_string(),
                    index: 3,
                },
            ],
        };
        assert_eq!(
            multi.to_string(),
            "agents.toml has 2 validation issue(s):\n  - \
             agent 'research': mode 'expert' is not one of primary/subagent/all\n  - \
             agent 'research': tool entry at index 3 is empty",
        );
    }
}
