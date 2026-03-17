//! Handler for the grep_files tool.
//!
//! Finds files whose contents match the pattern and lists them sorted by
//! modification time. Mirrors the behaviour of the codex grep_files tool.

use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use regex::Regex;
use tokio::time::timeout;
use walkdir::WalkDir;

use super::parse_arguments;
use crate::internal::ai::tools::{
    context::{GrepFilesArgs, ToolInvocation, ToolKind, ToolOutput},
    error::ToolError,
    registry::ToolHandler,
    spec::{FunctionParameters, ToolSpec},
    utils::resolve_path,
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
            Some(p) => resolve_path(Path::new(p), &working_dir)?,
            None => working_dir.clone(),
        };

        let include = args
            .include
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let results = run_grep_search(pattern, include.as_deref(), &search_path, limit).await?;

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
                ("path", "string", "Directory or file path to search, absolute or relative to the working directory (defaults to the working directory)"),
                ("limit", "integer", "Maximum number of file paths to return (default: 100, max: 2000)"),
            ],
            [("pattern", true)],
        ))
    }
}

async fn run_grep_search(
    pattern: &str,
    include: Option<&str>,
    search_path: &Path,
    limit: usize,
) -> Result<Vec<String>, ToolError> {
    let re = Regex::new(pattern)
        .map_err(|e| ToolError::InvalidArguments(format!("invalid regex pattern: {e}")))?;

    let include_owned = include.map(str::to_string);

    let search = search_path.to_path_buf();
    timeout(
        GREP_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            grep_files_blocking(&re, include_owned.as_deref(), &search, limit)
        }),
    )
    .await
    .map_err(|_| ToolError::ExecutionFailed("grep timed out after 30 seconds".to_string()))?
    .map_err(|e| ToolError::ExecutionFailed(format!("grep task failed: {e}")))?
}

/// Walk the directory, find files matching the pattern, and return paths sorted
/// by modification time (most recently modified first).
/// Check if a file name matches a simple glob pattern (e.g. `*.rs`, `*.{ts,tsx}`).
fn matches_glob(pattern: &str, file_name: &str) -> bool {
    // Handle brace expansion like "*.{ts,tsx}"
    if let Some(start) = pattern.find('{')
        && let Some(end) = pattern.find('}')
    {
        let prefix = &pattern[..start];
        let suffix = &pattern[end + 1..];
        return pattern[start + 1..end].split(',').any(|alt| {
            let expanded = format!("{prefix}{alt}{suffix}");
            matches_glob(&expanded, file_name)
        });
    }

    // Simple wildcard matching: only support leading `*`
    if let Some(ext) = pattern.strip_prefix('*') {
        file_name.ends_with(ext)
    } else {
        file_name == pattern
    }
}

/// Walk the directory, find files matching the pattern, and return paths sorted
/// by modification time (most recently modified first).
fn grep_files_blocking(
    re: &Regex,
    glob_pattern: Option<&str>,
    search_path: &Path,
    limit: usize,
) -> Result<Vec<String>, ToolError> {
    let mut matched: Vec<(String, SystemTime)> = Vec::new();

    for entry in WalkDir::new(search_path).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if let Some(glob) = glob_pattern
            && !matches_glob(glob, file_name)
        {
            continue;
        }

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // skip binary / unreadable files
        };

        if re.is_match(&content) {
            let rel = path
                .strip_prefix(search_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            let mtime = fs::metadata(path)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            matched.push((rel, mtime));
        }
    }

    // Sort by modification time, most recent first.
    matched.sort_by(|a, b| b.1.cmp(&a.1));

    Ok(matched.into_iter().map(|(p, _)| p).take(limit).collect())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::internal::ai::tools::{ToolKind, context::ToolPayload};

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
        assert!(text.contains("match_one.txt") || text.contains("match_two.txt"));
        assert!(!text.contains("no_match.txt"));
    }

    #[tokio::test]
    async fn test_grep_files_path_defaults_to_working_dir() {
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
    async fn test_grep_files_relative_path_resolves_inside_working_dir() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src").join("f.txt"), "needle").unwrap();

        let result = GrepFilesHandler
            .handle(make_invocation(
                serde_json::json!({ "pattern": "needle", "path": "src" }),
                temp.path().to_path_buf(),
            ))
            .await
            .unwrap();

        let text = result.as_text().unwrap();
        assert!(text.contains("f.txt"));
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
}
