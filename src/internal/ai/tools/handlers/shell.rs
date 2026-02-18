//! Handler for the shell tool.
//!
//! Executes shell commands in the user's default shell with configurable
//! working directory and timeout. Output is capped to prevent memory issues.

use std::path::Path;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::Duration;

use super::parse_arguments;
use crate::internal::ai::tools::{
    context::{ShellArgs, ToolInvocation, ToolKind, ToolOutput, ToolPayload},
    error::{ToolError, ToolResult},
    registry::ToolHandler,
    spec::ToolSpec,
    utils::validate_path,
};

/// Handler for executing shell commands.
pub struct ShellHandler;

/// Default timeout: 10 seconds (matches codex default).
const DEFAULT_TIMEOUT_MS: u64 = 10_000;
/// Maximum bytes captured per stream (stdout or stderr) before truncation.
const MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100 KiB
/// Exit code emitted when a command is killed due to timeout (matches GNU timeout).
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
            payload,
            working_dir,
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
        let cwd = if let Some(ref workdir) = args.workdir {
            let p = Path::new(workdir);
            validate_path(p, &working_dir)?;
            p.to_path_buf()
        } else {
            working_dir
        };

        let output = run_shell(&args.command, &cwd, args.timeout_ms).await?;

        let formatted = format_output(output.exit_code, &output.stdout, &output.stderr, output.timed_out);
        if output.exit_code == 0 {
            Ok(ToolOutput::success(formatted))
        } else {
            Ok(ToolOutput::failure(formatted))
        }
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::shell()
    }
}

// ── Internal types ────────────────────────────────────────────────────────────

struct ExecOutput {
    exit_code: i32,
    stdout: String,
    stderr: String,
    timed_out: bool,
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

// ── Execution ─────────────────────────────────────────────────────────────────

async fn run_shell(
    command: &str,
    cwd: &Path,
    timeout_ms: Option<u64>,
) -> ToolResult<ExecOutput> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let timeout_dur = Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));

    let mut cmd = Command::new(&shell);
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| ToolError::ExecutionFailed(format!("failed to spawn shell: {e}")))?;

    // Take pipe handles before waiting to avoid deadlocks on large output.
    let stdout_pipe = child.stdout.take().expect("stdout is piped");
    let stderr_pipe = child.stderr.take().expect("stderr is piped");

    // Drain both streams concurrently. Continuing to drain after MAX_OUTPUT_BYTES
    // prevents the process from blocking on a full pipe buffer.
    let stdout_task = tokio::spawn(async move { drain_reader(stdout_pipe, MAX_OUTPUT_BYTES).await });
    let stderr_task = tokio::spawn(async move { drain_reader(stderr_pipe, MAX_OUTPUT_BYTES).await });

    let (exit_code, timed_out) = tokio::select! {
        status = child.wait() => {
            let code = status
                .map_err(|e| ToolError::ExecutionFailed(format!("wait failed: {e}")))?
                .code()
                .unwrap_or(-1);
            (code, false)
        }
        _ = tokio::time::sleep(timeout_dur) => {
            // Kill the process and reap the zombie before collecting output.
            let _ = child.kill().await;
            let _ = child.wait().await;
            (TIMEOUT_EXIT_CODE, true)
        }
    };

    // Both tasks finish once the pipe write-ends are closed (process exit or kill).
    let stdout_bytes = stdout_task.await.unwrap_or_default();
    let stderr_bytes = stderr_task.await.unwrap_or_default();

    let (mut stdout, stdout_truncated) = bytes_to_string(stdout_bytes, MAX_OUTPUT_BYTES);
    let (mut stderr, stderr_truncated) = bytes_to_string(stderr_bytes, MAX_OUTPUT_BYTES);

    if stdout_truncated {
        stdout.push_str("\n[stdout truncated]");
    }
    if stderr_truncated {
        stderr.push_str("\n[stderr truncated]");
    }

    Ok(ExecOutput {
        exit_code,
        stdout,
        stderr,
        timed_out,
    })
}

/// Reads from `reader` into a byte buffer, capping at `max_bytes`.
/// Continues draining after the cap to prevent pipe-buffer deadlock.
async fn drain_reader(
    mut reader: impl AsyncReadExt + Unpin,
    max_bytes: usize,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4096_usize.min(max_bytes));
    let mut tmp = [0u8; 8192];
    let mut buf_full = false;
    loop {
        match reader.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if !buf_full {
                    let take = max_bytes.saturating_sub(buf.len()).min(n);
                    buf.extend_from_slice(&tmp[..take]);
                    if buf.len() >= max_bytes {
                        buf_full = true;
                    }
                }
                // When buf_full, keep reading and discarding to unblock the writer.
            }
        }
    }
    buf
}

fn bytes_to_string(bytes: Vec<u8>, max_bytes: usize) -> (String, bool) {
    let truncated = bytes.len() >= max_bytes;
    (String::from_utf8_lossy(&bytes).into_owned(), truncated)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
        assert!(text.contains("[stderr]"), "expected [stderr] label:\n{text}");
        assert!(text.contains("out"), "{text}");
        assert!(text.contains("err"), "{text}");
    }

    // ── Working directory ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_shell_workdir_default() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path().to_path_buf();
        let inv = make_invocation(
            serde_json::json!({ "command": "pwd" }),
            working_dir.clone(),
        );
        let result = ShellHandler.handle(inv).await.unwrap();
        let text = result.as_text().unwrap();
        // Compare the last path component to avoid macOS /tmp → /private/tmp symlink issues.
        let dir_name = working_dir.file_name().unwrap().to_str().unwrap();
        assert!(text.contains(dir_name), "expected dir name in output:\n{text}");
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
        assert!(text.contains("inner_subdir"), "expected inner_subdir in output:\n{text}");
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
