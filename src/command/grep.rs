//! Provides grep command logic for searching text patterns in working tree, index, or commit trees
//! with regex support, pathspec filtering, and various output formatting options.

use std::{
    io::IsTerminal,
    path::PathBuf,
};

use clap::Parser;
use colored::Colorize;
use regex::RegexBuilder;
use serde::Serialize;
use walkdir::WalkDir;

use crate::{
    command::load_object,
    internal::head::Head,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        ignore::{self, IgnorePolicy},
        output::{OutputConfig, emit_json_data},
        pager::Pager,
        path, util,
    },
};

/// Search for patterns in tracked files in the working tree, index, or commit trees.
#[derive(Parser, Debug)]
pub struct GrepArgs {
    /// The pattern to search for. Supports regular expressions by default.
    #[clap(value_name = "PATTERN")]
    pattern: String,

    /// Interpret pattern as a fixed string, not a regular expression.
    #[clap(short = 'F', long)]
    fixed_string: bool,

    /// Ignore case distinctions in patterns and data.
    #[clap(short = 'i', long)]
    ignore_case: bool,

    /// Show only the number of matching lines for each file.
    #[clap(short = 'c', long)]
    count: bool,

    /// Show only the names of files with matches.
    #[clap(short = 'l', long, alias = "files-with-matches")]
    files_with_matches: bool,

    /// Show only the names of files without matches.
    #[clap(short = 'L', long, alias = "files-without-match")]
    files_without_match: bool,

    /// Show line numbers for matching lines.
    #[clap(short = 'n', long)]
    line_number: bool,

    /// Select only those lines containing matches that form whole words.
    #[clap(short = 'w', long)]
    word_regexp: bool,

    /// Select non-matching lines.
    #[clap(short = 'v', long)]
    invert_match: bool,

    /// Show the column number of the first match on each line (1-based).
    #[clap(short = 'b', long)]
    byte_offset: bool,

    /// Only search in files matching the given pathspec.
    #[clap(value_name = "PATHS", num_args = 0..)]
    pathspec: Vec<String>,

    /// Search in the specified commit or tree instead of the working tree.
    #[clap(short = 'e', long, value_name = "TREE")]
    tree: Option<String>,
}

/// A single grep match result.
#[derive(Debug, Clone, Serialize)]
pub struct GrepMatch {
    /// The file path where the match was found.
    pub path: String,
    /// The line number (1-based) where the match was found.
    pub line_number: usize,
    /// The matching line content.
    pub line: String,
    /// The column offset (0-based) of the match start, if --byte-offset was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<usize>,
}

/// Aggregated count result for a file (used with --count).
#[derive(Debug, Clone, Serialize)]
pub struct GrepCount {
    /// The file path.
    pub path: String,
    /// The number of matching lines in the file.
    pub count: usize,
}

/// Output structure for JSON mode.
#[derive(Debug, Clone, Serialize)]
pub struct GrepOutput {
    /// The pattern searched for.
    pub pattern: String,
    /// The search context (working-tree, index, or tree ref).
    pub context: String,
    /// Total number of matching lines across all files.
    pub total_matches: usize,
    /// Total number of files with matches.
    pub total_files: usize,
    /// Individual match results (when not using --count, -l, or -L).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<GrepMatch>>,
    /// Count results per file (when using --count).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<Vec<GrepCount>>,
    /// Files with matches (when using -l).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_with_matches: Option<Vec<String>>,
    /// Files without matches (when using -L).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_without_match: Option<Vec<String>>,
}

pub async fn execute(args: GrepArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`] instead of printing
/// errors and exiting. Searches for pattern matches in tracked files.
pub async fn execute_safe(args: GrepArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    let result = run_grep(&args).await?;
    render_grep_output(&args, &result, output)
}

async fn run_grep(args: &GrepArgs) -> CliResult<GrepOutput> {
    // Build the regex matcher based on arguments
    let matcher = build_matcher(args)?;

    // Resolve the search context (working tree, index, or specific tree)
    let context_label = if let Some(tree_ref) = &args.tree {
        format!("tree:{}", tree_ref)
    } else {
        "working-tree".to_string()
    };

    // Get the list of files to search
    let files = get_search_files(args).await?;

    // Process each file
    let mut matches: Vec<GrepMatch> = Vec::new();
    let mut counts: Vec<GrepCount> = Vec::new();
    let mut files_with_matches: Vec<String> = Vec::new();
    let mut files_without_match: Vec<String> = Vec::new();
    let mut total_matches = 0usize;

    for file_path in &files {
        let content = read_file_content(file_path, &args.tree).await?;
        let file_matches = search_in_content(&content, &matcher, args);

        if file_matches.is_empty() {
            if args.files_without_match {
                files_without_match.push(file_path.display().to_string());
            }
        } else {
            if args.files_with_matches {
                files_with_matches.push(file_path.display().to_string());
            } else if args.count {
                counts.push(GrepCount {
                    path: file_path.display().to_string(),
                    count: file_matches.len(),
                });
            } else {
                for (line_num, line, byte_off) in file_matches {
                    matches.push(GrepMatch {
                        path: file_path.display().to_string(),
                        line_number: line_num,
                        line,
                        byte_offset: args.byte_offset.then_some(byte_off),
                    });
                }
            }
            total_matches += file_matches.len();
        }
    }

    Ok(GrepOutput {
        pattern: args.pattern.clone(),
        context: context_label,
        total_matches,
        total_files: files_with_matches.len() + files_without_match.len(),
        matches: if !args.count && !args.files_with_matches && !args.files_without_match {
            Some(matches)
        } else {
            None
        },
        counts: args.count.then_some(counts),
        files_with_matches: args.files_with_matches.then_some(files_with_matches),
        files_without_match: args.files_without_match.then_some(files_without_match),
    })
}

/// Build a regex matcher based on command arguments.
fn build_matcher(args: &GrepArgs) -> CliResult<regex::Regex> {
    let pattern = if args.word_regexp {
        // Wrap pattern with word boundaries
        format!(r"\b{}\b", escape_regex(&args.pattern))
    } else if args.fixed_string {
        // Escape regex metacharacters
        escape_regex(&args.pattern)
    } else {
        args.pattern.clone()
    };

    RegexBuilder::new(&pattern)
        .case_insensitive(args.ignore_case)
        .build()
        .map_err(|e| {
            CliError::command_usage(format!("invalid regex pattern '{}': {}", args.pattern, e))
                .with_stable_code(StableErrorCode::CliInvalidArguments)
        })
}

/// Escape regex metacharacters in a string for literal matching.
fn escape_regex(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() * 2);
    for ch in s.chars() {
        if "[](){}.*+?^$|#\\".contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Get the list of files to search, respecting pathspec and ignore rules.
async fn get_search_files(args: &GrepArgs) -> CliResult<Vec<PathBuf>> {
    if let Some(tree_ref) = &args.tree {
        // Search in a specific tree/commit
        get_tree_files(tree_ref, &args.pathspec).await
    } else {
        // Search in working tree
        get_working_tree_files(&args.pathspec)
    }
}

/// Get files from a specific tree or commit.
async fn get_tree_files(tree_ref: &str, pathspec: &[String]) -> CliResult<Vec<PathBuf>> {
    use git_internal::internal::object::tree::Tree;
    use crate::utils::object_ext::TreeExt;

    // Resolve the tree reference
    let commit_hash = if tree_ref.len() == 40 && tree_ref.chars().all(|c| c.is_ascii_hexdigit()) {
        // Direct hash
        git_internal::hash::ObjectHash::from_hex(tree_ref)
            .map_err(|_| CliError::command_usage(format!("invalid object hash: {}", tree_ref))
                .with_stable_code(StableErrorCode::CliInvalidTarget))?
    } else {
        // Try to resolve as branch or ref
        let head = Head::current().await;

        // For now, only support HEAD or direct hashes
        // A more complete implementation would use ref resolution
        if tree_ref == "HEAD" {
            Head::current_commit().await.ok_or_else(|| {
                CliError::fatal("HEAD does not point to a valid commit")
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
            })?
        } else {
            // Try parsing as hex hash
            git_internal::hash::ObjectHash::from_hex(tree_ref)
                .map_err(|_| CliError::command_usage(format!("invalid tree reference: {}", tree_ref))
                    .with_stable_code(StableErrorCode::CliInvalidTarget))?
        }
    };

    // Load the commit and get its tree
    let commit: git_internal::internal::object::commit::Commit = load_object(&commit_hash).map_err(|e| {
        CliError::fatal(format!("failed to load commit '{}': {}", commit_hash, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    let tree: Tree = load_object(&commit.tree_id).map_err(|e| {
        CliError::fatal(format!("failed to load tree '{}': {}", commit.tree_id, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    // Get all files from the tree, filtering by pathspec
    let all_files: Vec<(PathBuf, git_internal::hash::ObjectHash)> = tree.get_plain_items();

    let path_filters: Vec<PathBuf> = pathspec.iter().map(util::to_workdir_path).collect();

    let files = if path_filters.is_empty() {
        all_files.iter().map(|(p, _)| p.clone()).collect()
    } else {
        all_files
            .iter()
            .filter(|(p, _)| path_filters.iter().any(|f| util::is_sub_path(p, f)))
            .map(|(p, _)| p.clone())
            .collect()
    };

    Ok(files)
}

/// Get files from the working tree, respecting ignore rules.
fn get_working_tree_files(pathspec: &[String]) -> CliResult<Vec<PathBuf>> {
    use git_internal::internal::index::Index;

    let index = Index::load(path::index()).map_err(|e| {
        CliError::fatal(format!("failed to load index: {}", e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    let path_filters: Vec<PathBuf> = pathspec.iter().map(util::to_workdir_path).collect();

    // Walk the working directory
    let mut files: Vec<PathBuf> = Vec::new();
    let workdir = util::cur_dir();

    for entry in WalkDir::new(&workdir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let rel_path = util::to_workdir_path(path);

        // Skip files in .libra directory
        if rel_path.starts_with(".libra") {
            continue;
        }

        // Apply ignore rules
        if ignore::should_ignore(&rel_path, IgnorePolicy::Respect, &index) {
            continue;
        }

        // Apply pathspec filter
        if !path_filters.is_empty() {
            if !path_filters.iter().any(|f| util::is_sub_path(&rel_path, f)) {
                continue;
            }
        }

        files.push(rel_path);
    }

    Ok(files)
}

/// Read file content from working tree or from a tree object.
async fn read_file_content(path: &PathBuf, tree_ref: &Option<String>) -> CliResult<Vec<u8>> {
    if let Some(_) = tree_ref {
        // Read from tree object
        use git_internal::internal::object::blob::Blob;

        // We need to find the blob hash for this path
        // This requires looking up the blob in the tree
        // For now, return error for tree searches
        Err(CliError::fatal("tree search requires blob lookup which is not yet implemented")
            .with_stable_code(StableErrorCode::Unsupported))
    } else {
        // Read from working tree
        let abs_path = util::workdir_to_absolute(path);
        std::fs::read(&abs_path).map_err(|e| {
            CliError::fatal(format!("failed to read file '{}': {}", abs_path.display(), e))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })
    }
}

/// Search for pattern matches in file content.
/// Returns a list of (line_number, line_content, byte_offset) tuples.
fn search_in_content(content: &[u8], matcher: &regex::Regex, args: &GrepArgs) -> Vec<(usize, String, usize)> {
    let content_str = String::from_utf8_lossy(content);
    let lines: Vec<&str> = content_str.lines().collect();

    let mut results: Vec<(usize, String, usize)> = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let has_match = if args.invert_match {
            !matcher.is_match(line)
        } else {
            matcher.is_match(line)
        };

        if has_match {
            // Find byte offset of first match
            let byte_off = if args.byte_offset {
                matcher.find(line).map(|m| m.start()).unwrap_or(0)
            } else {
                0
            };

            results.push((line_num, line.to_string(), byte_off));
        }
    }

    results
}

/// Render grep output to stdout or JSON.
fn render_grep_output(args: &GrepArgs, result: &GrepOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("grep", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    let mut pager = Pager::with_config(output)?;
    let should_color = std::io::stdout().is_terminal() && !output.is_json();

    if args.files_with_matches {
        for file in result.files_with_matches.as_ref().unwrap_or(&Vec::new()) {
            pager.write_line(file)?;
        }
    } else if args.files_without_match {
        for file in result.files_without_match.as_ref().unwrap_or(&Vec::new()) {
            pager.write_line(file)?;
        }
    } else if args.count {
        for count in result.counts.as_ref().unwrap_or(&Vec::new()) {
            pager.write_line(&format!("{}:{}", count.path, count.count))?;
        }
    } else {
        // Regular match output with optional highlighting
        for match_item in result.matches.as_ref().unwrap_or(&Vec::new()) {
            let line = if should_color && !args.invert_match {
                colorize_match(&match_item.line, &args.pattern, args.ignore_case)
            } else {
                match_item.line.clone()
            };

            if args.byte_offset {
                pager.write_line(&format!("{}:{}:{}:{}", match_item.path, match_item.line_number, match_item.byte_offset.unwrap_or(0), line))?;
            } else if args.line_number {
                pager.write_line(&format!("{}:{}:{}", match_item.path, match_item.line_number, line))?;
            } else {
                pager.write_line(&format!("{}:{}", match_item.path, line))?;
            }
        }
    }

    pager.finish()?;
    Ok(())
}

/// Colorize matching portions of a line.
fn colorize_match(line: &str, pattern: &str, ignore_case: bool) -> String {
    let search_str = if ignore_case {
        pattern.to_lowercase()
    } else {
        pattern.to_string()
    };
    let line_cmp = if ignore_case {
        line.to_lowercase()
    } else {
        line.to_string()
    };

    // Simple approach: find all matches and colorize
    let mut result = String::new();
    let mut last_end = 0;

    let mut search_pos = 0;
    while search_pos < line_cmp.len() {
        if let Some(start) = line_cmp[search_pos..].find(&search_str) {
            let abs_start = search_pos + start;
            let abs_end = abs_start + search_str.len();

            // Add non-matching prefix
            result.push_str(&line[last_end..abs_start.min(line.len())]);
            // Add colored match
            if abs_end <= line.len() {
                result.push_str(&line[abs_start..abs_end].red().bold().to_string());
            }
            last_end = abs_end.min(line.len());
            search_pos = abs_end;
        } else {
            break;
        }
    }

    // Add remaining text
    if last_end < line.len() {
        result.push_str(&line[last_end..]);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_regex() {
        assert_eq!(escape_regex("hello"), "hello");
        assert_eq!(escape_regex("foo.bar"), "foo\\.bar");
        assert_eq!(escape_regex("a*b+c?"), "a\\*b\\+c\\?");
        assert_eq!(escape_regex("[test]"), "\\[test\\]");
        assert_eq!(escape_regex("(group)"), "\\(group\\)");
        assert_eq!(escape_regex("$100"), "\\$100");
        assert_eq!(escape_regex("^start"), "\\^start");
        assert_eq!(escape_regex("a|b"), "a\\|b");
        assert_eq!(escape_regex("#comment"), "\\#comment");
        assert_eq!(escape_regex("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn test_grep_args_parsing() {
        let args = GrepArgs::parse_from(["libra", "grep", "pattern"]);
        assert_eq!(args.pattern, "pattern");
        assert!(!args.fixed_string);
        assert!(!args.ignore_case);
        assert!(!args.line_number);

        let args = GrepArgs::parse_from(["libra", "grep", "-i", "-n", "pattern"]);
        assert!(args.ignore_case);
        assert!(args.line_number);

        let args = GrepArgs::parse_from(["libra", "grep", "-F", "-l", "pattern"]);
        assert!(args.fixed_string);
        assert!(args.files_with_matches);

        let args = GrepArgs::parse_from(["libra", "grep", "-c", "-w", "pattern"]);
        assert!(args.count);
        assert!(args.word_regexp);

        let args = GrepArgs::parse_from(["libra", "grep", "pattern", "src/", "lib/"]);
        assert_eq!(args.pathspec, vec!["src/", "lib/"]);
    }

    #[test]
    fn test_build_matcher_basic() {
        let args = GrepArgs::parse_from(["libra", "grep", "test"]);
        let matcher = build_matcher(&args).unwrap();
        assert!(matcher.is_match("this is a test"));
        assert!(!matcher.is_match("no match here"));
    }

    #[test]
    fn test_build_matcher_fixed_string() {
        let args = GrepArgs::parse_from(["libra", "grep", "-F", "foo.bar"]);
        let matcher = build_matcher(&args).unwrap();
        assert!(matcher.is_match("this is foo.bar"));
        // With fixed string, the dot should not match any character
        assert!(!matcher.is_match("this is fooXbar"));
    }

    #[test]
    fn test_build_matcher_case_insensitive() {
        let args = GrepArgs::parse_from(["libra", "grep", "-i", "HELLO"]);
        let matcher = build_matcher(&args).unwrap();
        assert!(matcher.is_match("hello world"));
        assert!(matcher.is_match("HELLO WORLD"));
        assert!(matcher.is_match("HeLLo WoRLd"));
    }

    #[test]
    fn test_build_matcher_word_regexp() {
        let args = GrepArgs::parse_from(["libra", "grep", "-w", "test"]);
        let matcher = build_matcher(&args).unwrap();
        assert!(matcher.is_match("this is a test"));
        assert!(matcher.is_match("test case"));
        assert!(!matcher.is_match("testing"));
        assert!(!matcher.is_match("atestb"));
    }

    #[test]
    fn test_search_in_content_simple() {
        let content = b"line one\nline two\nline three\n";
        let args = GrepArgs::parse_from(["libra", "grep", "two"]);
        let matcher = build_matcher(&args).unwrap();

        let results = search_in_content(content, &matcher, &args);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 2); // line number
        assert_eq!(results[0].1, "line two");
    }

    #[test]
    fn test_search_in_content_invert() {
        let content = b"line one\nline two\nline three\n";
        let args = GrepArgs::parse_from(["libra", "grep", "-v", "two"]);
        let matcher = build_matcher(&args).unwrap();

        let results = search_in_content(content, &matcher, &args);
        assert_eq!(results.len(), 2); // lines 1 and 3
        assert_eq!(results[0].0, 1);
        assert_eq!(results[1].0, 3);
    }

    #[test]
    fn test_search_in_content_multiple_matches() {
        let content = b"hello world\nhello again\nno match\nhello there\n";
        let args = GrepArgs::parse_from(["libra", "grep", "hello"]);
        let matcher = build_matcher(&args).unwrap();

        let results = search_in_content(content, &matcher, &args);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_search_in_content_with_byte_offset() {
        let content = b"hello world\nfoo bar\n";
        let args = GrepArgs::parse_from(["libra", "grep", "-b", "bar"]);
        let matcher = build_matcher(&args).unwrap();

        let results = search_in_content(content, &matcher, &args);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 2);
        assert_eq!(results[0].2, 4); // "bar" starts at position 4 in "foo bar"
    }

    #[test]
    fn test_colorize_match_basic() {
        colored::control::set_override(true);
        let line = "hello world hello";
        let colored = colorize_match(line, "hello", false);
        assert!(colored.contains("\u{1b}[")); // Contains ANSI escape
        colored::control::unset_override();
    }

    #[test]
    fn test_colorize_match_case_insensitive() {
        colored::control::set_override(true);
        let line = "Hello World HELLO";
        let colored = colorize_match(line, "hello", true);
        assert!(colored.contains("\u{1b}["));
        colored::control::unset_override();
    }

    #[test]
    fn test_colorize_match_preserves_content() {
        let line = "hello world";
        let colored = colorize_match(line, "hello", false);
        // Remove ANSI codes and check content is preserved
        let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
            .unwrap()
            .replace_all(&colored, "");
        assert_eq!(plain, "hello world");
    }
}