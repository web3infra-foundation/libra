//! Handler for the apply_patch tool using Codex-style format.

use async_trait::async_trait;

use super::parse_arguments;
use crate::internal::ai::tools::{
    apply_patch::{self, ApplyPatchArgs},
    context::{ToolInvocation, ToolKind, ToolOutput},
    error::ToolError,
    registry::ToolHandler,
    spec::{FunctionParameters, ToolSpec},
    utils::validate_path,
};

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

        // Parse arguments (new format: only needs patch string)
        let args: ApplyPatchArgs = parse_arguments(&arguments)?;

        // Apply the patch (paths in patch content are relative to working_dir)
        let working_dir_clone = working_dir.clone();
        let result = tokio::task::spawn_blocking(move || {
            apply_patch::apply_patch(&args.input, &working_dir_clone)
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))??;

        // Validate all affected paths are within working directory
        // Note: We don't canonicalize paths because deleted files won't exist,
        // and symlink resolution can cause path mismatches on macOS.
        // The apply_patch function already constructs absolute paths from relative ones.
        for path in result
            .added
            .iter()
            .chain(result.modified.iter())
            .chain(result.deleted.iter())
        {
            validate_path(path, &working_dir)?;
        }

        // Format result
        let output = apply_patch::format_summary(&result);
        Ok(ToolOutput::success(output))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "apply_patch",
            r#"Use the `apply_patch` tool to edit files.
Your patch language is a stripped‑down, file‑oriented diff format designed to be easy to parse and safe to apply. You can think of it as a high‑level envelope:

*** Begin Patch
[ one or more file sections ]
*** End Patch

Within that envelope, you get a sequence of file operations.
You MUST include a header to specify the action you are taking.
Each operation starts with one of three headers:

*** Add File: <path> - create a new file. Every following line is a + line (the initial contents).
*** Delete File: <path> - remove an existing file. Nothing follows.
*** Update File: <path> - patch an existing file in place (optionally with a rename).

May be immediately followed by *** Move to: <new path> if you want to rename the file.
Then one or more "hunks", each introduced by @@ (optionally followed by a hunk header).
Within a hunk each line starts with:

For instructions on [context_before] and [context_after]:
- By default, show 3 lines of code immediately above and 3 lines immediately below each change. If a change is within 3 lines of a previous change, do NOT duplicate the first change's [context_after] lines in the second change's [context_before] lines.
- If 3 lines of context is insufficient to uniquely identify the snippet of code within the file, use the @@ operator to indicate the class or function to which the snippet belongs. For instance, we might have:
@@ class BaseClass
[3 lines of pre-context]
- [old_code]
+ [new_code]
[3 lines of post-context]

- If a code block is repeated so many times in a class or function such that even a single `@@` statement and 3 lines of context cannot uniquely identify the snippet of code, you can use multiple `@@` statements to jump to the right context. For instance:

@@ class BaseClass
@@ 	 def method():
[3 lines of pre-context]
- [old_code]
+ [new_code]
[3 lines of post-context]

The full grammar definition is below:
Patch := Begin { FileOp } End
Begin := "*** Begin Patch" NEWLINE
End := "*** End Patch" NEWLINE
FileOp := AddFile | DeleteFile | UpdateFile
AddFile := "*** Add File: " path NEWLINE { "+" line NEWLINE }
DeleteFile := "*** Delete File: " path NEWLINE
UpdateFile := "*** Update File: " path NEWLINE [ MoveTo ] { Hunk }
MoveTo := "*** Move to: " newPath NEWLINE
Hunk := "@@" [ header ] NEWLINE { HunkLine } [ "*** End of File" NEWLINE ]
HunkLine := (" " | "-" | "+") text NEWLINE

A full patch can combine several operations:

*** Begin Patch
*** Add File: hello.txt
+Hello world
*** Update File: src/app.py
*** Move to: src/main.py
@@ def greet():
-print("Hi")
+print("Hello, world!")
*** Delete File: obsolete.txt
*** End Patch

It is important to remember:

- You must include a header with your intended action (Add/Delete/Update)
- You must prefix new lines with `+` even when creating a new file
- File references can only be relative, NEVER ABSOLUTE.
"#,
        )
        .with_parameters(FunctionParameters::object(
            [("input", "string", "The entire contents of the apply_patch command")],
            [("input", true)],
        ))
    }
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
}
