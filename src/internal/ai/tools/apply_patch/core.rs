//! Core patch application logic.
//!
//! This module provides the main `apply_patch` function that applies parsed hunks
//! to the filesystem.

use std::path::{Path, PathBuf};

use thiserror::Error;

use super::parser::{Hunk, ParseError, UpdateFileChunk, parse_patch};

#[derive(Debug, Error, PartialEq)]
pub enum ApplyPatchError {
    #[error(transparent)]
    ParseError(#[from] ParseError),
    #[error(transparent)]
    IoError(#[from] IoError),
    /// Error that occurs while computing replacements when applying patch chunks
    #[error("{0}")]
    ComputeReplacements(String),
}

impl From<std::io::Error> for ApplyPatchError {
    fn from(err: std::io::Error) -> Self {
        ApplyPatchError::IoError(IoError {
            context: "I/O error".to_string(),
            source: err,
        })
    }
}

impl From<&std::io::Error> for ApplyPatchError {
    fn from(err: &std::io::Error) -> Self {
        ApplyPatchError::IoError(IoError {
            context: "I/O error".to_string(),
            source: std::io::Error::new(err.kind(), err.to_string()),
        })
    }
}

#[derive(Debug, Error)]
#[error("{context}: {source}")]
pub struct IoError {
    context: String,
    #[source]
    source: std::io::Error,
}

impl PartialEq for IoError {
    fn eq(&self, other: &Self) -> bool {
        self.context == other.context && self.source.to_string() == other.source.to_string()
    }
}

/// Tracks file paths affected by applying a patch.
#[derive(Debug, PartialEq)]
pub struct AffectedPaths {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

/// Applies the patch to files in the given working directory.
///
/// The patch uses the Codex-style format:
/// - `*** Begin Patch` / `*** End Patch` markers
/// - `*** Add File:`, `*** Delete File:`, `*** Update File:` operations
/// - Relative paths (resolved against `cwd`)
///
/// # Arguments
///
/// * `patch` - The patch string in Codex format
/// * `cwd` - The working directory to resolve relative paths against
///
/// # Returns
///
/// Returns `Ok(AffectedPaths)` with lists of added, modified, and deleted files,
/// or an error if the patch could not be parsed or applied.
pub fn apply_patch(patch: &str, cwd: &Path) -> Result<AffectedPaths, ApplyPatchError> {
    let args = parse_patch(patch)?;
    apply_hunks(&args.hunks, cwd)
}

/// Applies hunks to the filesystem.
fn apply_hunks(hunks: &[Hunk], cwd: &Path) -> Result<AffectedPaths, ApplyPatchError> {
    if hunks.is_empty() {
        return Err(ApplyPatchError::ComputeReplacements(
            "No files were modified.".to_string(),
        ));
    }

    let mut added: Vec<PathBuf> = Vec::new();
    let mut modified: Vec<PathBuf> = Vec::new();
    let mut deleted: Vec<PathBuf> = Vec::new();

    for hunk in hunks {
        match hunk {
            Hunk::AddFile { path, contents } => {
                let abs_path = cwd.join(path);
                if let Some(parent) = abs_path.parent()
                    && !parent.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        ApplyPatchError::IoError(IoError {
                            context: format!(
                                "Failed to create parent directories for {}",
                                abs_path.display()
                            ),
                            source: e,
                        })
                    })?;
                }
                std::fs::write(&abs_path, contents).map_err(|e| {
                    ApplyPatchError::IoError(IoError {
                        context: format!("Failed to write file {}", abs_path.display()),
                        source: e,
                    })
                })?;
                added.push(abs_path);
            }
            Hunk::DeleteFile { path } => {
                let abs_path = cwd.join(path);
                std::fs::remove_file(&abs_path).map_err(|e| {
                    ApplyPatchError::IoError(IoError {
                        context: format!("Failed to delete file {}", abs_path.display()),
                        source: e,
                    })
                })?;
                deleted.push(abs_path);
            }
            Hunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                let abs_path = cwd.join(path);
                let AppliedPatch { new_contents, .. } =
                    derive_new_contents_from_chunks(&abs_path, chunks)?;

                if let Some(dest) = move_path {
                    let dest_abs = cwd.join(dest);
                    if let Some(parent) = dest_abs.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            ApplyPatchError::IoError(IoError {
                                context: format!(
                                    "Failed to create parent directories for {}",
                                    dest_abs.display()
                                ),
                                source: e,
                            })
                        })?;
                    }
                    std::fs::write(&dest_abs, new_contents).map_err(|e| {
                        ApplyPatchError::IoError(IoError {
                            context: format!("Failed to write file {}", dest_abs.display()),
                            source: e,
                        })
                    })?;
                    std::fs::remove_file(&abs_path).map_err(|e| {
                        ApplyPatchError::IoError(IoError {
                            context: format!("Failed to remove original {}", abs_path.display()),
                            source: e,
                        })
                    })?;
                    modified.push(dest_abs);
                } else {
                    std::fs::write(&abs_path, new_contents).map_err(|e| {
                        ApplyPatchError::IoError(IoError {
                            context: format!("Failed to write file {}", abs_path.display()),
                            source: e,
                        })
                    })?;
                    modified.push(abs_path);
                }
            }
        }
    }

    Ok(AffectedPaths {
        added,
        modified,
        deleted,
    })
}

struct AppliedPatch {
    #[allow(dead_code)]
    original_contents: String,
    new_contents: String,
}

/// Return the new file contents after applying the chunks to the file at `path`.
fn derive_new_contents_from_chunks(
    path: &Path,
    chunks: &[UpdateFileChunk],
) -> std::result::Result<AppliedPatch, ApplyPatchError> {
    let original_contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) => {
            return Err(ApplyPatchError::IoError(IoError {
                context: format!("Failed to read file to update {}", path.display()),
                source: err,
            }));
        }
    };

    let mut original_lines: Vec<String> = original_contents.split('\n').map(String::from).collect();

    // Drop the trailing empty element that results from the final newline so
    // that line counts match the behaviour of standard `diff`.
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }

    let replacements = compute_replacements(&original_lines, path, chunks)?;
    let mut new_lines = apply_replacements(original_lines, &replacements);
    if !new_lines.last().is_some_and(String::is_empty) {
        new_lines.push(String::new());
    }
    let new_contents = new_lines.join("\n");
    Ok(AppliedPatch {
        original_contents,
        new_contents,
    })
}

/// Compute a list of replacements needed to transform `original_lines` into the
/// new lines, given the patch `chunks`. Each replacement is returned as
/// `(start_index, old_len, new_lines)`.
fn compute_replacements(
    original_lines: &[String],
    path: &Path,
    chunks: &[UpdateFileChunk],
) -> std::result::Result<Vec<(usize, usize, Vec<String>)>, ApplyPatchError> {
    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        // If a chunk has a `change_context`, we use seek_sequence to find it, then
        // adjust our `line_index` to continue from there.
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = super::seek_sequence::seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(ApplyPatchError::ComputeReplacements(format!(
                    "Failed to find context '{}' in {}",
                    ctx_line,
                    path.display()
                )));
            }
        }

        if chunk.old_lines.is_empty() {
            // Pure addition (no old lines). We'll add them at the end or just
            // before the final empty line if one exists.
            let insertion_idx = if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push((insertion_idx, 0, chunk.new_lines.clone()));
            continue;
        }

        // Otherwise, try to match the existing lines in the file with the old lines
        // from the chunk. If found, schedule that region for replacement.
        // Attempt to locate the `old_lines` verbatim within the file.  In many
        // real‑world diffs the last element of `old_lines` is an *empty* string
        // representing the terminating newline of the region being replaced.
        // This sentinel is not present in `original_lines` because we strip the
        // trailing empty slice emitted by `split('\n')`.  If a direct search
        // fails and the pattern ends with an empty string, retry without that
        // final element so that modifications touching the end‑of‑file can be
        // located reliably.

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found = super::seek_sequence::seek_sequence(
            original_lines,
            pattern,
            line_index,
            chunk.is_end_of_file,
        );

        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            // Retry without the trailing empty line which represents the final
            // newline in the file.
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }

            found = super::seek_sequence::seek_sequence(
                original_lines,
                pattern,
                line_index,
                chunk.is_end_of_file,
            );
        }

        if let Some(start_idx) = found {
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            line_index = start_idx + pattern.len();
        } else {
            return Err(ApplyPatchError::ComputeReplacements(format!(
                "Failed to find expected lines in {}:\n{}",
                path.display(),
                chunk.old_lines.join("\n"),
            )));
        }
    }

    replacements.sort_by_key(|(idx, _, _)| *idx);

    Ok(replacements)
}

/// Apply the `(start_index, old_len, new_lines)` replacements to `original_lines`,
/// returning the modified file contents as a vector of lines.
fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    // We must apply replacements in descending order so that earlier replacements
    // don't shift the positions of later ones.
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        let old_len = *old_len;

        // Remove old lines.
        for _ in 0..old_len {
            if start_idx < lines.len() {
                lines.remove(start_idx);
            }
        }

        // Insert new lines.
        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
        }
    }

    lines
}

/// Print the summary of changes in git-style format.
pub fn format_summary(affected: &AffectedPaths) -> String {
    let mut output = String::new();
    output.push_str("Success. Updated the following files:\n");
    for path in &affected.added {
        output.push_str(&format!("A {}\n", path.display()));
    }
    for path in &affected.modified {
        output.push_str(&format!("M {}\n", path.display()));
    }
    for path in &affected.deleted {
        output.push_str(&format!("D {}\n", path.display()));
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    /// Helper to construct a patch with the given body.
    fn wrap_patch(body: &str) -> String {
        format!("*** Begin Patch\n{body}\n*** End Patch")
    }

    #[test]
    fn test_add_file_hunk_creates_file_with_contents() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("add.txt");
        let patch = wrap_patch(&format!(
            r#"*** Add File: {}
+ab
+cd"#,
            path.display()
        ));
        let result = apply_patch(&patch, dir.path()).unwrap();
        // Verify expected output
        let expected = AffectedPaths {
            added: vec![path.clone()],
            modified: vec![],
            deleted: vec![],
        };
        assert_eq!(result, expected);
        let contents = fs::read_to_string(path).unwrap();
        assert_eq!(contents, "ab\ncd\n");
    }

    #[test]
    fn test_delete_file_hunk_removes_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("del.txt");
        fs::write(&path, "x").unwrap();
        let patch = wrap_patch(&format!("*** Delete File: {}", path.display()));
        let result = apply_patch(&patch, dir.path()).unwrap();
        let expected = AffectedPaths {
            added: vec![],
            modified: vec![],
            deleted: vec![path.clone()],
        };
        assert_eq!(result, expected);
        assert!(!path.exists());
    }

    #[test]
    fn test_update_file_hunk_modifies_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("update.txt");
        fs::write(&path, "foo\nbar\n").unwrap();
        let patch = wrap_patch(&format!(
            r#"*** Update File: {}
@@
 foo
-bar
+baz"#,
            path.display()
        ));
        let result = apply_patch(&patch, dir.path()).unwrap();
        let expected = AffectedPaths {
            added: vec![],
            modified: vec![path.clone()],
            deleted: vec![],
        };
        assert_eq!(result, expected);
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "foo\nbaz\n");
    }

    #[test]
    fn test_update_file_hunk_can_move_file() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dest = dir.path().join("dst.txt");
        fs::write(&src, "line\n").unwrap();
        let patch = wrap_patch(&format!(
            r#"*** Update File: {}
*** Move to: {}
@@
-line
+line2"#,
            src.display(),
            dest.display()
        ));
        let result = apply_patch(&patch, dir.path()).unwrap();
        let expected = AffectedPaths {
            added: vec![],
            modified: vec![dest.clone()],
            deleted: vec![],
        };
        assert_eq!(result, expected);
        assert!(!src.exists());
        let contents = fs::read_to_string(&dest).unwrap();
        assert_eq!(contents, "line2\n");
    }

    /// Verify that a single `Update File` hunk with multiple change chunks can update different
    /// parts of a file and that the file is listed only once in the summary.
    #[test]
    fn test_multiple_update_chunks_apply_to_single_file() {
        // Start with a file containing four lines.
        let dir = tempdir().unwrap();
        let path = dir.path().join("multi.txt");
        fs::write(&path, "foo\nbar\nbaz\nqux\n").unwrap();
        // Construct an update patch with two separate change chunks.
        // The first chunk uses the line `foo` as context and transforms `bar` into `BAR`.
        // The second chunk uses `baz` as context and transforms `qux` into `QUX`.
        let patch = wrap_patch(&format!(
            r#"*** Update File: {}
@@
 foo
-bar
+BAR
@@
 baz
-qux
+QUX"#,
            path.display()
        ));
        let result = apply_patch(&patch, dir.path()).unwrap();
        let expected = AffectedPaths {
            added: vec![],
            modified: vec![path.clone()],
            deleted: vec![],
        };
        assert_eq!(result, expected);
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "foo\nBAR\nbaz\nQUX\n");
    }

    /// A more involved `Update File` hunk that exercises additions, deletions and
    /// replacements in separate chunks that appear in non‑adjacent parts of the
    /// file.  Verifies that all edits are applied and that the summary lists the
    /// file only once.
    #[test]
    fn test_update_file_hunk_interleaved_changes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("interleaved.txt");

        // Original file: six numbered lines.
        fs::write(&path, "a\nb\nc\nd\ne\nf\n").unwrap();

        // Patch performs:
        //  • Replace `b` → `B`
        //  • Replace `e` → `E` (using surrounding context)
        //  • Append new line `g` at the end‑of‑file
        let patch = wrap_patch(&format!(
            r#"*** Update File: {}
@@
 a
-b
+B
@@
 c
 d
-e
+E
@@
 f
+g
*** End of File"#,
            path.display()
        ));

        let result = apply_patch(&patch, dir.path()).unwrap();

        let expected = AffectedPaths {
            added: vec![],
            modified: vec![path.clone()],
            deleted: vec![],
        };
        assert_eq!(result, expected);

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "a\nB\nc\nd\nE\nf\ng\n");
    }

    #[test]
    fn test_pure_addition_chunk_followed_by_removal() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("panic.txt");
        fs::write(&path, "line1\nline2\nline3\n").unwrap();
        let patch = wrap_patch(&format!(
            r#"*** Update File: {}
@@
+after-context
+second-line
@@
 line1
-line2
-line3
+line2-replacement"#,
            path.display()
        ));
        let _result = apply_patch(&patch, dir.path()).unwrap();
        let contents = fs::read_to_string(path).unwrap();
        assert_eq!(
            contents,
            "line1\nline2-replacement\nafter-context\nsecond-line\n"
        );
    }

    /// Ensure that patches authored with ASCII characters can update lines that
    /// contain typographic Unicode punctuation (e.g. EN DASH, NON-BREAKING
    /// HYPHEN). Historically `git apply` succeeds in such scenarios but our
    /// internal matcher failed requiring an exact byte-for-byte match.  The
    /// fuzzy-matching pass that normalises common punctuation should now bridge
    /// the gap.
    #[test]
    fn test_update_line_with_unicode_dash() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("unicode.py");

        // Original line contains EN DASH (\u{2013}) and NON-BREAKING HYPHEN (\u{2011}).
        let original = "import asyncio  # local import \u{2013} avoids top\u{2011}level dep\n";
        std::fs::write(&path, original).unwrap();

        // Patch uses plain ASCII dash / hyphen.
        let patch = wrap_patch(&format!(
            r#"*** Update File: {}
@@
-import asyncio  # local import - avoids top-level dep
+import asyncio  # HELLO"#,
            path.display()
        ));

        let _result = apply_patch(&patch, dir.path()).unwrap();

        // File should now contain the replaced comment.
        let expected = "import asyncio  # HELLO\n";
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, expected);
    }

    #[test]
    fn test_relative_path_resolution() {
        let dir = tempdir().unwrap();
        let patch = wrap_patch(
            r#"*** Add File: subdir/newfile.txt
+content"#,
        );
        let result = apply_patch(&patch, dir.path()).unwrap();
        let expected_path = dir.path().join("subdir/newfile.txt");
        assert!(expected_path.exists());
        assert_eq!(result.added, vec![expected_path]);
    }

    #[test]
    fn test_format_summary() {
        let affected = AffectedPaths {
            added: vec![PathBuf::from("/tmp/a.txt")],
            modified: vec![PathBuf::from("/tmp/b.txt")],
            deleted: vec![PathBuf::from("/tmp/c.txt")],
        };
        let summary = format_summary(&affected);
        assert!(summary.contains("Success. Updated the following files:"));
        assert!(summary.contains("A /tmp/a.txt"));
        assert!(summary.contains("M /tmp/b.txt"));
        assert!(summary.contains("D /tmp/c.txt"));
    }

    #[test]
    fn test_apply_patch_fails_on_write_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("readonly.txt");
        fs::write(&path, "before\n").unwrap();

        // Make the file read-only
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms).unwrap();

        let patch = wrap_patch(&format!(
            "*** Update File: {}\n@@\n-before\n+after\n*** End Patch",
            path.display()
        ));

        let result = apply_patch(&patch, dir.path());
        assert!(result.is_err());
    }
}
