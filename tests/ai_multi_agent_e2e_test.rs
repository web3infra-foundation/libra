//! S7 acceptance scenario: multi-agent declarative config E2E
//! (OC-Phase 5 P5.5).
//!
//! Per docs/improvement/opencode.md:1724-1732 the scenario covers:
//!
//! 1. Load `examples/multi_agent.toml` and validate the documented
//!    `planner → coder → reviewer` pipeline parses cleanly under the
//!    OC-Phase 5 P5.1 schema + validator.
//! 2. Each of the three agents records usage with a distinct
//!    `agent_name` (P5.2). The query layer aggregates at three grains
//!    (`(provider, model)`, `(agent)`, `(agent, provider, model)`)
//!    and surfaces the agent dimension to the TUI table renderer.
//! 3. Setting `max_session_cost_usd` low forces the budget tracker
//!    (P5.3) to fail with `BudgetExceededError` whose
//!    `stable_code() == StableErrorCode::AgentBudgetExceeded`
//!    (`LBR-AGENT-001`); the rendered error message surfaces the
//!    threshold + actual values for the operator.
//!
//! This is the data-flow E2E. The dispatcher integration that
//! actually drives the planner → sub-agent loop lives behind the
//! `subagent-scaffold` feature gate (OC-Phase 3); when that gate
//! lifts in P3 GA the scaffolded sub-agent run hooks into the same
//! `BudgetTracker` + `agent_name` columns this test exercises by
//! hand. The walk is feature-gated on `test-provider` per the
//! doc-mandated scope so a default-build CI run does not pull in
//! the fake-provider fixture machinery.

#![cfg(feature = "test-provider")]

use libra::internal::{
    ai::{
        agent::{
            BudgetTracker, format_agents_table, format_budget_status, format_usage_table,
            profile::config::AgentsConfig,
        },
        completion::CompletionUsageSummary,
        usage::{UsageContext, UsageGrouping, UsageQuery, UsageQueryFilter, UsageRecorder},
    },
    db::migration::run_builtin_migrations,
};
use sea_orm::Database;

const SAMPLE_TOML_PATH: &str = "examples/multi_agent.toml";

fn fake_usage(input: u64, output: u64, cost_usd: f64) -> CompletionUsageSummary {
    CompletionUsageSummary {
        input_tokens: input,
        output_tokens: output,
        cached_tokens: None,
        reasoning_tokens: None,
        total_tokens: Some(input + output),
        cost_usd: Some(cost_usd),
    }
}

fn agent_context(agent: &str, provider: &str, model: &str) -> UsageContext {
    UsageContext {
        session_id: Some("session-s7".to_string()),
        thread_id: Some("thread-s7".to_string()),
        agent_run_id: None,
        run_id: Some(format!("run-{agent}")),
        provider: provider.to_string(),
        model: model.to_string(),
        request_kind: "completion".to_string(),
        intent: None,
        agent_name: Some(agent.to_string()),
    }
}

/// S7 phase 1: the canonical declarative config in
/// `examples/multi_agent.toml` parses + validates cleanly under the
/// P5.1 schema. Pins the documented `planner → coder → reviewer`
/// pipeline shape so a future schema change either updates the
/// example or breaks this test (whichever the operator notices
/// first).
#[test]
fn s7_canonical_example_toml_parses_and_validates() {
    let toml_str = std::fs::read_to_string(SAMPLE_TOML_PATH)
        .expect("examples/multi_agent.toml must be readable from the repo root");
    let cfg = AgentsConfig::from_toml_str(&toml_str)
        .expect("examples/multi_agent.toml must parse under the P5.1 schema");
    cfg.validate()
        .expect("examples/multi_agent.toml must pass P5.1 validation");

    // Documented pipeline shape.
    assert!(cfg.multi_agent.enabled, "feature flag must be on");
    assert_eq!(cfg.agents.len(), 3);
    assert!(cfg.agents.contains_key("planner"));
    assert!(cfg.agents.contains_key("coder"));
    assert!(cfg.agents.contains_key("reviewer"));
    assert_eq!(cfg.agents["planner"].mode, "primary");
    assert_eq!(cfg.agents["coder"].mode, "subagent");
    assert_eq!(cfg.agents["reviewer"].mode, "subagent");

    // Reviewer is read-only by construction — `write = "deny"` is the
    // belt-and-braces guard that an unintentional re-use of the
    // reviewer for edits trips the pairing validator before any side
    // effect.
    let reviewer_perm = &cfg.agents["reviewer"].permission;
    assert_eq!(
        reviewer_perm
            .get("write")
            .map(|p| format!("{p:?}").to_lowercase()),
        Some("deny".to_string())
    );

    // Per-agent budgets are present for the two sub-agents.
    assert!(cfg.budget.per_agent.contains_key("coder"));
    assert!(cfg.budget.per_agent.contains_key("reviewer"));

    // Snapshot the agents-table renderer against the actual fixture
    // so a future renderer change is caught at the same time as a
    // schema change.
    let agents_render = format_agents_table(&cfg);
    assert!(agents_render.contains("planner"));
    assert!(agents_render.contains("coder"));
    assert!(agents_render.contains("reviewer"));
    assert!(agents_render.contains("anthropic/claude-3-5-sonnet-latest"));
}

/// S7 phase 2: each of the three agents records usage with a
/// distinct `agent_name`. The `(agent_name, provider, model)`
/// aggregation surfaces three rows; the agent-only grain folds them
/// into per-agent buckets for the `/usage --by=agent` surface; the
/// rendered table contains every agent name.
#[tokio::test]
async fn s7_three_agents_persist_distinct_agent_name_rows() {
    let conn = Database::connect("sqlite::memory:").await.unwrap();
    run_builtin_migrations(&conn).await.unwrap();
    let recorder = UsageRecorder::new(conn.clone());

    recorder
        .record_summary(
            &agent_context("planner", "anthropic", "claude-3-5-sonnet-latest"),
            &fake_usage(120, 60, 0.001),
            Some(4_000),
        )
        .await
        .unwrap();
    recorder
        .record_summary(
            &agent_context("coder", "deepseek", "deepseek-chat"),
            &fake_usage(800, 400, 0.0025),
            Some(15_000),
        )
        .await
        .unwrap();
    recorder
        .record_summary(
            &agent_context("reviewer", "openai", "gpt-4o-mini"),
            &fake_usage(150, 90, 0.0008),
            Some(5_000),
        )
        .await
        .unwrap();

    let query = UsageQuery::new(conn.clone());

    // Full grain: three distinct (agent, provider, model) rows.
    let by_apm = query
        .aggregate_filtered(
            UsageGrouping::AgentProviderModel,
            &UsageQueryFilter::default(),
        )
        .await
        .unwrap();
    assert_eq!(by_apm.len(), 3, "expect three agent rows");
    let agent_names: Vec<&str> = by_apm
        .iter()
        .filter_map(|r| r.agent_name.as_deref())
        .collect();
    assert!(agent_names.contains(&"planner"));
    assert!(agent_names.contains(&"coder"));
    assert!(agent_names.contains(&"reviewer"));

    // Agent-only grain: same three buckets, agnostic to model.
    let by_agent = query
        .aggregate_filtered(UsageGrouping::Agent, &UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(by_agent.len(), 3);

    // (provider, model) legacy grain: also three buckets here because
    // each agent runs on a different (provider, model) pair, but the
    // important shape contract is that `agent_name` is `None` on
    // every row (the legacy renderer never surfaces the agent
    // dimension regardless of source data). This is the back-compat
    // gate for any caller that pre-dates P5.2.
    let by_pm = query
        .aggregate_filtered(UsageGrouping::ProviderModel, &UsageQueryFilter::default())
        .await
        .unwrap();
    assert_eq!(by_pm.len(), 3);
    assert!(
        by_pm.iter().all(|r| r.agent_name.is_none()),
        "ProviderModel grain must hide the agent dimension; got: {by_pm:?}"
    );
    let providers: Vec<&str> = by_pm.iter().map(|r| r.provider.as_str()).collect();
    assert!(providers.contains(&"anthropic"));
    assert!(providers.contains(&"deepseek"));
    assert!(providers.contains(&"openai"));

    // Renderer surfaces every agent in the /usage --by=agent table.
    let table = format_usage_table(&by_apm);
    assert!(table.contains("Usage:"));
    for name in &["planner", "coder", "reviewer"] {
        assert!(
            table.contains(name),
            "rendered usage table must mention `{name}`, got:\n{table}"
        );
    }
}

/// S7 phase 3: a tight session-wide cost cap (`max_session_cost_usd
/// = 0.001`) crossed by accumulated fake usage produces a
/// `BudgetExceededError` with the documented stable error code,
/// scope, axis, and renderable message. The check is idempotent so
/// the dispatcher (when it lands in P5 follow-ups) can poll on every
/// turn boundary without re-firing.
#[test]
fn s7_session_cost_cap_breach_surfaces_agent_budget_exceeded_code() {
    use libra::{
        internal::ai::agent::{BudgetAxis, BudgetScope},
        utils::error::StableErrorCode,
    };

    // Use the canonical example as the source of truth for the cap
    // shape, but lower the cap to a tight value so the test does not
    // depend on the example's session ceiling.
    let mut cfg =
        AgentsConfig::from_toml_str(&std::fs::read_to_string(SAMPLE_TOML_PATH).unwrap()).unwrap();
    cfg.budget.max_session_cost_usd = Some(0.001);
    cfg.budget.warn_session_cost_usd = Some(0.0005);

    let mut tracker = BudgetTracker::new();
    // Three turns each adding $0.0005 — accumulates to $0.0015,
    // crossing both warn and hard cap.
    tracker.accumulate(&fake_usage(50, 25, 0.0005), Some(2_000), Some("planner"));
    tracker.accumulate(&fake_usage(50, 25, 0.0005), Some(2_000), Some("coder"));
    tracker.accumulate(&fake_usage(50, 25, 0.0005), Some(2_000), Some("reviewer"));

    let err = tracker
        .check_session(&cfg)
        .expect_err("session cost cap must be breached");
    assert_eq!(err.axis, BudgetAxis::Cost);
    assert_eq!(err.scope, BudgetScope::Session);
    assert_eq!(err.stable_code(), StableErrorCode::AgentBudgetExceeded);

    // Rendered message carries the doc-mandated stable code +
    // operator-actionable threshold + actual values.
    let rendered = err.to_string();
    assert!(rendered.contains("LBR-AGENT-001"));
    assert!(rendered.contains("$0.0010")); // threshold
    assert!(rendered.contains("$0.0015")); // actual
    assert!(rendered.contains("session"));

    // Idempotent: re-checking with no further accumulation produces
    // the same Err shape.
    let err2 = tracker
        .check_session(&cfg)
        .expect_err("idempotent re-check");
    assert_eq!(err2, err);

    // The /budget renderer surfaces the warning section once the
    // running total has crossed the warn threshold.
    let warnings = tracker.drain_warnings(&cfg);
    assert!(
        warnings
            .iter()
            .any(|w| w.axis == BudgetAxis::Cost && w.scope == BudgetScope::Session),
        "drain_warnings must emit a session-cost warning, got: {warnings:?}"
    );
    let budget_render = format_budget_status(&cfg, &tracker, &warnings);
    assert!(budget_render.contains("Budget:"));
    assert!(budget_render.contains("session:"));
    // The session row shows the actual + cap together so the
    // operator can see how much they are over.
    assert!(budget_render.contains("$0.0015"));
    assert!(budget_render.contains("$0.0010"));
}
