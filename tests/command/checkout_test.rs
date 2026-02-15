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
    internal::head::Head,
    utils::{test, util},
};
use serial_test::serial;
use tempfile::tempdir;
async fn test_check_branch() {
    println!("\n\x1b[1mTest check_branch function.\x1b[0m");

    // For non-existent branches, it should return None
    assert_eq!(check_branch("non_existent_branch").await, None);
    // For the current branch, it should return None
    assert_eq!(
        check_branch(&get_current_branch().await.unwrap_or("main".to_string())).await,
        None
    );
    // For other existing branches, it should return Some(false)
    assert_eq!(check_branch("new_branch_01").await, Some(false));
}

async fn test_switch_branch() {
    println!("\n\x1b[1mTest switch_branch function.\x1b[0m");

    let show_all_branches = async || {
        // Use the list_branches function of the branch module to list all current local branches
        branch::list_branches(branch::BranchListMode::Local, &[], &[]).await;
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
    switch_branch("new_branch_01").await;
    show_all_branches().await;
    switch_branch("new_branch_02").await;
    show_all_branches().await;
    switch_branch("main").await;
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
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: None,
        ref_format: None,
        separate_git_dir: None,
    };

    init::init(init_args)
        .await
        .expect("Error initializing repository");

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
    commit::execute(commit_args).await;

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
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: false,
        template: None,
        shared: None,
        object_format: Some("sha256".to_string()),
        ref_format: None,
        separate_git_dir: None,
    };

    init::init(init_args)
        .await
        .expect("Error initializing repository");

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
    commit::execute(commit_args).await;

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
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: true,
        template: None,
        shared: None,
        object_format: Some("sha256".to_string()),
        ref_format: None,
        separate_git_dir: None,
    })
    .await
    .unwrap();

    // create and commit a file
    test::ensure_file("foo.txt", Some("v1"));
    add::execute(AddArgs {
        pathspec: vec!["foo.txt".into()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;
    commit::execute(commit::CommitArgs {
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
    .await;

    // modify the file
    test::ensure_file("foo.txt", Some("v2"));

    // try to restore using a SHA-1 length hash in a SHA-256 repo; should no-op
    restore::execute(RestoreArgs {
        worktree: true,
        staged: true,
        source: Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904".into()),
        pathspec: vec![util::working_dir_string()],
    })
    .await;

    let content = std::fs::read_to_string(util::working_dir().join("foo.txt")).unwrap();
    assert_eq!(content, "v2", "invalid hash should not restore file");
}
