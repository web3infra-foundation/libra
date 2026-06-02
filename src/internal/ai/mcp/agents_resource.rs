//! Sub-agent run MCP resource URI routing (CEX-S2-16, Step 2.6).
//!
//! CEX-S2-16 adds six read-only MCP resource URIs that let an MCP client observe
//! sub-agent runs (agent.md Step 2.6 应该完成的功能 (3)/(4)):
//!
//! - `libra://agents/runs` — list every agent run
//! - `libra://agents/runs/{id}` — one run's detail
//! - `libra://agents/runs/{id}/permissions` — its permission profile
//! - `libra://agents/runs/{id}/budget` — its budget / usage
//! - `libra://agents/runs/{id}/context` — its context pack
//! - `libra://agents/merge-candidates/{id}` — one merge candidate's detail
//!
//! This module owns the **pure** URI → typed-request routing *and* the
//! request → JSON-body rendering ([`render_agent_resource`]). Loading the
//! `AgentRun` / `MergeCandidate` records is the server's job (and lands with the
//! run-persistence path); this module renders whatever records the server
//! supplies. Keeping parse + render pure makes the whole resource family
//! exhaustively unit-testable without any storage, and gives the server one
//! place to dispatch from. Neither parsing nor rendering performs I/O.

use serde_json::{Value, json};

use crate::internal::ai::agent_run::{
    AgentBudget, AgentContextPack, AgentPermissionProfile, AgentRun, MergeCandidate, RunUsage,
};

/// A parsed `libra://agents/*` resource request. The `{id}` segments are
/// captured as raw strings; validating them as real run / candidate ids is the
/// resolver's job (an unknown id is a not-found at read time, not a parse
/// error).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentResourceRequest {
    /// `libra://agents/runs` — list all runs.
    RunList,
    /// `libra://agents/runs/{id}` — one run's detail.
    RunDetail { run_id: String },
    /// `libra://agents/runs/{id}/permissions`.
    RunPermissions { run_id: String },
    /// `libra://agents/runs/{id}/budget`.
    RunBudget { run_id: String },
    /// `libra://agents/runs/{id}/context`.
    RunContext { run_id: String },
    /// `libra://agents/merge-candidates/{id}`.
    MergeCandidate { candidate_id: String },
}

/// The URI scheme + authority prefix every agent resource shares.
const AGENTS_PREFIX: &str = "libra://agents/";

impl AgentResourceRequest {
    /// Parse a `libra://agents/...` URI into a typed request, or `None` if the
    /// URI is not a recognised agent resource (the caller then falls through to
    /// the next resource family). Returns `None` — never a partial match — for a
    /// malformed agent URI (e.g. an empty `{id}` or an unknown trailing
    /// segment) so an ambiguous URI is a clean not-found rather than a
    /// mis-route.
    pub fn parse(uri: &str) -> Option<Self> {
        let rest = uri.strip_prefix(AGENTS_PREFIX)?;

        if let Some(candidate_id) = rest.strip_prefix("merge-candidates/") {
            return non_empty_single_segment(candidate_id)
                .map(|id| Self::MergeCandidate { candidate_id: id });
        }

        let runs_rest = rest.strip_prefix("runs")?;
        // `libra://agents/runs` — list (allow an optional trailing slash).
        if runs_rest.is_empty() || runs_rest == "/" {
            return Some(Self::RunList);
        }
        // Everything else must be `runs/{id}[/sub]`.
        let after = runs_rest.strip_prefix('/')?;
        let mut segments = after.split('/');
        let run_id = segments.next().filter(|s| !s.is_empty())?.to_string();
        match segments.next() {
            None => Some(Self::RunDetail { run_id }),
            Some("permissions") if segments.next().is_none() => {
                Some(Self::RunPermissions { run_id })
            }
            Some("budget") if segments.next().is_none() => Some(Self::RunBudget { run_id }),
            Some("context") if segments.next().is_none() => Some(Self::RunContext { run_id }),
            // Unknown sub-resource or extra trailing segments -> not an agent
            // resource we recognise.
            _ => None,
        }
    }

    /// The run id this request targets, if any (`None` for [`RunList`] and
    /// [`MergeCandidate`]).
    pub fn run_id(&self) -> Option<&str> {
        match self {
            Self::RunDetail { run_id }
            | Self::RunPermissions { run_id }
            | Self::RunBudget { run_id }
            | Self::RunContext { run_id } => Some(run_id),
            Self::RunList | Self::MergeCandidate { .. } => None,
        }
    }
}

/// `Some(seg)` when `seg` is a single non-empty path segment (no `/`), else
/// `None` — used to reject `merge-candidates/` with a missing or multi-segment
/// id.
fn non_empty_single_segment(seg: &str) -> Option<String> {
    if seg.is_empty() || seg.contains('/') {
        None
    } else {
        Some(seg.to_string())
    }
}

/// Render one [`AgentRun`] into the summary row used by the `runs` list view.
pub fn render_run_row(run: &AgentRun) -> Value {
    json!({
        "id": run.id.0.to_string(),
        "task_id": run.task_id.0.to_string(),
        "status": run.status,
        "provider": run.provider,
        "model": run.model,
        "transcript_path": run.transcript_path,
        "workspace_path": run.workspace_path,
    })
}

/// Render the `libra://agents/runs` list body for the supplied runs (in the
/// order given). A read-only projection — no I/O.
pub fn render_run_list(runs: &[AgentRun]) -> Value {
    json!({ "runs": runs.iter().map(render_run_row).collect::<Vec<_>>() })
}

/// Render the `libra://agents/runs/{id}` detail body — the run summary plus its
/// persisted source-call count (CEX-S2-16; the per-run trace link landed
/// v0.17.1254). The list view stays lean (no per-run count query); the detail
/// view carries the activity metric.
pub fn render_run_detail(run: &AgentRun, source_call_count: u32) -> Value {
    let mut body = render_run_row(run);
    if let Value::Object(map) = &mut body {
        map.insert(
            "source_call_count".to_string(),
            Value::from(source_call_count),
        );
    }
    body
}

/// Render the `libra://agents/runs/{id}/permissions` body from the run's
/// permission profile.
pub fn render_run_permissions(run_id: &str, profile: &AgentPermissionProfile) -> Value {
    json!({
        "agent_run_id": run_id,
        "allowed_tools": profile.allowed_tools.iter().collect::<Vec<_>>(),
        "denied_tools": profile.denied_tools.iter().collect::<Vec<_>>(),
        "allowed_source_slugs": profile.allowed_source_slugs.iter().collect::<Vec<_>>(),
        "approval_routing": profile.approval_routing,
        "may_spawn_sub_agents": profile.may_spawn_sub_agents,
    })
}

/// Render the `libra://agents/runs/{id}/budget` body: the configured budget plus
/// the run's current usage and the dimensions (if any) it has exceeded.
pub fn render_run_budget(
    run_id: &str,
    budget: &AgentBudget,
    usage: &RunUsage,
    source_call_count: u32,
) -> Value {
    let exceeded = budget.exceeded_dimensions(usage, source_call_count);
    // CEX-S2-16 (1) "budget remaining": the headroom still unspent on each
    // enforced dimension (`limit - used`, saturating), the inverse of
    // `exceeded_dimensions`. Rendered in declaration order as
    // `{dimension, remaining}` objects so the view is deterministic and a
    // consumer can read both the dimension and its remaining quantity.
    let remaining = budget
        .remaining_dimensions(usage, source_call_count)
        .into_iter()
        .map(|(dimension, remaining)| json!({ "dimension": dimension, "remaining": remaining }))
        .collect::<Vec<_>>();
    json!({
        "agent_run_id": run_id,
        "budget": budget,
        "usage": {
            "total_tokens": usage.total_tokens(),
            "tool_call_count": usage.tool_call_count,
            "wall_clock_ms": usage.wall_clock_ms,
            "source_call_count": source_call_count,
            "cost_estimate_micro_dollars": usage.cost_estimate_micro_dollars,
        },
        "exceeded_dimensions": exceeded,
        "remaining": remaining,
    })
}

/// Render the `libra://agents/runs/{id}/context` body from the run's context
/// pack.
pub fn render_run_context(run_id: &str, pack: &AgentContextPack) -> Value {
    json!({
        "agent_run_id": run_id,
        "task_id": pack.task_id.0.to_string(),
        "goal": pack.goal,
        "read_scope": pack.read_scope,
        "write_scope": pack.write_scope,
        "source_intent_id": pack.source_intent_id.map(|id| id.to_string()),
    })
}

/// Render the `libra://agents/merge-candidates/{id}` body.
pub fn render_merge_candidate(candidate: &MergeCandidate) -> Value {
    json!({
        "id": candidate.id.0.to_string(),
        "review_state": candidate.review_state,
        "patchset_ids": candidate
            .patchset_ids
            .iter()
            .map(|p| p.0.to_string())
            .collect::<Vec<_>>(),
        "agent_run_ids": candidate
            .agent_run_ids
            .iter()
            .map(|r| r.0.to_string())
            .collect::<Vec<_>>(),
        "review_evidence": candidate
            .review_evidence
            .iter()
            .map(|e| e.0.to_string())
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_run_list_with_and_without_trailing_slash() {
        assert_eq!(
            AgentResourceRequest::parse("libra://agents/runs"),
            Some(AgentResourceRequest::RunList),
        );
        assert_eq!(
            AgentResourceRequest::parse("libra://agents/runs/"),
            Some(AgentResourceRequest::RunList),
        );
    }

    #[test]
    fn parses_run_detail_and_sub_resources() {
        assert_eq!(
            AgentResourceRequest::parse("libra://agents/runs/abc123"),
            Some(AgentResourceRequest::RunDetail {
                run_id: "abc123".to_string()
            }),
        );
        assert_eq!(
            AgentResourceRequest::parse("libra://agents/runs/abc123/permissions"),
            Some(AgentResourceRequest::RunPermissions {
                run_id: "abc123".to_string()
            }),
        );
        assert_eq!(
            AgentResourceRequest::parse("libra://agents/runs/abc123/budget"),
            Some(AgentResourceRequest::RunBudget {
                run_id: "abc123".to_string()
            }),
        );
        assert_eq!(
            AgentResourceRequest::parse("libra://agents/runs/abc123/context"),
            Some(AgentResourceRequest::RunContext {
                run_id: "abc123".to_string()
            }),
        );
    }

    #[test]
    fn parses_merge_candidate() {
        assert_eq!(
            AgentResourceRequest::parse("libra://agents/merge-candidates/cand-1"),
            Some(AgentResourceRequest::MergeCandidate {
                candidate_id: "cand-1".to_string()
            }),
        );
    }

    #[test]
    fn all_six_documented_uris_route() {
        // The exact set from agent.md Step 2.6 (3): every documented URI must
        // parse to a distinct request and none collide.
        let parsed = [
            "libra://agents/runs",
            "libra://agents/runs/ID",
            "libra://agents/runs/ID/permissions",
            "libra://agents/runs/ID/budget",
            "libra://agents/runs/ID/context",
            "libra://agents/merge-candidates/ID",
        ]
        .map(|u| AgentResourceRequest::parse(u).expect("documented URI must parse"));
        // All six are distinct.
        for i in 0..parsed.len() {
            for j in (i + 1)..parsed.len() {
                assert_ne!(parsed[i], parsed[j], "URIs {i} and {j} must not collide");
            }
        }
    }

    #[test]
    fn rejects_non_agent_and_malformed_uris() {
        for uri in [
            "libra://history/latest",              // different family
            "libra://agents/",                     // no sub-resource
            "libra://agents/runs/ID/unknown",      // unknown sub-resource
            "libra://agents/runs/ID/budget/x",     // trailing segment
            "libra://agents/runs//permissions",    // empty run id
            "libra://agents/merge-candidates/",    // missing candidate id
            "libra://agents/merge-candidates/a/b", // multi-segment candidate id
            "libra://agents/unknown",              // unknown agent sub-tree
        ] {
            assert_eq!(
                AgentResourceRequest::parse(uri),
                None,
                "`{uri}` must not parse as an agent resource",
            );
        }
    }

    #[test]
    fn run_id_accessor_matches_variant() {
        assert_eq!(
            AgentResourceRequest::parse("libra://agents/runs/r1")
                .unwrap()
                .run_id(),
            Some("r1"),
        );
        assert_eq!(
            AgentResourceRequest::parse("libra://agents/runs/r1/budget")
                .unwrap()
                .run_id(),
            Some("r1"),
        );
        assert_eq!(
            AgentResourceRequest::RunList.run_id(),
            None,
            "RunList targets no single run",
        );
        assert_eq!(
            AgentResourceRequest::parse("libra://agents/merge-candidates/c1")
                .unwrap()
                .run_id(),
            None,
            "a merge-candidate request targets no run id",
        );
    }

    mod render {
        use uuid::Uuid;

        use super::super::*;
        use crate::internal::ai::agent_run::{
            AgentBudget, AgentContextPack, AgentPermissionProfile, AgentRun, AgentRunId,
            AgentRunStatus, AgentTaskId, MergeCandidate, MergeCandidateId, RunUsage,
        };

        fn run() -> AgentRun {
            AgentRun {
                id: AgentRunId(Uuid::from_u128(1)),
                task_id: AgentTaskId(Uuid::from_u128(2)),
                thread_id: Uuid::from_u128(3),
                provider: "deepseek".to_string(),
                model: "deepseek-chat".to_string(),
                transcript_path: ".libra/sessions/t/agents/r.jsonl".to_string(),
                workspace_path: None,
                status: AgentRunStatus::Running,
            }
        }

        #[test]
        fn run_list_renders_each_run_row() {
            let body = render_run_list(std::slice::from_ref(&run()));
            let runs = body["runs"].as_array().expect("runs array");
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0]["status"], json!("running"));
            assert_eq!(runs[0]["provider"], json!("deepseek"));
            assert_eq!(runs[0]["id"], json!(Uuid::from_u128(1).to_string()));
            // Unmaterialized workspace renders as JSON null.
            assert!(runs[0]["workspace_path"].is_null());
        }

        #[test]
        fn empty_run_list_renders_empty_array() {
            let body = render_run_list(&[]);
            assert_eq!(body["runs"], json!([]));
        }

        #[test]
        fn permissions_view_lists_default_deny_profile() {
            let profile = AgentPermissionProfile::default();
            let body = render_run_permissions("r1", &profile);
            assert_eq!(body["agent_run_id"], json!("r1"));
            assert_eq!(body["allowed_tools"], json!([]));
            assert_eq!(body["may_spawn_sub_agents"], json!(false));
            assert_eq!(body["approval_routing"], json!("layer1_human"));
        }

        #[test]
        fn budget_view_reports_exceeded_dimensions() {
            let budget = AgentBudget {
                max_tokens: Some(10),
                ..AgentBudget::default()
            };
            let usage = RunUsage {
                prompt_tokens: 100,
                ..RunUsage::default()
            };
            let body = render_run_budget("r1", &budget, &usage, 0);
            assert_eq!(body["usage"]["total_tokens"], json!(100));
            assert_eq!(body["exceeded_dimensions"], json!(["token"]));
            // CEX-S2-16 (1) "budget remaining": an over-budget token dimension
            // saturates to zero headroom (never wraps) and is the only enforced
            // dimension reported.
            assert_eq!(
                body["remaining"],
                json!([{ "dimension": "token", "remaining": 0 }]),
            );
        }

        /// The budget view reports per-dimension remaining headroom for an
        /// under-budget run, in `BudgetDimension` declaration order, alongside
        /// an empty `exceeded_dimensions` (CEX-S2-16 (1) "budget remaining").
        #[test]
        fn budget_view_reports_remaining_headroom() {
            let budget = AgentBudget {
                max_tokens: Some(1_000),
                max_source_calls: Some(4),
                ..AgentBudget::default()
            };
            let usage = RunUsage {
                prompt_tokens: 250,
                completion_tokens: 50, // total 300 → 700 left
                ..RunUsage::default()
            };
            let body = render_run_budget("r1", &budget, &usage, 1); // 1 source call → 3 left
            assert_eq!(body["exceeded_dimensions"], json!([]));
            assert_eq!(
                body["remaining"],
                json!([
                    { "dimension": "token", "remaining": 700 },
                    { "dimension": "source_call", "remaining": 3 },
                ]),
            );
        }

        #[test]
        fn context_view_renders_scope_and_goal() {
            let pack = AgentContextPack {
                task_id: AgentTaskId(Uuid::from_u128(7)),
                goal: "fix the bug".to_string(),
                read_scope: vec!["src".to_string()],
                write_scope: vec!["src/a.rs".to_string()],
                source_intent_id: None,
            };
            let body = render_run_context("r1", &pack);
            assert_eq!(body["goal"], json!("fix the bug"));
            assert_eq!(body["read_scope"], json!(["src"]));
            assert_eq!(body["write_scope"], json!(["src/a.rs"]));
            assert!(body["source_intent_id"].is_null());
        }

        #[test]
        fn merge_candidate_view_renders_aggregate_ids_and_state() {
            let candidate = MergeCandidate::new(MergeCandidateId::new(), vec![], vec![]);
            let body = render_merge_candidate(&candidate);
            // S2-INV-07 default surfaces to the observer.
            assert_eq!(body["review_state"], json!("needs_human_review"));
            assert_eq!(body["patchset_ids"], json!([]));
            assert_eq!(body["agent_run_ids"], json!([]));
        }
    }
}
