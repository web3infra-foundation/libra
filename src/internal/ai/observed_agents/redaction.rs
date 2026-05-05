//! Redaction engine and the [`RedactedBytes`] compile-time contract.
//!
//! # Why a newtype?
//!
//! `agent_capture` writes transcript bytes into Git blobs that become part of
//! `refs/libra/agent-traces` and (eventually) sync to R2/D1. If a future
//! refactor accidentally hands raw `&[u8]` to one of those persistence paths,
//! every still-unscanned secret in the bytes leaks into the durable store.
//! The Phase 1 risk table calls this out as P0.
//!
//! `RedactedBytes` is a transparent newtype around `Vec<u8>` that can only be
//! produced inside this module. Persistence functions take `&RedactedBytes`,
//! not `&[u8]`, so the type system enforces the redaction step at every
//! callsite. There is no public `From<Vec<u8>>` impl by design.
//!
//! # Engine
//!
//! V1 ships with a small, conservative rule set covering the common
//! high-confidence formats (AWS / GCP / GitHub / Slack / generic JWT, plus
//! a `postgres://user:pass@…` URI rule). The full `gitleaks`-style rule
//! matrix and PII detection are Phase 3 work — see
//! `docs/improvement/entire.md` section 8.

use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::bytes::Regex;
use serde::Serialize;

/// Bytes that have passed through a [`Redactor`].
///
/// The newtype is *transparent* (the inner `Vec<u8>` is reachable via
/// [`Self::bytes`] / [`Self::into_inner`]) but not *constructible* from
/// arbitrary input — only this module can call [`Self::new_unchecked`].
///
/// Persistence APIs (`observed_agents::checkpoint::write_transcript_blob`, the
/// cloud-sync transcript uploader, `HistoryManager::create_append_commit`'s
/// transcript channel) accept `&RedactedBytes` rather than `&[u8]`. Calling
/// them therefore requires going through [`Redactor::redact`] first.
///
/// # Compile-time contract
///
/// The constructor is `pub(crate)`; downstream callers cannot mint a value
/// without round-tripping through [`Redactor::redact`]. The doctest below
/// pins this — if a future refactor accidentally widens the constructor to
/// `pub`, doctest compilation succeeds and `cargo test` flips red because
/// the `compile_fail` annotation expects failure.
///
/// ```compile_fail
/// use libra::internal::ai::observed_agents::RedactedBytes;
/// // Must NOT compile — `new_unchecked` is `pub(crate)`. If this ever
/// // builds, the contract has been silently widened and every downstream
/// // sink can be fed un-redacted bytes.
/// let _ = RedactedBytes::new_unchecked(vec![0u8]);
/// ```
///
/// ```compile_fail
/// use libra::internal::ai::observed_agents::RedactedBytes;
/// // Must NOT compile — `data` is private.
/// let _ = RedactedBytes { data: vec![0u8] };
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedBytes {
    data: Vec<u8>,
}

impl RedactedBytes {
    /// Construct a `RedactedBytes` from already-redacted input.
    ///
    /// Visibility note: `pub(crate)` rather than `pub` so that only code
    /// inside this crate can build the type, *and* by convention only
    /// [`Redactor::redact`] (and a couple of well-named test helpers below)
    /// invokes it. External crates have no way to bypass redaction.
    pub(crate) fn new_unchecked(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Borrow the redacted byte slice.
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    /// Length of the redacted byte buffer in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// `true` if the redacted buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Consume the wrapper and return the inner bytes.
    pub fn into_inner(self) -> Vec<u8> {
        self.data
    }
}

impl AsRef<[u8]> for RedactedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

/// Single regex-based redaction rule.
#[derive(Debug, Clone)]
pub struct RedactionRule {
    pub id: &'static str,
    pub regex: Regex,
    pub replacement: &'static str,
}

/// Where a rule fired in the input.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RedactionMatch {
    pub rule_id: String,
    pub start: usize,
    pub end: usize,
}

/// Aggregate report returned alongside [`RedactedBytes`] so callers can stamp
/// it onto `agent_session.redaction_report` and the per-checkpoint metadata
/// blob.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct RedactionReport {
    pub matches: Vec<RedactionMatch>,
    pub bytes_scanned: usize,
    pub bytes_redacted: usize,
}

/// Redaction engine. Cheap to clone (the rules are `Arc`-shared) so the
/// runtime can keep one instance per session without paying per-rule rebuild
/// costs.
#[derive(Debug, Clone)]
pub struct Redactor {
    rules: Arc<Vec<RedactionRule>>,
}

impl Redactor {
    /// Build a redactor with the v1 default rule set.
    pub fn new_default() -> Self {
        Self {
            rules: Arc::clone(&DEFAULT_RULES),
        }
    }

    /// Build a redactor with a caller-supplied rule set. Useful for tests.
    pub fn with_rules(rules: Vec<RedactionRule>) -> Self {
        Self {
            rules: Arc::new(rules),
        }
    }

    /// Number of rules registered. Mostly useful for tests / diagnostics.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Walk every rule across `input` and return the redacted bytes plus a
    /// report. Rules are applied in priority order (the order they appear in
    /// [`DEFAULT_RULES`]) and replacements are non-overlapping — once a span
    /// has been replaced, later rules don't re-scan the placeholder.
    pub fn redact(&self, input: &[u8]) -> (RedactedBytes, RedactionReport) {
        let mut output = input.to_vec();
        let mut report = RedactionReport {
            bytes_scanned: input.len(),
            ..Default::default()
        };

        for rule in self.rules.iter() {
            // Re-scan after each rule because earlier replacements can shift
            // byte offsets. The cost is bounded — typical transcripts are
            // <16 MiB and the rule set is small.
            let buffer = output.clone();
            let mut last_end = 0usize;
            let mut new_output = Vec::with_capacity(buffer.len());
            let placeholder = format!("<REDACTED:{}>", rule.id);
            let placeholder_bytes = placeholder.as_bytes();

            for m in rule.regex.find_iter(&buffer) {
                let start = m.start();
                let end = m.end();
                // Skip already-redacted spans so re-running the same rule is
                // idempotent and rules don't recursively eat each other's
                // placeholders.
                if buffer[start..end].starts_with(b"<REDACTED:") {
                    continue;
                }
                new_output.extend_from_slice(&buffer[last_end..start]);
                new_output.extend_from_slice(placeholder_bytes);
                report.matches.push(RedactionMatch {
                    rule_id: rule.id.to_string(),
                    start,
                    end,
                });
                report.bytes_redacted += end - start;
                last_end = end;
            }
            new_output.extend_from_slice(&buffer[last_end..]);
            output = new_output;
        }

        (RedactedBytes::new_unchecked(output), report)
    }
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new_default()
    }
}

/// Marker trait for sinks that may persist redacted bytes.
///
/// Phase 2 wiring (checkpoint commit writer, cloud-sync uploader) implements
/// this trait so that the only entry point that accepts bytes is
/// `accept(&RedactedBytes)`. Together with the `pub(crate)` constructor on
/// [`RedactedBytes`], no `&[u8]` can flow into a sink without first passing
/// through [`Redactor::redact`]. The trait exists as a placeholder in Phase 1
/// so test scaffolding (`tests/redaction_contract_test.rs`) can pin the
/// contract before the real sinks land.
pub trait RedactedSink {
    fn accept(&mut self, redacted: &RedactedBytes);
}

/// Default rule set. Conservative on purpose — false positives on
/// transcripts are very expensive (they make sessions unreadable) so each
/// rule below is anchored to a high-signal prefix.
static DEFAULT_RULES: Lazy<Arc<Vec<RedactionRule>>> = Lazy::new(|| {
    let raw: &[(&'static str, &'static str)] = &[
        // AWS access keys: the `AKIA` / `ASIA` / `AGPA` family.
        (
            "aws-access-key-id",
            r"\b(?:AKIA|ASIA|AGPA|AIDA|AROA|AIPA|ANPA|ANVA|ASCA)[0-9A-Z]{16}\b",
        ),
        // GitHub PATs and OAuth tokens.
        (
            "github-token",
            r"\b(?:ghp|gho|ghu|ghs|ghr)_[0-9A-Za-z]{36,251}\b",
        ),
        // GitHub fine-grained PATs (`github_pat_…`).
        (
            "github-fine-grained-pat",
            r"\bgithub_pat_[0-9A-Za-z_]{20,}\b",
        ),
        // AWS secret access key — appears next to `aws_secret_access_key`,
        // `secret_access_key`, or as the second half of `access_key:secret`
        // pairs. Keyed off the `aws_secret_access_key=…` / similar literal
        // because a bare 40-char base64 string is far too noisy to redact
        // unconditionally.
        (
            "aws-secret-access-key",
            r"(?i)(?:aws[_-]?secret[_-]?access[_-]?key|secret[_-]?access[_-]?key)\s*[:=]\s*[A-Za-z0-9/+=]{40}",
        ),
        // Slack bot/user/legacy tokens.
        ("slack-token", r"\bxox[abprs]-[0-9A-Za-z-]{10,72}\b"),
        // Google API keys.
        ("google-api-key", r"\bAIza[0-9A-Za-z_-]{35}\b"),
        // OpenAI API keys (current "sk-..." family — both legacy and project keys).
        ("openai-api-key", r"\bsk-[0-9A-Za-z_-]{20,}\b"),
        // Generic JWTs (header.payload.signature).
        (
            "jwt",
            r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
        ),
        // Postgres / MySQL connection URIs with embedded credentials.
        (
            "credential-uri",
            r"(?i)\b(?:postgres|postgresql|mysql|mongodb|redis|amqp|amqps)://[^\s/@:]+:[^\s/@]+@[^\s]+",
        ),
        // Private-key PEM headers — match the marker, not the body, so the
        // replacement collapses the entire armoured key into a placeholder.
        (
            "private-key-pem",
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
        ),
    ];

    Arc::new(
        raw.iter()
            .map(|(id, pattern)| RedactionRule {
                id,
                regex: Regex::new(pattern).expect("default redaction pattern must compile"),
                replacement: id,
            })
            .collect(),
    )
});

#[cfg(test)]
mod tests {
    use super::*;

    fn redact_str(redactor: &Redactor, input: &str) -> (String, RedactionReport) {
        let (bytes, report) = redactor.redact(input.as_bytes());
        (
            String::from_utf8(bytes.into_inner()).expect("UTF-8 round-trip"),
            report,
        )
    }

    #[test]
    fn redacts_aws_access_key() {
        let r = Redactor::new_default();
        let (out, report) = redact_str(&r, "AKIAIOSFODNN7EXAMPLE in transcript");
        assert!(out.contains("<REDACTED:aws-access-key-id>"));
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
        assert_eq!(report.matches.len(), 1);
    }

    #[test]
    fn redacts_github_pat() {
        let r = Redactor::new_default();
        let token = "ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let (out, _report) = redact_str(&r, &format!("token={token}"));
        assert!(out.contains("<REDACTED:github-token>"));
    }

    /// CEX-EntireIO Codex review P1 #7: fine-grained GitHub PATs use a
    /// distinct prefix (`github_pat_…`) and must be redacted.
    #[test]
    fn redacts_github_fine_grained_pat() {
        let r = Redactor::new_default();
        // Fine-grained PATs are quite long; 60 alphanumeric chars is well
        // within the lower bound of the live format.
        let token = format!("github_pat_{}", "x".repeat(60));
        let (out, _) = redact_str(&r, &format!("auth={token}"));
        assert!(out.contains("<REDACTED:github-fine-grained-pat>"));
        assert!(!out.contains(&token));
    }

    /// CEX-EntireIO Codex review P1 #7: AWS secret access keys.
    #[test]
    fn redacts_aws_secret_access_key_kv() {
        let r = Redactor::new_default();
        let secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        let (out, _) = redact_str(
            &r,
            &format!("aws_secret_access_key = {secret}\nrest of file"),
        );
        assert!(out.contains("<REDACTED:aws-secret-access-key>"));
        assert!(!out.contains(secret));
    }

    #[test]
    fn redacts_postgres_uri() {
        let r = Redactor::new_default();
        let (out, report) = redact_str(&r, "DSN=postgres://alice:s3cret@db.example.com:5432/app");
        assert!(out.contains("<REDACTED:credential-uri>"));
        assert!(!out.contains("s3cret"));
        assert_eq!(report.matches.len(), 1);
    }

    #[test]
    fn redacts_private_key_block() {
        let r = Redactor::new_default();
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";
        let (out, _) = redact_str(&r, pem);
        assert_eq!(out, "<REDACTED:private-key-pem>");
    }

    #[test]
    fn passes_through_clean_text_without_modification() {
        let r = Redactor::new_default();
        let clean = "this transcript discusses Rust borrow checker and references no secrets";
        let (out, report) = redact_str(&r, clean);
        assert_eq!(out, clean);
        assert!(report.matches.is_empty());
        assert_eq!(report.bytes_redacted, 0);
        assert_eq!(report.bytes_scanned, clean.len());
    }

    #[test]
    fn preserves_byte_count_metadata() {
        let r = Redactor::new_default();
        let (_, report) = redact_str(&r, "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(report.bytes_scanned, "AKIAIOSFODNN7EXAMPLE".len());
        // The 20-char AKIA key gets replaced with the placeholder, but
        // bytes_redacted measures pre-replacement bytes.
        assert!(report.bytes_redacted >= 20);
    }

    /// The newtype is the contract: only this module's redact path can build
    /// a `RedactedBytes`. We exercise that path here to confirm the round-trip
    /// works and the wrapper is transparent.
    #[test]
    fn redacted_bytes_is_transparent() {
        let r = Redactor::new_default();
        let (rb, _) = r.redact(b"hello");
        assert_eq!(rb.as_ref(), b"hello");
        assert_eq!(rb.len(), 5);
        assert!(!rb.is_empty());
        assert_eq!(rb.clone().into_inner(), b"hello");
    }

    #[test]
    fn idempotent_on_already_redacted_input() {
        let r = Redactor::new_default();
        let (first, _) = r.redact(b"AKIAIOSFODNN7EXAMPLE here");
        let (second, second_report) = r.redact(first.bytes());
        assert_eq!(first, second);
        // No new matches on the placeholder.
        assert!(second_report.matches.is_empty());
    }

    #[test]
    fn applies_multiple_rules_in_one_pass() {
        let r = Redactor::new_default();
        let input = "ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa AKIAIOSFODNN7EXAMPLE";
        let (out, report) = redact_str(&r, input);
        assert!(out.contains("<REDACTED:github-token>"));
        assert!(out.contains("<REDACTED:aws-access-key-id>"));
        assert_eq!(report.matches.len(), 2);
    }
}
