//! Review helpers for turning an [`IntentSpec`] into actionable quality checks.
//!
//! 将 [`IntentSpec`] 转换为可操作质量检查的审查助手。
//!
//! Boundary: this module only derives review guidance from already-normalized intent
//! data; parsing, repair, and persistence stay in sibling modules. Regression coverage
//! lives in the intent-spec and orchestrator tests that assert risk gates and checks
//! survive draft normalization.

use super::types::{Check, IntentSpec};

pub fn build_intentspec_review(
    spec: &IntentSpec,
    intent_id: Option<&str>,
    context_snapshot_id: Option<&str>,
    warnings: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("# IntentSpec Review\n\n");
    out.push_str("| Field | Value |\n");
    out.push_str("| --- | --- |\n");
    push_table_row(&mut out, "Intent ID", intent_id.unwrap_or("not-persisted"));
    push_table_row(
        &mut out,
        "Context Snapshot ID",
        context_snapshot_id.unwrap_or("not-frozen"),
    );
    push_table_row(&mut out, "Spec ID", &spec.metadata.id);
    push_table_row(&mut out, "Repository", &spec.metadata.target.repo.locator);
    push_table_row(&mut out, "Base Ref", &spec.metadata.target.base_ref);
    push_table_row(
        &mut out,
        "Change Type",
        &format!("{:?}", spec.intent.change_type),
    );
    push_table_row(&mut out, "Risk", &format!("{:?}", spec.risk.level));
    push_table_row(
        &mut out,
        "Human Review",
        if spec.risk.human_in_loop.required {
            "required"
        } else {
            "not required"
        },
    );

    out.push_str("\n## Intent\n\n");
    push_section_value(&mut out, "Summary", &spec.intent.summary);
    push_section_value(
        &mut out,
        "Problem Statement",
        &spec.intent.problem_statement,
    );

    push_list(
        &mut out,
        "Objectives",
        spec.intent
            .objectives
            .iter()
            .map(|item| format!("{} ({:?})", item.title, item.kind)),
    );
    push_list(&mut out, "In Scope", spec.intent.in_scope.iter().cloned());
    if scope_grants_repository_root(&spec.intent.in_scope) {
        out.push_str(
            "**Scope Notice:** In Scope grants repository-wide write scope (`.`). Confirm this is intentional before executing.\n\n",
        );
    }
    push_list(
        &mut out,
        "Out Of Scope",
        spec.intent.out_of_scope.iter().cloned(),
    );
    if let Some(touch_hints) = &spec.intent.touch_hints {
        push_list(
            &mut out,
            "Touch Hint Files",
            touch_hints.files.iter().cloned(),
        );
        push_list(
            &mut out,
            "Touch Hint Symbols",
            touch_hints.symbols.iter().cloned(),
        );
        push_list(
            &mut out,
            "Touch Hint APIs",
            touch_hints.apis.iter().cloned(),
        );
    }

    out.push_str("\n## Acceptance\n\n");
    push_list(
        &mut out,
        "Success Criteria",
        spec.acceptance.success_criteria.iter().cloned(),
    );
    push_checks(
        &mut out,
        "Fast Checks",
        &spec.acceptance.verification_plan.fast_checks,
    );
    push_checks(
        &mut out,
        "Integration Checks",
        &spec.acceptance.verification_plan.integration_checks,
    );
    push_checks(
        &mut out,
        "Security Checks",
        &spec.acceptance.verification_plan.security_checks,
    );
    push_checks(
        &mut out,
        "Release Checks",
        &spec.acceptance.verification_plan.release_checks,
    );

    out.push_str("\n## Constraints\n\n");
    push_table_row_block(
        &mut out,
        "Network Policy",
        &format!("{:?}", spec.constraints.security.network_policy),
    );
    push_table_row_block(
        &mut out,
        "Dependency Policy",
        &format!("{:?}", spec.constraints.security.dependency_policy),
    );
    push_table_row_block(
        &mut out,
        "Max Parallel Tasks",
        &spec.execution.concurrency.max_parallel_tasks.to_string(),
    );

    out.push_str("\n## Risk\n\n");
    push_section_value(&mut out, "Rationale", &spec.risk.rationale);
    push_list(&mut out, "Factors", spec.risk.factors.iter().cloned());

    out.push_str("\n## Artifacts\n\n");
    push_list(
        &mut out,
        "Required",
        spec.artifacts.required.iter().map(|artifact| {
            format!(
                "{:?} at {:?} ({})",
                artifact.name,
                artifact.stage,
                if artifact.required {
                    "required"
                } else {
                    "optional"
                }
            )
        }),
    );

    if !warnings.is_empty() {
        push_list(&mut out, "Warnings", warnings.iter().cloned());
    }

    out.push_str(
        "\nConfirm this IntentSpec to generate an execution plan, modify it to revise scope, or cancel.\n",
    );
    out
}

fn scope_grants_repository_root(in_scope: &[String]) -> bool {
    in_scope
        .iter()
        .map(|scope| scope.trim())
        .any(|scope| matches!(scope, "." | "./"))
}

fn push_table_row(out: &mut String, key: &str, value: &str) {
    out.push_str(&format!(
        "| {} | {} |\n",
        escape_table(key),
        escape_table(value)
    ));
}

fn push_table_row_block(out: &mut String, key: &str, value: &str) {
    out.push_str(&format!("- **{}:** {}\n", key, value));
}

fn push_section_value(out: &mut String, key: &str, value: &str) {
    out.push_str(&format!("**{}:** {}\n\n", key, value.trim()));
}

fn push_list<I>(out: &mut String, title: &str, items: I)
where
    I: IntoIterator<Item = String>,
{
    out.push_str(&format!("### {title}\n\n"));
    let mut wrote = false;
    for item in items {
        wrote = true;
        out.push_str(&format!("- {}\n", item));
    }
    if !wrote {
        out.push_str("- none\n");
    }
    out.push('\n');
}

fn push_checks(out: &mut String, title: &str, checks: &[Check]) {
    push_list(
        out,
        title,
        checks.iter().map(|check| {
            let command = check
                .command
                .as_deref()
                .map(|value| format!(" command=`{value}`"))
                .unwrap_or_default();
            format!(
                "{} ({:?}, required={}{}{})",
                check.id,
                check.kind,
                check.required,
                command,
                check
                    .timeout_seconds
                    .map(|seconds| format!(" timeout={}s", seconds))
                    .unwrap_or_default()
            )
        }),
    );
}

fn escape_table(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::{
        ResolveContext,
        draft::{DraftAcceptance, DraftIntent, DraftRisk, IntentDraft},
        resolve_intentspec,
        types::{ChangeType, Objective, ObjectiveKind, RiskLevel},
    };

    #[test]
    fn review_renders_intentspec_before_plan_generation() {
        let spec = resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "Fix the Ollama planner flow".to_string(),
                    problem_statement: "The TUI skips IntentSpec review".to_string(),
                    change_type: ChangeType::Bugfix,
                    objectives: vec![Objective {
                        title: "Show IntentSpec review first".to_string(),
                        kind: ObjectiveKind::Implementation,
                    }],
                    in_scope: vec!["src/internal/tui".to_string()],
                    out_of_scope: vec!["provider rewrite".to_string()],
                    touch_hints: None,
                },
                acceptance: DraftAcceptance {
                    success_criteria: vec!["IntentSpec is visible before plan".to_string()],
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                risk: DraftRisk {
                    rationale: "UI flow only".to_string(),
                    factors: vec!["review gate".to_string()],
                    level: Some(RiskLevel::Low),
                },
            },
            RiskLevel::Low,
            ResolveContext {
                working_dir: "/tmp/repo".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        let review = build_intentspec_review(&spec, Some("intent-123"), None, &[]);

        assert!(review.contains("# IntentSpec Review"));
        assert!(review.contains("Intent ID"));
        assert!(review.contains("| Context Snapshot ID | not-frozen |"));
        assert!(review.contains("Fix the Ollama planner flow"));
        assert!(review.contains("Confirm this IntentSpec to generate an execution plan"));
        assert!(!review.contains("Execution plan ready"));
    }

    /// `intent_id = None` must render as `not-persisted` in the
    /// Intent ID row — the explicit sentinel for un-persisted specs.
    /// Pins the surface so audit consumers can detect "no Intent
    /// object yet exists" by exact text match.
    #[test]
    fn review_renders_not_persisted_marker_when_intent_id_is_none() {
        let spec = minimal_spec_with_scope(vec!["src".to_string()], RiskLevel::Low);
        let review = build_intentspec_review(&spec, None, None, &[]);
        assert!(
            review.contains("| Intent ID | not-persisted |"),
            "review must render the not-persisted sentinel for absent intent_id; got:\n{review}",
        );
    }

    #[test]
    fn review_renders_context_snapshot_id_when_present() {
        let spec = minimal_spec_with_scope(vec!["src".to_string()], RiskLevel::Low);
        let review = build_intentspec_review(&spec, Some("intent-1"), Some("snapshot-123"), &[]);
        assert!(
            review.contains("| Context Snapshot ID | snapshot-123 |"),
            "review must render the persisted ContextSnapshot id; got:\n{review}",
        );
    }

    /// Non-empty `warnings` must render under a `### Warnings`
    /// heading. Each warning must appear as a `- ` list item.
    #[test]
    fn review_renders_warnings_section_when_non_empty() {
        let spec = minimal_spec_with_scope(vec!["src".to_string()], RiskLevel::Low);
        let warnings = vec![
            "stale linter config".to_string(),
            "missing changelog entry".to_string(),
        ];
        let review = build_intentspec_review(&spec, Some("intent-1"), None, &warnings);
        assert!(
            review.contains("### Warnings"),
            "Warnings heading must render; got:\n{review}",
        );
        assert!(review.contains("- stale linter config"));
        assert!(review.contains("- missing changelog entry"));
    }

    /// Empty `warnings` must omit the `Warnings` heading entirely —
    /// no "- none" placeholder, no empty section. This is the inverse
    /// of `push_list`'s behaviour because the warnings rendering
    /// guards on `!warnings.is_empty()`.
    #[test]
    fn review_omits_warnings_section_when_empty() {
        let spec = minimal_spec_with_scope(vec!["src".to_string()], RiskLevel::Low);
        let review = build_intentspec_review(&spec, Some("intent-1"), None, &[]);
        assert!(
            !review.contains("### Warnings"),
            "Warnings heading must NOT render for empty warnings; got:\n{review}",
        );
    }

    /// `push_list` must render `- none` when the iterator is empty.
    /// Exercised via the `Out Of Scope` section which is empty in the
    /// minimal spec.
    #[test]
    fn review_renders_none_placeholder_for_empty_lists() {
        let spec = minimal_spec_with_scope(vec!["src".to_string()], RiskLevel::Low);
        let review = build_intentspec_review(&spec, Some("intent-1"), None, &[]);
        assert!(
            review.contains("### Out Of Scope\n\n- none\n"),
            "empty list must render '- none' placeholder; got:\n{review}",
        );
    }

    /// `scope_grants_repository_root` must accept both `.` and `./`
    /// after trimming, and reject any other scope value (including
    /// `src` and `src/`).
    #[test]
    fn scope_grants_repo_root_accepts_dot_or_dot_slash() {
        assert!(scope_grants_repository_root(&[".".to_string()]));
        assert!(scope_grants_repository_root(&["./".to_string()]));
        assert!(scope_grants_repository_root(&["  .  ".to_string()]));
        assert!(scope_grants_repository_root(&[
            "src".to_string(),
            ".".to_string()
        ]));

        assert!(!scope_grants_repository_root(&[]));
        assert!(!scope_grants_repository_root(&["src".to_string()]));
        assert!(!scope_grants_repository_root(&["src/".to_string()]));
        assert!(!scope_grants_repository_root(&["..".to_string()]));
    }

    /// `escape_table` must convert `|` → `\|` and replace newlines with
    /// spaces so a value containing markdown table metacharacters
    /// doesn't break the rendered table row.
    #[test]
    fn escape_table_quotes_pipe_and_newline() {
        assert_eq!(escape_table("a|b|c"), "a\\|b\\|c");
        assert_eq!(escape_table("line1\nline2"), "line1 line2");
        assert_eq!(escape_table("a|b\nc"), "a\\|b c");
        assert_eq!(escape_table("plain"), "plain");
    }

    fn minimal_spec_with_scope(in_scope: Vec<String>, risk: RiskLevel) -> IntentSpec {
        resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "summary".to_string(),
                    problem_statement: "problem".to_string(),
                    change_type: ChangeType::Chore,
                    objectives: vec![Objective {
                        title: "obj".to_string(),
                        kind: ObjectiveKind::Implementation,
                    }],
                    in_scope,
                    out_of_scope: vec![],
                    touch_hints: None,
                },
                acceptance: DraftAcceptance {
                    success_criteria: vec!["ok".to_string()],
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                risk: DraftRisk {
                    rationale: "rationale".to_string(),
                    factors: vec![],
                    level: Some(risk.clone()),
                },
            },
            risk,
            ResolveContext {
                working_dir: "/tmp/repo".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        )
    }

    #[test]
    fn review_warns_when_scope_grants_repository_root() {
        let spec = resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "Apply broad cleanup".to_string(),
                    problem_statement: "Planner omitted a narrow scope".to_string(),
                    change_type: ChangeType::Bugfix,
                    objectives: vec![Objective {
                        title: "Cleanup".to_string(),
                        kind: ObjectiveKind::Implementation,
                    }],
                    in_scope: vec![".".to_string()],
                    out_of_scope: vec![],
                    touch_hints: None,
                },
                acceptance: DraftAcceptance {
                    success_criteria: vec!["Cleanup is complete".to_string()],
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                risk: DraftRisk {
                    rationale: "repository-wide scope requires review".to_string(),
                    factors: vec![],
                    level: Some(RiskLevel::Medium),
                },
            },
            RiskLevel::Medium,
            ResolveContext {
                working_dir: "/tmp/repo".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        let review = build_intentspec_review(&spec, Some("intent-123"), None, &[]);

        assert!(review.contains("repository-wide write scope"));
    }
}
