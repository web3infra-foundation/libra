//! Final gate evaluation for orchestrated AI runs.
//!
//! Boundary: gates combine verifier output, policy violations, and timing metadata into
//! a pass/fail result; they do not modify objects or worktrees. Validation-decision
//! tests cover accepted, rejected, and incomplete evidence outcomes.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    time::Instant,
};

use serde_json::json;

use super::{
    policy,
    types::{GateReport, GateResult, TaskSpec},
};
use crate::internal::ai::{
    intentspec::types::{Check, CheckKind, IntentSpec},
    sandbox::{ToolRuntimeContext, run_shell_command},
};

const DEFAULT_TIMEOUT_SECS: u64 = 900;
#[cfg(test)]
const TIMEOUT_EXIT_CODE: i32 = 124;
const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Execute a single verification check and return its result.
pub async fn run_check(check: &Check, working_dir: &Path) -> GateResult {
    run_check_with_context(check, working_dir, None, None, None).await
}

pub async fn run_check_with_context(
    check: &Check,
    working_dir: &Path,
    spec: Option<&IntentSpec>,
    task: Option<&TaskSpec>,
    runtime_context: Option<&ToolRuntimeContext>,
) -> GateResult {
    match check.kind {
        CheckKind::Policy | CheckKind::Command | CheckKind::TestSuite => {
            run_command_check(check, working_dir, spec, task, runtime_context).await
        }
    }
}

/// Execute multiple checks sequentially and aggregate results.
pub async fn run_gates(checks: &[Check], working_dir: &Path) -> GateReport {
    run_gates_with_context(checks, working_dir, None, None, None).await
}

pub async fn run_gates_with_context(
    checks: &[Check],
    working_dir: &Path,
    spec: Option<&IntentSpec>,
    task: Option<&TaskSpec>,
    runtime_context: Option<&ToolRuntimeContext>,
) -> GateReport {
    let mut results = Vec::with_capacity(checks.len());
    let mut all_required_passed = true;

    for check in checks {
        let result = run_check_with_context(check, working_dir, spec, task, runtime_context).await;
        if !result.passed && check.required {
            all_required_passed = false;
        }
        results.push(result);
    }

    GateReport {
        results,
        all_required_passed,
    }
}

async fn run_command_check(
    check: &Check,
    working_dir: &Path,
    spec: Option<&IntentSpec>,
    task: Option<&TaskSpec>,
    runtime_context: Option<&ToolRuntimeContext>,
) -> GateResult {
    let command = match &check.command {
        Some(cmd) => cmd.clone(),
        None => {
            return GateResult {
                check_id: check.id.clone(),
                kind: format!("{:?}", check.kind).to_lowercase(),
                passed: false,
                exit_code: -1,
                stdout: String::new(),
                stderr: "no command specified".into(),
                duration_ms: 0,
                timed_out: false,
            };
        }
    };

    let effective_working_dir = effective_check_working_dir(&command, working_dir, task)
        .unwrap_or_else(|| working_dir.to_path_buf());

    if let (Some(spec), Some(task)) = (spec, task) {
        let args = json!({ "command": command });
        if let Err(violation) =
            policy::evaluate_tool_call(spec, task, "shell", &args, &effective_working_dir)
        {
            return GateResult {
                check_id: check.id.clone(),
                kind: format!("{:?}", check.kind).to_lowercase(),
                passed: false,
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("policy preflight rejected check: {}", violation.message),
                duration_ms: 0,
                timed_out: false,
            };
        }
    }

    let timeout_secs = check.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let expected_exit = check.expected_exit_code.unwrap_or(0);
    let start = Instant::now();
    let max_output_bytes = runtime_context
        .and_then(|ctx| ctx.max_output_bytes)
        .unwrap_or(MAX_OUTPUT_BYTES);
    let sandbox = runtime_context.and_then(|ctx| ctx.sandbox.clone());
    let sandbox_runtime = runtime_context.and_then(|ctx| ctx.sandbox_runtime.as_ref());

    let result = run_shell_command(
        &command,
        &effective_working_dir,
        Some(timeout_secs.saturating_mul(1000)),
        max_output_bytes,
        sandbox,
        sandbox_runtime,
    )
    .await;

    let (exit_code, stdout, stderr, timed_out) = match result {
        Ok(output) => (
            output.exit_code,
            output.stdout,
            output.stderr,
            output.timed_out,
        ),
        Err(err) => (-1, String::new(), err, false),
    };

    let duration_ms = start.elapsed().as_millis() as u64;
    let passed = !timed_out && exit_code == expected_exit;

    GateResult {
        check_id: check.id.clone(),
        kind: format!("{:?}", check.kind).to_lowercase(),
        passed,
        exit_code,
        stdout,
        stderr,
        duration_ms,
        timed_out,
    }
}

fn effective_check_working_dir(
    command: &str,
    working_dir: &Path,
    task: Option<&TaskSpec>,
) -> Option<PathBuf> {
    let marker = project_marker_for_command(command)?;
    if working_dir.join(marker.filename).is_file() {
        // Project root sits at working_dir already; no relocation needed.
        return None;
    }

    // First, prefer a manifest path declared on the task contract — that is
    // authoritative when present. Fall back to a single-subdir filesystem walk
    // so gates still locate the project when the contract is too coarse (e.g.
    // a generic gate inheriting only an empty contract). The walk is bounded
    // to direct children to avoid surprising paths.
    if let Some(task) = task
        && let Some(dir) = task_scoped_manifest_dir(working_dir, task, marker.filename)
    {
        return Some(dir);
    }
    discover_single_child_with_marker(working_dir, marker.filename)
}

/// Recognise a build command and return its project-root marker file.
struct ProjectMarker {
    filename: &'static str,
}

fn project_marker_for_command(command: &str) -> Option<ProjectMarker> {
    if command.contains("--manifest-path") {
        return None;
    }

    let token = first_shell_token_after_env_assignments(command)?;
    match token {
        // Rust toolchain — Cargo.toml at the crate root.
        "cargo" | "cross" | "wasm-pack" | "trunk" | "maturin" => Some(ProjectMarker {
            filename: "Cargo.toml",
        }),
        // JS/TS toolchain — package.json at the project root.
        "npm" | "pnpm" | "yarn" | "bun" | "npx" | "pnpx" => Some(ProjectMarker {
            filename: "package.json",
        }),
        _ => None,
    }
}

/// Walk the immediate children of `working_dir` and return the one containing
/// `marker_filename` if and only if exactly one such child exists. Returning
/// `None` on ambiguity is intentional: silently picking a project root in a
/// multi-package workspace would be more confusing than the original error.
fn discover_single_child_with_marker(working_dir: &Path, marker_filename: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(working_dir).ok()?;
    let mut found: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !file_type.is_dir() {
            continue;
        }
        if path.join(marker_filename).is_file() {
            if found.is_some() {
                // Multiple candidates — bail out.
                return None;
            }
            found = Some(path);
        }
    }
    found
}

fn first_shell_token_after_env_assignments(command: &str) -> Option<&str> {
    command
        .split_whitespace()
        .find(|token| !is_env_assignment_token(token))
}

fn is_env_assignment_token(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn task_scoped_manifest_dir(
    working_dir: &Path,
    task: &TaskSpec,
    marker_filename: &str,
) -> Option<PathBuf> {
    let mut candidates = BTreeSet::new();

    for raw_path in task
        .contract
        .touch_files
        .iter()
        .chain(task.contract.write_scope.iter())
        .chain(task.scope_in.iter())
    {
        let Some(relative) = scoped_relative_path(working_dir, raw_path) else {
            continue;
        };

        let manifest = if relative
            .file_name()
            .is_some_and(|name| name == marker_filename)
        {
            working_dir.join(&relative)
        } else {
            working_dir.join(&relative).join(marker_filename)
        };

        if manifest.is_file()
            && let Some(parent) = manifest.parent()
        {
            candidates.insert(parent.to_path_buf());
        }
    }

    if candidates.len() == 1 {
        candidates.into_iter().next()
    } else {
        None
    }
}

fn scoped_relative_path(working_dir: &Path, raw_path: &str) -> Option<PathBuf> {
    let trimmed = raw_path.trim().trim_start_matches("./");
    if trimmed.is_empty() {
        return None;
    }

    let path = Path::new(trimmed);
    let relative = if path.is_absolute() {
        path.strip_prefix(working_dir).ok()?.to_path_buf()
    } else {
        path.to_path_buf()
    };

    if relative.components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        return None;
    }

    Some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_check(id: &str, kind: CheckKind, command: Option<&str>) -> Check {
        Check {
            id: id.into(),
            kind,
            command: command.map(String::from),
            timeout_seconds: Some(10),
            expected_exit_code: None,
            required: true,
            artifacts_produced: vec![],
        }
    }

    #[tokio::test]
    async fn test_run_check_command_true() {
        let check = make_check("t1", CheckKind::Command, Some("true"));
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(result.passed);
        assert_eq!(result.exit_code, 0);
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_run_check_command_false() {
        let check = make_check("t2", CheckKind::Command, Some("false"));
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(!result.passed);
        assert_ne!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_run_check_timeout() {
        let check = Check {
            id: "t3".into(),
            kind: CheckKind::Command,
            command: Some("sleep 60".into()),
            timeout_seconds: Some(1),
            expected_exit_code: None,
            required: true,
            artifacts_produced: vec![],
        };
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(!result.passed);
        assert!(result.timed_out);
        assert_eq!(result.exit_code, TIMEOUT_EXIT_CODE);
    }

    #[tokio::test]
    async fn test_run_check_expected_exit_code() {
        let check = Check {
            id: "t4".into(),
            kind: CheckKind::Command,
            command: Some("exit 42".into()),
            timeout_seconds: Some(10),
            expected_exit_code: Some(42),
            required: true,
            artifacts_produced: vec![],
        };
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(result.passed);
        assert_eq!(result.exit_code, 42);
    }

    #[tokio::test]
    async fn test_run_check_policy_passthrough() {
        let check = make_check("t5", CheckKind::Policy, Some("true"));
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(result.passed);
        assert_eq!(result.kind, "policy");
    }

    #[tokio::test]
    async fn test_run_check_no_command() {
        let check = make_check("t6", CheckKind::Command, None);
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(!result.passed);
        assert_eq!(result.exit_code, -1);
    }

    #[tokio::test]
    async fn test_run_gates_aggregate() {
        let checks = vec![
            make_check("g1", CheckKind::Command, Some("true")),
            make_check("g2", CheckKind::Command, Some("true")),
        ];
        let dir = tempfile::tempdir().unwrap();
        let report = run_gates(&checks, dir.path()).await;
        assert!(report.all_required_passed);
        assert_eq!(report.results.len(), 2);
    }

    #[tokio::test]
    async fn test_run_gates_required_failure() {
        let checks = vec![
            make_check("g3", CheckKind::Command, Some("true")),
            make_check("g4", CheckKind::Command, Some("false")),
        ];
        let dir = tempfile::tempdir().unwrap();
        let report = run_gates(&checks, dir.path()).await;
        assert!(!report.all_required_passed);
    }

    #[tokio::test]
    async fn test_run_gates_optional_failure() {
        let checks = vec![Check {
            id: "g5".into(),
            kind: CheckKind::Command,
            command: Some("false".into()),
            timeout_seconds: Some(10),
            expected_exit_code: None,
            required: false,
            artifacts_produced: vec![],
        }];
        let dir = tempfile::tempdir().unwrap();
        let report = run_gates(&checks, dir.path()).await;
        assert!(report.all_required_passed);
        assert!(!report.results[0].passed);
    }

    #[tokio::test]
    async fn test_run_check_captures_stdout() {
        let check = make_check("t7", CheckKind::Command, Some("echo hello"));
        let dir = tempfile::tempdir().unwrap();
        let result = run_check(&check, dir.path()).await;
        assert!(result.passed);
        assert!(result.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_run_check_uses_task_local_build_environment() {
        let temp = tempfile::tempdir().unwrap();
        let worktree_root = temp
            .path()
            .join(".libra/worktrees/tasks/libra-task-worktree-copy-123-019e");
        let workspace = worktree_root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let worktree_root = worktree_root.canonicalize().unwrap();
        let workspace = worktree_root.join("workspace");
        let check = make_check(
            "t8",
            CheckKind::Command,
            Some("printf '%s\\n%s\\n' \"$HOME\" \"$CARGO_HOME\""),
        );

        let result = run_check(&check, &workspace).await;

        assert!(result.passed, "stderr: {}", result.stderr);
        let lines = result.stdout.lines().collect::<Vec<_>>();
        let expected = vec![
            worktree_root.join("home").to_string_lossy().into_owned(),
            worktree_root
                .join("cargo-home")
                .to_string_lossy()
                .into_owned(),
        ];
        assert_eq!(
            lines,
            expected.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    #[test]
    fn project_marker_recognises_known_toolchains() {
        assert!(project_marker_for_command("cargo fmt --all --check").is_some());
        assert!(project_marker_for_command("cross build --release").is_some());
        assert!(project_marker_for_command("wasm-pack build --target web").is_some());
        assert!(project_marker_for_command("npm run lint").is_some());
        assert!(project_marker_for_command("pnpm test").is_some());
        assert!(project_marker_for_command("yarn build").is_some());
        // Explicit manifest path opts out of relocation.
        assert!(
            project_marker_for_command("cargo fmt --manifest-path foo/Cargo.toml --all").is_none()
        );
        // Env assignments are skipped when locating the program token.
        assert!(
            project_marker_for_command("CARGO_TARGET_DIR=/tmp/t cargo clippy -- -D warnings")
                .is_some()
        );
        // Unknown commands are not relocated.
        assert!(project_marker_for_command("make test").is_none());
    }

    #[test]
    fn discover_single_child_with_marker_finds_unique_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let project = root.join("linked");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]\nname=\"linked\"").unwrap();
        // A sibling without Cargo.toml must not confuse the search.
        std::fs::create_dir(root.join("docs")).unwrap();

        let found = discover_single_child_with_marker(root, "Cargo.toml");
        assert_eq!(found.as_deref(), Some(project.as_path()));
    }

    #[test]
    fn discover_single_child_with_marker_bails_on_ambiguity() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["a", "b"] {
            let sub = root.join(name);
            std::fs::create_dir(&sub).unwrap();
            std::fs::write(sub.join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        }
        assert!(discover_single_child_with_marker(root, "Cargo.toml").is_none());
    }

    #[test]
    fn effective_check_working_dir_relocates_subdir_cargo_project_via_walk() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let project = root.join("linked");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]\nname=\"linked\"").unwrap();

        // No task contract — relocation should fall back to filesystem walk.
        let dir_picked = effective_check_working_dir("cargo fmt --all --check", root, None);
        assert_eq!(dir_picked.as_deref(), Some(project.as_path()));
    }

    #[test]
    fn effective_check_working_dir_keeps_root_when_manifest_present() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        // Even though a subdir has its own Cargo.toml, the root manifest wins.
        let project = root.join("linked");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]\nname=\"linked\"").unwrap();

        let dir_picked = effective_check_working_dir("cargo fmt --all --check", root, None);
        assert!(dir_picked.is_none());
    }

    #[test]
    fn effective_check_working_dir_relocates_npm_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let project = root.join("web");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("package.json"), "{\"name\":\"web\"}").unwrap();

        let dir_picked = effective_check_working_dir("npm run lint", root, None);
        assert_eq!(dir_picked.as_deref(), Some(project.as_path()));
    }

    #[test]
    fn effective_check_working_dir_ignores_unknown_command() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let project = root.join("linked");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]\nname=\"linked\"").unwrap();

        let dir_picked = effective_check_working_dir("make test", root, None);
        assert!(dir_picked.is_none());
    }
}
