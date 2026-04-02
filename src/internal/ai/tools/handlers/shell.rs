//! Handler for the shell tool.
//!
//! Executes shell commands in the user's default shell with configurable
//! working directory and timeout. Output is capped to prevent memory issues.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use git_internal::hash::ObjectHash;

use super::parse_arguments;
use crate::{
    command::calc_file_blob_hash,
    internal::ai::tools::{
        context::{ShellArgs, ToolInvocation, ToolKind, ToolOutput, ToolPayload},
        error::{ToolError, ToolResult},
        registry::ToolHandler,
        spec::ToolSpec,
        utils::validate_path,
    },
    utils::util,
};

/// Handler for executing shell commands.
pub struct ShellHandler;

#[derive(Clone, Debug, Default)]
struct WorkspaceSnapshot {
    entries: BTreeMap<PathBuf, WorkspaceEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum WorkspaceEntry {
    File(ObjectHash),
    Symlink(PathBuf),
}

/// Maximum bytes captured per stream (stdout or stderr) before truncation.
const DEFAULT_MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100 KiB
/// Exit code emitted when a command is killed due to timeout (matches GNU timeout).
#[cfg(test)]
const TIMEOUT_EXIT_CODE: i32 = 124;

#[async_trait]
impl ToolHandler for ShellHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    /// Shell commands always mutate the environment.
    async fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        let ToolInvocation {
            call_id,
            payload,
            working_dir,
            runtime_context,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "shell handler only accepts Function payloads".to_string(),
                ));
            }
        };

        let args: ShellArgs = parse_arguments(&arguments)?;

        // Resolve and validate the execution working directory.
        let cwd = resolve_workdir(args.workdir.as_deref(), &working_dir)?;

        let max_output_bytes = runtime_context
            .as_ref()
            .and_then(|ctx| ctx.max_output_bytes)
            .unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);
        let sandbox_runtime = runtime_context
            .as_ref()
            .and_then(|ctx| ctx.sandbox_runtime.clone());
        let approval = runtime_context
            .as_ref()
            .and_then(|ctx| ctx.approval.clone());
        let sandbox = runtime_context.as_ref().and_then(|ctx| {
            ctx.sandbox.clone().map(|mut sandbox| {
                sandbox.permissions = args.sandbox_permissions;
                sandbox
            })
        });
        let baseline_snapshot = snapshot_workspace(&working_dir).map_err(|err| {
            ToolError::ExecutionFailed(format!("failed to snapshot workspace: {err}"))
        })?;

        let output = crate::internal::ai::sandbox::run_shell_command_with_approval(
            crate::internal::ai::sandbox::ShellCommandRequest {
                call_id,
                command: args.command,
                cwd,
                timeout_ms: args.timeout_ms,
                max_output_bytes,
                sandbox,
                sandbox_runtime,
                approval,
                justification: args.justification,
            },
        )
        .await
        .map_err(ToolError::ExecutionFailed)?;
        let final_snapshot = snapshot_workspace(&working_dir).map_err(|err| {
            ToolError::ExecutionFailed(format!(
                "failed to inspect workspace changes after shell command: {err}"
            ))
        })?;
        let metadata = serde_json::json!({
            "paths_written": changed_paths_since_baseline(
                &baseline_snapshot,
                &final_snapshot,
                &working_dir,
            ),
        });

        let formatted = format_output(
            output.exit_code,
            &output.stdout,
            &output.stderr,
            output.timed_out,
        );
        let rendered = if output.exit_code == 0 {
            ToolOutput::success(formatted)
        } else {
            ToolOutput::failure(formatted)
        };
        Ok(rendered.with_metadata(metadata))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::shell()
    }
}

// ── Output formatting ─────────────────────────────────────────────────────────

fn format_output(exit_code: i32, stdout: &str, stderr: &str, timed_out: bool) -> String {
    let mut parts: Vec<String> = Vec::new();

    if timed_out {
        parts.push("[Command timed out]".to_string());
    }
    parts.push(format!("Exit code: {exit_code}"));

    if !stdout.is_empty() {
        parts.push(String::new()); // blank separator line
        parts.push(stdout.to_string());
    }
    if !stderr.is_empty() {
        parts.push("[stderr]".to_string());
        parts.push(stderr.to_string());
    }

    parts.join("\n")
}

fn resolve_workdir(requested_workdir: Option<&str>, working_dir: &Path) -> ToolResult<PathBuf> {
    let Some(workdir) = requested_workdir else {
        return Ok(working_dir.to_path_buf());
    };

    let requested = Path::new(workdir);
    validate_path(requested, working_dir)?;

    let requested_canon = std::fs::canonicalize(requested).map_err(|e| {
        ToolError::ExecutionFailed(format!(
            "failed to canonicalize workdir '{}': {e}",
            requested.display()
        ))
    })?;
    let working_dir_canon = std::fs::canonicalize(working_dir).map_err(|e| {
        ToolError::ExecutionFailed(format!(
            "failed to canonicalize working_dir '{}': {e}",
            working_dir.display()
        ))
    })?;

    if !crate::utils::util::is_sub_path(&requested_canon, &working_dir_canon) {
        return Err(ToolError::PathOutsideWorkingDir(requested.to_path_buf()));
    }

    Ok(requested_canon)
}

fn snapshot_workspace(root: &Path) -> io::Result<WorkspaceSnapshot> {
    fn visit_dir(
        root: &Path,
        dir: &Path,
        entries: &mut BTreeMap<PathBuf, WorkspaceEntry>,
    ) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if protected_workspace_entry(&entry.file_name()) {
                continue;
            }

            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                visit_dir(root, &path, entries)?;
                continue;
            }

            let rel = path
                .strip_prefix(root)
                .map_err(|err| io::Error::other(err.to_string()))?
                .to_path_buf();
            entries.insert(rel, snapshot_entry(&path, &file_type)?);
        }
        Ok(())
    }

    let mut entries = BTreeMap::new();
    visit_dir(root, root, &mut entries)?;
    Ok(WorkspaceSnapshot { entries })
}

fn protected_workspace_entry(file_name: &std::ffi::OsStr) -> bool {
    file_name
        .to_str()
        .is_some_and(|name| matches!(name, ".git" | ".libra" | ".codex" | ".agents"))
}

fn snapshot_entry(path: &Path, file_type: &fs::FileType) -> io::Result<WorkspaceEntry> {
    if file_type.is_symlink() {
        return Ok(WorkspaceEntry::Symlink(fs::read_link(path)?));
    }

    Ok(WorkspaceEntry::File(calc_file_blob_hash(path)?))
}

fn changed_paths_since_baseline(
    baseline: &WorkspaceSnapshot,
    current: &WorkspaceSnapshot,
    root: &Path,
) -> Vec<String> {
    baseline
        .entries
        .keys()
        .chain(current.entries.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| baseline.entries.get(path) != current.entries.get(path))
        .map(|path| {
            util::workdir_to_relative(root.join(path), root)
                .to_string_lossy()
                .to_string()
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::internal::ai::tools::context::ToolPayload;

    fn make_invocation(args: serde_json::Value, working_dir: std::path::PathBuf) -> ToolInvocation {
        ToolInvocation::new(
            "call-1",
            "shell",
            ToolPayload::Function {
                arguments: args.to_string(),
            },
            working_dir,
        )
    }

    // ── Basic execution ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_shell_echo() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({ "command": "echo hello" }),
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();
        assert!(result.is_success());
        assert!(result.as_text().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn test_shell_exit_code_zero() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({ "command": "true" }),
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();
        assert!(result.is_success());
        assert!(result.as_text().unwrap().contains("Exit code: 0"));
    }

    #[tokio::test]
    async fn test_shell_exit_code_nonzero() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({ "command": "exit 42" }),
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();
        assert!(!result.is_success());
        assert!(result.as_text().unwrap().contains("Exit code: 42"));
    }

    #[tokio::test]
    async fn test_shell_multiline_output() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({ "command": "printf 'line1\\nline2\\nline3\\n'" }),
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();
        let text = result.as_text().unwrap();
        assert!(text.contains("line1"), "{text}");
        assert!(text.contains("line2"), "{text}");
        assert!(text.contains("line3"), "{text}");
    }

    // ── Stderr ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_shell_stderr_captured() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({ "command": "echo error_msg >&2" }),
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();
        let text = result.as_text().unwrap();
        assert!(text.contains("error_msg"), "stderr not captured:\n{text}");
    }

    #[tokio::test]
    async fn test_shell_stderr_section_label() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({ "command": "echo out; echo err >&2" }),
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();
        let text = result.as_text().unwrap();
        assert!(
            text.contains("[stderr]"),
            "expected [stderr] label:\n{text}"
        );
        assert!(text.contains("out"), "{text}");
        assert!(text.contains("err"), "{text}");
    }

    // ── Working directory ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_shell_workdir_default() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path().to_path_buf();
        let inv = make_invocation(serde_json::json!({ "command": "pwd" }), working_dir.clone());
        let result = ShellHandler.handle(inv).await.unwrap();
        let text = result.as_text().unwrap();
        // Compare the last path component to avoid macOS /tmp → /private/tmp symlink issues.
        let dir_name = working_dir.file_name().unwrap().to_str().unwrap();
        assert!(
            text.contains(dir_name),
            "expected dir name in output:\n{text}"
        );
    }

    #[tokio::test]
    async fn test_shell_workdir_override() {
        let outer = TempDir::new().unwrap();
        let inner_path = outer.path().join("inner_subdir");
        std::fs::create_dir(&inner_path).unwrap();

        let inv = make_invocation(
            serde_json::json!({
                "command": "pwd",
                "workdir": inner_path.to_str().unwrap()
            }),
            outer.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();
        let text = result.as_text().unwrap();
        assert!(
            text.contains("inner_subdir"),
            "expected inner_subdir in output:\n{text}"
        );
    }

    #[tokio::test]
    async fn test_shell_workdir_outside_sandbox_fails() {
        let sandbox = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();

        let inv = make_invocation(
            serde_json::json!({
                "command": "pwd",
                "workdir": outside.path().to_str().unwrap()
            }),
            sandbox.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await;
        assert!(
            matches!(result, Err(ToolError::PathOutsideWorkingDir(_))),
            "expected PathOutsideWorkingDir, got: {result:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_shell_workdir_symlink_escape_fails() {
        use std::os::unix::fs::symlink;

        let sandbox = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let link_path = sandbox.path().join("escape");
        symlink(outside.path(), &link_path).unwrap();

        let inv = make_invocation(
            serde_json::json!({
                "command": "pwd",
                "workdir": link_path.to_str().unwrap()
            }),
            sandbox.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await;
        assert!(
            matches!(result, Err(ToolError::PathOutsideWorkingDir(_))),
            "expected PathOutsideWorkingDir, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_shell_workdir_relative_path_fails() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({
                "command": "pwd",
                "workdir": "relative/path"
            }),
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await;
        assert!(
            matches!(result, Err(ToolError::PathNotAbsolute(_))),
            "expected PathNotAbsolute, got: {result:?}"
        );
    }

    // ── Timeout ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_shell_timeout() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({ "command": "sleep 60", "timeout_ms": 200 }),
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();
        let text = result.as_text().unwrap();
        assert!(
            text.contains("[Command timed out]"),
            "expected timeout notice:\n{text}"
        );
        assert!(
            text.contains(&format!("Exit code: {TIMEOUT_EXIT_CODE}")),
            "expected exit code {TIMEOUT_EXIT_CODE}:\n{text}"
        );
        assert!(!result.is_success());
    }

    #[tokio::test]
    async fn test_shell_background_child_does_not_hang() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({ "command": "sleep 5 & echo done", "timeout_ms": 5000 }),
            temp.path().to_path_buf(),
        );

        let result = tokio::time::timeout(Duration::from_secs(2), ShellHandler.handle(inv))
            .await
            .expect("shell handler should not hang")
            .unwrap();
        let text = result.as_text().unwrap();
        assert!(text.contains("Exit code: 0"), "{text}");
        assert!(text.contains("done"), "{text}");
    }

    // ── Output truncation ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_shell_large_output_truncated() {
        let temp = TempDir::new().unwrap();
        // seq 1 200000 produces ~1.4 MB; MAX_OUTPUT_BYTES = 100 KiB → should truncate.
        let inv = make_invocation(
            serde_json::json!({ "command": "seq 1 200000" }),
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();
        let text = result.as_text().unwrap();
        assert!(
            text.contains("truncated"),
            "expected truncation notice:\n{text}"
        );
    }

    #[tokio::test]
    async fn test_shell_metadata_tracks_written_paths() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({ "command": "printf 'hello\\n' > touched.txt" }),
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();

        let metadata = result
            .metadata()
            .expect("shell results should include metadata");
        assert_eq!(
            metadata["paths_written"],
            serde_json::json!(["touched.txt"])
        );
    }

    // ── Handler metadata ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_shell_kind_is_function() {
        assert_eq!(ShellHandler.kind(), ToolKind::Function);
    }

    #[tokio::test]
    async fn test_shell_schema() {
        let schema = ShellHandler.schema();
        assert_eq!(schema.function.name, "shell");
        assert!(schema.function.description.len() > 10);
        let json = schema.to_json();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "shell");
        // "command" must be a required parameter.
        let required = json["function"]["parameters"]["required"]
            .as_array()
            .unwrap();
        assert!(
            required.iter().any(|v| v == "command"),
            "command should be required"
        );
    }

    #[tokio::test]
    async fn test_shell_is_mutating() {
        let temp = TempDir::new().unwrap();
        let inv = make_invocation(
            serde_json::json!({ "command": "echo hi" }),
            temp.path().to_path_buf(),
        );
        assert!(ShellHandler.is_mutating(&inv).await);
    }

    #[tokio::test]
    async fn test_shell_incompatible_payload() {
        let temp = TempDir::new().unwrap();
        let inv = ToolInvocation::new(
            "call-1",
            "shell",
            ToolPayload::Custom {
                input: "test".to_string(),
            },
            temp.path().to_path_buf(),
        );
        let result = ShellHandler.handle(inv).await;
        assert!(
            matches!(result, Err(ToolError::IncompatiblePayload(_))),
            "expected IncompatiblePayload"
        );
    }

    // ── Unit tests for internal helpers ───────────────────────────────────────

    #[test]
    fn test_format_output_success_stdout_only() {
        let text = format_output(0, "hello world\n", "", false);
        assert!(text.contains("Exit code: 0"));
        assert!(text.contains("hello world"));
        assert!(!text.contains("[stderr]"));
        assert!(!text.contains("[Command timed out]"));
    }

    #[test]
    fn test_format_output_failure_with_stderr() {
        let text = format_output(1, "", "error occurred\n", false);
        assert!(text.contains("Exit code: 1"));
        assert!(text.contains("[stderr]"));
        assert!(text.contains("error occurred"));
    }

    #[test]
    fn test_format_output_timed_out() {
        let text = format_output(TIMEOUT_EXIT_CODE, "", "", true);
        assert!(text.contains("[Command timed out]"));
        assert!(text.contains(&format!("Exit code: {TIMEOUT_EXIT_CODE}")));
    }

    #[test]
    fn test_format_output_both_streams() {
        let text = format_output(0, "stdout content\n", "stderr content\n", false);
        assert!(text.contains("stdout content"));
        assert!(text.contains("[stderr]"));
        assert!(text.contains("stderr content"));
    }
}
