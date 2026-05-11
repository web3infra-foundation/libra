//! Publish preflight: ignore + deny-rule evaluation per
//! `docs/improvement/publish.md`.
//!
//! Phase 3 requires every file the snapshot builder considers to
//! pass the built-in deny rules and the optional
//! `.librapublishignore` overlay. The deny rules cover credential-
//! style filenames (`.env*`, `*.pem`, `*.key`, `id_rsa*`, …) plus a
//! handful of always-bad paths (`.git/`, `.libra/config.db`).
//!
//! The `.librapublishignore` parser is a gitignore subset:
//!
//!   * empty + `#` comment lines are skipped
//!   * `pattern` matches paths whose any segment equals `pattern`
//!     (we use the `ignore` crate elsewhere, but here we keep the
//!     surface simple to avoid pulling more dependencies into the
//!     snapshot path)
//!   * trailing `/` matches directories only
//!   * `!pattern` un-ignores a previously-matched path
//!   * nested includes are NOT supported (publish.md non-goal)
//!
//! For v1 the implementation is intentionally simple — it covers
//! the publish.md acceptance criteria without re-implementing every
//! gitignore corner case.

use std::path::Path;

/// Outcome of evaluating a single path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreflightDecision {
    /// Allow publication.
    Allow,
    /// Deny publication; the reason is encoded so the sync run can
    /// surface it as a warning or hard failure depending on
    /// visibility.
    Deny(DenyReason),
}

/// Why a path was denied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DenyReason {
    /// One of the built-in credential / secret / system path rules.
    BuiltinCredential,
    /// A user-authored `.librapublishignore` rule.
    UserIgnore,
}

/// Built-in deny patterns (segment-aware, case-insensitive).
///
/// Mirrors the spirit of `embed_path_is_allowed` in
/// `worker_template.rs` so a file the embed step rejects at
/// publish-init time also gets rejected at sync time. The shapes
/// are different (worker_template uses substring + allowlist; here
/// we do segment-equal + suffix patterns) because `.librapublishignore`
/// follows gitignore conventions.
const BUILTIN_DENY_EXACT: &[&str] = &[".git", ".libra"];
const BUILTIN_DENY_SUFFIXES: &[&str] = &[".pem", ".key"];
const BUILTIN_DENY_PREFIXES: &[&str] = &["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", ".env"];

/// Bounded credential keywords — match only when preceded by an
/// underscore / dash / dot separator. Matches the embed runtime
/// allowlist so `tokens.css` stays publishable while `auth_token.json`
/// is denied.
const BUILTIN_DENY_BOUNDED_FRAGMENTS: &[&str] = &["token", "secret"];
const BUILTIN_DENY_FRAGMENTS: &[&str] = &["credential"];

/// Runtime evaluator for one repo's preflight policy.
#[derive(Clone, Debug, Default)]
pub struct Preflight {
    user_rules: Vec<UserRule>,
    /// Paths the operator explicitly opted out of via
    /// `--allow-sensitive-path` on `libra publish sync`. Only
    /// honored on private sites — the snapshot orchestrator must
    /// guard the assignment, NOT this evaluator.
    allow_sensitive_paths: Vec<String>,
}

/// One parsed `.librapublishignore` rule.
#[derive(Clone, Debug)]
struct UserRule {
    /// Bare segment / suffix / prefix string (lowercased).
    needle: String,
    /// Match style.
    style: UserRuleStyle,
    /// `true` for `!pattern` overrides.
    negated: bool,
    /// `true` when the rule ended with `/`, restricting it to
    /// directory matches.
    dir_only: bool,
}

#[derive(Clone, Copy, Debug)]
#[allow(clippy::enum_variant_names)] // every variant operates on a path "segment"; the prefix is the domain term, not noise
enum UserRuleStyle {
    /// Exact segment match (no `*`).
    SegmentExact,
    /// Segment ends with the needle (`*.log` → suffix `.log`).
    SegmentSuffix,
    /// Segment starts with the needle (`build*` → prefix `build`).
    SegmentPrefix,
}

impl Preflight {
    /// Construct an empty evaluator. Callers populate it via
    /// [`Self::extend_with_ignore_text`] and
    /// [`Self::with_allow_sensitive_paths`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse a `.librapublishignore` file body and extend the rule
    /// set. Lines starting with `#` and blank lines are skipped.
    pub fn extend_with_ignore_text(&mut self, text: &str) {
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (negated, body) = match line.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            let (dir_only, body) = match body.strip_suffix('/') {
                Some(rest) => (true, rest),
                None => (false, body),
            };
            let body = body.trim();
            if body.is_empty() {
                continue;
            }
            let lower = body.to_ascii_lowercase();
            // Translate one of `pattern`, `*.suffix`, `prefix*` to a
            // typed rule. `*?` and brace expansion are publish.md
            // non-goals.
            let style = if let Some(rest) = lower.strip_prefix('*') {
                UserRuleStyle::SegmentSuffix.into_with(rest.to_string(), negated, dir_only)
            } else if let Some(rest) = lower.strip_suffix('*') {
                UserRuleStyle::SegmentPrefix.into_with(rest.to_string(), negated, dir_only)
            } else {
                UserRuleStyle::SegmentExact.into_with(lower, negated, dir_only)
            };
            self.user_rules.push(style);
        }
    }

    /// Mark a list of relative paths as opted-in via
    /// `--allow-sensitive-path`. Only the orchestrator should call
    /// this on sites where the visibility setting permits the
    /// override.
    pub fn with_allow_sensitive_paths(mut self, paths: Vec<String>) -> Self {
        self.allow_sensitive_paths = paths;
        self
    }

    /// Evaluate one repo-relative path. `is_dir` is `true` for
    /// directory entries (used by gitignore-style trailing-`/` rules).
    pub fn evaluate(&self, relative_path: &Path, is_dir: bool) -> PreflightDecision {
        // Allowlist override — operator explicitly accepted this
        // path. Honored only when the orchestrator has populated
        // the list (we trust the caller's visibility check).
        if self.allow_sensitive_paths.iter().any(|allow| {
            // Match by exact relative path; segment-equal so a
            // user-allowed `config/foo.env` does not unblock
            // `bigger-config/foo.env`.
            relative_path.as_os_str() == std::ffi::OsString::from(allow)
        }) {
            return PreflightDecision::Allow;
        }

        // User rule pass — evaluate in declaration order so a
        // later `!pattern` can rescue a path matched earlier.
        let mut user_decision: Option<PreflightDecision> = None;
        for rule in &self.user_rules {
            if rule_matches(rule, relative_path, is_dir) {
                user_decision = Some(if rule.negated {
                    PreflightDecision::Allow
                } else {
                    PreflightDecision::Deny(DenyReason::UserIgnore)
                });
            }
        }
        if let Some(PreflightDecision::Deny(_)) = user_decision {
            return PreflightDecision::Deny(DenyReason::UserIgnore);
        }

        // Built-in deny pass.
        for segment in relative_path
            .iter()
            .filter_map(|s| s.to_str().map(|s| s.to_ascii_lowercase()))
        {
            for exact in BUILTIN_DENY_EXACT {
                if segment == *exact {
                    return PreflightDecision::Deny(DenyReason::BuiltinCredential);
                }
            }
            for suffix in BUILTIN_DENY_SUFFIXES {
                if segment.ends_with(suffix) {
                    return PreflightDecision::Deny(DenyReason::BuiltinCredential);
                }
            }
            for prefix in BUILTIN_DENY_PREFIXES {
                if segment.starts_with(prefix) {
                    return PreflightDecision::Deny(DenyReason::BuiltinCredential);
                }
            }
            for needle in BUILTIN_DENY_FRAGMENTS {
                if segment.contains(needle) {
                    return PreflightDecision::Deny(DenyReason::BuiltinCredential);
                }
            }
            for needle in BUILTIN_DENY_BOUNDED_FRAGMENTS {
                for sep in ['_', '-', '.'] {
                    let bounded = format!("{sep}{needle}");
                    if segment.contains(&bounded) {
                        return PreflightDecision::Deny(DenyReason::BuiltinCredential);
                    }
                }
            }
        }

        match user_decision {
            Some(decision) => decision,
            None => PreflightDecision::Allow,
        }
    }
}

trait UserRuleStyleExt {
    fn into_with(self, needle: String, negated: bool, dir_only: bool) -> UserRule;
}

impl UserRuleStyleExt for UserRuleStyle {
    fn into_with(self, needle: String, negated: bool, dir_only: bool) -> UserRule {
        UserRule {
            needle,
            style: self,
            negated,
            dir_only,
        }
    }
}

fn rule_matches(rule: &UserRule, relative_path: &Path, is_dir: bool) -> bool {
    if rule.dir_only && !is_dir {
        return false;
    }
    relative_path
        .iter()
        .filter_map(|s| s.to_str().map(|s| s.to_ascii_lowercase()))
        .any(|segment| match rule.style {
            UserRuleStyle::SegmentExact => segment == rule.needle,
            UserRuleStyle::SegmentSuffix => segment.ends_with(&rule.needle),
            UserRuleStyle::SegmentPrefix => segment.starts_with(&rule.needle),
        })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn allow(p: &str) -> PreflightDecision {
        Preflight::new().evaluate(&PathBuf::from(p), false)
    }

    #[test]
    fn builtin_denies_env_files() {
        for path in [".env", ".env.local", ".env.production", ".env.example"] {
            assert_eq!(
                allow(path),
                PreflightDecision::Deny(DenyReason::BuiltinCredential),
                "{path:?} must be denied"
            );
        }
    }

    #[test]
    fn builtin_denies_ssh_keys_with_prefix_match() {
        for path in [
            "id_rsa",
            "config/id_rsa.pub",
            "keys/id_ed25519_work",
            "keys/id_ecdsa-2024",
            "keys/id_dsa.bak",
        ] {
            assert_eq!(
                allow(path),
                PreflightDecision::Deny(DenyReason::BuiltinCredential),
                "{path:?} must be denied"
            );
        }
    }

    #[test]
    fn builtin_denies_pem_key_credential() {
        for path in ["server.pem", "api.key", "aws-credentials.json"] {
            assert_eq!(
                allow(path),
                PreflightDecision::Deny(DenyReason::BuiltinCredential),
                "{path:?} must be denied"
            );
        }
    }

    #[test]
    fn builtin_allows_design_token_assets() {
        for path in [
            "src/lib.rs",
            "README.md",
            "design/tokens.css",
            "design/tokens.ts",
            "components/SecretSauce.tsx",
        ] {
            assert_eq!(
                allow(path),
                PreflightDecision::Allow,
                "{path:?} must be allowed"
            );
        }
    }

    #[test]
    fn builtin_denies_bounded_token_secret() {
        for path in [
            "auth_token.json",
            "api-token.txt",
            "service.token.config",
            "auth_secret.json",
        ] {
            assert_eq!(
                allow(path),
                PreflightDecision::Deny(DenyReason::BuiltinCredential),
                "{path:?} must be denied"
            );
        }
    }

    #[test]
    fn user_rule_overrides_via_negation() {
        let mut p = Preflight::new();
        p.extend_with_ignore_text("*.log\n!keep.log\n");
        assert_eq!(
            p.evaluate(&PathBuf::from("logs/server.log"), false),
            PreflightDecision::Deny(DenyReason::UserIgnore),
        );
        assert_eq!(
            p.evaluate(&PathBuf::from("logs/keep.log"), false),
            PreflightDecision::Allow,
        );
    }

    #[test]
    fn user_dir_rule_only_matches_dirs() {
        let mut p = Preflight::new();
        p.extend_with_ignore_text("build/\n");
        assert_eq!(
            p.evaluate(&PathBuf::from("build"), true),
            PreflightDecision::Deny(DenyReason::UserIgnore),
        );
        assert_eq!(
            p.evaluate(&PathBuf::from("build"), false),
            PreflightDecision::Allow,
        );
    }

    #[test]
    fn allow_sensitive_path_overrides_builtin() {
        let p = Preflight::new().with_allow_sensitive_paths(vec![".env.local".to_string()]);
        assert_eq!(
            p.evaluate(&PathBuf::from(".env.local"), false),
            PreflightDecision::Allow,
        );
        // Other secrets stay denied.
        assert_eq!(
            p.evaluate(&PathBuf::from("server.pem"), false),
            PreflightDecision::Deny(DenyReason::BuiltinCredential),
        );
    }

    #[test]
    fn comments_and_blank_lines_skipped() {
        let mut p = Preflight::new();
        p.extend_with_ignore_text("# comment\n\n   \n*.bak\n");
        assert_eq!(
            p.evaluate(&PathBuf::from("file.bak"), false),
            PreflightDecision::Deny(DenyReason::UserIgnore),
        );
    }
}
