//! Resolver that locates IntentSpecs from user-supplied identifiers, stored objects,
//! or active scheduler context.
//!
//! Boundary: resolution should be deterministic and must distinguish "not found" from
//! malformed IDs so callers can show actionable errors. MCP and intent-flow tests cover
//! plain UUIDs, prefixed object IDs, and absent references.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use chrono::Utc;
use uuid::Uuid;

use super::{
    draft::{DraftCheck, IntentDraft},
    profiles,
    types::{
        Acceptance, ArtifactName, ArtifactReq, ArtifactStage, Artifacts, ChangeType, Check,
        Constraints, CreatedBy, CreatorType, DependencyPolicy, Intent, IntentSpec, Lifecycle,
        LifecycleStatus, Metadata, QualityGates, RepoTarget, RepoType, RiskLevel, Target, ToolRule,
        TouchHints, VerificationPlan,
    },
};

#[derive(Clone, Debug)]
pub struct ResolveContext {
    pub working_dir: String,
    pub base_ref: String,
    pub created_by_id: String,
}

/// Resolves an IntentDraft into a full IntentSpec by applying risk-based defaults and metadata.
///
/// This is the final stage of "Plan" generation before the IntentSpec is presented to the user
/// and persisted via MCP (see [`crate::internal::ai::intentspec::persist_intentspec`]).
pub fn resolve_intentspec(
    draft: IntentDraft,
    risk_level: RiskLevel,
    ctx: ResolveContext,
) -> IntentSpec {
    let mut constraints = profiles::default_constraints(risk_level.clone());
    let has_implementation_work = draft.intent.has_implementation_objectives();
    let mut artifacts = profiles::default_artifacts(risk_level.clone(), has_implementation_work);
    let needs_shell_access = draft_needs_shell_access(&draft);
    let dependency_derivation = explicit_new_dependency_request(&draft);
    if constraints.security.dependency_policy == DependencyPolicy::NoNew
        && dependency_derivation.is_some()
    {
        constraints.security.dependency_policy = DependencyPolicy::AllowWithReview;
    }
    let mut security = profiles::default_security();
    if needs_shell_access {
        ensure_tool_rule(&mut security.tool_acl.allow, "shell", &["execute"]);
    }

    let verification_plan = VerificationPlan {
        fast_checks: draft
            .acceptance
            .fast_checks
            .into_iter()
            .map(convert_draft_check)
            .collect(),
        integration_checks: draft
            .acceptance
            .integration_checks
            .into_iter()
            .map(convert_draft_check)
            .collect(),
        security_checks: draft
            .acceptance
            .security_checks
            .into_iter()
            .map(convert_draft_check)
            .collect(),
        release_checks: draft
            .acceptance
            .release_checks
            .into_iter()
            .map(convert_draft_check)
            .collect(),
    };

    merge_artifacts_from_checks(&verification_plan, &mut artifacts);
    // Drop required artifacts whose stage has no declared checks: a profile-injected
    // security artifact (e.g. SAST) cannot be produced if no security check was scheduled.
    if verification_plan.security_checks.is_empty() {
        artifacts
            .required
            .retain(|a| a.stage != ArtifactStage::Security);
    }
    harmonize_retention(&constraints, &mut artifacts);

    let quality_gates = QualityGates {
        require_new_tests_when_bugfix: if matches!(draft.intent.change_type, ChangeType::Bugfix) {
            Some(true)
        } else {
            None
        },
        max_allowed_regression: None,
    };

    let mut extensions = BTreeMap::new();
    if let Some(derivation) = dependency_derivation
        && constraints.security.dependency_policy == DependencyPolicy::AllowWithReview
    {
        extensions.insert(
            "libra.ai.dependencyPolicyDerivation".to_string(),
            serde_json::json!({
                "source": "explicit-new-dependency-request",
                "policy": "allow-with-review",
                "reason": derivation.reason,
                "detectedDependencies": derivation.dependencies,
            }),
        );
    }

    IntentSpec {
        api_version: "intentspec.io/v1alpha1".to_string(),
        kind: "IntentSpec".to_string(),
        metadata: Metadata {
            id: Uuid::now_v7().to_string(),
            created_at: Utc::now().to_rfc3339(),
            created_by: CreatedBy {
                creator_type: CreatorType::User,
                id: ctx.created_by_id,
                display_name: None,
            },
            target: Target {
                repo: RepoTarget {
                    repo_type: RepoType::Local,
                    locator: normalize_working_dir(&ctx.working_dir),
                },
                base_ref: if ctx.base_ref.trim().is_empty() {
                    "HEAD".to_string()
                } else {
                    ctx.base_ref
                },
                workspace_id: None,
                labels: BTreeMap::new(),
            },
        },
        intent: Intent {
            summary: draft.intent.summary,
            problem_statement: draft.intent.problem_statement,
            change_type: draft.intent.change_type,
            objectives: draft.intent.objectives,
            in_scope: normalize_path_entries(draft.intent.in_scope, &ctx.working_dir),
            out_of_scope: normalize_path_entries(draft.intent.out_of_scope, &ctx.working_dir),
            touch_hints: draft.intent.touch_hints.map(|hints| TouchHints {
                files: normalize_path_entries(hints.files, &ctx.working_dir),
                symbols: hints.symbols,
                apis: hints.apis,
            }),
        },
        acceptance: Acceptance {
            success_criteria: draft.acceptance.success_criteria,
            verification_plan,
            quality_gates: Some(quality_gates),
        },
        constraints,
        risk: profiles::default_risk(risk_level.clone(), draft.risk.rationale, draft.risk.factors),
        evidence: profiles::default_evidence(risk_level.clone()),
        security,
        execution: profiles::default_execution(risk_level.clone()),
        artifacts,
        provenance: profiles::default_provenance(risk_level, has_implementation_work),
        lifecycle: Lifecycle {
            schema_version: "1.0.0".to_string(),
            status: LifecycleStatus::Active,
            change_log: Vec::new(),
        },
        libra: None,
        extensions,
    }
}

fn convert_draft_check(c: DraftCheck) -> Check {
    Check {
        id: c.id,
        kind: c.kind,
        command: c.command,
        timeout_seconds: c.timeout_seconds,
        expected_exit_code: c.expected_exit_code,
        required: c.required,
        artifacts_produced: c.artifacts_produced,
    }
}

fn draft_needs_shell_access(draft: &IntentDraft) -> bool {
    draft
        .acceptance
        .fast_checks
        .iter()
        .chain(draft.acceptance.integration_checks.iter())
        .chain(draft.acceptance.security_checks.iter())
        .chain(draft.acceptance.release_checks.iter())
        .any(|check| {
            check
                .command
                .as_deref()
                .is_some_and(looks_like_shell_command)
        })
        || draft
            .acceptance
            .success_criteria
            .iter()
            .any(|criterion| mentions_shell_command(criterion))
        || draft
            .intent
            .objectives
            .iter()
            .any(|objective| mentions_shell_command(&objective.title))
        || draft
            .intent
            .in_scope
            .iter()
            .any(|scope| mentions_shell_command(scope))
        || draft
            .intent
            .touch_hints
            .as_ref()
            .is_some_and(|touch_hints| {
                touch_hints
                    .apis
                    .iter()
                    .any(|api| mentions_shell_command(api))
            })
}

#[derive(Clone, Debug)]
struct DependencyPolicyDerivation {
    reason: String,
    dependencies: Vec<String>,
}

fn explicit_new_dependency_request(draft: &IntentDraft) -> Option<DependencyPolicyDerivation> {
    let mut dependencies = HashSet::new();
    let mut matched_reason = None;

    for text in dependency_request_texts(draft) {
        if negates_new_dependency_request(text) {
            continue;
        }
        let command_dependencies = dependency_names_from_package_manager_command(text);
        dependencies.extend(command_dependencies);

        if text_explicitly_requests_new_dependency(text) {
            dependencies.extend(dependency_names_from_natural_language(text));
            if matched_reason.is_none() {
                matched_reason = Some(format!(
                    "draft explicitly requests adding or using a new dependency: {}",
                    truncate_dependency_reason(text)
                ));
            }
        }
    }

    matched_reason.map(|reason| {
        let mut dependencies = dependencies.into_iter().collect::<Vec<_>>();
        dependencies.sort();
        DependencyPolicyDerivation {
            reason,
            dependencies,
        }
    })
}

fn dependency_request_texts(draft: &IntentDraft) -> Vec<&str> {
    let mut texts = vec![
        draft.intent.summary.as_str(),
        draft.intent.problem_statement.as_str(),
        draft.risk.rationale.as_str(),
    ];
    texts.extend(
        draft
            .intent
            .objectives
            .iter()
            .map(|objective| objective.title.as_str()),
    );
    texts.extend(draft.intent.in_scope.iter().map(String::as_str));
    texts.extend(draft.intent.out_of_scope.iter().map(String::as_str));
    texts.extend(draft.acceptance.success_criteria.iter().map(String::as_str));
    texts.extend(
        draft
            .acceptance
            .fast_checks
            .iter()
            .chain(draft.acceptance.integration_checks.iter())
            .chain(draft.acceptance.security_checks.iter())
            .chain(draft.acceptance.release_checks.iter())
            .filter_map(|check| check.command.as_deref()),
    );
    if let Some(touch_hints) = &draft.intent.touch_hints {
        texts.extend(touch_hints.files.iter().map(String::as_str));
        texts.extend(touch_hints.symbols.iter().map(String::as_str));
        texts.extend(touch_hints.apis.iter().map(String::as_str));
    }
    texts
}

fn negates_new_dependency_request(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "no new dependencies",
        "no new dependency",
        "without adding dependencies",
        "without adding any dependencies",
        "do not add dependencies",
        "don't add dependencies",
        "avoid adding dependencies",
        "avoid new dependencies",
        "prefer std",
        "std-only",
        "不新增依赖",
        "不要新增依赖",
        "避免新增依赖",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn text_explicitly_requests_new_dependency(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("cargo add ")
        || lower.contains("npm install ")
        || lower.contains("pnpm add ")
        || lower.contains("yarn add ")
    {
        return true;
    }

    let has_dependency_noun = [
        "dependency",
        "dependencies",
        "crate",
        "package",
        "third-party",
        "3rd-party",
        "依赖",
        "第三方",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let has_add_verb = [
        "add ",
        "adding ",
        "introduce ",
        "install ",
        "新增",
        "添加",
        "引入",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if has_dependency_noun && has_add_verb {
        return true;
    }

    dependency_use_verb_is_explicit(&lower) && text_uses_named_or_new_dependency(text)
}

fn dependency_names_from_package_manager_command(text: &str) -> Vec<String> {
    let tokens = shell_like_tokens(text);
    let mut names = Vec::new();
    for window in tokens.windows(3) {
        match [window[0].as_str(), window[1].as_str()] {
            ["cargo", "add"] | ["pnpm", "add"] | ["yarn", "add"] => {
                push_dependency_token(&mut names, &window[2]);
            }
            ["npm", "install"] => {
                push_dependency_token(&mut names, &window[2]);
            }
            _ => {}
        }
    }
    names
}

fn dependency_names_from_natural_language(text: &str) -> Vec<String> {
    let tokens = shell_like_tokens(text);
    let mut names = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let lower = token.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "crate" | "crates" | "package" | "packages" | "dependency" | "dependencies" | "依赖"
        ) && index > 0
            && let Some(name) = tokens[..index]
                .iter()
                .rev()
                .find(|candidate| dependency_name_candidate(candidate.as_str()))
        {
            push_dependency_token(&mut names, name.as_str());
        }
    }
    names
}

fn dependency_use_verb_is_explicit(lower: &str) -> bool {
    ["use ", "using ", "使用"]
        .iter()
        .any(|marker| lower.contains(marker))
}

fn text_uses_named_or_new_dependency(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if [
        "new dependency",
        "new dependencies",
        "new crate",
        "new package",
        "third-party dependency",
        "third-party crate",
        "third-party package",
        "3rd-party dependency",
        "3rd-party crate",
        "3rd-party package",
        "新依赖",
        "新增依赖",
        "第三方依赖",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return true;
    }

    !dependency_names_from_natural_language(text).is_empty()
}

fn dependency_name_candidate(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    !matches!(
        lower.as_str(),
        "a" | "an"
            | "the"
            | "as"
            | "for"
            | "from"
            | "in"
            | "into"
            | "of"
            | "on"
            | "to"
            | "via"
            | "with"
            | "new"
            | "existing"
            | "current"
            | "local"
            | "project"
            | "workspace"
            | "metadata"
            | "cache"
            | "dependency"
            | "dependencies"
            | "crate"
            | "crates"
            | "package"
            | "packages"
            | "third-party"
            | "use"
            | "using"
            | "add"
            | "adding"
            | "install"
            | "introduce"
    )
}

fn shell_like_tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | ':' | ',' | ';'
            )
    })
    .filter_map(|token| {
        let token = token
            .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
            .trim();
        (!token.is_empty()).then(|| token.to_string())
    })
    .collect()
}

fn push_dependency_token(names: &mut Vec<String>, token: &str) {
    let token = token
        .trim_start_matches('@')
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-');
    if token.is_empty() || token.starts_with('-') {
        return;
    }
    let lower = token.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "a" | "an"
            | "the"
            | "new"
            | "dependency"
            | "dependencies"
            | "crate"
            | "crates"
            | "package"
            | "packages"
            | "third-party"
            | "use"
            | "using"
            | "add"
            | "adding"
            | "install"
            | "introduce"
    ) {
        return;
    }
    names.push(token.to_string());
}

fn truncate_dependency_reason(text: &str) -> String {
    const MAX_LEN: usize = 180;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX_LEN {
        return trimmed.to_string();
    }
    let mut truncated = trimmed.chars().take(MAX_LEN).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn mentions_shell_command(text: &str) -> bool {
    let tokens = text
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '`' | '"' | '\'' | '(' | ')' | ':'))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    tokens.windows(2).any(|window| {
        is_shell_command_mention_name(window[0]) && !window[1].trim_matches(['-', '_']).is_empty()
    })
}

fn looks_like_shell_command(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .is_some_and(is_shell_command_name)
}

fn is_shell_command_name(token: &str) -> bool {
    let token = token
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .to_ascii_lowercase();
    matches!(
        token.as_str(),
        "cargo"
            | "rustc"
            | "npm"
            | "pnpm"
            | "yarn"
            | "make"
            | "cmake"
            | "pytest"
            | "python"
            | "python3"
            | "go"
    )
}

fn is_shell_command_mention_name(token: &str) -> bool {
    let token = token
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-')
        .to_ascii_lowercase();
    matches!(
        token.as_str(),
        "cargo" | "rustc" | "npm" | "pnpm" | "yarn" | "cmake" | "pytest" | "python" | "python3"
    )
}

fn ensure_tool_rule(allow: &mut Vec<ToolRule>, tool: &str, actions: &[&str]) {
    if let Some(rule) = allow.iter_mut().find(|rule| rule.tool == tool) {
        for action in actions {
            if !rule.actions.iter().any(|existing| existing == action) {
                rule.actions.push((*action).to_string());
            }
        }
        return;
    }

    allow.push(ToolRule {
        tool: tool.to_string(),
        actions: actions.iter().map(|action| (*action).to_string()).collect(),
        constraints: Default::default(),
    });
}

fn normalize_working_dir(raw: &str) -> String {
    let p = Path::new(raw);
    p.canonicalize()
        .map(|v| v.display().to_string())
        .unwrap_or_else(|_| raw.to_string())
}

/// Convert raw path entries (often absolute paths supplied by the LLM) into
/// repository-relative paths that downstream scope/contract checks can match.
///
/// Why: tasks execute in isolated worktrees rooted at a different absolute path
/// than the user's repo. Comparing relative changed paths against absolute
/// touch-files patterns silently fails (e.g. `Cargo.lock` vs
/// `/Volumes/Data/repo/Cargo.toml`), causing every sync-back to be reported as a
/// contract violation and triggering pointless replans.
fn normalize_path_entries(entries: Vec<String>, working_dir: &str) -> Vec<String> {
    let canonical = canonical_working_dir(working_dir);
    entries
        .into_iter()
        .filter_map(|raw| normalize_single_path_entry(raw, working_dir, canonical.as_deref()))
        .collect()
}

fn canonical_working_dir(working_dir: &str) -> Option<PathBuf> {
    let trimmed = working_dir.trim();
    if trimmed.is_empty() {
        return None;
    }
    Path::new(trimmed).canonicalize().ok()
}

fn normalize_single_path_entry(
    raw: String,
    working_dir: &str,
    canonical_working_dir: Option<&Path>,
) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let unified = trimmed.replace('\\', "/");
    let relative = strip_working_dir_prefix(&unified, working_dir, canonical_working_dir);
    let relative = relative.trim_start_matches("./").trim();
    if relative.is_empty() {
        return None;
    }
    Some(relative.to_string())
}

fn strip_working_dir_prefix(
    path: &str,
    working_dir: &str,
    canonical_working_dir: Option<&Path>,
) -> String {
    let trimmed_wd = working_dir.trim().trim_end_matches('/');
    if !trimmed_wd.is_empty()
        && let Some(rest) = path.strip_prefix(trimmed_wd)
        && let Some(rest) = rest.strip_prefix('/')
    {
        return rest.to_string();
    }
    if let Some(canonical) = canonical_working_dir {
        let canonical_str = canonical.to_string_lossy();
        let trimmed_canonical = canonical_str.trim_end_matches('/');
        if !trimmed_canonical.is_empty()
            && let Some(rest) = path.strip_prefix(trimmed_canonical)
            && let Some(rest) = rest.strip_prefix('/')
        {
            return rest.to_string();
        }
    }
    path.to_string()
}

fn merge_artifacts_from_checks(plan: &VerificationPlan, artifacts: &mut Artifacts) {
    let mut existing: HashSet<(ArtifactName, ArtifactStage)> = artifacts
        .required
        .iter()
        .map(|a| (a.name.clone(), a.stage.clone()))
        .collect();

    for (stage, checks) in [
        (ArtifactStage::PerTask, &plan.fast_checks),
        (ArtifactStage::Integration, &plan.integration_checks),
        (ArtifactStage::Security, &plan.security_checks),
        (ArtifactStage::Release, &plan.release_checks),
    ] {
        for check in checks {
            for name in &check.artifacts_produced {
                if let Some(parsed_name) = parse_artifact_name(name) {
                    let key = (parsed_name.clone(), stage.clone());
                    if !existing.contains(&key) {
                        artifacts.required.push(ArtifactReq {
                            name: parsed_name.clone(),
                            stage: stage.clone(),
                            required: true,
                            format: String::new(),
                        });
                        existing.insert(key);
                    }
                }
            }
        }
    }
}

fn parse_artifact_name(name: &str) -> Option<ArtifactName> {
    match name {
        "patchset" => Some(ArtifactName::Patchset),
        "test-log" => Some(ArtifactName::TestLog),
        "build-log" => Some(ArtifactName::BuildLog),
        "sast-report" => Some(ArtifactName::SastReport),
        "sca-report" => Some(ArtifactName::ScaReport),
        "sbom" => Some(ArtifactName::Sbom),
        "provenance-attestation" => Some(ArtifactName::ProvenanceAttestation),
        "transparency-proof" => Some(ArtifactName::TransparencyProof),
        "release-notes" => Some(ArtifactName::ReleaseNotes),
        _ => None,
    }
}

fn harmonize_retention(constraints: &Constraints, artifacts: &mut Artifacts) {
    let effective = constraints
        .privacy
        .retention_days
        .min(artifacts.retention.days);
    artifacts.retention.days = effective;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::{
        draft::{DraftAcceptance, DraftCheck, DraftIntent, DraftRisk},
        types::{CheckKind, Objective, ObjectiveKind},
    };

    #[test]
    fn test_resolve_intentspec_low_profile() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Fix bug".to_string(),
                problem_statement: "Bug details".to_string(),
                change_type: ChangeType::Bugfix,
                objectives: vec![Objective {
                    title: "fix".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["tests pass".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "safe".to_string(),
                factors: vec![],
                level: None,
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert_eq!(spec.risk.level, RiskLevel::Low);
        assert_eq!(spec.kind, "IntentSpec");
        assert!(
            spec.acceptance
                .quality_gates
                .as_ref()
                .and_then(|q| q.require_new_tests_when_bugfix)
                .unwrap_or(false)
        );
    }

    #[test]
    fn test_resolve_cargo_command_draft_allows_shell_execution() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Initialize cargo project".to_string(),
                problem_statement: "Create a Rust CLI scaffold with cargo init".to_string(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "Initialize with cargo init --vcs none --name libra".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["Cargo.toml".to_string(), "src/main.rs".to_string()],
                out_of_scope: vec!["git setup".to_string()],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["cargo build succeeds".to_string()],
                fast_checks: vec![DraftCheck {
                    id: "build".to_string(),
                    kind: CheckKind::Command,
                    command: Some("cargo build".to_string()),
                    timeout_seconds: Some(60),
                    expected_exit_code: Some(0),
                    required: true,
                    artifacts_produced: vec![],
                }],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "local scaffold".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert!(spec.security.tool_acl.allow.iter().any(|rule| {
            rule.tool == "shell" && rule.actions.iter().any(|action| action == "execute")
        }));
    }

    #[test]
    fn test_resolve_plain_implementation_does_not_allow_shell_execution() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Edit a file".to_string(),
                problem_statement: "Update source text".to_string(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "Update the greeting text".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src/main.rs".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["Greeting text is updated".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "single file edit".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert!(
            !spec
                .security
                .tool_acl
                .allow
                .iter()
                .any(|rule| rule.tool == "shell")
        );
    }

    #[test]
    fn resolve_low_risk_explicit_new_dependency_uses_allow_with_review() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Add clap crate support".to_string(),
                problem_statement: "The CLI should use the clap crate for argument parsing"
                    .to_string(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "Add the clap crate and wire parser setup".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["Cargo.toml".to_string(), "src/main.rs".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["cargo test passes".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "small CLI change".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert_eq!(
            spec.constraints.security.dependency_policy,
            DependencyPolicy::AllowWithReview
        );
        let derivation = spec
            .extensions
            .get("libra.ai.dependencyPolicyDerivation")
            .expect("dependency policy derivation extension");
        assert_eq!(derivation["policy"], "allow-with-review");
        assert!(
            derivation["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("clap"))
        );
    }

    #[test]
    fn resolve_low_risk_use_named_crate_uses_allow_with_review() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Use the clap crate for parsing".to_string(),
                problem_statement: "The CLI should use clap as a dependency for argument parsing"
                    .to_string(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "Wire parser setup through the clap crate".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["Cargo.toml".to_string(), "src/main.rs".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["cargo test passes".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "small CLI change".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert_eq!(
            spec.constraints.security.dependency_policy,
            DependencyPolicy::AllowWithReview
        );
    }

    #[test]
    fn resolve_low_risk_without_explicit_new_dependency_keeps_no_new_policy() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Improve CLI parser".to_string(),
                problem_statement: "Use the existing parser code to handle flags".to_string(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "Handle --verbose with existing code".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src/main.rs".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["cargo test passes".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "small CLI change".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert_eq!(
            spec.constraints.security.dependency_policy,
            DependencyPolicy::NoNew
        );
        assert!(
            !spec
                .extensions
                .contains_key("libra.ai.dependencyPolicyDerivation")
        );
    }

    #[test]
    fn resolve_low_risk_package_metadata_wording_keeps_no_new_policy() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Use package metadata cache".to_string(),
                problem_statement: "Use package metadata to avoid recomputing local cache keys"
                    .to_string(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "Read package metadata from the existing cache".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src/internal/cache.rs".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["cargo test passes".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "small cache change".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert_eq!(
            spec.constraints.security.dependency_policy,
            DependencyPolicy::NoNew
        );
        assert!(
            !spec
                .extensions
                .contains_key("libra.ai.dependencyPolicyDerivation")
        );
    }

    #[test]
    fn test_resolve_make_sure_wording_does_not_allow_shell_execution() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Update help text".to_string(),
                problem_statement: "Make sure the CLI message is clearer".to_string(),
                change_type: ChangeType::Docs,
                objectives: vec![Objective {
                    title: "Make sure help text mentions backup behavior".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src/main.rs".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["Help text is clearer".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "wording only".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert!(
            !spec
                .security
                .tool_acl
                .allow
                .iter()
                .any(|rule| rule.tool == "shell")
        );
    }

    #[test]
    fn test_merge_artifacts_preserves_stage_for_same_name() {
        let plan = VerificationPlan {
            fast_checks: vec![],
            integration_checks: vec![Check {
                id: "integration".into(),
                kind: CheckKind::Command,
                command: Some("cargo test".into()),
                timeout_seconds: None,
                expected_exit_code: None,
                required: true,
                artifacts_produced: vec!["test-log".into()],
            }],
            security_checks: vec![],
            release_checks: vec![Check {
                id: "release".into(),
                kind: CheckKind::Command,
                command: Some("cargo test --release".into()),
                timeout_seconds: None,
                expected_exit_code: None,
                required: true,
                artifacts_produced: vec!["test-log".into()],
            }],
        };
        let mut artifacts = Artifacts {
            required: vec![],
            retention: crate::internal::ai::intentspec::types::ArtifactRetention::default(),
        };

        merge_artifacts_from_checks(&plan, &mut artifacts);

        assert!(artifacts.required.iter().any(|req| {
            req.name == ArtifactName::TestLog && req.stage == ArtifactStage::Integration
        }));
        assert!(artifacts.required.iter().any(|req| {
            req.name == ArtifactName::TestLog && req.stage == ArtifactStage::Release
        }));
    }

    #[test]
    fn test_resolve_analysis_only_does_not_require_patchset_by_default() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Diagnose repository".to_string(),
                problem_statement: "Summarize technical debt hotspots".to_string(),
                change_type: ChangeType::Unknown,
                objectives: vec![Objective {
                    title: "Inventory key issues".to_string(),
                    kind: ObjectiveKind::Analysis,
                }],
                in_scope: vec!["src".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["Findings are summarized".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "read-only analysis".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert!(
            !spec
                .artifacts
                .required
                .iter()
                .any(|req| req.name == ArtifactName::Patchset),
            "{:?}",
            spec.artifacts.required
        );
    }

    #[test]
    fn test_resolve_analysis_only_medium_risk_has_no_default_security_or_release_artifacts() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Diagnose repository".to_string(),
                problem_statement: "Summarize technical debt hotspots".to_string(),
                change_type: ChangeType::Unknown,
                objectives: vec![Objective {
                    title: "Inventory key issues".to_string(),
                    kind: ObjectiveKind::Analysis,
                }],
                in_scope: vec!["src".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["Findings are summarized".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "read-only analysis".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Medium),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Medium,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert!(
            spec.artifacts.required.is_empty(),
            "{:?}",
            spec.artifacts.required
        );
        assert!(!spec.provenance.require_slsa_provenance);
        assert!(!spec.provenance.require_sbom);
    }

    #[test]
    fn test_resolve_implementation_only_does_not_require_test_log_without_checks() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Implement feature".to_string(),
                problem_statement: "Add the requested behavior".to_string(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "Ship feature".to_string(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src".to_string()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["feature works".to_string()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "normal feature".to_string(),
                factors: vec![],
                level: Some(RiskLevel::Low),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: ".".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "tester".to_string(),
            },
        );

        assert!(
            spec.artifacts
                .required
                .iter()
                .any(|req| req.name == ArtifactName::Patchset),
            "{:?}",
            spec.artifacts.required
        );
        assert!(
            !spec
                .artifacts
                .required
                .iter()
                .any(|req| req.name == ArtifactName::TestLog
                    && req.stage == ArtifactStage::PerTask),
            "{:?}",
            spec.artifacts.required
        );
    }

    #[test]
    fn normalize_path_entries_strips_working_dir_prefix() {
        let normalized = normalize_path_entries(
            vec![
                "/Volumes/Data/linked/Cargo.toml".into(),
                "/Volumes/Data/linked/src/main.rs".into(),
                "Cargo.lock".into(),
                "./README.md".into(),
                "  ".into(),
            ],
            "/Volumes/Data/linked",
        );
        assert_eq!(
            normalized,
            vec![
                "Cargo.toml".to_string(),
                "src/main.rs".to_string(),
                "Cargo.lock".to_string(),
                "README.md".to_string(),
            ]
        );
    }

    #[test]
    fn normalize_path_entries_leaves_unrelated_absolute_paths_alone() {
        let normalized = normalize_path_entries(
            vec!["/etc/hosts".into(), "src/lib.rs".into()],
            "/Volumes/Data/linked",
        );
        assert_eq!(
            normalized,
            vec!["/etc/hosts".to_string(), "src/lib.rs".to_string()]
        );
    }

    #[test]
    fn resolve_intentspec_normalizes_touch_hint_paths_to_repo_relative() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Bootstrap libra crate".into(),
                problem_statement: "Initialize project".into(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "init".into(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["/Volumes/Data/linked/Cargo.toml".into()],
                out_of_scope: vec!["/Volumes/Data/linked/.git/**".into()],
                touch_hints: Some(crate::internal::ai::intentspec::types::TouchHints {
                    files: vec![
                        "/Volumes/Data/linked/Cargo.toml".into(),
                        "/Volumes/Data/linked/src/main.rs".into(),
                    ],
                    symbols: vec![],
                    apis: vec![],
                }),
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["builds".into()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "trivial".into(),
                factors: vec![],
                level: None,
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Low,
            ResolveContext {
                working_dir: "/Volumes/Data/linked".into(),
                base_ref: "HEAD".into(),
                created_by_id: "tester".into(),
            },
        );

        let touch_hint_files = spec
            .intent
            .touch_hints
            .as_ref()
            .expect("touch hints preserved")
            .files
            .clone();
        assert_eq!(
            touch_hint_files,
            vec!["Cargo.toml".to_string(), "src/main.rs".to_string()]
        );
        assert_eq!(spec.intent.in_scope, vec!["Cargo.toml".to_string()]);
        assert_eq!(spec.intent.out_of_scope, vec![".git/**".to_string()]);
    }

    #[test]
    fn medium_risk_without_security_checks_does_not_require_security_artifacts() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Init libra crate".into(),
                problem_statement: "Scaffold cargo project".into(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "scaffold".into(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src".into()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["builds".into()],
                fast_checks: vec![DraftCheck {
                    id: "fmt".into(),
                    kind: CheckKind::Command,
                    command: Some("cargo fmt --all --check".into()),
                    timeout_seconds: Some(30),
                    expected_exit_code: Some(0),
                    required: true,
                    artifacts_produced: vec![],
                }],
                integration_checks: vec![],
                security_checks: vec![],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "single external crate".into(),
                factors: vec![],
                level: Some(RiskLevel::Medium),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Medium,
            ResolveContext {
                working_dir: ".".into(),
                base_ref: "HEAD".into(),
                created_by_id: "tester".into(),
            },
        );

        let security_artifacts: Vec<&ArtifactReq> = spec
            .artifacts
            .required
            .iter()
            .filter(|a| a.stage == ArtifactStage::Security)
            .collect();
        assert!(
            security_artifacts.is_empty(),
            "medium risk without declared security_checks should not require security artifacts, got: {security_artifacts:?}"
        );
        assert!(
            spec.artifacts
                .required
                .iter()
                .any(|a| a.name == ArtifactName::Patchset && a.stage == ArtifactStage::PerTask),
            "patchset@per-task should remain required for implementation work"
        );
    }

    #[test]
    fn medium_risk_with_security_checks_keeps_security_artifacts() {
        let draft = IntentDraft {
            intent: DraftIntent {
                summary: "Add auth feature".into(),
                problem_statement: "Implement login".into(),
                change_type: ChangeType::Feature,
                objectives: vec![Objective {
                    title: "auth".into(),
                    kind: ObjectiveKind::Implementation,
                }],
                in_scope: vec!["src".into()],
                out_of_scope: vec![],
                touch_hints: None,
            },
            acceptance: DraftAcceptance {
                success_criteria: vec!["secure".into()],
                fast_checks: vec![],
                integration_checks: vec![],
                security_checks: vec![DraftCheck {
                    id: "audit".into(),
                    kind: CheckKind::Command,
                    command: Some("cargo audit".into()),
                    timeout_seconds: Some(120),
                    expected_exit_code: Some(0),
                    required: true,
                    artifacts_produced: vec![],
                }],
                release_checks: vec![],
            },
            risk: DraftRisk {
                rationale: "auth changes".into(),
                factors: vec![],
                level: Some(RiskLevel::Medium),
            },
        };

        let spec = resolve_intentspec(
            draft,
            RiskLevel::Medium,
            ResolveContext {
                working_dir: ".".into(),
                base_ref: "HEAD".into(),
                created_by_id: "tester".into(),
            },
        );

        assert!(
            spec.artifacts
                .required
                .iter()
                .any(|a| a.stage == ArtifactStage::Security),
            "medium risk with declared security_checks should keep security artifacts"
        );
    }
}
