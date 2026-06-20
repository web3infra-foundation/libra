//! CEX-S2-12 / S2-INV-08 sub-agent flag-off regression tests.
//!
//! Spec: `docs/development/commands/agent.md` Step 2.3 (CEX-S2-12) and the Step 2 exit
//! standard "兼容性" row (CP-S2-3 flag-off equivalence): with
//! `code.sub_agents.enabled = false` — the default — the sub-agent runtime must
//! stay completely inert, so a fresh single-agent install behaves identically to
//! the Step 1 baseline. Any change that makes the sub-agent surface active by
//! default is a silent compatibility regression.
//!
//! These checks run against the **public** config re-exports
//! (`libra::internal::ai::agent::profile`) so a regression that flips a default
//! or couples the sub-agent gate to another config surfaces here rather than at
//! runtime. The dispatcher-construction gate itself (`if
//! agents_config.sub_agents.enabled { … }` in `src/command/code.rs`) is the
//! single runtime branch these defaults feed; pinning the defaults pins the
//! flag-off behaviour the branch enforces.

use libra::internal::ai::agent::profile::{
    AgentsConfig, AutoMergeConfig, MultiAgentConfig, SubAgentsConfig,
};

/// S2-INV-08: `SubAgentsConfig::default()` is disabled. This is the value the
/// session bootstrap reads when an install has no `[code.sub_agents]` table, and
/// the runtime only builds the dispatcher when it is `true`.
#[test]
fn sub_agents_config_defaults_to_disabled() {
    let config = SubAgentsConfig::default();
    assert!(
        !config.enabled,
        "S2-INV-08: sub-agents must default to disabled",
    );
    // Auto-merge is a separately-gated CEX-S2-15 feature, also off by default.
    assert!(
        !config.auto_merge.enabled,
        "auto-merge must default to disabled",
    );
}

/// The companion `[code.multi_agent]` surface is likewise off by default and
/// enforces the conservative single-sub-agent limits, so even a partial config
/// that enables one surface but not the other cannot widen concurrency.
#[test]
fn multi_agent_config_defaults_are_conservative() {
    let config = MultiAgentConfig::default();
    assert!(!config.enabled, "multi-agent must default to disabled");
    assert_eq!(config.max_subagent_depth, 1, "default depth must be 1");
    assert_eq!(
        config.max_concurrent_subagents, 1,
        "default concurrency must be 1",
    );
    assert!(
        !config.allow_full_copy,
        "full-copy workspace fallback must be opt-in",
    );
    assert_eq!(
        config.source_concurrency_limit, 0,
        "CEX-S2-14 per-slug source throttle must default to 0 (disabled) so \
         flag-off source-call behaviour is unbounded as before",
    );
}

/// An `AgentsConfig` parsed from empty TOML — the canonical fresh-install
/// state — has every sub-agent surface off. This is the flag-off baseline the
/// CP-S2-3 equivalence check relies on.
#[test]
fn empty_config_is_fully_flag_off() {
    let config: AgentsConfig = toml::from_str("").expect("empty TOML parses");
    assert!(!config.sub_agents.enabled);
    assert!(!config.sub_agents.auto_merge.enabled);
    assert!(!config.multi_agent.enabled);
    // The throttle is absent from empty TOML → defaults to disabled.
    assert_eq!(config.multi_agent.source_concurrency_limit, 0);
}

/// CEX-S2-14: an explicit `source_concurrency_limit` in `[multi_agent]` is
/// parsed (and accepted under `deny_unknown_fields`), so the bootstrap can
/// activate per-slug source throttling from `agents.toml`.
#[test]
fn multi_agent_source_concurrency_limit_parses_from_toml() {
    let toml = r#"
[multi_agent]
enabled = true
source_concurrency_limit = 4
"#;
    let config: AgentsConfig = toml::from_str(toml).expect("config parses");
    assert_eq!(config.multi_agent.source_concurrency_limit, 4);
}

/// Explicitly setting `enabled = false` round-trips to a disabled config —
/// parsing the flag-off form is not silently coerced to enabled, and an
/// operator can pin the default explicitly.
#[test]
fn explicit_disabled_sub_agents_stays_disabled() {
    let toml = r#"
[sub_agents]
enabled = false
max_parallel = 4
"#;
    let config: AgentsConfig = toml::from_str(toml).expect("config parses");
    assert!(
        !config.sub_agents.enabled,
        "an explicit `enabled = false` must stay disabled",
    );
    // A configured `max_parallel` is parsed but irrelevant while disabled — the
    // gate is `enabled`, not the parallelism budget.
    assert_eq!(config.sub_agents.max_parallel, 4);
}

/// The default `AutoMergeConfig` is disabled — pinned independently because
/// CEX-S2-15 auto-merge is the most safety-sensitive flag: it is the only path
/// that could apply a sub-agent patch without a human, so it must never default
/// on.
#[test]
fn auto_merge_defaults_to_disabled() {
    assert!(!AutoMergeConfig::default().enabled);
}
