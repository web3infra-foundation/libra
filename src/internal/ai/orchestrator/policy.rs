//! Execution policy helpers for orchestrated AI runs.
//!
//! Boundary: this module computes allowed paths, commands, and workspace constraints;
//! concrete process execution and object persistence live in sibling modules. ACL and
//! hardening tests cover traversal, cargo-lock companion, and denied command cases.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{
    acl::{
        AclVerdict, ScopeVerdict, cargo_lock_companion_allowed, check_scope,
        check_tool_acl_with_context,
    },
    types::{PolicyViolation, TaskKind, TaskSpec, ToolCallRecord, ToolDiffRecord},
};
use crate::internal::ai::{
    intentspec::types::{DependencyPolicy, IntentSpec, NetworkPolicy},
    libra_vcs::unsupported_command_message,
    tools::{
        ToolOutput,
        apply_patch::{ApplyPatchArgs, parse_patch},
        utils::command_invokes_git_version_control,
    },
};

#[derive(Clone, Debug)]
pub struct ToolPreflight {
    pub record: ToolCallRecord,
}

pub fn evaluate_tool_call(
    spec: &IntentSpec,
    task: &TaskSpec,
    tool_name: &str,
    arguments: &Value,
    working_dir: &Path,
) -> Result<ToolPreflight, PolicyViolation> {
    let (acl_tool, action, reads, writes) =
        derive_tool_footprint(tool_name, arguments, working_dir).map_err(|message| {
            PolicyViolation {
                code: "invalid-tool-arguments".into(),
                message,
                tool_name: Some(tool_name.to_string()),
                path: None,
            }
        })?;

    match check_tool_acl_with_context(
        &spec.security.tool_acl,
        &acl_tool,
        &action,
        Some(arguments),
        &writes,
    ) {
        AclVerdict::Allow => {}
        AclVerdict::Deny(reason) => {
            if gate_shell_uses_internal_verification_allowance(task, tool_name, &reason) {
                // Gate tasks execute spec-defined verification commands directly, so
                // they do not need the interactive shell ACL that governs agent-chosen
                // tool calls.
            } else if terminal_handshake_allowance(task, tool_name, &reason) {
                // `submit_task_complete` is the runtime↔agent terminal handshake that
                // ends the tool loop; it is always exposed to Implementation/Analysis
                // tasks (see executor::allowed_tools_for_task) and must not be gated by
                // user-authored IntentSpec ACLs. An explicit deny rule still wins —
                // only the implicit "no allow rule" verdict is relaxed here.
            } else {
                return Err(PolicyViolation {
                    code: "tool-acl-deny".into(),
                    message: reason,
                    tool_name: Some(tool_name.to_string()),
                    path: None,
                });
            }
        }
    }

    if tool_name == "web_search" && spec.constraints.security.network_policy == NetworkPolicy::Deny
    {
        return Err(PolicyViolation {
            code: "network-policy-deny".into(),
            message: "web_search requires network access while networkPolicy=deny".into(),
            tool_name: Some(tool_name.to_string()),
            path: None,
        });
    }

    if tool_name == "shell" {
        if shell_invokes_git_version_control(arguments) {
            return Err(PolicyViolation {
                code: "git-version-control-deny".into(),
                message: "git is not allowed for Libra-managed agent execution; use run_libra_vcs or Libra version-control commands instead".into(),
                tool_name: Some(tool_name.to_string()),
                path: None,
            });
        }

        if shell_requests_escalation(arguments) {
            return Err(PolicyViolation {
                code: "sandbox-escalation-deny".into(),
                message: "shell escalation is not allowed for orchestrator-managed tasks".into(),
                tool_name: Some(tool_name.to_string()),
                path: None,
            });
        }

        if spec.constraints.security.network_policy == NetworkPolicy::Deny
            && shell_looks_networked(arguments)
        {
            return Err(PolicyViolation {
                code: "network-policy-deny".into(),
                message: "shell command appears to require network access while networkPolicy=deny"
                    .into(),
                tool_name: Some(tool_name.to_string()),
                path: None,
            });
        }
    }

    validate_task_write_contract(task, tool_name, &writes)?;

    Ok(ToolPreflight {
        record: ToolCallRecord {
            tool_name: tool_name.to_string(),
            action,
            arguments_json: Some(arguments.clone()),
            paths_read: reads,
            paths_written: writes,
            success: false,
            summary: None,
            diffs: Vec::new(),
        },
    })
}

fn gate_shell_uses_internal_verification_allowance(
    task: &TaskSpec,
    tool_name: &str,
    reason: &str,
) -> bool {
    task.kind == TaskKind::Gate
        && tool_name == "shell"
        && reason.starts_with("no allow rule for tool 'shell' action 'execute'")
}

fn terminal_handshake_allowance(task: &TaskSpec, tool_name: &str, reason: &str) -> bool {
    matches!(task.kind, TaskKind::Implementation | TaskKind::Analysis)
        && tool_name == "submit_task_complete"
        && reason.starts_with("no allow rule for tool 'submit_task_complete' action 'execute'")
}

pub fn evaluate_tool_result(
    spec: &IntentSpec,
    task: &TaskSpec,
    tool_name: &str,
    output: &ToolOutput,
    record: &mut ToolCallRecord,
) -> Result<(), PolicyViolation> {
    record.success = output.is_success();
    record.summary = output
        .as_text()
        .map(|text| text.lines().next().unwrap_or_default().trim().to_string())
        .filter(|summary| !summary.is_empty());
    if let Some(meta) = output.metadata() {
        if tool_name == "apply_patch" || tool_name == "shell" {
            record.diffs = extract_patch_diffs(meta);
        }
        if tool_name == "shell" {
            record.paths_written = extract_written_paths(meta);
        }
    }

    if tool_name == "shell" && !record.paths_written.is_empty() {
        validate_recorded_writes(spec, task, record)?;
    }

    if output.is_success() {
        validate_dependency_policy(spec, tool_name, record)?;
    }

    if spec.security.output_handling.no_direct_eval
        && tool_name == "apply_patch"
        && let Some(meta) = output.metadata()
        && patch_metadata_looks_unsafe(meta)
    {
        return Err(PolicyViolation {
            code: "unsafe-direct-eval".into(),
            message: "patch introduces potentially unsafe direct execution patterns".into(),
            tool_name: Some(tool_name.to_string()),
            path: extract_first_diff_path(meta),
        });
    }

    let acl_tool_name = acl_tool_alias(&record.tool_name);
    if let Some(limit) = max_output_limit(spec, acl_tool_name, &record.action)
        && let Some(text) = output.as_text()
        && text.len() > limit
    {
        let output_bytes = text.len();
        return Err(PolicyViolation {
            code: "tool-output-too-large".into(),
            message: format!(
                "tool output exceeds maxOutputBytes constraint ({} > {})",
                output_bytes, limit
            ),
            tool_name: Some(tool_name.to_string()),
            path: None,
        });
    }

    Ok(())
}

fn validate_dependency_policy(
    spec: &IntentSpec,
    tool_name: &str,
    record: &ToolCallRecord,
) -> Result<(), PolicyViolation> {
    if spec.constraints.security.dependency_policy != DependencyPolicy::NoNew {
        return Ok(());
    }

    for diff in &record.diffs {
        if cargo_manifest_diff_adds_dependency(diff) {
            return Err(PolicyViolation {
                code: "dependency-policy-no-new".into(),
                message: format!(
                    "dependency-policy:no-new forbids adding new dependencies in '{}'",
                    diff.path
                ),
                tool_name: Some(tool_name.to_string()),
                path: Some(diff.path.clone()),
            });
        }
    }

    Ok(())
}

fn cargo_manifest_diff_adds_dependency(diff: &ToolDiffRecord) -> bool {
    if !is_cargo_manifest_path(&diff.path) {
        return false;
    }

    let mut current_dependency_collection = None;
    let mut added_dependency_tables = BTreeSet::new();
    let mut removed_dependency_tables = BTreeSet::new();
    let mut added_keys_by_table: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut removed_keys_by_table: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for raw_line in diff.diff.lines() {
        let (is_added, is_removed, line) = match raw_line.as_bytes().first() {
            Some(b'+') => (true, false, &raw_line[1..]),
            Some(b'-') => (false, true, &raw_line[1..]),
            Some(b' ') => (false, false, &raw_line[1..]),
            _ => (false, false, raw_line),
        };
        let trimmed = line.trim();

        if let Some(table) = toml_table_header(trimmed) {
            let normalized_table = normalize_toml_table_name(table);
            if is_dependency_declaration_table_name(&normalized_table) {
                if is_added {
                    added_dependency_tables.insert(normalized_table.clone());
                } else if is_removed {
                    removed_dependency_tables.insert(normalized_table.clone());
                }
            }
            current_dependency_collection =
                is_dependency_collection_table_name(&normalized_table).then_some(normalized_table);
            continue;
        }

        if let Some(table) = current_dependency_collection.as_ref()
            && let Some(key) = toml_dependency_key(trimmed)
        {
            if is_added {
                added_keys_by_table
                    .entry(table.clone())
                    .or_default()
                    .insert(key);
            } else if is_removed {
                removed_keys_by_table
                    .entry(table.clone())
                    .or_default()
                    .insert(key);
            }
        }
    }

    if added_dependency_tables
        .iter()
        .any(|table| !removed_dependency_tables.contains(table))
    {
        return true;
    }

    for (table, added_keys) in added_keys_by_table {
        let removed_keys = removed_keys_by_table.get(&table);
        if added_keys
            .iter()
            .any(|key| removed_keys.is_none_or(|removed| !removed.contains(key)))
        {
            return true;
        }
    }

    false
}

fn is_cargo_manifest_path(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .is_some_and(|name| name == "Cargo.toml")
}

fn toml_table_header(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?.trim();
    if inner.starts_with('[') || inner.ends_with(']') {
        return None;
    }
    Some(inner)
}

fn is_dependency_collection_table_name(normalized: &str) -> bool {
    matches!(
        normalized,
        "dependencies"
            | "dev-dependencies"
            | "build-dependencies"
            | "workspace.dependencies"
            | "workspace.dev-dependencies"
            | "workspace.build-dependencies"
    ) || (normalized.starts_with("target.")
        && (normalized.ends_with(".dependencies")
            || normalized.ends_with(".dev-dependencies")
            || normalized.ends_with(".build-dependencies")))
}

fn is_dependency_declaration_table_name(normalized: &str) -> bool {
    normalized.starts_with("dependencies.")
        || normalized.starts_with("dev-dependencies.")
        || normalized.starts_with("build-dependencies.")
        || normalized.starts_with("workspace.dependencies.")
        || normalized.starts_with("workspace.dev-dependencies.")
        || normalized.starts_with("workspace.build-dependencies.")
        || (normalized.starts_with("target.")
            && (normalized.contains(".dependencies.")
                || normalized.contains(".dev-dependencies.")
                || normalized.contains(".build-dependencies.")))
}

fn normalize_toml_table_name(table: &str) -> String {
    table
        .chars()
        .filter(|ch| *ch != '"' && *ch != '\'')
        .collect::<String>()
}

fn toml_line_adds_key(line: &str) -> bool {
    !line.is_empty() && !line.starts_with('#') && !line.starts_with('[') && line.contains('=')
}

fn toml_dependency_key(line: &str) -> Option<String> {
    if !toml_line_adds_key(line) {
        return None;
    }
    let key = line.split_once('=')?.0.trim();
    let normalized = normalize_toml_key_name(key);
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_toml_key_name(key: &str) -> String {
    key.chars()
        .filter(|ch| !matches!(*ch, '"' | '\'') && !ch.is_whitespace())
        .collect::<String>()
}

fn acl_tool_alias(tool_name: &str) -> &str {
    match tool_name {
        "read_file" | "list_dir" | "grep_files" | "search_files" | "apply_patch" => "workspace.fs",
        "web_search" => "web.search",
        "request_user_input" => "interaction",
        "submit_intent_draft" | "submit_plan_draft" => "planning",
        _ => tool_name,
    }
}

fn max_output_limit(spec: &IntentSpec, tool_name: &str, action: &str) -> Option<usize> {
    spec.security
        .tool_acl
        .allow
        .iter()
        .filter(|rule| rule.tool == tool_name || rule.tool == "*")
        .filter(|rule| {
            rule.actions
                .iter()
                .any(|value| value == action || value == "*")
        })
        .filter_map(|rule| rule.constraints.get("maxOutputBytes"))
        .filter_map(|value| value.as_u64())
        .map(|value| value as usize)
        .min()
}

fn extract_patch_diffs(meta: &Value) -> Vec<ToolDiffRecord> {
    meta.get("diffs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(ToolDiffRecord {
                path: entry.get("path")?.as_str()?.to_string(),
                change_type: entry
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("modify")
                    .to_string(),
                diff: entry.get("diff")?.as_str()?.to_string(),
            })
        })
        .collect()
}

fn extract_written_paths(meta: &Value) -> Vec<String> {
    let mut paths = meta
        .get("paths_written")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    for diff in extract_patch_diffs(meta) {
        if !paths.iter().any(|path| path == &diff.path) {
            paths.push(diff.path);
        }
    }

    paths
}

fn validate_recorded_writes(
    spec: &IntentSpec,
    task: &TaskSpec,
    record: &ToolCallRecord,
) -> Result<(), PolicyViolation> {
    let acl_tool = acl_tool_alias(&record.tool_name);
    match check_tool_acl_with_context(
        &spec.security.tool_acl,
        acl_tool,
        &record.action,
        record.arguments_json.as_ref(),
        &record.paths_written,
    ) {
        AclVerdict::Allow => {}
        AclVerdict::Deny(reason) => {
            return Err(PolicyViolation {
                code: "tool-acl-deny".into(),
                message: reason,
                tool_name: Some(record.tool_name.clone()),
                path: record.paths_written.first().cloned(),
            });
        }
    }

    validate_task_write_contract(task, &record.tool_name, &record.paths_written)?;

    Ok(())
}

fn validate_task_write_contract(
    task: &TaskSpec,
    tool_name: &str,
    paths_written: &[String],
) -> Result<(), PolicyViolation> {
    for path in paths_written {
        if let Some(reason) = task_write_contract_violation(task, path) {
            return Err(PolicyViolation {
                code: "scope-creep".into(),
                message: reason,
                tool_name: Some(tool_name.to_string()),
                path: Some(path.clone()),
            });
        }
    }

    Ok(())
}

fn task_write_contract_violation(task: &TaskSpec, path: &str) -> Option<String> {
    if !task.contract.touch_files.is_empty() {
        if let ScopeVerdict::OutOfScope(reason) = check_scope(&[], &task.scope_out, path) {
            return Some(reason);
        }
        if cargo_lock_companion_allowed(&task.contract.touch_files, path) {
            return None;
        }
        return match check_scope(&task.contract.touch_files, &[], path) {
            ScopeVerdict::InScope => None,
            ScopeVerdict::OutOfScope(reason) => Some(format!("not in touchFiles: {reason}")),
        };
    }

    if let ScopeVerdict::OutOfScope(reason) = check_scope(&[], &task.scope_out, path) {
        return Some(reason);
    }
    if cargo_lock_companion_allowed(&task.scope_in, path) {
        return None;
    }
    match check_scope(&task.scope_in, &task.scope_out, path) {
        ScopeVerdict::InScope => None,
        ScopeVerdict::OutOfScope(reason) => Some(reason),
    }
}

fn derive_tool_footprint(
    tool_name: &str,
    arguments: &Value,
    working_dir: &Path,
) -> Result<(String, String, Vec<String>, Vec<String>), String> {
    match tool_name {
        "read_file" => {
            let path = required_string(arguments, "file_path")?;
            Ok((
                "workspace.fs".into(),
                "read".into(),
                vec![normalize_path(path, working_dir)],
                Vec::new(),
            ))
        }
        "list_dir" => {
            let path = required_string(arguments, "dir_path")?;
            Ok((
                "workspace.fs".into(),
                "read".into(),
                vec![normalize_path(path, working_dir)],
                Vec::new(),
            ))
        }
        "grep_files" | "search_files" => {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| working_dir.to_string_lossy().to_string());
            Ok((
                "workspace.fs".into(),
                "read".into(),
                vec![normalize_path(path, working_dir)],
                Vec::new(),
            ))
        }
        "web_search" => Ok(("web.search".into(), "query".into(), Vec::new(), Vec::new())),
        "apply_patch" => {
            let patch_text = parse_patch_text(arguments)?;
            let patch = parse_patch(&patch_text).map_err(|e| e.to_string())?;
            let writes = patch
                .hunks
                .iter()
                .flat_map(|hunk| hunk.all_resolved_paths(working_dir))
                .map(|path| relative_or_display(path, working_dir))
                .collect::<Vec<_>>();
            Ok(("workspace.fs".into(), "write".into(), Vec::new(), writes))
        }
        "shell" => Ok(("shell".into(), "execute".into(), Vec::new(), Vec::new())),
        "run_libra_vcs" => {
            let command = required_string(arguments, "command")?;
            Ok((
                "libra.vcs".into(),
                libra_vcs_action(command)?.into(),
                Vec::new(),
                Vec::new(),
            ))
        }
        "request_user_input" => Ok((
            "interaction".into(),
            "prompt".into(),
            Vec::new(),
            Vec::new(),
        )),
        "submit_intent_draft" | "submit_plan_draft" => {
            Ok(("planning".into(), "submit".into(), Vec::new(), Vec::new()))
        }
        other => Ok((other.to_string(), "execute".into(), Vec::new(), Vec::new())),
    }
}

fn parse_patch_text(arguments: &Value) -> Result<String, String> {
    match arguments {
        Value::String(raw) => Ok(raw.clone()),
        Value::Object(_) => serde_json::from_value::<ApplyPatchArgs>(arguments.clone())
            .map(|args| args.input)
            .or_else(|_| serde_json::from_value::<String>(arguments.clone()))
            .map_err(|e| e.to_string()),
        _ => serde_json::from_value::<String>(arguments.clone()).map_err(|e| e.to_string()),
    }
}

fn required_string<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing required string argument '{}'", key))
}

fn normalize_path(path: impl Into<String>, working_dir: &Path) -> String {
    let raw = PathBuf::from(path.into());
    relative_or_display(
        if raw.is_absolute() {
            raw
        } else {
            working_dir.join(raw)
        },
        working_dir,
    )
}

fn relative_or_display(path: PathBuf, working_dir: &Path) -> String {
    path.strip_prefix(working_dir)
        .map(|rel| rel.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

fn shell_looks_networked(arguments: &Value) -> bool {
    let command = arguments
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let needles = [
        "curl ",
        "wget ",
        "http://",
        "https://",
        "npm install",
        "pnpm add",
        "yarn add",
        "cargo add",
        "pip install",
        "git fetch",
    ];
    needles.iter().any(|needle| command.contains(needle))
}

fn shell_invokes_git_version_control(arguments: &Value) -> bool {
    arguments
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(command_invokes_git_version_control)
}

fn libra_vcs_action(command: &str) -> Result<&'static str, String> {
    match command.trim() {
        "status" | "diff" | "branch" | "log" | "show" | "show-ref" => Ok("read"),
        "add" | "commit" | "switch" => Ok("write"),
        "" => Err("missing run_libra_vcs command".to_string()),
        other => Err(unsupported_command_message("run_libra_vcs", other)),
    }
}

fn shell_requests_escalation(arguments: &Value) -> bool {
    arguments
        .get("sandbox_permissions")
        .and_then(Value::as_str)
        .map(|value| {
            let normalized = value.to_ascii_lowercase();
            normalized == "require_escalated" || normalized == "require-escalated"
        })
        .unwrap_or(false)
}

fn patch_metadata_looks_unsafe(metadata: &Value) -> bool {
    let diffs = metadata
        .get("diffs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let banned = [
        "eval(",
        "exec(",
        "os.system(",
        "subprocess.call(",
        "subprocess.run(",
        "shell=true",
    ];

    diffs.iter().any(|diff| {
        diff.get("diff")
            .and_then(Value::as_str)
            .map(|text| {
                let normalized = text.to_ascii_lowercase();
                banned.iter().any(|needle| normalized.contains(needle))
            })
            .unwrap_or(false)
    })
}

fn extract_first_diff_path(metadata: &Value) -> Option<String> {
    metadata
        .get("diffs")
        .and_then(Value::as_array)
        .and_then(|diffs| diffs.first())
        .and_then(|diff| diff.get("path"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use git_internal::internal::object::{task::Task as GitTask, types::ActorRef};
    use tempfile::tempdir;

    use super::*;
    use crate::internal::ai::{
        intentspec::types::{
            ConstraintLicensing, ConstraintPlatform, ConstraintPrivacy, ConstraintResources,
            ConstraintSecurity, Constraints, CreatedBy, CreatorType, DependencyPolicy,
            DomainAllowlistMode, EncodingPolicy, EvidencePolicy, EvidenceStrategy, HumanInLoop,
            Intent, Metadata, NetworkPolicy, OutputHandlingPolicy, PromptInjectionPolicy,
            QualityGates, RepoTarget, RepoType, Risk, RiskLevel, SecretAccessPolicy, SecretPolicy,
            SecurityPolicy, Target, ToolAcl, ToolRule, TouchHints, TrustTier,
        },
        orchestrator::types::{TaskContract, TaskKind, TaskSpec, ToolCallRecord},
    };

    fn spec() -> IntentSpec {
        IntentSpec {
            api_version: "intentspec.io/v1alpha1".into(),
            kind: "IntentSpec".into(),
            metadata: Metadata {
                id: "id".into(),
                created_at: "2025-01-01T00:00:00Z".into(),
                created_by: CreatedBy {
                    creator_type: CreatorType::User,
                    id: "user".into(),
                    display_name: None,
                },
                target: Target {
                    repo: RepoTarget {
                        repo_type: RepoType::Local,
                        locator: "/tmp".into(),
                    },
                    base_ref: "HEAD".into(),
                    workspace_id: None,
                    labels: BTreeMap::new(),
                },
            },
            intent: Intent {
                summary: "summary".into(),
                problem_statement: "problem".into(),
                change_type: crate::internal::ai::intentspec::types::ChangeType::Feature,
                objectives: vec![crate::internal::ai::intentspec::types::Objective {
                    title: "obj".into(),
                    kind: crate::internal::ai::intentspec::types::ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src/".into()],
                out_of_scope: vec!["vendor/".into()],
                touch_hints: Some(TouchHints {
                    files: vec![],
                    symbols: vec![],
                    apis: vec![],
                }),
            },
            acceptance: crate::internal::ai::intentspec::types::Acceptance {
                success_criteria: vec!["done".into()],
                verification_plan: crate::internal::ai::intentspec::types::VerificationPlan {
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                quality_gates: Some(QualityGates {
                    require_new_tests_when_bugfix: Some(true),
                    max_allowed_regression: None,
                }),
            },
            constraints: Constraints {
                security: ConstraintSecurity {
                    network_policy: NetworkPolicy::Deny,
                    dependency_policy: DependencyPolicy::NoNew,
                    crypto_policy: String::new(),
                },
                privacy: ConstraintPrivacy {
                    data_classes_allowed: vec![
                        crate::internal::ai::intentspec::types::DataClass::Public,
                    ],
                    redaction_required: false,
                    retention_days: 30,
                },
                licensing: ConstraintLicensing {
                    allowed_spdx: vec![],
                    forbid_new_licenses: false,
                },
                platform: ConstraintPlatform {
                    language_runtime: "rust".into(),
                    supported_os: vec![],
                },
                resources: ConstraintResources {
                    max_wall_clock_seconds: 60,
                    max_cost_units: 10,
                },
            },
            risk: Risk {
                level: RiskLevel::Low,
                rationale: "low".into(),
                factors: vec![],
                human_in_loop: HumanInLoop {
                    required: false,
                    min_approvers: 0,
                },
            },
            evidence: EvidencePolicy {
                strategy: EvidenceStrategy::RepoFirst,
                trust_tiers: vec![TrustTier::Repo],
                domain_allowlist_mode: DomainAllowlistMode::Disabled,
                allowed_domains: vec![],
                blocked_domains: vec![],
                min_citations_per_decision: 1,
            },
            security: SecurityPolicy {
                tool_acl: ToolAcl {
                    allow: vec![
                        ToolRule {
                            tool: "workspace.fs".into(),
                            actions: vec!["read".into(), "write".into()],
                            constraints: BTreeMap::new(),
                        },
                        ToolRule {
                            tool: "shell".into(),
                            actions: vec!["execute".into()],
                            constraints: BTreeMap::new(),
                        },
                    ],
                    deny: vec![],
                },
                secrets: SecretPolicy {
                    policy: SecretAccessPolicy::DenyAll,
                    allowed_scopes: vec![],
                },
                prompt_injection: PromptInjectionPolicy {
                    treat_retrieved_content_as_untrusted: true,
                    enforce_output_schema: true,
                    disallow_instruction_from_evidence: true,
                },
                output_handling: OutputHandlingPolicy {
                    encoding_policy: EncodingPolicy::ContextualEscape,
                    no_direct_eval: true,
                },
            },
            execution: crate::internal::ai::intentspec::types::ExecutionPolicy {
                retry: crate::internal::ai::intentspec::types::RetryPolicy {
                    max_retries: 1,
                    backoff_seconds: 0,
                },
                replan: crate::internal::ai::intentspec::types::ReplanPolicy { triggers: vec![] },
                concurrency: crate::internal::ai::intentspec::types::ConcurrencyPolicy {
                    max_parallel_tasks: 1,
                },
            },
            artifacts: crate::internal::ai::intentspec::types::Artifacts {
                required: vec![],
                retention: crate::internal::ai::intentspec::types::ArtifactRetention { days: 30 },
            },
            provenance: crate::internal::ai::intentspec::types::ProvenancePolicy {
                require_slsa_provenance: false,
                require_sbom: false,
                transparency_log: crate::internal::ai::intentspec::types::TransparencyLogPolicy {
                    mode: crate::internal::ai::intentspec::types::TransparencyMode::None,
                },
                bindings: crate::internal::ai::intentspec::types::ProvenanceBindings {
                    embed_intent_spec_digest: false,
                    embed_evidence_digests: false,
                },
            },
            lifecycle: crate::internal::ai::intentspec::types::Lifecycle {
                schema_version: "1.0.0".into(),
                status: crate::internal::ai::intentspec::types::LifecycleStatus::Active,
                change_log: vec![],
            },
            libra: None,
            extensions: BTreeMap::new(),
        }
    }

    fn task() -> TaskSpec {
        let actor = ActorRef::agent("test-policy").unwrap();
        let task = GitTask::new(actor, "edit", None).unwrap();
        TaskSpec {
            step: git_internal::internal::object::plan::PlanStep::new("edit"),
            task,
            objective: "edit file".into(),
            kind: TaskKind::Implementation,
            gate_stage: None,
            owner_role: Some("coder".into()),
            scope_in: vec!["src/".into()],
            scope_out: vec!["vendor/".into()],
            checks: vec![],
            contract: TaskContract::default(),
        }
    }

    fn gate_task() -> TaskSpec {
        TaskSpec {
            kind: TaskKind::Gate,
            owner_role: Some("verifier".into()),
            ..task()
        }
    }

    #[test]
    fn test_scope_violation_rejected() {
        let res = evaluate_tool_call(
            &spec(),
            &task(),
            "apply_patch",
            &serde_json::json!({
                "input": "*** Begin Patch\n*** Add File: vendor/foo.rs\n+fn x() {}\n*** End Patch"
            }),
            Path::new("/tmp/work"),
        );
        assert!(matches!(res, Err(PolicyViolation { code, .. }) if code == "scope-creep"));
    }

    #[test]
    fn test_apply_patch_scope_preflight_uses_relative_path_inside_worktree() {
        let temp = tempdir().unwrap();
        let src_dir = temp.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let target = src_dir.join("lib.rs");

        let res = evaluate_tool_call(
            &spec(),
            &task(),
            "apply_patch",
            &serde_json::json!({
                "input": format!(
                    "*** Begin Patch\n*** Add File: {}\n+pub fn demo() {{}}\n*** End Patch",
                    target.display()
                )
            }),
            temp.path(),
        )
        .expect("absolute path inside isolated worktree should stay in scope");

        assert_eq!(res.record.paths_written, vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn test_apply_patch_preflight_rejects_writes_outside_touch_files() {
        let mut task = task();
        task.contract.touch_files = vec!["src/lib.rs".into()];

        let violation = evaluate_tool_call(
            &spec(),
            &task,
            "apply_patch",
            &serde_json::json!({
                "input": "*** Begin Patch\n*** Add File: src/main.rs\n+fn main() {}\n*** End Patch"
            }),
            Path::new("/tmp/work"),
        )
        .expect_err("writes outside touchFiles must be rejected before execution");

        assert_eq!(violation.code, "scope-creep");
        assert_eq!(violation.path.as_deref(), Some("src/main.rs"));
        assert!(violation.message.contains("not in touchFiles"));
    }

    #[test]
    fn test_network_policy_rejected() {
        let res = evaluate_tool_call(
            &spec(),
            &task(),
            "shell",
            &serde_json::json!({ "command": "curl https://example.com" }),
            Path::new("/tmp/work"),
        );
        assert!(matches!(res, Err(PolicyViolation { code, .. }) if code == "network-policy-deny"));
    }

    #[test]
    fn test_web_search_honors_network_policy() {
        let mut intent = spec();
        intent.security.tool_acl.allow.push(ToolRule {
            tool: "web.search".into(),
            actions: vec!["query".into()],
            constraints: BTreeMap::new(),
        });

        let denied = evaluate_tool_call(
            &intent,
            &task(),
            "web_search",
            &serde_json::json!({ "query": "Rust 2024 edition stable" }),
            Path::new("/tmp/work"),
        )
        .expect_err("web_search should be blocked when networkPolicy=deny");

        assert_eq!(denied.code, "network-policy-deny");

        intent.constraints.security.network_policy = NetworkPolicy::Allow;
        let allowed = evaluate_tool_call(
            &intent,
            &task(),
            "web_search",
            &serde_json::json!({ "query": "Rust 2024 edition stable" }),
            Path::new("/tmp/work"),
        )
        .expect("web_search should be allowed when ACL and network policy allow it");

        assert_eq!(allowed.record.action, "query");
    }

    #[test]
    fn test_shell_git_version_control_is_rejected() {
        let mut intent = spec();
        intent.constraints.security.network_policy = NetworkPolicy::Allow;
        let res = evaluate_tool_call(
            &intent,
            &task(),
            "shell",
            &serde_json::json!({ "command": "git status" }),
            Path::new("/tmp/work"),
        );

        assert!(
            matches!(res, Err(PolicyViolation { code, .. }) if code == "git-version-control-deny")
        );
    }

    #[test]
    fn test_run_libra_vcs_uses_libra_vcs_acl() {
        let mut intent = spec();
        intent.security.tool_acl.allow.push(ToolRule {
            tool: "libra.vcs".into(),
            actions: vec!["read".into(), "write".into()],
            constraints: BTreeMap::new(),
        });

        let preflight = evaluate_tool_call(
            &intent,
            &task(),
            "run_libra_vcs",
            &serde_json::json!({ "command": "status" }),
            Path::new("/tmp/work"),
        )
        .expect("libra status should be allowed by libra.vcs ACL");

        assert_eq!(preflight.record.tool_name, "run_libra_vcs");
        assert_eq!(preflight.record.action, "read");
    }

    #[test]
    fn test_run_libra_vcs_unknown_command_error_is_actionable() {
        let error = libra_vcs_action("ls-files").unwrap_err();

        assert!(error.contains("allowed commands"));
        assert!(error.contains("status --json"));
        assert!(error.contains("workspace file tools"));
    }

    #[test]
    fn test_network_policy_rejects_shell_escalation() {
        let res = evaluate_tool_call(
            &spec(),
            &task(),
            "shell",
            &serde_json::json!({
                "command": "echo hi",
                "sandbox_permissions": "require_escalated",
                "justification": "needs host access",
            }),
            Path::new("/tmp/work"),
        );
        assert!(
            matches!(res, Err(PolicyViolation { code, .. }) if code == "sandbox-escalation-deny")
        );
    }

    #[test]
    fn test_shell_escalation_is_rejected_even_without_justification() {
        let mut intent = spec();
        intent.constraints.security.network_policy = NetworkPolicy::Allow;
        let res = evaluate_tool_call(
            &intent,
            &task(),
            "shell",
            &serde_json::json!({
                "command": "echo hi",
                "sandbox_permissions": "require_escalated",
            }),
            Path::new("/tmp/work"),
        );
        assert!(matches!(
            res,
            Err(PolicyViolation { code, .. }) if code == "sandbox-escalation-deny"
        ));
    }

    #[test]
    fn test_shell_escalation_is_rejected_even_with_justification() {
        let mut intent = spec();
        intent.constraints.security.network_policy = NetworkPolicy::Allow;
        let res = evaluate_tool_call(
            &intent,
            &task(),
            "shell",
            &serde_json::json!({
                "command": "echo hi",
                "sandbox_permissions": "require_escalated",
                "justification": "requires host tools",
            }),
            Path::new("/tmp/work"),
        );
        assert!(matches!(
            res,
            Err(PolicyViolation { code, .. }) if code == "sandbox-escalation-deny"
        ));
    }

    #[test]
    fn test_gate_shell_is_allowed_without_interactive_shell_acl() {
        let mut intent = spec();
        intent
            .security
            .tool_acl
            .allow
            .retain(|rule| rule.tool != "shell");
        let res = evaluate_tool_call(
            &intent,
            &gate_task(),
            "shell",
            &serde_json::json!({ "command": "cargo test --lib" }),
            Path::new("/tmp/work"),
        );
        assert!(res.is_ok(), "{res:?}");
    }

    #[test]
    fn test_gate_shell_still_honors_explicit_shell_denies() {
        let mut intent = spec();
        intent
            .security
            .tool_acl
            .allow
            .retain(|rule| rule.tool != "shell");
        intent.security.tool_acl.deny.push(ToolRule {
            tool: "shell".into(),
            actions: vec!["execute".into()],
            constraints: BTreeMap::new(),
        });
        let res = evaluate_tool_call(
            &intent,
            &gate_task(),
            "shell",
            &serde_json::json!({ "command": "cargo test --lib" }),
            Path::new("/tmp/work"),
        );
        assert!(matches!(res, Err(PolicyViolation { code, .. }) if code == "tool-acl-deny"));
    }

    fn submit_task_complete_args() -> Value {
        serde_json::json!({
            "result": "pass",
            "summary": "all acceptance checks pass",
            "evidence": [
                { "command": "cargo build", "exit_code": 0, "output_excerpt": "Finished" }
            ]
        })
    }

    #[test]
    fn test_submit_task_complete_terminal_handshake_allowed_without_acl() {
        let intent = spec();
        assert!(
            !intent
                .security
                .tool_acl
                .allow
                .iter()
                .any(|rule| rule.tool == "submit_task_complete"),
            "default IntentSpec must not register submit_task_complete in ACL — the runtime exemption is what unblocks it"
        );

        let res = evaluate_tool_call(
            &intent,
            &task(),
            "submit_task_complete",
            &submit_task_complete_args(),
            Path::new("/tmp/work"),
        );
        assert!(res.is_ok(), "{res:?}");
    }

    #[test]
    fn test_submit_task_complete_handshake_applies_to_analysis_tasks() {
        let mut analysis = task();
        analysis.kind = TaskKind::Analysis;
        let res = evaluate_tool_call(
            &spec(),
            &analysis,
            "submit_task_complete",
            &submit_task_complete_args(),
            Path::new("/tmp/work"),
        );
        assert!(res.is_ok(), "{res:?}");
    }

    #[test]
    fn test_submit_task_complete_blocked_for_gate_tasks() {
        let res = evaluate_tool_call(
            &spec(),
            &gate_task(),
            "submit_task_complete",
            &submit_task_complete_args(),
            Path::new("/tmp/work"),
        );
        assert!(matches!(res, Err(PolicyViolation { code, .. }) if code == "tool-acl-deny"));
    }

    #[test]
    fn test_submit_task_complete_still_honors_explicit_deny() {
        let mut intent = spec();
        intent.security.tool_acl.deny.push(ToolRule {
            tool: "submit_task_complete".into(),
            actions: vec!["execute".into()],
            constraints: BTreeMap::new(),
        });
        let res = evaluate_tool_call(
            &intent,
            &task(),
            "submit_task_complete",
            &submit_task_complete_args(),
            Path::new("/tmp/work"),
        );
        assert!(matches!(res, Err(PolicyViolation { code, .. }) if code == "tool-acl-deny"));
    }

    #[test]
    fn test_shell_result_records_written_paths_from_metadata() {
        let output = ToolOutput::success("Exit code: 0").with_metadata(serde_json::json!({
            "paths_written": ["src/lib.rs"]
        }));
        let mut record = ToolCallRecord {
            tool_name: "shell".into(),
            action: "execute".into(),
            arguments_json: Some(
                serde_json::json!({ "command": "perl -pi -e 's/x/y/' src/lib.rs" }),
            ),
            ..ToolCallRecord::default()
        };

        evaluate_tool_result(&spec(), &task(), "shell", &output, &mut record).unwrap();

        assert_eq!(record.paths_written, vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn test_shell_result_rejects_out_of_scope_metadata_writes() {
        let output = ToolOutput::success("Exit code: 0").with_metadata(serde_json::json!({
            "paths_written": ["vendor/generated.rs"]
        }));
        let mut record = ToolCallRecord {
            tool_name: "shell".into(),
            action: "execute".into(),
            arguments_json: Some(
                serde_json::json!({ "command": "printf hi > vendor/generated.rs" }),
            ),
            ..ToolCallRecord::default()
        };

        let violation = evaluate_tool_result(&spec(), &task(), "shell", &output, &mut record)
            .expect_err("out-of-scope shell writes must be rejected");

        assert_eq!(violation.code, "scope-creep");
        assert_eq!(violation.path.as_deref(), Some("vendor/generated.rs"));
    }

    #[test]
    fn test_shell_result_rejects_writes_outside_touch_files() {
        let mut task = task();
        task.contract.touch_files = vec!["src/lib.rs".into()];
        let output = ToolOutput::success("Exit code: 0").with_metadata(serde_json::json!({
            "paths_written": ["src/main.rs"]
        }));
        let mut record = ToolCallRecord {
            tool_name: "shell".into(),
            action: "execute".into(),
            arguments_json: Some(
                serde_json::json!({ "command": "printf 'fn main() {}' > src/main.rs" }),
            ),
            ..ToolCallRecord::default()
        };

        let violation = evaluate_tool_result(&spec(), &task, "shell", &output, &mut record)
            .expect_err("shell writes outside touchFiles must be rejected");

        assert_eq!(violation.code, "scope-creep");
        assert_eq!(violation.path.as_deref(), Some("src/main.rs"));
        assert!(violation.message.contains("not in touchFiles"));
    }

    #[test]
    fn test_shell_result_allows_cargo_lock_companion_for_cargo_toml_touch_file() {
        let mut task = task();
        task.contract.touch_files = vec!["libra/Cargo.toml".into(), "libra/src/main.rs".into()];
        let output = ToolOutput::success("Exit code: 0").with_metadata(serde_json::json!({
            "paths_written": ["libra/Cargo.lock"]
        }));
        let mut record = ToolCallRecord {
            tool_name: "shell".into(),
            action: "execute".into(),
            arguments_json: Some(serde_json::json!({ "command": "cargo build" })),
            ..ToolCallRecord::default()
        };

        evaluate_tool_result(&spec(), &task, "shell", &output, &mut record).unwrap();

        assert_eq!(record.paths_written, vec!["libra/Cargo.lock".to_string()]);
    }

    #[test]
    fn test_dependency_policy_no_new_rejects_cargo_toml_dependency_addition() {
        let output = ToolOutput::success("Applied patch").with_metadata(serde_json::json!({
            "diffs": [{
                "path": "Cargo.toml",
                "type": "update",
                "diff": "@@\n [dependencies]\n+clap = { version = \"4\", features = [\"derive\"] }\n"
            }]
        }));
        let mut record = ToolCallRecord {
            tool_name: "apply_patch".into(),
            action: "write".into(),
            ..ToolCallRecord::default()
        };

        let violation = evaluate_tool_result(&spec(), &task(), "apply_patch", &output, &mut record)
            .expect_err("dependency-policy:no-new must reject newly added Cargo dependencies");

        assert_eq!(violation.code, "dependency-policy-no-new");
        assert_eq!(violation.path.as_deref(), Some("Cargo.toml"));
    }

    #[test]
    fn test_dependency_policy_no_new_allows_cargo_toml_dependency_version_update() {
        let output = ToolOutput::success("Applied patch").with_metadata(serde_json::json!({
            "diffs": [{
                "path": "Cargo.toml",
                "type": "update",
                "diff": "@@\n [dependencies]\n-serde = \"1.0.0\"\n+serde = \"1.0.1\"\n"
            }]
        }));
        let mut record = ToolCallRecord {
            tool_name: "apply_patch".into(),
            action: "write".into(),
            ..ToolCallRecord::default()
        };

        evaluate_tool_result(&spec(), &task(), "apply_patch", &output, &mut record).unwrap();
    }

    #[test]
    fn test_dependency_policy_no_new_rejects_cargo_toml_dependency_subtable_addition() {
        let output = ToolOutput::success("Applied patch").with_metadata(serde_json::json!({
            "diffs": [{
                "path": "Cargo.toml",
                "type": "update",
                "diff": "@@\n+[dependencies.clap]\n+version = \"4\"\n"
            }]
        }));
        let mut record = ToolCallRecord {
            tool_name: "apply_patch".into(),
            action: "write".into(),
            ..ToolCallRecord::default()
        };

        let violation = evaluate_tool_result(&spec(), &task(), "apply_patch", &output, &mut record)
            .expect_err("dependency-policy:no-new must reject dependency subtables");

        assert_eq!(violation.code, "dependency-policy-no-new");
        assert_eq!(violation.path.as_deref(), Some("Cargo.toml"));
    }

    #[test]
    fn test_dependency_policy_no_new_allows_non_dependency_manifest_edits() {
        let output = ToolOutput::success("Applied patch").with_metadata(serde_json::json!({
            "diffs": [{
                "path": "Cargo.toml",
                "type": "update",
                "diff": "@@\n [package]\n-name = \"old\"\n+name = \"new\"\n"
            }]
        }));
        let mut record = ToolCallRecord {
            tool_name: "apply_patch".into(),
            action: "write".into(),
            ..ToolCallRecord::default()
        };

        evaluate_tool_result(&spec(), &task(), "apply_patch", &output, &mut record).unwrap();
    }

    #[test]
    fn test_dependency_policy_allow_with_review_allows_cargo_toml_dependency_addition() {
        let mut intent = spec();
        intent.constraints.security.dependency_policy = DependencyPolicy::AllowWithReview;
        let output = ToolOutput::success("Applied patch").with_metadata(serde_json::json!({
            "diffs": [{
                "path": "Cargo.toml",
                "type": "update",
                "diff": "@@\n [dependencies]\n+clap = \"4\"\n"
            }]
        }));
        let mut record = ToolCallRecord {
            tool_name: "apply_patch".into(),
            action: "write".into(),
            ..ToolCallRecord::default()
        };

        evaluate_tool_result(&intent, &task(), "apply_patch", &output, &mut record).unwrap();
    }
}
