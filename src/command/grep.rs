//! Provides grep command logic for searching text patterns in working tree, index, or commit trees
//! with regex support, pathspec filtering, and various output formatting options.

use std::{fs, io::IsTerminal, path::PathBuf};

use clap::Parser;
use colored::Colorize;
use git_internal::internal::index::Index;
use regex::RegexBuilder;
use serde::Serialize;

use crate::{
    command::load_object,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        pager::Pager,
        path, util,
    },
};

/// Search for patterns in tracked files in the working tree, index, or commit trees.
#[derive(Parser, Debug)]
pub struct GrepArgs {
    /// The pattern to search for. Supports regular expressions by default.
    #[clap(value_name = "PATTERN", required_unless_present_any = ["regexp", "pattern_file"])]
    pattern: Option<String>,

    /// Add a pattern to search for. Can be specified multiple times.
    #[clap(short = 'e', long = "regexp", value_name = "PATTERN", action = clap::ArgAction::Append)]
    regexp: Vec<String>,

    /// Read patterns from a file, one per line. Can be specified multiple times.
    #[clap(short = 'f', long = "file", value_name = "FILE", action = clap::ArgAction::Append)]
    pattern_file: Vec<String>,

    /// Require all patterns to match at least once in a file.
    #[clap(long)]
    all_match: bool,

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
    files_without_matches: bool,

    /// Show line numbers for matching lines.
    #[clap(short = 'n', long)]
    line_number: bool,

    /// Select only those lines containing matches that form whole words.
    #[clap(short = 'w', long)]
    word_regexp: bool,

    /// Select non-matching lines.
    #[clap(short = 'v', long)]
    invert_match: bool,

    /// Show the 0-based byte offset of the first match on each line.
    #[clap(short = 'b', long)]
    byte_offset: bool,

    /// Only search in files matching the given pathspec.
    #[clap(value_name = "PATHS", num_args = 0..)]
    pathspec: Vec<String>,

    /// Search in the specified revision or commit instead of the working tree.
    #[clap(long, value_name = "REVISION")]
    tree: Option<String>,

    /// Search within tracked files in the index (staging area) instead of the working tree.
    #[clap(long)]
    cached: bool,
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
    /// The 0-based byte offset of the match start, if --byte-offset was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<usize>,
}

/// Internal representation of a file to search, with optional blob hash for tree/index searches.
struct SearchFile {
    /// Relative path from working directory root.
    path: PathBuf,
    /// Blob hash for tree/index searches (None for working tree searches).
    blob_hash: Option<git_internal::hash::ObjectHash>,
}

/// Aggregated count result for a file (used with --count).
#[derive(Debug, Clone, Serialize)]
pub struct GrepCount {
    /// The file path.
    pub path: String,
    /// The number of matching lines in the file.
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GrepWarning {
    /// The file path that triggered the warning.
    pub path: String,
    /// Human-readable warning message.
    pub message: String,
}

/// Output structure for JSON mode.
#[derive(Debug, Clone, Serialize)]
pub struct GrepOutput {
    /// The pattern searched for.
    pub pattern: String,
    /// The full list of effective patterns searched.
    pub patterns: Vec<String>,
    /// The search context (working-tree, index, or tree ref).
    pub context: String,
    /// Total number of matching lines across all files.
    pub total_matches: usize,
    /// Total number of files with at least one matching line.
    pub total_files: usize,
    /// Individual match results (when not using --count, -l, or -L).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matches: Option<Vec<GrepMatch>>,
    /// Count results per file (when using --count). Each count is the number of matching lines in that file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counts: Option<Vec<GrepCount>>,
    /// Files with matches (when using -l).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_with_matches: Option<Vec<String>>,
    /// Files without matches (when using -L).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_without_matches: Option<Vec<String>>,
    /// Warnings about skipped or unreadable files.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub warnings: Vec<GrepWarning>,
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

/// Maximum file size to search (512KB, matching Git's core.bigFileThreshold default).
const MAX_FILE_SIZE: u64 = 512 * 1024;

/// Check if content appears to be binary (contains NUL bytes).
fn is_binary(content: &[u8]) -> bool {
    // Git checks the first 8000 bytes for NUL
    content.iter().take(8000).any(|&b| b == 0)
}

async fn run_grep(args: &GrepArgs) -> CliResult<GrepOutput> {
    let patterns = collect_patterns(args)?;
    let matcher = build_matcher(&patterns, args)?;
    let all_matchers = if args.all_match && patterns.len() > 1 {
        Some(build_individual_matchers(&patterns, args)?)
    } else {
        None
    };

    // Resolve the search context (working tree, index, or specific tree)
    let context_label = if let Some(tree_ref) = &args.tree {
        format!("tree:{}", tree_ref)
    } else if args.cached {
        "index".to_string()
    } else {
        "working-tree".to_string()
    };

    // Get the list of files to search
    let files = get_search_files(args).await?;

    // Process each file
    let mut matches: Vec<GrepMatch> = Vec::new();
    let mut counts: Vec<GrepCount> = Vec::new();
    let mut files_with_matches: Vec<String> = Vec::new();
    let mut files_without_matches: Vec<String> = Vec::new();
    let mut warnings: Vec<GrepWarning> = Vec::new();
    let mut total_matches = 0usize;
    let mut matched_file_count = 0usize;

    for search_file in &files {
        let path_str = search_file.path.display().to_string();
        let content = match read_file_content(search_file) {
            Ok(c) => c,
            Err(e) => {
                warnings.push(GrepWarning {
                    path: path_str,
                    message: e,
                });
                continue;
            }
        };

        if is_binary(&content) {
            warnings.push(GrepWarning {
                path: search_file.path.display().to_string(),
                message: "skipped binary file".to_string(),
            });
            continue;
        }

        let file_matches = search_in_content(&content, &matcher, args);
        let all_patterns_match = all_matchers.as_ref().is_none_or(|matchers| {
            matchers.iter().all(|pattern_matcher| {
                content
                    .split(|&byte| byte == b'\n')
                    .map(String::from_utf8_lossy)
                    .any(|line| pattern_matcher.is_match(&line))
            })
        });
        let match_count = if all_patterns_match {
            file_matches.len()
        } else {
            0
        };

        if match_count == 0 {
            if args.files_without_matches {
                files_without_matches.push(search_file.path.display().to_string());
            }
        } else {
            matched_file_count += 1;
            if args.files_with_matches {
                files_with_matches.push(search_file.path.display().to_string());
            } else if args.count {
                counts.push(GrepCount {
                    path: search_file.path.display().to_string(),
                    count: file_matches.len(),
                });
            } else {
                for (line_num, line, byte_off) in file_matches {
                    matches.push(GrepMatch {
                        path: search_file.path.display().to_string(),
                        line_number: line_num,
                        line,
                        byte_offset: args.byte_offset.then_some(byte_off),
                    });
                }
            }
            total_matches += match_count;
        }
    }

    Ok(GrepOutput {
        pattern: patterns.first().cloned().unwrap_or_default(),
        patterns,
        context: context_label,
        total_matches,
        total_files: matched_file_count,
        matches: if !args.count && !args.files_with_matches && !args.files_without_matches {
            Some(matches)
        } else {
            None
        },
        counts: args.count.then_some(counts),
        files_with_matches: args.files_with_matches.then_some(files_with_matches),
        files_without_matches: args.files_without_matches.then_some(files_without_matches),
        warnings,
    })
}

fn collect_patterns(args: &GrepArgs) -> CliResult<Vec<String>> {
    let mut patterns = Vec::new();

    if let Some(pattern) = &args.pattern {
        patterns.push(pattern.clone());
    }

    patterns.extend(args.regexp.iter().cloned());

    for pattern_file in &args.pattern_file {
        let content = fs::read_to_string(pattern_file).map_err(|e| {
            CliError::fatal(format!(
                "failed to read pattern file '{}': {}",
                pattern_file, e
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        patterns.extend(
            content
                .lines()
                .filter(|line| !line.is_empty())
                .map(ToString::to_string),
        );
    }

    if patterns.is_empty() {
        return Err(
            CliError::command_usage("at least one search pattern is required")
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }

    Ok(patterns)
}

/// Build a regex matcher based on command arguments.
fn build_matcher(patterns: &[String], args: &GrepArgs) -> CliResult<regex::Regex> {
    let compiled_patterns = normalize_patterns(patterns, args);

    let combined = if compiled_patterns.len() == 1 {
        compiled_patterns[0].clone()
    } else {
        format!("(?:{})", compiled_patterns.join(")|(?:"))
    };

    RegexBuilder::new(&combined)
        .case_insensitive(args.ignore_case)
        .build()
        .map_err(|e| {
            CliError::command_usage(format!(
                "invalid regex pattern '{}': {}",
                patterns.join(", "),
                e
            ))
            .with_stable_code(StableErrorCode::CliInvalidArguments)
        })
}

fn build_individual_matchers(patterns: &[String], args: &GrepArgs) -> CliResult<Vec<regex::Regex>> {
    normalize_patterns(patterns, args)
        .into_iter()
        .map(|pattern| {
            RegexBuilder::new(&pattern)
                .case_insensitive(args.ignore_case)
                .build()
                .map_err(|e| {
                    CliError::command_usage(format!("invalid regex pattern '{}': {}", pattern, e))
                        .with_stable_code(StableErrorCode::CliInvalidArguments)
                })
        })
        .collect()
}

fn normalize_patterns(patterns: &[String], args: &GrepArgs) -> Vec<String> {
    patterns
        .iter()
        .map(|pattern| {
            let mut normalized = if args.fixed_string {
                escape_regex(pattern)
            } else {
                pattern.clone()
            };

            if args.word_regexp {
                normalized = format!(r"\b(?:{})\b", normalized);
            }

            normalized
        })
        .collect()
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
async fn get_search_files(args: &GrepArgs) -> CliResult<Vec<SearchFile>> {
    if let Some(tree_ref) = &args.tree {
        // Search in a specific tree/commit
        get_tree_files(tree_ref, &args.pathspec).await
    } else if args.cached {
        // Search in index (staged files)
        get_index_files(&args.pathspec)
    } else {
        // Search in working tree
        get_working_tree_files(&args.pathspec)
    }
}

async fn get_tree_files(tree_ref: &str, pathspec: &[String]) -> CliResult<Vec<SearchFile>> {
    use git_internal::internal::object::tree::Tree;

    use crate::utils::object_ext::TreeExt;

    let commit_hash = util::get_commit_base(tree_ref).await.map_err(|_| {
        CliError::command_usage(format!("invalid revision: {}", tree_ref))
            .with_stable_code(StableErrorCode::CliInvalidTarget)
    })?;

    // Load the commit and get its tree.
    let commit: git_internal::internal::object::commit::Commit = load_object(&commit_hash)
        .map_err(|e| {
            CliError::fatal(format!("failed to load commit '{}': {}", commit_hash, e))
                .with_stable_code(StableErrorCode::RepoCorrupt)
        })?;

    let tree: Tree = load_object(&commit.tree_id).map_err(|e| {
        CliError::fatal(format!("failed to load tree '{}': {}", commit.tree_id, e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;

    // Get all files from the tree with their blob hashes.
    let all_files: Vec<(PathBuf, git_internal::hash::ObjectHash)> = tree.get_plain_items();

    let path_filters: Vec<PathBuf> = pathspec.iter().map(util::to_workdir_path).collect();

    let files: Vec<SearchFile> = if path_filters.is_empty() {
        all_files
            .into_iter()
            .map(|(path, blob_hash)| SearchFile {
                path,
                blob_hash: Some(blob_hash),
            })
            .collect()
    } else {
        all_files
            .into_iter()
            .filter(|(p, _)| path_filters.iter().any(|f| util::is_sub_path(p, f)))
            .map(|(path, blob_hash)| SearchFile {
                path,
                blob_hash: Some(blob_hash),
            })
            .collect()
    };

    Ok(files)
}

/// Get files from the index (staged files).
fn get_index_files(pathspec: &[String]) -> CliResult<Vec<SearchFile>> {
    let index = load_index()?;
    let path_filters: Vec<PathBuf> = pathspec.iter().map(util::to_workdir_path).collect();

    Ok(tracked_files_from_index(&index, &path_filters, true))
}

/// Get tracked files from the working tree while reading their current on-disk contents.
fn get_working_tree_files(pathspec: &[String]) -> CliResult<Vec<SearchFile>> {
    let index = load_index()?;
    let path_filters: Vec<PathBuf> = pathspec.iter().map(util::to_workdir_path).collect();

    Ok(tracked_files_from_index(&index, &path_filters, false))
}

fn load_index() -> CliResult<Index> {
    Index::load(path::index()).map_err(|e| {
        CliError::fatal(format!("failed to load index: {}", e))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })
}

fn tracked_files_from_index(
    index: &Index,
    path_filters: &[PathBuf],
    include_blob_hash: bool,
) -> Vec<SearchFile> {
    index
        .tracked_entries(0)
        .into_iter()
        .filter(|entry| {
            if path_filters.is_empty() {
                true
            } else {
                let entry_path = PathBuf::from(&entry.name);
                path_filters
                    .iter()
                    .any(|f| util::is_sub_path(&entry_path, f))
            }
        })
        .map(|entry| SearchFile {
            path: PathBuf::from(&entry.name),
            blob_hash: include_blob_hash.then_some(entry.hash),
        })
        .collect()
}

/// Read file content from working tree or from a blob object.
fn read_file_content(search_file: &SearchFile) -> Result<Vec<u8>, String> {
    let content = if let Some(blob_hash) = &search_file.blob_hash {
        // Read from blob object (tree/index search)
        let blob: git_internal::internal::object::blob::Blob =
            load_object(blob_hash).map_err(|e| format!("failed to load blob: {}", e))?;
        blob.data
    } else {
        // Read from working tree
        let abs_path = util::workdir_to_absolute(&search_file.path);

        let metadata = std::fs::symlink_metadata(&abs_path)
            .map_err(|e| format!("failed to stat file: {}", e))?;

        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err("skipped symbolic link".to_string());
        }
        if !file_type.is_file() {
            return Err("skipped non-regular file".to_string());
        }

        // Check file size before reading
        if metadata.len() > MAX_FILE_SIZE {
            return Err(format!(
                "file too large ({} bytes, max {} bytes)",
                metadata.len(),
                MAX_FILE_SIZE
            ));
        }

        std::fs::read(&abs_path).map_err(|e| format!("failed to read file: {}", e))?
    };

    if content.len() as u64 > MAX_FILE_SIZE {
        return Err(format!(
            "file too large ({} bytes, max {} bytes)",
            content.len(),
            MAX_FILE_SIZE
        ));
    }

    Ok(content)
}

/// Search for pattern matches in file content.
/// Returns a list of (line_number, line_content, byte_offset) tuples.
fn search_in_content(
    content: &[u8],
    matcher: &regex::Regex,
    args: &GrepArgs,
) -> Vec<(usize, String, usize)> {
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
fn render_grep_output(
    args: &GrepArgs,
    result: &GrepOutput,
    output: &OutputConfig,
) -> CliResult<()> {
    for _warning in &result.warnings {
        crate::utils::output::record_warning();
    }

    if output.is_json() {
        return emit_json_data("grep", result, output);
    }

    for warning in &result.warnings {
        eprintln!("warning: {}: {}", warning.path, warning.message);
    }

    if output.quiet {
        return Ok(());
    }

    let mut pager = Pager::with_config(output)?;
    let should_color = std::io::stdout().is_terminal() && !output.is_json();
    let matcher = should_color
        .then(|| build_matcher(&result.patterns, args))
        .transpose()?;

    if args.files_with_matches {
        for file in result.files_with_matches.as_ref().unwrap_or(&Vec::new()) {
            pager.write_line(file)?;
        }
    } else if args.files_without_matches {
        for file in result.files_without_matches.as_ref().unwrap_or(&Vec::new()) {
            pager.write_line(file)?;
        }
    } else if args.count {
        for count in result.counts.as_ref().unwrap_or(&Vec::new()) {
            pager.write_line(&format!("{}:{}", count.path, count.count))?;
        }
    } else {
        // Regular match output with optional highlighting
        for match_item in result.matches.as_ref().unwrap_or(&Vec::new()) {
            let line = if let Some(matcher) = matcher.as_ref().filter(|_| !args.invert_match) {
                colorize_match(&match_item.line, matcher)
            } else {
                match_item.line.clone()
            };

            if args.byte_offset {
                pager.write_line(&format!(
                    "{}:{}:{}:{}",
                    match_item.path,
                    match_item.line_number,
                    match_item.byte_offset.unwrap_or(0),
                    line
                ))?;
            } else if args.line_number {
                pager.write_line(&format!(
                    "{}:{}:{}",
                    match_item.path, match_item.line_number, line
                ))?;
            } else {
                pager.write_line(&format!("{}:{}", match_item.path, line))?;
            }
        }
    }

    pager.finish()?;
    Ok(())
}

/// Colorize matching portions of a line using the actual matcher spans.
fn colorize_match(line: &str, matcher: &regex::Regex) -> String {
    let mut result = String::new();
    let mut last_end = 0;

    for matched in matcher.find_iter(line) {
        result.push_str(&line[last_end..matched.start()]);
        result.push_str(&matched.as_str().red().bold().to_string());
        last_end = matched.end();
    }

    result.push_str(&line[last_end..]);
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
        let args = GrepArgs::parse_from(["grep", "pattern"]);
        assert_eq!(args.pattern.as_deref(), Some("pattern"));
        assert!(!args.fixed_string);
        assert!(!args.ignore_case);
        assert!(!args.line_number);

        let args = GrepArgs::parse_from(["grep", "-i", "-n", "pattern"]);
        assert!(args.ignore_case);
        assert!(args.line_number);

        let args = GrepArgs::parse_from(["grep", "-F", "-l", "pattern"]);
        assert!(args.fixed_string);
        assert!(args.files_with_matches);

        let args = GrepArgs::parse_from(["grep", "-L", "pattern"]);
        assert!(args.files_without_matches);

        let args = GrepArgs::parse_from(["grep", "-e", "foo", "-e", "bar"]);
        assert_eq!(args.regexp, vec!["foo", "bar"]);

        let args = GrepArgs::parse_from(["grep", "--all-match", "-e", "foo", "-e", "bar"]);
        assert!(args.all_match);

        let args = GrepArgs::parse_from(["grep", "-c", "-w", "pattern"]);
        assert!(args.count);
        assert!(args.word_regexp);

        let args = GrepArgs::parse_from(["grep", "pattern", "src/", "lib/"]);
        assert_eq!(args.pathspec, vec!["src/", "lib/"]);
    }

    #[test]
    fn test_collect_patterns_merges_positional_and_regexp() {
        let args = GrepArgs::parse_from(["grep", "pattern", "-e", "extra"]);
        let patterns = collect_patterns(&args).unwrap();
        assert_eq!(patterns, vec!["pattern", "extra"]);
    }

    #[test]
    fn test_build_matcher_basic() {
        let args = GrepArgs::parse_from(["grep", "test"]);
        let patterns = collect_patterns(&args).unwrap();
        let matcher = build_matcher(&patterns, &args).unwrap();
        assert!(matcher.is_match("this is a test"));
        assert!(!matcher.is_match("no match here"));
    }

    #[test]
    fn test_build_matcher_fixed_string() {
        let args = GrepArgs::parse_from(["grep", "-F", "foo.bar"]);
        let patterns = collect_patterns(&args).unwrap();
        let matcher = build_matcher(&patterns, &args).unwrap();
        assert!(matcher.is_match("this is foo.bar"));
        // With fixed string, the dot should not match any character
        assert!(!matcher.is_match("this is fooXbar"));
    }

    #[test]
    fn test_build_matcher_case_insensitive() {
        let args = GrepArgs::parse_from(["grep", "-i", "HELLO"]);
        let patterns = collect_patterns(&args).unwrap();
        let matcher = build_matcher(&patterns, &args).unwrap();
        assert!(matcher.is_match("hello world"));
        assert!(matcher.is_match("HELLO WORLD"));
        assert!(matcher.is_match("HeLLo WoRLd"));
    }

    #[test]
    fn test_build_matcher_word_regexp() {
        let args = GrepArgs::parse_from(["grep", "-w", "test"]);
        let patterns = collect_patterns(&args).unwrap();
        let matcher = build_matcher(&patterns, &args).unwrap();
        assert!(matcher.is_match("this is a test"));
        assert!(matcher.is_match("test case"));
        assert!(!matcher.is_match("testing"));
        assert!(!matcher.is_match("atestb"));
    }

    #[test]
    fn test_search_in_content_simple() {
        let content = b"line one\nline two\nline three\n";
        let args = GrepArgs::parse_from(["grep", "two"]);
        let patterns = collect_patterns(&args).unwrap();
        let matcher = build_matcher(&patterns, &args).unwrap();

        let results = search_in_content(content, &matcher, &args);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 2); // line number
        assert_eq!(results[0].1, "line two");
    }

    #[test]
    fn test_search_in_content_invert() {
        let content = b"line one\nline two\nline three\n";
        let args = GrepArgs::parse_from(["grep", "-v", "two"]);
        let patterns = collect_patterns(&args).unwrap();
        let matcher = build_matcher(&patterns, &args).unwrap();

        let results = search_in_content(content, &matcher, &args);
        assert_eq!(results.len(), 2); // lines 1 and 3
        assert_eq!(results[0].0, 1);
        assert_eq!(results[1].0, 3);
    }

    #[test]
    fn test_search_in_content_multiple_matches() {
        let content = b"hello world\nhello again\nno match\nhello there\n";
        let args = GrepArgs::parse_from(["grep", "hello"]);
        let patterns = collect_patterns(&args).unwrap();
        let matcher = build_matcher(&patterns, &args).unwrap();

        let results = search_in_content(content, &matcher, &args);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_search_in_content_with_byte_offset() {
        let content = b"hello world\nfoo bar\n";
        let args = GrepArgs::parse_from(["grep", "-b", "bar"]);
        let patterns = collect_patterns(&args).unwrap();
        let matcher = build_matcher(&patterns, &args).unwrap();

        let results = search_in_content(content, &matcher, &args);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 2);
        assert_eq!(results[0].2, 4); // "bar" starts at byte offset 4 in "foo bar"
    }

    #[test]
    fn test_colorize_match_basic() {
        colored::control::set_override(true);
        let line = "hello world hello";
        let matcher = regex::Regex::new("hello").unwrap();
        let colored = colorize_match(line, &matcher);
        assert!(colored.contains("\u{1b}[")); // Contains ANSI escape
        colored::control::unset_override();
    }

    #[test]
    fn test_colorize_match_regex_highlights_full_match() {
        colored::control::set_override(true);
        let line = "foo123bar baz";
        let matcher = regex::Regex::new(r"foo\d+bar").unwrap();
        let colored = colorize_match(line, &matcher);
        let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
            .unwrap()
            .replace_all(&colored, "");
        assert_eq!(plain, "foo123bar baz");
        assert!(colored.contains("\u{1b}["));
        colored::control::unset_override();
    }

    #[test]
    fn test_colorize_match_case_insensitive() {
        colored::control::set_override(true);
        let line = "Hello World HELLO";
        let matcher = regex::RegexBuilder::new("hello")
            .case_insensitive(true)
            .build()
            .unwrap();
        let colored = colorize_match(line, &matcher);
        let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
            .unwrap()
            .replace_all(&colored, "");
        assert_eq!(plain, "Hello World HELLO");
        assert_ne!(colored, line);
        colored::control::unset_override();
    }

    #[test]
    fn test_colorize_match_preserves_content() {
        let line = "hello world";
        let matcher = regex::Regex::new("hello").unwrap();
        let colored = colorize_match(line, &matcher);
        // Remove ANSI codes and check content is preserved
        let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
            .unwrap()
            .replace_all(&colored, "");
        assert_eq!(plain, "hello world");
    }
}
