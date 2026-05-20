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
