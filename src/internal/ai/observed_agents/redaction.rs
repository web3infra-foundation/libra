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

/// Redaction behaviour selected via `[agent.redaction] mode` in
/// `.libra/config` (entire.md §8.4). Defaults to [`Redact`](Self::Redact).
///
/// Note this is a *capture-time* concept distinct from the unrelated
/// `RedactionMode` in `src/internal/publish/` (Worker field-stripping).
///
/// # Safety boundary (entire.md §8.3)
///
/// `mode` governs only the *free-form session-row fields* (`prompt`,
/// `tool_input`) whose redacted form lands in `agent_session.redaction_report`.
/// The full transcript blob written to `refs/libra/agent-traces` is **always**
/// force-scanned with [`RedactionMode::Redact`] regardless of config — a
/// `warn`/`off` setting can never let an un-redacted transcript reach durable
/// storage. The caller is responsible for constructing a force-redact
/// [`Redactor`] for that path; see `hooks::runtime::build_checkpoint_transcript_redacted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RedactionMode {
    /// Replace every match with `<REDACTED:rule_id>` (default, recommended).
    #[default]
    Redact,
    /// Detect and record matches into the [`RedactionReport`] but leave the
    /// bytes unchanged — an audit-only mode.
    Warn,
    /// Skip detection entirely. Documented as not recommended.
    Off,
}

impl RedactionMode {
    /// Parse a config string (`redact` / `warn` / `off`, case-insensitive).
    /// Unknown values fall back to the safe default ([`Redact`](Self::Redact))
    /// so a typo can never silently disable redaction.
    pub fn from_config_str(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "warn" => Self::Warn,
            "off" => Self::Off,
            _ => Self::Redact,
        }
    }
}

/// Opt-in PII detection configuration (entire.md §8.2 P3). Every category
/// defaults to `false` — PII redaction never runs unless explicitly enabled
/// via `[agent.redaction] pii.* = true`, mirroring the EntireIO default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PiiConfig {
    /// Master switch — when `false`, no PII category runs.
    pub enabled: bool,
    pub email: bool,
    pub phone: bool,
}

impl PiiConfig {
    /// `true` when PII detection should run for at least one category.
    fn any_active(&self) -> bool {
        self.enabled && (self.email || self.phone)
    }
}

/// Redaction engine. Cheap to clone (the rules are `Arc`-shared) so the
/// runtime can keep one instance per session without paying per-rule rebuild
/// costs.
#[derive(Debug, Clone)]
pub struct Redactor {
    rules: Arc<Vec<RedactionRule>>,
    mode: RedactionMode,
    pii: PiiConfig,
}

impl Redactor {
    /// Build a redactor with the v1 default rule set and [`RedactionMode::Redact`].
    pub fn new_default() -> Self {
        Self {
            rules: Arc::clone(&DEFAULT_RULES),
            mode: RedactionMode::Redact,
            pii: PiiConfig::default(),
        }
    }

    /// Build a redactor with a caller-supplied rule set. Useful for tests.
    pub fn with_rules(rules: Vec<RedactionRule>) -> Self {
        Self {
            rules: Arc::new(rules),
            mode: RedactionMode::Redact,
            pii: PiiConfig::default(),
        }
    }

    /// Override the redaction mode (config-driven; entire.md §8.4).
    pub fn with_mode(mut self, mode: RedactionMode) -> Self {
        self.mode = mode;
        self
    }

    /// Enable opt-in PII categories (entire.md §8.2 P3).
    pub fn with_pii(mut self, pii: PiiConfig) -> Self {
        self.pii = pii;
        self
    }

    /// The configured redaction mode.
    pub fn mode(&self) -> RedactionMode {
        self.mode
    }

    /// Number of rules registered. Mostly useful for tests / diagnostics.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Walk every detector across `input` and return the redacted bytes plus a
    /// report. Detection runs in layers (entire.md §8.2): the static
    /// prefix/regex rules first (priority order from [`DEFAULT_RULES`]), then
    /// the structural detectors (Shannon-entropy, connection-strings, bounded
    /// credential K/V, and opt-in PII). Replacements are non-overlapping —
    /// once a span has been replaced, later layers don't re-scan the
    /// placeholder.
    ///
    /// [`RedactionMode`] governs the *output bytes*: `Redact` returns the
    /// scrubbed buffer; `Warn` returns the original bytes but still reports
    /// every match (audit mode); `Off` returns the original bytes and an
    /// empty match list. The report's `bytes_redacted` always reflects what
    /// *would* be redacted, so a `warn`-mode row records the would-be volume.
    pub fn redact(&self, input: &[u8]) -> (RedactedBytes, RedactionReport) {
        match self.mode {
            RedactionMode::Off => (
                RedactedBytes::new_unchecked(input.to_vec()),
                RedactionReport {
                    bytes_scanned: input.len(),
                    ..Default::default()
                },
            ),
            RedactionMode::Warn => {
                let (_redacted, report) = self.redact_core(input);
                // Audit mode: report the matches but leave bytes untouched.
                (RedactedBytes::new_unchecked(input.to_vec()), report)
            }
            RedactionMode::Redact => {
                let (redacted, report) = self.redact_core(input);
                (RedactedBytes::new_unchecked(redacted), report)
            }
        }
    }

    /// Unconditional redaction core — always scrubs, ignoring [`Self::mode`].
    /// Used directly by the §8.3 forced-scan transcript-blob path which must
    /// never honour `warn`/`off`.
    fn redact_core(&self, input: &[u8]) -> (Vec<u8>, RedactionReport) {
        let mut output = input.to_vec();
        let mut report = RedactionReport {
            bytes_scanned: input.len(),
            ..Default::default()
        };

        // Layer 1: static prefix/regex rules.
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

        // Layer 2: Shannon-entropy high-entropy tokens (entire.md §8.2 P1).
        apply_detector(&mut output, &mut report, "high-entropy", &ENTROPY_CANDIDATE, |src, cs, ce| {
            refine_entropy_span(src, cs, ce)
        });

        // Layer 3: DB / connection-string detection with placeholder-aware
        // gating (entire.md §8.2 P1). Only runs when the buffer contains `=`.
        if output.contains(&b'=') {
            for rule in CONNECTION_STRING_RULES.iter() {
                apply_detector(&mut output, &mut report, "connection-string", &rule.pattern, |src, cs, ce| {
                    let end = trim_connection_string_end(src, cs, ce);
                    if cs >= end {
                        return None;
                    }
                    let candidate = std::str::from_utf8(&src[cs..end]).ok()?;
                    if (rule.has_secret)(candidate) {
                        Some((cs, end))
                    } else {
                        None
                    }
                });
            }
        }

        // Layer 4: bounded vendor-prefixed credential key/value (§8.2 P2).
        apply_detector_captures(&mut output, &mut report, "credential-kv", &CREDENTIAL_VALUE, |src, caps| {
            let m = caps.get(2)?;
            let (start, end) = unquote_range(src, m.start(), m.end());
            let value = std::str::from_utf8(&src[start..end]).ok()?;
            if has_non_placeholder_password_value(value) {
                Some((start, end))
            } else {
                None
            }
        });

        // Layer 5: opt-in PII (entire.md §8.2 P3) — never runs unless enabled.
        if self.pii.any_active() {
            if self.pii.email {
                apply_detector(&mut output, &mut report, "pii-email", &PII_EMAIL, |src, cs, ce| {
                    let value = std::str::from_utf8(&src[cs..ce]).ok()?;
                    if is_allowlisted_email(value) {
                        None
                    } else {
                        Some((cs, ce))
                    }
                });
            }
            if self.pii.phone {
                apply_detector(&mut output, &mut report, "pii-phone", &PII_PHONE, |_src, cs, ce| {
                    Some((cs, ce))
                });
            }
        }

        (output, report)
    }

    /// JSON-aware redaction (entire.md §8). Parses `input` as a single JSON
    /// value (e.g. a pretty-printed OpenCode export) or, failing that, as
    /// JSONL (one value per line — the Claude/Gemini transcript shape) and
    /// redacts only string *values*, skipping structural fields (`*id`,
    /// `*ids`, `filepath`/`cwd`/`path`, …) and image objects so high-entropy
    /// identifiers and file paths are never corrupted. Lines that don't parse
    /// as JSON fall back to scalar [`Self::redact`]. Honours [`Self::mode`]
    /// exactly like [`Self::redact`].
    pub fn redact_jsonl(&self, input: &[u8]) -> (RedactedBytes, RedactionReport) {
        if self.mode == RedactionMode::Off {
            return (
                RedactedBytes::new_unchecked(input.to_vec()),
                RedactionReport {
                    bytes_scanned: input.len(),
                    ..Default::default()
                },
            );
        }
        let Ok(text) = std::str::from_utf8(input) else {
            // Non-UTF-8 transcript — fall back to raw scalar redaction.
            return self.redact(input);
        };
        let mut report = RedactionReport {
            bytes_scanned: input.len(),
            ..Default::default()
        };
        let redacted_text = self.redact_jsonl_text(text, &mut report);
        let out = if self.mode == RedactionMode::Warn {
            input.to_vec()
        } else {
            redacted_text.into_bytes()
        };
        (RedactedBytes::new_unchecked(out), report)
    }

    /// Core of [`Self::redact_jsonl`]: returns the redacted text and folds
    /// match counts into `report`. Always computes the scrubbed text (the
    /// caller decides whether to emit it based on mode).
    fn redact_jsonl_text(&self, content: &str, report: &mut RedactionReport) -> String {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            // Try the whole content as a single JSON value first (multi-line
            // pretty-printed object/array). serde_json::from_str only succeeds
            // when the *entire* string is one value, so a successful parse here
            // already implies the single-value/EOF condition.
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                let repls = self.collect_json_replacements(&value, report);
                return apply_json_replacements(content, &repls);
            }
        }
        // Fall back to line-by-line JSONL.
        let mut out = String::with_capacity(content.len());
        for (i, line) in content.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            if line.trim().is_empty() {
                out.push_str(line);
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(line.trim()) {
                Ok(value) => {
                    let repls = self.collect_json_replacements(&value, report);
                    out.push_str(&apply_json_replacements(line, &repls));
                }
                Err(_) => {
                    // Non-JSON line — scrub as raw scalar text.
                    let (redacted, line_report) = self.redact_core(line.as_bytes());
                    report.matches.extend(line_report.matches);
                    report.bytes_redacted += line_report.bytes_redacted;
                    out.push_str(&String::from_utf8_lossy(&redacted));
                }
            }
        }
        out
    }

    /// Recursively walk a parsed JSON value, collecting `(key, original,
    /// redacted)` replacements for string values that need scrubbing. Skips
    /// structural fields and image objects (entire.md §8 JSON-aware rules).
    fn collect_json_replacements(
        &self,
        value: &serde_json::Value,
        report: &mut RedactionReport,
    ) -> Vec<JsonReplacement> {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut repls: Vec<JsonReplacement> = Vec::new();
        self.walk_json(value, "", &mut seen, &mut repls, report);
        repls
    }

    fn walk_json(
        &self,
        value: &serde_json::Value,
        key: &str,
        seen: &mut std::collections::HashSet<String>,
        repls: &mut Vec<JsonReplacement>,
        report: &mut RedactionReport,
    ) {
        match value {
            serde_json::Value::Object(map) => {
                if should_skip_json_object(map) {
                    return;
                }
                for (k, child) in map {
                    if should_skip_json_field(k) {
                        continue;
                    }
                    self.walk_json(child, k, seen, repls, report);
                }
            }
            serde_json::Value::Array(items) => {
                for child in items {
                    self.walk_json(child, "", seen, repls, report);
                }
            }
            serde_json::Value::String(s) => {
                let (redacted, value_report) = self.redact_core(s.as_bytes());
                let redacted = String::from_utf8_lossy(&redacted).into_owned();
                if &redacted != s {
                    let dedup = format!("{key}\u{0}{s}");
                    if seen.insert(dedup) {
                        report.matches.extend(value_report.matches);
                        report.bytes_redacted += value_report.bytes_redacted;
                        repls.push(JsonReplacement {
                            key: key.to_string(),
                            original: s.clone(),
                            redacted,
                        });
                    }
                }
            }
            _ => {}
        }
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
        // Anthropic API keys (`sk-ant-…`). MUST come BEFORE the bare
        // `sk-…` OpenAI rule below — the OpenAI pattern is a strict
        // superset of the Anthropic shape (both start `sk-`), so without
        // this earlier rule Anthropic keys would be silently mistagged
        // as `openai-api-key`.
        ("anthropic-api-key", r"\bsk-ant-[0-9A-Za-z_-]{20,}\b"),
        // OpenAI API keys (current "sk-..." family — both legacy and project keys).
        ("openai-api-key", r"\bsk-[0-9A-Za-z_-]{20,}\b"),
        // Generic JWTs (header.payload.signature).
        (
            "jwt",
            r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
        ),
        // Any-scheme connection URI with embedded userinfo password, e.g.
        // `postgres://user:pass@host`, `redis://:pass@host` (empty user), or a
        // custom-scheme `clickhouse://u:p@host`. Generalised from the original
        // fixed-scheme allowlist to match EntireIO's `credentialedURIPattern`
        // (redact.go:26) — the leading scheme is `[a-z][a-z0-9+.-]{1,31}`.
        (
            "credential-uri",
            r#"(?i)\b[a-z][a-z0-9+.-]{1,31}://[^\s/?#@"'`<>:]*:[^\s/?#@"'`<>]+@[^\s"'`<>]+"#,
        ),
        // Google service-account JSON `private_key` field. JSON pretty-
        // printers escape the BEGIN/END markers; this rule catches both the
        // raw and the JSON-escaped forms by anchoring on `\"private_key\"`.
        // MUST come BEFORE the bare `private-key-pem` rule below — the
        // bare PEM rule matches the inner armoured key first otherwise,
        // which produces `<REDACTED:private-key-pem>` for the inner span
        // and never gives this more-specific JSON-aware rule a chance.
        (
            "google-service-account-private-key",
            r#""private_key"\s*:\s*"-----BEGIN [^"]*PRIVATE KEY-----[\s\S]*?-----END [^"]*PRIVATE KEY-----[^"]*""#,
        ),
        // Private-key PEM headers — match the marker, not the body, so the
        // replacement collapses the entire armoured key into a placeholder.
        (
            "private-key-pem",
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
        ),
        // Stripe live + test secrets (rk_, sk_, pk_ — pk is publishable but
        // still high-signal in transcripts).
        (
            "stripe-key",
            r"\b(?:sk|rk|pk)_(?:live|test)_[0-9A-Za-z]{20,}\b",
        ),
        // Twilio Account SID. The `AC`/`SK` prefix plus 32 hex is the
        // documented format. The 32-hex-only Auth Token is intentionally
        // NOT matched here — bare hex strings of that length are too noisy
        // to redact unconditionally without a keyword anchor.
        ("twilio-account-sid", r"\b(?:AC|SK)[0-9a-fA-F]{32}\b"),
        // SendGrid API keys (`SG.<24>.<43>`).
        (
            "sendgrid-api-key",
            r"\bSG\.[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{30,}\b",
        ),
        // Mailgun keys (`key-<hex>` legacy style and the new `key-…`).
        ("mailgun-api-key", r"\bkey-[0-9a-fA-F]{32}\b"),
        // npm automation tokens — both legacy `npm_…` and `npm-…` shapes.
        ("npm-token", r"\bnpm_[0-9A-Za-z]{32,}\b"),
        // PyPI upload tokens (`pypi-…`).
        ("pypi-token", r"\bpypi-[A-Za-z0-9_-]{32,}\b"),
        // GitLab personal/access tokens — `glpat-` prefix is the modern PAT
        // shape; older deploy tokens follow `gldt-`.
        ("gitlab-pat", r"\b(?:glpat|gldt)-[0-9A-Za-z_-]{20,}\b"),
        // Atlassian API tokens commonly start with `ATATT`. Conservative
        // length bound to dodge stray words.
        ("atlassian-api-token", r"\bATATT[0-9A-Za-z_-]{32,}\b"),
        // Cloudflare API tokens — opaque random strings; we anchor off the
        // common `Bearer <40 char>` shape inside `Authorization` headers
        // because a bare token is too noisy to redact unconditionally.
        (
            "cloudflare-bearer",
            r"(?i)Authorization:\s*Bearer\s+[A-Za-z0-9_-]{40}\b",
        ),
        // (`google-service-account-private-key` rule lives earlier, ahead
        // of the bare `private-key-pem` rule, so the JSON wrapper takes
        // precedence over the inner armoured key. Don't add a second copy
        // here.)
        // (Heroku API keys are UUID-shaped — too generic to redact safely
        // without a lookaround. The `regex` crate does not support
        // lookahead/lookbehind, so a naive `(?=heroku)` pattern would
        // poison `DEFAULT_RULES` on first use. Skipped intentionally.)
        // Hugging Face hub tokens (`hf_…`).
        ("huggingface-token", r"\bhf_[A-Za-z0-9]{32,}\b"),
        // DigitalOcean PATs (`dop_v1_…`).
        ("digitalocean-pat", r"\bdop_v1_[A-Za-z0-9]{40,}\b"),
        // Telegram bot tokens — `<numeric>:<35 alnum>`. We don't anchor on
        // a leading `\b` because tokens commonly appear as URL substrings
        // like `https://api.telegram.org/bot<token>/getMe`, where there
        // is no word boundary between `bot` and the digits. The character
        // class `[A-Za-z0-9_-]` is greedy so it stops naturally at the
        // first non-class character (e.g. `/`).
        ("telegram-bot-token", r"\d{8,11}:[A-Za-z0-9_-]{30,}"),
        // Discord bot tokens — three dot-separated base64ish parts.
        (
            "discord-bot-token",
            r"\b[MN][A-Za-z\d]{23}\.[\w-]{6}\.[\w-]{27,}\b",
        ),
        // High-entropy `password` / `secret_key` literals in shell-style
        // env files. Bound is 32+ chars rather than 16 to dodge benign
        // 16-char tokens that show up in test fixtures, UUID-like names,
        // and DB connection-pool defaults — Codex Phase 3 review flagged
        // the previous {16,} as too aggressive.
        (
            "env-password-assignment",
            r#"(?i)(?:password|passwd|secret_key|api_secret)\s*[:=]\s*['"]?[A-Za-z0-9._/+=-]{32,}['"]?"#,
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

// ---------------------------------------------------------------------------
// Layer 2+: structural detectors (entire.md §8.2). These run after the static
// rule loop on the post-rule buffer. Each is a span-collecting pass that skips
// bytes already inside a `<REDACTED:…>` placeholder.
// ---------------------------------------------------------------------------

/// Minimum Shannon entropy (bits/byte) for a candidate token to be treated as
/// a secret. 4.5 matches EntireIO (`redact.go:59`): high enough to skip git
/// SHAs / UUIDs / prose, low enough to catch API keys and base64 tokens.
const ENTROPY_THRESHOLD: f64 = 4.5;

/// Candidate token shape for entropy scanning. `/` and `.` are excluded so
/// file paths aren't swallowed as one token (`redact.go:21`).
static ENTROPY_CANDIDATE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[A-Za-z0-9+_=-]{10,}").expect("entropy candidate regex must compile"));

/// A connection-string detector: a coarse structural regex plus a
/// placeholder-aware predicate that confirms a *real* secret before redacting.
struct ConnectionStringRule {
    pattern: Regex,
    has_secret: fn(&str) -> bool,
}

static CONNECTION_STRING_RULES: Lazy<Vec<ConnectionStringRule>> = Lazy::new(|| {
    vec![
        ConnectionStringRule {
            pattern: Regex::new(r#"(?i)\bjdbc:[^\s"'<>`]+"#).unwrap(),
            has_secret: has_jdbc_password,
        },
        ConnectionStringRule {
            pattern: Regex::new(r#"(?i)\b(?:postgres(?:ql)?|mysql|mariadb|mongodb(?:\+srv)?|redis)://[^\s"'<>`]+"#).unwrap(),
            has_secret: has_database_url_secret,
        },
        ConnectionStringRule {
            pattern: Regex::new(r#"(?i)\b[a-z_][a-z0-9_]*=(?:"[^"]*"|'[^']*'|[^\s"']+)(?:\s+[a-z_][a-z0-9_]*=(?:"[^"]*"|'[^']*'|[^\s"']+)){2,}"#).unwrap(),
            has_secret: has_keyword_dsn_password,
        },
        ConnectionStringRule {
            pattern: Regex::new(r#"(?i)\b[a-z][a-z0-9 _-]*=(?:\{[^}]*\}|"[^"]*"|'[^']*'|[^=;"'\s]+)(?:;[a-z][a-z0-9 _-]*=(?:\{[^}]*\}|"[^"]*"|'[^']*'|[^=;"'\s]+)){2,}"#).unwrap(),
            has_secret: has_semicolon_connection_password,
        },
    ]
});

/// Vendor-prefixed bounded credential K/V (`DB_PASSWORD=…`). Group 1 = key,
/// group 2 = value (the span actually redacted). Mirrors `redact.go:32,42`.
static CREDENTIAL_VALUE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)(?:^|[^A-Za-z0-9])((?:db|database|pg|postgres|postgresql|mysql|mariadb|redis|mongo|mongodb|sqlserver|mssql|jdbc)(?:[_-]+[a-z0-9]+)*[_-]*(?:password|passwd|pwd))\s*=\s*("[^"]*"|'[^']*'|[^\s,;&]+)"#).unwrap()
});

static PASSWORD_ASSIGNMENT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)(?:^|[?&;\s])(?:password|pwd)=("[^"]*"|'[^']*'|[^&;\s"']+)"#).unwrap());
static KEYWORD_HOST: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|\s)host=").unwrap());
static KEYWORD_USER: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|\s)user=").unwrap());
static SEMICOLON_SERVER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|;)\s*(?:server|data source|datasource|addr|address|network address)\s*=").unwrap());
static SEMICOLON_USER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:^|;)\s*(?:user id|userid|user|uid)\s*=").unwrap());

/// Opt-in PII patterns (entire.md §8.2 P3). Conservative shapes; phone
/// requires explicit separators to bound false positives.
static PII_EMAIL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap());
static PII_PHONE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:\+\d{1,3}[ .-]?)?(?:\(\d{2,4}\)|\d{2,4})[ .-]\d{2,4}[ .-]\d{2,4}").unwrap()
});

/// Lowercase values treated as non-secret placeholders (`redact.go:69-82`).
static PLACEHOLDER_SECRET_VALUES: Lazy<std::collections::HashSet<&'static str>> = Lazy::new(|| {
    [
        "redacted",
        "[redacted]",
        "<redacted>",
        "changeme",
        "example",
        "placeholder",
        "your_password",
        "your_db_password",
        "your_secret",
        "secret_here",
    ]
    .into_iter()
    .collect()
});

/// A `(key, original, redacted)` triple for JSON-aware replacement.
struct JsonReplacement {
    key: String,
    original: String,
    redacted: String,
}

/// Byte ranges already occupied by `<REDACTED:…>` placeholders, so structural
/// detectors never re-scan or corrupt a prior replacement.
fn redacted_regions(src: &[u8]) -> Vec<(usize, usize)> {
    const MARKER: &[u8] = b"<REDACTED:";
    let mut regions = Vec::new();
    let mut i = 0;
    while i + MARKER.len() <= src.len() {
        if &src[i..i + MARKER.len()] == MARKER
            && let Some(rel) = src[i..].iter().position(|&b| b == b'>')
        {
            regions.push((i, i + rel + 1));
            i += rel + 1;
            continue;
        }
        i += 1;
    }
    regions
}

fn intersects_any(start: usize, end: usize, regions: &[(usize, usize)]) -> bool {
    regions.iter().any(|&(s, e)| start < e && s < end)
}

/// Generic span-collecting detector pass: find candidates with `regex`, refine
/// each to the actual redaction span (or `None` to skip) via `refine`, then
/// splice in `<REDACTED:label>`.
fn apply_detector(
    buffer: &mut Vec<u8>,
    report: &mut RedactionReport,
    label: &str,
    regex: &Regex,
    refine: impl Fn(&[u8], usize, usize) -> Option<(usize, usize)>,
) {
    let src = std::mem::take(buffer);
    let regions = redacted_regions(&src);
    let placeholder = format!("<REDACTED:{label}>");
    let mut out = Vec::with_capacity(src.len());
    let mut last_end = 0usize;
    for m in regex.find_iter(&src) {
        let Some((start, end)) = refine(&src, m.start(), m.end()) else {
            continue;
        };
        if start < last_end || intersects_any(start, end, &regions) {
            continue;
        }
        out.extend_from_slice(&src[last_end..start]);
        out.extend_from_slice(placeholder.as_bytes());
        report.matches.push(RedactionMatch {
            rule_id: label.to_string(),
            start,
            end,
        });
        report.bytes_redacted += end - start;
        last_end = end;
    }
    out.extend_from_slice(&src[last_end..]);
    *buffer = out;
}

/// Like [`apply_detector`] but the refine closure receives the full
/// [`regex::bytes::Captures`] so it can redact a specific capture group.
fn apply_detector_captures(
    buffer: &mut Vec<u8>,
    report: &mut RedactionReport,
    label: &str,
    regex: &Regex,
    refine: impl Fn(&[u8], &regex::bytes::Captures) -> Option<(usize, usize)>,
) {
    let src = std::mem::take(buffer);
    let regions = redacted_regions(&src);
    let placeholder = format!("<REDACTED:{label}>");
    let mut out = Vec::with_capacity(src.len());
    let mut last_end = 0usize;
    for caps in regex.captures_iter(&src) {
        let Some((start, end)) = refine(&src, &caps) else {
            continue;
        };
        if start < last_end || intersects_any(start, end, &regions) {
            continue;
        }
        out.extend_from_slice(&src[last_end..start]);
        out.extend_from_slice(placeholder.as_bytes());
        report.matches.push(RedactionMatch {
            rule_id: label.to_string(),
            start,
            end,
        });
        report.bytes_redacted += end - start;
        last_end = end;
    }
    out.extend_from_slice(&src[last_end..]);
    *buffer = out;
}

/// Shannon entropy in bits/byte.
fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut freq = [0usize; 256];
    for &b in bytes {
        freq[b as usize] += 1;
    }
    let len = bytes.len() as f64;
    let mut entropy = 0.0;
    for &count in freq.iter() {
        if count > 0 {
            let p = count as f64 / len;
            entropy -= p * p.log2();
        }
    }
    entropy
}

/// Refine an entropy candidate: apply the JSON-escape guard and the entropy
/// threshold (`redact.go:179-191`).
fn refine_entropy_span(src: &[u8], cs: usize, ce: usize) -> Option<(usize, usize)> {
    let mut start = cs;
    if start > 0
        && src[start - 1] == b'\\'
        && matches!(
            src[start],
            b'n' | b't' | b'r' | b'b' | b'f' | b'u' | b'"' | b'\\' | b'/'
        )
    {
        start += 1;
        if ce.saturating_sub(start) < 10 {
            return None;
        }
    }
    if shannon_entropy(&src[start..ce]) > ENTROPY_THRESHOLD {
        Some((start, ce))
    } else {
        None
    }
}

/// Trim trailing sentence punctuation off a connection-string match
/// (`redact.go:293-303`).
fn trim_connection_string_end(src: &[u8], start: usize, mut end: usize) -> usize {
    while end > start {
        match src[end - 1] {
            b'.' | b',' | b';' | b':' | b'!' | b'?' | b')' | b']' => end -= 1,
            _ => return end,
        }
    }
    end
}

/// Strip a single pair of matching surrounding quotes from a byte range.
fn unquote_range(src: &[u8], start: usize, end: usize) -> (usize, usize) {
    if end - start < 2 {
        return (start, end);
    }
    let (first, last) = (src[start], src[end - 1]);
    if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
        (start + 1, end - 1)
    } else {
        (start, end)
    }
}

fn has_non_placeholder_password_value(value: &str) -> bool {
    !value.is_empty() && !is_placeholder_secret_value(value)
}

/// Reports whether `value` is a documentation/masked placeholder rather than a
/// real secret (`redact.go:387-441`).
fn is_placeholder_secret_value(value: &str) -> bool {
    let trimmed = value.trim().trim_matches(['"', '\'']);
    if trimmed.is_empty() {
        return true;
    }
    if is_bracketed_placeholder(trimmed) {
        return true;
    }
    let normalized = trimmed.to_ascii_lowercase();
    if normalized.starts_with("${") && normalized.ends_with('}') {
        return true;
    }
    if PLACEHOLDER_SECRET_VALUES.contains(normalized.as_str()) {
        return true;
    }
    is_repeated_char_placeholder(&normalized)
}

/// `<name>` doc placeholder: lowercase letters joined by `-`/`_`, total len ≥ 5.
fn is_bracketed_placeholder(s: &str) -> bool {
    if s.len() < 5 || !s.starts_with('<') || !s.ends_with('>') {
        return false;
    }
    let inner = &s[1..s.len() - 1];
    let mut chars = inner.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    inner
        .chars()
        .all(|c| c.is_ascii_lowercase() || c == '_' || c == '-')
}

/// A run of a single masking char (`***`, `xxxx`, `----`, `....`), len ≥ 3.
fn is_repeated_char_placeholder(s: &str) -> bool {
    if s.len() < 3 {
        return false;
    }
    let bytes = s.as_bytes();
    match bytes[0] {
        b'*' | b'x' | b'.' | b'-' => {}
        _ => return false,
    }
    bytes.iter().all(|&b| b == bytes[0])
}

fn candidate_has_non_placeholder_password_assignment(candidate: &str) -> bool {
    for caps in PASSWORD_ASSIGNMENT.captures_iter(candidate.as_bytes()) {
        if let Some(m) = caps.get(1) {
            let (s, e) = unquote_range(candidate.as_bytes(), m.start(), m.end());
            if let Some(value) = candidate.get(s..e)
                && has_non_placeholder_password_value(value)
            {
                return true;
            }
        }
    }
    false
}

fn has_jdbc_password(candidate: &str) -> bool {
    candidate.to_ascii_lowercase().starts_with("jdbc:")
        && candidate_has_non_placeholder_password_assignment(candidate)
}

fn has_database_url_secret(candidate: &str) -> bool {
    // Userinfo passwords are already handled by the `credential-uri` rule; here
    // we catch `?password=…` / `?pwd=…` query credentials.
    candidate_has_non_placeholder_password_assignment(candidate)
}

fn has_keyword_dsn_password(candidate: &str) -> bool {
    KEYWORD_HOST.is_match(candidate.as_bytes())
        && KEYWORD_USER.is_match(candidate.as_bytes())
        && candidate_has_non_placeholder_password_assignment(candidate)
}

fn has_semicolon_connection_password(candidate: &str) -> bool {
    SEMICOLON_SERVER.is_match(candidate.as_bytes())
        && SEMICOLON_USER.is_match(candidate.as_bytes())
        && candidate_has_non_placeholder_password_assignment(candidate)
}

fn is_allowlisted_email(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let local = lower.split('@').next().unwrap_or("");
    local.contains("noreply")
        || local.contains("no-reply")
        || local.contains("donotreply")
        || local.contains("do-not-reply")
}

/// JSON keys whose string values are structural, not secrets, and must never
/// be scrubbed (`redact.go:684-703`).
fn should_skip_json_field(key: &str) -> bool {
    if key == "signature" {
        return true;
    }
    let lower = key.to_ascii_lowercase();
    if lower.ends_with("id") || lower.ends_with("ids") {
        return true;
    }
    matches!(
        lower.as_str(),
        "filepath" | "file_path" | "cwd" | "root" | "directory" | "dir" | "path"
    )
}

/// Skip whole objects that carry binary image payloads (`redact.go:706-709`).
fn should_skip_json_object(obj: &serde_json::Map<String, serde_json::Value>) -> bool {
    matches!(obj.get("type"), Some(serde_json::Value::String(t)) if t.starts_with("image") || t == "base64")
}

fn is_json_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// JSON-encode a string without HTML escaping (serde_json default), matching
/// the encoding used in the raw transcript so substring replacement aligns.
fn json_encode_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"))
}

/// Apply collected `(key, original, redacted)` pairs to the raw JSON text,
/// replacing JSON-encoded originals with redacted forms in value position only.
fn apply_json_replacements(s: &str, repls: &[JsonReplacement]) -> String {
    if repls.is_empty() {
        return s.to_string();
    }
    let mut s = s.to_string();
    for r in repls {
        let orig_json = json_encode_string(&r.original);
        let repl_json = json_encode_string(&r.redacted);
        if r.key.is_empty() {
            s = s.replace(&orig_json, &repl_json);
        } else {
            let key_json = json_encode_string(&r.key);
            s = replace_keyed_json_value(&s, &key_json, &orig_json, &repl_json);
        }
    }
    s
}

/// Replace `origJSON` only where it follows `keyJSON : ` (value position), so a
/// key whose redacted text collides with another field's value is left alone
/// (`redact.go:588-625`).
fn replace_keyed_json_value(s: &str, key_json: &str, orig_json: &str, repl_json: &str) -> String {
    if !s.contains(key_json) {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let Some(rel) = s[i..].find(key_json) else {
            out.push_str(&s[i..]);
            break;
        };
        let key_end = i + rel + key_json.len();
        out.push_str(&s[i..key_end]);
        let mut p = key_end;
        while p < bytes.len() && is_json_ws(bytes[p]) {
            p += 1;
        }
        if p >= bytes.len() || bytes[p] != b':' {
            i = key_end;
            continue;
        }
        p += 1;
        while p < bytes.len() && is_json_ws(bytes[p]) {
            p += 1;
        }
        if p + orig_json.len() <= bytes.len() && &s[p..p + orig_json.len()] == orig_json {
            out.push_str(&s[key_end..p]);
            out.push_str(repl_json);
            i = p + orig_json.len();
        } else {
            i = key_end;
        }
    }
    out
}

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

    /// False-positive guard: realistic non-secret developer content
    /// must pass through untouched. The positive tests confirm secrets
    /// ARE redacted; this confirms an over-broad rule doesn't corrupt a
    /// captured transcript by redacting legitimate tokens (a 40-hex git
    /// SHA, a UUID, a `path:line` ref, a semver, and prose that merely
    /// contains the words "sk"/"key"). Each string below is a plausible
    /// false-positive candidate — if a rule regex is ever loosened to
    /// match one, this test flips red.
    #[test]
    fn clean_developer_content_is_not_over_redacted() {
        let r = Redactor::new_default();
        for clean in [
            // 40-char lowercase hex git SHA — must NOT trip a rule.
            // No rule matches a bare hex run: the entropy-bearing rules
            // (aws-secret, etc.) require a `key=`/`secret=` context, and
            // the structural rules (telegram `\d{8,11}:…`, discord, jwt
            // `eyJ…`) require their own distinct shapes that 40 hex
            // chars don't satisfy.
            "commit 9f8e7d6c5b4a39281706f5e4d3c2b1a09f8e7d6c",
            // A UUID (e.g. a thread id) — hyphen-separated hex groups.
            "thread 550e8400-e29b-41d4-a716-446655440000 resumed",
            // A repo-relative path with a line number.
            "see src/internal/ai/observed_agents/redaction.rs:163",
            // A semver / version banner.
            "libra 0.17.1004 release build",
            // Prose containing the substrings "sk" and "key" without a
            // key SHAPE (no `sk-`+20chars, no `AIza`, no `xox…`).
            "the sk module exports a key helper for the task",
        ] {
            let (out, report) = redact_str(&r, clean);
            assert_eq!(out, clean, "clean content must pass through unchanged");
            assert!(
                report.matches.is_empty(),
                "clean content `{clean}` must not match any redaction rule, got {:?}",
                report.matches,
            );
        }
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

    // ----- Layer 2: Shannon entropy (entire.md §8.2 P1) -----

    #[test]
    fn entropy_redacts_high_entropy_token() {
        let r = Redactor::new_default();
        // A 44-char random base64-ish token: entropy well above 4.5.
        let token = "Zk9Qx2Lm7Wp4Rt8Yn1Bv6Cd3Fg5Hj0Ks9Ld2Mq8Nr4Tw";
        let (out, report) = redact_str(&r, &format!("blob {token} end"));
        assert!(
            out.contains("<REDACTED:high-entropy>"),
            "high-entropy token must be redacted, got `{out}`"
        );
        assert!(report.matches.iter().any(|m| m.rule_id == "high-entropy"));
    }

    #[test]
    fn entropy_leaves_git_sha_and_uuid_alone() {
        let r = Redactor::new_default();
        for clean in [
            "commit 9f8e7d6c5b4a39281706f5e4d3c2b1a09f8e7d6c done",
            "thread 550e8400-e29b-41d4-a716-446655440000 resumed",
        ] {
            let (out, _) = redact_str(&r, clean);
            assert_eq!(out, clean, "low-entropy id must pass through: `{clean}`");
        }
    }

    #[test]
    fn entropy_json_escape_guard_keeps_escape_valid() {
        // `\n` followed by a token must not be eaten such that `\R` appears.
        let r = Redactor::new_default();
        let (out, _) = redact_str(&r, r#"{"text":"controller.go\nmodelHandlerFactory"}"#);
        assert!(!out.contains(r"\R"), "must not create an invalid \\R escape: `{out}`");
    }

    // ----- Layer 2b: generalised credential URI -----

    #[test]
    fn redacts_generic_scheme_credential_uri() {
        let r = Redactor::new_default();
        for uri in [
            "redis://:s3cretPass@cache.host:6379/0",
            "clickhouse://admin:Hunter2Pass@db.internal:9000",
        ] {
            let (out, _) = redact_str(&r, &format!("conn {uri} ok"));
            assert!(
                out.contains("<REDACTED:credential-uri>"),
                "credential URI must redact: `{uri}` -> `{out}`"
            );
        }
    }

    // ----- Layer 3: DB connection strings -----

    #[test]
    fn redacts_jdbc_with_real_password() {
        let r = Redactor::new_default();
        let (out, _) = redact_str(
            &r,
            "url=jdbc:postgresql://h:5432/db?user=app&password=S3cretPg99 next",
        );
        assert!(
            out.contains("<REDACTED:connection-string>") || out.contains("<REDACTED:"),
            "jdbc with real password must redact: `{out}`"
        );
    }

    #[test]
    fn keeps_jdbc_with_placeholder_password() {
        let r = Redactor::new_default();
        let input = "jdbc:postgresql://h/db?user=app&password=<password>";
        let (out, _) = redact_str(&r, input);
        assert!(
            !out.contains("<REDACTED:connection-string>"),
            "placeholder password must not trip the connection-string detector: `{out}`"
        );
    }

    // ----- Layer 4: bounded vendor-prefixed credential K/V -----

    #[test]
    fn redacts_short_vendor_prefixed_credential() {
        let r = Redactor::new_default();
        let (out, report) = redact_str(&r, "DB_PASSWORD=hunter2");
        assert!(
            out.contains("<REDACTED:credential-kv>"),
            "short DB_PASSWORD value must redact via vendor-prefix rule: `{out}`"
        );
        assert!(report.matches.iter().any(|m| m.rule_id == "credential-kv"));
    }

    #[test]
    fn keeps_non_vendor_password_word() {
        let r = Redactor::new_default();
        // `mydbpassword` has no non-alnum boundary before the vendor shape and
        // a short value, so neither the K/V nor env rules should fire.
        let (out, _) = redact_str(&r, "mydbpassword=x");
        assert_eq!(out, "mydbpassword=x");
    }

    #[test]
    fn keeps_placeholder_credential_value() {
        let r = Redactor::new_default();
        let (out, _) = redact_str(&r, "DB_PASSWORD=changeme");
        assert_eq!(out, "DB_PASSWORD=changeme", "placeholder value must not redact");
    }

    #[test]
    fn placeholder_whitelist_classifies_values() {
        for v in ["${VAR}", "<password>", "***", "xxxx", "changeme", "", "REDACTED"] {
            assert!(is_placeholder_secret_value(v), "`{v}` must be a placeholder");
        }
        for v in ["Hunter2Real", "s3cret-value", "abc123xyz"] {
            assert!(!is_placeholder_secret_value(v), "`{v}` must NOT be a placeholder");
        }
    }

    // ----- JSON-aware redaction (entire.md §8) -----

    #[test]
    fn jsonl_skips_id_and_path_fields_but_redacts_command() {
        let r = Redactor::new_default();
        // tool_use_id is high-entropy but must survive; file_path must survive;
        // the command carrying an AWS key must be redacted.
        let line = r#"{"tool_use_id":"Zk9Qx2Lm7Wp4Rt8Yn1Bv6Cd3Fg5Hj0Ks","file_path":"/a/b/Zk9Qx2Lm7Wp4Rt8.rs","command":"export AWS=AKIAIOSFODNN7EXAMPLE"}"#;
        let (rb, _) = r.redact_jsonl(line.as_bytes());
        let out = String::from_utf8(rb.into_inner()).unwrap();
        assert!(out.contains("Zk9Qx2Lm7Wp4Rt8Yn1Bv6Cd3Fg5Hj0Ks"), "tool_use_id preserved");
        assert!(out.contains("/a/b/Zk9Qx2Lm7Wp4Rt8.rs"), "file_path preserved");
        assert!(out.contains("<REDACTED:aws-access-key-id>"), "command key redacted: `{out}`");
    }

    #[test]
    fn jsonl_skips_image_objects() {
        let r = Redactor::new_default();
        let doc = r#"{"type":"image","source":{"data":"Zk9Qx2Lm7Wp4Rt8Yn1Bv6Cd3Fg5Hj0KsLd2Mq8Nr4Tw"}}"#;
        let (rb, _) = r.redact_jsonl(doc.as_bytes());
        let out = String::from_utf8(rb.into_inner()).unwrap();
        assert_eq!(out, doc, "image object must pass through untouched");
    }

    #[test]
    fn jsonl_falls_back_to_scalar_for_non_json_line() {
        let r = Redactor::new_default();
        let line = "plain log AKIAIOSFODNN7EXAMPLE trailing";
        let (rb, _) = r.redact_jsonl(line.as_bytes());
        let out = String::from_utf8(rb.into_inner()).unwrap();
        assert!(out.contains("<REDACTED:aws-access-key-id>"), "non-json line still scrubbed: `{out}`");
    }

    // ----- PII opt-in (entire.md §8.2 P3) -----

    #[test]
    fn pii_off_by_default() {
        let r = Redactor::new_default();
        let (out, _) = redact_str(&r, "contact alice@example.com or +1 415-555-0199");
        assert!(out.contains("alice@example.com"), "email must survive when PII off");
    }

    #[test]
    fn pii_redacts_when_enabled() {
        let r = Redactor::new_default().with_pii(PiiConfig {
            enabled: true,
            email: true,
            phone: false,
        });
        let (out, report) = redact_str(&r, "contact alice@example.com please");
        assert!(out.contains("<REDACTED:pii-email>"), "email redacted when enabled: `{out}`");
        assert!(report.matches.iter().any(|m| m.rule_id == "pii-email"));
    }

    #[test]
    fn pii_email_allowlist_skips_noreply() {
        let r = Redactor::new_default().with_pii(PiiConfig {
            enabled: true,
            email: true,
            phone: false,
        });
        let (out, _) = redact_str(&r, "from noreply@example.com here");
        assert!(out.contains("noreply@example.com"), "allowlisted email survives: `{out}`");
    }

    // ----- Config mode (entire.md §8.4) -----

    #[test]
    fn mode_warn_reports_but_does_not_replace() {
        let r = Redactor::new_default().with_mode(RedactionMode::Warn);
        let (rb, report) = r.redact(b"key AKIAIOSFODNN7EXAMPLE end");
        assert_eq!(rb.bytes(), b"key AKIAIOSFODNN7EXAMPLE end", "warn leaves bytes intact");
        assert!(!report.matches.is_empty(), "warn still records matches");
        assert!(report.bytes_redacted >= 20, "warn records would-be redacted volume");
    }

    #[test]
    fn mode_off_skips_detection() {
        let r = Redactor::new_default().with_mode(RedactionMode::Off);
        let (rb, report) = r.redact(b"key AKIAIOSFODNN7EXAMPLE end");
        assert_eq!(rb.bytes(), b"key AKIAIOSFODNN7EXAMPLE end");
        assert!(report.matches.is_empty(), "off records nothing");
        assert_eq!(report.bytes_redacted, 0);
    }

    #[test]
    fn mode_from_config_str_defaults_safe() {
        assert_eq!(RedactionMode::from_config_str("warn"), RedactionMode::Warn);
        assert_eq!(RedactionMode::from_config_str("OFF"), RedactionMode::Off);
        assert_eq!(RedactionMode::from_config_str("redact"), RedactionMode::Redact);
        // Unknown / typo falls back to the safe default.
        assert_eq!(RedactionMode::from_config_str("scrub"), RedactionMode::Redact);
        assert_eq!(RedactionMode::from_config_str(""), RedactionMode::Redact);
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

    // ── Phase 3.2 expanded rules ─────────────────────────────────────────

    #[test]
    fn redacts_anthropic_api_key_with_correct_tag() {
        // Anthropic keys share the `sk-` prefix with OpenAI keys, so the
        // more-specific `sk-ant-…` rule must fire first to give the right
        // provider tag. If this regresses (rule order swapped or the
        // `anthropic-api-key` rule is removed), the placeholder would be
        // `<REDACTED:openai-api-key>` and downstream provenance would lose
        // the Anthropic attribution.
        let r = Redactor::new_default();
        let key = format!("sk-ant-{}", "a".repeat(40));
        let (out, report) = redact_str(&r, &format!("ANTHROPIC_API_KEY={key}"));
        assert!(out.contains("<REDACTED:anthropic-api-key>"));
        assert!(!out.contains("<REDACTED:openai-api-key>"));
        assert!(!out.contains(&key));
        assert!(
            report
                .matches
                .iter()
                .any(|m| m.rule_id == "anthropic-api-key")
        );
    }

    #[test]
    fn redacts_stripe_secret_key() {
        let r = Redactor::new_default();
        // Composed at runtime to dodge GitHub's secret-scanning push
        // protection — the literal `sk_live_<24+ alphanumeric>` shape
        // is flagged by Stripe's pattern even when the value is fake.
        let key = format!("sk_live_{}", "a".repeat(24));
        let (out, _) = redact_str(&r, &format!("STRIPE={key}"));
        assert!(out.contains("<REDACTED:stripe-key>"));
        assert!(!out.contains(&key));
    }

    #[test]
    fn redacts_stripe_test_key() {
        let r = Redactor::new_default();
        let key = format!("sk_test_{}", "b".repeat(24));
        let (out, _) = redact_str(&r, &key);
        assert!(out.contains("<REDACTED:stripe-key>"));
    }

    #[test]
    fn redacts_twilio_account_sid() {
        let r = Redactor::new_default();
        // Fixture is composed at runtime so the literal `AC<32 hex>`
        // string never appears in source — GitHub's secret-scanning push
        // protection flags both AC- and SK-prefixed Twilio shapes
        // verbatim regardless of whether the digits are real.
        let sid = format!("AC{}", "0123456789abcdef".repeat(2));
        let (out, _) = redact_str(&r, &format!("twilio={sid}"));
        assert!(out.contains("<REDACTED:twilio-account-sid>"));
        assert!(!out.contains(&sid));
    }

    #[test]
    fn redacts_sendgrid_key() {
        let r = Redactor::new_default();
        let key = "SG.aaaaaaaaaaaaaaaaaaaaaa.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let (out, _) = redact_str(&r, key);
        assert!(out.contains("<REDACTED:sendgrid-api-key>"));
    }

    #[test]
    fn redacts_npm_token() {
        let r = Redactor::new_default();
        let token = "npm_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let (out, _) = redact_str(&r, &format!("NPM_TOKEN={token}"));
        assert!(out.contains("<REDACTED:npm-token>"));
    }

    #[test]
    fn redacts_gitlab_pat() {
        let r = Redactor::new_default();
        let token = "glpat-aaaaaaaaaaaaaaaaaaaa";
        let (out, _) = redact_str(&r, &format!("GITLAB_TOKEN={token}"));
        assert!(out.contains("<REDACTED:gitlab-pat>"));
    }

    #[test]
    fn redacts_huggingface_token() {
        let r = Redactor::new_default();
        let token = "hf_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let (out, _) = redact_str(&r, token);
        assert!(out.contains("<REDACTED:huggingface-token>"));
    }

    #[test]
    fn redacts_digitalocean_pat() {
        let r = Redactor::new_default();
        let token = "dop_v1_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let (out, _) = redact_str(&r, token);
        assert!(out.contains("<REDACTED:digitalocean-pat>"));
    }

    #[test]
    fn redacts_env_password_assignment() {
        let r = Redactor::new_default();
        // Value is 36 chars, above the 32-char floor.
        let secret = "correcthorsebatterystaple_42abcdefAB";
        assert!(secret.len() >= 32);
        let (out, _) = redact_str(&r, &format!("DB_PASSWORD={secret}"));
        assert!(out.contains("<REDACTED:env-password-assignment>"));
        assert!(!out.contains(secret));
    }

    /// Regression for the Phase 3 threshold tightening: a benign 16-char
    /// value next to a `password=` keyword must NOT be redacted under the
    /// new {32,} lower bound. This is the false-positive class Codex
    /// flagged.
    #[test]
    fn does_not_redact_short_env_password_values() {
        let r = Redactor::new_default();
        let benign = "password=changeme12345678";
        assert_eq!(benign.len() - "password=".len(), 16);
        let (out, _) = redact_str(&r, benign);
        assert!(
            !out.contains("<REDACTED:env-password-assignment>"),
            "expected benign 16-char value to round-trip; got {out}"
        );
    }

    #[test]
    fn redacts_atlassian_api_token() {
        let r = Redactor::new_default();
        let token = "ATATT3xFfGF0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let (out, _) = redact_str(&r, token);
        assert!(out.contains("<REDACTED:atlassian-api-token>"));
    }

    #[test]
    fn redacts_pypi_token() {
        let r = Redactor::new_default();
        let token = "pypi-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let (out, _) = redact_str(&r, token);
        assert!(out.contains("<REDACTED:pypi-token>"));
    }

    #[test]
    fn redacts_mailgun_api_key() {
        let r = Redactor::new_default();
        // Composed at runtime so the literal `key-<32 hex>` shape never
        // appears in source — same reason as the Twilio fixture.
        let key = format!("key-{}", "0123456789abcdef".repeat(2));
        let (out, _) = redact_str(&r, &format!("MAILGUN={key}"));
        assert!(out.contains("<REDACTED:mailgun-api-key>"));
        assert!(!out.contains(&key));
    }

    #[test]
    fn redacts_cloudflare_bearer() {
        let r = Redactor::new_default();
        // 40 alphanumeric chars after `Bearer ` is the documented shape.
        let token = "abcdefghijklmnopqrstuvwxyz0123456789ABCD";
        assert_eq!(token.len(), 40);
        let header = format!("Authorization: Bearer {token}\n");
        let (out, _) = redact_str(&r, &header);
        assert!(out.contains("<REDACTED:cloudflare-bearer>"));
        assert!(!out.contains(token));
    }

    #[test]
    fn redacts_google_service_account_private_key() {
        let r = Redactor::new_default();
        // Compose the JSON at runtime so the literal PKCS#8 prefix (which
        // some secret scanners use as a heuristic) never appears in
        // source. The fixture body is just `xxxx…` — enough to satisfy
        // the regex's `[\s\S]*?` between the BEGIN/END markers.
        let body = "x".repeat(40);
        let begin = "-----BEGIN PRIVATE KEY-----";
        let end = "-----END PRIVATE KEY-----";
        let json = format!(
            r#"{{"type":"service_account","private_key":"{begin}\n{body}\n{end}\n","client_email":"x@y.iam.gserviceaccount.com"}}"#
        );
        let (out, _) = redact_str(&r, &json);
        assert!(
            out.contains("<REDACTED:google-service-account-private-key>"),
            "expected service-account redaction; got {out}"
        );
        assert!(!out.contains(&body));
        // The non-private-key fields stay around — only the key itself is
        // collapsed to the placeholder.
        assert!(out.contains("\"type\":\"service_account\""));
        assert!(out.contains("\"client_email\""));
    }

    #[test]
    fn redacts_telegram_bot_token() {
        let r = Redactor::new_default();
        let token = "1234567890:AAEhBP0av28aaaaaaaaaaaaaaaaaaaaaaaa";
        let (out, _) = redact_str(&r, &format!("https://api.telegram.org/bot{token}/getMe"));
        assert!(out.contains("<REDACTED:telegram-bot-token>"));
        assert!(!out.contains(token));
    }

    /// Slack tokens (`xox[abprs]-…`) must be redacted before an observed
    /// transcript is persisted. The fixture is composed at runtime so a
    /// literal Slack-token shape isn't checked into source (GitHub
    /// secret-scanning push protection flags the literal shape even for
    /// fake values).
    #[test]
    fn redacts_slack_token() {
        let r = Redactor::new_default();
        let token = format!("xoxb-{}", "a".repeat(24));
        let (out, report) = redact_str(&r, &format!("SLACK_TOKEN={token}"));
        assert!(out.contains("<REDACTED:slack-token>"));
        assert!(!out.contains(&token));
        assert!(report.matches.iter().any(|m| m.rule_id == "slack-token"));
    }

    /// Google API keys (`AIza` + 35 chars) must be redacted. Composed at
    /// runtime to dodge secret-scanning push protection.
    #[test]
    fn redacts_google_api_key() {
        let r = Redactor::new_default();
        let key = format!("AIza{}", "a".repeat(35));
        let (out, report) = redact_str(&r, &format!("GOOGLE_API_KEY={key}"));
        assert!(out.contains("<REDACTED:google-api-key>"));
        assert!(!out.contains(&key));
        assert!(report.matches.iter().any(|m| m.rule_id == "google-api-key"));
    }

    /// OpenAI keys (`sk-…`, distinct from the more-specific `sk-ant-…`
    /// Anthropic shape) must be redacted and tagged `openai-api-key`.
    /// The fixture deliberately does NOT start with `ant-` so the
    /// Anthropic rule does not claim it.
    ///
    /// `redacts_anthropic_api_key_with_correct_tag` already asserts the
    /// *negative* (an `sk-ant-…` key must NOT get the `openai-api-key`
    /// tag); this is the missing *positive* counterpart — a plain
    /// `sk-…` key actually gets redacted and tagged `openai-api-key`.
    #[test]
    fn redacts_openai_api_key() {
        let r = Redactor::new_default();
        let key = format!("sk-{}", "a".repeat(24));
        let (out, report) = redact_str(&r, &format!("OPENAI_API_KEY={key}"));
        assert!(out.contains("<REDACTED:openai-api-key>"));
        assert!(!out.contains("<REDACTED:anthropic-api-key>"));
        assert!(!out.contains(&key));
        assert!(report.matches.iter().any(|m| m.rule_id == "openai-api-key"));
    }

    /// JWTs (`eyJ…header.payload.signature`) must be redacted — they
    /// frequently carry bearer credentials. Composed from three
    /// base64url-shaped segments at runtime.
    #[test]
    fn redacts_jwt() {
        let r = Redactor::new_default();
        let jwt = format!(
            "eyJ{}.{}.{}",
            "a".repeat(12),
            "b".repeat(12),
            "c".repeat(12)
        );
        let (out, report) = redact_str(&r, &format!("Authorization: Bearer {jwt}"));
        assert!(out.contains("<REDACTED:jwt>"));
        assert!(!out.contains(&jwt));
        assert!(report.matches.iter().any(|m| m.rule_id == "jwt"));
    }

    #[test]
    fn redacts_discord_bot_token() {
        let r = Redactor::new_default();
        // Synthesised fixture matching the documented Discord token shape
        // (`<24-char-base64>.<6-char>.<27+-char>`) without checking in a
        // single literal string that GitHub's secret-scanning push
        // protection would flag as a "Discord Bot Token" regardless of
        // whether it's actually live. Splitting the parts and joining at
        // runtime keeps the test deterministic without tripping the
        // scanner.
        let part1 = "M".repeat(24);
        let part2 = "GabcDe";
        let part3 = "a".repeat(29);
        let token = format!("{part1}.{part2}.{part3}");
        let (out, _) = redact_str(&r, &token);
        assert!(out.contains("<REDACTED:discord-bot-token>"));
        assert!(!out.contains(&token));
    }

    /// Regression: the new rules should not fire on benign nearby text. A
    /// short clean transcript with no secret-like patterns must round-trip
    /// untouched.
    #[test]
    fn expanded_rules_do_not_fire_on_benign_text() {
        let r = Redactor::new_default();
        let benign = "the quick brown fox jumps over the lazy dog and eats kibble";
        let (out, report) = redact_str(&r, benign);
        assert_eq!(out, benign);
        assert!(report.matches.is_empty());
    }

    /// Belt-and-suspenders test: the `DEFAULT_RULES` `Lazy` static is
    /// initialized via `Regex::new(...).expect(...)` and a single bad
    /// pattern would poison every subsequent caller (we hit this exact
    /// failure mode in CEX-EntireIO Phase 3.2 with a `(?=...)` lookahead
    /// pattern). This test forces the Lazy to evaluate eagerly so any
    /// future bad regex turns into a localised, named failure rather than
    /// a "Lazy instance has previously been poisoned" cascade.
    #[test]
    fn default_rules_initialize_without_poisoning_lazy() {
        let r = Redactor::new_default();
        assert!(
            r.rule_count() >= 8,
            "default redactor must register at least the v1 rule set; got {}",
            r.rule_count()
        );
    }
}
