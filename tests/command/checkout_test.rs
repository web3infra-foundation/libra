//! Tests checkout behavior for switching branches and creating new ones via restore logic.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use colored::Colorize;
use libra::{
    command::{
        add::{self, AddArgs},
        branch,
        checkout::{check_branch, get_current_branch, switch_branch},
        commit, init,
        restore::{self, RestoreArgs},
    },
    internal::{config::ConfigKv, head::Head},
    utils::{output::OutputConfig, test, util},
};
use serial_test::serial;
use tempfile::tempdir;

async fn configure_identity_for_test() {
    ConfigKv::set("user.name", "Checkout Test User", false)
        .await
        .unwrap();
    ConfigKv::set("user.email", "checkout-test@example.com", false)
        .await
        .unwrap();
}
async fn test_check_branch() {
    println!("\n\x1b[1mTest check_branch function.\x1b[0m");

    // For non-existent branches, it should return Err
    assert!(check_branch("non_existent_branch").await.is_err());
    // For the current branch, it should return Ok(None)
    assert_eq!(
        check_branch(&get_current_branch().await.unwrap_or("main".to_string()))
            .await
            .unwrap(),
        None
    );
    // For other existing branches, it should return Ok(Some(false))
    assert_eq!(check_branch("new_branch_01").await.unwrap(), Some(false));
}

async fn test_switch_branch() {
    println!("\n\x1b[1mTest switch_branch function.\x1b[0m");

    let show_all_branches = async || {
        // Use the list_branches function of the branch module to list all current local branches
        let _ = branch::list_branches(branch::BranchListMode::Local, &[], &[]).await;
        println!(
            "Current branch is '{}'.",
            get_current_branch()
                .await
                .unwrap_or("Get_current_branch_failed".to_string())
                .green()
        );
    };

    // Switch to the new branch and back
    show_all_branches().await;
    switch_branch("new_branch_01").await.unwrap();
    show_all_branches().await;
    switch_branch("new_branch_02").await.unwrap();
    show_all_branches().await;
    switch_branch("main").await.unwrap();
    show_all_branches().await;
}

#[tokio::test]
#[serial]
/// Tests branch creation, switching and validation functionality in the checkout module.
/// Verifies proper branch management and HEAD reference updates when switching between branches.
async fn test_checkout_module_functions() {
    println!("\n\x1b[1mTest checkout module functions.\x1b[0m");

    let temp_path = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp_path.path());
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let init_args = init::InitArgs {
        bare: false,
        initial_branch: Some("main".to_string()),
        template: None,
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: false,
        shared: None,
        object_format: None,
        ref_format: None,
        from_git_repository: None,
        vault: false,
    };

    init::init(init_args)
        .await
        .expect("Error initializing repository");
    configure_identity_for_test().await;

    // Initialize the main branch by creating an empty commit
    let commit_args = commit::CommitArgs {
        message: Some("An empty initial commit".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute_safe(commit_args, &OutputConfig::default())
        .await
        .expect("initial commit should succeed");

    // Create tow new branch
    branch::create_branch(String::from("new_branch_01"), get_current_branch().await).await;
    branch::create_branch(String::from("new_branch_02"), get_current_branch().await).await;

    // Test the checkout module funsctions
    test_check_branch().await;
    test_switch_branch().await;
}

#[tokio::test]
#[serial]
/// Same branch workflow but in a SHA-256 repository; verifies commit id length matches hash kind.
async fn test_checkout_module_functions_sha256() {
    let temp_path = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp_path.path());
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    let init_args = init::InitArgs {
        bare: false,
        initial_branch: Some("main".to_string()),
        template: None,
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: false,
        shared: None,
        object_format: Some("sha256".to_string()),
        ref_format: None,
        from_git_repository: None,
        vault: false,
    };

    init::init(init_args)
        .await
        .expect("Error initializing repository");
    configure_identity_for_test().await;

    let commit_args = commit::CommitArgs {
        message: Some("An empty initial commit".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute_safe(commit_args, &OutputConfig::default())
        .await
        .expect("initial commit should succeed");

    // Ensure HEAD commit uses SHA-256 (64 hex chars)
    let head_commit = Head::current_commit().await.expect("HEAD missing");
    assert_eq!(head_commit.to_string().len(), 64);

    branch::create_branch(String::from("new_branch_01"), get_current_branch().await).await;
    branch::create_branch(String::from("new_branch_02"), get_current_branch().await).await;

    // Reuse existing helpers; they should work under SHA-256 as well.
    test_check_branch().await;
    test_switch_branch().await;
}

#[tokio::test]
#[serial]
/// In a SHA-256 repo, attempting to restore with a SHA-1 length hash should not change the worktree.
async fn checkout_restore_rejects_sha1_hash_in_sha256_repo() {
    let temp_path = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp_path.path());
    let _guard = test::ChangeDirGuard::new(temp_path.path());

    init::init(init::InitArgs {
        bare: false,
        initial_branch: Some("main".to_string()),
        template: None,
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: true,
        shared: None,
        object_format: Some("sha256".to_string()),
        ref_format: None,
        from_git_repository: None,
        vault: false,
    })
    .await
    .unwrap();
    configure_identity_for_test().await;

    // create and commit a file
    test::ensure_file("foo.txt", Some("v1"));
    add::execute_safe(
        AddArgs {
            pathspec: vec!["foo.txt".into()],
            all: false,
            update: false,
            refresh: false,
            force: false,
            verbose: false,
            dry_run: false,
            ignore_errors: false,
            ..Default::default()
        },
        &OutputConfig::default(),
    )
    .await
    .expect("add should succeed");
    commit::execute_safe(
        commit::CommitArgs {
            message: Some("init".into()),
            file: None,
            allow_empty: false,
            conventional: false,
            no_edit: false,
            amend: false,
            signoff: false,
            disable_pre: true,
            all: true,
            no_verify: false,
            author: None,
        },
        &OutputConfig::default(),
    )
    .await
    .expect("commit should succeed");

    // modify the file
    test::ensure_file("foo.txt", Some("v2"));

    // try to restore using a SHA-1 length hash in a SHA-256 repo; should no-op
    let _ = restore::execute_safe(
        RestoreArgs {
            worktree: true,
            staged: true,
            source: Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904".into()),
            pathspec: vec![util::working_dir_string()],
        },
        &OutputConfig::default(),
    )
    .await;

    let content = std::fs::read_to_string(util::working_dir().join("foo.txt")).unwrap();
    assert_eq!(content, "v2", "invalid hash should not restore file");
}

/// Verifies that `checkout -b` returns a [`CliError`] when the worktree has
/// uncommitted staged changes that would be overwritten.
#[tokio::test]
#[serial]
async fn test_checkout_new_branch_with_dirty_worktree_returns_error() {
    use clap::Parser;
    use libra::{
        command::{
            add::{self, AddArgs},
            checkout::{self, CheckoutArgs},
            commit,
        },
        internal::config::ConfigKv,
        utils::test::{self, ChangeDirGuard},
    };

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    ConfigKv::set("user.name", "Checkout Tester", false)
        .await
        .unwrap();
    ConfigKv::set("user.email", "checkout@test.com", false)
        .await
        .unwrap();

    // Create initial commit so HEAD exists
    test::ensure_file("base.txt", Some("base"));
    add::execute_safe(
        AddArgs {
            pathspec: vec!["base.txt".into()],
            all: false,
            update: false,
            refresh: false,
            verbose: false,
            force: false,
            dry_run: false,
            ignore_errors: false,
            ..Default::default()
        },
        &OutputConfig::default(),
    )
    .await
    .expect("add should succeed");
    commit::execute_safe(
        commit::CommitArgs {
            message: Some("initial".into()),
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
        },
        &OutputConfig::default(),
    )
    .await
    .expect("initial commit should succeed");

    // Stage a change without committing — working tree is "dirty"
    test::ensure_file("dirty.txt", Some("uncommitted"));
    add::execute_safe(
        AddArgs {
            pathspec: vec!["dirty.txt".into()],
            all: false,
            update: false,
            refresh: false,
            verbose: false,
            force: false,
            dry_run: false,
            ignore_errors: false,
            ..Default::default()
        },
        &OutputConfig::default(),
    )
    .await
    .expect("add dirty file should succeed");

    let result = checkout::execute_safe(
        CheckoutArgs::try_parse_from(["checkout", "-b", "new"]).unwrap(),
        &OutputConfig::default(),
    )
    .await;
    assert!(
        result.is_err(),
        "checkout should fail when worktree is dirty"
    );
    let err = result.unwrap_err();
    assert!(
        err.message().contains("local changes"),
        "error should mention local changes, got: {}",
        err.message()
    );
}

/// Checking out the current branch should be a no-op even when the worktree
/// is dirty (Git prints "Already on ...").
#[tokio::test]
#[serial]
async fn test_checkout_current_branch_with_dirty_worktree_succeeds() {
    use clap::Parser;
    use libra::{
        command::{
            add::{self, AddArgs},
            checkout::{self, CheckoutArgs},
            commit,
        },
        internal::config::ConfigKv,
        utils::test::{self, ChangeDirGuard},
    };

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    ConfigKv::set("user.name", "Checkout Tester", false)
        .await
        .unwrap();
    ConfigKv::set("user.email", "checkout@test.com", false)
        .await
        .unwrap();

    test::ensure_file("base.txt", Some("base"));
    add::execute_safe(
        AddArgs {
            pathspec: vec!["base.txt".into()],
            all: false,
            update: false,
            refresh: false,
            verbose: false,
            force: false,
            dry_run: false,
            ignore_errors: false,
            ..Default::default()
        },
        &OutputConfig::default(),
    )
    .await
    .expect("add should succeed");
    commit::execute_safe(
        commit::CommitArgs {
            message: Some("initial".into()),
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
        },
        &OutputConfig::default(),
    )
    .await
    .expect("initial commit should succeed");

    // Dirty the worktree with unstaged content.
    test::ensure_file("base.txt", Some("base\nlocal change"));

    let current = get_current_branch()
        .await
        .expect("current branch should be present");
    let args = CheckoutArgs::try_parse_from(["checkout", current.as_str()]).unwrap();
    let result = checkout::execute_safe(args, &OutputConfig::default()).await;
    assert!(
        result.is_ok(),
        "checkout current branch should not fail on dirty worktree"
    );

    let after = get_current_branch()
        .await
        .expect("current branch should still be present");
    assert_eq!(after, current, "branch should remain unchanged");
    let content = std::fs::read_to_string("base.txt").unwrap();
    assert_eq!(content, "base\nlocal change");
}

/// Switching to another branch should keep checkout-specific dirty-worktree
/// wording even when the worktree has only unstaged changes.
#[tokio::test]
#[serial]
async fn test_checkout_existing_branch_with_unstaged_dirty_worktree_returns_error() {
    use clap::Parser;
    use libra::{
        command::{
            add::{self, AddArgs},
            checkout::{self, CheckoutArgs},
            commit,
        },
        internal::config::ConfigKv,
        utils::test::{self, ChangeDirGuard},
    };

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    ConfigKv::set("user.name", "Checkout Tester", false)
        .await
        .unwrap();
    ConfigKv::set("user.email", "checkout@test.com", false)
        .await
        .unwrap();

    test::ensure_file("base.txt", Some("base"));
    add::execute_safe(
        AddArgs {
            pathspec: vec!["base.txt".into()],
            all: false,
            update: false,
            refresh: false,
            verbose: false,
            force: false,
            dry_run: false,
            ignore_errors: false,
            ..Default::default()
        },
        &OutputConfig::default(),
    )
    .await
    .expect("add should succeed");
    commit::execute_safe(
        commit::CommitArgs {
            message: Some("initial".into()),
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
        },
        &OutputConfig::default(),
    )
    .await
    .expect("initial commit should succeed");

    branch::create_branch(String::from("other"), get_current_branch().await).await;

    test::ensure_file("base.txt", Some("base\nlocal change"));

    let result = checkout::execute_safe(
        CheckoutArgs::try_parse_from(["checkout", "other"]).unwrap(),
        &OutputConfig::default(),
    )
    .await;
    if let Ok(()) = result {
        panic!("checkout should fail when unstaged changes would be overwritten");
    }
    let err = result.unwrap_err();
    assert!(
        err.message()
            .contains("local changes would be overwritten by checkout"),
        "error should preserve checkout dirty-worktree wording, got: {}",
        err.message()
    );
}

#[test]
fn test_checkout_existing_branch_with_conflicting_untracked_file_returns_error() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_cli_error_stderr,
        run_libra_command,
    };

    let repo = create_committed_repo_via_cli();

    let create = run_libra_command(&["switch", "-c", "other"], repo.path());
    assert_cli_success(&create, "switch -c other");

    std::fs::write(repo.path().join("conflict.txt"), "tracked on other\n").unwrap();
    let add = run_libra_command(&["add", "conflict.txt"], repo.path());
    assert_cli_success(&add, "add conflict.txt on other");
    let commit = run_libra_command(
        &["commit", "-m", "other adds conflict", "--no-verify"],
        repo.path(),
    );
    assert_cli_success(&commit, "commit conflict.txt on other");

    let back = run_libra_command(&["switch", "main"], repo.path());
    assert_cli_success(&back, "switch main");

    std::fs::write(repo.path().join("conflict.txt"), "local untracked\n").unwrap();

    let output = run_libra_command(&["checkout", "other"], repo.path());
    assert_eq!(output.status.code(), Some(128));
    let (_human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        report
            .message
            .contains("local changes would be overwritten by checkout"),
        "error should preserve checkout wording, got: {}",
        report.message
    );

    let content = std::fs::read_to_string(repo.path().join("conflict.txt")).unwrap();
    assert_eq!(content, "local untracked\n");
}

#[test]
fn test_checkout_json_show_current_branch() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
    };

    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "checkout"], repo.path());
    assert_cli_success(&output, "json checkout show current");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "checkout");
    assert_eq!(json["data"]["action"], "show-current");
    assert_eq!(json["data"]["branch"], "main");
    assert_eq!(json["data"]["detached"], false);
    assert_eq!(json["data"]["switched"], false);
    assert!(json["data"]["commit"].as_str().unwrap_or_default().len() >= 7);
    assert!(output.stderr.is_empty());
}

#[test]
fn test_checkout_json_switch_existing_branch() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
    };

    let repo = create_committed_repo_via_cli();
    let branch = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&branch, "branch feature");

    let output = run_libra_command(&["--json", "checkout", "feature"], repo.path());
    assert_cli_success(&output, "json checkout feature");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "checkout");
    assert_eq!(json["data"]["action"], "switch");
    assert_eq!(json["data"]["previous_branch"], "main");
    assert_eq!(json["data"]["branch"], "feature");
    assert_eq!(json["data"]["switched"], true);
    assert_eq!(json["data"]["created"], false);
    assert_eq!(json["data"]["pulled"], false);
    assert!(output.stderr.is_empty());
}

#[test]
fn test_checkout_json_current_branch_reports_already_on() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
    };

    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "checkout", "main"], repo.path());
    assert_cli_success(&output, "json checkout main");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "checkout");
    assert_eq!(json["data"]["action"], "already-on");
    assert_eq!(json["data"]["branch"], "main");
    assert_eq!(json["data"]["already_on"], true);
    assert_eq!(json["data"]["switched"], false);
    assert!(output.stderr.is_empty());
}

#[test]
fn test_checkout_json_create_branch() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
    };

    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "checkout", "-b", "feature"], repo.path());
    assert_cli_success(&output, "json checkout -b feature");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "checkout");
    assert_eq!(json["data"]["action"], "create");
    assert_eq!(json["data"]["previous_branch"], "main");
    assert_eq!(json["data"]["branch"], "feature");
    assert_eq!(json["data"]["switched"], true);
    assert_eq!(json["data"]["created"], true);
    assert!(output.stderr.is_empty());
}

#[test]
fn test_checkout_separator_path_restores_worktree_from_index() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

    let repo = create_committed_repo_via_cli();
    let branch = run_libra_command(&["branch", "tracked.txt"], repo.path());
    assert_cli_success(&branch, "branch tracked.txt");

    std::fs::write(repo.path().join("tracked.txt"), "worktree edit\n").unwrap();

    let output = run_libra_command(&["checkout", "--", "tracked.txt"], repo.path());
    assert_cli_success(&output, "checkout -- tracked.txt");

    let content = std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap();
    assert_eq!(content, "tracked\n");

    let branch = run_libra_command(&["branch", "--show-current"], repo.path());
    assert_cli_success(&branch, "branch --show-current");
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "main");
}

#[test]
fn test_checkout_plain_name_stays_branch_mode_when_file_matches_branch() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

    let repo = create_committed_repo_via_cli();
    let branch = run_libra_command(&["branch", "tracked.txt"], repo.path());
    assert_cli_success(&branch, "branch tracked.txt");

    let output = run_libra_command(&["checkout", "tracked.txt"], repo.path());
    assert_cli_success(&output, "checkout tracked.txt");

    let branch = run_libra_command(&["branch", "--show-current"], repo.path());
    assert_cli_success(&branch, "branch --show-current");
    assert_eq!(
        String::from_utf8_lossy(&branch.stdout).trim(),
        "tracked.txt"
    );
}

// ════════════════════════════════════════════════════════════════════════
//  Batch 0 — -B / --detach / --orphan branch checkout modes
// ════════════════════════════════════════════════════════════════════════

/// `libra rev-parse <rev>` → trimmed OID (panics on failure).
fn rev_parse(repo: &std::path::Path, rev: &str) -> String {
    let out = super::run_libra_command(&["rev-parse", rev], repo);
    assert!(
        out.status.success(),
        "rev-parse {rev} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Add a second empty commit so `HEAD~1` exists.
fn add_empty_commit(repo: &std::path::Path, msg: &str) {
    super::assert_cli_success(
        &super::run_libra_command(&["commit", "--allow-empty", "-m", msg, "--no-verify"], repo),
        "empty commit",
    );
}

/// `-B <branch>` resets an existing branch to the current HEAD and switches.
#[test]
fn checkout_force_branch_resets_existing_branch_to_head() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    // branch_x is created at the first commit, then main advances.
    assert_cli_success(
        &run_libra_command(&["branch", "branch_x"], repo.path()),
        "branch",
    );
    add_empty_commit(repo.path(), "m2");
    let head = rev_parse(repo.path(), "HEAD");
    assert_ne!(
        rev_parse(repo.path(), "branch_x"),
        head,
        "precondition: branch_x behind HEAD"
    );

    let out = run_libra_command(&["checkout", "-B", "branch_x"], repo.path());
    assert_cli_success(&out, "checkout -B branch_x");
    assert_eq!(
        rev_parse(repo.path(), "branch_x"),
        head,
        "branch_x reset to HEAD"
    );
    let cur = run_libra_command(&["branch", "--show-current"], repo.path());
    assert_eq!(String::from_utf8_lossy(&cur.stdout).trim(), "branch_x");
}

/// `-B <branch> <start_point>` resets to the given start point.
#[test]
fn checkout_force_branch_with_start_point_resets_to_target() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "branch_x"], repo.path()),
        "branch",
    );
    add_empty_commit(repo.path(), "m2");
    let parent = rev_parse(repo.path(), "HEAD~1");

    let out = run_libra_command(&["checkout", "-B", "branch_x", "HEAD~1"], repo.path());
    assert_cli_success(&out, "checkout -B branch_x HEAD~1");
    assert_eq!(
        rev_parse(repo.path(), "branch_x"),
        parent,
        "reset to HEAD~1"
    );
}

/// `-B <new>` (absent) creates the branch at the current HEAD.
#[test]
fn checkout_force_branch_creates_when_absent() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let head = rev_parse(repo.path(), "HEAD");
    let out = run_libra_command(&["checkout", "-B", "new_x"], repo.path());
    assert_cli_success(&out, "checkout -B new_x");
    assert_eq!(
        rev_parse(repo.path(), "new_x"),
        head,
        "new_x created at HEAD"
    );
}

/// `-B <new> <start_point>` (absent) creates the branch at the start point, not HEAD.
#[test]
fn checkout_force_branch_creates_absent_at_start_point() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    add_empty_commit(repo.path(), "m2");
    let parent = rev_parse(repo.path(), "HEAD~1");
    let head = rev_parse(repo.path(), "HEAD");
    let out = run_libra_command(&["checkout", "-B", "new_x", "HEAD~1"], repo.path());
    assert_cli_success(&out, "checkout -B new_x HEAD~1");
    assert_eq!(
        rev_parse(repo.path(), "new_x"),
        parent,
        "created at HEAD~1, not HEAD"
    );
    assert_ne!(parent, head, "HEAD~1 must differ from HEAD");
}

/// `-B intent` (AI-managed branch) is blocked with CliInvalidTarget (129).
#[test]
fn checkout_force_branch_on_intent_is_blocked() {
    use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["checkout", "-B", "intent"], repo.path());
    assert_eq!(out.status.code(), Some(129));
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
}

/// `-B main` and `--detach main~1` are NOT blocked (main is not AI-managed).
#[test]
fn checkout_force_branch_on_main_is_allowed() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    add_empty_commit(repo.path(), "m2");
    // -B main resets main to HEAD (no-op-ish), must not be blocked.
    let out = run_libra_command(&["checkout", "-B", "main"], repo.path());
    assert_cli_success(&out, "-B main must be allowed");
    // --detach main~1 must also be allowed.
    let out = run_libra_command(&["checkout", "--detach", "main~1"], repo.path());
    assert_cli_success(&out, "--detach main~1 must be allowed");
}

/// `--detach <commit>` moves HEAD to a detached state and prints the banner.
#[test]
fn checkout_detach_to_commit_sets_detached_head() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    add_empty_commit(repo.path(), "m2");
    let parent = rev_parse(repo.path(), "HEAD~1");

    let out = run_libra_command(&["checkout", "--detach", "HEAD~1"], repo.path());
    assert_cli_success(&out, "checkout --detach HEAD~1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("HEAD detached at"),
        "detach banner: {stdout}"
    );
    assert!(
        stdout.contains(&parent[..8]),
        "banner shows short hash: {stdout}"
    );
}

/// After `--detach`, JSON checkout reports `detached == true`.
#[test]
fn checkout_detach_then_show_current_reports_detached() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
    };
    let repo = create_committed_repo_via_cli();
    add_empty_commit(repo.path(), "m2");
    assert_cli_success(
        &run_libra_command(&["checkout", "--detach", "HEAD~1"], repo.path()),
        "detach",
    );
    let out = run_libra_command(&["--json", "checkout"], repo.path());
    let json = parse_json_stdout(&out);
    assert_eq!(json["data"]["detached"], true);
}

/// From a detached HEAD, `checkout <branch>` rebinds HEAD to that branch.
#[test]
fn checkout_from_detached_back_to_branch_rebinds_head() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    add_empty_commit(repo.path(), "m2");
    assert_cli_success(
        &run_libra_command(&["checkout", "--detach", "HEAD~1"], repo.path()),
        "detach",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], repo.path()),
        "back to main",
    );
    let cur = run_libra_command(&["branch", "--show-current"], repo.path());
    assert_eq!(String::from_utf8_lossy(&cur.stdout).trim(), "main");
}

/// `--detach agent-traces~1` (AI-managed revision suffix) is blocked (129).
#[test]
fn checkout_detach_on_ai_managed_revision_suffix_is_blocked() {
    use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["checkout", "--detach", "agent-traces~1"], repo.path());
    assert_eq!(out.status.code(), Some(129));
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
}

/// `--orphan <branch>` creates an unborn branch (JSON: orphan=true, no commit).
#[test]
fn checkout_orphan_creates_unborn_branch() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
    };
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["--json", "checkout", "--orphan", "orphan_x"], repo.path());
    assert_cli_success(&out, "checkout --orphan orphan_x");
    let json = parse_json_stdout(&out);
    assert_eq!(json["data"]["action"], "create");
    assert_eq!(json["data"]["branch"], "orphan_x");
    assert_eq!(json["data"]["orphan"], true);
    assert_eq!(json["data"]["created"], true);
    assert!(json["data"]["commit"].is_null(), "unborn: no commit");
    // HEAD is on orphan_x but has no commit yet (rev-parse HEAD fails).
    let cur = run_libra_command(&["branch", "--show-current"], repo.path());
    assert_eq!(String::from_utf8_lossy(&cur.stdout).trim(), "orphan_x");
    let head = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert!(!head.status.success(), "unborn HEAD has no commit");
}

/// `--orphan agent-traces` (AI-managed name) is blocked (129).
#[test]
fn checkout_orphan_on_agent_traces_is_blocked() {
    use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["checkout", "--orphan", "agent-traces"], repo.path());
    assert_eq!(out.status.code(), Some(129));
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
}

/// `--orphan` then a first commit produces a parentless commit that fsck accepts.
#[test]
fn checkout_orphan_then_first_commit_passes_fsck() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["checkout", "--orphan", "orphan_x"], repo.path()),
        "orphan",
    );
    std::fs::write(repo.path().join("seed.txt"), "seed\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "seed.txt"], repo.path()), "add");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "orphan root", "--no-verify"], repo.path()),
        "first orphan commit",
    );
    // The new commit has no parent.
    let head = rev_parse(repo.path(), "HEAD");
    let show = run_libra_command(&["cat-file", "-p", &head], repo.path());
    assert!(
        !String::from_utf8_lossy(&show.stdout).contains("parent "),
        "orphan root commit must have no parent"
    );
    let fsck = run_libra_command(&["fsck"], repo.path());
    assert_cli_success(&fsck, "fsck after orphan commit");
}

/// `-B` writes a `checkout: moving from <old> to <new>` reflog entry.
#[test]
fn checkout_force_branch_reflog_records_moving_from_to() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
    };
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["branch", "branch_x"], repo.path()),
        "branch",
    );
    add_empty_commit(repo.path(), "m2");
    assert_cli_success(
        &run_libra_command(&["checkout", "-B", "branch_x"], repo.path()),
        "-B",
    );

    let out = run_libra_command(&["--json", "reflog", "show"], repo.path());
    let json = parse_json_stdout(&out);
    let newest = &json["data"]["entries"][0];
    assert_eq!(newest["action"], "checkout");
    let msg = newest["message"].as_str().unwrap_or("");
    assert!(
        msg.starts_with("moving from") && msg.contains("branch_x"),
        "reflog msg: {msg}"
    );
}

/// `--orphan` writes NO HEAD reflog entry (verified against stock Git, whose
/// `.git/logs/HEAD` gains no row): the target branch is unborn, so there is no
/// commit OID to record. The pre-orphan entry stays newest and `reflog show`
/// still renders cleanly (no all-zero `new_oid` to choke the commit lookup).
#[test]
fn checkout_orphan_writes_no_head_reflog_entry() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
    };
    let repo = create_committed_repo_via_cli();

    let before = run_libra_command(&["--json", "reflog", "show"], repo.path());
    assert_cli_success(&before, "reflog show before orphan");
    let before_json = parse_json_stdout(&before);
    let before_count = before_json["data"]["entries"]
        .as_array()
        .map(|e| e.len())
        .unwrap_or(0);
    let before_newest = before_json["data"]["entries"][0]["new_oid"]
        .as_str()
        .unwrap_or("")
        .to_string();

    assert_cli_success(
        &run_libra_command(&["checkout", "--orphan", "orphan_x"], repo.path()),
        "orphan",
    );

    // reflog show must still succeed (valid JSON) and be unchanged: orphan adds
    // no entry, so the count and the newest commit OID are identical.
    let after = run_libra_command(&["--json", "reflog", "show"], repo.path());
    assert_cli_success(&after, "reflog show after orphan");
    let after_json = parse_json_stdout(&after);
    let after_entries = after_json["data"]["entries"].as_array();
    assert_eq!(
        after_entries.map(|e| e.len()).unwrap_or(0),
        before_count,
        "orphan must not add a HEAD reflog entry"
    );
    assert_eq!(
        after_json["data"]["entries"][0]["new_oid"]
            .as_str()
            .unwrap_or(""),
        before_newest,
        "newest reflog entry must be unchanged by orphan"
    );
    let newest_msg = after_json["data"]["entries"][0]["message"]
        .as_str()
        .unwrap_or("");
    assert!(
        !newest_msg.contains("orphan_x"),
        "orphan must not write a checkout reflog entry, got: {newest_msg}"
    );
}

/// Dirty worktree without `--force` blocks `--detach` (RepoStateInvalid, 128); HEAD unchanged.
#[test]
fn checkout_detach_dirty_without_force_blocks() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_cli_error_stderr,
        run_libra_command,
    };
    let repo = create_committed_repo_via_cli();
    add_empty_commit(repo.path(), "m2");
    // Make the worktree dirty (modify a tracked file).
    std::fs::write(repo.path().join("tracked.txt"), "dirty edit\n").unwrap();
    let out = run_libra_command(&["checkout", "--detach", "HEAD~1"], repo.path());
    assert_eq!(out.status.code(), Some(128), "dirty detach blocked");
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
    // HEAD still on main.
    let cur = run_libra_command(&["branch", "--show-current"], repo.path());
    assert_eq!(String::from_utf8_lossy(&cur.stdout).trim(), "main");
    assert_cli_success(&cur, "still on a branch");
}

/// Dirty worktree without `--force` blocks `--orphan` (128).
#[test]
fn checkout_orphan_dirty_without_force_blocks() {
    use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};
    let repo = create_committed_repo_via_cli();
    add_empty_commit(repo.path(), "m2");
    std::fs::write(repo.path().join("tracked.txt"), "dirty edit\n").unwrap();
    let out = run_libra_command(&["checkout", "--orphan", "orphan_x", "HEAD~1"], repo.path());
    assert_eq!(out.status.code(), Some(128), "dirty orphan blocked");
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
}

/// `--detach -b x` (mode conflict) is rejected as a usage error. Libra remaps
/// clap's `ArgumentConflict` for a present subcommand to `command_usage` (129),
/// not clap's native exit 2.
#[test]
fn checkout_detach_with_b_is_clap_conflict() {
    use super::{create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["checkout", "--detach", "-b", "x"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(129),
        "clap mode conflict → command_usage"
    );
}

#[test]
fn test_checkout_json_treeish_separator_path_restores_index_and_worktree() {
    use super::{
        assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
    };

    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "staged edit\n").unwrap();
    let add = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add, "add staged edit");
    std::fs::write(repo.path().join("tracked.txt"), "worktree edit\n").unwrap();

    let output = run_libra_command(
        &["--json", "checkout", "HEAD", "--", "tracked.txt"],
        repo.path(),
    );
    assert_cli_success(&output, "json checkout HEAD -- tracked.txt");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "checkout");
    assert_eq!(json["data"]["action"], "restore-paths");
    assert_eq!(json["data"]["switched"], false);
    assert_eq!(json["data"]["restore"]["source"], "HEAD");
    assert_eq!(json["data"]["restore"]["worktree"], true);
    assert_eq!(json["data"]["restore"]["staged"], true);
    assert_eq!(json["data"]["restore"]["restored_files"][0], "tracked.txt");
    assert!(output.stderr.is_empty());

    let content = std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap();
    assert_eq!(content, "tracked\n");

    let status = run_libra_command(&["status", "--porcelain"], repo.path());
    assert_cli_success(&status, "status --porcelain");
    assert!(
        String::from_utf8_lossy(&status.stdout).trim().is_empty(),
        "checkout HEAD -- path should leave index and worktree clean, got: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

#[test]
fn test_checkout_machine_separator_path_outputs_single_json_line() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "worktree edit\n").unwrap();

    let output = run_libra_command(&["--machine", "checkout", "--", "tracked.txt"], repo.path());
    assert_cli_success(&output, "machine checkout -- tracked.txt");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line, got: {stdout}"
    );
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("expected JSON");
    assert_eq!(json["command"], "checkout");
    assert_eq!(json["data"]["action"], "restore-paths");
    assert_eq!(json["data"]["restore"]["source"], serde_json::Value::Null);
    assert_eq!(json["data"]["restore"]["worktree"], true);
    assert_eq!(json["data"]["restore"]["staged"], false);
    assert!(output.stderr.is_empty());
}

#[test]
fn test_checkout_machine_outputs_single_json_line() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

    let repo = create_committed_repo_via_cli();
    let branch = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&branch, "branch feature");

    let output = run_libra_command(&["--machine", "checkout", "feature"], repo.path());
    assert_cli_success(&output, "machine checkout feature");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line, got: {stdout}"
    );
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("expected JSON");
    assert_eq!(json["command"], "checkout");
    assert_eq!(json["data"]["action"], "switch");
    assert_eq!(json["data"]["branch"], "feature");
    assert!(output.stderr.is_empty());
}

#[test]
fn test_checkout_json_missing_branch_reports_invalid_target() {
    use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};

    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "checkout", "missing"], repo.path());

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let (_human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        report
            .message
            .contains("path specification 'missing' did not match")
    );
}

#[test]
fn test_checkout_json_reserved_branch_reports_invalid_target() {
    use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};

    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "checkout", "intent"], repo.path());

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let (_human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(report.message.contains("checking out 'intent' branch"));
}

/// opencode.md OC-Phase 3 acceptance criterion 5 requires that
/// `checkout` refuse to route user work onto `agent-traces`, the same
/// way it already refuses `intent`. The branch is reserved for the
/// external-agent capture subsystem (CEX-EntireIO) and any user-driven
/// checkout that lands on it would let `restore` / `reset` rewind
/// working state to AI-managed commits.
#[test]
fn test_checkout_agent_traces_branch_reports_invalid_target() {
    use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};

    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "checkout", "agent-traces"], repo.path());

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let (_human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        report.message.contains("'agent-traces'"),
        "error message must name the agent-traces branch verbatim, got: {}",
        report.message,
    );
}

/// Counterpart that exercises the create-new-branch path: `checkout -b
/// agent-traces` must fail, otherwise a user (or stray AI agent) could
/// clobber the reserved capture ref by creating a same-named local
/// branch and pushing it.
#[test]
fn test_checkout_create_agent_traces_branch_is_blocked() {
    use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};

    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "checkout", "-b", "agent-traces"], repo.path());

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let (_human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert!(
        report.message.contains("'agent-traces'"),
        "error message must name the agent-traces branch verbatim, got: {}",
        report.message,
    );
}

// ── Batch 1: --ours / --theirs conflict-path checkout + -f/--force ──

/// Build a repo paused mid-merge with a 3-way conflict on each of `files`.
/// ours (main / stage #2) = `ours-<name>\n`; theirs (feature / stage #3) =
/// `theirs-<name>\n`; common base = `base\n`. When `executable`, each file is
/// mode `0o755` before every `add` (unix only), so the conflict stages carry the
/// executable bit.
fn create_conflict_repo(files: &[&str], executable: bool) -> tempfile::TempDir {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let p = repo.path().to_path_buf();

    let write = |name: &str, content: &str| {
        let fp = p.join(name);
        std::fs::write(&fp, content).expect("write conflict file");
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fp, std::fs::Permissions::from_mode(0o755))
                .expect("chmod conflict file");
        }
    };
    let add = |name: &str| {
        assert_cli_success(&run_libra_command(&["add", name], &p), "add conflict file");
    };
    let commit = |msg: &str| {
        assert_cli_success(
            &run_libra_command(&["commit", "-m", msg, "--no-verify"], &p),
            "commit conflict file",
        );
    };

    for name in files {
        write(name, "base\n");
        add(name);
    }
    commit("conflict base");
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], &p),
        "branch feature",
    );

    for name in files {
        write(name, &format!("ours-{name}\n"));
        add(name);
    }
    commit("main edit");

    assert_cli_success(
        &run_libra_command(&["switch", "feature"], &p),
        "switch feature",
    );
    for name in files {
        write(name, &format!("theirs-{name}\n"));
        add(name);
    }
    commit("feature edit");

    assert_cli_success(&run_libra_command(&["switch", "main"], &p), "switch main");
    let merge = run_libra_command(&["merge", "feature"], &p);
    assert!(
        !merge.status.success(),
        "expected merge conflict, got success: {}",
        String::from_utf8_lossy(&merge.stdout)
    );
    repo
}

/// Load the on-disk index for in-process stage/mode assertions.
fn load_index(repo: &std::path::Path) -> git_internal::internal::index::Index {
    git_internal::internal::index::Index::load(repo.join(".libra/index")).expect("load index")
}

/// `--ours -- <path>` restores the stage #2 (our side) content into the worktree.
#[test]
fn checkout_ours_restores_stage2() {
    use super::{assert_cli_success, run_libra_command};
    let repo = create_conflict_repo(&["conflict.txt"], false);
    assert_cli_success(
        &run_libra_command(&["checkout", "--ours", "--", "conflict.txt"], repo.path()),
        "checkout --ours",
    );
    let content = std::fs::read_to_string(repo.path().join("conflict.txt")).unwrap();
    assert_eq!(content, "ours-conflict.txt\n");
}

/// `--theirs -- <path>` restores the stage #3 (their side) content into the worktree.
#[test]
fn checkout_theirs_restores_stage3() {
    use super::{assert_cli_success, run_libra_command};
    let repo = create_conflict_repo(&["conflict.txt"], false);
    assert_cli_success(
        &run_libra_command(&["checkout", "--theirs", "--", "conflict.txt"], repo.path()),
        "checkout --theirs",
    );
    let content = std::fs::read_to_string(repo.path().join("conflict.txt")).unwrap();
    assert_eq!(content, "theirs-conflict.txt\n");
}

/// After `--ours`, the conflicted path collapses to a single stage #0 index entry.
#[test]
fn checkout_ours_clears_conflict_to_stage0() {
    use super::{assert_cli_success, run_libra_command};
    let repo = create_conflict_repo(&["conflict.txt"], false);
    // Sanity: the merge really left conflict stages behind.
    let before = load_index(repo.path());
    assert!(
        before.get("conflict.txt", 2).is_some() && before.get("conflict.txt", 3).is_some(),
        "fixture must start in a conflicted (stage 2/3) state",
    );

    assert_cli_success(
        &run_libra_command(&["checkout", "--ours", "--", "conflict.txt"], repo.path()),
        "checkout --ours",
    );

    let after = load_index(repo.path());
    assert!(
        after.get("conflict.txt", 0).is_some(),
        "stage 0 entry must exist after --ours",
    );
    assert!(
        after.get("conflict.txt", 1).is_none()
            && after.get("conflict.txt", 2).is_none()
            && after.get("conflict.txt", 3).is_none(),
        "all conflict stages (1/2/3) must be cleared after --ours",
    );
}

/// `--ours` on a path that is not in a merge conflict is rejected (128, LBR-CONFLICT-002).
#[test]
fn checkout_ours_on_non_conflict_path_errors() {
    use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["checkout", "--ours", "--", "tracked.txt"], repo.path());
    assert_eq!(out.status.code(), Some(128), "non-conflict --ours blocked");
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(
        report.message.contains("not in a merge conflict"),
        "message: {}",
        report.message
    );
}

/// `--ours` on a clean (non-conflicted) file errors WITHOUT silently rewriting it.
#[test]
fn checkout_ours_on_clean_file_no_silent_rewrite() {
    use super::{create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let before = std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap();
    let out = run_libra_command(&["checkout", "--ours", "--", "tracked.txt"], repo.path());
    assert_eq!(out.status.code(), Some(128));
    let after = std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap();
    assert_eq!(after, before, "clean file must not be rewritten on error");
}

/// `--ours --theirs` together is a usage conflict. Libra remaps clap's
/// `ArgumentConflict` (subcommand present) to `command_usage` (129), not clap's
/// native exit 2.
#[test]
fn checkout_ours_and_theirs_conflict_rejected_by_clap() {
    use super::{create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(
        &["checkout", "--ours", "--theirs", "--", "tracked.txt"],
        repo.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(129),
        "clap mode conflict → command_usage: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `--ours` without a pathspec is a usage error (CliInvalidArguments).
#[test]
fn checkout_ours_without_pathspec_errors() {
    use super::{create_committed_repo_via_cli, parse_cli_error_stderr, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["checkout", "--ours"], repo.path());
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        report.message.contains("requires a pathspec"),
        "message: {}",
        report.message
    );
}

/// Pin the exit code for `--ours`/`--theirs` without a pathspec at 129.
#[test]
fn checkout_ours_without_pathspec_exit_code_129() {
    use super::{create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(&["checkout", "--theirs"], repo.path());
    assert_eq!(out.status.code(), Some(129));
}

/// `-f` switching overwrites uncommitted changes that would otherwise block the switch.
#[test]
fn checkout_force_switch_overwrites_dirty_worktree() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    // A second branch where tracked.txt differs.
    assert_cli_success(&run_libra_command(&["branch", "other"], p), "branch other");
    assert_cli_success(&run_libra_command(&["switch", "other"], p), "switch other");
    std::fs::write(p.join("tracked.txt"), "other-content\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "tracked.txt"], p), "add other");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "other edit", "--no-verify"], p),
        "commit other",
    );
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");

    // Dirty the worktree, then a plain switch must be refused.
    std::fs::write(p.join("tracked.txt"), "dirty-uncommitted\n").unwrap();
    let blocked = run_libra_command(&["checkout", "other"], p);
    assert!(
        !blocked.status.success(),
        "plain checkout over a dirty file should be blocked",
    );

    // `-f` forces the switch through and overwrites the dirty edit.
    assert_cli_success(
        &run_libra_command(&["checkout", "-f", "other"], p),
        "checkout -f",
    );
    let content = std::fs::read_to_string(p.join("tracked.txt")).unwrap();
    assert_ne!(
        content, "dirty-uncommitted\n",
        "dirty edit must be overwritten"
    );
    assert_eq!(content, "other-content\n");
}

/// After a forced switch, the working tree is aligned to the target commit's tree.
#[test]
fn checkout_force_switch_aligns_worktree_to_target() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    assert_cli_success(&run_libra_command(&["branch", "other"], p), "branch other");
    assert_cli_success(&run_libra_command(&["switch", "other"], p), "switch other");
    std::fs::write(p.join("tracked.txt"), "target-tree\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "tracked.txt"], p), "add other");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "other", "--no-verify"], p),
        "commit other",
    );
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");
    std::fs::write(p.join("tracked.txt"), "scratch\n").unwrap();

    assert_cli_success(
        &run_libra_command(&["checkout", "--force", "other"], p),
        "checkout --force",
    );
    assert_eq!(
        std::fs::read_to_string(p.join("tracked.txt")).unwrap(),
        "target-tree\n",
    );
}

/// `--ours` promotes the chosen stage to stage #0 while preserving the index mode
/// (executable bit), proving the owned-entry path (not `new_from_blob`) is used.
#[cfg(unix)]
#[test]
fn checkout_ours_preserves_index_mode() {
    use super::{assert_cli_success, run_libra_command};
    let repo = create_conflict_repo(&["conflict.txt"], true);
    let before = load_index(repo.path());
    let stage2_mode = before.get("conflict.txt", 2).expect("stage 2 entry").mode;
    assert_eq!(stage2_mode, 0o100755, "fixture stage 2 must be executable");

    assert_cli_success(
        &run_libra_command(&["checkout", "--ours", "--", "conflict.txt"], repo.path()),
        "checkout --ours",
    );

    let after = load_index(repo.path());
    let stage0_mode = after.get("conflict.txt", 0).expect("stage 0 entry").mode;
    assert_eq!(
        stage0_mode, stage2_mode,
        "stage 0 mode must match the promoted stage 2 mode (executable bit preserved)",
    );
}

/// `--ours` handles multiple pathspecs in one invocation.
#[test]
fn checkout_ours_multiple_pathspecs() {
    use super::{assert_cli_success, run_libra_command};
    let repo = create_conflict_repo(&["a.txt", "b.txt"], false);
    assert_cli_success(
        &run_libra_command(&["checkout", "--ours", "--", "a.txt", "b.txt"], repo.path()),
        "checkout --ours a.txt b.txt",
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "ours-a.txt\n",
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("b.txt")).unwrap(),
        "ours-b.txt\n",
    );
}

/// A corrupt on-disk index propagates as a read failure (128, LBR-IO-001).
#[test]
fn checkout_ours_read_error_propagates() {
    use super::{parse_cli_error_stderr, run_libra_command};
    let repo = create_conflict_repo(&["conflict.txt"], false);
    std::fs::write(repo.path().join(".libra/index"), b"not a valid DIRC index").unwrap();
    let out = run_libra_command(&["checkout", "--ours", "--", "conflict.txt"], repo.path());
    assert_eq!(out.status.code(), Some(128), "corrupt index → read failure");
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-IO-001");
}

/// A worktree write failure (read-only target file) propagates as a write failure
/// (128, LBR-IO-002).
#[cfg(unix)]
#[test]
fn checkout_ours_write_error_propagates() {
    use std::os::unix::fs::PermissionsExt;

    use super::{parse_cli_error_stderr, run_libra_command, skip_permission_denied_test_if_root};
    if skip_permission_denied_test_if_root("checkout_ours_write_error_propagates") {
        return;
    }
    let repo = create_conflict_repo(&["conflict.txt"], false);
    let fp = repo.path().join("conflict.txt");
    std::fs::set_permissions(&fp, std::fs::Permissions::from_mode(0o444)).unwrap();

    let out = run_libra_command(&["checkout", "--ours", "--", "conflict.txt"], repo.path());

    // Restore perms so the TempDir can be cleaned up.
    let _ = std::fs::set_permissions(&fp, std::fs::Permissions::from_mode(0o644));

    assert_eq!(
        out.status.code(),
        Some(128),
        "read-only worktree file → write failure: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (_h, report) = parse_cli_error_stderr(&out.stderr);
    assert_eq!(report.error_code, "LBR-IO-002");
}
