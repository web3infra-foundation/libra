//! Handler for the read_file tool.

use std::path::Path;

use async_trait::async_trait;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
};

use super::parse_arguments;
use crate::internal::ai::tools::{
    context::{ReadFileArgs, ToolInvocation, ToolKind, ToolOutput, ToolPayload},
    error::ToolError,
    registry::ToolHandler,
    spec::{FunctionParameters, ToolSpec},
    utils::resolve_path,
};

/// Handler for reading file contents.
pub struct ReadFileHandler;

const MAX_LINE_LENGTH: usize = 500;

#[async_trait]
impl ToolHandler for ReadFileHandler {
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
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "read_file handler only accepts Function payloads".to_string(),
                ));
            }
        };

        let args: ReadFileArgs = parse_arguments(&arguments)?;

        // Validate offset and limit
        if args.offset == 0 {
            return Err(ToolError::InvalidArguments(
                "offset must be a 1-indexed line number (>= 1)".to_string(),
            ));
        }

        if args.limit == 0 {
            return Err(ToolError::InvalidArguments(
                "limit must be greater than zero".to_string(),
            ));
        }

        // Validate and resolve path
        let path = resolve_path(Path::new(&args.file_path), &working_dir)?;

        // Read the file
        let lines = read_file_slice(&path, args.offset, args.limit).await?;

        Ok(ToolOutput::success(lines.join("\n")))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "read_file",
            "Read the contents of a file. Returns the file content with each line prefixed as 'L{n}: content'. Blank lines appear as 'L{n}: ' (nothing after the space). Supports pagination with offset and limit parameters.",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("file_path", "string", "Path to the file to read, absolute or relative to the working directory"),
                ("offset", "integer", "1-indexed line number to start reading from (default: 1)"),
                ("limit", "integer", "Maximum number of lines to return (default: 2000)"),
            ],
            [("file_path", true)],
        ))
    }
}

/// Read a slice of lines from a file.
async fn read_file_slice(
    path: &Path,
    offset: usize,
    limit: usize,
) -> Result<Vec<String>, ToolError> {
    let file = File::open(path).await.map_err(|e| {
        ToolError::ExecutionFailed(format!("Failed to open file '{}': {}", path.display(), e))
    })?;

    let mut reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut line_number = 0;
    let mut buffer = Vec::new();

    loop {
        buffer.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut buffer)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read file: {}", e)))?;

        if bytes_read == 0 {
            break; // EOF
        }

        // Remove newline characters
        if buffer.last() == Some(&b'\n') {
            buffer.pop();
            if buffer.last() == Some(&b'\r') {
                buffer.pop();
            }
        }

        line_number += 1;

        // Skip lines before offset
        if line_number < offset {
            continue;
        }

        // Stop if we've reached the limit
        if lines.len() >= limit {
            lines.push(format!(
                "[truncated: more lines remain; continue with offset {line_number}]"
            ));
            break;
        }

        // Format the line
        let line_content = format_line(&buffer);
        lines.push(format!("L{line_number}: {line_content}"));
    }

    // Check if offset was beyond file length
    if line_number < offset {
        return Err(ToolError::ExecutionFailed(format!(
            "Offset {} exceeds file length ({} lines)",
            offset, line_number
        )));
    }

    Ok(lines)
}

/// Format a line from raw bytes, handling encoding and length limits.
fn format_line(bytes: &[u8]) -> String {
    let decoded = String::from_utf8_lossy(bytes);

    if decoded.len() > MAX_LINE_LENGTH {
        // Truncate at character boundary
        let end = decoded
            .char_indices()
            .nth(MAX_LINE_LENGTH)
            .map(|(i, _)| i)
            .unwrap_or(decoded.len());
        format!("{}...", &decoded[..end])
    } else {
        decoded.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::{NamedTempFile, TempDir};

    use super::*;
    use crate::internal::ai::tools::{ToolKind, context::ToolPayload};

    #[tokio::test]
    async fn test_read_file_basic() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let mut temp = NamedTempFile::new_in(&working_dir).unwrap();
        writeln!(temp, "line 1").unwrap();
        writeln!(temp, "line 2").unwrap();
        writeln!(temp, "line 3").unwrap();

        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": temp.path(),
                    "offset": 1,
                    "limit": 3
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("L1: line 1"));
        assert!(text.contains("L2: line 2"));
        assert!(text.contains("L3: line 3"));
    }

    #[tokio::test]
    async fn test_read_file_with_offset() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let mut temp = NamedTempFile::new_in(&working_dir).unwrap();
        writeln!(temp, "line 1").unwrap();
        writeln!(temp, "line 2").unwrap();
        writeln!(temp, "line 3").unwrap();

        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": temp.path(),
                    "offset": 2,
                    "limit": 2
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(!text.contains("L1:"));
        assert!(text.contains("L2: line 2"));
        assert!(text.contains("L3: line 3"));
    }

    #[tokio::test]
    async fn test_read_file_nonexistent() {
        let temp_dir = tempfile::tempdir().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": working_dir.join("nonexistent.txt"),
                    "offset": 1,
                    "limit": 100
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_file_zero_offset() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        tokio::fs::write(&file_path, "content").await.unwrap();

        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path,
                    "offset": 0,
                    "limit": 100
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[tokio::test]
    async fn test_read_file_path_validation() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let handler = ReadFileHandler;
        let file_path = working_dir.join("relative").join("path.txt");
        tokio::fs::create_dir_all(file_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&file_path, "hello").await.unwrap();

        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": "relative/path.txt",
                    "offset": 1,
                    "limit": 100
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await.unwrap();
        assert!(result.as_text().unwrap().contains("L1: hello"));
    }

    #[tokio::test]
    async fn test_read_file_zero_limit() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        tokio::fs::write(&file_path, "content").await.unwrap();

        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path,
                    "offset": 1,
                    "limit": 0
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[tokio::test]
    async fn test_read_file_with_limit() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let mut temp = NamedTempFile::new_in(&working_dir).unwrap();
        writeln!(temp, "line 1").unwrap();
        writeln!(temp, "line 2").unwrap();
        writeln!(temp, "line 3").unwrap();
        writeln!(temp, "line 4").unwrap();

        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": temp.path(),
                    "offset": 1,
                    "limit": 2
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(text.contains("L1: line 1"));
        assert!(text.contains("L2: line 2"));
        assert!(!text.contains("L3:"));
        assert!(text.contains("[truncated: more lines remain; continue with offset 3]"));
    }

    #[tokio::test]
    async fn test_read_file_outside_working_dir() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let other_dir = TempDir::new().unwrap();
        let other_path = other_dir.path().join("secret.txt");
        tokio::fs::write(&other_path, "secret").await.unwrap();

        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": other_path,
                    "offset": 1,
                    "limit": 10
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::PathOutsideWorkingDir(_))));
    }

    #[tokio::test]
    async fn test_read_file_utf8_content() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("utf8.txt");
        tokio::fs::write(&file_path, "Hello 世界\nПривет мир\n🎉 Emoji test\n")
            .await
            .unwrap();

        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path,
                    "offset": 1,
                    "limit": 10
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("世界"));
        assert!(text.contains("Привет"));
        assert!(text.contains("🎉"));
    }

    #[tokio::test]
    async fn test_read_file_empty_file_fails_on_offset() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("empty.txt");
        tokio::fs::write(&file_path, "").await.unwrap();

        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path,
                    "offset": 1,
                    "limit": 10
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed(_))));
    }

    #[tokio::test]
    async fn test_read_file_offset_beyond_length() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("short.txt");
        tokio::fs::write(&file_path, "line 1\nline 2\n")
            .await
            .unwrap();

        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path,
                    "offset": 10,
                    "limit": 1
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed(_))));
    }

    #[tokio::test]
    async fn test_read_file_default_parameters() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("default.txt");
        tokio::fs::write(&file_path, "line 1\nline 2\nline 3\n")
            .await
            .unwrap();

        let handler = ReadFileHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "read_file",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "file_path": file_path
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("L1: line 1"));
        assert!(text.contains("L3: line 3"));
    }

    #[tokio::test]
    async fn test_read_file_kind_and_schema() {
        let handler = ReadFileHandler;
        assert_eq!(handler.kind(), ToolKind::Function);

        let schema = handler.schema();
        assert_eq!(schema.function.name, "read_file");
        assert!(schema.function.description.contains("file"));
    }

    #[test]
    fn test_format_line_truncation() {
        let long_line = "x".repeat(MAX_LINE_LENGTH + 100);
        let bytes = long_line.as_bytes();
        let formatted = format_line(bytes);
        assert!(formatted.len() <= MAX_LINE_LENGTH + 4); // +4 for "..."
        assert!(formatted.ends_with("..."));
    }
}
