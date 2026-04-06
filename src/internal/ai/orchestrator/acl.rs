use serde_json::Value;
use wax::Glob;

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
    check_tool_acl_with_context(acl, tool_name, action, None, &[])
}

/// Check tool access against the ACL rules with optional invocation arguments.
pub fn check_tool_acl_with_args(
    acl: &crate::internal::ai::intentspec::types::ToolAcl,
    tool_name: &str,
    action: &str,
    arguments: Option<&Value>,
) -> AclVerdict {
    check_tool_acl_with_context(acl, tool_name, action, arguments, &[])
}

/// Check tool access against ACL rules with invocation arguments and resolved write paths.
pub fn check_tool_acl_with_context(
    acl: &crate::internal::ai::intentspec::types::ToolAcl,
    tool_name: &str,
    action: &str,
    arguments: Option<&Value>,
    paths_written: &[String],
) -> AclVerdict {
    // Check deny rules first
    for rule in &acl.deny {
        if matches_rule(&rule.tool, tool_name)
            && (rule.actions.contains(&"*".to_string()) || rule.actions.iter().any(|a| a == action))
        {
            if let Some(reason) =
                check_deny_constraints(&rule.constraints, arguments, paths_written)
            {
                return AclVerdict::Deny(reason);
            }
            if rule.constraints.is_empty() {
                return AclVerdict::Deny(format!(
                    "tool '{}' action '{}' denied by rule",
                    tool_name, action
                ));
            }
        }
    }

    // Check allow rules
    let mut matched_allow_rule = false;
    let mut first_constraint_error: Option<String> = None;
    for rule in &acl.allow {
        if matches_rule(&rule.tool, tool_name)
            && (rule.actions.contains(&"*".to_string()) || rule.actions.iter().any(|a| a == action))
        {
            matched_allow_rule = true;
            match check_allow_constraints(&rule.constraints, arguments, paths_written) {
                Ok(()) => return AclVerdict::Allow,
                Err(reason) => {
                    if first_constraint_error.is_none() {
                        first_constraint_error = Some(reason);
                    }
                }
            }
        }
    }

    if let Some(reason) = first_constraint_error {
        return AclVerdict::Deny(reason);
    }

    // No allow rule matched → deny
    if matched_allow_rule {
        AclVerdict::Deny(format!(
            "allow rule constraints rejected tool '{}' action '{}'",
            tool_name, action
        ))
    } else {
        AclVerdict::Deny(format!(
            "no allow rule for tool '{}' action '{}'",
            tool_name, action
        ))
    }
}

/// Check deny constraints for additional matching.
fn check_deny_constraints(
    constraints: &std::collections::BTreeMap<String, Value>,
    arguments: Option<&Value>,
    paths_written: &[String],
) -> Option<String> {
    if constraints.is_empty() {
        return Some("tool denied by explicit deny rule".to_string());
    }

    if let Some(Value::Array(substrings)) = constraints.get("denySubstrings")
        && let Some(reason) = substring_violation(arguments, substrings)
    {
        return Some(reason);
    }

    if let Some(Value::Array(roots)) = constraints.get("writeRoots")
        && let Some(reason) = write_roots_violation(paths_written, roots)
    {
        return Some(reason);
    }

    None
}

fn check_allow_constraints(
    constraints: &std::collections::BTreeMap<String, Value>,
    arguments: Option<&Value>,
    paths_written: &[String],
) -> Result<(), String> {
    if let Some(Value::Array(allow_commands)) = constraints.get("allowCommands") {
        let command = arguments.and_then(extract_command).ok_or_else(|| {
            "allowCommands constraint requires a string 'command' argument".to_string()
        })?;
        let matches = allow_commands
            .iter()
            .filter_map(Value::as_str)
            .any(|pattern| {
                let pattern = pattern.trim();
                if let Some(prefix) = pattern.strip_suffix('*') {
                    command.starts_with(prefix.trim())
                } else {
                    command == pattern
                        || command
                            .strip_prefix(pattern)
                            .is_some_and(|rest| rest.starts_with(' '))
                }
            });
        if !matches {
            return Err(format!(
                "command '{}' is not allowed by allowCommands",
                command
            ));
        }
    }

    if let Some(Value::Array(substrings)) = constraints.get("denySubstrings")
        && let Some(reason) = substring_violation(arguments, substrings)
    {
        return Err(reason);
    }

    if let Some(Value::Array(roots)) = constraints.get("writeRoots")
        && let Some(reason) = write_roots_violation(paths_written, roots)
    {
        return Err(reason);
    }

    Ok(())
}

fn substring_violation(arguments: Option<&Value>, substrings: &[Value]) -> Option<String> {
    let rules: Vec<String> = substrings
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_ascii_lowercase()))
        .collect();
    if rules.is_empty() {
        return None;
    }

    let arguments = arguments?;

    let mut string_leaves = Vec::new();
    collect_string_leaves(arguments, &mut string_leaves);
    string_leaves.push(arguments.to_string());
    let haystack = string_leaves.join("\n").to_ascii_lowercase();
    let matched = rules
        .iter()
        .filter(|needle| haystack.contains(needle.as_str()))
        .cloned()
        .collect::<Vec<_>>();

    if !matched.is_empty() {
        return Some(format!("denied substrings matched: {}", matched.join(", ")));
    }

    None
}

fn write_roots_violation(paths_written: &[String], roots: &[Value]) -> Option<String> {
    if paths_written.is_empty() {
        return None;
    }

    let normalized_roots: Vec<String> = roots
        .iter()
        .filter_map(Value::as_str)
        .map(normalize_slash_path)
        .collect();
    if normalized_roots.is_empty() {
        return Some("writeRoots is empty but writes were requested".to_string());
    }

    for path in paths_written.iter().map(normalize_slash_path) {
        let allowed = normalized_roots
            .iter()
            .any(|root| path_matches_root(&path, root));
        if !allowed {
            return Some(format!(
                "path '{}' is outside allowed writeRoots ({})",
                path,
                normalized_roots.join(", ")
            ));
        }
    }

    None
}

fn normalize_slash_path(path: impl Into<String>) -> String {
    let mut normalized = path.into().replace('\\', "/");
    if normalized.starts_with("./") {
        normalized = normalized.trim_start_matches("./").to_string();
    }
    while normalized.ends_with('/') && normalized.len() > 1 {
        normalized.pop();
    }
    normalized
}

fn path_matches_root(path: &str, root: &str) -> bool {
    let path = path.trim_start_matches('/');
    let root = root.trim_start_matches('/').trim_end_matches('/');
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn extract_command(arguments: &Value) -> Option<&str> {
    arguments.get("command").and_then(Value::as_str)
}

fn collect_string_leaves(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => {
            for item in items {
                collect_string_leaves(item, out);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_string_leaves(item, out);
            }
        }
        _ => {}
    }
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

/// Match repository-relative paths against scope patterns.
fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim().replace('\\', "/");
    let path = normalize_slash_path(path);

    if pattern == "*" || pattern == "**" {
        return true;
    }

    // "src/" matches "src/foo.rs" and "src/bar/baz.rs"
    if pattern.ends_with('/') {
        let prefix = pattern.trim_end_matches('/');
        return path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'));
    }

    let pattern = normalize_slash_path(pattern);

    // "src/**" matches anything under src/
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'));
    }

    Glob::new(&pattern)
        .map(|glob| wax::Program::is_match(&glob, path.as_str()))
        .unwrap_or_else(|_| pattern == path)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::internal::ai::intentspec::types::{ToolAcl, ToolRule};

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
        assert_eq!(check_tool_acl(&acl, "shell", "execute"), AclVerdict::Allow);
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
        let verdict = check_tool_acl_with_args(
            &acl,
            "shell",
            "execute",
            Some(&serde_json::json!({ "command": "echo ok && rm -rf /tmp/test" })),
        );
        assert!(
            matches!(verdict, AclVerdict::Deny(reason) if reason.contains("denied substrings matched"))
        );
    }

    #[test]
    fn test_deny_with_constraints_allows_safe_invocation() {
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
        let verdict = check_tool_acl_with_args(
            &acl,
            "shell",
            "execute",
            Some(&serde_json::json!({ "command": "echo safe" })),
        );
        assert_eq!(verdict, AclVerdict::Allow);
    }

    #[test]
    fn test_allow_constraints_allow_commands() {
        let mut constraints = BTreeMap::new();
        constraints.insert(
            "allowCommands".into(),
            Value::Array(vec![Value::String("cargo test".into())]),
        );
        let acl = ToolAcl {
            allow: vec![ToolRule {
                tool: "shell".into(),
                actions: vec!["execute".into()],
                constraints,
            }],
            deny: vec![],
        };
        let ok = check_tool_acl_with_args(
            &acl,
            "shell",
            "execute",
            Some(&serde_json::json!({ "command": "cargo test -p libra" })),
        );
        assert_eq!(ok, AclVerdict::Allow);

        let denied = check_tool_acl_with_args(
            &acl,
            "shell",
            "execute",
            Some(&serde_json::json!({ "command": "npm test" })),
        );
        assert!(matches!(denied, AclVerdict::Deny(reason) if reason.contains("allowCommands")));
    }

    #[test]
    fn test_allow_constraints_write_roots() {
        let mut constraints = BTreeMap::new();
        constraints.insert(
            "writeRoots".into(),
            Value::Array(vec![Value::String("src".into())]),
        );
        let acl = ToolAcl {
            allow: vec![ToolRule {
                tool: "workspace.fs".into(),
                actions: vec!["write".into()],
                constraints,
            }],
            deny: vec![],
        };

        let allowed = check_tool_acl_with_context(
            &acl,
            "workspace.fs",
            "write",
            None,
            &["src/lib.rs".into()],
        );
        assert_eq!(allowed, AclVerdict::Allow);

        let denied = check_tool_acl_with_context(
            &acl,
            "workspace.fs",
            "write",
            None,
            &["docs/readme.md".into()],
        );
        assert!(matches!(denied, AclVerdict::Deny(reason) if reason.contains("writeRoots")));
    }

    // Scope tests

    #[test]
    fn test_scope_in_scope() {
        let verdict = check_scope(&["src/".into()], &["vendor/".into()], "src/main.rs");
        assert_eq!(verdict, ScopeVerdict::InScope);
    }

    #[test]
    fn test_scope_out_of_scope_pattern() {
        let verdict = check_scope(&["src/".into()], &["vendor/".into()], "vendor/lib.rs");
        assert!(matches!(verdict, ScopeVerdict::OutOfScope(_)));
    }

    #[test]
    fn test_scope_not_in_any_pattern() {
        let verdict = check_scope(&["src/".into()], &[], "docs/readme.md");
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
    fn test_glob_nested_pattern() {
        let verdict = check_scope(&["src/**/*.rs".into()], &[], "src/foo/bar.rs");
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
