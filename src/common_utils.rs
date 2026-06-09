//! Common helpers for formatting commit messages, parsing embedded GPG signatures, and
//! validating Conventional Commit styles.
//!
//! This module is intentionally dependency-light so that it can be shared by both the CLI
//! command layer and lower-level repository code without introducing dependency cycles.
//! All functions here are pure (no I/O, no global state) and operate on string slices.

use std::sync::LazyLock;

use regex::Regex;

/// Trailer appended to every Libra-produced commit message body.
pub const VERSION_CONTROL_BY_TRAILER: &str = "Version-control-by: Libra <https://libra.tools>";

/// Append [`VERSION_CONTROL_BY_TRAILER`] to `msg` when not already present.
///
/// Trailers are separated from the message body with a blank line. Idempotent:
/// if any line already starts with `Version-control-by:`, `msg` is returned unchanged.
pub fn append_version_control_trailer(msg: &str) -> String {
    if msg
        .lines()
        .any(|line| line.trim_start().starts_with("Version-control-by:"))
    {
        return msg.to_string();
    }

    let trimmed = msg.trim_end();
    if trimmed.is_empty() {
        VERSION_CONTROL_BY_TRAILER.to_string()
    } else {
        format!("{trimmed}\n\n{VERSION_CONTROL_BY_TRAILER}")
    }
}

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
/// - Trailing whitespace at the end of `msg` is trimmed before the
///   `Version-control-by` trailer is appended. Interior whitespace is
///   preserved as-is.
/// - `gpg_sig` is treated as opaque text — no parsing is performed.
pub fn format_commit_msg(msg: &str, gpg_sig: Option<&str>) -> String {
    let msg = append_version_control_trailer(msg);
    match gpg_sig {
        None => {
            format!("\n{msg}")
        }
        Some(gpg) => {
            format!("{gpg}\n\n{msg}")
        }
    }
}

fn strip_version_control_trailer(message: &str) -> &str {
    let trimmed = message.trim_end();
    let Some(without_trailer) = trimmed.strip_suffix(VERSION_CONTROL_BY_TRAILER) else {
        return trimmed;
    };

    let without_trailer = without_trailer.trim_end();
    without_trailer
        .strip_suffix("\n\n")
        .unwrap_or(without_trailer)
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

        let msg = strip_version_control_trailer(
            msg_gpg[signature.len() + GPGSIG_PREFIX_LEN..].trim_start(),
        );
        (msg, Some(signature))
    } else {
        (strip_version_control_trailer(msg_gpg.trim_start()), None)
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

#[cfg(test)]
mod tests {
    //! `format_commit_msg` is used as a fixture builder in several
    //! command integration tests, but its exact output contract, the
    //! `parse_commit_msg` round-trip, and `check_conventional_commits_message`
    //! (which has no test references at all) were never directly
    //! asserted. These pins guard the commit-object byte format — the
    //! leading-newline / blank-line separators are load-bearing
    //! (a missing separator makes remote `git unpack` reject the
    //! object).

    use super::*;

    /// Without a signature the body is exactly `"\n{msg}"` — the
    /// leading blank line is required by the Git object format; remote
    /// `git unpack` fails without it.
    #[test]
    fn format_commit_msg_unsigned_prepends_blank_line() {
        assert_eq!(
            format_commit_msg("hello", None),
            format!("\nhello\n\n{VERSION_CONTROL_BY_TRAILER}"),
        );
        // Interior whitespace is preserved; trailing whitespace before
        // the generated trailer is normalized.
        assert_eq!(
            format_commit_msg("a\nb ", None),
            format!("\na\nb\n\n{VERSION_CONTROL_BY_TRAILER}"),
        );
    }

    /// With a signature the body is exactly `"{gpg}\n\n{msg}"` — the
    /// blank line separates the signature trailer from the message.
    #[test]
    fn format_commit_msg_signed_places_sig_then_blank_line() {
        assert_eq!(
            format_commit_msg("subject", Some("gpgsig BLOCK")),
            format!("gpgsig BLOCK\n\nsubject\n\n{VERSION_CONTROL_BY_TRAILER}"),
        );
    }

    #[test]
    fn append_version_control_trailer_is_idempotent() {
        let once = append_version_control_trailer("feat: add thing");
        assert!(once.ends_with(VERSION_CONTROL_BY_TRAILER));
        assert_eq!(append_version_control_trailer(&once), once);
    }

    #[test]
    fn append_version_control_trailer_after_signoff() {
        let msg = "feat: add thing\n\nSigned-off-by: Alice <alice@example.com>";
        let with_trailer = append_version_control_trailer(msg);
        assert!(with_trailer.contains("Signed-off-by: Alice <alice@example.com>"));
        assert!(with_trailer.ends_with(VERSION_CONTROL_BY_TRAILER));
    }

    /// A plain (unsigned) body parses back to the trimmed message and
    /// `None`; leading whitespace is stripped but inner content is kept.
    #[test]
    fn parse_commit_msg_plain_message_has_no_signature() {
        assert_eq!(parse_commit_msg("  hello\nworld"), ("hello\nworld", None));
        assert_eq!(parse_commit_msg("subject"), ("subject", None));
    }

    #[test]
    fn parse_commit_msg_hides_version_control_trailer() {
        assert_eq!(
            parse_commit_msg(&format!("subject\n\n{VERSION_CONTROL_BY_TRAILER}")),
            ("subject", None)
        );
        assert_eq!(
            parse_commit_msg(&format!(
                "subject\n\nSigned-off-by: Alice <alice@example.com>\n\n{VERSION_CONTROL_BY_TRAILER}"
            )),
            ("subject\n\nSigned-off-by: Alice <alice@example.com>", None)
        );
    }

    /// A `gpgsig`-prefixed PGP signature block round-trips: the parsed
    /// signature is the BEGIN..END block (without the `gpgsig ` prefix)
    /// and the message is the trimmed remainder. Built via
    /// `format_commit_msg` so the format⇄parse pair is exercised
    /// together.
    #[test]
    fn parse_commit_msg_round_trips_pgp_signature() {
        let sig = "-----BEGIN PGP SIGNATURE-----\nabcDEF123\n-----END PGP SIGNATURE-----";
        let body = format_commit_msg("the subject", Some(&format!("gpgsig {sig}")));
        let (msg, parsed_sig) = parse_commit_msg(&body);
        assert_eq!(msg, "the subject");
        assert_eq!(parsed_sig, Some(sig));
    }

    /// SSH signature blocks are recognised the same way as PGP.
    #[test]
    fn parse_commit_msg_recognises_ssh_signature() {
        let sig = "-----BEGIN SSH SIGNATURE-----\nU1NIU0lH\n-----END SSH SIGNATURE-----";
        let body = format!("gpgsig {sig}\n\nssh-signed subject");
        let (msg, parsed_sig) = parse_commit_msg(&body);
        assert_eq!(msg, "ssh-signed subject");
        assert_eq!(parsed_sig, Some(sig));
    }

    /// Conventional-commit subjects: accept `type: desc`, optional
    /// `(scope)` and breaking `!`.
    #[test]
    fn conventional_commits_accepts_valid_subjects() {
        for ok in [
            "feat: add a thing",
            "fix(parser): handle empty input",
            "feat!: breaking change",
            "chore(api)!: drop field",
            "refactor: tidy module",
            // Only the first line matters; body is ignored.
            "docs: update readme\n\nlong body here",
            // Non-recommended type tokens are still accepted (spec only
            // *recommends* the eight canonical types).
            "wibble: custom type allowed",
        ] {
            assert!(
                check_conventional_commits_message(ok),
                "expected `{ok}` to be a valid conventional-commit subject",
            );
        }
    }

    /// Rejects subjects that break the grammar: empty, no `: `
    /// separator, empty description, or a leading space before the
    /// type.
    #[test]
    fn conventional_commits_rejects_invalid_subjects() {
        for bad in [
            "",
            "just a plain message",
            "feat:no space after colon",
            "feat: ",            // empty description
            "feat",              // no colon at all
            " feat: leading sp", // leading space before type
            ": no type",         // empty type
        ] {
            assert!(
                !check_conventional_commits_message(bad),
                "expected `{bad}` to be rejected as a conventional-commit subject",
            );
        }
    }
}
