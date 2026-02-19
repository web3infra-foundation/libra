//! Handler for the list_dir tool.
//!
//! Lists entries in a local directory with pagination, sorted alphabetically.
//! Mirrors the behaviour of the codex list_dir tool:
//!   - Directories shown with a trailing `/`
//!   - Symlinks shown with a trailing `@`
//!   - Supports `offset` + `limit` pagination
//!   - `depth` controls recursive traversal (default: 2)

use std::{
    collections::VecDeque,
    ffi::OsStr,
    fs::FileType,
    path::{Path, PathBuf},
};

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

pub struct ListDirHandler;

const MAX_ENTRY_LENGTH: usize = 500;
const INDENTATION: usize = 2;

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

        if args.offset == 0 {
            return Err(ToolError::InvalidArguments(
                "offset must be a 1-indexed entry number".to_string(),
            ));
        }
        if args.limit == 0 {
            return Err(ToolError::InvalidArguments(
                "limit must be greater than zero".to_string(),
            ));
        }
        if args.depth == 0 {
            return Err(ToolError::InvalidArguments(
                "depth must be greater than zero".to_string(),
            ));
        }

        let path = Path::new(&args.dir_path);
        if !path.is_absolute() {
            return Err(ToolError::PathNotAbsolute(path.to_path_buf()));
        }
        validate_path(path, &working_dir)?;

        let entries = list_dir_slice(path, args.offset, args.limit, args.depth).await?;

        let mut output = Vec::with_capacity(entries.len() + 1);
        output.push(format!("Absolute path: {}", path.display()));
        output.extend(entries);

        Ok(ToolOutput::success(output.join("\n")))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "list_dir",
            "Lists entries in a local directory with 1-indexed entry numbers and type labels (/ for dirs, @ for symlinks).",
        )
        .with_parameters(FunctionParameters::object(
            [
                ("dir_path", "string", "Absolute path to the directory to list"),
                ("offset", "integer", "1-indexed entry number to start listing from (default: 1)"),
                ("limit", "integer", "Maximum number of entries to return (default: 25)"),
                ("depth", "integer", "Maximum directory depth to traverse (default: 2, must be >= 1)"),
            ],
            [("dir_path", true)],
        ))
    }
}

// ── Entry collection ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

impl From<&FileType> for EntryKind {
    fn from(ft: &FileType) -> Self {
        if ft.is_symlink() {
            EntryKind::Symlink
        } else if ft.is_dir() {
            EntryKind::Directory
        } else if ft.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        }
    }
}

struct DirEntry {
    /// Sort key (normalised relative path).
    sort_key: String,
    /// Display name (just the filename component).
    display_name: String,
    /// Nesting depth relative to root (0 = root's children).
    depth: usize,
    kind: EntryKind,
}

async fn list_dir_slice(
    root: &Path,
    offset: usize,
    limit: usize,
    max_depth: usize,
) -> Result<Vec<String>, ToolError> {
    let mut entries = Vec::new();
    collect_entries(root, Path::new(""), max_depth, &mut entries).await?;

    if entries.is_empty() {
        return Ok(Vec::new());
    }

    entries.sort_unstable_by(|a, b| a.sort_key.cmp(&b.sort_key));

    let start = offset - 1;
    if start >= entries.len() {
        return Err(ToolError::ExecutionFailed(
            "offset exceeds directory entry count".to_string(),
        ));
    }

    let remaining = entries.len() - start;
    let capped = limit.min(remaining);
    let end = start + capped;
    let selected = &entries[start..end];

    let mut formatted: Vec<String> = selected.iter().map(format_entry).collect();
    if end < entries.len() {
        formatted.push(format!("More than {capped} entries found"));
    }

    Ok(formatted)
}

async fn collect_entries(
    dir: &Path,
    prefix: &Path,
    depth: usize,
    out: &mut Vec<DirEntry>,
) -> Result<(), ToolError> {
    let mut queue: VecDeque<(PathBuf, PathBuf, usize)> = VecDeque::new();
    queue.push_back((dir.to_path_buf(), prefix.to_path_buf(), depth));

    while let Some((current_dir, current_prefix, remaining)) = queue.pop_front() {
        let mut read_dir = fs::read_dir(&current_dir).await.map_err(|e| {
            ToolError::ExecutionFailed(format!(
                "failed to read directory '{}': {e}",
                current_dir.display()
            ))
        })?;

        let mut batch: Vec<(PathBuf, PathBuf, EntryKind, DirEntry)> = Vec::new();

        while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
            ToolError::ExecutionFailed(format!("failed to read directory entry: {e}"))
        })? {
            let file_type = entry
                .file_type()
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("failed to inspect entry: {e}")))?;

            let file_name = entry.file_name();
            let relative = if current_prefix.as_os_str().is_empty() {
                PathBuf::from(&file_name)
            } else {
                current_prefix.join(&file_name)
            };

            let display_depth = current_prefix.components().count();
            let sort_key = truncate_path(&relative);
            let display_name = truncate_name(&file_name);
            let kind = EntryKind::from(&file_type);

            batch.push((
                entry.path(),
                relative,
                kind,
                DirEntry {
                    sort_key,
                    display_name,
                    depth: display_depth,
                    kind,
                },
            ));
        }

        batch.sort_unstable_by(|a, b| a.3.sort_key.cmp(&b.3.sort_key));

        for (abs_path, rel_path, kind, dir_entry) in batch {
            if kind == EntryKind::Directory && remaining > 1 {
                queue.push_back((abs_path, rel_path, remaining - 1));
            }
            out.push(dir_entry);
        }
    }

    Ok(())
}

fn format_entry(entry: &DirEntry) -> String {
    let indent = " ".repeat(entry.depth * INDENTATION);
    let mut name = entry.display_name.clone();
    match entry.kind {
        EntryKind::Directory => name.push('/'),
        EntryKind::Symlink => name.push('@'),
        EntryKind::Other => name.push('?'),
        EntryKind::File => {}
    }
    format!("{indent}{name}")
}

fn truncate_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.len() > MAX_ENTRY_LENGTH {
        s[..MAX_ENTRY_LENGTH].to_string()
    } else {
        s
    }
}

fn truncate_name(name: &OsStr) -> String {
    let s = name.to_string_lossy();
    if s.len() > MAX_ENTRY_LENGTH {
        s[..MAX_ENTRY_LENGTH].to_string()
    } else {
        s.to_string()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::internal::ai::tools::{ToolKind, context::ToolPayload};

    fn make_invocation(args: serde_json::Value, working_dir: PathBuf) -> ToolInvocation {
        ToolInvocation::new(
            "call-1",
            "list_dir",
            ToolPayload::Function {
                arguments: args.to_string(),
            },
            working_dir,
        )
    }

    // ── Output format ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_dirs_have_trailing_slash() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        fs::create_dir(dir.join("subdir")).unwrap();
        fs::write(dir.join("file.txt"), "").unwrap();

        let result = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": dir, "depth": 1 }),
                dir.to_path_buf(),
            ))
            .await
            .unwrap();

        let text = result.as_text().unwrap();
        assert!(text.contains("subdir/"), "expected trailing /: {text}");
        assert!(text.contains("file.txt"), "expected plain file: {text}");
        assert!(!text.contains("[DIR]"), "should not contain [DIR] label");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_symlinks_have_at_suffix() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        fs::write(dir.join("target.txt"), "").unwrap();
        symlink(dir.join("target.txt"), dir.join("link")).unwrap();

        let result = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": dir, "depth": 1 }),
                dir.to_path_buf(),
            ))
            .await
            .unwrap();

        let text = result.as_text().unwrap();
        assert!(text.contains("link@"), "expected @ suffix: {text}");
    }

    #[tokio::test]
    async fn test_absolute_path_header() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        fs::write(dir.join("a.txt"), "").unwrap();

        let result = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": dir }),
                dir.to_path_buf(),
            ))
            .await
            .unwrap();

        let text = result.as_text().unwrap();
        let first_line = text.lines().next().unwrap();
        assert!(
            first_line.starts_with("Absolute path:"),
            "expected 'Absolute path:' header: {text}"
        );
    }

    // ── Pagination ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_limit_truncates_and_adds_notice() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        for i in 0..10 {
            fs::write(dir.join(format!("file{i:02}.txt")), "").unwrap();
        }

        let result = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": dir, "limit": 3, "depth": 1 }),
                dir.to_path_buf(),
            ))
            .await
            .unwrap();

        let text = result.as_text().unwrap();
        // 1 header + 3 entries + 1 "More than..." notice = 5 lines
        assert_eq!(text.lines().count(), 5, "{text}");
        assert!(text.contains("More than 3 entries found"), "{text}");
    }

    #[tokio::test]
    async fn test_offset_skips_entries() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        // Create files that sort as a.txt, b.txt, c.txt
        fs::write(dir.join("a.txt"), "").unwrap();
        fs::write(dir.join("b.txt"), "").unwrap();
        fs::write(dir.join("c.txt"), "").unwrap();

        let result = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": dir, "offset": 2, "limit": 10, "depth": 1 }),
                dir.to_path_buf(),
            ))
            .await
            .unwrap();

        let text = result.as_text().unwrap();
        assert!(!text.contains("a.txt"), "should skip first entry: {text}");
        assert!(text.contains("b.txt"), "{text}");
        assert!(text.contains("c.txt"), "{text}");
    }

    // ── Depth ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_depth_controls_recursion() {
        let temp = TempDir::new().unwrap();
        let dir = temp.path();
        let nested = dir.join("sub");
        let deeper = nested.join("deeper");
        fs::create_dir_all(&deeper).unwrap();
        fs::write(deeper.join("leaf.txt"), "").unwrap();

        // depth=1: only top-level entries visible
        let shallow = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": dir, "depth": 1 }),
                dir.to_path_buf(),
            ))
            .await
            .unwrap();
        let text = shallow.as_text().unwrap();
        assert!(text.contains("sub/"), "{text}");
        assert!(
            !text.contains("leaf.txt"),
            "depth=1 should not show leaf: {text}"
        );

        // depth=3: leaf visible
        let deep = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": dir, "depth": 3 }),
                dir.to_path_buf(),
            ))
            .await
            .unwrap();
        assert!(deep.as_text().unwrap().contains("leaf.txt"));
    }

    // ── Parameter validation ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_depth_zero_rejected() {
        let temp = TempDir::new().unwrap();
        let result = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": temp.path(), "depth": 0 }),
                temp.path().to_path_buf(),
            ))
            .await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[tokio::test]
    async fn test_path_outside_working_dir_fails() {
        let sandbox = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let result = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": outside.path() }),
                sandbox.path().to_path_buf(),
            ))
            .await;
        assert!(matches!(result, Err(ToolError::PathOutsideWorkingDir(_))));
    }

    #[tokio::test]
    async fn test_relative_path_fails() {
        let temp = TempDir::new().unwrap();
        let result = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": "relative/path" }),
                temp.path().to_path_buf(),
            ))
            .await;
        assert!(matches!(result, Err(ToolError::PathNotAbsolute(_))));
    }

    #[tokio::test]
    async fn test_empty_directory() {
        let temp = TempDir::new().unwrap();
        let result = ListDirHandler
            .handle(make_invocation(
                serde_json::json!({ "dir_path": temp.path() }),
                temp.path().to_path_buf(),
            ))
            .await
            .unwrap();
        // Only the header line, no entries.
        let text = result.as_text().unwrap();
        assert_eq!(
            text.lines().count(),
            1,
            "empty dir should have only header: {text}"
        );
    }

    // ── Schema ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_kind_and_schema() {
        assert_eq!(ListDirHandler.kind(), ToolKind::Function);
        let schema = ListDirHandler.schema();
        assert_eq!(schema.function.name, "list_dir");
        let json = schema.to_json();
        let required = json["function"]["parameters"]["required"]
            .as_array()
            .unwrap();
        assert!(required.iter().any(|v| v == "dir_path"));
        assert!(!required.iter().any(|v| v == "depth"));
        assert!(!required.iter().any(|v| v == "max_depth"));
    }
}
