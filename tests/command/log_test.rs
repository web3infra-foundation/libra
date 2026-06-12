//! Tests log command output ordering and formatting of commit history.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{cmp::min, str::FromStr};

use clap::Parser;
use git_internal::{
    Diff,
    hash::ObjectHash,
    internal::object::{blob::Blob, commit::Commit, tree::Tree},
};
use libra::{
    internal::{db::get_db_conn_instance, model::reference},
    utils::{object_ext::TreeExt, output::OutputConfig, pager::LIBRA_PAGER_ENV, util},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serial_test::serial;

use super::*;

#[test]
fn test_log_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();

    let output = run_libra_command(&["log", "--oneline"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn test_log_cli_empty_repository_returns_fatal_128() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["log", "--oneline"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert_eq!(
        report.message,
        "your current branch 'main' does not have any commits yet"
    );
    assert!(
        report
            .hints
            .iter()
            .any(|hint| hint == "create a commit first before running 'libra log'."),
        "missing log hint: {:?}",
        report.hints
    );
    assert_eq!(
        stderr,
        "fatal: your current branch 'main' does not have any commits yet\nError-Code: LBR-REPO-003\n\nHint: create a commit first before running 'libra log'."
    );
}

#[tokio::test]
#[serial]
async fn test_log_corrupt_head_reference_returns_repo_corrupt() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());

    let db = get_db_conn_instance().await;
    let head = reference::Entity::find()
        .filter(reference::Column::Kind.eq(reference::ConfigKind::Head))
        .filter(reference::Column::Remote.is_null())
        .one(&db)
        .await
        .unwrap()
        .expect("expected HEAD row");
    let mut head: reference::ActiveModel = head.into();
    head.name = Set(None);
    head.commit = Set(Some("not-a-valid-hash".to_string()));
    head.update(&db).await.unwrap();

    let output = run_libra_command(&["log", "--oneline"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("failed to resolve HEAD"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        stderr.contains("invalid detached HEAD commit hash"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn test_log_json_output_includes_commit_list() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "log", "-n", "1"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "log");
    assert_eq!(json["data"]["commits"][0]["subject"], "base");
    assert!(json["data"]["commits"][0]["files"].as_array().is_some());
}

#[tokio::test]
#[serial]
async fn test_log_quiet_does_not_initialize_pager() {
    if cfg!(windows) {
        return;
    }

    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let missing_bin_dir = tempdir().unwrap();
    let _path = test::ScopedEnvVar::set("PATH", missing_bin_dir.path());
    let _pager = test::ScopedEnvVar::set(LIBRA_PAGER_ENV, "always");

    let args = LogArgs::try_parse_from(["libra", "--oneline"]).unwrap();
    let output = OutputConfig {
        quiet: true,
        ..OutputConfig::default()
    };

    let result = libra::command::log::execute_safe(args, &output).await;
    assert!(
        result.is_ok(),
        "quiet log should not initialize pager: {result:?}"
    );
}

#[test]
fn test_log_invalid_since_uses_command_usage_error() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["log", "--since", "not-a-date"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.starts_with("error: "));
    assert!(stderr.contains("supported formats: YYYY-MM-DD"));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert_eq!(report.category, "cli");
    assert_eq!(report.exit_code, 129);
    assert_eq!(report.severity, "error");
}

#[test]
fn test_log_invalid_decorate_uses_command_usage_error() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "log", "--decorate=bogus"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
    assert!(stderr.is_empty(), "unexpected human stderr: {stderr}");
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert_eq!(report.category, "cli");
    assert_eq!(report.exit_code, 129);
    assert_eq!(report.severity, "error");
    assert_eq!(report.message, "invalid --decorate option: bogus");
    assert_eq!(report.hints, vec!["valid options: no, short, full, auto"]);
}

fn assert_log_rejects_escaping_pathspec(pathspec: &str) {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["log", pathspec], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(129),
        "escaping pathspec should be usage error, stderr: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "escaping pathspec must not produce log output"
    );
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report.message.contains("pathspec"),
        "message should identify pathspec problem: {}",
        report.message
    );
}

#[test]
fn test_log_pathspec_rejects_parent_escape() {
    assert_log_rejects_escaping_pathspec("../outside.txt");
}

#[test]
fn test_log_pathspec_rejects_absolute_path() {
    let outside = tempdir().unwrap();
    let path = outside.path().join("outside.txt");
    assert_log_rejects_escaping_pathspec(&path.to_string_lossy());
}

#[test]
fn test_log_pathspec_rejects_windows_parent_separator() {
    assert_log_rejects_escaping_pathspec(r"..\outside.txt");
}

#[tokio::test]
#[serial]
async fn test_log_decorate_no_skips_corrupt_reference_map() {
    let repo = create_committed_repo_via_cli();

    let create_branch = run_libra_command(&["branch", "topic"], repo.path());
    assert!(
        create_branch.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&create_branch.stderr)
    );

    let _guard = ChangeDirGuard::new(repo.path());
    let db = get_db_conn_instance().await;
    let topic = reference::Entity::find()
        .filter(reference::Column::Kind.eq(reference::ConfigKind::Branch))
        .filter(reference::Column::Name.eq("topic"))
        .filter(reference::Column::Remote.is_null())
        .one(&db)
        .await
        .unwrap()
        .expect("expected topic branch row");
    let mut topic: reference::ActiveModel = topic.into();
    topic.commit = Set(Some("not-a-valid-hash".to_string()));
    topic.update(&db).await.unwrap();

    let output = run_libra_command(&["log", "--decorate=no", "--oneline"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("base"),
        "expected log output to remain available, got: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_log_patch_fails_when_commit_blob_is_missing() {
    let repo = create_committed_repo_via_cli();

    let tracked_blob = {
        let _guard = ChangeDirGuard::new(repo.path());
        let head = Head::current_commit().await.expect("expected HEAD commit");
        let commit: Commit = load_object(&head).expect("expected HEAD commit object");
        let tree: Tree = load_object(&commit.tree_id).expect("expected HEAD tree");
        tree.get_plain_items()
            .into_iter()
            .find(|(path, _)| path == &std::path::PathBuf::from("tracked.txt"))
            .map(|(_, hash)| hash.to_string())
            .expect("expected tracked.txt blob in HEAD tree")
    };
    std::fs::remove_file(loose_object_path(repo.path(), &tracked_blob))
        .expect("failed to delete committed blob");
    std::fs::write(
        repo.path().join("tracked.txt"),
        "mutated worktree fallback\n",
    )
    .expect("failed to mutate worktree file");

    let output = run_libra_command(&["log", "-n", "1", "--patch"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("failed to load blob object"),
        "expected repo corruption error, got: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_log_quiet_patch_fails_when_commit_blob_is_missing() {
    let repo = create_committed_repo_via_cli();

    let tracked_blob = {
        let _guard = ChangeDirGuard::new(repo.path());
        let head = Head::current_commit().await.expect("expected HEAD commit");
        let commit: Commit = load_object(&head).expect("expected HEAD commit object");
        let tree: Tree = load_object(&commit.tree_id).expect("expected HEAD tree");
        tree.get_plain_items()
            .into_iter()
            .find(|(path, _)| path == &std::path::PathBuf::from("tracked.txt"))
            .map(|(_, hash)| hash.to_string())
            .expect("expected tracked.txt blob in HEAD tree")
    };
    std::fs::remove_file(loose_object_path(repo.path(), &tracked_blob))
        .expect("failed to delete committed blob");
    std::fs::write(
        repo.path().join("tracked.txt"),
        "mutated worktree fallback\n",
    )
    .expect("failed to mutate worktree file");

    let output = run_libra_command(&["--quiet", "log", "-n", "1", "--patch"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("failed to load blob object"),
        "expected repo corruption error, got: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_log_quiet_stat_respects_selected_history_range() {
    let repo = create_committed_repo_via_cli();

    std::fs::write(repo.path().join("tracked.txt"), "tracked\nsecond\n").unwrap();
    let add_second = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert!(
        add_second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_second.stderr)
    );
    let commit_second = run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());
    assert!(
        commit_second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&commit_second.stderr)
    );

    std::fs::write(repo.path().join("tracked.txt"), "tracked\nthird\n").unwrap();
    let add_third = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert!(
        add_third.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_third.stderr)
    );
    let commit_third = run_libra_command(&["commit", "-m", "third", "--no-verify"], repo.path());
    assert!(
        commit_third.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&commit_third.stderr)
    );

    let oldest_blob = {
        let _guard = ChangeDirGuard::new(repo.path());
        let head = Head::current_commit().await.expect("expected HEAD commit");
        let latest: Commit = load_object(&head).expect("expected latest commit");
        let middle_id = latest.parent_commit_ids[0];
        let middle: Commit = load_object(&middle_id).expect("expected middle commit");
        let oldest_id = middle.parent_commit_ids[0];
        let oldest: Commit = load_object(&oldest_id).expect("expected oldest commit");
        let tree: Tree = load_object(&oldest.tree_id).expect("expected oldest tree");
        tree.get_plain_items()
            .into_iter()
            .find(|(path, _)| path == &std::path::PathBuf::from("tracked.txt"))
            .map(|(_, hash)| hash.to_string())
            .expect("expected tracked.txt blob in oldest tree")
    };
    std::fs::remove_file(loose_object_path(repo.path(), &oldest_blob))
        .expect("failed to delete oldest committed blob");
    std::fs::write(
        repo.path().join("tracked.txt"),
        "mutated worktree fallback\n",
    )
    .expect("failed to mutate worktree file");

    let top_only = run_libra_command(&["--quiet", "log", "-n", "1", "--stat"], repo.path());
    assert!(
        top_only.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&top_only.stderr)
    );

    let output = run_libra_command(&["--quiet", "log", "-n", "2", "--stat"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("failed to load blob object"),
        "expected repo corruption error, got: {stderr}"
    );
}

#[test]
fn test_log_json_total_reflects_filtered_scope() {
    let repo = create_committed_repo_via_cli();

    let name_output = run_libra_command(&["config", "user.name", "Other User"], repo.path());
    assert!(
        name_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&name_output.stderr)
    );
    let email_output =
        run_libra_command(&["config", "user.email", "other@example.com"], repo.path());
    assert!(
        email_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&email_output.stderr)
    );

    std::fs::write(
        repo.path().join("tracked.txt"),
        "tracked\nupdated by other\n",
    )
    .expect("failed to update tracked.txt");
    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert!(
        add_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_output.stderr)
    );
    let commit_output = run_libra_command(
        &["commit", "-m", "other update", "--no-verify"],
        repo.path(),
    );
    assert!(
        commit_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&commit_output.stderr)
    );

    let output = run_libra_command(&["--json", "log", "--author", "Other User"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "log");
    assert_eq!(json["data"]["total"], 1);
    let commits = json["data"]["commits"]
        .as_array()
        .expect("commits should be an array");
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0]["author_name"], "Other User");
    assert_eq!(commits[0]["subject"], "other update");
}

#[tokio::test]
#[serial]
/// Tests retrieval of commits reachable from a specific commit hash
async fn test_get_reachable_commits() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let commit_id = create_test_commit_tree().await;

    let reachable_commits = get_reachable_commits(commit_id, None).await.unwrap();
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
        // Migrated from `Branch::find_branch` (lossy wrapper) to
        // `Branch::find_branch_result` per `docs/improvement/branch.md` —
        // storage errors no longer silently degrade to "no commits yet".
        match Branch::find_branch_result(&branch_name, None).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                panic!("fatal: your current branch '{branch_name}' does not have any commits yet ")
            }
            Err(err) => {
                panic!("fatal: failed to query branch '{branch_name}': {err:?}")
            }
        }
    }

    let commit_hash = Head::current_commit().await.unwrap().to_string();

    let mut reachable_commits = get_reachable_commits(commit_hash.clone(), None)
        .await
        .unwrap();
    // newest first
    reachable_commits.sort_by_key(|c| std::cmp::Reverse(c.committer.timestamp));

    //the last seven commits
    let max_output_number = min(6, reachable_commits.len());
    let expected_msgs = [
        "Commit_6", "Commit_5", "Commit_4", "Commit_3", "Commit_2", "Commit_1",
    ];
    for (i, commit) in reachable_commits.iter().take(max_output_number).enumerate() {
        let (msg, _) = libra::common_utils::parse_commit_msg(&commit.message);
        assert_eq!(msg, expected_msgs[i]);
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

    Branch::update_branch(&branch_name, &commit_6.id.to_string(), None)
        .await
        .unwrap();

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
    let reachable_commits = get_reachable_commits(commit_id, None).await.unwrap();

    // Test oneline format
    let args = LogArgs::try_parse_from(["libra", "--number", "3", "--oneline"]);

    // Since execute function writes to stdout, we'll test the logic directly
    let mut sorted_commits = reachable_commits.clone();
    sorted_commits.sort_by_key(|c| std::cmp::Reverse(c.committer.timestamp));

    let max_commits = std::cmp::min(
        args.unwrap().number.unwrap_or(usize::MAX),
        sorted_commits.len(),
    );

    let expected_msgs = ["Commit_6", "Commit_5", "Commit_4"];
    for (i, commit) in sorted_commits.iter().take(max_commits).enumerate() {
        // Test short hash format (should be 7 characters)
        let short_hash = &commit.id.to_string()[..7];
        assert_eq!(short_hash.len(), 7);

        // Test that commit message parsing works
        let (msg, _) = libra::common_utils::parse_commit_msg(&commit.message);
        assert!(!msg.is_empty());

        // For our test commits, verify the expected format
        assert_eq!(msg.trim(), expected_msgs[i]);
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
        ..Default::default()
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
        author: None,
        ..Default::default()
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
        ..Default::default()
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
        author: None,
        ..Default::default()
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
        let _path = test::ScopedEnvVar::set("PATH", &new_path);
        let _pager = test::ScopedEnvVar::set(LIBRA_PAGER_ENV, "always");

        let args = LogArgs::try_parse_from(["libra", "--number", "2", "-p"]).unwrap();
        libra::command::log::execute(args).await;

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
        ..Default::default()
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
        author: None,
        ..Default::default()
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
        let _path = test::ScopedEnvVar::set("PATH", &new_path);
        let _pager = test::ScopedEnvVar::set(LIBRA_PAGER_ENV, "always");

        let args = LogArgs::try_parse_from(["libra", "-p", "A.txt"]).unwrap();
        libra::command::log::execute(args).await;

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
    let reachable_commits = get_reachable_commits(commit_hash, None).await.unwrap();

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
        ..Default::default()
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
        author: None,
        ..Default::default()
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
        ..Default::default()
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
        author: None,
        ..Default::default()
    })
    .await;

    let commit_hash = Head::current_commit().await.unwrap().to_string();
    let commit_id = ObjectHash::from_str(&commit_hash).unwrap();
    let commit = load_object::<Commit>(&commit_id).unwrap();

    let stats = libra::command::log::compute_commit_stat(&commit, Vec::new())
        .await
        .unwrap();

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
        ..Default::default()
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
        author: None,
        ..Default::default()
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
        ..Default::default()
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
        author: None,
        ..Default::default()
    })
    .await;

    let commit_hash = Head::current_commit().await.unwrap().to_string();
    let commit_id = ObjectHash::from_str(&commit_hash).unwrap();
    let commit = load_object::<Commit>(&commit_id).unwrap();

    let stats = libra::command::log::compute_commit_stat(&commit, Vec::new())
        .await
        .unwrap();

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
    let reachable_commits = get_reachable_commits(commit_id, None).await.unwrap();

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
        ..Default::default()
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
        author: None,
        ..Default::default()
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
        ..Default::default()
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
        author: None,
        ..Default::default()
    })
    .await;

    let commit_hash = Head::current_commit().await.unwrap().to_string();
    let reachable_commits = get_reachable_commits(commit_hash, None).await.unwrap();

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
        ..Default::default()
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
        author: None,
        ..Default::default()
    })
    .await;

    let args = LogArgs::try_parse_from(["libra", "--graph", "--stat"]).unwrap();
    assert!(args.graph);
    assert!(args.stat);

    let commit_hash = Head::current_commit().await.unwrap().to_string();
    let commit_id = ObjectHash::from_str(&commit_hash).unwrap();
    let commit = load_object::<Commit>(&commit_id).unwrap();

    let stats = libra::command::log::compute_commit_stat(&commit, Vec::new())
        .await
        .unwrap();
    assert_eq!(stats.len(), 1);

    let mut graph_state = libra::command::log::GraphState::new();
    let prefix = graph_state.render(&commit);
    assert!(!prefix.is_empty());
}

fn run_log_cmd(args: &[&str], cwd: &std::path::Path) -> (std::process::ExitStatus, String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(cwd)
        .arg("log")
        .args(args)
        .output()
        .expect("Failed to execute log command");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status, stdout, stderr)
}

fn run_libra_cmd(
    args: &[&str],
    cwd: &std::path::Path,
) -> (std::process::ExitStatus, String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("Failed to execute libra command");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (output.status, stdout, stderr)
}

fn count_commit_lines(output: &str) -> usize {
    output.lines().filter(|l| l.starts_with("commit ")).count()
}

#[tokio::test]
#[serial]
async fn test_log_short_number_flag_equivalent_to_number() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let _ = create_test_commit_tree().await;

    let (status_short, out_short, err_short) = run_log_cmd(&["-2"], temp_path.path());
    assert!(status_short.success(), "log -2 failed: {err_short}");

    let (status_long, out_long, err_long) = run_log_cmd(&["-n", "2"], temp_path.path());
    assert!(status_long.success(), "log -n 2 failed: {err_long}");

    let short_count = count_commit_lines(&out_short);
    let long_count = count_commit_lines(&out_long);

    assert_eq!(short_count, 2);
    assert_eq!(long_count, 2);
    assert_eq!(short_count, long_count);
}

#[tokio::test]
#[serial]
async fn test_log_short_number_flag_multi_digit() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let _ = create_test_commit_tree().await;

    let (status_long, out_long, err_long) = run_log_cmd(&["-n", "10"], temp_path.path());
    assert!(status_long.success(), "log -n 10 failed: {err_long}");

    let expected_count = count_commit_lines(&out_long);

    let (status_short, out_short, err_short) = run_log_cmd(&["-10"], temp_path.path());
    assert!(status_short.success(), "log -10 failed: {err_short}");

    let short_count = count_commit_lines(&out_short);
    assert_eq!(short_count, expected_count);
}

#[tokio::test]
#[serial]
/// Ensure `log -- -2` treats `-2` as a pathspec, not as `-n 2`.
async fn test_log_double_dash_disables_short_number_rewrite() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Commit a normal file first.
    test::ensure_file("a.txt", Some("A\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("a.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Add a".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;

    // Commit a file named "-2" to validate pathspec handling.
    test::ensure_file("-2", Some("dash\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("-2")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("Add dash".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;

    let (status, out, err) = run_log_cmd(&["--", "-2"], temp_path.path());
    assert!(status.success(), "log -- -2 failed: {err}");

    // Only the commit touching "-2" should be listed.
    assert_eq!(count_commit_lines(&out), 1);
    assert!(out.contains("Add dash"));
}

#[tokio::test]
#[serial]
/// Ensure `log` rewrite does not trigger when `log` is a positional path for another subcommand.
async fn test_add_with_log_path_does_not_trigger_log_rewrite() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create files named "log" and "-2".
    test::ensure_file("log", Some("logfile\n"));
    test::ensure_file("-2", Some("dashfile\n"));

    let (status_add, _out_add, err_add) =
        run_libra_cmd(&["add", "log", "--", "-2"], temp_path.path());
    assert!(status_add.success(), "add failed: {err_add}");

    let (status_status, out_status, err_status) =
        run_libra_cmd(&["status", "--porcelain"], temp_path.path());
    assert!(
        status_status.success(),
        "status --porcelain failed: {err_status}"
    );

    // Both files should be staged (porcelain v1 uses "A  <path>").
    assert!(out_status.lines().any(|l| l == "A  log"));
    assert!(out_status.lines().any(|l| l == "A  -2"));
}

#[tokio::test]
#[serial]
/// Ensure `libra -- log -2` treats `log` as the subcommand and rewrites `-2` correctly.
async fn test_log_short_number_flag_with_double_dash_before_subcommand() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let _ = create_test_commit_tree().await;

    let (status, out, err) = run_libra_cmd(&["--", "log", "-2"], temp_path.path());
    assert!(status.success(), "libra -- log -2 failed: {err}");
    assert_eq!(count_commit_lines(&out), 2);
}

#[test]
fn test_log_machine_output_is_single_line_json() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--machine", "log", "-n", "1"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let non_empty_lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        non_empty_lines.len(),
        1,
        "machine output should be exactly one non-empty line, got: {stdout}"
    );

    let parsed: serde_json::Value =
        serde_json::from_str(non_empty_lines[0]).expect("machine output should be valid JSON");
    assert_eq!(parsed["command"], "log");
    assert!(parsed["data"]["commits"].as_array().is_some());
}

#[test]
fn test_log_json_root_commit_has_empty_parents_and_added_files() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "log", "-n", "1"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    let commit = &json["data"]["commits"][0];

    // Root commit has no parents.
    let parents = commit["parents"]
        .as_array()
        .expect("parents should be an array");
    assert!(parents.is_empty(), "root commit should have no parents");

    // Root commit files should all be "added".
    let files = commit["files"]
        .as_array()
        .expect("files should be an array");
    assert!(
        !files.is_empty(),
        "root commit should have at least one file"
    );
    for file in files {
        assert_eq!(
            file["status"], "added",
            "root commit files should all be 'added', got: {}",
            file["status"]
        );
    }
}

#[test]
fn test_log_json_since_filter_restricts_results() {
    let repo = create_committed_repo_via_cli();

    // The committed repo has one commit. Querying with --since far in the future
    // should return zero commits.
    let output = run_libra_command(&["--json", "log", "--since", "2099-01-01"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    let commits = json["data"]["commits"]
        .as_array()
        .expect("commits should be an array");
    assert!(
        commits.is_empty(),
        "no commits should match a future --since date"
    );
}

#[test]
fn test_log_json_oneline_flag_does_not_alter_schema() {
    let repo = create_committed_repo_via_cli();

    let plain = run_libra_command(&["--json", "log", "-n", "1"], repo.path());
    let with_oneline = run_libra_command(&["--json", "log", "-n", "1", "--oneline"], repo.path());
    assert!(plain.status.success());
    assert!(with_oneline.status.success());

    let plain_json = parse_json_stdout(&plain);
    let oneline_json = parse_json_stdout(&with_oneline);

    // JSON schema should be identical regardless of --oneline.
    assert_eq!(
        plain_json["data"]["commits"][0]["hash"],
        oneline_json["data"]["commits"][0]["hash"]
    );
    assert_eq!(
        plain_json["data"]["commits"][0]["subject"],
        oneline_json["data"]["commits"][0]["subject"]
    );
    assert_eq!(
        plain_json["data"]["commits"][0]["author_name"],
        oneline_json["data"]["commits"][0]["author_name"]
    );
}

// ============================================================================
// --grep 参数测试
// ============================================================================

// Test grep parameter parsing
#[test]
fn test_log_args_grep() {
    let args = LogArgs::parse_from(["libra", "--grep", "fix"]);
    assert_eq!(args.grep, Some("fix".to_string()));

    let args = LogArgs::parse_from(["libra"]);
    assert_eq!(args.grep, None);
}

// Test grep combined with other arguments
#[test]
fn test_grep_with_other_args() {
    let args = LogArgs::parse_from(["libra", "--grep", "feature", "--oneline", "-n", "5"]);
    assert_eq!(args.grep, Some("feature".to_string()));
    assert!(args.oneline);
    assert_eq!(args.number, Some(5));
}

// Test case-sensitive matching
#[test]
fn test_grep_case_sensitive() {
    let args = LogArgs::parse_from(["libra", "--grep", "FIX"]);
    assert_eq!(args.grep, Some("FIX".to_string()));
}

// Test empty string grep
#[test]
fn test_grep_empty_string() {
    let args = LogArgs::parse_from(["libra", "--grep", ""]);
    assert_eq!(args.grep, Some("".to_string()));
}

// Test graph with grep combination
#[test]
fn test_graph_with_grep() {
    let args = LogArgs::parse_from(["libra", "--graph", "--grep", "fix"]);
    assert!(args.graph);
    assert_eq!(args.grep, Some("fix".to_string()));
}

// Integration test: verify actual filtering behavior
#[tokio::test]
#[serial]
async fn test_log_grep_filtering() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create first commit: fix message
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
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("fix: bug fix".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;

    // Create second commit: feat message
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
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("feat: new feature".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;

    // Create third commit: docs message
    test::ensure_file("file3.txt", Some("content3\n"));
    add::execute(AddArgs {
        pathspec: vec![String::from("file3.txt")],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        ..Default::default()
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("docs: update readme".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: false,
        author: None,
        ..Default::default()
    })
    .await;

    // Test grep "fix" - should only show the fix commit
    let (status, stdout, stderr) = run_log_cmd(&["--grep", "fix"], temp_path.path());
    assert!(status.success(), "log --grep failed: {stderr}");
    assert!(stdout.contains("fix: bug fix"));
    assert!(!stdout.contains("feat: new feature"));
    assert!(!stdout.contains("docs: update readme"));

    // Test grep "feat" - should only show the feat commit
    let (status, stdout, stderr) = run_log_cmd(&["--grep", "feat"], temp_path.path());
    assert!(status.success(), "log --grep failed: {stderr}");
    assert!(stdout.contains("feat: new feature"));
    assert!(!stdout.contains("fix: bug fix"));
    assert!(!stdout.contains("docs: update readme"));

    // Test grep "nonexistent" - should show no commits
    let (status, stdout, stderr) = run_log_cmd(&["--grep", "nonexistent"], temp_path.path());
    assert!(status.success(), "log --grep failed: {stderr}");
    assert!(!stdout.contains("fix: bug fix"));
    assert!(!stdout.contains("feat: new feature"));
    assert!(!stdout.contains("docs: update readme"));
    // With no matches, stdout should be empty
    assert!(stdout.is_empty());

    // Test empty grep pattern - should show all commits
    let (status, stdout, stderr) = run_log_cmd(&["--grep", ""], temp_path.path());
    assert!(status.success(), "log --grep failed: {stderr}");
    assert!(stdout.contains("fix: bug fix"));
    assert!(stdout.contains("feat: new feature"));
    assert!(stdout.contains("docs: update readme"));

    // Test case-sensitive matching - "Fix" should not match "fix"
    let (status, stdout, stderr) = run_log_cmd(&["--grep", "Fix"], temp_path.path());
    assert!(status.success(), "log --grep failed: {stderr}");
    assert!(!stdout.contains("fix: bug fix"));
    assert!(!stdout.contains("feat: new feature"));
    assert!(!stdout.contains("docs: update readme"));
    assert!(stdout.is_empty());

    // Test case-insensitive should not work (we document case-sensitive)
    // but that's the intended behavior

    // Test grep with -n limit
    let (status, stdout, stderr) = run_log_cmd(&["--grep", "fix", "-n", "1"], temp_path.path());
    assert!(status.success(), "log --grep failed: {stderr}");
    // Should show at most 1 commit with "fix"
    let commit_count = count_commit_lines(&stdout);
    assert_eq!(commit_count, 1);
}

// ── Custom pretty-format placeholders (log-improvement-plan Batch 1) ──

#[test]
fn test_pretty_format_full_hash_cli() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["log", "-n", "1", "--pretty=format:%H"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(
        hash.len(),
        40,
        "expected a 40-char SHA-1 hash, got: {hash:?}"
    );
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_pretty_format_in_json_is_noop() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["--json", "log", "-n", "1", "--pretty=format:%H"],
        repo.path(),
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Under --json the pretty template is a no-op: the schema is unchanged.
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "log");
    assert_eq!(json["data"]["commits"][0]["subject"], "base");
}

// ── log filter alignment: regex grep, committer, parents, first-parent
//    (log-improvement-plan Batch 2) ──

/// Make a commit touching `file` with `content` and message `msg`.
fn log_commit(repo: &std::path::Path, file: &str, content: &str, msg: &str) {
    fs::write(repo.join(file), content).expect("write file");
    let add = run_libra_command(&["add", file], repo);
    assert!(
        add.status.success(),
        "add {file} failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let commit = run_libra_command(&["commit", "-m", msg, "--no-verify"], repo);
    assert!(
        commit.status.success(),
        "commit '{msg}' failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );
}

#[test]
fn test_log_grep_regex_matches() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    log_commit(repo.path(), "a.txt", "a", "fix: alpha bug");
    log_commit(repo.path(), "b.txt", "b", "feat: beta feature");

    // Anchored regex: only the message starting with "fix" matches.
    let out = run_libra_command(&["log", "--grep", "^fix", "--oneline"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("fix: alpha bug"), "stdout: {stdout}");
    assert!(!stdout.contains("feat: beta feature"), "stdout: {stdout}");
}

#[test]
fn test_log_grep_ignore_case() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    log_commit(repo.path(), "a.txt", "a", "feat: lower case");

    // Without -i, uppercase pattern does not match; with -i it does.
    let sensitive = run_libra_command(&["log", "--grep", "FEAT", "--oneline"], repo.path());
    assert!(sensitive.status.success());
    assert!(
        !String::from_utf8_lossy(&sensitive.stdout).contains("feat: lower case"),
        "case-sensitive grep must not match"
    );

    let insensitive = run_libra_command(&["log", "--grep", "FEAT", "-i", "--oneline"], repo.path());
    assert!(insensitive.status.success());
    assert!(
        String::from_utf8_lossy(&insensitive.stdout).contains("feat: lower case"),
        "case-insensitive grep must match"
    );
}

#[test]
fn test_log_grep_regex_invalid() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    log_commit(repo.path(), "a.txt", "a", "fix: bug");

    let out = run_libra_command(&["log", "--grep", "(unbalanced"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(129),
        "invalid regex must exit 129, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("LBR-CLI-002"),
        "expected LBR-CLI-002, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn test_log_grep_regex_too_long() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    log_commit(repo.path(), "a.txt", "a", "fix: bug");

    let huge = "a".repeat(5000);
    let out = run_libra_command(&["log", "--grep", &huge], repo.path());
    assert_eq!(
        out.status.code(),
        Some(129),
        "oversized regex must exit 129, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("LBR-CLI-002"));
}

#[test]
fn test_log_committer_filter() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    // First committer: alice.
    run_libra_command(&["config", "user.name", "Alice"], repo.path());
    run_libra_command(&["config", "user.email", "alice@example.com"], repo.path());
    log_commit(repo.path(), "a.txt", "a", "alice work");
    // Second committer: bob.
    run_libra_command(&["config", "user.name", "Bob"], repo.path());
    run_libra_command(&["config", "user.email", "bob@example.com"], repo.path());
    log_commit(repo.path(), "b.txt", "b", "bob work");

    let out = run_libra_command(&["log", "--committer", "alice", "--oneline"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("alice work"), "stdout: {stdout}");
    assert!(!stdout.contains("bob work"), "stdout: {stdout}");
}

/// Build a repo with a real merge commit. main: root -> A; feature: root -> B;
/// then main merges feature, producing a 2-parent commit. Returns the repo.
fn log_repo_with_merge() -> tempfile::TempDir {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    log_commit(repo.path(), "root.txt", "root", "root commit");

    let branch = run_libra_command(&["branch", "feature"], repo.path());
    assert!(
        branch.status.success(),
        "branch: {}",
        String::from_utf8_lossy(&branch.stderr)
    );

    // Advance main.
    log_commit(repo.path(), "main.txt", "main", "on main");

    // Advance feature.
    let sw = run_libra_command(&["switch", "feature"], repo.path());
    assert!(
        sw.status.success(),
        "switch feature: {}",
        String::from_utf8_lossy(&sw.stderr)
    );
    log_commit(repo.path(), "feat.txt", "feat", "on feature");

    // Merge feature into main (true merge — histories diverged).
    let sw_main = run_libra_command(&["switch", "main"], repo.path());
    assert!(
        sw_main.status.success(),
        "switch main: {}",
        String::from_utf8_lossy(&sw_main.stderr)
    );
    let merge = run_libra_command(&["merge", "feature", "-m", "merge feature"], repo.path());
    assert!(
        merge.status.success(),
        "merge: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    repo
}

#[test]
fn test_log_merges_and_no_merges() {
    let repo = log_repo_with_merge();

    let merges = run_libra_command(&["log", "--merges", "--oneline"], repo.path());
    assert!(
        merges.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&merges.stderr)
    );
    let mstdout = String::from_utf8_lossy(&merges.stdout);
    assert!(
        mstdout.contains("merge feature"),
        "--merges should show the merge: {mstdout}"
    );
    assert!(
        !mstdout.contains("on main"),
        "--merges should hide non-merges: {mstdout}"
    );

    let no_merges = run_libra_command(&["log", "--no-merges", "--oneline"], repo.path());
    assert!(no_merges.status.success());
    let nstdout = String::from_utf8_lossy(&no_merges.stdout);
    assert!(
        !nstdout.contains("merge feature"),
        "--no-merges should hide the merge: {nstdout}"
    );
    assert!(
        nstdout.contains("on main"),
        "--no-merges should show non-merges: {nstdout}"
    );
}

#[test]
fn test_log_min_max_parents() {
    let repo = log_repo_with_merge();

    // --min-parents=2 is equivalent to --merges.
    let min2 = run_libra_command(&["log", "--min-parents", "2", "--oneline"], repo.path());
    assert!(min2.status.success());
    let s = String::from_utf8_lossy(&min2.stdout);
    assert!(
        s.contains("merge feature") && !s.contains("on main"),
        "min-parents=2: {s}"
    );

    // --max-parents=1 is equivalent to --no-merges.
    let max1 = run_libra_command(&["log", "--max-parents", "1", "--oneline"], repo.path());
    assert!(max1.status.success());
    let s = String::from_utf8_lossy(&max1.stdout);
    assert!(
        !s.contains("merge feature") && s.contains("on main"),
        "max-parents=1: {s}"
    );
}

#[test]
fn test_log_first_parent() {
    let repo = log_repo_with_merge();

    // First-parent walk from the merge follows main (root -> on main -> merge),
    // never entering the feature side ("on feature").
    let out = run_libra_command(&["log", "--first-parent", "--oneline"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("merge feature"), "stdout: {stdout}");
    assert!(stdout.contains("on main"), "stdout: {stdout}");
    assert!(
        !stdout.contains("on feature"),
        "first-parent must skip the merged branch: {stdout}"
    );
}

#[test]
fn test_log_filters_json_total() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    log_commit(repo.path(), "a.txt", "a", "fix: one");
    log_commit(repo.path(), "b.txt", "b", "feat: two");
    log_commit(repo.path(), "c.txt", "c", "fix: three");

    let out = run_libra_command(&["--json", "log", "--grep", "^fix"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json = parse_json_stdout(&out);
    let commits = json["data"]["commits"].as_array().expect("commits array");
    assert_eq!(commits.len(), 2, "two fix commits expected: {json}");
    // total reflects the filtered scope when no -n is given.
    assert_eq!(
        json["data"]["total"].as_u64(),
        Some(2),
        "total must equal filtered count: {json}"
    );
}

// ── pickaxe -S / -G content filters (log-improvement-plan Batch 3) ──

/// Build f.txt history demonstrating the -S vs -G distinction:
/// base("x") -> C1 add debug_flag=1 -> C2 change to debug_flag=2 -> C3 remove it.
/// -S debug_flag matches C1 (0->1) and C3 (1->0) but NOT C2 (1->1, count unchanged).
/// -G debug_flag matches C1, C2, C3 (each has a +/- line containing debug_flag).
fn pickaxe_repo() -> tempfile::TempDir {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    log_commit(repo.path(), "f.txt", "x\n", "base");
    log_commit(repo.path(), "f.txt", "x\ndebug_flag=1\n", "add flag");
    log_commit(repo.path(), "f.txt", "x\ndebug_flag=2\n", "change flag");
    log_commit(repo.path(), "f.txt", "x\n", "remove flag");
    repo
}

#[test]
fn test_pickaxe_string() {
    let repo = pickaxe_repo();
    let out = run_libra_command(&["log", "-S", "debug_flag", "--oneline"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("add flag"),
        "-S must match the count 0->1 commit: {stdout}"
    );
    assert!(
        stdout.contains("remove flag"),
        "-S must match the count 1->0 commit: {stdout}"
    );
    assert!(
        !stdout.contains("change flag"),
        "-S must NOT match a count-unchanged (1->1) edit: {stdout}"
    );
    assert!(
        !stdout.contains("base"),
        "-S must not match the base commit: {stdout}"
    );
}

#[test]
fn test_pickaxe_regex() {
    let repo = pickaxe_repo();
    let out = run_libra_command(&["log", "-G", "debug_[a-z]+", "--oneline"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // -G matches every commit whose +/- diff lines contain a debug_* token.
    assert!(stdout.contains("add flag"), "stdout: {stdout}");
    assert!(
        stdout.contains("change flag"),
        "-G must match the line-changing commit (unlike -S): {stdout}"
    );
    assert!(stdout.contains("remove flag"), "stdout: {stdout}");
    assert!(!stdout.contains("base"), "stdout: {stdout}");
}

#[test]
fn test_pickaxe_string_no_match_exits_zero() {
    let repo = pickaxe_repo();
    let out = run_libra_command(
        &["log", "-S", "nonexistent_token", "--oneline"],
        repo.path(),
    );
    assert!(out.status.success(), "no-match -S should exit 0");
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "no-match -S should print nothing: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn test_pickaxe_regex_invalid() {
    let repo = pickaxe_repo();
    let out = run_libra_command(&["log", "-G", "(unbalanced"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(129),
        "invalid -G regex must exit 129, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("LBR-CLI-002"));
}

#[test]
fn test_pickaxe_string_with_pathspec() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    log_commit(repo.path(), "a.txt", "x\n", "base a");
    // The debug_flag change is in a.txt only.
    log_commit(repo.path(), "a.txt", "x\ndebug_flag\n", "add flag to a");
    // An unrelated commit on b.txt.
    log_commit(repo.path(), "b.txt", "hello\n", "add b");

    // -S restricted to b.txt must NOT match the a.txt change (pathspec AND).
    let out = run_libra_command(
        &["log", "-S", "debug_flag", "--oneline", "--", "b.txt"],
        repo.path(),
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("add flag to a"),
        "pathspec must scope pickaxe: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// ── revision range A..B / A...B / ^A B (log-improvement-plan Batch 4, part 1) ──

/// Build base -> diverge into main("main work") and feature("feature work").
fn rev_range_repo() -> tempfile::TempDir {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    log_commit(repo.path(), "base.txt", "base", "base");
    let branch = run_libra_command(&["branch", "feature"], repo.path());
    assert!(
        branch.status.success(),
        "branch: {}",
        String::from_utf8_lossy(&branch.stderr)
    );
    log_commit(repo.path(), "m.txt", "m", "main work");
    let sw = run_libra_command(&["switch", "feature"], repo.path());
    assert!(
        sw.status.success(),
        "switch: {}",
        String::from_utf8_lossy(&sw.stderr)
    );
    log_commit(repo.path(), "f.txt", "f", "feature work");
    repo
}

#[test]
fn test_rev_range_two_dot() {
    let repo = rev_range_repo();
    let out = run_libra_command(&["log", "main..feature", "--oneline"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("feature work"),
        "A..B must include B-only commits: {stdout}"
    );
    assert!(
        !stdout.contains("main work"),
        "A..B must exclude A-side commits: {stdout}"
    );
    assert!(
        !stdout.contains("base"),
        "A..B must exclude the common ancestor: {stdout}"
    );
}

#[test]
fn test_rev_range_caret() {
    let repo = rev_range_repo();
    // `^main feature` is equivalent to `main..feature`.
    let out = run_libra_command(&["log", "^main", "feature", "--oneline"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("feature work"), "stdout: {stdout}");
    assert!(!stdout.contains("main work"), "stdout: {stdout}");
    assert!(!stdout.contains("base"), "stdout: {stdout}");
}

#[test]
fn test_rev_range_three_dot() {
    let repo = rev_range_repo();
    // Symmetric difference: commits reachable from exactly one side.
    let out = run_libra_command(&["log", "main...feature", "--oneline"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("feature work"), "stdout: {stdout}");
    assert!(
        stdout.contains("main work"),
        "A...B must include A-only commits too: {stdout}"
    );
    assert!(
        !stdout.contains("base"),
        "the common ancestor is reachable from both: {stdout}"
    );
}

#[test]
fn test_rev_range_bad_ref() {
    let repo = rev_range_repo();
    let out = run_libra_command(&["log", "nonexist..HEAD"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(129),
        "an unknown range endpoint must exit 129, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("LBR-CLI-003"),
        "expected CliInvalidTarget, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "stdout must stay clean on a bad range ref"
    );
}

#[test]
fn test_rev_range_json_total() {
    let repo = rev_range_repo();
    let out = run_libra_command(&["--json", "log", "main..feature"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json = parse_json_stdout(&out);
    let commits = json["data"]["commits"].as_array().expect("commits");
    assert_eq!(
        commits.len(),
        1,
        "main..feature has exactly one commit: {json}"
    );
    assert_eq!(commits[0]["subject"], "feature work");
}

#[test]
fn test_rev_range_double_dash_pathspec_filters_results() {
    let repo = rev_range_repo();
    let matches_feature = run_libra_command(
        &["log", "--oneline", "main..feature", "--", "f.txt"],
        repo.path(),
    );
    assert!(
        matches_feature.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&matches_feature.stderr)
    );
    let stdout = String::from_utf8_lossy(&matches_feature.stdout);
    assert!(stdout.contains("feature work"), "stdout: {stdout}");

    let misses_feature = run_libra_command(
        &["log", "--oneline", "main..feature", "--", "m.txt"],
        repo.path(),
    );
    assert!(
        misses_feature.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&misses_feature.stderr)
    );
    let stdout = String::from_utf8_lossy(&misses_feature.stdout);
    assert!(
        !stdout.contains("feature work"),
        "range pathspec must filter commit file changes: {stdout}"
    );
}

#[test]
fn test_rev_range_double_dash_pathspec_rejects_parent_escape() {
    let repo = rev_range_repo();
    let out = run_libra_command(&["log", "main..feature", "--", "../outside"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&out.stderr);

    assert_eq!(
        out.status.code(),
        Some(129),
        "escaping separated pathspec should be usage error, stderr: {stderr}"
    );
    assert!(out.stdout.is_empty());
    assert_eq!(report.error_code, "LBR-CLI-002");
}

fn follow_rename_repo() -> tempfile::TempDir {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());
    log_commit(
        repo.path(),
        "old.txt",
        "line one\nline two\n",
        "create old file",
    );

    let mv = run_libra_command(&["mv", "old.txt", "renamed.txt"], repo.path());
    assert!(
        mv.status.success(),
        "mv failed: {}",
        String::from_utf8_lossy(&mv.stderr)
    );
    let commit = run_libra_command(
        &["commit", "-m", "rename old file", "--no-verify"],
        repo.path(),
    );
    assert!(
        commit.status.success(),
        "rename commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    log_commit(
        repo.path(),
        "renamed.txt",
        "line one\nline two\nline three\n",
        "modify renamed file",
    );
    repo
}

#[test]
fn test_follow_rename_history() {
    let repo = follow_rename_repo();
    let out = run_libra_command(
        &["log", "--follow", "--oneline", "renamed.txt"],
        repo.path(),
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("modify renamed file"), "stdout: {stdout}");
    assert!(stdout.contains("rename old file"), "stdout: {stdout}");
    assert!(
        stdout.contains("create old file"),
        "--follow must continue through the rename to the old path: {stdout}"
    );
}

#[test]
fn test_follow_name_status_renders_rename_human_only() {
    let repo = follow_rename_repo();
    let out = run_libra_command(
        &["log", "--follow", "--name-status", "renamed.txt"],
        repo.path(),
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("R100\told.txt\trenamed.txt"),
        "--follow --name-status should show the rename relationship: {stdout}"
    );
}

#[test]
fn test_follow_multi_path_rejected() {
    let repo = follow_rename_repo();
    let out = run_libra_command(
        &["log", "--follow", "renamed.txt", "other.txt"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(129),
        "multi-path --follow should be usage error, stderr: {stderr}"
    );
    assert!(out.stdout.is_empty());
    assert_eq!(report.error_code, "LBR-CLI-002");
}

#[test]
fn test_follow_json_schema_is_stable() {
    let repo = follow_rename_repo();
    let out = run_libra_command(&["--json", "log", "--follow", "renamed.txt"], repo.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json = parse_json_stdout(&out);
    let commits = json["data"]["commits"].as_array().expect("commits array");
    let subjects = commits
        .iter()
        .map(|commit| commit["subject"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        subjects.contains(&"create old file"),
        "JSON --follow must include old-path history: {json}"
    );
    for commit in commits {
        for file in commit["files"].as_array().expect("files array") {
            let status = file["status"].as_str().expect("file status");
            assert!(
                matches!(status, "added" | "modified" | "deleted"),
                "JSON schema must not grow a rename status: {json}"
            );
        }
    }
}

#[test]
fn test_graph_color_respects_flag() {
    let repo = create_committed_repo_via_cli();
    let never = run_libra_command(
        &["log", "--graph", "--color=never", "--oneline"],
        repo.path(),
    );
    assert!(
        never.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&never.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&never.stdout).contains('\u{1b}'),
        "--color=never must render a plain graph: {:?}",
        String::from_utf8_lossy(&never.stdout)
    );

    let always = run_libra_command(
        &["log", "--graph", "--color=always", "--oneline"],
        repo.path(),
    );
    assert!(
        always.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&always.stderr)
    );
    assert!(
        String::from_utf8_lossy(&always.stdout).contains('\u{1b}'),
        "--color=always must color the graph: {:?}",
        String::from_utf8_lossy(&always.stdout)
    );
}

#[test]
fn test_log_reverse_shows_oldest_first() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    run_libra_command(&["config", "user.name", "Test User"], repo.path());
    run_libra_command(&["config", "user.email", "test@example.com"], repo.path());

    run_libra_command(
        &["commit", "--allow-empty", "-m", "first commit"],
        repo.path(),
    );
    run_libra_command(
        &["commit", "--allow-empty", "-m", "second commit"],
        repo.path(),
    );
    run_libra_command(
        &["commit", "--allow-empty", "-m", "third commit"],
        repo.path(),
    );

    let forward = run_libra_command(&["log", "--oneline"], repo.path());
    let forward_stdout = String::from_utf8_lossy(&forward.stdout);
    let forward_lines: Vec<&str> = forward_stdout.lines().collect();

    let reverse = run_libra_command(&["log", "--oneline", "--reverse"], repo.path());
    let reverse_stdout = String::from_utf8_lossy(&reverse.stdout);
    let reverse_lines: Vec<&str> = reverse_stdout.lines().collect();

    assert_eq!(
        forward.status.code(),
        Some(0),
        "forward: {}",
        String::from_utf8_lossy(&forward.stderr)
    );
    assert_eq!(
        reverse.status.code(),
        Some(0),
        "reverse: {}",
        String::from_utf8_lossy(&reverse.stderr)
    );

    assert!(
        !forward_lines.is_empty(),
        "forward output should not be empty"
    );
    assert!(
        !reverse_lines.is_empty(),
        "reverse output should not be empty"
    );
    assert_eq!(
        forward_lines.len(),
        reverse_lines.len(),
        "line counts must match"
    );

    assert!(
        forward_lines[0].contains("third commit"),
        "forward should start with newest (third) commit"
    );
    assert!(
        reverse_lines[0].contains("first commit"),
        "reverse should start with oldest (first) commit"
    );

    assert!(
        reverse_lines.last().unwrap().contains("third commit"),
        "reverse should end with newest (third) commit"
    );
}
