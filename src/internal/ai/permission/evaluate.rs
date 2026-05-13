//! Wildcard match + ruleset evaluation algorithms.
//!
//! The two functions here mirror opencode's `permission/evaluate.ts` and
//! `Permission.disabled()` so a future opencode ⇄ Libra ruleset interchange
//! is a structural copy rather than a semantic translation.

use std::collections::HashSet;

use super::rule::{PermissionAction, PermissionRule, PermissionRuleset};

/// Tool names / aliases reserved as `edit` permission targets by the
/// registry pre-filter. A `permission: "edit"` rule covers any tool whose
/// name appears in this list, so an author writes one rule instead of three.
///
/// Note: some entries here are spec-declared aliases that may not have a
/// live handler in this build (e.g. only `apply_patch` is currently
/// registered as a real `ToolHandler`). The names that lack a handler are
/// still listed because the spec uses them and a future handler will pick
/// them up automatically; until then they are no-ops at the filter layer.
///
/// Adding a new edit-class handler? Append its tool name here and add a
/// case to the `disabled_groups_edit_tools_under_edit_key` regression
/// test in the registry pre-filter suite.
pub const EDIT_TOOLS: &[&str] = &["apply_patch", "write_file", "patch"];

/// Returns `EDIT_TOOLS` as a slice of `&str` so callers do not need to
/// know the constant's storage shape. The pre-filter and tests both reach
/// for this function.
pub fn edit_tools() -> &'static [&'static str] {
    EDIT_TOOLS
}

/// Glob-style match against a single rule field.
///
/// Functional scope:
/// - `pattern == "*"` matches any input.
/// - Patterns containing **no** wildcard characters fall back to byte-equal
///   string compare (the common case for `permission: "edit"`).
/// - `*` inside a pattern matches any run of characters (including the
///   empty run); this is the same semantics as opencode's `Wildcard.match`.
///   `?` is **not** supported (opencode does not use it either).
///
/// Boundary conditions:
/// - The match is byte-precise; case differences and Unicode normalisation
///   are caller responsibilities.
/// - A pattern that is purely wildcards (`****`) collapses to "any input".
/// - The prefix is anchored via `starts_with` and the suffix via
///   `ends_with`, so a duplicated suffix earlier in the input (e.g.
///   `"prefixfoofoo"` against `"prefix*foo"`) still matches — the suffix
///   anchor honors the rightmost occurrence, not the first.
pub fn wildcard_match(input: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return input == pattern;
    }

    // Split on `*`. Empty segments are produced by consecutive `*` or by a
    // leading / trailing `*` and represent "any run of characters" for
    // the prefix or suffix anchor.
    let segments: Vec<&str> = pattern.split('*').collect();
    let first = segments.first().copied().unwrap_or("");
    let last = segments.last().copied().unwrap_or("");

    // Anchor the prefix and the suffix independently so a literal suffix
    // that also appears earlier in the input does not steal the match.
    if !input.starts_with(first) {
        return false;
    }
    if !input.ends_with(last) {
        return false;
    }
    // Prefix and suffix must fit without overlapping.
    if first.len() + last.len() > input.len() {
        return false;
    }

    // Middle segments must appear in order, in the area strictly between
    // the prefix and suffix anchors. Leftmost match is fine for middles
    // because the suffix is already pinned to the end.
    if segments.len() > 2 {
        let mut cursor = first.len();
        let end_boundary = input.len() - last.len();
        for &seg in &segments[1..segments.len() - 1] {
            if seg.is_empty() {
                continue;
            }
            if cursor > end_boundary {
                return false;
            }
            let search_area = &input[cursor..end_boundary];
            match search_area.find(seg) {
                Some(found) => cursor += found + seg.len(),
                None => return false,
            }
        }
    }
    true
}

/// Result of looking up a `(permission_key, pattern)` pair in one or more
/// rulesets.
///
/// Returns the **last** matching rule across the concatenated rulesets
/// (opencode's `findLast` semantics). Order is therefore significant —
/// callers that want a session ruleset to override a project ruleset must
/// pass the project ruleset first.
pub fn evaluate<'a>(
    permission_key: &str,
    pattern_input: &str,
    rulesets: &'a [&'a PermissionRuleset],
) -> Option<&'a PermissionRule> {
    rulesets
        .iter()
        .flat_map(|ruleset| ruleset.iter())
        .rfind(|rule| {
            wildcard_match(permission_key, &rule.permission)
                && wildcard_match(pattern_input, &rule.pattern)
        })
}

/// Compute the set of tool names that must be **removed from the model's
/// schema** because a `pattern == "*"` Deny rule covers them.
///
/// Algorithm (matches `docs/improvement/opencode.md` `Tool Registry 预过滤
/// 合同` verbatim):
///
/// 1. For each tool name in `all_tools`, decide whether it groups under the
///    `edit` permission key (see [`EDIT_TOOLS`]) or stays under its own name.
/// 2. Walk the ruleset in **reverse** and find the first rule whose
///    `permission` field wildcard-matches the chosen permission key.
/// 3. If that rule has `pattern == "*"` and `action == Deny`, the tool is
///    disabled. Any pattern other than `*` does **not** disable the tool —
///    the runtime can still evaluate the rule per call site to refuse a
///    specific path.
///
/// Why pre-filter at the schema layer: once the model sees a tool name in
/// the schema, it will keep retrying to use it. Removing the tool entirely
/// turns "model burns tokens trying a denied tool" into "model never sees
/// the tool", which is both cheaper and less noisy for transcript review.
pub fn disabled(all_tools: &[&str], ruleset: &PermissionRuleset) -> HashSet<String> {
    let mut out = HashSet::new();
    for &tool in all_tools {
        let permission_key = if EDIT_TOOLS.contains(&tool) {
            "edit"
        } else {
            tool
        };
        let last = ruleset
            .iter()
            .rev()
            .find(|rule| wildcard_match(permission_key, &rule.permission));
        if let Some(rule) = last
            && rule.pattern == "*"
            && rule.action == PermissionAction::Deny
        {
            out.insert(tool.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(permission: &str, pattern: &str, action: PermissionAction) -> PermissionRule {
        PermissionRule::new(permission, pattern, action)
    }

    /// Scenario: the documented `wildcard_match` shapes — exact, `*`,
    /// `prefix*`, `*suffix`, and `prefix*suffix` — all return the right
    /// answers. `*` matches zero-or-more characters, so a literal infix
    /// pattern like `middle_*end` accepts both `middle_end` (empty run)
    /// and `middle_xend` (non-empty run).
    #[test]
    fn wildcard_match_covers_documented_shapes() {
        assert!(wildcard_match("edit", "*"));
        assert!(wildcard_match("edit", "edit"));
        assert!(!wildcard_match("edit", "shell"));

        assert!(wildcard_match("read_file", "read_*"));
        assert!(!wildcard_match("read", "read_*")); // missing the underscore prefix segment
        assert!(wildcard_match("foo_bar", "*_bar"));
        assert!(wildcard_match("middle_chunk_end", "middle_*_end"));
        // `*` matches the empty run, so the prefix and suffix can sit flush.
        assert!(wildcard_match("middle_end", "middle_*end"));
        assert!(wildcard_match("middle_xend", "middle_*end"));
        // But the suffix is anchored: a stray character after the literal
        // tail breaks the match.
        assert!(!wildcard_match("middle_endX", "middle_*end"));
    }

    /// Scenario: `*****` (a pattern of only wildcards) collapses to
    /// "any input" rather than failing to match.
    #[test]
    fn wildcard_match_handles_only_wildcards() {
        assert!(wildcard_match("anything", "*"));
        assert!(wildcard_match("anything", "**"));
        assert!(wildcard_match("", "*"));
        assert!(wildcard_match("", "**"));
    }

    /// Scenario: a literal suffix that also appears earlier in the input
    /// must still match — the suffix anchor honors the rightmost
    /// occurrence, not the first. Regression test for the
    /// `find` vs `ends_with` bug Codex flagged in OC-Phase 2 P2.3 review.
    #[test]
    fn wildcard_match_handles_duplicated_suffix() {
        assert!(wildcard_match("foofoo", "*foo"));
        assert!(wildcard_match("prefixfoofoo", "prefix*foo"));
        // Multi-`*` shape: each middle segment must appear, suffix anchors.
        assert!(wildcard_match("abcb", "*a*b"));
        // A stray character past the suffix still rejects.
        assert!(!wildcard_match("foofoox", "*foo"));
        assert!(!wildcard_match("prefixfoofoox", "prefix*foo"));
    }

    /// Scenario: empty inputs and exact-empty patterns. `wildcard_match("", "")`
    /// is the trivial accept; `wildcard_match("x", "")` rejects.
    #[test]
    fn wildcard_match_handles_empty_inputs() {
        assert!(wildcard_match("", ""));
        assert!(!wildcard_match("x", ""));
        // An empty input still matches `*` and any wildcard-only pattern.
        assert!(wildcard_match("", "*"));
        assert!(wildcard_match("", "**"));
    }

    /// Scenario: `evaluate` returns the **last** matching rule across the
    /// concatenated rulesets. Earlier rules act as fallbacks; later ones
    /// override.
    #[test]
    fn evaluate_returns_last_matching_rule_across_rulesets() {
        let project = vec![
            rule("edit", "*", PermissionAction::Deny),
            rule("read_file", "*", PermissionAction::Allow),
        ];
        let session = vec![rule("edit", "src/**", PermissionAction::Allow)];
        let chained: [&PermissionRuleset; 2] = [&project, &session];

        // Session override wins for `(edit, src/foo.rs)`.
        let hit = evaluate("edit", "src/foo.rs", &chained).unwrap();
        assert_eq!(hit.action, PermissionAction::Allow);
        assert_eq!(hit.pattern, "src/**");

        // Outside src/ the project deny wins (no session match).
        let hit = evaluate("edit", "tests/foo.rs", &chained).unwrap();
        assert_eq!(hit.action, PermissionAction::Deny);
        assert_eq!(hit.pattern, "*");

        // Unknown permission key returns None even with rulesets present.
        assert!(evaluate("nonexistent", "*", &chained).is_none());
    }

    /// Scenario: a `[{edit:*: deny}]` ruleset disables every member of
    /// `EDIT_TOOLS` (`apply_patch`, `write_file`, `patch`) — they all group
    /// under the `edit` permission key.
    #[test]
    fn disabled_groups_edit_tools_under_edit_key() {
        let ruleset = vec![rule("edit", "*", PermissionAction::Deny)];
        let all_tools = ["apply_patch", "write_file", "patch", "read_file", "grep"];
        let out = disabled(&all_tools, &ruleset);
        assert!(out.contains("apply_patch"));
        assert!(out.contains("write_file"));
        assert!(out.contains("patch"));
        assert!(!out.contains("read_file"));
        assert!(!out.contains("grep"));
    }

    /// Scenario: a partial-pattern rule (`{edit: "src/**": allow}`) does NOT
    /// disable the edit tools — only `pattern == "*"` deny rules can take
    /// the tool entirely off the model's schema.
    #[test]
    fn disabled_keeps_edit_tools_when_pattern_is_partial() {
        let ruleset = vec![
            rule("edit", "*", PermissionAction::Deny),
            rule("edit", "src/**", PermissionAction::Allow),
        ];
        // The LAST rule for `edit` permission is now the `src/**` allow,
        // whose pattern is not `*`, so the tool stays in the schema.
        let all_tools = ["apply_patch"];
        let out = disabled(&all_tools, &ruleset);
        assert!(
            !out.contains("apply_patch"),
            "partial-pattern allow must un-disable apply_patch (got disabled = {out:?})"
        );
    }

    /// Scenario: `[{*:deny}, {grep:allow}]` — every tool except `grep` is
    /// disabled. The `*:deny` is the broad fallback; only `grep`'s explicit
    /// allow rule lands as the last match for that key.
    #[test]
    fn disabled_respects_per_tool_overrides_against_wildcard_deny() {
        let ruleset = vec![
            rule("*", "*", PermissionAction::Deny),
            rule("grep", "*", PermissionAction::Allow),
        ];
        let all_tools = ["grep", "read_file", "apply_patch"];
        let out = disabled(&all_tools, &ruleset);
        assert!(!out.contains("grep"), "grep override must keep it visible");
        assert!(out.contains("read_file"));
        assert!(out.contains("apply_patch"));
    }

    /// Scenario: an empty ruleset is a no-op — no tools are disabled.
    /// This is the flag-off baseline that `available_for(spec, &[])` must
    /// match: the model sees every tool the registry exposes.
    #[test]
    fn disabled_is_empty_for_empty_ruleset() {
        let ruleset: PermissionRuleset = Vec::new();
        let all_tools = ["apply_patch", "read_file", "grep"];
        let out = disabled(&all_tools, &ruleset);
        assert!(out.is_empty());
    }
}
