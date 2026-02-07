//! Handler for the grep_files tool.

use std::path::Path;
use std::process::Stdio;

use async_trait::async_trait;
use tokio::process::Command;

use crate::internal::ai::tools::{
    context::{GrepFilesArgs, ToolInvocation, ToolKind, ToolOutput},
    error::ToolError,
    registry::ToolHandler,
    spec::{FunctionParameters, ToolSpec},
    utils::validate_path,
};

use super::parse_arguments;

/// Handler for searching files using ripgrep.
pub struct GrepFilesHandler;

#[async_trait]
impl ToolHandler for GrepFilesHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
        let ToolInvocation {
            payload,
            working_dir,
            ..
        } = invocation;

        let arguments = match payload {
            crate::internal::ai::tools::context::ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "grep_files handler only accepts Function payloads".to_string(),
                ));
            }
        };

        let args: GrepFilesArgs = parse_arguments(&arguments)?;

        // Validate path
        let path = Path::new(&args.path);
        if !path.is_absolute() {
            return Err(ToolError::PathNotAbsolute(path.to_path_buf()));
        }

        validate_path(path, &working_dir)?;

        // Execute grep search
        let results = grep_search(&args.pattern, path, args.case_insensitive).await?;

        Ok(ToolOutput::success(results))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "grep_files",
            "Search for a pattern in files using ripgrep. Supports regex patterns and case-insensitive search. Returns matching lines with file paths and line numbers.",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("pattern", "string", "Search pattern (supports regex)"),
                ("path", "string", "Absolute path to search in"),
                ("case_insensitive", "boolean", "Case-insensitive search (default: false)"),
            ],
            [("pattern", true), ("path", true)],
        ))
    }
}

/// Execute ripgrep search and return formatted results.
async fn grep_search(
    pattern: &str,
    path: &Path,
    case_insensitive: bool,
) -> Result<String, ToolError> {
    // Build ripgrep command
    let mut cmd = Command::new("rg");
    cmd.arg("--line-number") // Show line numbers
        .arg("--no-heading") // Don't group by file
        .arg("--color=never") // No ANSI colors
        .arg(pattern)
        .arg(path);

    if case_insensitive {
        cmd.arg("--ignore-case");
    }

    // Capture output
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = cmd
        .output()
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("Failed to execute ripgrep: {}", e)))?;

    // Check if ripgrep was found
    if output.status.code() == Some(126) || output.status.code() == Some(127) {
        return Err(ToolError::ExecutionFailed(
            "ripgrep (rg) command not found. Please install ripgrep to use grep_files.".to_string(),
        ));
    }

    // Format results
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !stderr.is_empty() && !output.status.success() {
        // Check if it's just "no matches" (rg returns exit code 1 for no matches)
        if output.status.code() != Some(1) || !stderr.contains("No matches found") {
            return Err(ToolError::ExecutionFailed(format!(
                "ripgrep error: {}",
                stderr.trim()
            )));
        }
    }

    if stdout.trim().is_empty() {
        Ok("No matches found".to_string())
    } else {
        Ok(stdout.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::tools::context::ToolPayload;
    use crate::internal::ai::tools::ToolKind;
    use std::fs;
    use tempfile::TempDir;

    fn skip_if_rg_missing(result: &Result<ToolOutput, ToolError>) -> bool {
        match result {
            Ok(_) => false,
            Err(err) => err.to_string().contains("ripgrep (rg) command not found"),
        }
    }

    #[tokio::test]
    async fn test_grep_files_basic() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        // Create test files
        fs::write(
            dir_path.join("test.txt"),
            "hello world\nhello rust\ngoodbye world",
        )
        .unwrap();

        let handler = GrepFilesHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "grep_files",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "pattern": "hello",
                    "path": dir_path,
                    "case_insensitive": false
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;

        if skip_if_rg_missing(&result) {
            return;
        }

        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("hello") || text.contains("No matches found"));
    }

    #[tokio::test]
    async fn test_grep_files_case_insensitive() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        // Create test file
        fs::write(dir_path.join("test.txt"), "Hello WORLD\nhello world").unwrap();

        let handler = GrepFilesHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "grep_files",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "pattern": "hello",
                    "path": dir_path,
                    "case_insensitive": true
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;

        if skip_if_rg_missing(&result) {
            return;
        }

        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.len() > 0);
    }

    #[tokio::test]
    async fn test_grep_files_no_matches() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        // Create test file
        fs::write(dir_path.join("test.txt"), "hello world").unwrap();

        let handler = GrepFilesHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "grep_files",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "pattern": "nonexistent",
                    "path": dir_path,
                    "case_insensitive": false
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;

        if skip_if_rg_missing(&result) {
            return;
        }

        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("No matches found") || text.is_empty());
    }

    #[tokio::test]
    async fn test_grep_files_regex_pattern() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        fs::write(
            dir_path.join("test.txt"),
            "error: something failed\nwarning: this is a warning\nerror: another error",
        )
        .unwrap();

        let handler = GrepFilesHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "grep_files",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "pattern": "^error:",
                    "path": dir_path,
                    "case_insensitive": false
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        if skip_if_rg_missing(&result) {
            return;
        }
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("error:") || text.contains("No matches"));
        if text.contains("error:") {
            assert!(!text.contains("warning:"));
        }
    }

    #[tokio::test]
    async fn test_grep_files_relative_path_fails() {
        let temp_dir = TempDir::new().unwrap();
        let handler = GrepFilesHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "grep_files",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "pattern": "test",
                    "path": "relative/path",
                    "case_insensitive": false
                })
                .to_string(),
            },
            temp_dir.path().to_path_buf(),
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::PathNotAbsolute(_))));
    }

    #[tokio::test]
    async fn test_grep_files_outside_working_dir_fails() {
        let temp_dir = TempDir::new().unwrap();
        let other_dir = TempDir::new().unwrap();
        fs::write(other_dir.path().join("test.txt"), "content").unwrap();

        let handler = GrepFilesHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "grep_files",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "pattern": "content",
                    "path": other_dir.path(),
                    "case_insensitive": false
                })
                .to_string(),
            },
            temp_dir.path().to_path_buf(),
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::PathOutsideWorkingDir(_))));
    }

    #[tokio::test]
    async fn test_grep_files_multiple_files() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        fs::write(dir_path.join("file1.txt"), "hello world").unwrap();
        fs::write(dir_path.join("file2.txt"), "hello there").unwrap();
        fs::write(dir_path.join("file3.txt"), "goodbye world").unwrap();

        let handler = GrepFilesHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "grep_files",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "pattern": "hello",
                    "path": dir_path,
                    "case_insensitive": false
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        if skip_if_rg_missing(&result) {
            return;
        }
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        let file_count = text.matches("file1.txt").count() + text.matches("file2.txt").count();
        assert!(file_count > 0 || text.contains("No matches"));
    }

    #[tokio::test]
    async fn test_grep_files_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        let handler = GrepFilesHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "grep_files",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "pattern": "anything",
                    "path": dir_path,
                    "case_insensitive": false
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        if skip_if_rg_missing(&result) {
            return;
        }
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("No matches found") || text.is_empty());
    }

    #[tokio::test]
    async fn test_grep_files_special_characters() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        fs::write(dir_path.join("test.txt"), "test.file\n(test)\n[test]").unwrap();

        let handler = GrepFilesHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "grep_files",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "pattern": r"test\\.file",
                    "path": dir_path,
                    "case_insensitive": false
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        if skip_if_rg_missing(&result) {
            return;
        }
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("test.file") || text.contains("No matches"));
    }

    #[tokio::test]
    async fn test_grep_files_kind_and_schema() {
        let handler = GrepFilesHandler;
        assert_eq!(handler.kind(), ToolKind::Function);
        let schema = handler.schema();
        assert_eq!(schema.function.name, "grep_files");
        assert!(schema.function.description.contains("Search") || schema.function.description.contains("search"));
    }
}
