//! Phase 4.4 (entire.md §14.4 item 4): promote the five v1-preview
//! adapters (Cursor, Codex, OpenCode, GitHub Copilot CLI, Factory AI
//! Droid) from `AgentStability::Preview` to `AgentStability::Stable`.
//!
//! Each adapter ships a real `read_transcript` that loads bytes from
//! `AgentSessionCtx.transcript_path` (when the hook envelope captured
//! one), capped at the same 16 MB ceiling used by
//! [`super::claude_code::ClaudeCodeObservedAgent`]. Per-agent
//! transcript-format knowledge (line schema, message-uuid pairing,
//! tool_use semantics) is not yet implemented — that's why none of
//! these adapters carry the `TranscriptTruncator` capability. A v2
//! follow-up will add per-agent truncation. The adapter is still
//! useful in the meantime: hook ingestion + restore + `agent session
//! show --extract-transcript` (forthcoming) all rely on
//! `read_transcript`, which is now real.
//!
//! All five share the same shape, so they go through one
//! [`StablePromotedSpec`] table rather than five hand-written
//! near-duplicates.

use std::{fs, io};

use anyhow::{Context, Result, anyhow};

use super::super::adapter::{AgentKind, AgentSessionCtx, AgentStability, ObservedAgent};

const MAX_TRANSCRIPT_BYTES: u64 = 16 * 1024 * 1024;

/// Static description of a Phase 4.4 stable-promoted adapter. Stays
/// `Copy + 'static` so the registry can hand out cheap references.
#[derive(Debug, Clone, Copy)]
pub struct StablePromotedSpec {
    pub kind: AgentKind,
    pub provider_name: &'static str,
    pub protected_dirs: &'static [&'static str],
}

/// Concrete `ObservedAgent` over a [`StablePromotedSpec`]. Reports
/// `AgentStability::Stable` and reads transcript bytes from the
/// session ctx's `transcript_path` slot.
#[derive(Debug, Clone, Copy)]
pub struct StablePromotedAgent(pub &'static StablePromotedSpec);

impl ObservedAgent for StablePromotedAgent {
    fn provider_kind(&self) -> AgentKind {
        self.0.kind
    }
    fn provider_name(&self) -> &'static str {
        self.0.provider_name
    }
    fn stability(&self) -> AgentStability {
        AgentStability::Stable
    }
    fn read_transcript(&self, session: &AgentSessionCtx) -> Result<Option<Vec<u8>>> {
        let Some(path) = session.transcript_path.as_ref() else {
            return Ok(None);
        };
        match fs::metadata(path) {
            Ok(meta) if meta.len() == 0 => Ok(Some(Vec::new())),
            Ok(meta) if meta.len() > MAX_TRANSCRIPT_BYTES => Err(anyhow!(
                "transcript at {} exceeds {} byte cap; refusing to load",
                path.display(),
                MAX_TRANSCRIPT_BYTES
            )),
            Ok(_) => {
                let bytes = fs::read(path)
                    .with_context(|| format!("read transcript {}", path.display()))?;
                Ok(Some(bytes))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => {
                Err(anyhow!(err)).with_context(|| format!("stat transcript {}", path.display()))
            }
        }
    }
    fn protected_dirs(&self) -> &'static [&'static str] {
        self.0.protected_dirs
    }
}

/// Phase 4.4 stable-promoted adapter table. Mirrors the v1 adapter
/// matrix (entire.md §5.2) for the five agents that previously
/// returned `AgentNotYetImplemented`. The `protected_dirs` mirror each
/// agent's well-known config directory so `clean` / `rewind --apply`
/// won't trample them.
pub static STABLE_PROMOTED_SPECS: &[StablePromotedSpec] = &[
    StablePromotedSpec {
        kind: AgentKind::Cursor,
        provider_name: "cursor",
        protected_dirs: &[".cursor"],
    },
    StablePromotedSpec {
        kind: AgentKind::Codex,
        provider_name: "codex",
        protected_dirs: &[".codex"],
    },
    StablePromotedSpec {
        kind: AgentKind::OpenCode,
        provider_name: "opencode",
        protected_dirs: &[".opencode"],
    },
    StablePromotedSpec {
        kind: AgentKind::Copilot,
        provider_name: "copilot",
        protected_dirs: &[".copilot"],
    },
    StablePromotedSpec {
        kind: AgentKind::FactoryAi,
        provider_name: "factory_ai",
        protected_dirs: &[".factory"],
    },
];

/// Lookup a stable-promoted spec by `AgentKind`. Returns `None` for
/// kinds that aren't in the Phase 4.4 promotion set (the two original
/// stable adapters — Claude Code, Gemini — have their own dedicated
/// types with extra capabilities like `TranscriptTruncator`).
pub fn stable_promoted_spec_for(kind: AgentKind) -> Option<&'static StablePromotedSpec> {
    STABLE_PROMOTED_SPECS.iter().find(|spec| spec.kind == kind)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn promoted_specs_cover_every_v1_preview_kind() {
        // The five agents that were `Preview` in Phase 1 must all be
        // present here. The two original stable kinds (ClaudeCode,
        // Gemini) must NOT — they have dedicated structs.
        for kind in AgentKind::all() {
            let is_dedicated_stable = matches!(kind, AgentKind::ClaudeCode | AgentKind::Gemini);
            assert_eq!(
                stable_promoted_spec_for(*kind).is_some(),
                !is_dedicated_stable,
                "promotion coverage mismatch for {kind:?}"
            );
        }
    }

    /// Companion to `promoted_specs_cover_every_v1_preview_kind`. The
    /// prior test asserts ClaudeCode / Gemini are absent from
    /// `STABLE_PROMOTED_SPECS`, implicitly assuming they have dedicated
    /// adapter structs elsewhere. Removing `ClaudeCodeObservedAgent`
    /// or `GeminiObservedAgent` would not fail that test by itself,
    /// so an entire `AgentKind` could end up with no adapter at all.
    ///
    /// Pin the partition directly: instantiate each dedicated struct
    /// and verify it reports the expected `AgentKind`. A future refactor
    /// that drops either dedicated type fails this test rather than
    /// silently leaving the kind unserviced.
    #[test]
    fn dedicated_stable_adapters_exist_and_report_their_kind() {
        use super::super::{ClaudeCodeObservedAgent, GeminiObservedAgent};

        let claude = ClaudeCodeObservedAgent::new();
        assert_eq!(claude.provider_kind(), AgentKind::ClaudeCode);
        assert_eq!(claude.stability(), AgentStability::Stable);

        let gemini = GeminiObservedAgent::new();
        assert_eq!(gemini.provider_kind(), AgentKind::Gemini);
        assert_eq!(gemini.stability(), AgentStability::Stable);
    }

    #[test]
    fn promoted_agent_reports_stable_tier() {
        let spec = stable_promoted_spec_for(AgentKind::Cursor).unwrap();
        let agent = StablePromotedAgent(spec);
        assert_eq!(agent.stability(), AgentStability::Stable);
        assert_eq!(agent.provider_kind(), AgentKind::Cursor);
        assert_eq!(agent.provider_name(), "cursor");
        assert_eq!(agent.protected_dirs(), &[".cursor"]);
    }

    #[test]
    fn read_transcript_returns_none_when_path_unset() {
        let spec = stable_promoted_spec_for(AgentKind::Codex).unwrap();
        let agent = StablePromotedAgent(spec);
        let ctx = AgentSessionCtx {
            session_id: "s".to_string(),
            provider_session_id: "p".to_string(),
            working_dir: PathBuf::from("/tmp"),
            transcript_path: None,
        };
        assert!(agent.read_transcript(&ctx).unwrap().is_none());
    }

    #[test]
    fn read_transcript_returns_bytes_when_path_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"{\"hello\":1}\n").unwrap();
        let spec = stable_promoted_spec_for(AgentKind::OpenCode).unwrap();
        let agent = StablePromotedAgent(spec);
        let ctx = AgentSessionCtx {
            session_id: "s".to_string(),
            provider_session_id: "p".to_string(),
            working_dir: dir.path().to_path_buf(),
            transcript_path: Some(path),
        };
        let bytes = agent.read_transcript(&ctx).unwrap().expect("Some(bytes)");
        assert_eq!(bytes, b"{\"hello\":1}\n");
    }

    #[test]
    fn read_transcript_returns_none_when_path_missing() {
        let spec = stable_promoted_spec_for(AgentKind::Copilot).unwrap();
        let agent = StablePromotedAgent(spec);
        let ctx = AgentSessionCtx {
            session_id: "s".to_string(),
            provider_session_id: "p".to_string(),
            working_dir: PathBuf::from("/tmp"),
            transcript_path: Some(PathBuf::from("/no/such/file.jsonl")),
        };
        assert!(agent.read_transcript(&ctx).unwrap().is_none());
    }
}
