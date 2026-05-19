//! Dynamic workspace context injected into the agent system prompt.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::internal::ai::{agent::TaskIntent, context_budget::ContextBudget};

const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_CONTEXT_FILE_BYTES: usize = 8 * 1024;

#[derive(Clone)]
struct CachedWorkspaceSection {
    captured_at: Instant,
    content: String,
}

static WORKSPACE_CONTEXT_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedWorkspaceSection>>> =
    OnceLock::new();

/// Build the dynamic prompt section for the current workspace and intent.
pub fn build_dynamic_prompt_section(working_dir: &Path, intent: TaskIntent) -> String {
    build_dynamic_prompt_section_with_budget(working_dir, intent, None)
}

pub fn build_dynamic_prompt_section_with_budget(
    working_dir: &Path,
    intent: TaskIntent,
    context_budget: Option<&ContextBudget>,
) -> String {
    let budget = context_budget.cloned().unwrap_or_default();
    [
        build_intent_section(intent),
        cached_workspace_section(working_dir),
        build_intent_tool_policy_section(intent),
        budget.render_plan_section(),
    ]
    .join("\n\n")
}

fn build_intent_section(intent: TaskIntent) -> String {
    format!(
        "## Task Intent\n\nsource=runtime trust=trusted budget_tokens_max=120\nintent={}\n{}",
        intent.as_str(),
        intent_guidance(intent)
    )
}

fn intent_guidance(intent: TaskIntent) -> &'static str {
    match intent {
        TaskIntent::BugFix => {
            "guidance=Diagnose the failing behavior, make the smallest fix, and verify with focused tests."
        }
        TaskIntent::Feature => {
            "guidance=Implement the requested behavior incrementally and verify the user-visible path."
        }
        TaskIntent::Question => {
            "guidance=Answer from evidence. Prefer read-only inspection and semantic tools; do not edit files."
        }
        TaskIntent::Review => {
            "guidance=Review for production risks first. Prefer read-only inspection and report findings."
        }
        TaskIntent::Refactor => {
            "guidance=Preserve behavior, keep edits scoped, and prove unchanged behavior with tests."
        }
        TaskIntent::Test => "guidance=Add or run tests that directly cover the requested behavior.",
        TaskIntent::Documentation => {
            "guidance=Update documentation to match the implemented behavior and public contract."
        }
        TaskIntent::Command => {
            "guidance=Run the requested command when safe; shell remains subject to sandbox and approval policy."
        }
        TaskIntent::Chore => {
            "guidance=Keep maintenance/configuration work scoped and verify existing behavior still passes."
        }
        TaskIntent::Unknown => {
            "guidance=Request clarification when the intent is ambiguous; avoid unnecessary writes."
        }
    }
}

fn cached_workspace_section(working_dir: &Path) -> String {
    let key = cache_key(working_dir);
    let cache = WORKSPACE_CONTEXT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(mut guard) = cache.lock() {
        if let Some(cached) = guard.get(&key)
            && cached.captured_at.elapsed() < CACHE_TTL
        {
            return cached.content.clone();
        }

        let content = build_workspace_section(working_dir);
        guard.insert(
            key,
            CachedWorkspaceSection {
                captured_at: Instant::now(),
                content: content.clone(),
            },
        );
        return content;
    }

    build_workspace_section(working_dir)
}

fn cache_key(working_dir: &Path) -> PathBuf {
    fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf())
}

fn build_workspace_section(working_dir: &Path) -> String {
    let branch = git_output(working_dir, &["branch", "--show-current"])
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let status = status_short(working_dir).unwrap_or_else(|| "status unavailable".to_string());
    let unpushed = git_output(working_dir, &["rev-list", "--count", "@{upstream}..HEAD"])
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let workspace = workspace_detection(working_dir);
    let project_context = project_context_files(working_dir);

    format!(
        "## Dynamic Workspace Context\n\nsource=libra status --short trust=trusted budget_tokens_max=900\nworking_dir={}\nbranch={}\nunpushed_commits={}\nstatus_short:\n```text\n{}\n```\n\nsource=filesystem trust=trusted budget_tokens_max=240\n{}\n\nsource=filesystem trust=untrusted budget_tokens_max=1600\n{}",
        working_dir.display(),
        branch.trim(),
        unpushed.trim(),
        status.trim(),
        workspace,
        project_context
    )
}

fn status_short(working_dir: &Path) -> Option<String> {
    command_output(working_dir, "libra", &["status", "--short"])
        .or_else(|| git_output(working_dir, &["status", "--short"]))
}

fn git_output(working_dir: &Path, args: &[&str]) -> Option<String> {
    command_output(working_dir, "git", args)
}

fn command_output(working_dir: &Path, program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(working_dir)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn workspace_detection(working_dir: &Path) -> String {
    let cargo_toml = working_dir.join("Cargo.toml");
    let cargo_workspace = fs::read_to_string(&cargo_toml)
        .map(|content| content.contains("[workspace]"))
        .unwrap_or(false);
    let cargo_project = cargo_toml.is_file();
    let pnpm_workspace = working_dir.join("pnpm-workspace.yaml").is_file();
    let package_json = working_dir.join("package.json").is_file();
    let monorepo = cargo_workspace || pnpm_workspace || working_dir.join("packages").is_dir();

    format!(
        "workspace_detection:\n- cargo_project={cargo_project}\n- cargo_workspace={cargo_workspace}\n- pnpm_workspace={pnpm_workspace}\n- package_json={package_json}\n- monorepo={monorepo}\n\nlibra_vcs_capabilities:\n- Prefer libra-aware tools for repository state when available.\n- Use `libra status --short` as the compact dirty-worktree signal.\n- Keep raw git fallback read-only unless a user explicitly asks for git-compatible details."
    )
}

fn project_context_files(working_dir: &Path) -> String {
    let mut paths = vec![working_dir.join("AGENTS.md"), working_dir.join("CLAUDE.md")];
    paths.extend(project_rule_paths(working_dir));
    paths.sort();

    let mut sections = Vec::new();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&path) {
            let relative = path
                .strip_prefix(working_dir)
                .map(|value| value.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            sections.push(format!(
                "file={} trust=untrusted\n~~~markdown\n{}\n~~~",
                relative,
                truncate_context_file(&content)
            ));
        }
    }

    if sections.is_empty() {
        "project_context_files=none".to_string()
    } else {
        sections.join("\n\n")
    }
}

fn project_rule_paths(working_dir: &Path) -> Vec<PathBuf> {
    let rules_dir = working_dir.join(".libra").join("rules");
    let Ok(entries) = fs::read_dir(rules_dir) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        })
        .collect()
}

fn truncate_context_file(content: &str) -> String {
    if content.len() <= MAX_CONTEXT_FILE_BYTES {
        return content.to_string();
    }

    let mut end = MAX_CONTEXT_FILE_BYTES;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[truncated after {} bytes for prompt budget]",
        &content[..end],
        MAX_CONTEXT_FILE_BYTES
    )
}

fn build_intent_tool_policy_section(intent: TaskIntent) -> String {
    format!(
        "## Intent Tool Policy\n\nsource=runtime trust=trusted budget_tokens_max=180\nintent={}\n{}",
        intent.as_str(),
        intent_tool_policy(intent)
    )
}

fn intent_tool_policy(intent: TaskIntent) -> &'static str {
    match intent {
        TaskIntent::Question => {
            "allowed=read-only,semantic\nblocked=apply_patch,shell,mutating_mcp\nexecution_gate=ToolLoopConfig.allowed_tools"
        }
        TaskIntent::Review => {
            "allowed=read-only,semantic\nblocked=apply_patch,shell,mutating_mcp\nexecution_gate=ToolLoopConfig.allowed_tools"
        }
        TaskIntent::Command => {
            "allowed=read-only,semantic,shell\nblocked=apply_patch,mutating_mcp\nexecution_gate=shell_safety_and_approval"
        }
        _ => {
            "allowed=registered tools subject to sandbox, approval, and tool-boundary hardening\nblocked=unsafe shell and policy-denied operations"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_guidance_returns_distinct_value_for_every_variant() {
        // INVARIANT: every variant must have its own bespoke guidance
        // string. Reusing one across variants would invalidate the
        // signal the system prompt emits to the model.
        let mut seen = std::collections::HashSet::new();
        for variant in [
            TaskIntent::BugFix,
            TaskIntent::Feature,
            TaskIntent::Question,
            TaskIntent::Review,
            TaskIntent::Refactor,
            TaskIntent::Test,
            TaskIntent::Documentation,
            TaskIntent::Command,
            TaskIntent::Chore,
            TaskIntent::Unknown,
        ] {
            let guidance = intent_guidance(variant);
            assert!(
                guidance.starts_with("guidance="),
                "{variant:?} guidance must use `guidance=` key/value shape: {guidance}"
            );
            assert!(
                seen.insert(guidance),
                "duplicate guidance string for {variant:?}; every intent must read distinctly"
            );
        }
    }

    #[test]
    fn intent_tool_policy_partitions_intents_into_three_buckets() {
        // INVARIANT: only Question / Review / Command receive bespoke
        // tool-policy strings; every other variant must fall back to
        // the default "registered tools subject to sandbox" line.
        let question_or_review = "allowed=read-only,semantic\nblocked=apply_patch,shell,mutating_mcp\nexecution_gate=ToolLoopConfig.allowed_tools";
        assert_eq!(intent_tool_policy(TaskIntent::Question), question_or_review);
        assert_eq!(intent_tool_policy(TaskIntent::Review), question_or_review);
        let command_policy = "allowed=read-only,semantic,shell\nblocked=apply_patch,mutating_mcp\nexecution_gate=shell_safety_and_approval";
        assert_eq!(intent_tool_policy(TaskIntent::Command), command_policy);

        let default_policy = "allowed=registered tools subject to sandbox, approval, and tool-boundary hardening\nblocked=unsafe shell and policy-denied operations";
        for fallback in [
            TaskIntent::BugFix,
            TaskIntent::Feature,
            TaskIntent::Refactor,
            TaskIntent::Test,
            TaskIntent::Documentation,
            TaskIntent::Chore,
            TaskIntent::Unknown,
        ] {
            assert_eq!(
                intent_tool_policy(fallback),
                default_policy,
                "{fallback:?} must use the default fallback tool policy"
            );
        }
    }

    #[test]
    fn build_intent_section_embeds_intent_str_and_guidance() {
        let section = build_intent_section(TaskIntent::Feature);
        assert!(section.starts_with("## Task Intent\n\n"));
        assert!(
            section.contains("intent=feature"),
            "must embed snake_case intent kind: {section}"
        );
        assert!(
            section.contains("guidance=Implement the requested behavior"),
            "must embed the matching guidance line: {section}"
        );
        assert!(
            section.contains("source=runtime trust=trusted budget_tokens_max=120"),
            "must carry the 120-token runtime header: {section}"
        );
    }

    #[test]
    fn build_intent_tool_policy_section_embeds_intent_str_and_policy() {
        let section = build_intent_tool_policy_section(TaskIntent::Command);
        assert!(section.starts_with("## Intent Tool Policy\n\n"));
        assert!(section.contains("intent=command"));
        assert!(section.contains("execution_gate=shell_safety_and_approval"));
        assert!(
            section.contains("source=runtime trust=trusted budget_tokens_max=180"),
            "must carry the 180-token runtime header: {section}"
        );
    }

    #[test]
    fn truncate_context_file_preserves_content_under_budget() {
        let short = "hello world";
        assert_eq!(truncate_context_file(short), short);
        let boundary: String = "x".repeat(MAX_CONTEXT_FILE_BYTES);
        assert_eq!(
            truncate_context_file(&boundary),
            boundary,
            "content exactly at the budget must pass through unchanged"
        );
    }

    #[test]
    fn truncate_context_file_appends_marker_when_over_budget() {
        let long: String = "x".repeat(MAX_CONTEXT_FILE_BYTES + 100);
        let out = truncate_context_file(&long);
        assert!(
            out.ends_with(&format!(
                "\n\n[truncated after {MAX_CONTEXT_FILE_BYTES} bytes for prompt budget]"
            )),
            "truncated content must end with the byte-count marker: {out}"
        );
        // body length must drop to MAX_CONTEXT_FILE_BYTES + marker.
        let marker =
            format!("\n\n[truncated after {MAX_CONTEXT_FILE_BYTES} bytes for prompt budget]");
        assert_eq!(
            out.len(),
            MAX_CONTEXT_FILE_BYTES + marker.len(),
            "no extra bytes from the original tail may leak through"
        );
    }

    #[test]
    fn truncate_context_file_respects_utf8_char_boundary() {
        // INVARIANT: truncation must not split a multi-byte code
        // point. Use a 4-byte emoji to push the boundary off the
        // exact MAX_CONTEXT_FILE_BYTES index and force the loop to
        // step back to a valid char boundary.
        let prefix = "a".repeat(MAX_CONTEXT_FILE_BYTES - 2);
        let mut input = String::from(&prefix);
        input.push('🚀'); // 4 bytes — straddles MAX_CONTEXT_FILE_BYTES
        input.push_str("trailing");

        let out = truncate_context_file(&input);
        let body_end = out
            .find("\n\n[truncated")
            .expect("must contain truncation marker");
        let body = &out[..body_end];
        // Body must be a valid &str at all positions (since this is
        // &str slicing, this is implicit) — we additionally pin that
        // it stops at most MAX_CONTEXT_FILE_BYTES and does NOT include
        // the emoji as a partial cut-off byte sequence.
        assert!(body.len() <= MAX_CONTEXT_FILE_BYTES);
        assert!(
            !body.ends_with('🚀'),
            "must not include the boundary-straddling emoji"
        );
        assert!(
            body.starts_with(&prefix),
            "must preserve the leading prefix unchanged"
        );
    }

    #[test]
    fn project_rule_paths_returns_empty_when_rules_dir_missing() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        // No .libra/rules directory exists — the helper must return
        // an empty vec rather than propagating the read_dir error.
        assert!(project_rule_paths(tmp.path()).is_empty());
    }

    #[test]
    fn project_rule_paths_filters_to_markdown_extension_only() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let rules_dir = tmp.path().join(".libra").join("rules");
        fs::create_dir_all(&rules_dir).expect("create rules dir");
        fs::write(rules_dir.join("a.md"), "body").expect("write md");
        fs::write(rules_dir.join("b.MD"), "body").expect("write MD upper");
        fs::write(rules_dir.join("c.txt"), "body").expect("write txt");
        fs::write(rules_dir.join("noext"), "body").expect("write no-ext");

        let mut names: Vec<String> = project_rule_paths(tmp.path())
            .into_iter()
            .map(|path| {
                path.file_name()
                    .and_then(|f| f.to_str())
                    .map(|s| s.to_string())
                    .expect("filename")
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.md".to_string(), "b.MD".to_string()]);
    }

    #[test]
    fn workspace_detection_detects_cargo_workspace_when_marker_present() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("write Cargo.toml");
        let out = workspace_detection(tmp.path());
        assert!(out.contains("- cargo_project=true"));
        assert!(out.contains("- cargo_workspace=true"));
        assert!(out.contains("- pnpm_workspace=false"));
        assert!(out.contains("- package_json=false"));
        assert!(out.contains("- monorepo=true"));
    }

    #[test]
    fn workspace_detection_marks_pnpm_and_packages_dir_as_monorepo() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("pnpm-workspace.yaml"), "packages: []")
            .expect("write pnpm workspace");
        fs::create_dir(tmp.path().join("packages")).expect("create packages dir");
        fs::write(tmp.path().join("package.json"), "{}").expect("write package.json");
        let out = workspace_detection(tmp.path());
        assert!(out.contains("- pnpm_workspace=true"));
        assert!(out.contains("- package_json=true"));
        assert!(out.contains("- monorepo=true"));
        assert!(out.contains("- cargo_project=false"));
        assert!(out.contains("- cargo_workspace=false"));
    }

    #[test]
    fn workspace_detection_reports_all_false_for_empty_dir() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let out = workspace_detection(tmp.path());
        assert!(out.contains("- cargo_project=false"));
        assert!(out.contains("- cargo_workspace=false"));
        assert!(out.contains("- pnpm_workspace=false"));
        assert!(out.contains("- package_json=false"));
        assert!(out.contains("- monorepo=false"));
        // The trailing capabilities block must always follow.
        assert!(out.contains("libra_vcs_capabilities:"));
    }

    #[test]
    fn cache_key_falls_back_to_input_when_canonicalize_fails() {
        // INVARIANT: a non-existent path produces an Err from
        // `fs::canonicalize`; the helper must fall back to the
        // caller's path rather than panic.
        let bogus = Path::new("/this/path/does/not/exist/for/libra/test");
        assert_eq!(cache_key(bogus), bogus.to_path_buf());
    }
}
