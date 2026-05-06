//! `AgentExecutionSpec` and friends — the executable agent contract.
//!
//! This module is the OC-Phase 0 deliverable from `docs/improvement/opencode.md`.
//! It defines the **schema** an agent profile gets lifted to once we move past the
//! "system prompt + tool name list" representation in [`super::parser::AgentProfile`].
//!
//! What this module is:
//! - Pure data types with `serde` round-trip support.
//! - Forward-stable shapes whose field names **and container types** match the
//!   feature-gated
//!   [`crate::internal::ai::agent_run::permission::AgentPermissionProfile`]
//!   verbatim, so the OC-Phase 3 runtime conversion is a 1:1 field copy with
//!   no dedup, ordering, or type coercion.
//!
//! What this module is **not**:
//! - It does **not** parse frontmatter (OC-Phase 2 wires the parser).
//! - It does **not** build any [`crate::internal::ai::completion::CompletionModel`]
//!   instance (OC-Phase 1 introduces `ProviderFactory`).
//! - It does **not** dispatch sub-agents (OC-Phase 3 introduces
//!   `SubAgentDispatcher`).
//!
//! Why types live here before the runtime catches up:
//! Several phases (parser, router, dispatcher, registry pre-filter) all need to
//! agree on the same vocabulary. Freezing the schema first lets each phase be
//! reviewed and merged independently without re-litigating field shapes.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A `(provider_id, model_id, variant)` triple identifying which LLM to invoke.
///
/// The string form (parsed by [`ModelBinding::parse`] and rendered by
/// [`ModelBinding::to_canonical_string`]) is `provider/model[@variant]`:
/// - The first `/` splits provider id from model id.
/// - The model id may itself contain `/` (e.g.
///   `bedrock/anthropic.claude-3-5-sonnet/v1`), mirroring opencode's
///   `Provider.parseModel()` semantics.
/// - An optional `@variant` suffix on the **last** segment carries reasoning /
///   thinking variants (e.g. `anthropic/claude-opus-4@thinking`). Variants are
///   intentionally surfaced as a separate field so [provider transform layers]
///   can apply them as request options rather than re-parsing the model id.
///
/// Compatibility note: the legacy `model_preference` strings (`"default"`,
/// `"fast"`, `"powerful"`) deliberately do **not** parse into a `ModelBinding`.
/// Callers must check for an explicit provider prefix before promoting a string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelBinding {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

impl ModelBinding {
    /// Parse `provider/model[@variant]`. Returns `None` for unprefixed legacy
    /// values like `default` / `fast` so the caller can keep them as
    /// `model_preference`.
    ///
    /// Boundary conditions:
    /// - Requires at least one `/` after a non-empty provider id.
    /// - Trailing `/` (no model id) is rejected.
    /// - `@` may appear once at the end of the model id to introduce a variant
    ///   (e.g. `anthropic/claude-opus-4@thinking`). An `@` with an empty
    ///   variant suffix (`foo/bar@`) is rejected so silently-empty variants do
    ///   not slip through.
    /// - Whitespace around the input is trimmed; whitespace inside is preserved
    ///   verbatim so an unusual provider/model id is not silently mangled.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (provider_id, rest) = s.split_once('/')?;
        // Trim each side of the `/` so input like `openai / gpt-4` (with
        // whitespace around the separator) does not produce ids that carry
        // a trailing/leading space. Inner whitespace inside an id is left
        // untouched — model identifiers normally have no whitespace, but if
        // a vendor ever uses one it survives verbatim.
        let provider_id = provider_id.trim();
        let rest = rest.trim();
        if provider_id.is_empty() || rest.is_empty() {
            return None;
        }

        // Variant qualifier: only an `@` in the FINAL slash-separated segment
        // of the model id introduces a variant. `@` characters earlier in the
        // path (e.g. `azure/foo@bar/baz`) stay attached to the model id, so a
        // model id that legitimately contains `@` does not get truncated.
        // Within that final segment we use `rsplit_once` so a multi-`@`
        // identifier like `openai/gpt-4@v1@thinking` keeps `gpt-4@v1` as the
        // model id and `thinking` as the variant.
        let (path_prefix, last_segment) = match rest.rfind('/') {
            Some(idx) => (Some(&rest[..idx]), &rest[idx + 1..]),
            None => (None, rest),
        };

        let (model_id, variant) = match last_segment.rsplit_once('@') {
            Some((model_part, variant)) => {
                let model_part = model_part.trim();
                let variant = variant.trim();
                if model_part.is_empty() || variant.is_empty() {
                    // Trailing `@` with empty variant (`foo/bar@`) or leading
                    // `@` with empty model id in the final segment
                    // (`foo/@variant`): both are half-formed and must not
                    // silently produce a binding with `model_id = ""` or
                    // `model_id = "@variant"`.
                    return None;
                }
                let model_id = match path_prefix {
                    Some(prefix) => format!("{prefix}/{model_part}"),
                    None => model_part.to_string(),
                };
                (model_id, Some(variant.to_string()))
            }
            None => (rest.to_string(), None),
        };

        Some(Self {
            provider_id: provider_id.to_string(),
            model_id,
            variant,
        })
    }

    /// Render back to the canonical `provider/model[@variant]` string.
    ///
    /// `parse(b.to_canonical_string()) == Some(b)` for every `ModelBinding`
    /// whose fields satisfy [`ModelBinding::parse`]'s contract — so a binding
    /// that came in via parsing round-trips losslessly through the string.
    pub fn to_canonical_string(&self) -> String {
        match &self.variant {
            Some(v) => format!("{}/{}@{}", self.provider_id, self.model_id, v),
            None => format!("{}/{}", self.provider_id, self.model_id),
        }
    }
}

/// Whether an agent can be selected as the primary turn driver, dispatched as a
/// sub-agent via the `task` tool, or both.
///
/// The default for a profile that omits this field is [`AgentMode::Primary`] so
/// existing `.libra/agents/*.md` files do not silently appear in sub-agent lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    #[default]
    Primary,
    Subagent,
    All,
}

impl AgentMode {
    /// Whether this mode permits selection as a session's primary agent.
    pub fn is_primary_eligible(self) -> bool {
        matches!(self, Self::Primary | Self::All)
    }

    /// Whether this mode permits dispatch as a sub-agent through the `task` tool.
    pub fn is_subagent_eligible(self) -> bool {
        matches!(self, Self::Subagent | Self::All)
    }
}

/// How an agent's tool list is computed.
///
/// `Inherit` has different defaults depending on caller context: for a primary
/// agent it means "use whatever the session allow-list provides"; for a sub-agent
/// dispatched via `task` it means "empty allow-list" (default deny per
/// S2-INV-05). The runtime resolves this contextually in OC-Phase 3; the schema
/// only records intent.
///
/// Wire format is adjacently tagged so tuple variants serialize as
/// `{"kind":"allow","tools":[...]}` and the unit variant as
/// `{"kind":"inherit"}` — the same shape OC-Phase 5 TOML / JSON config expects.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "tools", rename_all = "snake_case")]
pub enum ToolSelection {
    /// Defer to the runtime default for this agent's mode.
    #[default]
    Inherit,
    /// Allow only this explicit list. Order is irrelevant.
    Allow(Vec<String>),
    /// Allow everything the runtime would normally expose, minus this list.
    /// Deny always wins over allow when an entry is in both lists.
    Deny(Vec<String>),
}

/// Where approval prompts route when the agent invokes a tool that requires
/// human consent. Mirrors
/// [`crate::internal::ai::agent_run::permission::ApprovalRouting`] so the
/// feature-gated runtime conversion stays purely structural; do not reorder
/// variants without keeping the gated module in lock-step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRoutingSpec {
    /// All approvals route to Layer 1 / human reviewer.
    #[default]
    Layer1Human,
    /// Pre-approved for the duration of this run. Used by read-only sub-agents
    /// like `explore` where an interactive prompt would be pure friction.
    SessionPreApproved,
}

/// Permission shape attached to an `AgentExecutionSpec`.
///
/// Field names, container types (`BTreeSet<String>`) and defaults mirror the
/// feature-gated
/// [`crate::internal::ai::agent_run::permission::AgentPermissionProfile`] so
/// the OC-Phase 3 runtime can convert one into the other without renaming or
/// re-deduplicating. This struct is available in the **default** build (no
/// `subagent-scaffold` feature required) so config and parsing code can use it
/// before the dispatcher lands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPermissionSpec {
    /// Tool names this agent may invoke. Empty = deny everything.
    #[serde(default)]
    pub allowed_tools: BTreeSet<String>,

    /// Hard denies that override `allowed_tools` even on partial overlap.
    #[serde(default)]
    pub denied_tools: BTreeSet<String>,

    /// MCP / Source Pool slugs the agent may read from.
    #[serde(default)]
    pub allowed_source_slugs: BTreeSet<String>,

    /// Where approval prompts route. Defaults to Layer1Human per S2-INV-06.
    #[serde(default)]
    pub approval_routing: ApprovalRoutingSpec,

    /// Whether this agent may spawn further sub-agents through `task`.
    /// Per S2-INV-09 this is `false` by default; only Layer 1 is a legitimate
    /// spawner unless an operator explicitly opts in via config.
    #[serde(default)]
    pub may_spawn_sub_agents: bool,
}

/// The executable form of an agent profile.
///
/// Compared with [`super::parser::AgentProfile`] this struct adds:
/// - **Mode** (`primary` / `subagent` / `all`) so the router can filter agents.
/// - **ModelBinding** so the OC-Phase 1 `ProviderFactory` knows which provider
///   client to instantiate.
/// - **ToolSelection** so the OC-Phase 3 registry pre-filter can compute
///   `available_for(spec, ruleset)`.
/// - **AgentPermissionSpec** so OC-Phase 3 can merge parent ∩ child rulesets
///   without falling back to a string list.
/// - Per-agent **temperature** / **top_p** / **max_steps** override knobs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentExecutionSpec {
    pub name: String,

    #[serde(default)]
    pub description: String,

    #[serde(default)]
    pub mode: AgentMode,

    /// `Some(_)` only when the source agent profile / config carried an explicit
    /// `provider/model` binding. Plain `default` / `fast` keep `model = None`
    /// and surface as a `model_preference` string elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelBinding>,

    #[serde(default)]
    pub system_prompt: String,

    #[serde(default)]
    pub tools: ToolSelection,

    #[serde(default)]
    pub permission: AgentPermissionSpec,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scenario: bare `provider/model` lifts cleanly; legacy `default` / `fast`
    /// strings stay un-lifted so the caller can keep them as `model_preference`.
    #[test]
    fn parse_model_binding_basic_and_legacy() {
        let mb = ModelBinding::parse("anthropic/claude-3-5-sonnet-latest").unwrap();
        assert_eq!(mb.provider_id, "anthropic");
        assert_eq!(mb.model_id, "claude-3-5-sonnet-latest");
        assert!(mb.variant.is_none());

        assert!(ModelBinding::parse("default").is_none());
        assert!(ModelBinding::parse("fast").is_none());
        assert!(ModelBinding::parse("").is_none());
    }

    /// Scenario: model id may itself contain `/`, mirroring opencode's
    /// `Provider.parseModel()`. Only the first segment is the provider id.
    #[test]
    fn parse_model_binding_preserves_inner_slashes() {
        let mb = ModelBinding::parse("azure/openai-gpt-4").unwrap();
        assert_eq!(mb.provider_id, "azure");
        assert_eq!(mb.model_id, "openai-gpt-4");

        let mb = ModelBinding::parse("bedrock/anthropic.claude-3-5-sonnet/v1").unwrap();
        assert_eq!(mb.provider_id, "bedrock");
        assert_eq!(mb.model_id, "anthropic.claude-3-5-sonnet/v1");
        assert_eq!(
            mb.to_canonical_string(),
            "bedrock/anthropic.claude-3-5-sonnet/v1"
        );
    }

    /// Scenario: `@variant` suffix lifts to the dedicated field; the round-trip
    /// through [`ModelBinding::to_canonical_string`] is lossless and re-parses
    /// to the same binding.
    #[test]
    fn parse_model_binding_extracts_variant_and_round_trips() {
        let mb = ModelBinding::parse("anthropic/claude-opus-4@thinking").unwrap();
        assert_eq!(mb.provider_id, "anthropic");
        assert_eq!(mb.model_id, "claude-opus-4");
        assert_eq!(mb.variant.as_deref(), Some("thinking"));
        let canonical = mb.to_canonical_string();
        assert_eq!(canonical, "anthropic/claude-opus-4@thinking");
        assert_eq!(ModelBinding::parse(&canonical), Some(mb));

        // Within the final slash-separated segment the variant separator
        // splits at the LAST '@', so a model id that itself ends with
        // `@something` (rare but legal) still round-trips.
        let mb = ModelBinding::parse("openai/gpt-4@v1@thinking").unwrap();
        assert_eq!(mb.model_id, "gpt-4@v1");
        assert_eq!(mb.variant.as_deref(), Some("thinking"));
    }

    /// Scenario: an `@` in a non-final slash-separated segment is part of the
    /// model id, not a variant separator. `provider/foo@bar/baz` must keep
    /// the slash-containing model id intact and report no variant — and the
    /// canonical string must round-trip without surfacing a phantom variant.
    #[test]
    fn parse_model_binding_at_in_non_final_segment_is_part_of_model_id() {
        let mb = ModelBinding::parse("provider/foo@bar/baz").unwrap();
        assert_eq!(mb.provider_id, "provider");
        assert_eq!(mb.model_id, "foo@bar/baz");
        assert!(mb.variant.is_none());
        let canonical = mb.to_canonical_string();
        assert_eq!(canonical, "provider/foo@bar/baz");
        assert_eq!(ModelBinding::parse(&canonical), Some(mb));

        // Combined: `@` in earlier segment AND `@variant` in the final segment
        // must split only on the final `@`.
        let mb = ModelBinding::parse("provider/foo@bar/baz@thinking").unwrap();
        assert_eq!(mb.model_id, "foo@bar/baz");
        assert_eq!(mb.variant.as_deref(), Some("thinking"));
        let canonical = mb.to_canonical_string();
        assert_eq!(canonical, "provider/foo@bar/baz@thinking");
        assert_eq!(ModelBinding::parse(&canonical), Some(mb));
    }

    /// Scenario: malformed bindings (empty halves, leading/trailing `/`, empty
    /// variant suffix, empty model id before `@`) are rejected rather than
    /// producing zero-length provider, model, or variant ids — and never
    /// silently absorbing a stray `@` into the model id.
    #[test]
    fn parse_model_binding_rejects_malformed() {
        assert!(ModelBinding::parse("/foo").is_none());
        assert!(ModelBinding::parse("foo/").is_none());
        assert!(ModelBinding::parse("/").is_none());
        // Trailing `@` with empty variant must NOT be accepted.
        assert!(ModelBinding::parse("anthropic/claude@").is_none());
        // Empty model id with non-empty variant (`provider/@variant`) must
        // also be rejected — silently producing `model_id = "@variant"` would
        // mask a typo.
        assert!(ModelBinding::parse("anthropic/@thinking").is_none());
    }

    /// Scenario: every field round-trips through JSON without surprising default
    /// re-encoding (variants, options, enums) — this is the contract guarantee
    /// callers rely on when persisting a spec snapshot.
    #[test]
    fn agent_execution_spec_roundtrips_full() {
        let spec = AgentExecutionSpec {
            name: "planner".to_string(),
            description: "Implementation planning specialist".to_string(),
            mode: AgentMode::Primary,
            model: Some(ModelBinding {
                provider_id: "anthropic".to_string(),
                model_id: "claude-3-5-sonnet-latest".to_string(),
                variant: Some("reasoning".to_string()),
            }),
            system_prompt: "You are a planner.".to_string(),
            tools: ToolSelection::Allow(vec!["read_file".to_string(), "list_dir".to_string()]),
            permission: AgentPermissionSpec {
                allowed_tools: BTreeSet::from(["read_file".to_string()]),
                denied_tools: BTreeSet::from(["shell".to_string()]),
                allowed_source_slugs: BTreeSet::from(["builtin".to_string()]),
                approval_routing: ApprovalRoutingSpec::SessionPreApproved,
                may_spawn_sub_agents: true,
            },
            temperature: Some(0.5),
            top_p: Some(0.95),
            max_steps: Some(30),
        };

        let json = serde_json::to_string(&spec).unwrap();
        let back: AgentExecutionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, spec);
    }

    /// Scenario: a minimal spec (only `name`) decodes with all the documented
    /// defaults — `Primary`, no model binding, `Inherit` tool selection, empty
    /// permission, no overrides. This is the shape an OC-Phase 2 parser will
    /// produce for a legacy frontmatter that only carries a name.
    #[test]
    fn agent_execution_spec_minimal_defaults() {
        let json = r#"{"name":"planner"}"#;
        let spec: AgentExecutionSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.name, "planner");
        assert_eq!(spec.description, "");
        assert_eq!(spec.mode, AgentMode::Primary);
        assert!(spec.model.is_none());
        assert_eq!(spec.tools, ToolSelection::Inherit);
        assert_eq!(spec.permission, AgentPermissionSpec::default());
        assert!(spec.temperature.is_none());
        assert!(spec.top_p.is_none());
        assert!(spec.max_steps.is_none());
    }

    /// Scenario: the three `AgentMode` variants serialize / deserialize exactly
    /// as the documented snake_case strings. The router and config loaders use
    /// these strings verbatim, so this guards against an accidental rename.
    #[test]
    fn agent_mode_serde_strings() {
        let primary: AgentMode = serde_json::from_str("\"primary\"").unwrap();
        let subagent: AgentMode = serde_json::from_str("\"subagent\"").unwrap();
        let all: AgentMode = serde_json::from_str("\"all\"").unwrap();
        assert_eq!(primary, AgentMode::Primary);
        assert_eq!(subagent, AgentMode::Subagent);
        assert_eq!(all, AgentMode::All);

        assert_eq!(
            serde_json::to_string(&AgentMode::Primary).unwrap(),
            "\"primary\""
        );
        assert_eq!(
            serde_json::to_string(&AgentMode::Subagent).unwrap(),
            "\"subagent\""
        );
        assert_eq!(serde_json::to_string(&AgentMode::All).unwrap(), "\"all\"");
    }

    /// Scenario: an unknown `AgentMode` variant is rejected loudly. We do not
    /// want a typo (`"subgent"`) to silently coerce to `Primary`.
    #[test]
    fn agent_mode_rejects_unknown_variant() {
        let err = serde_json::from_str::<AgentMode>("\"subgent\"").unwrap_err();
        assert!(err.to_string().contains("subgent"));
    }

    /// Scenario: mode predicates encode the eligibility table from the doc:
    /// Primary may drive a session, Subagent may be dispatched via `task`,
    /// All may do both.
    #[test]
    fn agent_mode_eligibility_predicates() {
        assert!(AgentMode::Primary.is_primary_eligible());
        assert!(!AgentMode::Primary.is_subagent_eligible());

        assert!(!AgentMode::Subagent.is_primary_eligible());
        assert!(AgentMode::Subagent.is_subagent_eligible());

        assert!(AgentMode::All.is_primary_eligible());
        assert!(AgentMode::All.is_subagent_eligible());
    }

    /// Scenario: the three `ToolSelection` variants tag-encode adjacently as
    /// `{"kind":"allow","tools":[...]}`. The kind string is the public contract
    /// for human-edited TOML / JSON agent definitions.
    #[test]
    fn tool_selection_serde_tagged() {
        let inherit = ToolSelection::Inherit;
        let allow = ToolSelection::Allow(vec!["read_file".to_string()]);
        let deny = ToolSelection::Deny(vec!["shell".to_string()]);

        let inherit_json = serde_json::to_string(&inherit).unwrap();
        let allow_json = serde_json::to_string(&allow).unwrap();
        let deny_json = serde_json::to_string(&deny).unwrap();

        assert_eq!(inherit_json, r#"{"kind":"inherit"}"#);
        assert_eq!(allow_json, r#"{"kind":"allow","tools":["read_file"]}"#);
        assert_eq!(deny_json, r#"{"kind":"deny","tools":["shell"]}"#);

        let back_inherit: ToolSelection = serde_json::from_str(&inherit_json).unwrap();
        let back_allow: ToolSelection = serde_json::from_str(&allow_json).unwrap();
        let back_deny: ToolSelection = serde_json::from_str(&deny_json).unwrap();
        assert_eq!(back_inherit, inherit);
        assert_eq!(back_allow, allow);
        assert_eq!(back_deny, deny);
    }

    /// Scenario: empty allow / deny lists round-trip as `{"kind":"allow","tools":[]}`
    /// rather than collapsing into `Inherit`. An author who writes
    /// `{kind: allow, tools: []}` is explicitly opting into deny-everything;
    /// the runtime must see that as different from `Inherit`.
    #[test]
    fn tool_selection_serde_empty_lists() {
        let allow_empty = ToolSelection::Allow(Vec::new());
        let deny_empty = ToolSelection::Deny(Vec::new());

        let allow_json = serde_json::to_string(&allow_empty).unwrap();
        let deny_json = serde_json::to_string(&deny_empty).unwrap();
        assert_eq!(allow_json, r#"{"kind":"allow","tools":[]}"#);
        assert_eq!(deny_json, r#"{"kind":"deny","tools":[]}"#);

        let back_allow: ToolSelection = serde_json::from_str(&allow_json).unwrap();
        let back_deny: ToolSelection = serde_json::from_str(&deny_json).unwrap();
        assert_eq!(back_allow, allow_empty);
        assert_eq!(back_deny, deny_empty);
        assert_ne!(back_allow, ToolSelection::Inherit);
        assert_ne!(back_deny, ToolSelection::Inherit);
    }

    /// Scenario: an unknown `ToolSelection.kind` string is rejected. A typo
    /// like `{"kind":"alloow"}` must not silently fall back to `Inherit`.
    #[test]
    fn tool_selection_rejects_unknown_kind() {
        let err =
            serde_json::from_str::<ToolSelection>(r#"{"kind":"alloow","tools":[]}"#).unwrap_err();
        assert!(err.to_string().contains("alloow"));
    }

    /// Scenario: an explicit JSON `null` for an `Option<f32>` field deserializes
    /// to `None`, identical to omitting the field. The serialized form for a
    /// `None` value omits the key entirely (`skip_serializing_if`).
    #[test]
    fn agent_execution_spec_optional_fields_null_and_omitted() {
        let with_null = r#"{"name":"x","temperature":null,"top_p":null,"max_steps":null}"#;
        let spec_null: AgentExecutionSpec = serde_json::from_str(with_null).unwrap();
        assert!(spec_null.temperature.is_none());
        assert!(spec_null.top_p.is_none());
        assert!(spec_null.max_steps.is_none());

        let omitted = r#"{"name":"x"}"#;
        let spec_omitted: AgentExecutionSpec = serde_json::from_str(omitted).unwrap();
        assert_eq!(spec_omitted, spec_null);

        let json = serde_json::to_string(&spec_omitted).unwrap();
        assert!(!json.contains("temperature"));
        assert!(!json.contains("top_p"));
        assert!(!json.contains("max_steps"));
    }

    /// Scenario: an unknown frontmatter field for the spec is rejected, so a
    /// typo like `permisson` (missing `i`) does not silently degrade to
    /// default-deny without operator awareness.
    #[test]
    fn agent_execution_spec_rejects_unknown_fields() {
        let json = r#"{"name":"x","unexpected_field":42}"#;
        let err = serde_json::from_str::<AgentExecutionSpec>(json).unwrap_err();
        assert!(err.to_string().contains("unexpected_field"));
    }

    /// Scenario: an unknown `ApprovalRoutingSpec` variant is rejected. The
    /// approval routing decision is security-sensitive; unknown values must not
    /// silently fall back to `Layer1Human`.
    #[test]
    fn approval_routing_rejects_unknown_variant() {
        let err = serde_json::from_str::<ApprovalRoutingSpec>("\"layer1_robot\"").unwrap_err();
        assert!(err.to_string().contains("layer1_robot"));
    }

    /// Scenario: `AgentPermissionSpec` defaults match the gated runtime
    /// `AgentPermissionProfile` defaults — empty `BTreeSet`s, Layer1Human
    /// routing, no nested spawn — so the OC-Phase 3 conversion is structural.
    #[test]
    fn agent_permission_spec_defaults_match_runtime() {
        let spec = AgentPermissionSpec::default();
        assert!(spec.allowed_tools.is_empty());
        assert!(spec.denied_tools.is_empty());
        assert!(spec.allowed_source_slugs.is_empty());
        assert_eq!(spec.approval_routing, ApprovalRoutingSpec::Layer1Human);
        assert!(!spec.may_spawn_sub_agents);
    }

    /// Scenario: `BTreeSet` containers automatically dedup repeated tool names
    /// in the input JSON. Authoring noise like `["read_file", "read_file"]`
    /// collapses to a single entry on parse.
    #[test]
    fn agent_permission_spec_btreeset_dedups_input() {
        let json = r#"{"allowed_tools":["read_file","read_file","list_dir"]}"#;
        let spec: AgentPermissionSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.allowed_tools.len(), 2);
        assert!(spec.allowed_tools.contains("read_file"));
        assert!(spec.allowed_tools.contains("list_dir"));
    }
}
