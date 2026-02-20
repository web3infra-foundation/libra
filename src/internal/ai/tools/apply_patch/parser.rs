//! This module is responsible for parsing & validating a patch into a list of "hunks".
//! (It does not attempt to actually check that the patch can be applied to the filesystem.)
//!
//! The official Lark grammar for the apply-patch format is:
//!
//! start: begin_patch hunk+ end_patch
//! begin_patch: "*** Begin Patch" LF
//! end_patch: "*** End Patch" LF?
//!
//! hunk: add_hunk | delete_hunk | update_hunk
//! add_hunk: "*** Add File: " filename LF add_line+
//! delete_hunk: "*** Delete File: " filename LF
//! update_hunk: "*** Update File: " filename LF change_move? change?
//! filename: /(.+)/
//! add_line: "+" /(.+)/ LF -> line
//!
//! change_move: "*** Move to: " filename LF
//! change: (change_context | change_line)+ eof_line?
//! change_context: ("@@" | "@@ " /(.+)/) LF
//! change_line: ("+" | "-" | " ") /(.+)/ LF
//! eof_line: "*** End of File" LF
//!
//! The parser below is a little more lenient than the explicit spec and allows for
//! leading/trailing whitespace around patch markers.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
const END_PATCH_MARKER: &str = "*** End Patch";
const ADD_FILE_MARKER: &str = "*** Add File: ";
const DELETE_FILE_MARKER: &str = "*** Delete File: ";
const UPDATE_FILE_MARKER: &str = "*** Update File: ";
const MOVE_TO_MARKER: &str = "*** Move to: ";
const EOF_MARKER: &str = "*** End of File";
const CHANGE_CONTEXT_MARKER: &str = "@@ ";
const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";

/// Currently, the only OpenAI model that knowingly requires lenient parsing is
/// gpt-4.1. While we could try to require everyone to pass in a strictness
/// param when invoking apply_patch, it is a pain to thread it through all of
/// the call sites, so we resign ourselves allowing lenient parsing for all
/// models. See [`ParseMode::Lenient`] for details on the exceptions we make for
/// gpt-4.1.
const PARSE_IN_STRICT_MODE: bool = false;

#[derive(Debug, PartialEq, Error, Clone)]
pub enum ParseError {
    #[error("invalid patch: {0}")]
    InvalidPatchError(String),
    #[error("invalid hunk at line {line_number}, {message}")]
    InvalidHunkError { message: String, line_number: usize },
}
use ParseError::*;

#[derive(Debug, PartialEq, Clone)]
#[allow(clippy::enum_variant_names)]
pub enum Hunk {
    AddFile {
        path: PathBuf,
        contents: String,
    },
    DeleteFile {
        path: PathBuf,
    },
    UpdateFile {
        path: PathBuf,
        move_path: Option<PathBuf>,

        /// Chunks should be in order, i.e. the `change_context` of one chunk
        /// should occur later in the file than the previous chunk.
        chunks: Vec<UpdateFileChunk>,
    },
}

impl Hunk {
    pub fn resolve_path(&self, cwd: &Path) -> PathBuf {
        match self {
            Hunk::AddFile { path, .. } => cwd.join(path),
            Hunk::DeleteFile { path } => cwd.join(path),
            Hunk::UpdateFile { path, .. } => cwd.join(path),
        }
    }

    /// Return every resolved path that this hunk will touch on the filesystem,
    /// including `move_path` targets for `UpdateFile` hunks.
    pub fn all_resolved_paths(&self, cwd: &Path) -> Vec<PathBuf> {
        match self {
            Hunk::AddFile { path, .. } => vec![cwd.join(path)],
            Hunk::DeleteFile { path } => vec![cwd.join(path)],
            Hunk::UpdateFile {
                path, move_path, ..
            } => {
                let mut paths = vec![cwd.join(path)];
                if let Some(dest) = move_path {
                    paths.push(cwd.join(dest));
                }
                paths
            }
        }
    }
}

use Hunk::*;

#[derive(Debug, PartialEq, Clone)]
pub struct UpdateFileChunk {
    /// A single line of context used to narrow down the position of the chunk
    /// (this is usually a class, method, or function definition.)
    pub change_context: Option<String>,

    /// A contiguous block of lines that should be replaced with `new_lines`.
    /// `old_lines` must occur strictly after `change_context`.
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,

    /// If set to true, `old_lines` must occur at the end of the source file.
    /// (Tolerance around trailing newlines should be encouraged.)
    pub is_end_of_file: bool,
}

/// Arguments for applying a patch.
#[derive(Debug, PartialEq, Clone, Deserialize)]
pub struct ApplyPatchArgs {
    /// The patch string in Codex format.
    #[serde(alias = "patch", alias = "text")]
    pub input: String,
    #[serde(skip)]
    pub hunks: Vec<Hunk>,
}

pub fn parse_patch(patch: &str) -> Result<ApplyPatchArgs, ParseError> {
    let mode = if PARSE_IN_STRICT_MODE {
        ParseMode::Strict
    } else {
        ParseMode::Lenient
    };
    parse_patch_text(patch, mode)
}

enum ParseMode {
    /// Parse the patch text argument as is.
    Strict,

    /// GPT-4.1 is known to formulate the `command` array for the `local_shell`
    /// tool call for `apply_patch` call using something like the following:
    ///
    /// ```json
    /// [
    ///   "apply_patch",
    ///   "<<'EOF'\n*** Begin Patch\n*** Update File: README.md\n@@...\n*** End Patch\nEOF\n",
    /// ]
    /// ```
    ///
    /// This is a problem because `local_shell` is a bit of a misnomer: the
    /// `command` is not invoked by passing the arguments to a shell like Bash,
    /// but are invoked using something akin to `execvpe(3)`.
    ///
    /// This is significant in this case because where a shell would interpret
    /// `<<'EOF'...` as a heredoc and pass the contents via stdin (which is
    /// fine, as `apply_patch` is specified to read from stdin if no argument is
    /// passed), `execvpe(3)` interprets the heredoc as a literal string. To get
    /// the `local_shell` tool to run a command the way shell would, the
    /// `command` array must be something like:
    ///
    /// ```json
    /// [
    ///   "bash",
    ///   "-lc",
    ///   "apply_patch <<'EOF'\n*** Begin Patch\n*** Update File: README.md\n@@...\n*** End Patch\nEOF\n",
    /// ]
    /// ```
    ///
    /// In lenient mode, we check if the argument to `apply_patch` starts with
    /// `<<'EOF'` and ends with `EOF\n`. If so, we strip off these markers,
    /// trim() the result, and treat what is left as the patch text.
    Lenient,
}

fn parse_patch_text(patch: &str, mode: ParseMode) -> Result<ApplyPatchArgs, ParseError> {
    let trimmed = patch.trim();

    // In lenient mode, auto-complete a truncated patch that is missing the
    // closing *** End Patch marker. This handles the common model failure of
    // stopping generation before the patch is fully written.
    let auto_completed;
    let effective_text = if matches!(mode, ParseMode::Lenient) {
        let first_line = trimmed.lines().next().map(str::trim).unwrap_or("");
        let last_line = trimmed.lines().last().map(str::trim).unwrap_or("");
        // Only auto-complete when the patch is NOT heredoc-wrapped (heredoc form
        // is handled by check_patch_boundaries_lenient and must not be modified).
        let is_heredoc = first_line.starts_with("<<");
        if !is_heredoc && trimmed.contains(BEGIN_PATCH_MARKER) && last_line != END_PATCH_MARKER {
            auto_completed = format!("{trimmed}\n{END_PATCH_MARKER}");
            auto_completed.as_str()
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    let lines: Vec<&str> = effective_text.lines().collect();
    let lines: &[&str] = match check_patch_boundaries_strict(&lines) {
        Ok(()) => &lines,
        Err(e) => match mode {
            ParseMode::Strict => {
                return Err(e);
            }
            ParseMode::Lenient => check_patch_boundaries_lenient(&lines, e)?,
        },
    };

    let mut hunks: Vec<Hunk> = Vec::new();
    // The above checks ensure that lines.len() >= 2.
    let last_line_index = lines.len().saturating_sub(1);
    let mut remaining_lines = &lines[1..last_line_index];
    let mut line_number = 2;
    while !remaining_lines.is_empty() {
        let (hunk, hunk_lines) = parse_one_hunk(remaining_lines, line_number)?;
        hunks.push(hunk);
        line_number += hunk_lines;
        remaining_lines = &remaining_lines[hunk_lines..]
    }
    let input = lines.join("\n");
    Ok(ApplyPatchArgs { hunks, input })
}

/// Checks the start and end lines of the patch text for `apply_patch`,
/// returning an error if they do not match the expected markers.
fn check_patch_boundaries_strict(lines: &[&str]) -> Result<(), ParseError> {
    let (first_line, last_line) = match lines {
        [] => (None, None),
        [first] => (Some(first), Some(first)),
        [first, .., last] => (Some(first), Some(last)),
    };
    check_start_and_end_lines_strict(first_line, last_line)
}

/// If we are in lenient mode, we check if the first line starts with `<<EOF`
/// (possibly quoted) and the last line ends with `EOF`. There must be at least
/// 4 lines total because the heredoc markers take up 2 lines and the patch text
/// must have at least 2 lines.
///
/// If successful, returns the lines of the patch text that contain the patch
/// contents, excluding the heredoc markers.
fn check_patch_boundaries_lenient<'a>(
    original_lines: &'a [&'a str],
    original_parse_error: ParseError,
) -> Result<&'a [&'a str], ParseError> {
    match original_lines {
        [first, .., last] => {
            if (first == &"<<EOF" || first == &"<<'EOF'" || first == &"<<\"EOF\"")
                && last.ends_with("EOF")
                && original_lines.len() >= 4
            {
                let inner_lines = &original_lines[1..original_lines.len() - 1];
                match check_patch_boundaries_strict(inner_lines) {
                    Ok(()) => Ok(inner_lines),
                    Err(e) => Err(e),
                }
            } else {
                Err(original_parse_error)
            }
        }
        _ => Err(original_parse_error),
    }
}

fn check_start_and_end_lines_strict(
    first_line: Option<&&str>,
    last_line: Option<&&str>,
) -> Result<(), ParseError> {
    let first_line = first_line.map(|line| line.trim());
    let last_line = last_line.map(|line| line.trim());

    match (first_line, last_line) {
        (Some(first), Some(last)) if first == BEGIN_PATCH_MARKER && last == END_PATCH_MARKER => {
            Ok(())
        }
        (Some(first), _) if first != BEGIN_PATCH_MARKER => Err(InvalidPatchError(String::from(
            "The first line of the patch must be '*** Begin Patch'",
        ))),
        _ => Err(InvalidPatchError(String::from(
            "The last line of the patch must be '*** End Patch'",
        ))),
    }
}

/// Attempts to parse a single hunk from the start of lines.
/// Returns the parsed hunk and the number of lines parsed (or a ParseError).
fn parse_one_hunk(lines: &[&str], line_number: usize) -> Result<(Hunk, usize), ParseError> {
    // Be tolerant of case mismatches and extra padding around marker strings.
    let first_line = lines[0].trim();
    if let Some(path) = first_line.strip_prefix(ADD_FILE_MARKER) {
        // Add File
        let mut contents = String::new();
        let mut parsed_lines = 1;
        for add_line in &lines[1..] {
            if let Some(line_to_add) = add_line.strip_prefix('+') {
                contents.push_str(line_to_add);
                contents.push('\n');
                parsed_lines += 1;
            } else {
                break;
            }
        }
        return Ok((
            AddFile {
                path: PathBuf::from(path),
                contents,
            },
            parsed_lines,
        ));
    } else if let Some(path) = first_line.strip_prefix(DELETE_FILE_MARKER) {
        // Delete File
        return Ok((
            DeleteFile {
                path: PathBuf::from(path),
            },
            1,
        ));
    } else if let Some(path) = first_line.strip_prefix(UPDATE_FILE_MARKER) {
        // Update File
        let mut remaining_lines = &lines[1..];
        let mut parsed_lines = 1;

        // Optional: move file line
        let move_path = remaining_lines
            .first()
            .and_then(|x| x.strip_prefix(MOVE_TO_MARKER));

        if move_path.is_some() {
            remaining_lines = &remaining_lines[1..];
            parsed_lines += 1;
        }

        let mut chunks = Vec::new();
        // NOTE: we need to know to stop once we reach the next special marker header.
        while !remaining_lines.is_empty() {
            // Skip over any completely blank lines that may separate chunks.
            if remaining_lines[0].trim().is_empty() {
                parsed_lines += 1;
                remaining_lines = &remaining_lines[1..];
                continue;
            }

            if remaining_lines[0].starts_with("***") {
                break;
            }

            let (chunk, chunk_lines) = parse_update_file_chunk(
                remaining_lines,
                line_number + parsed_lines,
                chunks.is_empty(),
            )?;
            chunks.push(chunk);
            parsed_lines += chunk_lines;
            remaining_lines = &remaining_lines[chunk_lines..]
        }

        if chunks.is_empty() {
            return Err(InvalidHunkError {
                message: format!("Update file hunk for path '{path}' is empty"),
                line_number,
            });
        }

        return Ok((
            UpdateFile {
                path: PathBuf::from(path),
                move_path: move_path.map(PathBuf::from),
                chunks,
            },
            parsed_lines,
        ));
    }

    Err(InvalidHunkError {
        message: format!(
            "'{first_line}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
        ),
        line_number,
    })
}

/// Strip the read_file `L{n}:` line-number prefix if present.
///
/// Matches `L` followed by one or more ASCII digits then `:` and an optional
/// single space. Returns the remainder of the string after the prefix.
fn strip_line_number_prefix(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'L') {
        return None;
    }
    let mut i = 1;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i <= 1 || bytes.get(i) != Some(&b':') {
        return None;
    }

    let mut j = i + 1;
    if bytes.get(j) == Some(&b' ') {
        j += 1;
    }

    Some(&s[j..])
}

/// Strip a read_file line-number prefix, allowing a single leading space
/// (common when the model writes `- L{n}: ...` or `  L{n}: ...`).
fn strip_line_number_prefix_optional_space(s: &str) -> &str {
    if let Some(stripped) = strip_line_number_prefix(s) {
        return stripped;
    }
    if let Some(rest) = s.strip_prefix(' ')
        && let Some(stripped) = strip_line_number_prefix(rest)
    {
        return stripped;
    }
    s
}

fn parse_update_file_chunk(
    lines: &[&str],
    line_number: usize,
    allow_missing_context: bool,
) -> Result<(UpdateFileChunk, usize), ParseError> {
    if lines.is_empty() {
        return Err(InvalidHunkError {
            message: "Update hunk does not contain any lines".to_string(),
            line_number,
        });
    }
    // If we see an explicit context marker @@ or @@ <context>, consume it; otherwise, optionally
    // allow treating the chunk as starting directly with diff lines.
    let (change_context, start_index) = if lines[0] == EMPTY_CHANGE_CONTEXT_MARKER {
        (None, 1)
    } else if let Some(context) = lines[0].strip_prefix(CHANGE_CONTEXT_MARKER) {
        // If the text after "@@ " is empty (model wrote "@@ " with trailing space
        // instead of just "@@"), treat it as no context to avoid matching a random
        // empty line and mis-advancing the file position.
        let context = if context.trim().is_empty() {
            None
        } else {
            Some(context.to_string())
        };
        (context, 1)
    } else {
        if !allow_missing_context {
            return Err(InvalidHunkError {
                message: format!(
                    "Expected update hunk to start with a @@ context marker, got: '{}'",
                    lines[0]
                ),
                line_number,
            });
        }
        (None, 0)
    };
    if start_index >= lines.len() {
        return Err(InvalidHunkError {
            message: "Update hunk does not contain any lines".to_string(),
            line_number: line_number + 1,
        });
    }
    let mut chunk = UpdateFileChunk {
        change_context,
        old_lines: Vec::new(),
        new_lines: Vec::new(),
        is_end_of_file: false,
    };
    let mut parsed_lines = 0;
    for line in &lines[start_index..] {
        match *line {
            EOF_MARKER => {
                if parsed_lines == 0 {
                    return Err(InvalidHunkError {
                        message: "Update hunk does not contain any lines".to_string(),
                        line_number: line_number + 1,
                    });
                }
                chunk.is_end_of_file = true;
                parsed_lines += 1;
                break;
            }
            raw_line => {
                let mut effective = raw_line;

                if effective.is_empty() {
                    chunk.old_lines.push(String::new());
                    chunk.new_lines.push(String::new());
                    parsed_lines += 1;
                    continue;
                }

                // Handle lines that start with the diff marker directly.
                // Also strip an optional `L{n}:` prefix from the *content*
                // portion (common model mistake: `-L12: foo` / `- L12: foo`).
                match effective.as_bytes().first().copied() {
                    Some(b' ') | Some(b'+') | Some(b'-') => {
                        let marker = effective.as_bytes()[0] as char;
                        let mut content = &effective[1..];
                        content = strip_line_number_prefix_optional_space(content);

                        match marker {
                            ' ' => {
                                chunk.old_lines.push(content.to_string());
                                chunk.new_lines.push(content.to_string());
                            }
                            '+' => chunk.new_lines.push(content.to_string()),
                            '-' => chunk.old_lines.push(content.to_string()),
                            _ => {}
                        }

                        parsed_lines += 1;
                        continue;
                    }
                    _ => {}
                }

                // Otherwise, first strip a read_file prefix from the whole line
                // (handles `L12: -foo` / `L12:  foo`, etc).
                effective = strip_line_number_prefix_optional_space(effective);

                if effective.is_empty() {
                    chunk.old_lines.push(String::new());
                    chunk.new_lines.push(String::new());
                    parsed_lines += 1;
                    continue;
                }

                // After stripping, the line may now start with a diff marker
                // (e.g. `L2: -remove` → `-remove`).
                match effective.as_bytes().first().copied() {
                    Some(b' ') | Some(b'+') | Some(b'-') => {
                        let marker = effective.as_bytes()[0] as char;
                        let content = &effective[1..];
                        match marker {
                            ' ' => {
                                chunk.old_lines.push(content.to_string());
                                chunk.new_lines.push(content.to_string());
                            }
                            '+' => chunk.new_lines.push(content.to_string()),
                            '-' => chunk.old_lines.push(content.to_string()),
                            _ => {}
                        }
                        parsed_lines += 1;
                        continue;
                    }
                    _ => {}
                }

                // Structural markers end the chunk.
                if effective.starts_with("***") || effective.starts_with("@@") {
                    break;
                }

                // Treat as a context line without the leading space.
                // Models sometimes omit the required space prefix on
                // context lines; interpreting them as context lets
                // seek_sequence validate against the actual file.
                chunk.old_lines.push(effective.to_string());
                chunk.new_lines.push(effective.to_string());
                parsed_lines += 1;
            }
        }
    }

    Ok((chunk, parsed_lines + start_index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_patch() {
        assert_eq!(
            parse_patch_text("bad", ParseMode::Strict),
            Err(InvalidPatchError(
                "The first line of the patch must be '*** Begin Patch'".to_string()
            ))
        );
        assert_eq!(
            parse_patch_text("*** Begin Patch\nbad", ParseMode::Strict),
            Err(InvalidPatchError(
                "The last line of the patch must be '*** End Patch'".to_string()
            ))
        );

        assert_eq!(
            parse_patch_text(
                concat!(
                    "*** Begin Patch",
                    " ",
                    "\n*** Add File: foo\n+hi\n",
                    " ",
                    "*** End Patch"
                ),
                ParseMode::Strict
            )
            .unwrap()
            .hunks,
            vec![AddFile {
                path: PathBuf::from("foo"),
                contents: "hi\n".to_string()
            }]
        );
        assert_eq!(
            parse_patch_text(
                "*** Begin Patch\n\
                 *** Update File: test.py\n\
                 *** End Patch",
                ParseMode::Strict
            ),
            Err(InvalidHunkError {
                message: "Update file hunk for path 'test.py' is empty".to_string(),
                line_number: 2,
            })
        );
        assert_eq!(
            parse_patch_text(
                "*** Begin Patch\n\
                 *** End Patch",
                ParseMode::Strict
            )
            .unwrap()
            .hunks,
            Vec::new()
        );
        assert_eq!(
            parse_patch_text(
                "*** Begin Patch\n\
                 *** Add File: path/add.py\n\
                 +abc\n\
                 +def\n\
                 *** Delete File: path/delete.py\n\
                 *** Update File: path/update.py\n\
                 *** Move to: path/update2.py\n\
                 @@ def f():\n\
                 -    pass\n\
                 +    return 123\n\
                 *** End Patch",
                ParseMode::Strict
            )
            .unwrap()
            .hunks,
            vec![
                AddFile {
                    path: PathBuf::from("path/add.py"),
                    contents: "abc\ndef\n".to_string()
                },
                DeleteFile {
                    path: PathBuf::from("path/delete.py")
                },
                UpdateFile {
                    path: PathBuf::from("path/update.py"),
                    move_path: Some(PathBuf::from("path/update2.py")),
                    chunks: vec![UpdateFileChunk {
                        change_context: Some("def f():".to_string()),
                        old_lines: vec!["    pass".to_string()],
                        new_lines: vec!["    return 123".to_string()],
                        is_end_of_file: false
                    }]
                }
            ]
        );
        // Update hunk followed by another hunk (Add File).
        assert_eq!(
            parse_patch_text(
                "*** Begin Patch\n\
                 *** Update File: file.py\n\
                 @@\n\
                 +line\n\
                 *** Add File: other.py\n\
                 +content\n\
                 *** End Patch",
                ParseMode::Strict
            )
            .unwrap()
            .hunks,
            vec![
                UpdateFile {
                    path: PathBuf::from("file.py"),
                    move_path: None,
                    chunks: vec![UpdateFileChunk {
                        change_context: None,
                        old_lines: vec![],
                        new_lines: vec!["line".to_string()],
                        is_end_of_file: false
                    }],
                },
                AddFile {
                    path: PathBuf::from("other.py"),
                    contents: "content\n".to_string()
                }
            ]
        );

        // Update hunk without an explicit @@ header for the first chunk should parse.
        // Use a raw string to preserve the leading space diff marker on the context line.
        assert_eq!(
            parse_patch_text(
                r#"*** Begin Patch
*** Update File: file2.py
 import foo
+bar
*** End Patch"#,
                ParseMode::Strict
            )
            .unwrap()
            .hunks,
            vec![UpdateFile {
                path: PathBuf::from("file2.py"),
                move_path: None,
                chunks: vec![UpdateFileChunk {
                    change_context: None,
                    old_lines: vec!["import foo".to_string()],
                    new_lines: vec!["import foo".to_string(), "bar".to_string()],
                    is_end_of_file: false,
                }],
            }]
        );
    }

    #[test]
    fn test_parse_patch_lenient() {
        let patch_text = r#"*** Begin Patch
*** Update File: file2.py
 import foo
+bar
*** End Patch"#;
        let expected_patch = vec![UpdateFile {
            path: PathBuf::from("file2.py"),
            move_path: None,
            chunks: vec![UpdateFileChunk {
                change_context: None,
                old_lines: vec!["import foo".to_string()],
                new_lines: vec!["import foo".to_string(), "bar".to_string()],
                is_end_of_file: false,
            }],
        }];
        let expected_error =
            InvalidPatchError("The first line of the patch must be '*** Begin Patch'".to_string());

        let patch_text_in_heredoc = format!("<<EOF\n{patch_text}\nEOF\n");
        assert_eq!(
            parse_patch_text(&patch_text_in_heredoc, ParseMode::Strict),
            Err(expected_error.clone())
        );
        assert_eq!(
            parse_patch_text(&patch_text_in_heredoc, ParseMode::Lenient),
            Ok(ApplyPatchArgs {
                hunks: expected_patch.clone(),
                input: patch_text.to_string(),
            })
        );

        let patch_text_in_single_quoted_heredoc = format!("<<'EOF'\n{patch_text}\nEOF\n");
        assert_eq!(
            parse_patch_text(&patch_text_in_single_quoted_heredoc, ParseMode::Strict),
            Err(expected_error.clone())
        );
        assert_eq!(
            parse_patch_text(&patch_text_in_single_quoted_heredoc, ParseMode::Lenient),
            Ok(ApplyPatchArgs {
                hunks: expected_patch.clone(),
                input: patch_text.to_string(),
            })
        );

        let patch_text_in_double_quoted_heredoc = format!("<<\"EOF\"\n{patch_text}\nEOF\n");
        assert_eq!(
            parse_patch_text(&patch_text_in_double_quoted_heredoc, ParseMode::Strict),
            Err(expected_error.clone())
        );
        assert_eq!(
            parse_patch_text(&patch_text_in_double_quoted_heredoc, ParseMode::Lenient),
            Ok(ApplyPatchArgs {
                hunks: expected_patch,
                input: patch_text.to_string(),
            })
        );

        let patch_text_in_mismatched_quotes_heredoc = format!("<<\"EOF'\n{patch_text}\nEOF\n");
        assert_eq!(
            parse_patch_text(&patch_text_in_mismatched_quotes_heredoc, ParseMode::Strict),
            Err(expected_error.clone())
        );
        assert_eq!(
            parse_patch_text(&patch_text_in_mismatched_quotes_heredoc, ParseMode::Lenient),
            Err(expected_error.clone())
        );

        let patch_text_with_missing_closing_heredoc =
            "<<EOF\n*** Begin Patch\n*** Update File: file2.py\nEOF\n".to_string();
        assert_eq!(
            parse_patch_text(&patch_text_with_missing_closing_heredoc, ParseMode::Strict),
            Err(expected_error)
        );
        assert_eq!(
            parse_patch_text(&patch_text_with_missing_closing_heredoc, ParseMode::Lenient),
            Err(InvalidPatchError(
                "The last line of the patch must be '*** End Patch'".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_one_hunk() {
        assert_eq!(
            parse_one_hunk(&["bad"], 234),
            Err(InvalidHunkError {
                message: "'bad' is not a valid hunk header. \
                Valid hunk headers: '*** Add File: {path}', '*** Delete File: {path}', '*** Update File: {path}'".to_string(),
                line_number: 234
            })
        );
        // Other edge cases are already covered by tests above/below.
    }

    #[test]
    fn test_update_file_chunk() {
        // "bad" line without @@ marker and allow_missing_context=false errors.
        assert_eq!(
            parse_update_file_chunk(&["bad"], 123, false),
            Err(InvalidHunkError {
                message: "Expected update hunk to start with a @@ context marker, got: 'bad'"
                    .to_string(),
                line_number: 123
            })
        );
        assert_eq!(
            parse_update_file_chunk(&["@@"], 123, false),
            Err(InvalidHunkError {
                message: "Update hunk does not contain any lines".to_string(),
                line_number: 124
            })
        );
        // "bad" after @@ is treated as a context line (lenient: model forgot
        // the leading space).  seek_sequence will catch actual mismatches.
        assert_eq!(
            parse_update_file_chunk(&["@@", "bad"], 123, false),
            Ok((
                (UpdateFileChunk {
                    change_context: None,
                    old_lines: vec!["bad".to_string()],
                    new_lines: vec!["bad".to_string()],
                    is_end_of_file: false
                }),
                2
            ))
        );
        assert_eq!(
            parse_update_file_chunk(&["@@", "*** End of File"], 123, false),
            Err(InvalidHunkError {
                message: "Update hunk does not contain any lines".to_string(),
                line_number: 124
            })
        );
        assert_eq!(
            parse_update_file_chunk(
                &[
                    "@@ change_context",
                    "",
                    " context",
                    "-remove",
                    "+add",
                    " context2",
                    "*** End Patch",
                ],
                123,
                false
            ),
            Ok((
                (UpdateFileChunk {
                    change_context: Some("change_context".to_string()),
                    old_lines: vec![
                        "".to_string(),
                        "context".to_string(),
                        "remove".to_string(),
                        "context2".to_string()
                    ],
                    new_lines: vec![
                        "".to_string(),
                        "context".to_string(),
                        "add".to_string(),
                        "context2".to_string()
                    ],
                    is_end_of_file: false
                }),
                6
            ))
        );
        assert_eq!(
            parse_update_file_chunk(&["@@", "+line", "*** End of File"], 123, false),
            Ok((
                (UpdateFileChunk {
                    change_context: None,
                    old_lines: vec![],
                    new_lines: vec!["line".to_string()],
                    is_end_of_file: true
                }),
                3
            ))
        );
    }

    #[test]
    fn test_lenient_mode_auto_completes_missing_end_patch() {
        // A truncated patch (missing *** End Patch) should be auto-completed in lenient mode.
        let truncated = "*** Begin Patch\n*** Update File: foo.txt\n@@\n-old\n+new";
        let result = parse_patch_text(truncated, ParseMode::Lenient);
        assert!(
            result.is_ok(),
            "expected lenient mode to auto-complete: {result:?}"
        );
        let args = result.unwrap();
        assert_eq!(args.hunks.len(), 1);
        match &args.hunks[0] {
            UpdateFile { path, chunks, .. } => {
                assert_eq!(path, &PathBuf::from("foo.txt"));
                assert_eq!(chunks[0].old_lines, vec!["old"]);
                assert_eq!(chunks[0].new_lines, vec!["new"]);
            }
            _ => panic!("expected UpdateFile hunk"),
        }
    }

    #[test]
    fn test_lenient_mode_does_not_auto_complete_when_end_patch_present() {
        // Lenient mode must NOT double-append *** End Patch when already present.
        let complete = "*** Begin Patch\n*** Add File: bar.txt\n+hello\n*** End Patch";
        let result = parse_patch_text(complete, ParseMode::Lenient);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(result.unwrap().hunks.len(), 1);
    }

    #[test]
    fn test_strict_mode_rejects_missing_end_patch() {
        // Strict mode must still reject a truncated patch.
        let truncated = "*** Begin Patch\n*** Update File: foo.txt\n@@\n-old\n+new";
        assert!(matches!(
            parse_patch_text(truncated, ParseMode::Strict),
            Err(InvalidPatchError(_))
        ));
    }

    #[test]
    fn test_non_at_line_treated_as_context() {
        // When the model includes a non-diff line like "## Next Section" after
        // the first chunk, the parser treats it as a context line (lenient mode:
        // the model forgot the leading space). seek_sequence will catch any
        // actual mismatch with the file later.
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Update File: readme.md\n",
            "@@\n",
            "-# Old Title\n",
            "+# New Title\n",
            " \n",           // empty context line (single space)
            " Some text.\n", // context line (space + content)
            "## Next Section\n",
            "*** End Patch",
        );
        let result = parse_patch_text(patch, ParseMode::Lenient);
        // "## Next Section" is treated as a context line, not rejected.
        assert!(result.is_ok(), "failed to parse: {result:?}");
        let args = result.unwrap();
        assert_eq!(args.hunks.len(), 1);
        if let Hunk::UpdateFile { chunks, .. } = &args.hunks[0] {
            assert_eq!(chunks.len(), 1);
            // "## Next Section" should be in old_lines and new_lines as context.
            assert!(chunks[0].old_lines.contains(&"## Next Section".to_string()));
        } else {
            panic!("expected UpdateFile hunk");
        }
    }

    #[test]
    fn test_line_number_prefix_stripped_in_chunk() {
        // When the model copies lines from read_file output (L{n}: prefix)
        // directly into the patch, the parser should strip the prefix and
        // interpret the line correctly.
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Update File: test.rs\n",
            "@@\n",
            "L1:  fn main() {\n", // context line: L1: then space + content
            "L2: -    old();\n",  // removal: L2: then -content
            "L3: +    new();\n",  // addition: L3: then +content
            "L4:  }\n",           // context line
            "*** End Patch",
        );
        let result = parse_patch_text(patch, ParseMode::Lenient);
        assert!(result.is_ok(), "failed to parse with L prefix: {result:?}");
        let args = result.unwrap();
        assert_eq!(args.hunks.len(), 1);
        if let Hunk::UpdateFile { chunks, .. } = &args.hunks[0] {
            assert_eq!(chunks.len(), 1);
            // old_lines: context "fn main() {", removed "    old();", context "}"
            assert_eq!(chunks[0].old_lines, vec!["fn main() {", "    old();", "}"]);
            // new_lines: context "fn main() {", added "    new();", context "}"
            assert_eq!(chunks[0].new_lines, vec!["fn main() {", "    new();", "}"]);
        } else {
            panic!("expected UpdateFile hunk");
        }
    }

    #[test]
    fn test_line_number_prefix_blank_line() {
        // A blank line from read_file output appears as "L{n}: " (nothing after space).
        // After stripping the prefix, this should be treated as an empty context line.
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Update File: test.txt\n",
            "@@\n",
            "L1:  first\n",
            "L2: \n", // blank line: L2: then nothing → empty after strip
            "L3: -old\n",
            "L4: +new\n",
            "*** End Patch",
        );
        let result = parse_patch_text(patch, ParseMode::Lenient);
        assert!(result.is_ok(), "failed to parse blank L-prefix: {result:?}");
        let args = result.unwrap();
        if let Hunk::UpdateFile { chunks, .. } = &args.hunks[0] {
            assert_eq!(chunks[0].old_lines, vec!["first", "", "old"]);
            assert_eq!(chunks[0].new_lines, vec!["first", "", "new"]);
        } else {
            panic!("expected UpdateFile hunk");
        }
    }

    #[test]
    fn test_line_number_prefix_stripped_after_diff_marker() {
        // Common model mistake: include the read_file L{n}: prefix *after* the
        // diff marker (e.g. `-L2: foo` or `- L2: foo`).
        assert_eq!(
            parse_update_file_chunk(&["@@", " L1: keep", "-L2: remove", "+ L3: add"], 10, false),
            Ok((
                UpdateFileChunk {
                    change_context: None,
                    old_lines: vec!["keep".to_string(), "remove".to_string()],
                    new_lines: vec!["keep".to_string(), "add".to_string()],
                    is_end_of_file: false,
                },
                4
            ))
        );
    }
}
