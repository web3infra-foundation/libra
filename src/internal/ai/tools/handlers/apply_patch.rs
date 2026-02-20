//! Handler for the apply_patch tool using Codex-style format.

use async_trait::async_trait;

use crate::internal::ai::tools::{
    apply_patch::{self, ApplyPatchArgs},
    context::{ToolInvocation, ToolKind, ToolOutput, ToolPayload},
    error::ToolError,
    registry::ToolHandler,
    spec::ToolSpec,
    utils::validate_path,
};

/// Handler for applying patches to files.
pub struct ApplyPatchHandler;

#[async_trait]
impl ToolHandler for ApplyPatchHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(
            payload,
            ToolPayload::Function { .. } | ToolPayload::Custom { .. }
        )
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
        let ToolInvocation {
            payload,
            working_dir,
            ..
        } = invocation;

        // Accept both Function-style JSON arguments and Custom/freeform input to
        // stay compatible with Codex-style tool calls.
        let patch_text = match payload {
            ToolPayload::Function { arguments } => parse_patch_text_from_arguments(&arguments)?,
            ToolPayload::Custom { input } => input,
            _ => unreachable!("matches_kind limits payload types"),
        };

        // Parse the patch first, then validate all paths BEFORE any filesystem I/O.
        let parsed = apply_patch::parse_patch(&patch_text)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        for hunk in &parsed.hunks {
            for path in hunk.all_resolved_paths(&working_dir) {
                validate_path(&path, &working_dir)?;
            }
        }

        // All paths validated — safe to apply.
        let working_dir_for_task = working_dir.clone();
        let hunks = parsed.hunks.clone();
        let result = tokio::task::spawn_blocking(move || {
            apply_patch::apply_hunks(&hunks, &working_dir_for_task)
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))??;

        // Build unified diffs for TUI display (metadata — not sent to model).
        let diffs_json: Vec<serde_json::Value> = result
            .file_diffs
            .iter()
            .map(|fd| {
                let patch = diffy::create_patch(&fd.old_content, &fd.new_content);
                let kind = if fd.old_content.is_empty() {
                    "add"
                } else if fd.new_content.is_empty() {
                    "delete"
                } else {
                    "update"
                };
                serde_json::json!({
                    "path": fd.path.display().to_string(),
                    "diff": patch.to_string(),
                    "type": kind,
                })
            })
            .collect();

        let output = apply_patch::format_summary(&result.affected);
        let metadata = serde_json::json!({ "diffs": diffs_json });
        Ok(ToolOutput::success(output).with_metadata(metadata))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::apply_patch()
    }
}

fn parse_patch_text_from_arguments(arguments: &str) -> Result<String, ToolError> {
    // 1) Preferred: JSON object, supports aliases like `patch`.
    if let Ok(args) = serde_json::from_str::<ApplyPatchArgs>(arguments) {
        return Ok(args.input);
    }

    // 2) JSON string (Codex-style freeform patch encoded as JSON).
    if let Ok(s) = serde_json::from_str::<String>(arguments) {
        return Ok(s);
    }

    // 3) Raw text (non-JSON) – accept for compatibility.
    Ok(arguments.to_string())
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use tempfile::{NamedTempFile, TempDir};

    use super::*;
    use crate::internal::ai::tools::{ToolKind, context::ToolPayload};

    fn wrap_patch(body: &str) -> String {
        format!("*** Begin Patch\n{body}\n*** End Patch")
    }

    #[tokio::test]
    async fn test_apply_patch_add_file() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "input": wrap_patch(
                        r#"*** Add File: new_file.txt
+line 1
+line 2"#
                    )
                })
                .to_string(),
            },
            working_dir.clone(),
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert!(output.as_text().unwrap().contains("Success"));
        assert!(output.as_text().unwrap().contains("A"));

        // Verify the file was created
        let content = fs::read_to_string(working_dir.join("new_file.txt")).unwrap();
        assert_eq!(content, "line 1\nline 2\n");
    }

    #[tokio::test]
    async fn test_apply_patch_delete_file() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("to_delete.txt");
        fs::write(&file_path, "content").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "input": wrap_patch("*** Delete File: to_delete.txt")
                })
                .to_string(),
            },
            working_dir.clone(),
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        // Verify the file was deleted
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_apply_patch_update_file() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "input": wrap_patch(
                        r#"*** Update File: test.txt
@@
 line 1
-line 2
+line 2 modified
 line 3"#
                    )
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        // Verify the file was modified
        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("line 2 modified"));
        assert!(!content.contains("line 2\n"));
    }

    #[tokio::test]
    async fn test_apply_patch_move_file() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let src_path = working_dir.join("src.txt");
        fs::write(&src_path, "content\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "input": wrap_patch(
                        r#"*** Update File: src.txt
*** Move to: dst.txt
@@
-content
+modified content"#
                    )
                })
                .to_string(),
            },
            working_dir.clone(),
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        // Verify the file was moved
        assert!(!src_path.exists());
        let content = fs::read_to_string(working_dir.join("dst.txt")).unwrap();
        assert_eq!(content, "modified content\n");
    }

    #[tokio::test]
    async fn test_apply_patch_multiple_files() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file1 = working_dir.join("file1.txt");
        fs::write(&file1, "original\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "input": wrap_patch(
                        r#"*** Add File: new.txt
+new content
*** Update File: file1.txt
@@
-original
+modified"#
                    )
                })
                .to_string(),
            },
            working_dir.clone(),
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());

        // Verify both operations
        assert!(working_dir.join("new.txt").exists());
        let content = fs::read_to_string(&file1).unwrap();
        assert_eq!(content, "modified\n");
    }

    #[tokio::test]
    async fn test_apply_patch_unicode_content() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        fs::write(&file_path, "Hello 世界\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "input": wrap_patch(
                        r#"*** Update File: test.txt
@@
-Hello 世界
+Hello 世界!"#
                    )
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
    async fn test_apply_patch_outside_working_dir_fails() {
        let temp_dir = TempDir::new().unwrap();
        let outside_dir = TempDir::new().unwrap();
        let mut outside = NamedTempFile::new_in(&outside_dir).unwrap();
        writeln!(outside, "old").unwrap();

        // Try to delete a file outside working_dir
        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "input": wrap_patch(&format!(
                        "*** Delete File: {}",
                        outside.path().display()
                    ))
                })
                .to_string(),
            },
            temp_dir.path().to_path_buf(),
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::PathOutsideWorkingDir(_))));

        // Verify the file was NOT deleted (path validation prevented the operation)
        assert!(
            outside.path().exists(),
            "File outside working dir should NOT be deleted"
        );
    }

    #[tokio::test]
    async fn test_apply_patch_traversal_add_file_blocked() {
        let temp_dir = TempDir::new().unwrap();
        let outside_dir = TempDir::new().unwrap();
        let target = outside_dir.path().join("evil.txt");

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "input": wrap_patch(&format!(
                        "*** Add File: {}\n+malicious content",
                        target.display()
                    ))
                })
                .to_string(),
            },
            temp_dir.path().to_path_buf(),
        );

        let result = handler.handle(invocation).await;
        assert!(matches!(result, Err(ToolError::PathOutsideWorkingDir(_))));

        // Verify the file was NOT created
        assert!(
            !target.exists(),
            "File outside working dir should NOT be created"
        );
    }

    #[tokio::test]
    async fn test_apply_patch_relative_traversal_blocked() {
        let temp_dir = TempDir::new().unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "input": wrap_patch(
                        "*** Add File: ../../etc/evil.txt\n+malicious"
                    )
                })
                .to_string(),
            },
            temp_dir.path().to_path_buf(),
        );

        let result = handler.handle(invocation).await;
        assert!(
            result.is_err(),
            "Relative path traversal should be rejected"
        );
    }

    #[tokio::test]
    async fn test_apply_patch_kind_and_schema() {
        let handler = ApplyPatchHandler;
        assert_eq!(handler.kind(), ToolKind::Function);
        let schema = handler.schema();
        assert_eq!(schema.function.name, "apply_patch");
        assert!(
            schema.function.description.contains("Codex")
                || schema.function.description.contains("apply_patch")
        );
        // Verify only 'patch' parameter is required
        if let crate::internal::ai::tools::spec::FunctionParameters::Object { required, .. } =
            schema.function.parameters
        {
            assert!(required.contains(&"input".to_string()));
            assert!(!required.contains(&"file_path".to_string()));
        } else {
            panic!("Expected Object parameters");
        }
    }

    #[tokio::test]
    async fn test_apply_patch_accepts_patch_alias() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        fs::write(&file_path, "a\nb\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Function {
                arguments: serde_json::json!({
                    "patch": wrap_patch(
                        r#"*** Update File: test.txt
@@
 a
-b
+c"#
                    )
                })
                .to_string(),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "a\nc\n");
    }

    #[tokio::test]
    async fn test_apply_patch_accepts_custom_payload() {
        let temp_dir = TempDir::new().unwrap();
        let working_dir = temp_dir.path().to_path_buf();
        let file_path = working_dir.join("test.txt");
        fs::write(&file_path, "x\ny\n").unwrap();

        let handler = ApplyPatchHandler;
        let invocation = ToolInvocation::new(
            "call-1",
            "apply_patch",
            ToolPayload::Custom {
                input: wrap_patch(
                    r#"*** Update File: test.txt
@@
 x
-y
+z"#,
                ),
            },
            working_dir,
        );

        let result = handler.handle(invocation).await;
        assert!(result.is_ok());
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "x\nz\n");
    }
}
