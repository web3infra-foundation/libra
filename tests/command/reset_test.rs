//! Tests reset command modes (soft/mixed/hard) and resulting state changes.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
use libra::utils::error::StableErrorCode;
use libra::{
    command::{
        branch::{self, BranchArgs},
        remove::{self, RemoveArgs},
        reset::{self, ResetArgs},
        status::{changes_to_be_committed, changes_to_be_staged},
    },
    internal::{branch::Branch as InternalBranch, config::ConfigKv},
    utils::{error::StableErrorCode, test::setup_with_new_libra_in},
};

use super::*;

async fn setup_reset_user_identity() {
    ConfigKv::set("user.name", "Test User", false)
        .await
        .unwrap();
    ConfigKv::set("user.email", "test@example.com", false)
        .await
        .unwrap();
}

#[test]
#[serial]
fn test_reset_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["reset", "HEAD"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn test_reset_unborn_head_returns_repo_state_error() {
    fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) {
        fs::create_dir_all(to).expect("failed to create destination directory");
        for entry in fs::read_dir(from).expect("failed to read source directory") {
            let entry = entry.expect("failed to read source entry");
            let source_path = entry.path();
            let destination_path = to.join(entry.file_name());
            if entry
                .file_type()
                .expect("failed to read source file type")
                .is_dir()
            {
                copy_dir_recursive(&source_path, &destination_path);
            } else {
                fs::copy(&source_path, &destination_path)
                    .expect("failed to copy object into unborn repository");
            }
        }
    }

    let source_repo = create_committed_repo_via_cli();
    let source_head = run_libra_command(&["show-ref", "--heads", "main"], source_repo.path());
    assert_cli_success(&source_head, "show-ref --heads main");
    let commit_hash = String::from_utf8_lossy(&source_head.stdout)
        .split_whitespace()
        .next()
        .expect("show-ref should return the main commit hash")
        .to_string();

    let target_repo = tempdir().unwrap();
    init_repo_via_cli(target_repo.path());
    copy_dir_recursive(
        &source_repo.path().join(".libra/objects"),
        &target_repo.path().join(".libra/objects"),
    );

    let output = run_libra_command(&["reset", "--hard", &commit_hash], target_repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        stderr.contains("HEAD is unborn"),
        "expected unborn HEAD message, got: {stderr}"
    );
}

#[test]
fn test_reset_json_output_reports_target_commit() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nsecond\n").unwrap();
    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert!(
        add_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_output.stderr)
    );
    let commit_output = run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());
    assert!(
        commit_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&commit_output.stderr)
    );

    let output = run_libra_command(&["--json", "reset", "--hard", "HEAD~1"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "reset");
    assert_eq!(json["data"]["mode"], "hard");
    assert_eq!(json["data"]["subject"], "base");
    assert_eq!(json["data"]["files_restored"], 1);
}

#[test]
fn test_reset_json_hard_head_clean_repo_reports_zero_restores() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "reset", "--hard", "HEAD"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["mode"], "hard");
    assert_eq!(json["data"]["files_restored"], 0);
}

#[test]
fn test_reset_json_hard_head_reports_actual_restored_files() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let output = run_libra_command(&["--json", "reset", "--hard", "HEAD"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["mode"], "hard");
    assert_eq!(json["data"]["files_restored"], 1);
    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "tracked\n"
    );
}

#[test]
fn test_reset_hard_with_pathspec_returns_usage_error() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let output = run_libra_command(
        &["reset", "--hard", "HEAD", "--", "tracked.txt"],
        repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        stderr.contains("Cannot do hard reset with paths."),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn test_reset_json_hard_with_pathspec_returns_usage_error() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n").unwrap();

    let output = run_libra_command(
        &["--json", "reset", "--hard", "HEAD", "--", "tracked.txt"],
        repo.path(),
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("expected stderr JSON in --json mode");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(
        output.stdout.is_empty(),
        "stdout should stay empty on JSON error"
    );
    assert_eq!(report["error_code"], "LBR-CLI-002");
    assert!(
        stderr.contains("Cannot do hard reset with paths."),
        "unexpected stderr: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_reset_corrupt_head_reference_returns_repo_corrupt() {
    let repo = create_committed_repo_via_cli();
    let target_commit = {
        let _guard = ChangeDirGuard::new(repo.path());
        InternalBranch::find_branch("main", None)
            .await
            .expect("main branch should exist")
            .commit
            .to_string()
    };
    {
        let _guard = ChangeDirGuard::new(repo.path());
        InternalBranch::update_branch("main", "not-a-valid-hash", None)
            .await
            .unwrap();
    }

    let output = run_libra_command(&["reset", &target_commit], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("stored HEAD reference is corrupt"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        stderr.contains("stored branch reference 'main' is corrupt"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stderr.contains("HEAD is unborn"),
        "reset should not misreport corrupt HEAD as unborn: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_reset_corrupt_target_branch_returns_repo_corrupt() {
    let repo = create_committed_repo_via_cli();
    {
        let _guard = ChangeDirGuard::new(repo.path());
        InternalBranch::update_branch("main", "not-a-valid-hash", None)
            .await
            .unwrap();
    }

    let output = run_libra_command(&["reset", "main"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("failed to resolve branch 'main'"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        stderr.contains("stored branch reference 'main' is corrupt"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stderr.contains("invalid reference"),
        "reset should not misclassify corrupt branch storage as invalid target: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_reset_pathspec_surfaces_subtree_corruption_as_repo_corrupt() {
    let repo = create_committed_repo_via_cli();
    fs::create_dir_all(repo.path().join("dir")).unwrap();
    fs::write(repo.path().join("dir").join("nested.txt"), "nested\n").unwrap();

    let add = run_libra_command(&["add", "dir/nested.txt"], repo.path());
    assert_cli_success(&add, "add dir/nested.txt");

    let commit = run_libra_command(&["commit", "-m", "nested", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit nested");

    {
        let _guard = ChangeDirGuard::new(repo.path());
        let head = InternalBranch::find_branch("main", None)
            .await
            .expect("main branch should exist")
            .commit;
        let commit: Commit = load_object(&head).expect("load HEAD commit");
        let tree: Tree = load_object(&commit.tree_id).expect("load root tree");
        let dir_item = tree
            .tree_items
            .iter()
            .find(|item| item.name == "dir")
            .expect("expected dir subtree");
        let dir_hash = dir_item.id.to_string();
        let object_path = repo
            .path()
            .join(".libra")
            .join("objects")
            .join(&dir_hash[..2])
            .join(&dir_hash[2..]);
        fs::write(object_path, b"corrupt subtree").unwrap();
    }

    let output = run_libra_command(&["reset", "HEAD", "--", "dir/nested.txt"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("failed to load tree"),
        "unexpected stderr: {stderr}"
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial]
async fn test_reset_hard_io_failure_rolls_back_index_and_keeps_head() {
    let temp_path = tempdir().unwrap();
    let _guard = ChangeDirGuard::new(temp_path.path());
    setup_with_new_libra_in(temp_path.path()).await;
    setup_reset_user_identity().await;

    fs::write("base.txt", "base\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["base.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;

    fs::write("tracked.txt", "tracked\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("add tracked".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;

    let head_before = Head::current_commit().await.unwrap();
    let original_mode = fs::metadata(temp_path.path()).unwrap().permissions().mode();
    fs::set_permissions(temp_path.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

    let result = reset::execute_safe(
        ResetArgs {
            target: "HEAD~1".to_string(),
            soft: false,
            mixed: false,
            hard: true,
            pathspecs: vec![],
        },
        &libra::utils::output::OutputConfig::default(),
    )
    .await;

    fs::set_permissions(
        temp_path.path(),
        std::fs::Permissions::from_mode(original_mode),
    )
    .unwrap();

    let error = result.expect_err("hard reset should fail when tracked file removal is denied");
    assert_eq!(error.stable_code(), StableErrorCode::IoWriteFailed);
    assert_eq!(Head::current_commit().await.unwrap(), head_before);
    assert!(temp_path.path().join("tracked.txt").exists());
    assert!(
        changes_to_be_committed().await.is_empty(),
        "failed hard reset should restore the index to match HEAD"
    );
    assert!(
        changes_to_be_staged().unwrap().modified.is_empty()
            && changes_to_be_staged().unwrap().deleted.is_empty()
            && changes_to_be_staged().unwrap().new.is_empty(),
        "failed hard reset should restore the working tree to match HEAD"
    );
}

/// Setup a standard test repository with 4 commits and branches
async fn setup_standard_repo(
    temp_path: &std::path::Path,
) -> (ObjectHash, ObjectHash, ObjectHash, ObjectHash) {
    test::setup_with_new_libra_in(temp_path).await;

    fs::write("1.txt", "content 1").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["1.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("commit 1: add 1.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;
    let commit1 = Head::current_commit().await.unwrap();
    branch::execute(BranchArgs {
        new_branch: Some("1".to_string()),
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
    })
    .await;

    fs::write("2.txt", "content 2").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["2.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("commit 2: add 2.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;
    let commit2 = Head::current_commit().await.unwrap();
    branch::execute(BranchArgs {
        new_branch: Some("2".to_string()),
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
    })
    .await;

    fs::write("3.txt", "content 3").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["3.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("commit 3: add 3.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;
    let commit3 = Head::current_commit().await.unwrap();
    branch::execute(BranchArgs {
        new_branch: Some("3".to_string()),
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
    })
    .await;

    fs::write("4.txt", "content 4").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["4.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("commit 4: add 4.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;
    let commit4 = Head::current_commit().await.unwrap();
    branch::execute(BranchArgs {
        new_branch: Some("4".to_string()),
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
    })
    .await;

    (commit1, commit2, commit3, commit4)
}

/// Setup the standard test state: modify files and stage some changes
async fn setup_test_state() {
    fs::write("3.txt", "content 3\nnew line").unwrap();
    fs::write("4.txt", "content 4\nnew line").unwrap();

    fs::write("5.txt", "new line").unwrap();

    add::execute(AddArgs {
        pathspec: vec!["3.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
}

#[tokio::test]
#[serial]
/// Tests soft reset: only moves HEAD pointer, preserves index and working directory
async fn test_reset_soft() {
    let temp_path = tempdir().unwrap();
    let _guard = ChangeDirGuard::new(temp_path.path());

    let (commit1, _, _, _) = setup_standard_repo(temp_path.path()).await;
    setup_test_state().await;

    // Perform soft reset to commit 1
    reset::execute(ResetArgs {
        target: "1".to_string(), // Reset to branch 1
        soft: true,
        mixed: false,
        hard: false,
        pathspecs: vec![],
    })
    .await;

    // Verify HEAD moved to commit 1
    let current_commit = Head::current_commit().await.unwrap();
    assert_eq!(current_commit, commit1);

    // Verify all files still exist in working directory
    assert!(fs::metadata("1.txt").is_ok());
    assert!(fs::metadata("2.txt").is_ok());
    assert!(fs::metadata("3.txt").is_ok());
    assert!(fs::metadata("4.txt").is_ok());
    assert!(fs::metadata("5.txt").is_ok());

    // Verify file contents are preserved (including modifications)
    assert_eq!(fs::read_to_string("3.txt").unwrap(), "content 3\nnew line");
    assert_eq!(fs::read_to_string("4.txt").unwrap(), "content 4\nnew line");
    assert_eq!(fs::read_to_string("5.txt").unwrap(), "new line");

    // Verify index still has staged changes (3.txt should be staged)
    let staged = libra::command::status::changes_to_be_committed().await;
    assert!(
        !staged.is_empty(),
        "Staged changes should be preserved in soft reset"
    );
}

#[tokio::test]
#[serial]
/// Tests mixed reset: moves HEAD and resets index, preserves working directory
async fn test_reset_mixed() {
    let temp_path = tempdir().unwrap();
    let _guard = ChangeDirGuard::new(temp_path.path());

    let (commit1, _, _, _) = setup_standard_repo(temp_path.path()).await;
    setup_test_state().await;

    // Perform mixed reset (default) to commit 1
    reset::execute(ResetArgs {
        target: "1".to_string(), // Reset to branch 1
        soft: false,
        mixed: false, // false means default (mixed)
        hard: false,
        pathspecs: vec![],
    })
    .await;

    // Verify HEAD moved to commit 1
    let current_commit = Head::current_commit().await.unwrap();
    assert_eq!(current_commit, commit1);

    // Verify all files still exist in working directory
    assert!(fs::metadata("1.txt").is_ok());
    assert!(fs::metadata("2.txt").is_ok());
    assert!(fs::metadata("3.txt").is_ok());
    assert!(fs::metadata("4.txt").is_ok());
    assert!(fs::metadata("5.txt").is_ok());

    // Verify file contents are preserved (including modifications)
    assert_eq!(fs::read_to_string("3.txt").unwrap(), "content 3\nnew line");
    assert_eq!(fs::read_to_string("4.txt").unwrap(), "content 4\nnew line");
    assert_eq!(fs::read_to_string("5.txt").unwrap(), "new line");

    // Verify index was reset (no staged changes)
    let staged = libra::command::status::changes_to_be_committed().await;
    assert!(staged.is_empty(), "Index should be reset in mixed reset");

    // Verify unstaged changes exist (2.txt, 3.txt, 4.txt should be untracked/modified)
    let unstaged = changes_to_be_staged().unwrap();
    assert!(
        !unstaged.new.is_empty() || !unstaged.modified.is_empty(),
        "Should have unstaged changes after mixed reset"
    );
}

#[tokio::test]
#[serial]
/// Tests hard reset: moves HEAD, resets index and working directory
async fn test_reset_hard() {
    let temp_path = tempdir().unwrap();
    let _guard = ChangeDirGuard::new(temp_path.path());

    let (commit1, _, _, _) = setup_standard_repo(temp_path.path()).await;
    setup_test_state().await;

    // Perform hard reset to commit 1
    reset::execute(ResetArgs {
        target: "1".to_string(), // Reset to branch 1
        soft: false,
        mixed: false,
        hard: true,
        pathspecs: vec![],
    })
    .await;

    // Verify HEAD moved to commit 1
    let current_commit = Head::current_commit().await.unwrap();
    assert_eq!(current_commit, commit1);

    // Verify working directory was reset - only 1.txt should exist from commit 1
    assert!(fs::metadata("1.txt").is_ok());
    assert!(
        fs::metadata("2.txt").is_err(),
        "2.txt should be removed by hard reset"
    );
    assert!(
        fs::metadata("3.txt").is_err(),
        "3.txt should be removed by hard reset"
    );
    assert!(
        fs::metadata("4.txt").is_err(),
        "4.txt should be removed by hard reset"
    );

    // Untracked files should remain
    assert!(
        fs::metadata("5.txt").is_ok(),
        "Untracked files should remain after hard reset"
    );

    // Verify file content was restored to commit 1 state
    assert_eq!(fs::read_to_string("1.txt").unwrap(), "content 1");
    assert_eq!(fs::read_to_string("5.txt").unwrap(), "new line");

    // Verify index was reset
    let staged = libra::command::status::changes_to_be_committed().await;
    assert!(staged.is_empty(), "Index should be reset in hard reset");

    // Verify only untracked files remain
    let unstaged = changes_to_be_staged().unwrap();
    assert!(
        !unstaged.new.is_empty(),
        "Should have untracked files (5.txt)"
    );
    assert!(
        unstaged.modified.is_empty(),
        "Should have no modified files"
    );
    assert!(unstaged.deleted.is_empty(), "Should have no deleted files");
}

#[tokio::test]
#[serial]
async fn test_reset_mixed_same_target_resets_index_without_moving_head() {
    let temp_path = tempdir().unwrap();
    let _guard = ChangeDirGuard::new(temp_path.path());
    setup_with_new_libra_in(temp_path.path()).await;
    setup_reset_user_identity().await;

    fs::write("tracked.txt", "tracked\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;
    let head_before = Head::current_commit().await.unwrap();

    fs::write("tracked.txt", "tracked\nstaged\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    reset::execute_safe(
        ResetArgs {
            target: "HEAD".to_string(),
            soft: false,
            mixed: true,
            hard: false,
            pathspecs: vec![],
        },
        &libra::utils::output::OutputConfig::default(),
    )
    .await
    .expect("mixed reset to HEAD should succeed");

    assert_eq!(Head::current_commit().await.unwrap(), head_before);
    assert!(
        changes_to_be_committed().await.is_empty(),
        "mixed reset to HEAD should unstage tracked changes"
    );
    let unstaged = changes_to_be_staged().unwrap();
    assert!(
        unstaged
            .modified
            .iter()
            .any(|path| path.file_name().and_then(|name| name.to_str()) == Some("tracked.txt")),
        "tracked.txt should remain modified in the worktree after mixed reset"
    );
}

#[tokio::test]
#[serial]
async fn test_reset_hard_same_target_restores_worktree_and_removes_staged_additions() {
    let temp_path = tempdir().unwrap();
    let _guard = ChangeDirGuard::new(temp_path.path());
    setup_with_new_libra_in(temp_path.path()).await;
    setup_reset_user_identity().await;

    fs::write("tracked.txt", "tracked\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;

    fs::write("tracked.txt", "tracked\nmodified\n").unwrap();
    fs::write("new.txt", "new\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["new.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    reset::execute_safe(
        ResetArgs {
            target: "HEAD".to_string(),
            soft: false,
            mixed: false,
            hard: true,
            pathspecs: vec![],
        },
        &libra::utils::output::OutputConfig::default(),
    )
    .await
    .expect("hard reset to HEAD should succeed");

    assert_eq!(fs::read_to_string("tracked.txt").unwrap(), "tracked\n");
    assert!(
        fs::metadata("new.txt").is_err(),
        "hard reset to HEAD should remove staged additions not present in the target tree"
    );
    assert!(
        changes_to_be_committed().await.is_empty(),
        "hard reset to HEAD should clear staged changes"
    );
    let unstaged = changes_to_be_staged().unwrap();
    assert!(
        unstaged.modified.is_empty(),
        "tracked changes should be restored"
    );
    assert!(
        unstaged.deleted.is_empty(),
        "tracked deletions should be cleared"
    );
}

#[tokio::test]
#[serial]
async fn test_reset_hard_removes_paths_tracked_only_by_head_tree() {
    let temp_path = tempdir().unwrap();
    let _guard = ChangeDirGuard::new(temp_path.path());
    setup_with_new_libra_in(temp_path.path()).await;
    setup_reset_user_identity().await;

    fs::write("base.txt", "base\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["base.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;

    fs::write("tracked.txt", "tracked\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["tracked.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("add tracked".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    })
    .await;

    remove::execute(RemoveArgs {
        pathspec: vec!["tracked.txt".to_string()],
        cached: true,
        recursive: false,
        force: false,
        dry_run: false,
        ignore_unmatch: false,
        pathspec_from_file: None,
        pathspec_file_nul: false,
    })
    .await;
    fs::write("tracked.txt", "tracked\nstill here\n").unwrap();

    reset::execute_safe(
        ResetArgs {
            target: "HEAD~1".to_string(),
            soft: false,
            mixed: false,
            hard: true,
            pathspecs: vec![],
        },
        &libra::utils::output::OutputConfig::default(),
    )
    .await
    .expect("hard reset should remove files tracked by HEAD even when absent from the index");

    assert!(
        fs::metadata("tracked.txt").is_err(),
        "hard reset should remove tracked.txt because the target commit does not contain it"
    );
    assert!(
        changes_to_be_committed().await.is_empty(),
        "hard reset should clear staged deletions"
    );
    let unstaged = changes_to_be_staged().unwrap();
    assert!(
        unstaged.deleted.is_empty(),
        "hard reset should not leave tracked.txt as a deleted path"
    );
}

#[tokio::test]
#[serial]
/// Tests reset with HEAD~ syntax
async fn test_reset_with_head_reference() {
    let temp_path = tempdir().unwrap();
    let _guard = ChangeDirGuard::new(temp_path.path());

    let (_, _, _, _) = setup_standard_repo(temp_path.path()).await;
    let second_commit = Head::current_commit().await.unwrap();

    // Reset using HEAD~ syntax
    reset::execute(ResetArgs {
        target: "HEAD~1".to_string(),
        soft: false,
        mixed: true,
        hard: false,
        pathspecs: vec![],
    })
    .await;

    // Verify HEAD moved back one commit
    let current_commit = Head::current_commit().await.unwrap();
    assert_ne!(current_commit, second_commit);

    // Verify working directory still has files
    assert!(fs::metadata("1.txt").is_ok());
    assert!(fs::metadata("4.txt").is_ok());

    // Verify index was reset (4.txt should be untracked)
    let unstaged = changes_to_be_staged().unwrap();
    assert!(
        unstaged
            .new
            .iter()
            .any(|path| path.file_name().unwrap() == "4.txt")
    );
}

#[tokio::test]
#[serial]
/// Tests reset on a branch (should move branch pointer, not create detached HEAD)
async fn test_reset_on_branch() {
    let temp_path = tempdir().unwrap();
    let _guard = ChangeDirGuard::new(temp_path.path());

    let (commit1, _, _, _) = setup_standard_repo(temp_path.path()).await;

    // Verify we're on a branch before reset
    let head_before = Head::current().await;
    match head_before {
        Head::Branch(branch_name) => {
            assert_eq!(branch_name, "main"); // Default branch name

            // Perform reset
            reset::execute(ResetArgs {
                target: commit1.to_string(),
                soft: true,
                mixed: false,
                hard: false,
                pathspecs: vec![],
            })
            .await;

            // Verify we're still on the same branch after reset
            let head_after = Head::current().await;
            match head_after {
                Head::Branch(branch_name_after) => {
                    assert_eq!(branch_name_after, branch_name);
                }
                Head::Detached(_) => {
                    panic!("Reset should not create detached HEAD when on a branch");
                }
            }

            // Verify the branch pointer moved
            let current_commit = Head::current_commit().await.unwrap();
            assert_eq!(current_commit, commit1);
        }
        Head::Detached(_) => {
            panic!("Should be on a branch initially");
        }
    }
}
