//! Integration tests for the grep command.

use std::fs;

use clap::Parser;
use libra::command::grep::{GrepArgs, execute_safe};
use libra::utils::output::OutputConfig;
use libra::utils::test;
use serial_test::serial;
use tempfile::tempdir;

/// Test basic grep search in working tree.
#[tokio::test]
#[serial]
async fn test_grep_basic_search() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Create test files
    fs::write("file1.txt", "hello world\nthis is a test\n").unwrap();
    fs::write("file2.txt", "another file\nno match here\n").unwrap();
    fs::write("file3.txt", "test case\nmore testing\n").unwrap();

    // Add files to index
    let args = libra::command::add::AddArgs {
        pathspec: vec![".".to_string()],
        all: false,
        update: false,
        dry_run: false,
        ignore_pathspec_errors: false,
    };
    libra::command::add::execute_safe(args, &OutputConfig::default())
        .await
        .unwrap();

    // Search for "test"
    let grep_args = GrepArgs::parse_from(["libra", "grep", "test"]);
    let result = execute_safe(grep_args, &OutputConfig::default())
        .await;

    // Should succeed (or return empty if no matches in tracked files)
    // The exact behavior depends on whether we search tracked files only
    assert!(result.is_ok());
}

/// Test grep with --count flag.
#[tokio::test]
#[serial]
async fn test_grep_count() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    fs::write("file.txt", "hello\nhello world\nhello again\nno match\n").unwrap();

    // Add file to index
    let args = libra::command::add::AddArgs {
        pathspec: vec!["file.txt".to_string()],
        all: false,
        update: false,
        dry_run: false,
        ignore_pathspec_errors: false,
    };
    libra::command::add::execute_safe(args, &OutputConfig::default())
        .await
        .unwrap();

    // Search with --count
    let grep_args = GrepArgs::parse_from(["libra", "grep", "-c", "hello"]);
    let result = execute_safe(grep_args, &OutputConfig::default())
        .await;

    assert!(result.is_ok());
}

/// Test grep with --files-with-matches flag.
#[tokio::test]
#[serial]
async fn test_grep_files_with_matches() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    fs::write("match.txt", "found it\n").unwrap();
    fs::write("nomatch.txt", "nothing here\n").unwrap();

    // Add files to index
    let args = libra::command::add::AddArgs {
        pathspec: vec![".".to_string()],
        all: false,
        update: false,
        dry_run: false,
        ignore_pathspec_errors: false,
    };
    libra::command::add::execute_safe(args, &OutputConfig::default())
        .await
        .unwrap();

    // Search with -l
    let grep_args = GrepArgs::parse_from(["libra", "grep", "-l", "found"]);
    let result = execute_safe(grep_args, &OutputConfig::default())
        .await;

    assert!(result.is_ok());
}

/// Test grep with case insensitive flag.
#[tokio::test]
#[serial]
async fn test_grep_ignore_case() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    fs::write("file.txt", "HELLO WORLD\nhello world\nHeLLo WoRLd\n").unwrap();

    // Add file to index
    let args = libra::command::add::AddArgs {
        pathspec: vec!["file.txt".to_string()],
        all: false,
        update: false,
        dry_run: false,
        ignore_pathspec_errors: false,
    };
    libra::command::add::execute_safe(args, &OutputConfig::default())
        .await
        .unwrap();

    // Search with -i
    let grep_args = GrepArgs::parse_from(["libra", "grep", "-i", "hello"]);
    let result = execute_safe(grep_args, &OutputConfig::default())
        .await;

    assert!(result.is_ok());
}

/// Test grep with fixed string flag.
#[tokio::test]
#[serial]
async fn test_grep_fixed_string() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    fs::write("file.txt", "foo.bar\nfooXbar\n").unwrap();

    // Add file to index
    let args = libra::command::add::AddArgs {
        pathspec: vec!["file.txt".to_string()],
        all: false,
        update: false,
        dry_run: false,
        ignore_pathspec_errors: false,
    };
    libra::command::add::execute_safe(args, &OutputConfig::default())
        .await
        .unwrap();

    // Search with -F for "foo.bar" (literal dot, not regex)
    let grep_args = GrepArgs::parse_from(["libra", "grep", "-F", "foo.bar"]);
    let result = execute_safe(grep_args, &OutputConfig::default())
        .await;

    assert!(result.is_ok());
}

/// Test grep with invert match flag.
#[tokio::test]
#[serial]
async fn test_grep_invert_match() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    fs::write("file.txt", "match this\nno match here\nmatch again\n").unwrap();

    // Add file to index
    let args = libra::command::add::AddArgs {
        pathspec: vec!["file.txt".to_string()],
        all: false,
        update: false,
        dry_run: false,
        ignore_pathspec_errors: false,
    };
    libra::command::add::execute_safe(args, &OutputConfig::default())
        .await
        .unwrap();

    // Search with -v (invert)
    let grep_args = GrepArgs::parse_from(["libra", "grep", "-v", "match"]);
    let result = execute_safe(grep_args, &OutputConfig::default())
        .await;

    assert!(result.is_ok());
}

/// Test grep with word regexp flag.
#[tokio::test]
#[serial]
async fn test_grep_word_regexp() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    fs::write("file.txt", "test\ntesting\natestb\ntest case\n").unwrap();

    // Add file to index
    let args = libra::command::add::AddArgs {
        pathspec: vec!["file.txt".to_string()],
        all: false,
        update: false,
        dry_run: false,
        ignore_pathspec_errors: false,
    };
    libra::command::add::execute_safe(args, &OutputConfig::default())
        .await
        .unwrap();

    // Search with -w (word boundary)
    let grep_args = GrepArgs::parse_from(["libra", "grep", "-w", "test"]);
    let result = execute_safe(grep_args, &OutputConfig::default())
        .await;

    assert!(result.is_ok());
}

/// Test grep with pathspec.
#[tokio::test]
#[serial]
async fn test_grep_with_pathspec() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    fs::create_dir_all("src").unwrap();
    fs::create_dir_all("lib").unwrap();
    fs::write("src/main.rs", "fn main() {}\n").unwrap();
    fs::write("lib/util.rs", "pub fn util() {}\n").unwrap();
    fs::write("other.txt", "main function\n").unwrap();

    // Add files to index
    let args = libra::command::add::AddArgs {
        pathspec: vec![".".to_string()],
        all: false,
        update: false,
        dry_run: false,
        ignore_pathspec_errors: false,
    };
    libra::command::add::execute_safe(args, &OutputConfig::default())
        .await
        .unwrap();

    // Search only in src/ directory
    let grep_args = GrepArgs::parse_from(["libra", "grep", "main", "src/"]);
    let result = execute_safe(grep_args, &OutputConfig::default())
        .await;

    assert!(result.is_ok());
}

/// Test grep requires repository.
#[tokio::test]
#[serial]
async fn test_grep_requires_repo() {
    let temp_path = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    // Don't initialize a repo - grep should fail
    let grep_args = GrepArgs::parse_from(["libra", "grep", "pattern"]);
    let result = execute_safe(grep_args, &OutputConfig::default())
        .await;

    assert!(result.is_err());
}

/// Test grep with line numbers.
#[tokio::test]
#[serial]
async fn test_grep_line_numbers() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    fs::write("file.txt", "line 1\nline 2\nline 3\nmatch here\nline 5\n").unwrap();

    // Add file to index
    let args = libra::command::add::AddArgs {
        pathspec: vec!["file.txt".to_string()],
        all: false,
        update: false,
        dry_run: false,
        ignore_pathspec_errors: false,
    };
    libra::command::add::execute_safe(args, &OutputConfig::default())
        .await
        .unwrap();

    // Search with -n (line numbers)
    let grep_args = GrepArgs::parse_from(["libra", "grep", "-n", "match"]);
    let result = execute_safe(grep_args, &OutputConfig::default())
        .await;

    assert!(result.is_ok());
}