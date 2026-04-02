use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{
    acl::{AclVerdict, ScopeVerdict, check_scope, check_tool_acl_with_context},
    types::{PolicyViolation, TaskKind, TaskSpec, ToolCallRecord, ToolDiffRecord},
};
use crate::internal::ai::{
    intentspec::types::{IntentSpec, NetworkPolicy},
    tools::{
        ToolOutput,
        apply_patch::{ApplyPatchArgs, parse_patch},
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

    if tool_name == "shell" {
        if shell_requests_escalation(arguments)
            && spec.constraints.security.network_policy == NetworkPolicy::Deny
        {
            return Err(PolicyViolation {
                code: "sandbox-escalation-deny".into(),
                message:
                    "shell escalation is blocked while constraints.security.networkPolicy=deny"
                        .into(),
                tool_name: Some(tool_name.to_string()),
                path: None,
            });
        }

        if shell_requests_escalation(arguments) && !shell_has_justification(arguments) {
            return Err(PolicyViolation {
                code: "sandbox-escalation-justification-required".into(),
                message: "shell escalation requires a non-empty justification".into(),
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

    for path in &writes {
        match check_scope(&task.scope_in, &task.scope_out, path) {
            ScopeVerdict::InScope => {}
            ScopeVerdict::OutOfScope(reason) => {
                return Err(PolicyViolation {
                    code: "scope-creep".into(),
                    message: reason,
                    tool_name: Some(tool_name.to_string()),
                    path: Some(path.clone()),
                });
            }
        }
    }

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

pub fn evaluate_tool_result(
    spec: &IntentSpec,
    tool_name: &str,
    output: &ToolOutput,
    record: &mut ToolCallRecord,
) -> Result<(), PolicyViolation> {
    record.success = output.is_success();
    record.summary = output
        .as_text()
        .map(|text| text.lines().next().unwrap_or_default().trim().to_string())
        .filter(|summary| !summary.is_empty());
    if tool_name == "apply_patch"
        && let Some(meta) = output.metadata()
    {
        record.diffs = extract_patch_diffs(meta);
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

fn acl_tool_alias(tool_name: &str) -> &str {
    match tool_name {
        "read_file" | "list_dir" | "grep_files" | "apply_patch" => "workspace.fs",
        "request_user_input" => "interaction",
        "submit_intent_draft" => "planning",
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
        "grep_files" => {
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
        "request_user_input" => Ok((
            "interaction".into(),
            "prompt".into(),
            Vec::new(),
            Vec::new(),
        )),
        "submit_intent_draft" => Ok(("planning".into(), "submit".into(), Vec::new(), Vec::new())),
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

fn shell_has_justification(arguments: &Value) -> bool {
    arguments
        .get("justification")
        .and_then(Value::as_str)
        .map(|value| !value.trim().is_empty())
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
        orchestrator::types::{TaskContract, TaskKind, TaskSpec},
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
    fn test_shell_escalation_requires_justification_when_network_allowed() {
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
            Err(PolicyViolation { code, .. }) if code == "sandbox-escalation-justification-required"
        ));
    }

    #[test]
    fn test_shell_escalation_allowed_with_justification_when_network_allowed() {
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
        assert!(res.is_ok());
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
}
