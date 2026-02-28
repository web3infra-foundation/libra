use serde_json::Value;

/// Verdict for a tool ACL check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AclVerdict {
    Allow,
    Deny(String),
}

/// Check tool access against the ACL rules.
///
/// Deny rules are checked first (deny takes priority). Then allow rules
/// are checked. If no allow rule matches, the tool is denied.
pub fn check_tool_acl(
    acl: &crate::internal::ai::intentspec::types::ToolAcl,
    tool_name: &str,
    action: &str,
) -> AclVerdict {
    // Check deny rules first
    for rule in &acl.deny {
        if matches_rule(&rule.tool, tool_name)
            && (rule.actions.contains(&"*".to_string())
                || rule.actions.iter().any(|a| a == action))
        {
            if let Some(reason) = check_deny_constraints(&rule.constraints, tool_name, action) {
                return AclVerdict::Deny(reason);
            }
            return AclVerdict::Deny(format!(
                "tool '{}' action '{}' denied by rule",
                tool_name, action
            ));
        }
    }

    // Check allow rules
    for rule in &acl.allow {
        if matches_rule(&rule.tool, tool_name)
            && (rule.actions.contains(&"*".to_string())
                || rule.actions.iter().any(|a| a == action))
        {
            return AclVerdict::Allow;
        }
    }

    // No allow rule matched → deny
    AclVerdict::Deny(format!(
        "no allow rule for tool '{}' action '{}'",
        tool_name, action
    ))
}

/// Check deny constraints for additional matching.
fn check_deny_constraints(
    constraints: &std::collections::BTreeMap<String, Value>,
    _tool_name: &str,
    _action: &str,
) -> Option<String> {
    if let Some(Value::Array(substrings)) = constraints.get("denySubstrings") {
        let reasons: Vec<String> = substrings
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        if !reasons.is_empty() {
            return Some(format!("denied substrings: {}", reasons.join(", ")));
        }
    }
    None
}

fn matches_rule(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Support simple glob: "workspace.*" matches "workspace.fs"
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return tool_name.starts_with(prefix)
            && tool_name
                .get(prefix.len()..)
                .is_some_and(|rest| rest.starts_with('.'));
    }
    pattern == tool_name
}

/// Verdict for a scope check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeVerdict {
    InScope,
    OutOfScope(String),
}

/// Check whether a path is within the defined scope boundaries.
///
/// A path is out-of-scope if it matches any `out_of_scope` pattern.
/// A path is in-scope if `in_scope` is empty, or it matches any `in_scope` pattern.
pub fn check_scope(in_scope: &[String], out_of_scope: &[String], path: &str) -> ScopeVerdict {
    // Check out-of-scope first (takes priority)
    for pattern in out_of_scope {
        if glob_matches(pattern, path) {
            return ScopeVerdict::OutOfScope(format!("matches out-of-scope pattern '{}'", pattern));
        }
    }

    // If no in_scope defined, everything is in scope
    if in_scope.is_empty() {
        return ScopeVerdict::InScope;
    }

    // Check if path matches any in_scope pattern
    for pattern in in_scope {
        if glob_matches(pattern, path) {
            return ScopeVerdict::InScope;
        }
    }

    ScopeVerdict::OutOfScope(format!("path '{}' not in any in-scope pattern", path))
}

/// Simple glob matching: supports trailing `*` and prefix matching with `/`.
fn glob_matches(pattern: &str, path: &str) -> bool {
    if pattern == "*" || pattern == "**" {
        return true;
    }

    // "src/" matches "src/foo.rs" and "src/bar/baz.rs"
    if pattern.ends_with('/') {
        return path.starts_with(pattern) || path == pattern.trim_end_matches('/');
    }

    // "src/**" matches anything under src/
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path.starts_with(prefix)
            && path.get(prefix.len()..).is_some_and(|r| r.starts_with('/') || r.is_empty());
    }

    // "*.rs" matches "foo.rs"
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path.ends_with(suffix);
    }

    // Exact match
    pattern == path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::types::{ToolAcl, ToolRule};
    use std::collections::BTreeMap;

    fn make_rule(tool: &str, actions: &[&str]) -> ToolRule {
        ToolRule {
            tool: tool.into(),
            actions: actions.iter().map(|s| (*s).into()).collect(),
            constraints: BTreeMap::new(),
        }
    }

    #[test]
    fn test_allow_rule_matches() {
        let acl = ToolAcl {
            allow: vec![make_rule("workspace.fs", &["read", "write"])],
            deny: vec![],
        };
        assert_eq!(
            check_tool_acl(&acl, "workspace.fs", "read"),
            AclVerdict::Allow
        );
    }

    #[test]
    fn test_deny_rule_priority() {
        let acl = ToolAcl {
            allow: vec![make_rule("workspace.fs", &["*"])],
            deny: vec![make_rule("workspace.fs", &["write"])],
        };
        assert!(matches!(
            check_tool_acl(&acl, "workspace.fs", "write"),
            AclVerdict::Deny(_)
        ));
    }

    #[test]
    fn test_no_allow_rule_denies() {
        let acl = ToolAcl {
            allow: vec![make_rule("workspace.fs", &["read"])],
            deny: vec![],
        };
        assert!(matches!(
            check_tool_acl(&acl, "shell", "execute"),
            AclVerdict::Deny(_)
        ));
    }

    #[test]
    fn test_wildcard_tool_match() {
        let acl = ToolAcl {
            allow: vec![make_rule("workspace.*", &["read"])],
            deny: vec![],
        };
        assert_eq!(
            check_tool_acl(&acl, "workspace.fs", "read"),
            AclVerdict::Allow
        );
    }

    #[test]
    fn test_wildcard_action_match() {
        let acl = ToolAcl {
            allow: vec![make_rule("shell", &["*"])],
            deny: vec![],
        };
        assert_eq!(
            check_tool_acl(&acl, "shell", "execute"),
            AclVerdict::Allow
        );
    }

    #[test]
    fn test_deny_with_constraints() {
        let mut constraints = BTreeMap::new();
        constraints.insert(
            "denySubstrings".into(),
            Value::Array(vec![Value::String("rm -rf".into())]),
        );
        let acl = ToolAcl {
            allow: vec![make_rule("shell", &["*"])],
            deny: vec![ToolRule {
                tool: "shell".into(),
                actions: vec!["execute".into()],
                constraints,
            }],
        };
        let verdict = check_tool_acl(&acl, "shell", "execute");
        assert!(matches!(verdict, AclVerdict::Deny(reason) if reason.contains("denied substrings")));
    }

    // Scope tests

    #[test]
    fn test_scope_in_scope() {
        let verdict = check_scope(
            &["src/".into()],
            &["vendor/".into()],
            "src/main.rs",
        );
        assert_eq!(verdict, ScopeVerdict::InScope);
    }

    #[test]
    fn test_scope_out_of_scope_pattern() {
        let verdict = check_scope(
            &["src/".into()],
            &["vendor/".into()],
            "vendor/lib.rs",
        );
        assert!(matches!(verdict, ScopeVerdict::OutOfScope(_)));
    }

    #[test]
    fn test_scope_not_in_any_pattern() {
        let verdict = check_scope(
            &["src/".into()],
            &[],
            "docs/readme.md",
        );
        assert!(matches!(verdict, ScopeVerdict::OutOfScope(_)));
    }

    #[test]
    fn test_scope_empty_in_scope_allows_all() {
        let verdict = check_scope(&[], &[], "anything/goes.rs");
        assert_eq!(verdict, ScopeVerdict::InScope);
    }

    #[test]
    fn test_scope_out_of_scope_priority() {
        // Path matches both in-scope and out-of-scope; out-of-scope wins
        let verdict = check_scope(
            &["src/".into()],
            &["src/generated/".into()],
            "src/generated/types.rs",
        );
        assert!(matches!(verdict, ScopeVerdict::OutOfScope(_)));
    }

    #[test]
    fn test_glob_star_star() {
        let verdict = check_scope(&["src/**".into()], &[], "src/foo/bar.rs");
        assert_eq!(verdict, ScopeVerdict::InScope);
    }

    #[test]
    fn test_glob_extension() {
        let verdict = check_scope(&["*.rs".into()], &[], "foo.rs");
        assert_eq!(verdict, ScopeVerdict::InScope);
    }

    #[test]
    fn test_glob_no_match_extension() {
        let verdict = check_scope(&["*.rs".into()], &[], "foo.ts");
        assert!(matches!(verdict, ScopeVerdict::OutOfScope(_)));
    }
}
