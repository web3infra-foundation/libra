//! Tests checkout behavior for switching branches and creating new ones via restore logic.

use colored::Colorize;
use libra::{
    command::{
        add::{self, AddArgs},
        branch,
        checkout::{check_branch, get_current_branch, switch_branch},
        commit, init,
        restore::{self, RestoreArgs},
    },
    internal::{config::Config, head::Head},
    utils::{test, util},
};
use serial_test::serial;
use tempfile::tempdir;

async fn configure_identity_for_test() {
    Config::insert("user", None, "name", "Checkout Test User").await;
    Config::insert("user", None, "email", "checkout-test@example.com").await;
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
        separate_libra_dir: None,
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
    commit::execute_safe(commit_args)
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
        separate_libra_dir: None,
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
    commit::execute_safe(commit_args)
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
        separate_libra_dir: None,
    })
    .await
    .unwrap();
    configure_identity_for_test().await;

    // create and commit a file
    test::ensure_file("foo.txt", Some("v1"));
    add::execute_safe(AddArgs {
        pathspec: vec!["foo.txt".into()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await
    .expect("add should succeed");
    commit::execute_safe(commit::CommitArgs {
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
    })
    .await
    .expect("commit should succeed");

    // modify the file
    test::ensure_file("foo.txt", Some("v2"));

    // try to restore using a SHA-1 length hash in a SHA-256 repo; should no-op
    let _ = restore::execute_safe(RestoreArgs {
        worktree: true,
        staged: true,
        source: Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904".into()),
        pathspec: vec![util::working_dir_string()],
    })
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
        internal::config::Config,
        utils::test::{self, ChangeDirGuard},
    };

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    Config::insert("user", None, "name", "Checkout Tester").await;
    Config::insert("user", None, "email", "checkout@test.com").await;

    // Create initial commit so HEAD exists
    test::ensure_file("base.txt", Some("base"));
    add::execute_safe(AddArgs {
        pathspec: vec!["base.txt".into()],
        all: false,
        update: false,
        refresh: false,
        verbose: false,
        force: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await
    .expect("add should succeed");
    commit::execute_safe(commit::CommitArgs {
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
    })
    .await
    .expect("initial commit should succeed");

    // Stage a change without committing — working tree is "dirty"
    test::ensure_file("dirty.txt", Some("uncommitted"));
    add::execute_safe(AddArgs {
        pathspec: vec!["dirty.txt".into()],
        all: false,
        update: false,
        refresh: false,
        verbose: false,
        force: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await
    .expect("add dirty file should succeed");

    let result =
        checkout::execute_safe(CheckoutArgs::try_parse_from(["checkout", "-b", "new"]).unwrap())
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
        internal::config::Config,
        utils::test::{self, ChangeDirGuard},
    };

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    Config::insert("user", None, "name", "Checkout Tester").await;
    Config::insert("user", None, "email", "checkout@test.com").await;

    test::ensure_file("base.txt", Some("base"));
    add::execute_safe(AddArgs {
        pathspec: vec!["base.txt".into()],
        all: false,
        update: false,
        refresh: false,
        verbose: false,
        force: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await
    .expect("add should succeed");
    commit::execute_safe(commit::CommitArgs {
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
    })
    .await
    .expect("initial commit should succeed");

    // Dirty the worktree with unstaged content.
    test::ensure_file("base.txt", Some("base\nlocal change"));

    let current = get_current_branch()
        .await
        .expect("current branch should be present");
    let args = CheckoutArgs::try_parse_from(["checkout", current.as_str()]).unwrap();
    let result = checkout::execute_safe(args).await;
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
