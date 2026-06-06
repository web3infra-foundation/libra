//! Gemini [`ObservedAgent`] adapter.
//!
//! Pairs with the existing Gemini `HookProvider` (see
//! `src/internal/ai/hooks/providers/gemini.rs`): the hook layer owns
//! `LifecycleEvent` parsing and install/uninstall of the
//! `~/.gemini/settings.json` shell-hooks, while this adapter owns the
//! `ObservedAgent` surface — transcript ingestion plus the
//! [`ObservedAgent::protected_dirs`] manifest that `rewind` / `clean`
//! consume to leave Gemini's session storage alone.
//!
//! `docs/improvement/entire.md` §16.1 lists `builtin/gemini.rs` as a
//! required v1 file, and `builtin/stable_promoted.rs::promoted_specs_cover_every_v1_preview_kind`
//! asserts that `AgentKind::Gemini` is a "dedicated stable" type rather
//! than a row in `STABLE_PROMOTED_SPECS`. Until v0.17.672 the codebase
//! satisfied the stable-promoted exclusion but never landed the
//! dedicated type the assertion implies — this module closes that gap.
//!
//! `TranscriptTruncator` IS implemented (entire.md §14.4 phase-4 item 1):
//! Gemini's session transcript is a single JSON document
//! `{"sessionId":…,"messages":[{"id":…,"timestamp":"<ISO>","type":"user"|
//! "gemini"|"info",…}]}` (schema mirrored from EntireIO
//! `transcript/compact/gemini.go:17`). On `rewind --apply` the truncator
//! drops every message whose ISO-8601 `timestamp` parses strictly after the
//! checkpoint boundary, keeping messages with missing/unparseable timestamps
//! verbatim — the same conservative policy as the Claude Code JSONL truncator.

use std::{
    fs,
    io::{self},
    path::Path,
};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};

use super::super::adapter::{AgentKind, AgentSessionCtx, ObservedAgent, TranscriptTruncator};

/// Hard cap on how many bytes the transcript reader will pull off disk.
/// Mirrors the Claude Code adapter's 16 MiB ceiling: Gemini transcripts
/// also grow with conversation length but sit well under a few MB in
/// practice. The cap protects against a runaway file or a malformed
/// path that points at a giant binary.
const MAX_TRANSCRIPT_BYTES: u64 = 16 * 1024 * 1024;

/// Stable adapter for Gemini (`AgentKind::Gemini`).
///
/// Held as a unit struct so the registry can hand out a
/// `&'static dyn ObservedAgent` without lifetime gymnastics; identical
/// shape to [`super::claude_code::ClaudeCodeObservedAgent`].
#[derive(Debug, Default, Clone, Copy)]
pub struct GeminiObservedAgent;

impl GeminiObservedAgent {
    pub const fn new() -> Self {
        Self
    }
}

impl ObservedAgent for GeminiObservedAgent {
    fn provider_kind(&self) -> AgentKind {
        AgentKind::Gemini
    }

    fn provider_name(&self) -> &'static str {
        "gemini"
    }

    /// Read the raw transcript bytes from `session.transcript_path`.
    ///
    /// Returns `Ok(None)` when the path is absent (caller hasn't
    /// pointed at a transcript yet) or when the file is missing
    /// (session that never produced output); returns an actionable
    /// error when the file exceeds [`MAX_TRANSCRIPT_BYTES`] or the
    /// `stat`/`read` system calls fail with anything other than
    /// `NotFound`. Empty files round-trip as `Ok(Some(Vec::new()))` so
    /// the caller can distinguish "empty transcript" from "no
    /// transcript path configured".
    ///
    /// The returned bytes are **not yet redacted** — the
    /// `super::super::redaction::Redactor` layer is the only sanctioned
    /// path into persistence storage, per `entire.md` §13 Risk #1.
    fn read_transcript(&self, session: &AgentSessionCtx) -> Result<Option<Vec<u8>>> {
        let Some(path) = session.transcript_path.as_ref() else {
            return Ok(None);
        };
        read_transcript_capped(path, MAX_TRANSCRIPT_BYTES)
    }

    /// `~/.gemini/` holds Gemini's session storage and shell-hooks
    /// config. `rewind` / `clean` must not delete or rewrite these
    /// paths even when the AI worktree strategy says "scrub everything
    /// outside the writable roots".
    fn protected_dirs(&self) -> &'static [&'static str] {
        &[".gemini"]
    }
}

impl TranscriptTruncator for GeminiObservedAgent {
    /// Truncate the Gemini transcript at the checkpoint boundary. `checkpoint_id`
    /// carries the boundary as a serialised RFC-3339 timestamp (the
    /// `rewind --apply` caller resolves `agent_checkpoint.created_at` to that
    /// string), keeping the trait surface free of timestamp types — identical
    /// contract to [`super::claude_code::ClaudeCodeObservedAgent`].
    fn truncate_transcript(&self, transcript_data: &[u8], checkpoint_id: &str) -> Result<Vec<u8>> {
        let boundary: DateTime<Utc> = checkpoint_id.parse().with_context(|| {
            format!(
                "checkpoint boundary '{checkpoint_id}' must be an RFC-3339 timestamp \
                 (caller is responsible for resolving agent_checkpoint.created_at)"
            )
        })?;
        truncate_gemini_messages_after(transcript_data, boundary)
    }
}

/// Parse `transcript_data` as a single Gemini session document and drop every
/// entry in its top-level `messages` array whose ISO-8601 `timestamp` parses
/// strictly after `boundary`. Messages with a missing or unparseable timestamp
/// are kept (conservative — never silently erase a record we don't understand).
///
/// Non-JSON input, or a JSON document without a `messages` array, is returned
/// byte-for-byte unchanged so the truncator is a safe no-op on shapes it does
/// not recognise. All other top-level keys (`sessionId`, …) and per-message
/// fields are preserved.
fn truncate_gemini_messages_after(
    transcript_data: &[u8],
    boundary: DateTime<Utc>,
) -> Result<Vec<u8>> {
    let mut value: serde_json::Value = match serde_json::from_slice(transcript_data) {
        Ok(v) => v,
        // Not a single JSON document (empty, partial, or JSONL) — leave it
        // alone; physical truncation is the user's job once they inspect it.
        Err(_) => return Ok(transcript_data.to_vec()),
    };
    let Some(messages) = value
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(transcript_data.to_vec());
    };
    messages.retain(|msg| {
        match msg
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        {
            // Strictly after the checkpoint — drop.
            Some(ts) => ts <= boundary,
            // Missing or unparseable timestamp — keep verbatim.
            None => true,
        }
    });
    serde_json::to_vec(&value).context("re-serialise truncated gemini transcript")
}

fn read_transcript_capped(path: &Path, max_bytes: u64) -> Result<Option<Vec<u8>>> {
    match fs::metadata(path) {
        Ok(meta) if meta.len() == 0 => Ok(Some(Vec::new())),
        Ok(meta) if meta.len() > max_bytes => Err(anyhow!(
            "transcript at {} exceeds {} byte cap; refusing to load",
            path.display(),
            max_bytes
        )),
        Ok(_) => {
            let bytes =
                fs::read(path).with_context(|| format!("read transcript {}", path.display()))?;
            Ok(Some(bytes))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => {
            Err(anyhow!(err)).with_context(|| format!("stat transcript {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write, path::PathBuf};

    use tempfile::tempdir;

    use super::{
        super::super::adapter::{AgentKind, AgentSessionCtx, AgentStability},
        *,
    };

    fn ctx_with_transcript(path: Option<PathBuf>) -> AgentSessionCtx {
        AgentSessionCtx {
            session_id: "test-session-id".to_string(),
            provider_session_id: "gemini-session-id".to_string(),
            working_dir: PathBuf::from("/tmp/libra-gemini-test"),
            transcript_path: path,
        }
    }

    /// Pin the static identity surface: `AgentKind::Gemini`, the
    /// `gemini` provider name, the `.gemini` protected directory, and
    /// the `Stable` tier (so `is_preview` continues to return false
    /// for this kind even with no preview spec).
    #[test]
    fn gemini_observed_agent_reports_stable_identity_and_protected_dir() {
        let agent = GeminiObservedAgent::new();
        assert_eq!(agent.provider_kind(), AgentKind::Gemini);
        assert_eq!(agent.provider_name(), "gemini");
        assert_eq!(agent.protected_dirs(), &[".gemini"]);
        assert_eq!(agent.stability(), AgentStability::Stable);
    }

    /// `read_transcript` returns `Ok(None)` when no transcript path is
    /// configured on the session context. Mirrors the Claude Code
    /// adapter's contract so the "no transcript yet" branch is the
    /// same shape across stable adapters.
    #[test]
    fn read_transcript_returns_none_when_no_path_configured() {
        let agent = GeminiObservedAgent::new();
        let ctx = ctx_with_transcript(None);
        let result = agent.read_transcript(&ctx).expect("read must not fail");
        assert!(result.is_none());
    }

    /// `read_transcript` returns `Ok(None)` when the configured path
    /// does not exist on disk (session that pointed at a file the
    /// agent never created). Distinct from "empty file" (next test)
    /// because the caller pipeline branches on the `Option`.
    #[test]
    fn read_transcript_returns_none_when_path_missing() {
        let agent = GeminiObservedAgent::new();
        let dir = tempdir().unwrap();
        let path = dir.path().join("never-created.jsonl");
        let ctx = ctx_with_transcript(Some(path));
        let result = agent.read_transcript(&ctx).expect("read must not fail");
        assert!(result.is_none());
    }

    /// `read_transcript` returns `Ok(Some(<bytes>))` for non-empty
    /// files. Empty-file round-trip is exercised separately.
    #[test]
    fn read_transcript_returns_bytes_for_existing_file() {
        let agent = GeminiObservedAgent::new();
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let payload = b"{\"role\":\"user\",\"text\":\"hello\"}\n";
        fs::write(&path, payload).unwrap();
        let ctx = ctx_with_transcript(Some(path));
        let result = agent
            .read_transcript(&ctx)
            .expect("read must succeed")
            .expect("payload must be Some");
        assert_eq!(result, payload);
    }

    /// `read_transcript` round-trips empty files as `Ok(Some(Vec::new()))`
    /// — distinct from the "no path" / "missing path" branches above so
    /// downstream code can tell "agent created an empty transcript" from
    /// "no transcript file is around".
    #[test]
    fn read_transcript_returns_empty_vec_for_empty_file() {
        let agent = GeminiObservedAgent::new();
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        fs::File::create(&path).unwrap().flush().unwrap();
        let ctx = ctx_with_transcript(Some(path));
        let result = agent
            .read_transcript(&ctx)
            .expect("read must succeed")
            .expect("payload must be Some");
        assert!(result.is_empty());
    }

    /// `read_transcript` refuses to load files larger than the
    /// `MAX_TRANSCRIPT_BYTES` cap, surfacing a clear actionable error
    /// rather than silently materialising a huge buffer. Uses the
    /// `read_transcript_capped` free function with a deliberately tiny
    /// cap so the test stays fast and deterministic.
    #[test]
    fn read_transcript_capped_rejects_oversized_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("big.jsonl");
        fs::write(&path, vec![b'x'; 1024]).unwrap();
        let err = read_transcript_capped(&path, 64).expect_err("must reject oversized file");
        let message = format!("{err:#}");
        assert!(
            message.contains("exceeds 64 byte cap"),
            "unexpected error message: {message}",
        );
    }

    // ----- TranscriptTruncator (entire.md §14.4) -----

    const GEMINI_DOC: &str = r#"{"sessionId":"s1","messages":[
        {"id":"m1","timestamp":"2026-05-05T10:00:00Z","type":"user","content":"hello"},
        {"id":"m2","timestamp":"2026-05-05T10:00:05Z","type":"gemini","content":"hi","tokens":{"input":10,"output":5}},
        {"id":"m3","timestamp":"2026-05-05T10:00:10Z","type":"user","content":"bye"}
    ]}"#;

    fn messages(bytes: &[u8]) -> Vec<serde_json::Value> {
        let v: serde_json::Value = serde_json::from_slice(bytes).expect("valid json out");
        v.get("messages")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn truncator_drops_messages_after_boundary() {
        let agent = GeminiObservedAgent::new();
        // Boundary at 10:00:05 — keep m1 and m2, drop m3.
        let out = agent
            .truncate_transcript(GEMINI_DOC.as_bytes(), "2026-05-05T10:00:05Z")
            .expect("truncate must succeed");
        let msgs = messages(&out);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["id"], "m1");
        assert_eq!(msgs[1]["id"], "m2");
    }

    #[test]
    fn truncator_preserves_session_id_and_message_fields() {
        let agent = GeminiObservedAgent::new();
        let out = agent
            .truncate_transcript(GEMINI_DOC.as_bytes(), "2026-05-05T10:00:05Z")
            .expect("truncate must succeed");
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["sessionId"], "s1", "top-level sessionId preserved");
        let msgs = messages(&out);
        // The kept gemini message retains its nested tokens object.
        assert_eq!(msgs[1]["tokens"]["input"], 10);
        assert_eq!(msgs[1]["type"], "gemini");
    }

    #[test]
    fn truncator_keeps_messages_without_parseable_timestamp() {
        let agent = GeminiObservedAgent::new();
        let doc = r#"{"messages":[{"id":"m1","type":"info","content":"no ts"},{"id":"m2","timestamp":"2026-05-05T10:00:10Z","type":"user"}]}"#;
        // Boundary BEFORE m2 — m2 drops, but the timestamp-less m1 stays.
        let out = agent
            .truncate_transcript(doc.as_bytes(), "2026-05-05T09:00:00Z")
            .expect("truncate must succeed");
        let msgs = messages(&out);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["id"], "m1");
    }

    #[test]
    fn truncator_returns_non_json_unchanged() {
        let agent = GeminiObservedAgent::new();
        let garbage = b"not json at all\npartial {";
        let out = agent
            .truncate_transcript(garbage, "2026-05-05T10:00:05Z")
            .expect("non-json must be a no-op");
        assert_eq!(out, garbage);
    }

    #[test]
    fn truncator_returns_unchanged_when_no_messages_key() {
        let agent = GeminiObservedAgent::new();
        let doc = br#"{"sessionId":"s1","other":true}"#;
        let out = agent
            .truncate_transcript(doc, "2026-05-05T10:00:05Z")
            .expect("missing messages key must be a no-op");
        assert_eq!(out, doc);
    }

    #[test]
    fn truncator_empty_input_returns_empty() {
        let agent = GeminiObservedAgent::new();
        let out = agent
            .truncate_transcript(b"", "2026-05-05T10:00:05Z")
            .expect("empty input must not error");
        assert!(out.is_empty());
    }

    #[test]
    fn truncator_rejects_non_rfc3339_boundary() {
        let agent = GeminiObservedAgent::new();
        let err = agent
            .truncate_transcript(GEMINI_DOC.as_bytes(), "not-a-timestamp")
            .expect_err("non-rfc3339 boundary must error");
        assert!(format!("{err:#}").contains("RFC-3339"));
    }
}
