//! Tests log command output ordering and formatting of commit history.

use std::{cmp::min, str::FromStr};

use clap::Parser;
use git_internal::{
    Diff,
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree},
};
use libra::utils::{object_ext::TreeExt, util};

use super::*;
#[tokio::test]
#[serial]
/// Tests retrieval of commits reachable from a specific commit hash
async fn test_get_reachable_commits() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let commit_id = create_test_commit_tree().await;

    let reachable_commits = get_reachable_commits(commit_id).await;
    assert_eq!(reachable_commits.len(), 6);
}

#[tokio::test]
#[serial]
/// Tests log command execution functionality
async fn test_execute_log() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    let _ = create_test_commit_tree().await;

    // let args = LogArgs { number: Some(1) };
    // execute(args).await;
    let head = Head::current().await;
    // check if the current branch has any commits
    if let Head::Branch(branch_name) = head.to_owned() {
        let branch = Branch::find_branch(&branch_name, None).await;
        if branch.is_none() {
            panic!("fatal: your current branch '{branch_name}' does not have any commits yet ");
        }
    }

    let commit_hash = Head::current_commit().await.unwrap().to_string();

    let mut reachable_commits = get_reachable_commits(commit_hash.clone()).await;
    // default sort with signature time
    reachable_commits.sort_by(|a, b| b.committer.timestamp.cmp(&a.committer.timestamp));
    //the last seven commits
    let max_output_number = min(6, reachable_commits.len());
    let mut output_number = 6;
    for commit in reachable_commits.iter().take(max_output_number) {
        let msg = commit.message.trim_start_matches('\n');
        assert_eq!(msg, format!("Commit_{output_number}"));
        output_number -= 1;
    }
}

/// create a test commit tree structure as graph and create branch (master) head to commit 6
/// return a commit hash of commit 6
///            3 --  6
///          /      /
///    1 -- 2  --  5
//           \   /   \
///            4     7
async fn create_test_commit_tree() -> String {
    let mut commit_1 = Commit::from_tree_id(
        ObjectHash::new(&[1; 20]),
        vec![],
        &format_commit_msg("Commit_1", None),
    );
    commit_1.committer.timestamp = 1;
    // save_object(&commit_1);
    save_object(&commit_1, &commit_1.id).unwrap();

    let mut commit_2 = Commit::from_tree_id(
        ObjectHash::new(&[2; 20]),
        vec![commit_1.id],
        &format_commit_msg("Commit_2", None),
    );
    commit_2.committer.timestamp = 2;
    save_object(&commit_2, &commit_2.id).unwrap();

    let mut commit_3 = Commit::from_tree_id(
        ObjectHash::new(&[3; 20]),
        vec![commit_2.id],
        &format_commit_msg("Commit_3", None),
    );
    commit_3.committer.timestamp = 3;
    save_object(&commit_3, &commit_3.id).unwrap();

    let mut commit_4 = Commit::from_tree_id(
        ObjectHash::new(&[4; 20]),
        vec![commit_2.id],
        &format_commit_msg("Commit_4", None),
    );
    commit_4.committer.timestamp = 4;
    save_object(&commit_4, &commit_4.id).unwrap();

    let mut commit_5 = Commit::from_tree_id(
        ObjectHash::new(&[5; 20]),
        vec![commit_2.id, commit_4.id],
        &format_commit_msg("Commit_5", None),
    );
    commit_5.committer.timestamp = 5;
    save_object(&commit_5, &commit_5.id).unwrap();

    let mut commit_6 = Commit::from_tree_id(
        ObjectHash::new(&[6; 20]),
        vec![commit_3.id, commit_5.id],
        &format_commit_msg("Commit_6", None),
    );
    commit_6.committer.timestamp = 6;
    save_object(&commit_6, &commit_6.id).unwrap();

    let mut commit_7 = Commit::from_tree_id(
        ObjectHash::new(&[7; 20]),
        vec![commit_5.id],
        &format_commit_msg("Commit_7", None),
    );
    commit_7.committer.timestamp = 7;
    save_object(&commit_7, &commit_7.id).unwrap();

    // set current branch head to commit 6
    let head = Head::current().await;
    let branch_name = match head {
        Head::Branch(name) => name,
        _ => panic!("should be branch"),
    };

    Branch::update_branch(&branch_name, &commit_6.id.to_string(), None).await;

    commit_6.id.to_string()
}

#[tokio::test]
#[serial]
/// Tests log command with --oneline parameter
async fn test_log_oneline() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create test commits
    let commit_id = create_test_commit_tree().await;
    let reachable_commits = get_reachable_commits(commit_id).await;

    // Test oneline format
    let args = LogArgs::try_parse_from(["libra", "--number", "3", "--oneline"]);

    // Since execute function writes to stdout, we'll test the logic directly
    let mut sorted_commits = reachable_commits.clone();
    sorted_commits.sort_by(|a, b| b.committer.timestamp.cmp(&a.committer.timestamp));

    let max_commits = std::cmp::min(
        args.unwrap().number.unwrap_or(usize::MAX),
        sorted_commits.len(),
    );

    for (i, commit) in sorted_commits.iter().take(max_commits).enumerate() {
        // Test short hash format (should be 7 characters)
        let short_hash = &commit.id.to_string()[..7];
        assert_eq!(short_hash.len(), 7);

        // Test that commit message parsing works
        let (msg, _) = libra::common_utils::parse_commit_msg(&commit.message);
        assert!(!msg.is_empty());

        // For our test commits, verify the expected format
        let expected_number = 6 - i; // commits are numbered 6, 5, 4, 3, 2, 1
        assert_eq!(msg.trim(), format!("Commit_{expected_number}"));
    }
}

#[tokio::test]
#[serial]
/// Tests log -p (patch) without pathspec: create A -> commit -> create B -> commit -> assert diffs contain both A and B contents
async fn test_log_patch_no_pathspec() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create file A and commit
    test::ensure_file("A.txt", Some("Content A\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("A.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Add A".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
    })
    .await;

    // Create file B and commit
    test::ensure_file("B.txt", Some("Content B\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("B.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Add B".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
    })
    .await;

    let bin_dir = temp_path.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let out_file = temp_path.path().join("less_out.txt");

    // On Windows we inline diff generation to avoid relying on spawned pager
    if cfg!(windows) {
        let diffs = collect_combined_diff_for_commits(2, Vec::new()).await;
        assert!(
            diffs.contains("Content A"),
            "patch should contain A content, got: {}",
            diffs
        );
        assert!(
            diffs.contains("Content B"),
            "patch should contain B content, got: {}",
            diffs
        );
    } else {
        // Unix: create shell script that writes stdin to file
        let less_path = bin_dir.join("less");
        let script = format!("#!/bin/sh\ncat - > \"{}\"\n", out_file.display());
        std::fs::write(&less_path, script.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&less_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Set PATH and run
        let old_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", bin_dir.display(), old_path);
        unsafe {
            std::env::set_var("PATH", &new_path);
        }

        let args = LogArgs::try_parse_from(["libra", "--number", "2", "-p"]).unwrap();
        libra::command::log::execute(args).await;

        unsafe {
            // Restore PATH
            std::env::set_var("PATH", old_path);
        }

        let combined_out = std::fs::read_to_string(&out_file).unwrap_or_default();
        assert!(
            combined_out.contains("Content A"),
            "patch should contain A content, got: {}",
            combined_out
        );
        assert!(
            combined_out.contains("Content B"),
            "patch should contain B content, got: {}",
            combined_out
        );
    }
}

#[tokio::test]
#[serial]
/// Tests log -p with a specific pathspec: commit contains A and B, but log -p A should only include A
async fn test_log_patch_with_pathspec() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create files A and B and commit both in one commit
    test::ensure_file("A.txt", Some("Content A\n"));
    test::ensure_file("B.txt", Some("Content B\n"));

    add::execute(AddArgs {
        pathspec: vec![String::from(".")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("Add A and B".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
    })
    .await;

    let bin_dir = temp_path.path().join("bin2");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let out_file = temp_path.path().join("less_out_pathspec.txt");

    if cfg!(windows) {
        let paths = vec![util::to_workdir_path("A.txt")];
        let diffs = collect_combined_diff_for_commits(1, paths).await;
        assert!(
            diffs.contains("Content A"),
            "patch should contain A content, got: {}",
            diffs
        );
        assert!(
            !diffs.contains("Content B"),
            "patch should not contain B content when pathspec is A, got: {}",
            diffs
        );
    } else {
        let less_path = bin_dir.join("less");
        let script = format!("#!/bin/sh\ncat - > \"{}\"\n", out_file.display());
        std::fs::write(&less_path, script.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&less_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let old_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", bin_dir.display(), old_path);
        unsafe {
            std::env::set_var("PATH", &new_path);
        }

        let args = LogArgs::try_parse_from(["libra", "-p", "A.txt"]).unwrap();
        libra::command::log::execute(args).await;

        unsafe {
            std::env::set_var("PATH", old_path);
        }

        let out = std::fs::read_to_string(out_file).unwrap_or_default();
        assert!(
            out.contains("Content A"),
            "patch should contain A content, got: {}",
            out
        );
        assert!(
            !out.contains("Content B"),
            "patch should not contain B content when pathspec is A, got: {}",
            out
        );
    }
}

async fn collect_combined_diff_for_commits(count: usize, paths: Vec<std::path::PathBuf>) -> String {
    // Get head commit and reachable commits
    let commit_hash = Head::current_commit().await.unwrap().to_string();
    let mut reachable_commits = get_reachable_commits(commit_hash).await;
    reachable_commits.sort_by(|a, b| b.committer.timestamp.cmp(&a.committer.timestamp));

    let max_output_number = std::cmp::min(count, reachable_commits.len());
    let mut out = String::new();
    for commit in reachable_commits.into_iter().take(max_output_number) {
        let tree = load_object::<Tree>(&commit.tree_id).unwrap();
        let new_blobs: Vec<(std::path::PathBuf, ObjectHash)> = tree.get_plain_items();

        let old_blobs: Vec<(std::path::PathBuf, ObjectHash)> =
            if !commit.parent_commit_ids.is_empty() {
                let parent = &commit.parent_commit_ids[0];
                let parent_hash = ObjectHash::from_str(&parent.to_string()).unwrap();
                let parent_commit = load_object::<Commit>(&parent_hash).unwrap();
                let parent_tree = load_object::<Tree>(&parent_commit.tree_id).unwrap();
                parent_tree.get_plain_items()
            } else {
                Vec::new()
            };

        let read_content =
            |file: &std::path::PathBuf, hash: &ObjectHash| match load_object::<Blob>(hash) {
                Ok(blob) => blob.data,
                Err(_) => {
                    let file = util::to_workdir_path(file);
                    std::fs::read(&file).unwrap()
                }
            };

        let diffs = Diff::diff(
            old_blobs,
            new_blobs,
            paths.clone().into_iter().collect(),
            read_content,
        );
        for d in diffs {
            out.push_str(&d.data);
        }
    }
    out
}

#[tokio::test]
#[serial]
async fn test_log_stat() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    test::ensure_file("file1.txt", Some("line1\nline2\nline3\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Add file1".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
    })
    .await;

    test::ensure_file("file2.txt", Some("content A\ncontent B\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("file2.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Add file2".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
    })
    .await;

    let commit_hash = Head::current_commit().await.unwrap().to_string();
    let commit_id = ObjectHash::from_str(&commit_hash).unwrap();
    let commit = load_object::<Commit>(&commit_id).unwrap();

    let stats = libra::command::log::compute_commit_stat(&commit, Vec::new()).await;

    assert!(!stats.is_empty());
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].path, "file2.txt");
    assert_eq!(stats[0].insertions, 2);
    assert_eq!(stats[0].deletions, 0);

    let stat_output = libra::command::log::format_stat_output(&stats);
    assert!(stat_output.contains("file2.txt"));
    assert!(stat_output.contains("2"));
    assert!(stat_output.contains("1 file"));
    assert!(stat_output.contains("2 insertion"));
}

#[tokio::test]
#[serial]
async fn test_log_stat_with_modifications() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    test::ensure_file("test.txt", Some("line1\nline2\nline3\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("test.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Initial commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
    })
    .await;

    test::ensure_file("test.txt", Some("line1\nline2 modified\nline3\nline4\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("test.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Modify test.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
    })
    .await;

    let commit_hash = Head::current_commit().await.unwrap().to_string();
    let commit_id = ObjectHash::from_str(&commit_hash).unwrap();
    let commit = load_object::<Commit>(&commit_id).unwrap();

    let stats = libra::command::log::compute_commit_stat(&commit, Vec::new()).await;

    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].path, "test.txt");
    assert_eq!(stats[0].insertions, 2);
    assert_eq!(stats[0].deletions, 1);
}

#[tokio::test]
#[serial]
/// Tests log command with commit hash abbreviation parameters
async fn test_log_abbrev_params() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create test commits
    let commit_id = create_test_commit_tree().await;
    let reachable_commits = get_reachable_commits(commit_id).await;

    // Get the minimum unique hash length calculated by the log command
    let len = libra::utils::util::get_min_unique_hash_length(&reachable_commits);

    // Test with a single commit for consistency
    let commit = reachable_commits.first().unwrap();
    let commit_str = commit.id.to_string();
    let full_hash = commit_str.clone();
    // Extract the full hash length for subsequent oversized-abbreviation boundary tests
    let full_hash_len = full_hash.len();
    // Define an abbreviation length much larger than the hash (e.g., +1000) to simulate an extreme edge case
    let oversized_abbrev = full_hash_len + 1000;

    // Helper function to run log command and get the output
    let run_log_command = |args: &[&str]| -> String {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_libra"))
            .arg("log")
            .args(args)
            .output()
            .expect("Failed to execute log command");
        assert!(
            output.status.success(),
            "Log command failed with stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("Failed to parse log output")
    };

    // Helper function to extract the commit hash from log output
    let extract_commit_hash = |output: &str, oneline: bool| -> String {
        if oneline {
            // Oneline format: "hash message"
            output.split_whitespace().next().unwrap().to_string()
        } else {
            // Non-oneline format: "commit hash"
            output
                .lines()
                .find(|line| line.starts_with("commit "))
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string()
        }
    };

    let oneline_abbrev_over_len = format!("--abbrev={}", full_hash_len + 1);
    let oneline_abbrev_oversized = format!("--abbrev={}", oversized_abbrev);

    let non_oneline_abbrev_over_len = format!("--abbrev={}", full_hash_len + 1);
    let non_oneline_abbrev_oversized = format!("--abbrev={}", oversized_abbrev);

    // Test cases for oneline format
    let oneline_test_cases = vec![
        // (args, expected_hash_length)
        (vec!["--oneline"], len), // Default oneline uses min unique length
        (vec!["--oneline", "--abbrev=0"], 7), // oneline with abbrev=0 uses default 7
        (vec!["--oneline", "--abbrev=5"], 5), // oneline with abbrev=5 uses 5 characters
        (vec!["--oneline", "--no-abbrev-commit"], full_hash_len), // oneline with no_abbrev_commit uses full hash
        (vec!["--oneline", &oneline_abbrev_over_len], full_hash_len),
        (vec!["--oneline", &oneline_abbrev_oversized], full_hash_len),
    ];

    // Test oneline format cases
    for (args, expected_len) in oneline_test_cases {
        let output = run_log_command(&args);
        let hash = extract_commit_hash(&output, true);
        assert_eq!(
            hash.len(),
            expected_len,
            "Failed oneline test with args: {:?}, got hash: '{}' (length: {}), expected length: {}",
            args,
            hash,
            hash.len(),
            expected_len
        );
        // Also verify it's a prefix of the full hash
        assert!(
            commit_str.starts_with(&hash),
            "Hash '{}' is not a prefix of full hash '{}'",
            hash,
            commit_str
        );
    }

    // Test cases for non-oneline format
    let non_oneline_test_cases = vec![
        // (args, expected_hash_length)
        (vec![], full_hash_len),        // Default non-oneline uses full hash
        (vec!["--abbrev-commit"], len), // non-oneline with abbrev_commit uses min unique length
        (vec!["--abbrev-commit", "--abbrev=3"], 3), // non-oneline with abbrev_commit and abbrev=3 uses 3 characters
        (vec!["--abbrev-commit", "--no-abbrev-commit"], full_hash_len), // non-oneline with both uses full hash
        (
            vec!["--abbrev-commit", &non_oneline_abbrev_over_len],
            full_hash_len,
        ),
        (
            vec!["--abbrev-commit", &non_oneline_abbrev_oversized],
            full_hash_len,
        ),
    ];

    // Test non-oneline format cases
    for (args, expected_len) in non_oneline_test_cases {
        let output = run_log_command(&args);
        let hash = extract_commit_hash(&output, false);
        assert_eq!(
            hash.len(),
            expected_len,
            "Failed non-oneline test with args: {:?}, got hash: '{}' (length: {}), expected length: {}",
            args,
            hash,
            hash.len(),
            expected_len
        );
        // Also verify it's a prefix of the full hash
        assert!(
            commit_str.starts_with(&hash),
            "Hash '{}' is not a prefix of full hash '{}'",
            hash,
            commit_str
        );
    }
}

#[tokio::test]
#[serial]
async fn test_log_graph() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let commit_id = create_test_commit_tree().await;

    let args = LogArgs::try_parse_from(["libra", "--number", "6", "--graph"]).unwrap();
    assert!(args.graph);

    let mut graph_state = libra::command::log::GraphState::new();

    let commit_hash = ObjectHash::from_str(&commit_id).unwrap();
    let commit = load_object::<Commit>(&commit_hash).unwrap();

    let prefix = graph_state.render(&commit);
    assert!(!prefix.is_empty());
    assert!(prefix.contains('*'));
}

#[tokio::test]
#[serial]
async fn test_log_graph_simple_chain() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    test::ensure_file("file1.txt", Some("content1\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("file1.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("First commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
    })
    .await;

    test::ensure_file("file2.txt", Some("content2\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("file2.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Second commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
    })
    .await;

    let commit_hash = Head::current_commit().await.unwrap().to_string();
    let reachable_commits = get_reachable_commits(commit_hash).await;

    let mut graph_state = libra::command::log::GraphState::new();

    for commit in reachable_commits.iter().take(2) {
        let prefix = graph_state.render(commit);
        assert!(prefix.starts_with("* ") || prefix.contains("* "));
    }
}

#[tokio::test]
#[serial]
async fn test_log_stat_and_graph_combined() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    test::ensure_file("combo.txt", Some("line1\nline2\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("combo.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Add combo file".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
    })
    .await;

    let args = LogArgs::try_parse_from(["libra", "--graph", "--stat"]).unwrap();
    assert!(args.graph);
    assert!(args.stat);

    let commit_hash = Head::current_commit().await.unwrap().to_string();
    let commit_id = ObjectHash::from_str(&commit_hash).unwrap();
    let commit = load_object::<Commit>(&commit_id).unwrap();

    let stats = libra::command::log::compute_commit_stat(&commit, Vec::new()).await;
    assert_eq!(stats.len(), 1);

    let mut graph_state = libra::command::log::GraphState::new();
    let prefix = graph_state.render(&commit);
    assert!(!prefix.is_empty());
}
