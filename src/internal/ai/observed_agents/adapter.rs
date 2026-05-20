//! Adapter contracts for external-Agent capture.
//!
//! Designed as **one core trait + several optional capability traits** so a new
//! agent can be wired in with as little as `provider_kind`, `provider_name`,
//! `read_transcript`, and `protected_dirs`. Hooks, transcript truncation, and
//! chunking are all opt-in.
//!
//! See `docs/improvement/entire.md` (section 5) for the rationale and the v1
//! adapter matrix (Claude Code + Gemini stable; 5 preview stubs).

use std::path::PathBuf;

use anyhow::Result;

use crate::internal::ai::hooks::{
    lifecycle::{LifecycleEvent, SessionHookEnvelope},
    provider::{ProviderHookCommand, ProviderInstallOptions},
};

/// Identity for one of the externally-hosted agents Libra knows how to capture.
///
/// The variant set is closed because every variant maps to a CLI subcommand
/// (`libra agent enable claude-code`, …) and to a column value in
/// `agent_session.agent_kind`. Adding a new agent requires a v2 plan and a
/// migration touching the CHECK constraint on that column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentKind {
    ClaudeCode,
    Cursor,
    Codex,
    Gemini,
    OpenCode,
    Copilot,
    FactoryAi,
}

impl AgentKind {
    /// Snake-case identifier used as the `agent_session.agent_kind` value and
    /// in log lines. Stable across releases — downstream tooling joins on it.
    pub const fn as_db_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude_code",
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::OpenCode => "opencode",
            Self::Copilot => "copilot",
            Self::FactoryAi => "factory_ai",
        }
    }

    /// Inverse of [`as_db_str`](Self::as_db_str): parse a snake_case
    /// database tag back into the enum. Returns `None` for unknown
    /// input — the database surface uses strict snake_case only (no
    /// hyphens, no case-insensitive matching), so downstream readers
    /// cannot accidentally accept a CLI slug or a typo here. Mirrors
    /// the `FinalDecisionVerdict::from_variant_name` /
    /// `ProjectionStaleReason::from_variant_name` pattern (v0.17.659+).
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "claude_code" => Some(Self::ClaudeCode),
            "cursor" => Some(Self::Cursor),
            "codex" => Some(Self::Codex),
            "gemini" => Some(Self::Gemini),
            "opencode" => Some(Self::OpenCode),
            "copilot" => Some(Self::Copilot),
            "factory_ai" => Some(Self::FactoryAi),
            _ => None,
        }
    }

    /// Slug used on the CLI (`libra agent enable <slug>`). Hyphenated rather
    /// than snake_case to match the convention of other Libra subcommands.
    pub const fn as_cli_slug(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::OpenCode => "opencode",
            Self::Copilot => "copilot",
            Self::FactoryAi => "factory-ai",
        }
    }

    /// Parse a CLI slug back into a kind. Accepts both hyphen and underscore
    /// forms so users can paste either style. Returns `None` if the input
    /// isn't a recognised agent.
    pub fn from_cli_slug(slug: &str) -> Option<Self> {
        match slug {
            "claude-code" | "claude_code" | "claude" => Some(Self::ClaudeCode),
            "cursor" => Some(Self::Cursor),
            "codex" => Some(Self::Codex),
            "gemini" => Some(Self::Gemini),
            "opencode" | "open-code" => Some(Self::OpenCode),
            "copilot" | "github-copilot" => Some(Self::Copilot),
            "factory-ai" | "factory_ai" | "factory" => Some(Self::FactoryAi),
            _ => None,
        }
    }

    /// All variants in registration order. Useful for `libra agent enable`'s
    /// listing path and tests that want to round-trip every kind.
    pub const fn all() -> &'static [Self] {
        &[
            Self::ClaudeCode,
            Self::Cursor,
            Self::Codex,
            Self::Gemini,
            Self::OpenCode,
            Self::Copilot,
            Self::FactoryAi,
        ]
    }
}

/// Stability tier for an [`AgentKind`].
///
/// `Stable` means the v1 adapter implements `read_transcript` and is wired
/// through `libra agent` end-to-end. `Preview` means the agent is reachable
/// from the CLI but its adapter returns `Err(AgentNotYetImplemented)` for the
/// transcript/hook code paths.
///
/// `Serialize` is derived (lowercase via `#[serde(rename_all = "snake_case")]`)
/// because `libra agent doctor --json` emits this enum verbatim. Renaming
/// either variant changes a public CLI contract — bump the JSON schema if
/// you must.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStability {
    Stable,
    Preview,
}

/// Per-call context handed to [`ObservedAgent::read_transcript`] when the
/// runtime asks an adapter for the latest transcript bytes.
///
/// Kept as a small concrete struct (rather than passing the whole
/// `SessionState`) so adapters do not need to depend on the hook runtime's
/// internals.
#[derive(Debug, Clone)]
pub struct AgentSessionCtx {
    /// `agent_session.session_id`.
    pub session_id: String,
    /// `agent_session.provider_session_id` — the agent's own session id, used
    /// by the adapter to locate the transcript file.
    pub provider_session_id: String,
    /// Working directory the session was started in.
    pub working_dir: PathBuf,
    /// Absolute path to the agent's on-disk transcript file (e.g. Claude
    /// Code's session JSONL). Captured from the SessionStart hook
    /// envelope and persisted on the SessionState; the adapter relies on
    /// this to avoid having to reconstruct provider-specific path
    /// conventions (e.g. `~/.claude/projects/<workdir>/<id>.jsonl`).
    /// `None` when no envelope ever provided one.
    pub transcript_path: Option<PathBuf>,
}

/// Reasons an adapter call can fail.
///
/// Adapters return [`anyhow::Error`] from their methods, but the runtime
/// recognises `AgentError::NotYetImplemented` specifically so it can
/// downgrade the failure to a soft warning rather than an error: preview
/// adapters are expected to surface this. Use [`agent_not_yet_implemented`]
/// to construct the canonical instance.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("adapter for '{0}' is preview-only and not yet implemented")]
    NotYetImplemented(&'static str),
}

/// Convenience constructor for the preview-stub `Err` value. Kept as a free
/// function so callers can write `Err(agent_not_yet_implemented(self))?`
/// without importing the variant explicitly.
pub fn agent_not_yet_implemented(agent: &dyn ObservedAgent) -> AgentError {
    AgentError::NotYetImplemented(agent.provider_name())
}

/// Core trait every observed agent implements.
///
/// Boundary condition: [`Self::read_transcript`] returns the agent's *raw*
/// (un-redacted) bytes. The runtime is responsible for piping them through
/// `redaction::Redactor::redact` before any persistence path consumes them.
pub trait ObservedAgent: Send + Sync {
    fn provider_kind(&self) -> AgentKind;
    fn provider_name(&self) -> &'static str;

    /// Stability tier for this adapter. Defaults to [`AgentStability::Stable`]
    /// — preview stubs override.
    fn stability(&self) -> AgentStability {
        AgentStability::Stable
    }

    /// Read the agent's native transcript bytes. `Ok(None)` means "no
    /// transcript is currently available" (e.g. the session has not produced
    /// any output yet); `Err(...)` means the adapter could not access the
    /// transcript.
    ///
    /// The returned bytes are **not yet redacted** — callers must run them
    /// through [`super::redaction::Redactor`] before persistence.
    fn read_transcript(&self, session: &AgentSessionCtx) -> Result<Option<Vec<u8>>>;

    /// Directories owned by the agent that `rewind` and `clean` must leave
    /// alone (`.claude`, `.gemini`, …). Path elements are matched
    /// case-sensitively against the workspace tree walker.
    fn protected_dirs(&self) -> &'static [&'static str];
}

/// Optional capability: full hook lifecycle support.
///
/// An agent that implements this trait participates in `libra agent enable`
/// (hook installation), the hook ingestion pipeline (`libra agent hooks <name>
/// session-start` etc.), and the dedup machinery. It is purely additive —
/// adapters that don't implement it just don't show up in `libra agent
/// enable`'s listing.
pub trait ObservedAgentHooks: ObservedAgent {
    fn supported_commands(&self) -> &'static [ProviderHookCommand];
    fn parse_hook_event(
        &self,
        hook_event_name: &str,
        envelope: &SessionHookEnvelope,
    ) -> Result<LifecycleEvent>;
    fn dedup_identity_keys(&self) -> &'static [&'static str];
    fn install_hooks(&self, options: &ProviderInstallOptions) -> Result<()>;
    fn uninstall_hooks(&self) -> Result<()>;
    fn hooks_are_installed(&self) -> Result<bool>;
}

/// Optional capability: transcript truncation at a checkpoint boundary.
///
/// Required by `libra agent checkpoint rewind --apply` once Phase 2 lands.
/// V1 adapters do NOT implement this — `rewind --apply` therefore leaves the
/// agent's transcript file untouched and prints a warning, per
/// `docs/improvement/entire.md` section 7.3.
pub trait TranscriptTruncator: ObservedAgent {
    fn truncate_transcript(&self, transcript_data: &[u8], checkpoint_id: &str) -> Result<Vec<u8>>;
}

/// Optional capability: chunking very large transcripts before storage.
///
/// V2 candidate. Listed here so the trait surface is documented; v1 callers
/// don't reach for it because Git packfile delta compression already does the
/// job for the foreseeable size envelope.
pub trait TranscriptChunker: ObservedAgent {
    fn chunk_transcript(&self, content: &[u8], max_size: usize) -> Result<Vec<Vec<u8>>>;
    fn reassemble_transcript(&self, chunks: &[Vec<u8>]) -> Result<Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_round_trip() {
        for kind in AgentKind::all() {
            let slug = kind.as_cli_slug();
            assert_eq!(AgentKind::from_cli_slug(slug), Some(*kind));
        }
    }

    #[test]
    fn agent_kind_accepts_underscore_aliases() {
        assert_eq!(
            AgentKind::from_cli_slug("claude_code"),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(
            AgentKind::from_cli_slug("factory_ai"),
            Some(AgentKind::FactoryAi)
        );
        assert_eq!(
            AgentKind::from_cli_slug("github-copilot"),
            Some(AgentKind::Copilot)
        );
    }

    #[test]
    fn agent_kind_rejects_unknown() {
        assert_eq!(AgentKind::from_cli_slug("not-an-agent"), None);
        assert_eq!(AgentKind::from_cli_slug(""), None);
    }

    #[test]
    fn agent_error_display_pins_not_yet_implemented_template() {
        assert_eq!(
            AgentError::NotYetImplemented("Gemini").to_string(),
            "adapter for 'Gemini' is preview-only and not yet implemented",
        );
    }

    /// `as_db_str` produces stable snake_case identifiers for every
    /// variant. Pin the 7 strings explicitly — downstream tooling
    /// joins on the `agent_session.agent_kind` column, so a rename
    /// would break sessions persisted by older binaries.
    #[test]
    fn agent_kind_as_db_str_pins_seven_snake_case_strings() {
        for (kind, expected) in [
            (AgentKind::ClaudeCode, "claude_code"),
            (AgentKind::Cursor, "cursor"),
            (AgentKind::Codex, "codex"),
            (AgentKind::Gemini, "gemini"),
            (AgentKind::OpenCode, "opencode"),
            (AgentKind::Copilot, "copilot"),
            (AgentKind::FactoryAi, "factory_ai"),
        ] {
            assert_eq!(kind.as_db_str(), expected);
        }
    }

    /// `from_db_str` is the inverse of `as_db_str` — for every variant
    /// `kind`, `AgentKind::from_db_str(kind.as_db_str()) == Some(kind)`.
    /// Sweep via `all()` so a future variant lands round-trip coverage
    /// automatically.
    #[test]
    fn agent_kind_from_db_str_round_trips_every_variant() {
        for kind in AgentKind::all() {
            assert_eq!(
                AgentKind::from_db_str(kind.as_db_str()),
                Some(*kind),
                "round-trip mismatch for {kind:?}",
            );
        }
    }

    /// `from_db_str` accepts ONLY the snake_case wire form — it does
    /// not fall through to CLI-slug aliases. A `agent_session.agent_kind`
    /// row that somehow contains `"claude-code"` (the CLI slug shape)
    /// should be rejected as a schema mismatch, not silently mapped to
    /// `ClaudeCode`. Pinning the rejection here protects the
    /// schema contract so a future "lenient" parser cannot quietly
    /// accept CLI aliases at the db boundary.
    #[test]
    fn agent_kind_from_db_str_rejects_cli_slug_aliases_and_unknowns() {
        // Hyphenated CLI-slug forms must NOT be accepted.
        assert_eq!(AgentKind::from_db_str("claude-code"), None);
        assert_eq!(AgentKind::from_db_str("factory-ai"), None);
        // Aliases accepted by `from_cli_slug` must NOT be accepted here.
        assert_eq!(AgentKind::from_db_str("claude"), None);
        assert_eq!(AgentKind::from_db_str("github-copilot"), None);
        assert_eq!(AgentKind::from_db_str("open-code"), None);
        // Bare unknowns + empty input.
        assert_eq!(AgentKind::from_db_str("unknown"), None);
        assert_eq!(AgentKind::from_db_str(""), None);
    }

    /// `as_cli_slug` produces hyphenated strings (different from DB
    /// form for the two two-word agents). Pin all 7 so a rename can
    /// be reviewed at this gate.
    #[test]
    fn agent_kind_as_cli_slug_pins_seven_hyphenated_strings() {
        for (kind, expected) in [
            (AgentKind::ClaudeCode, "claude-code"),
            (AgentKind::Cursor, "cursor"),
            (AgentKind::Codex, "codex"),
            (AgentKind::Gemini, "gemini"),
            (AgentKind::OpenCode, "opencode"),
            (AgentKind::Copilot, "copilot"),
            (AgentKind::FactoryAi, "factory-ai"),
        ] {
            assert_eq!(kind.as_cli_slug(), expected);
        }
    }

    /// `from_cli_slug` accepts the documented short-form aliases:
    /// `"claude"` → ClaudeCode, `"open-code"` → OpenCode,
    /// `"factory"` → FactoryAi. Pin them all so a refactor that
    /// tightens the matcher gets caught.
    #[test]
    fn agent_kind_from_cli_slug_accepts_short_form_aliases() {
        assert_eq!(
            AgentKind::from_cli_slug("claude"),
            Some(AgentKind::ClaudeCode),
        );
        assert_eq!(
            AgentKind::from_cli_slug("open-code"),
            Some(AgentKind::OpenCode),
        );
        assert_eq!(
            AgentKind::from_cli_slug("factory"),
            Some(AgentKind::FactoryAi),
        );
    }

    /// `AgentKind::all()` returns exactly the 7 documented variants
    /// in registration order. Adding an 8th variant must force a
    /// test update.
    #[test]
    fn agent_kind_all_returns_seven_variants_in_registration_order() {
        let all = AgentKind::all();
        assert_eq!(all.len(), 7);
        assert_eq!(all[0], AgentKind::ClaudeCode);
        assert_eq!(all[6], AgentKind::FactoryAi);

        // All 7 must be distinct via HashSet.
        use std::collections::HashSet;
        let set: HashSet<AgentKind> = all.iter().copied().collect();
        assert_eq!(set.len(), 7);
    }

    /// `AgentStability` serde-serialises as snake_case — the
    /// `libra agent doctor --json` public CLI contract depends on
    /// these strings. Pin both variants.
    #[test]
    fn agent_stability_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&AgentStability::Stable).unwrap(),
            "\"stable\"",
        );
        assert_eq!(
            serde_json::to_string(&AgentStability::Preview).unwrap(),
            "\"preview\"",
        );
    }
}
