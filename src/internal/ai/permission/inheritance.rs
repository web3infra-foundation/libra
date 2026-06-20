//! Sub-agent permission inheritance + escalation gate.
//!
//! This module is the OC-Phase 3 P3.3 deliverable from
//! `docs/development/commands/_general.md`. It implements the two doc-specified
//! pure helpers the dispatcher uses to derive a child agent's effective
//! permission ruleset and to refuse a child that would silently override
//! a parent `Deny`:
//!
//! - [`agent_permission_spec_to_ruleset`] — bridges
//!   [`AgentPermissionSpec`] (the spec's allow/deny lists) to a
//!   [`PermissionRuleset`] (the runtime's ordered rule shape).
//! - [`child_ruleset`] — computes `effective_ruleset = inherited(parent)
//!   ++ sub_spec_rules ++ default_denies` matching the doc's pseudocode
//!   verbatim.
//! - [`assert_no_escalation`] — Libra-only safety gate that fails the
//!   dispatch if any `(permission, pattern)` combination would flip a
//!   parent `Deny` to a non-Deny child action.
//!
//! Why these are pure functions: P3.3 ships steps 1–7 of the dispatcher
//! main flow. The remaining steps (8–13) wire the runtime services. Pure
//! helpers here let P3.3 land with thorough test coverage that does not
//! require a live `PermissionService`, `ProviderFactory`, or session.

use super::{
    evaluate::evaluate,
    rule::{PermissionAction, PermissionRule, PermissionRuleset},
};
use crate::internal::ai::agent::profile::AgentPermissionSpec;

/// Lift an [`AgentPermissionSpec`] (allow/deny lists) into the ordered
/// [`PermissionRuleset`] shape the runtime uses.
///
/// Each tool name in `allowed_tools` becomes
/// `{permission: tool, pattern: "*", action: Allow}`. Each name in
/// `denied_tools` becomes the same shape with `action: Deny`. Source
/// slugs are NOT folded in here — they live on a separate gate that
/// OC-Phase 3 wires through [`crate::internal::ai::sources`]. Approval
/// routing is also out of scope; the runtime layer reads it directly
/// from [`AgentPermissionSpec`].
///
/// Order: deny rules come AFTER allow rules so a tool that appears in
/// both lists ends up Deny per `findLast` semantics. This matches the
/// doc rule "any `Deny` rule wins on overlap with `Allow`".
pub fn agent_permission_spec_to_ruleset(spec: &AgentPermissionSpec) -> PermissionRuleset {
    let mut out = Vec::with_capacity(spec.allowed_tools.len() + spec.denied_tools.len());
    for tool in &spec.allowed_tools {
        out.push(PermissionRule::new(
            tool.clone(),
            "*",
            PermissionAction::Allow,
        ));
    }
    for tool in &spec.denied_tools {
        out.push(PermissionRule::new(
            tool.clone(),
            "*",
            PermissionAction::Deny,
        ));
    }
    out
}

/// Compute the effective ruleset for a child sub-agent dispatch.
///
/// Algorithm (verbatim from `docs/development/commands/_general.md` "Sub-Agent
/// Permission Inheritance Algorithm"):
///
/// ```text
/// 1. base = []
/// 2. for rule in parent:
///      if rule.permission == "external_directory" OR rule.action == Deny:
///        base.push(rule)
/// 3. base.extend(sub_spec_rules)        # converted via
///                                       # agent_permission_spec_to_ruleset
/// 4. if "task" is not in sub_spec.allowed_tools:
///      base.push({permission: "task", pattern: "*", action: Deny})
/// 5. if "todowrite" is not in sub_spec.allowed_tools:
///      base.push({permission: "todowrite", pattern: "*", action: Deny})
/// 6. (experimental) extend with `primary_tools` allows; default empty
/// 7. return base
/// ```
///
/// What this function intentionally drops:
/// - The parent's positive `Allow` rules. The child gets only the
///   parent's `external_directory` allows (path-scoped) and `Deny`
///   rules (hard refusals). Capability is otherwise the child's own
///   spec.
///
/// What [`assert_no_escalation`] still has to enforce afterwards:
/// the child's own ruleset can declare an `Allow` whose `findLast`
/// would override the parent `Deny` — the doc explicitly carves out
/// this Libra-only safety gate to refuse such escalations. Run it on
/// the result of this function before dispatching the child.
pub fn child_ruleset(
    parent: &PermissionRuleset,
    sub_spec: &AgentPermissionSpec,
) -> PermissionRuleset {
    let mut out: PermissionRuleset = Vec::new();

    // Step 2: inherit parent's `external_directory` and Deny rules.
    for rule in parent {
        if rule.permission == "external_directory" || rule.action == PermissionAction::Deny {
            out.push(rule.clone());
        }
    }

    // Step 3: append the sub-spec's own rules (allow first, then deny).
    out.extend(agent_permission_spec_to_ruleset(sub_spec));

    // Step 4 + 5: default denies for `task` and `todowrite` unless the
    // sub-spec explicitly allows them. Default-deny prevents a child
    // from silently nesting another sub-agent or scribbling on the
    // parent's todo list when the spec did not opt in.
    if !sub_spec.allowed_tools.contains("task") {
        out.push(PermissionRule::new("task", "*", PermissionAction::Deny));
    }
    if !sub_spec.allowed_tools.contains("todowrite") {
        out.push(PermissionRule::new(
            "todowrite",
            "*",
            PermissionAction::Deny,
        ));
    }

    // Step 6: experimental `primary_tools` allows. The doc treats this
    // as a future extension hook; today we do nothing.
    out
}

/// Refuse the dispatch if any `(permission, pattern)` pair has the
/// parent ruleset evaluating to `Deny` while the effective child
/// ruleset evaluates to anything else. This is the Libra-only safety
/// gate the doc explicitly calls out — opencode's `findLast` semantics
/// would otherwise let a child silently override a parent deny.
///
/// The caller supplies `permission_keys` (every tool name + every
/// permission key the sub-spec declared) and `pattern_samples`
/// (typically `"*"` plus every pattern the sub-spec declared). Each
/// combination is evaluated against the parent and the child rulesets;
/// if a regression is found the function returns the offending
/// `(permission, pattern)` so the dispatcher can surface a
/// `TaskFailure::PermissionEscalationDenied` with that exact pair.
///
/// Returns `Ok(())` when no escalation is possible across the cartesian
/// product, or `Err((permission, pattern))` on the first regression.
pub fn assert_no_escalation(
    parent: &PermissionRuleset,
    effective: &PermissionRuleset,
    permission_keys: &[&str],
    pattern_samples: &[&str],
) -> Result<(), (String, String)> {
    let parent_only: [&PermissionRuleset; 1] = [parent];
    let effective_only: [&PermissionRuleset; 1] = [effective];
    for &perm in permission_keys {
        for &pattern in pattern_samples {
            let parent_action = evaluate(perm, pattern, &parent_only).map(|r| r.action);
            let effective_action = evaluate(perm, pattern, &effective_only).map(|r| r.action);
            if parent_action == Some(PermissionAction::Deny)
                && effective_action != Some(PermissionAction::Deny)
            {
                return Err((perm.to_string(), pattern.to_string()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn rule(permission: &str, pattern: &str, action: PermissionAction) -> PermissionRule {
        PermissionRule::new(permission, pattern, action)
    }

    fn spec_with_tools<const A: usize, const D: usize>(
        allowed: [&str; A],
        denied: [&str; D],
    ) -> AgentPermissionSpec {
        AgentPermissionSpec {
            allowed_tools: allowed
                .iter()
                .map(|s| (*s).to_string())
                .collect::<BTreeSet<_>>(),
            denied_tools: denied
                .iter()
                .map(|s| (*s).to_string())
                .collect::<BTreeSet<_>>(),
            ..AgentPermissionSpec::default()
        }
    }

    /// Scenario: a spec with `allowed_tools = [read_file]` and
    /// `denied_tools = [shell]` lifts to `[(read_file, *, Allow),
    /// (shell, *, Deny)]`. Allows come first so the trailing deny wins
    /// when both lists overlap.
    #[test]
    fn agent_permission_spec_lifts_into_ordered_ruleset() {
        let spec = spec_with_tools(["read_file"], ["shell"]);
        let ruleset = agent_permission_spec_to_ruleset(&spec);
        assert_eq!(ruleset.len(), 2);
        assert_eq!(ruleset[0].permission, "read_file");
        assert_eq!(ruleset[0].action, PermissionAction::Allow);
        assert_eq!(ruleset[1].permission, "shell");
        assert_eq!(ruleset[1].action, PermissionAction::Deny);
    }

    /// Scenario: a parent with mixed Allow / Deny / `external_directory`
    /// rules contributes ONLY its Deny + external_directory rules to
    /// the child. The parent's `Allow` for `read_file` does NOT carry
    /// over — the child must declare its own positive capability.
    #[test]
    fn child_ruleset_inherits_only_external_dir_and_deny_from_parent() {
        let parent = vec![
            rule("read_file", "*", PermissionAction::Allow),
            rule("shell", "rm -rf *", PermissionAction::Deny),
            rule("external_directory", "/tmp/**", PermissionAction::Allow),
        ];
        let sub_spec = spec_with_tools(["read_file"], []);
        let effective = child_ruleset(&parent, &sub_spec);

        // The child has the parent's deny + external_directory + its
        // own allow + the default `task`/`todowrite` denies.
        let permissions: Vec<&str> = effective.iter().map(|r| r.permission.as_str()).collect();
        assert!(permissions.contains(&"shell"));
        assert!(permissions.contains(&"external_directory"));
        assert!(permissions.contains(&"read_file"));
        assert!(permissions.contains(&"task"));
        assert!(permissions.contains(&"todowrite"));

        // Parent's positive `Allow` for read_file did NOT propagate;
        // the child's own `Allow` is the source of that capability.
        let read_file_rules: Vec<&PermissionRule> = effective
            .iter()
            .filter(|r| r.permission == "read_file")
            .collect();
        assert_eq!(
            read_file_rules.len(),
            1,
            "child must own the read_file rule, not inherit it"
        );
    }

    /// Scenario: when the sub-spec already lists `task` in its
    /// `allowed_tools`, the default `task: * deny` is NOT appended.
    /// This is the explicit opt-in path for sub-agents that are
    /// allowed to nest further dispatches.
    #[test]
    fn child_ruleset_skips_default_deny_when_sub_spec_allows_task() {
        let parent = Vec::new();
        let sub_spec = spec_with_tools(["task"], []);
        let effective = child_ruleset(&parent, &sub_spec);

        let task_rules: Vec<&PermissionRule> = effective
            .iter()
            .filter(|r| r.permission == "task")
            .collect();
        assert_eq!(
            task_rules.len(),
            1,
            "exactly one rule for `task` (the sub-spec's allow)"
        );
        assert_eq!(task_rules[0].action, PermissionAction::Allow);
    }

    /// Scenario: the doc's "default-deny todowrite" rule fires when the
    /// sub-spec does NOT list `todowrite`. A regression that drops this
    /// step would let a child quietly write to the parent's todo list.
    #[test]
    fn child_ruleset_appends_default_deny_for_todowrite_when_omitted() {
        let parent = Vec::new();
        let sub_spec = spec_with_tools(["read_file"], []);
        let effective = child_ruleset(&parent, &sub_spec);

        let todowrite_deny = effective.iter().any(|r| {
            r.permission == "todowrite" && r.pattern == "*" && r.action == PermissionAction::Deny
        });
        assert!(
            todowrite_deny,
            "default-deny todowrite must be appended when the sub-spec omits it"
        );
    }

    /// Scenario: the parent denies `edit` on `*`. The sub-spec opts the
    /// child into `edit: allow`. `assert_no_escalation` MUST refuse —
    /// per the doc the parent's blanket deny is a hard refusal that no
    /// child can override.
    #[test]
    fn assert_no_escalation_refuses_child_allow_over_parent_wildcard_deny() {
        let parent = vec![rule("edit", "*", PermissionAction::Deny)];
        let sub_spec = spec_with_tools(["edit"], []);
        let effective = child_ruleset(&parent, &sub_spec);

        let result = assert_no_escalation(&parent, &effective, &["edit"], &["*", "src/**"]);
        assert!(result.is_err());
        let (perm, pat) = result.unwrap_err();
        assert_eq!(perm, "edit");
        // The first failing pair surfaces; either pattern proves the
        // gate fired.
        assert!(pat == "*" || pat == "src/**");
    }

    /// Scenario: the parent has no rules and the child's own rules do
    /// not introduce escalations. `assert_no_escalation` returns `Ok`
    /// — a child that legitimately declares its own capabilities is
    /// allowed.
    #[test]
    fn assert_no_escalation_allows_child_capability_when_parent_silent() {
        let parent: PermissionRuleset = Vec::new();
        let sub_spec = spec_with_tools(["read_file"], []);
        let effective = child_ruleset(&parent, &sub_spec);

        let result = assert_no_escalation(&parent, &effective, &["read_file"], &["*"]);
        assert!(result.is_ok());
    }
}
