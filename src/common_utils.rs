//! Common helpers for formatting commit messages, parsing embedded GPG signatures, and
//! validating Conventional Commit styles.
//!
//! This module is intentionally dependency-light so that it can be shared by both the CLI
//! command layer and lower-level repository code without introducing dependency cycles.
//! All functions here are pure (no I/O, no global state) and operate on string slices.

use std::sync::LazyLock;

use regex::Regex;

/// Build the canonical commit body that will be hashed into a commit object.
///
/// Functional scope:
/// - When `gpg_sig` is `None`, prepends a single blank line before `msg`. The leading
///   newline is required: remote `git unpack` fails when the blank-line separator is
///   missing.
/// - When `gpg_sig` is `Some(sig)`, places the signature first, then a single blank
///   line, then the user-provided message. The blank line separates signature trailers
///   from the message body and is mandated by the Git object format.
///
/// Boundary conditions:
/// - `msg` is not trimmed; trailing whitespace inside the message is preserved as-is.
/// - `gpg_sig` is treated as opaque text — no parsing is performed.
pub fn format_commit_msg(msg: &str, gpg_sig: Option<&str>) -> String {
    match gpg_sig {
        None => {
            format!("\n{msg}")
        }
        Some(gpg) => {
            format!("{gpg}\n\n{msg}")
        }
    }
}

/// Split a stored commit body into `(message, optional_signature)`.
///
/// Functional scope:
/// - Detects an embedded `gpgsig` header at the start of the input (PGP or SSH).
/// - When a signature is present, returns the trimmed message body and a borrowed
///   slice covering only the signature block (without the leading `gpgsig ` prefix).
/// - When no signature header is found, returns the trimmed input as the message and
///   `None` for the signature.
///
/// Boundary conditions:
/// - The returned `&str` slices borrow from the original `msg_gpg` buffer; callers
///   must keep that buffer alive.
/// - Both PGP and SSH signature blocks are recognised; any other prefix (or a missing
///   prefix) is treated as a plain message.
/// - Leading whitespace on the message body is trimmed, but inner whitespace is kept
///   verbatim so that commit content survives a round-trip through this function.
pub fn parse_commit_msg(msg_gpg: &str) -> (&str, Option<&str>) {
    const SIG_PATTERN: &str = r"^gpgsig (-----BEGIN (?:PGP|SSH) SIGNATURE-----[\s\S]*?-----END (?:PGP|SSH) SIGNATURE-----)";
    const GPGSIG_PREFIX_LEN: usize = 7; // length of "gpgsig "
    static SIG_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        // INVARIANT: SIG_PATTERN is a validated regex literal checked in tests.
        Regex::new(SIG_PATTERN).expect("SIG_PATTERN must compile")
    });

    if let Some(caps) = SIG_REGEX.captures(msg_gpg) {
        // INVARIANT: SIG_PATTERN defines capture group 1 for the full signature body.
        let signature = caps
            .get(1)
            .expect("SIG_PATTERN must capture the signature body")
            .as_str();

        let msg = &msg_gpg[signature.len() + GPGSIG_PREFIX_LEN..].trim_start();
        (msg, Some(signature))
    } else {
        (msg_gpg.trim_start(), None)
    }
}

/// Check whether the first line of `msg` matches the Conventional Commits 1.0 grammar.
///
/// Functional scope:
/// - Only the *first* line (the subject) is validated. Body and footer text are
///   ignored, mirroring the Conventional Commits specification which only places
///   constraints on the subject line.
/// - Accepts the form `type(scope)?!?: description`, where `type` is restricted to
///   letters/digits/`_`/`-` and `scope`/`description` accept any visible Unicode.
///
/// Boundary conditions:
/// - Returns `false` for empty input, since `lines().next()` yields an empty subject
///   that cannot match the regex.
/// - The eight conventional types (`build`, `chore`, `ci`, `docs`, `feat`, `fix`,
///   `perf`, `refactor`) are recognised but **not** required — any non-empty type
///   token is accepted, matching the spec which only treats those names as
///   recommendations.
/// - The breaking-change marker `!` after `type` (or `(scope)`) is allowed but not
///   required.
///
/// Reference: <https://www.conventionalcommits.org/en/v1.0.0/>
pub fn check_conventional_commits_message(msg: &str) -> bool {
    let first_line = msg.lines().next().unwrap_or_default();
    #[allow(unused_variables)]
    let body_footer = msg.lines().skip(1).collect::<Vec<_>>().join("\n");

    let unicode_pattern = r"\p{L}\p{N}\p{P}\p{S}\p{Z}";
    // type only support characters&numbers, others fields support all unicode characters
    let regex_str = format!(
        r"^(?P<type>[\p{{L}}\p{{N}}_-]+)(?:\((?P<scope>[{unicode_pattern}]+)\))?!?: (?P<description>[{unicode_pattern}]+)$",
    );

    // INVARIANT: regex_str is assembled from static, validated fragments.
    let re = Regex::new(&regex_str).expect("conventional commit regex must compile");
    const RECOMMENDED_TYPES: [&str; 8] = [
        "build", "chore", "ci", "docs", "feat", "fix", "perf", "refactor",
    ];

    if let Some(captures) = re.captures(first_line) {
        let commit_type = captures.name("type").map(|m| m.as_str().to_string());
        #[allow(unused_variables)]
        let scope = captures.name("scope").map(|m| m.as_str().to_string());
        let description = captures.name("description").map(|m| m.as_str().to_string());
        if commit_type.is_none() || description.is_none() {
            return false;
        }

        let Some(commit_type) = commit_type else {
            return false;
        };
        let _is_recommended = RECOMMENDED_TYPES.contains(&commit_type.to_lowercase().as_str());

        // println!("{}({}): {}\n{}", commit_type, scope.unwrap_or("None".to_string()), description.unwrap(), body_footer);

        return true;
    }
    false
}
