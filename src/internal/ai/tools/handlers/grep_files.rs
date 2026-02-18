//! Handler for the grep_files tool.
//!
//! Finds files whose contents match the pattern and lists them sorted by
//! modification time. Mirrors the behaviour of the codex grep_files tool.

use std::{path::Path, time::Duration};

use async_trait::async_trait;
use tokio::{process::Command, time::timeout};

use super::parse_arguments;
use crate::internal::ai::tools::{
    context::{GrepFilesArgs, ToolInvocation, ToolKind, ToolOutput},
    error::ToolError,
    registry::ToolHandler,
    spec::{FunctionParameters, ToolSpec},
    utils::validate_path,
};

pub struct GrepFilesHandler;

const MAX_LIMIT: usize = 2000;
const GREP_TIMEOUT: Duration = Duration::from_secs(30);

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

        let pattern = args.pattern.trim();
        if pattern.is_empty() {
            return Err(ToolError::InvalidArguments(
                "pattern must not be empty".to_string(),
            ));
        }

        if args.limit == 0 {
            return Err(ToolError::InvalidArguments(
                "limit must be greater than zero".to_string(),
            ));
        }

        let limit = args.limit.min(MAX_LIMIT);

        // Resolve path: use provided path or fall back to working_dir.
        let search_path = match &args.path {
            Some(p) => {
                let path = Path::new(p);
                if !path.is_absolute() {
                    return Err(ToolError::PathNotAbsolute(path.to_path_buf()));
                }
                validate_path(path, &working_dir)?;
                path.to_path_buf()
            }
            None => working_dir.clone(),
        };

        let include = args
            .include
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let results = run_rg_search(pattern, include.as_deref(), &search_path, limit).await?;

        if results.is_empty() {
            Ok(ToolOutput::success("No matches found.".to_string()))
        } else {
            Ok(ToolOutput::success(results.join("\n")))
        }
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "grep_files",
            "Finds files whose contents match the pattern and lists them sorted by modification time.",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("pattern", "string", "Regular expression pattern to search for"),
                ("include", "string", "Optional glob limiting which files are searched (e.g. \"*.rs\" or \"*.{ts,tsx}\")"),
                ("path", "string", "Directory or file path to search (defaults to the working directory)"),
                ("limit", "integer", "Maximum number of file paths to return (default: 100, max: 2000)"),
            ],
            [("pattern", true)],
        ))
    }
}

async fn run_rg_search(
    pattern: &str,
    include: Option<&str>,
    search_path: &Path,
    limit: usize,
) -> Result<Vec<String>, ToolError> {
    let mut cmd = Command::new("rg");
    cmd.arg("--files-with-matches")
        .arg("--sortr=modified")
        .arg("--regexp")
        .arg(pattern)
        .arg("--no-messages");

    if let Some(glob) = include {
        cmd.arg("--glob").arg(glob);
    }

    cmd.arg("--").arg(search_path);

    let output = timeout(GREP_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            ToolError::ExecutionFailed("rg timed out after 30 seconds".to_string())
        })?
        .map_err(|e| {
            ToolError::ExecutionFailed(format!(
                "failed to launch rg: {e}. Ensure ripgrep is installed and on PATH."
            ))
        })?;

    match output.status.code() {
        Some(0) => Ok(parse_results(&output.stdout, limit)),
        Some(1) => Ok(Vec::new()), // rg exit 1 = no matches
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ToolError::ExecutionFailed(format!("rg failed: {stderr}")))
        }
    }
}

fn parse_results(stdout: &[u8], limit: usize) -> Vec<String> {
    let mut results = Vec::new();
    for line in stdout.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(line) {
            if !text.is_empty() {
                results.push(text.to_string());
                if results.len() == limit {
                    break;
                }
            }
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::internal::ai::tools::{ToolKind, context::ToolPayload};

    fn rg_available() -> bool {
        std::process::Command::new("rg")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn make_invocation(args: serde_json::Value, working_dir: std::path::PathBuf) -> ToolInvocation {
        ToolInvocation::new(
            "call-1",
            "grep_files",
            ToolPayload::Function {
                arguments: args.to_string(),
            },
            working_dir,
        )
    }

    #[tokio::test]
    async fn test_grep_files_returns_file_paths() {
        if !rg_available() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let dir = temp.path().to_path_buf();

        fs::write(dir.join("match_one.txt"), "hello world").unwrap();
        fs::write(dir.join("match_two.txt"), "hello rust").unwrap();
        fs::write(dir.join("no_match.txt"), "goodbye world").unwrap();

        let result = GrepFilesHandler
            .handle(make_invocation(
                serde_json::json!({ "pattern": "hello" }),
                dir.clone(),
            ))
            .await
            .unwrap();

        let text = result.as_text().unwrap();
        // Should return file paths, not content lines.
        assert!(!text.contains(":1:"), "should not contain line:col format");
        assert!(text.contains("match_one.txt") || text.contains("match_two.txt"));
        assert!(!text.contains("no_match.txt"));
    }

    #[tokio::test]
    async fn test_grep_files_path_defaults_to_working_dir() {
        if !rg_available() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let dir = temp.path().to_path_buf();
        fs::write(dir.join("found.txt"), "needle").unwrap();

        // No `path` argument — should default to working_dir.
        let result = GrepFilesHandler
            .handle(make_invocation(
                serde_json::json!({ "pattern": "needle" }),
                dir.clone(),
            ))
            .await
            .unwrap();

        let text = result.as_text().unwrap();
        assert!(text.contains("found.txt"));
    }

    #[tokio::test]
    async fn test_grep_files_include_glob_filters_files() {
        if !rg_available() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let dir = temp.path().to_path_buf();
        fs::write(dir.join("main.rs"), "hello rust").unwrap();
        fs::write(dir.join("main.txt"), "hello text").unwrap();

        let result = GrepFilesHandler
            .handle(make_invocation(
                serde_json::json!({ "pattern": "hello", "include": "*.rs" }),
                dir.clone(),
            ))
            .await
            .unwrap();

        let text = result.as_text().unwrap();
        assert!(text.contains("main.rs"));
        assert!(!text.contains("main.txt"));
    }

    #[tokio::test]
    async fn test_grep_files_limit_respected() {
        if !rg_available() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let dir = temp.path().to_path_buf();
        for i in 0..5 {
            fs::write(dir.join(format!("file{i}.txt")), "match").unwrap();
        }

        let result = GrepFilesHandler
            .handle(make_invocation(
                serde_json::json!({ "pattern": "match", "limit": 2 }),
                dir.clone(),
            ))
            .await
            .unwrap();

        let text = result.as_text().unwrap();
        assert_eq!(text.lines().count(), 2);
    }

    #[tokio::test]
    async fn test_grep_files_no_matches() {
        if !rg_available() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let dir = temp.path().to_path_buf();
        fs::write(dir.join("file.txt"), "hello").unwrap();

        let result = GrepFilesHandler
            .handle(make_invocation(
                serde_json::json!({ "pattern": "zzznomatch" }),
                dir.clone(),
            ))
            .await
            .unwrap();

        assert_eq!(result.as_text().unwrap(), "No matches found.");
    }

    #[tokio::test]
    async fn test_grep_files_path_outside_working_dir_fails() {
        let temp = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        fs::write(other.path().join("f.txt"), "x").unwrap();

        let result = GrepFilesHandler
            .handle(make_invocation(
                serde_json::json!({ "pattern": "x", "path": other.path() }),
                temp.path().to_path_buf(),
            ))
            .await;

        assert!(matches!(result, Err(ToolError::PathOutsideWorkingDir(_))));
    }

    #[tokio::test]
    async fn test_grep_files_empty_pattern_fails() {
        let temp = TempDir::new().unwrap();
        let result = GrepFilesHandler
            .handle(make_invocation(
                serde_json::json!({ "pattern": "  " }),
                temp.path().to_path_buf(),
            ))
            .await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[tokio::test]
    async fn test_grep_files_kind_and_schema() {
        assert_eq!(GrepFilesHandler.kind(), ToolKind::Function);
        let schema = GrepFilesHandler.schema();
        assert_eq!(schema.function.name, "grep_files");
        // pattern is required, path is optional
        let json = schema.to_json();
        let required = json["function"]["parameters"]["required"]
            .as_array()
            .unwrap();
        assert!(required.iter().any(|v| v == "pattern"));
        assert!(!required.iter().any(|v| v == "path"));
    }

    #[test]
    fn test_parse_results_basic() {
        let stdout = b"/tmp/a.rs\n/tmp/b.rs\n";
        assert_eq!(
            parse_results(stdout, 10),
            vec!["/tmp/a.rs".to_string(), "/tmp/b.rs".to_string()]
        );
    }

    #[test]
    fn test_parse_results_truncates_at_limit() {
        let stdout = b"/a\n/b\n/c\n";
        assert_eq!(parse_results(stdout, 2), vec!["/a".to_string(), "/b".to_string()]);
    }
}
