//! Claude Code [`ObservedAgent`] adapter, paired with a
//! [`TranscriptTruncator`] capability so `libra agent checkpoint rewind
//! --apply` can rewrite the on-disk transcript JSONL — phase-4 item 1
//! per `docs/improvement/entire.md` §14.4.
//!
//! Claude Code stores its session as line-delimited JSON. Each line has
//! a top-level `timestamp` field (ISO-8601 / RFC-3339). The truncator
//! drops every line whose timestamp is strictly greater than the
//! checkpoint boundary; lines with non-parseable or missing timestamps
//! are kept (they're typically session-meta records that pre-date the
//! first message, so dropping them would lose context).

use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};

use super::super::adapter::{AgentKind, AgentSessionCtx, ObservedAgent, TranscriptTruncator};

/// Hard cap on how many bytes the transcript reader will pull off disk.
/// Claude Code transcripts grow with conversation length; in practice
/// they sit under a few MB even for long-running sessions, so 16 MB is a
/// generous ceiling that still protects against a runaway file.
const MAX_TRANSCRIPT_BYTES: u64 = 16 * 1024 * 1024;

/// Stable adapter for Claude Code (`AgentKind::ClaudeCode`).
///
/// Held as a unit struct so the `ObservedAgent` registry can return a
/// `&'static dyn ObservedAgent` without lifetime gymnastics.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCodeObservedAgent;

impl ClaudeCodeObservedAgent {
    pub const fn new() -> Self {
        Self
    }
}

impl ObservedAgent for ClaudeCodeObservedAgent {
    fn provider_kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    fn provider_name(&self) -> &'static str {
        "claude-code"
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
        &[".claude"]
    }
}

impl TranscriptTruncator for ClaudeCodeObservedAgent {
    /// Drop every JSONL line whose `timestamp` field is strictly greater
    /// than the checkpoint's RFC-3339 boundary. The `checkpoint_id`
    /// argument carries the boundary as a serialised RFC-3339 timestamp
    /// (e.g. `"2026-05-05T12:34:56Z"`) — callers in
    /// `libra agent checkpoint rewind --apply` resolve
    /// `agent_checkpoint.created_at` (Unix seconds) to that string before
    /// invoking us, keeping the trait surface free of timestamp types.
    fn truncate_transcript(&self, transcript_data: &[u8], checkpoint_id: &str) -> Result<Vec<u8>> {
        let boundary: DateTime<Utc> = checkpoint_id.parse().with_context(|| {
            format!(
                "checkpoint boundary '{checkpoint_id}' must be an RFC-3339 timestamp \
                 (caller is responsible for resolving agent_checkpoint.created_at)"
            )
        })?;
        truncate_jsonl_after(transcript_data, boundary)
    }
}

/// Walk `transcript_data` line-by-line (LF separated), keeping every
/// line whose parsed `timestamp` field is `<= boundary` (or that lacks a
/// parseable timestamp altogether). Non-JSONL bytes — e.g. trailing
/// partial writes — are kept verbatim so the truncator never silently
/// erases a record we don't understand.
fn truncate_jsonl_after(transcript_data: &[u8], boundary: DateTime<Utc>) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(transcript_data.len());
    for line in transcript_data.split_inclusive(|&b| b == b'\n') {
        let trimmed = trim_trailing_newline(line);
        if trimmed.is_empty() {
            // Preserve empty separator lines verbatim — Claude Code
            // doesn't emit them but a hand-edited file might.
            out.extend_from_slice(line);
            continue;
        }
        match serde_json::from_slice::<serde_json::Value>(trimmed) {
            Ok(value) => {
                let ts = value
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok());
                match ts {
                    Some(parsed) if parsed > boundary => {
                        // Strictly after the checkpoint — drop.
                    }
                    _ => out.extend_from_slice(line),
                }
            }
            Err(_) => {
                // Non-JSON line (e.g. partial write at tail). Keep it
                // — physical truncation is the user's job once they've
                // inspected what survived.
                out.extend_from_slice(line);
            }
        }
    }
    Ok(out)
}

fn trim_trailing_newline(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && (line[end - 1] == b'\n' || line[end - 1] == b'\r') {
        end -= 1;
    }
    &line[..end]
}

/// Convert the Unix-second `agent_checkpoint.created_at` into the
/// RFC-3339 boundary string the truncator expects. Public so the
/// `libra agent checkpoint rewind --apply` path can resolve a boundary
/// without re-importing `chrono` glue at the call site.
///
/// Codex round-2 follow-up: a corrupt or out-of-range `created_at`
/// (e.g. negative or beyond the chrono representable range) used to
/// silently fall back to Unix epoch, which would set the truncation
/// boundary to 1970-01-01 and effectively erase the entire transcript
/// the next time `rewind --apply` ran. Now returns `Err` so the
/// caller (`truncate_agent_transcript_for_checkpoint_with_conn`)
/// surfaces a `Failed { reason: ... }` outcome and leaves the
/// transcript alone.
pub fn rfc3339_boundary_for_unix_seconds(created_at: i64) -> Result<String> {
    let parsed = DateTime::<Utc>::from_timestamp(created_at, 0).ok_or_else(|| {
        anyhow!(
            "agent_checkpoint.created_at {created_at} is outside the chrono \
             representable range; cannot derive an RFC-3339 boundary"
        )
    })?;
    Ok(parsed.to_rfc3339())
}

/// Persist `truncated_bytes` back to the original transcript path,
/// preserving file permissions where possible. Writes through a
/// temporary sibling and renames atomically so a partial write cannot
/// corrupt the live transcript that Claude Code may be appending to
/// concurrently.
///
/// Codex round-1 follow-up: closes the read→rename TOCTOU window. The
/// caller passes `expected_size_at_read` — the file size observed when
/// the bytes were first loaded. We re-stat the original immediately
/// before the rename and refuse to swap if the size has changed,
/// signalling to the caller that Claude Code (or another writer)
/// appended after our read. The rewind path then surfaces this as a
/// `Failed` outcome rather than silently dropping the appended bytes.
/// Pass `None` to skip the check (used by tests that don't model a
/// concurrent writer).
pub fn write_truncated_transcript(
    path: &PathBuf,
    truncated_bytes: &[u8],
    expected_size_at_read: Option<u64>,
) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("transcript path has no parent: {}", path.display()))?;
    fs::create_dir_all(dir)
        .with_context(|| format!("create transcript parent dir {}", dir.display()))?;
    let tmp_path = dir.join(format!(
        ".libra-truncate-{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("transcript")
    ));
    // Codex round-3 + round-4 follow-up: open the tmp file with
    // restricted Unix permissions BEFORE writing any bytes, AND use
    // `create_new(true)` so a pre-existing file at that path never
    // gets reused with whatever permissions it had. A reused
    // permissive tmp file (left over from a prior crashed run, or
    // planted by an attacker who can write into the parent dir) would
    // expose the transcript contents during the brief window between
    // open and chmod.
    //
    // If a leftover tmp file blocks creation, remove it once and
    // retry — this is the same idempotency the rewind path expects.
    let mut open_opts = fs::OpenOptions::new();
    open_opts.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode(0o600);
    }
    let open_result = match open_opts.open(&tmp_path) {
        Ok(f) => Ok(f),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            // Stale tmp file from a previously-crashed truncate.
            // Remove and retry; we still create_new on the retry so
            // an attacker racing to plant a new file in the gap will
            // simply lose to our atomic create.
            let _ = fs::remove_file(&tmp_path);
            open_opts.open(&tmp_path)
        }
        Err(err) => Err(err),
    };
    {
        let mut file =
            open_result.with_context(|| format!("create truncate temp {}", tmp_path.display()))?;
        file.write_all(truncated_bytes)
            .with_context(|| format!("write truncated bytes to {}", tmp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("fsync truncate temp {}", tmp_path.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Promote to the original transcript's mode if it was more
        // permissive (group-readable, etc.). Errors propagate — silent
        // swallowing here would leave the tmp file at 0600 and confuse
        // operators who explicitly chmod'd the transcript.
        if let Ok(meta) = fs::metadata(path) {
            let mode = meta.permissions().mode();
            // mode bits are platform-dependent; mask to the file-mode
            // bits to avoid copying inheritance bits.
            let file_mode = mode & 0o7777;
            if file_mode != 0o600 {
                fs::set_permissions(&tmp_path, fs::Permissions::from_mode(file_mode))
                    .with_context(|| {
                        format!(
                            "preserve original transcript mode 0o{file_mode:o} on {}",
                            tmp_path.display()
                        )
                    })?;
            }
        }
    }
    // Concurrent-writer guard: re-stat the original transcript *just*
    // before the atomic rename. If the file grew (or vanished), abort
    // and clean up the tmp file rather than overwrite — a silent
    // overwrite would lose the bytes the agent appended after our
    // read.
    if let Some(expected) = expected_size_at_read {
        match fs::metadata(path) {
            Ok(meta) if meta.len() != expected => {
                let actual = meta.len();
                let _ = fs::remove_file(&tmp_path);
                return Err(anyhow!(
                    "transcript {} grew from {} to {} bytes during truncation \
                     (concurrent writer). Aborted to avoid silently dropping \
                     appended bytes; rerun once the agent is idle.",
                    path.display(),
                    expected,
                    actual
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let _ = fs::remove_file(&tmp_path);
                return Err(anyhow!(
                    "transcript {} disappeared during truncation",
                    path.display()
                ));
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp_path);
                return Err(anyhow!(err))
                    .with_context(|| format!("re-stat transcript {}", path.display()));
            }
        }
    }
    fs::rename(&tmp_path, path)
        .with_context(|| format!("atomic rename {} -> {}", tmp_path.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncator_drops_lines_after_checkpoint_boundary() {
        let transcript = b"\
{\"timestamp\":\"2026-05-05T10:00:00Z\",\"type\":\"user\",\"text\":\"a\"}\n\
{\"timestamp\":\"2026-05-05T10:30:00Z\",\"type\":\"assistant\",\"text\":\"b\"}\n\
{\"timestamp\":\"2026-05-05T11:00:00Z\",\"type\":\"user\",\"text\":\"c\"}\n\
{\"timestamp\":\"2026-05-05T11:30:00Z\",\"type\":\"assistant\",\"text\":\"d\"}\n";
        let agent = ClaudeCodeObservedAgent::new();
        let truncated = agent
            .truncate_transcript(transcript, "2026-05-05T10:30:00Z")
            .unwrap();
        let s = String::from_utf8(truncated).unwrap();
        assert!(s.contains("\"a\""), "first kept");
        assert!(s.contains("\"b\""), "boundary-equal kept");
        assert!(!s.contains("\"c\""), "post-boundary dropped");
        assert!(!s.contains("\"d\""), "post-boundary dropped");
    }

    #[test]
    fn truncator_keeps_lines_with_unparseable_timestamps() {
        // Session-meta line at the very top of a Claude transcript has
        // no `timestamp` field. We must not drop it — it carries
        // session config that downstream tooling needs.
        let transcript = b"\
{\"type\":\"session_meta\",\"version\":\"1.0\"}\n\
{\"timestamp\":\"2026-05-05T11:00:00Z\",\"type\":\"user\"}\n";
        let agent = ClaudeCodeObservedAgent::new();
        let truncated = agent
            .truncate_transcript(transcript, "2026-05-05T10:00:00Z")
            .unwrap();
        let s = String::from_utf8(truncated).unwrap();
        assert!(s.contains("session_meta"), "untimestamped line preserved");
        assert!(!s.contains("2026-05-05T11:00:00Z"), "post-boundary dropped");
    }

    #[test]
    fn truncator_rejects_non_rfc3339_boundary() {
        let agent = ClaudeCodeObservedAgent::new();
        let err = agent
            .truncate_transcript(b"", "not-a-timestamp")
            .unwrap_err();
        assert!(
            err.to_string().contains("RFC-3339 timestamp"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn read_transcript_returns_none_when_path_missing() {
        let agent = ClaudeCodeObservedAgent::new();
        let ctx = AgentSessionCtx {
            session_id: "s".to_string(),
            provider_session_id: "p".to_string(),
            working_dir: PathBuf::from("/tmp"),
            transcript_path: Some(PathBuf::from("/no/such/file.jsonl")),
        };
        let bytes = agent.read_transcript(&ctx).unwrap();
        assert!(bytes.is_none());
    }

    #[test]
    fn read_transcript_returns_none_when_path_unset() {
        let agent = ClaudeCodeObservedAgent::new();
        let ctx = AgentSessionCtx {
            session_id: "s".to_string(),
            provider_session_id: "p".to_string(),
            working_dir: PathBuf::from("/tmp"),
            transcript_path: None,
        };
        let bytes = agent.read_transcript(&ctx).unwrap();
        assert!(bytes.is_none());
    }

    #[test]
    fn read_transcript_returns_bytes_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"{\"x\":1}\n").unwrap();
        let agent = ClaudeCodeObservedAgent::new();
        let ctx = AgentSessionCtx {
            session_id: "s".to_string(),
            provider_session_id: "p".to_string(),
            working_dir: dir.path().to_path_buf(),
            transcript_path: Some(path),
        };
        let bytes = agent.read_transcript(&ctx).unwrap();
        assert_eq!(bytes.unwrap(), b"{\"x\":1}\n");
    }

    #[test]
    fn write_truncated_transcript_atomic_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"old contents\n").unwrap();
        // Pass the original size (matches what the caller would have
        // observed during read) — guard should not trip on a stable
        // file.
        let expected = fs::metadata(&path).unwrap().len();
        write_truncated_transcript(&path, b"new\n", Some(expected)).unwrap();
        let read = fs::read(&path).unwrap();
        assert_eq!(read, b"new\n");
    }

    /// Codex round-1 follow-up: when the original transcript grew
    /// between read and rename (a concurrent Claude Code writer
    /// appending to the file), we must NOT overwrite. The function
    /// returns an error and leaves the file as the writer left it.
    #[test]
    fn write_truncated_transcript_aborts_on_concurrent_growth() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, b"old contents\n").unwrap();
        let expected = fs::metadata(&path).unwrap().len();
        // Simulate a concurrent appender by extending the file before
        // the rename guard fires.
        fs::write(&path, b"old contents\nappended after read\n").unwrap();
        let err = write_truncated_transcript(&path, b"new\n", Some(expected)).unwrap_err();
        assert!(
            err.to_string().contains("concurrent writer"),
            "expected concurrent-writer error, got: {err}"
        );
        // File preserves the appended bytes — we did NOT overwrite.
        let preserved = fs::read(&path).unwrap();
        assert_eq!(preserved, b"old contents\nappended after read\n");
        // Tmp file was cleaned up.
        let leftover = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with(".libra-truncate-"))
            .count();
        assert_eq!(leftover, 0, "tmp file removed on abort");
    }

    #[test]
    fn rfc3339_boundary_round_trips_unix_seconds() {
        let boundary = rfc3339_boundary_for_unix_seconds(1_778_020_200).unwrap();
        let parsed: DateTime<Utc> = boundary.parse().unwrap();
        assert_eq!(parsed.timestamp(), 1_778_020_200);
    }

    /// Codex round-2 follow-up: out-of-range `created_at` must surface
    /// as `Err` rather than silently falling back to Unix epoch (which
    /// would erase the entire transcript at the next `rewind --apply`).
    #[test]
    fn rfc3339_boundary_rejects_out_of_range_unix_seconds() {
        // i64::MIN is well outside chrono's representable range.
        let err = rfc3339_boundary_for_unix_seconds(i64::MIN).unwrap_err();
        assert!(
            err.to_string()
                .contains("outside the chrono representable range"),
            "unexpected: {err}"
        );
    }
}
