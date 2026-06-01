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
//! This module owns the **pure** URI → typed-request routing. Resolving a
//! request against persisted run state is the server's job (and lands with the
//! run-persistence path); keeping the parse separate makes the URI grammar
//! exhaustively unit-testable without any storage, and gives the server one
//! place to dispatch from. Parsing performs no I/O.

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
}
