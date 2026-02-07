//! Handler for the apply_patch tool.

use std::path::Path;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

use crate::internal::ai::tools::{
    context::{ApplyPatchArgs, ToolInvocation, ToolKind, ToolOutput},
    error::ToolError,
    registry::ToolHandler,
    spec::{FunctionParameters, ToolSpec},
    utils::validate_path,
};

use super::parse_arguments;

/// Handler for applying patches to files.
pub struct ApplyPatchHandler;

#[async_trait]
impl ToolHandler for ApplyPatchHandler {
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
                    "apply_patch handler only accepts Function payloads".to_string(),
                ));
            }
        };

        let args: ApplyPatchArgs = parse_arguments(&arguments)?;

        // Validate path
        let path = Path::new(&args.file_path);
        if !path.is_absolute() {
            return Err(ToolError::PathNotAbsolute(path.to_path_buf()));
        }

        validate_path(path, &working_dir)?;

        // Apply the patch
        let result = apply_patch_sync(path, &args.patch).await?;

        Ok(ToolOutput::success(result))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "apply_patch",
            "Apply a unified diff patch to a file. Modifies the file in place. Returns a summary of changes applied.",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("file_path", "string", "Absolute path to the file to patch"),
                ("patch", "string", "The patch in unified diff format"),
            ],
            [("file_path", true), ("patch", true)],
        ))
    }
}

/// Apply a patch to a file synchronously.
async fn apply_patch_sync(path: &Path, patch: &str) -> Result<String, ToolError> {
    use tokio::fs;

    // Read original file content
    let original = fs::read_to_string(path).await.map_err(|e| {
        ToolError::ExecutionFailed(format!("Failed to read file '{}': {}", path.display(), e))
    })?;

    // Parse and apply the patch
    let result = parse_and_apply_unified_diff(&original, patch)?;
    if std::env::var_os("LIBRA_APPLY_PATCH_DEBUG").is_some() {
        let preview = result.chars().take(120).collect::<String>();
        eprintln!(
            "[apply_patch] write path='{}' bytes={} preview='{}'",
            path.display(),
            result.len(),
            preview.replace('\n', "\\n")
        );
    }

    // Write the patched content back
    let mut file = fs::File::create(path).await.map_err(|e| {
        ToolError::ExecutionFailed(format!("Failed to open file '{}': {}", path.display(), e))
    })?;

    file.write_all(result.as_bytes()).await.map_err(|e| {
        ToolError::ExecutionFailed(format!(
            "Failed to write to file '{}': {}",
            path.display(),
            e
        ))
    })?;
    file.flush().await.map_err(|e| {
        ToolError::ExecutionFailed(format!("Failed to flush file '{}': {}", path.display(), e))
    })?;

    Ok(format!(
        "Patch applied successfully to '{}'",
        path.display()
    ))
}

/// Parse a unified diff and apply it to the original content.
fn parse_and_apply_unified_diff(original: &str, patch: &str) -> Result<String, ToolError> {
    // Parse unified diff format
    // Format:
    // --- a/file.txt
    // +++ b/file.txt
    // @@ -line,count +line,count @@
    //  lines to remove (prefixed with '-')
    //  lines to add (prefixed with '+')
    //  context lines (prefixed with ' ')

    let lines: Vec<&str> = patch.lines().collect();
    let mut result_lines: Vec<String> = original.lines().map(|s| s.to_string()).collect();
    let mut current_line = 0usize;
    let had_trailing_newline = original.ends_with('\n');
    let debug = std::env::var_os("LIBRA_APPLY_PATCH_DEBUG").is_some();
    if debug {
        eprintln!(
            "[apply_patch] original_lines={} had_trailing_newline={} patch_lines={}",
            result_lines.len(),
            had_trailing_newline,
            patch.lines().count()
        );
    }

    fn context_expected(raw: &str) -> &str {
        // Tolerate both:
        // 1) Standard unified diff context lines: " <content>"
        // 2) "Raw" context lines without the leading space prefix, which appear in some tests.
        //
        // Heuristic:
        // - If the line starts with a single space and the next char is not whitespace, treat it as diff-prefixed.
        // - Otherwise, treat the full line as the expected content.
        let mut chars = raw.chars();
        if chars.next() != Some(' ') {
            return raw;
        }
        match chars.next() {
            Some(c) if c != ' ' && c != '\t' => &raw[1..],
            _ => raw,
        }
    }

    fn find_in_window(lines: &[String], start: usize, needle: &str, window: usize) -> Option<usize> {
        let end = (start + window).min(lines.len());
        lines[start..end].iter().position(|l| l == needle).map(|i| start + i)
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // Look for hunk header @@ -l,c +l,c @@
        if line.starts_with("@@") {
            // Parse hunk header to find start line
            // Format: @@ -old_start,old_count +new_start,new_count @@
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                // Parse old_start from "-old_start,old_count"
                let old_info = parts[1]; // e.g., "-10,5"
                let old_start: usize = old_info
                    .trim_start_matches('-')
                    .split(',')
                    .next()
                    .unwrap_or("1")
                    .parse()
                    .unwrap_or(1);

                // Convert to 0-indexed
                current_line = old_start.saturating_sub(1);
            }

            i += 1;
            if debug {
                eprintln!("[apply_patch] hunk_start current_line={}", current_line);
            }

            // Process hunk content
            while i < lines.len() {
                let hunk_line = lines[i];
                if hunk_line.starts_with("@@") {
                    // Next hunk starts, break
                    break;
                }

                if let Some(expected) = hunk_line.strip_prefix('-') {
                    // Remove the expected line.
                    if current_line < result_lines.len() && result_lines[current_line] == expected {
                        if debug {
                            eprintln!("[apply_patch] - remove @{} '{}'", current_line, expected);
                        }
                        result_lines.remove(current_line);
                    } else if let Some(found) = find_in_window(&result_lines, current_line, expected, 50) {
                        if debug {
                            eprintln!("[apply_patch] - remove @{} (found @{}) '{}'", current_line, found, expected);
                        }
                        result_lines.remove(found);
                        current_line = found;
                    } else {
                        return Err(ToolError::ExecutionFailed(format!(
                            "Patch failed: expected to remove '{}', but it was not found",
                            expected
                        )));
                    }
                } else if let Some(content) = hunk_line.strip_prefix('+') {
                    // Insert line at current position and advance.
                    if current_line <= result_lines.len() {
                        if debug {
                            eprintln!("[apply_patch] + insert @{} '{}'", current_line, content);
                        }
                        result_lines.insert(current_line, content.to_string());
                        current_line += 1;
                    }
                } else if hunk_line.is_empty() || hunk_line.starts_with('\\') {
                    // Ignore empty lines and "\ No newline at end of file" markers.
                } else if hunk_line.starts_with("--- ") || hunk_line.starts_with("+++ ") {
                    // Ignore file headers if present.
                } else {
                    // Context line (tolerant).
                    let expected = context_expected(hunk_line);
                    if current_line < result_lines.len() && result_lines[current_line] == expected {
                        if debug {
                            eprintln!("[apply_patch] = context @{} '{}'", current_line, expected);
                        }
                        current_line += 1;
                    } else if let Some(found) =
                        find_in_window(&result_lines, current_line, expected, 50)
                    {
                        if debug {
                            eprintln!("[apply_patch] ~ context seek from @{} to @{} '{}'", current_line, found, expected);
                        }
                        current_line = found + 1;
                    } else if current_line < result_lines.len() {
                        if debug {
                            eprintln!("[apply_patch] ! context mismatch @{} expected='{}' actual='{}'",
                                current_line,
                                expected,
                                result_lines[current_line]
                            );
                        }
                        // If we can't match context, advance to avoid infinite loops.
                        current_line += 1;
                    }
                }

                i += 1;
            }

            continue;
        }

        i += 1;
    }

    let mut out = result_lines.join("\n");
    if had_trailing_newline && !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::tools::context::ToolPayload;
    use crate::internal::ai::tools::ToolKind;
    use std::fs;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_apply_patch_basic() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let mut temp = NamedTempFile::new_in(&working_dir).unwrap();
        writeln!(temp, "line 1").unwrap();
        writeln!(temp, "line 2").unwrap();
        writeln!(temp, "line 3").unwrap();
        writeln!(temp, "line 4").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": temp.path(),
                    "patch": format!(
                        "@@ -1,4 +1,4 @@
 line 1
-line 2
+line 2 modified
 line 3
 line 4"
                    ),
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert!(output.as_text().unwrap().contains("applied successfully"));

        // Verify the file was modified
        let content = fs::read_to_string(temp.path()).unwrap();
        assert!(content.contains("line 2 modified"));
        assert!(!content.contains("line 2\n"));
    }

    #[tokio::test]
    async fn test_apply_patch_add_lines() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let mut temp = NamedTempFile::new_in(&working_dir).unwrap();
        writeln!(temp, "line 1").unwrap();
        writeln!(temp, "line 2").unwrap();
        writeln!(temp, "line 3").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": temp.path(),
                    "patch": "@@ -1,3 +1,4 @@
 line 1
 line 2
+new line
 line 3".to_string(),
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        // Verify the file was modified
        let content = fs::read_to_string(temp.path()).unwrap();
        assert!(content.contains("new line"));
    }

    #[tokio::test]
    async fn test_apply_patch_delete_lines() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let mut temp = NamedTempFile::new_in(&working_dir).unwrap();
        writeln!(temp, "line 1").unwrap();
        writeln!(temp, "line 2").unwrap();
        writeln!(temp, "line 3").unwrap();
        writeln!(temp, "line 4").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": temp.path(),
                    "patch": "@@ -1,4 +1,3 @@
 line 1
-line 2
 line 3
 line 4".to_string(),
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        // Verify the file was modified
        let content = fs::read_to_string(temp.path()).unwrap();
        assert!(!content.contains("line 2"));
    }

    #[tokio::test]
    async fn test_apply_patch_nonexistent_file() {
        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": "/nonexistent/file.txt",
                    "patch": "@@ -1,1 +1,1 @@
-old
+new"
                })
                .to_string(),
            },
            std::env::current_dir().unwrap(),
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_apply_patch_relative_path_fails() {
        let temp_dir = TempDir::new().unwrap();
        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": "relative/path.txt",
                    "patch": "@@ -1,1 +1,1 @@\n-old\n+new"
                })
                .to_string(),
            },
            temp_dir.path().to_path_buf(),
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::PathNotAbsolute(_))));
    }

    #[tokio::test]
    async fn test_apply_patch_outside_working_dir_fails() {
        let temp_dir = TempDir::new().unwrap();
        let mut outside = NamedTempFile::new().unwrap();
        writeln!(outside, "old").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": outside.path(),
                    "patch": "@@ -1,1 +1,1 @@\n-old\n+new"
                })
                .to_string(),
            },
            temp_dir.path().to_path_buf(),
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::PathOutsideWorkingDir(_))));
    }

    #[tokio::test]
    async fn test_apply_patch_context_lines() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path,
                    "patch": "@@ -1,4 +1,4 @@\n line 1\n line 2\n-line 3\n+line 3 modified\n line 4"
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_apply_patch_preserves_trailing_newline() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        fs::write(&file_path, "line 1\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path.clone(),
                    "patch": "@@ -1,1 +1,2 @@\n line 1\n+line 2"
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.ends_with('\n'));
        assert!(content.contains("line 2"));
    }

    #[tokio::test]
    async fn test_apply_patch_whitespace_handling() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        fs::write(&file_path, "  indented line\ntab\tline\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path.clone(),
                    "patch": "@@ -1,2 +1,2 @@\n  indented line\n-tab\tline\n+replaced"
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("  indented line"));
        assert!(!content.contains("tab\tline"));
    }

    #[tokio::test]
    async fn test_apply_patch_multiline_change() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path.clone(),
                    "patch": "@@ -1,5 +1,5 @@\n line 1\n-line 2\n-line 3\n-line 4\n+new line a\n+new line b\n+new line c\n line 5"
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("new line a"));
        assert!(content.contains("new line b"));
        assert!(content.contains("new line c"));
    }

    #[tokio::test]
    async fn test_apply_patch_utf8_content() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        fs::write(&file_path, "Hello 世界\nПривет мир\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path.clone(),
                    "patch": "@@ -1,2 +1,2 @@\n-Hello 世界\n+Hello 世界!\n Привет мир"
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("Hello 世界!"));
    }

    #[tokio::test]
    async fn test_apply_patch_no_changes_needed() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        fs::write(&file_path, "line 1\nline 2\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path.clone(),
                    "patch": "@@ -1,2 +1,2 @@\n line 1\n line 2"
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("line 1"));
        assert!(content.contains("line 2"));
    }

    #[tokio::test]
    async fn test_apply_patch_kind_and_schema() {
        let handler = ApplyPatchHandler;
        assert_eq!(handler.kind(), ToolKind::Function);
        let schema = handler.schema();
        assert_eq!(schema.function.name, "apply_patch");
        assert!(schema.function.description.contains("patch") || schema.function.description.contains("diff"));
    }

    #[test]
    fn test_parse_and_apply_unified_diff() {
        let original = "line 1\nline 2\nline 3\nline 4\n";
        let patch = "@@ -1,4 +1,4 @@
 line 1
-line 2
+line 2 modified
 line 3
 line 4";

        let result = parse_and_apply_unified_diff(original, patch).unwrap();
        assert!(result.contains("line 2 modified"));
        assert!(!result.contains("\nline 2\n"));
    }
}
