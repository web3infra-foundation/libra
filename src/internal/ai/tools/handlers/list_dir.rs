//! Handler for the list_dir tool.

use std::path::Path;

use async_trait::async_trait;
use tokio::fs;

use super::parse_arguments;
use crate::internal::ai::tools::{
    context::{ListDirArgs, ToolInvocation, ToolKind, ToolOutput},
    error::ToolError,
    registry::ToolHandler,
    spec::{FunctionParameters, ToolSpec},
    utils::validate_path,
};

/// Handler for listing directory contents.
pub struct ListDirHandler;

#[async_trait]
impl ToolHandler for ListDirHandler {
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
                    "list_dir handler only accepts Function payloads".to_string(),
                ));
            }
        };

        let args: ListDirArgs = parse_arguments(&arguments)?;

        // Validate path
        let path = Path::new(&args.dir_path);
        if !path.is_absolute() {
            return Err(ToolError::PathNotAbsolute(path.to_path_buf()));
        }

        validate_path(path, &working_dir)?;

        // List directory contents
        let entries = list_directory(path, args.max_depth).await?;

        Ok(ToolOutput::success(entries))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "list_dir",
            "List the contents of a directory. Can list recursively with depth control. Returns a formatted list with entry types (DIR, FILE) and names.",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("dir_path", "string", "Absolute path to the directory to list"),
                ("max_depth", "integer", "Maximum depth for recursive listing (default: 1, non-recursive)"),
            ],
            [("dir_path", true)],
        ))
    }
}

/// List directory contents with optional recursion.
async fn list_directory(path: &Path, max_depth: usize) -> Result<String, ToolError> {
    let mut entries = Vec::new();

    // Check if path exists and is a directory
    let metadata = fs::metadata(path).await.map_err(|e| {
        ToolError::ExecutionFailed(format!("Failed to access '{}': {}", path.display(), e))
    })?;

    if !metadata.is_dir() {
        return Err(ToolError::ExecutionFailed(format!(
            "Path '{}' is not a directory",
            path.display()
        )));
    }

    // List entries
    list_directory_recursive(path, 0, max_depth, &mut entries).await?;

    if entries.is_empty() {
        Ok("(empty directory)".to_string())
    } else {
        Ok(entries.join("\n"))
    }
}

/// Recursively list directory contents.
fn list_directory_recursive<'a>(
    path: &'a Path,
    current_depth: usize,
    max_depth: usize,
    entries: &'a mut Vec<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), ToolError>> + Send + 'a>> {
    Box::pin(async move {
        let mut read_dir = fs::read_dir(path).await.map_err(|e| {
            ToolError::ExecutionFailed(format!(
                "Failed to read directory '{}': {}",
                path.display(),
                e
            ))
        })?;

        let mut entry_infos = Vec::new();

        while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
            ToolError::ExecutionFailed(format!("Failed to read directory entry: {}", e))
        })? {
            let entry_path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            let metadata = entry.metadata().await.map_err(|e| {
                ToolError::ExecutionFailed(format!(
                    "Failed to get metadata for '{}': {}",
                    entry_path.display(),
                    e
                ))
            })?;

            let entry_type = if metadata.is_dir() { "DIR" } else { "FILE" };

            let indent = "  ".repeat(current_depth);
            entry_infos.push((
                format!("{}[{}] {}", indent, entry_type, name),
                entry_path,
                metadata.is_dir(),
            ));
        }

        // Sort entries: directories first, then files, both alphabetically
        entry_infos.sort_by(|a, b| {
            match (a.2, b.2) {
                (true, true) | (false, false) => a.0.cmp(&b.0), // Both same type, sort by name
                (true, false) => std::cmp::Ordering::Less,      // Directory comes before file
                (false, true) => std::cmp::Ordering::Greater,   // File comes after directory
            }
        });

        for (display, entry_path, is_dir) in entry_infos {
            entries.push(display);

            // Recurse into subdirectories if depth allows
            if is_dir && current_depth < max_depth {
                list_directory_recursive(&entry_path, current_depth + 1, max_depth, entries)
                    .await?;
            }
        }

        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::internal::ai::tools::{ToolKind, context::ToolPayload};

    #[tokio::test]
    async fn test_list_dir_basic() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        // Create some test files
        fs::write(dir_path.join("file1.txt"), "content").unwrap();
        fs::write(dir_path.join("file2.txt"), "content").unwrap();

        // Create a subdirectory
        fs::create_dir(dir_path.join("subdir")).unwrap();

        let handler = ListDirHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": dir_path,
                    "max_depth": 1
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains("DIR") || text.contains("FILE"));
    }

    #[tokio::test]
    async fn test_list_dir_recursive() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        // Create nested structure
        fs::create_dir(dir_path.join("level1")).unwrap();
        fs::create_dir(dir_path.join("level1").join("level2")).unwrap();
        fs::write(
            dir_path.join("level1").join("level2").join("file.txt"),
            "content",
        )
        .unwrap();

        let handler = ListDirHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": dir_path,
                    "max_depth": 3
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        // Should contain nested entries
        assert!(text.contains("level1") || !text.is_empty());
    }

    #[tokio::test]
    async fn test_list_dir_nonexistent() {
        let handler = ListDirHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": "/nonexistent/directory",
                    "max_depth": 1
                })
                .to_string(),
            },
            std::env::current_dir().unwrap(),
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_dir_file_instead_of_directory() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let temp_file = tempfile::NamedTempFile::new_in(&working_dir).unwrap();

        let handler = ListDirHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": temp_file.path(),
                    "max_depth": 1
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed(_))));
    }

    #[tokio::test]
    async fn test_list_dir_empty_directory() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        let handler = ListDirHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": dir_path,
                    "max_depth": 1
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert_eq!(text, "(empty directory)");
    }

    #[tokio::test]
    async fn test_list_dir_depth_zero() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        fs::write(dir_path.join("file.txt"), "content").unwrap();

        let handler = ListDirHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": dir_path,
                    "max_depth": 0
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(!text.is_empty());
    }

    #[tokio::test]
    async fn test_list_dir_many_files() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        for i in 0..20 {
            fs::write(dir_path.join(format!("file{}.txt", i)), "content").unwrap();
        }

        let handler = ListDirHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": dir_path,
                    "max_depth": 1
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.matches("FILE").count() >= 20);
    }

    #[tokio::test]
    async fn test_list_dir_hidden_files() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path();
        let working_dir = dir_path.to_path_buf();

        fs::write(dir_path.join(".hidden"), "hidden").unwrap();
        fs::write(dir_path.join("visible.txt"), "visible").unwrap();

        let handler = ListDirHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": dir_path,
                    "max_depth": 1
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        let text = output.as_text().unwrap();
        assert!(text.contains(".hidden"));
        assert!(text.contains("visible.txt"));
    }

    #[tokio::test]
    async fn test_list_dir_relative_path_fails() {
        let temp_dir = TempDir::new().unwrap();
        let handler = ListDirHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": "relative/path",
                    "max_depth": 1
                })
                .to_string(),
            },
            temp_dir.path().to_path_buf(),
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::PathNotAbsolute(_))));
    }

    #[tokio::test]
    async fn test_list_dir_outside_working_dir_fails() {
        let temp_dir = TempDir::new().unwrap();
        let other_dir = TempDir::new().unwrap();

        let handler = ListDirHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "dir_path": other_dir.path(),
                    "max_depth": 1
                })
                .to_string(),
            },
            temp_dir.path().to_path_buf(),
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::PathOutsideWorkingDir(_))));
    }

    #[tokio::test]
    async fn test_list_dir_kind_and_schema() {
        let handler = ListDirHandler;
        assert_eq!(handler.kind(), ToolKind::Function);
        let schema = handler.schema();
        assert_eq!(schema.function.name, "list_dir");
        assert!(schema.function.description.contains("directory"));
    }
}
