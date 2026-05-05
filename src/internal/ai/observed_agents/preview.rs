//! Preview-tier `ObservedAgent` stubs for the five not-yet-stable agents
//! (Cursor, Codex, OpenCode, GitHub Copilot CLI, Factory AI Droid). Each
//! one returns hard-coded metadata, advertises itself as `Preview`, and
//! refuses to read a transcript with the canonical
//! [`AgentError::NotYetImplemented`] error.
//!
//! Per `docs/improvement/entire.md` §5.2 these adapters exist mainly so
//! `libra agent enable` can list every agent the plan recognises without
//! lying about implementation status. The corresponding `HookProvider`
//! integration lands in Phase 4 (or whenever the upstream agent ships hook
//! support); until then `libra agent enable <slug>` skips them with a
//! clearly-marked "preview adapter — landing in phase 3" message that
//! `command::agent::install_or_uninstall` already handles.
//!
//! The five stubs share a common shape — kind, provider name, and protected
//! directories — so they go through one [`PreviewAgent`] generic struct
//! rather than five hand-written near-duplicates. New preview agents drop
//! into [`PreviewSpec::all()`] without any new file.

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

/// All preview specs in stable order. Mirrors the v1 adapter matrix in
/// `docs/improvement/entire.md` §5.2 — Cursor, Codex, OpenCode, Copilot,
/// FactoryAi. The protected_dirs mirror each agent's well-known config
/// directory so a future `clean` / `rewind --apply` won't trample them.
pub static PREVIEW_SPECS: &[PreviewSpec] = &[
    PreviewSpec {
        kind: AgentKind::Cursor,
        provider_name: "cursor",
        protected_dirs: &[".cursor"],
    },
    PreviewSpec {
        kind: AgentKind::Codex,
        provider_name: "codex",
        protected_dirs: &[".codex"],
    },
    PreviewSpec {
        kind: AgentKind::OpenCode,
        provider_name: "opencode",
        protected_dirs: &[".opencode"],
    },
    PreviewSpec {
        kind: AgentKind::Copilot,
        provider_name: "copilot",
        protected_dirs: &[".copilot"],
    },
    PreviewSpec {
        kind: AgentKind::FactoryAi,
        provider_name: "factory_ai",
        protected_dirs: &[".factory"],
    },
];

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

    #[test]
    fn preview_specs_cover_every_preview_kind() {
        // The five agents listed in the §5.2 preview column must all be
        // present, and stable kinds must NOT.
        for kind in AgentKind::all() {
            let is_stable = matches!(kind, AgentKind::ClaudeCode | AgentKind::Gemini);
            assert_eq!(
                preview_spec_for(*kind).is_some(),
                !is_stable,
                "preview coverage mismatch for {kind:?}"
            );
        }
    }

    #[test]
    fn preview_agent_reports_preview_stability() {
        let spec = preview_spec_for(AgentKind::Cursor).unwrap();
        let agent = PreviewAgent(spec);
        assert_eq!(agent.stability(), AgentStability::Preview);
        assert_eq!(agent.provider_kind(), AgentKind::Cursor);
        assert_eq!(agent.provider_name(), "cursor");
        assert_eq!(agent.protected_dirs(), &[".cursor"]);
    }

    #[test]
    fn preview_read_transcript_returns_not_yet_implemented() {
        let spec = preview_spec_for(AgentKind::Codex).unwrap();
        let agent = PreviewAgent(spec);
        let ctx = AgentSessionCtx {
            session_id: "s".to_string(),
            provider_session_id: "p".to_string(),
            working_dir: std::path::PathBuf::from("/tmp"),
            transcript_path: None,
        };
        let err = agent.read_transcript(&ctx).unwrap_err();
        assert!(
            err.to_string().contains("preview-only"),
            "unexpected error: {err}"
        );
    }
}
