//! Preview-tier `ObservedAgent` stubs.
//!
//! As of Phase 4.4 (entire.md §14.4 item 4), the original five preview
//! adapters (Cursor, Codex, OpenCode, GitHub Copilot CLI, Factory AI
//! Droid) have been **promoted to stable** under
//! [`super::builtin::stable_promoted`]. [`PREVIEW_SPECS`] is therefore
//! empty in the current build, and [`is_preview`] returns `false` for
//! every `AgentKind`.
//!
//! The module is kept around for two reasons:
//! 1. [`PreviewAgent`] / [`PreviewSpec`] remain a useful template for
//!    landing future preview adapters (e.g. a new agent that joins
//!    after the v1 matrix). Future additions append to
//!    [`PREVIEW_SPECS`] without code churn elsewhere.
//! 2. Downstream callers that branch on `is_preview` continue to
//!    compile and behave correctly — the function now always returns
//!    `false`, which means stable-only paths fire for every agent.

use anyhow::Result;

use super::adapter::{
    AgentKind, AgentSessionCtx, AgentStability, ObservedAgent, agent_not_yet_implemented,
};

/// Static description of a preview adapter. The list of these in
/// [`PREVIEW_SPECS`] is the single source of truth — adding a new preview
/// agent is one row.
#[derive(Debug, Clone, Copy)]
pub struct PreviewSpec {
    pub kind: AgentKind,
    pub provider_name: &'static str,
    pub protected_dirs: &'static [&'static str],
}

/// Concrete `ObservedAgent` over a [`PreviewSpec`]. Cheap (zero-sized
/// `&'static`) so the registry can hand out boxed copies without paying
/// allocation per call.
#[derive(Debug, Clone, Copy)]
pub struct PreviewAgent(pub &'static PreviewSpec);

impl ObservedAgent for PreviewAgent {
    fn provider_kind(&self) -> AgentKind {
        self.0.kind
    }
    fn provider_name(&self) -> &'static str {
        self.0.provider_name
    }
    fn stability(&self) -> AgentStability {
        AgentStability::Preview
    }
    fn read_transcript(&self, _session: &AgentSessionCtx) -> Result<Option<Vec<u8>>> {
        Err(agent_not_yet_implemented(self).into())
    }
    fn protected_dirs(&self) -> &'static [&'static str] {
        self.0.protected_dirs
    }
}

/// All preview specs in stable order. Empty after the Phase 4.4
/// promotion — every kind that previously lived here is now a
/// [`super::builtin::stable_promoted::StablePromotedSpec`]. New
/// preview adapters added in future releases append here.
pub static PREVIEW_SPECS: &[PreviewSpec] = &[];

/// Look up a preview spec by `AgentKind`. Returns `None` for the stable
/// agents (`ClaudeCode`, `Gemini`) — those are wired through
/// `HookProvider` and don't need a preview stub.
pub fn preview_spec_for(kind: AgentKind) -> Option<&'static PreviewSpec> {
    PREVIEW_SPECS.iter().find(|spec| spec.kind == kind)
}

/// Returns `true` if the given kind is one of the five preview agents.
pub fn is_preview(kind: AgentKind) -> bool {
    preview_spec_for(kind).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 4.4 acceptance: PREVIEW_SPECS is empty after the promotion
    /// (every previous preview kind moved to
    /// `builtin::stable_promoted::STABLE_PROMOTED_SPECS`). New preview
    /// adapters can land back here without code churn elsewhere.
    #[test]
    fn preview_specs_is_empty_after_phase_4_4_promotion() {
        assert!(
            PREVIEW_SPECS.is_empty(),
            "PREVIEW_SPECS should be empty after Phase 4.4 — found {} entries",
            PREVIEW_SPECS.len()
        );
    }

    #[test]
    fn is_preview_returns_false_for_every_kind_after_promotion() {
        for kind in AgentKind::all() {
            assert!(
                !is_preview(*kind),
                "is_preview({kind:?}) should be false after Phase 4.4 promotion"
            );
        }
    }

    /// `PreviewAgent` itself remains buildable — it's the template for
    /// any future preview adapter that lands after the v1 matrix.
    #[test]
    fn preview_agent_template_is_still_constructible() {
        // Synthesise a fresh spec since PREVIEW_SPECS is empty.
        static FUTURE_SPEC: PreviewSpec = PreviewSpec {
            kind: AgentKind::Cursor, // arbitrary placeholder
            provider_name: "future-preview",
            protected_dirs: &[],
        };
        let agent = PreviewAgent(&FUTURE_SPEC);
        assert_eq!(agent.stability(), AgentStability::Preview);
        assert_eq!(agent.provider_name(), "future-preview");
        let ctx = AgentSessionCtx {
            session_id: "s".to_string(),
            provider_session_id: "p".to_string(),
            working_dir: std::path::PathBuf::from("/tmp"),
            transcript_path: None,
        };
        let err = agent.read_transcript(&ctx).unwrap_err();
        assert!(
            err.to_string().contains("preview-only"),
            "preview adapter still surfaces NotYetImplemented: {err}"
        );
    }
}
