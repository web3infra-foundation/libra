//! Integration tests for the commit command covering staged changes, message handling, and tree/hash updates.

use libra::utils::object_ext::TreeExt;
use serial_test::serial;
use tempfile::tempdir;

use super::*;
#[tokio::test]
#[serial]
#[should_panic]
/// A commit with no file changes should fail if `allow_empty` is false.
/// This test verifies that the commit command rejects empty changesets
/// when not explicitly permitted.
async fn test_execute_commit_with_empty_index_fail() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let args = CommitArgs {
        message: Some("init".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
    };
    commit::execute(args).await;
}

#[tokio::test]
#[serial]
/// Tests normal commit functionality with both `--amend` and `--allow_empty` flags.
/// Verifies that:
/// 1. Amending works correctly when allowed
/// 2. Empty commits are permitted when explicitly enabled
async fn test_execute_commit() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    // create first empty commit
    {
        let args = CommitArgs {
            message: Some("init".to_string()),
            file: None,
            allow_empty: true,
            conventional: false,
            amend: false,
            signoff: false,
            disable_pre: true,
            all: false,
        };
        commit::execute(args).await;

        // check head branch exists
        let head = Head::current().await;
        let branch_name = match head {
            Head::Branch(name) => name,
            _ => panic!("head not in branch"),
        };
        let branch = Branch::find_branch(&branch_name, None).await.unwrap();
        let commit: Commit = load_object(&branch.commit).unwrap();

        assert_eq!(commit.message.trim(), "init");
        let branch = Branch::find_branch(&branch_name, None).await.unwrap();
        assert_eq!(branch.commit, commit.id);
    }

    // modify first empty commit
    {
        let args = CommitArgs {
            message: Some("init commit".to_string()),
            file: None,
            allow_empty: true,
            conventional: false,
            amend: true,
            signoff: false,
            disable_pre: true,
            all: false,
        };
        commit::execute(args).await;

        // check head branch exists
        let head = Head::current().await;
        let branch_name = match head {
            Head::Branch(name) => name,
            _ => panic!("head not in branch"),
        };
        let branch = Branch::find_branch(&branch_name, None).await.unwrap();
        let commit: Commit = load_object(&branch.commit).unwrap();

        assert_eq!(commit.message.trim(), "init commit");
        let branch = Branch::find_branch(&branch_name, None).await.unwrap();
        assert_eq!(branch.commit, commit.id);
    }

    // create a new commit
    {
        // create `a.txt` `bb/b.txt` `bb/c.txt`
        test::ensure_file("a.txt", Some("a"));
        test::ensure_file("bb/b.txt", Some("b"));
        test::ensure_file("bb/c.txt", Some("c"));
        let args = AddArgs {
            all: true,
            update: false,
            verbose: false,
            pathspec: vec![],
            dry_run: false,
            ignore_errors: false,
            refresh: false,
            force: false,
        };
        add::execute(args).await;
    }

    {
        let args = CommitArgs {
            message: Some("add some files".to_string()),
            file: None,
            allow_empty: false,
            conventional: false,
            amend: false,
            signoff: false,
            disable_pre: true,
            all: false,
        };
        commit::execute(args).await;

        let commit_id = Head::current_commit().await.unwrap();
        let commit: Commit = load_object(&commit_id).unwrap();
        assert_eq!(
            commit.message.trim(),
            "add some files",
            "{}",
            commit.message
        );

        let pre_commit_id = commit.parent_commit_ids[0];
        let pre_commit: Commit = load_object(&pre_commit_id).unwrap();
        assert_eq!(pre_commit.message.trim(), "init commit");

        let tree_id = commit.tree_id;
        let tree: Tree = load_object(&tree_id).unwrap();
        assert_eq!(tree.tree_items.len(), 2); // 2 subtree according to the test data
    }
    //modify new commit
    {
        let args = CommitArgs {
            message: Some("add some txt files".to_string()),
            file: None,
            allow_empty: true,
            conventional: false,
            amend: true,
            signoff: false,
            disable_pre: true,
            all: false,
        };
        commit::execute(args).await;

        let commit_id = Head::current_commit().await.unwrap();
        let commit: Commit = load_object(&commit_id).unwrap();
        assert_eq!(
            commit.message.trim(),
            "add some txt files",
            "{}",
            commit.message
        );

        let pre_commit_id = commit.parent_commit_ids[0];
        let pre_commit: Commit = load_object(&pre_commit_id).unwrap();
        assert_eq!(pre_commit.message.trim(), "init commit");

        let tree_id = commit.tree_id;
        let tree: Tree = load_object(&tree_id).unwrap();
        assert_eq!(tree.tree_items.len(), 2); // 2 subtree according to the test data
    }
}

#[tokio::test]
#[serial]
async fn test_commit_with_all_flag_stages_tracked_changes() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    test::ensure_file("tracked.txt", Some("v1"));
    add::execute(AddArgs {
        pathspec: vec!["tracked.txt".into()],
        all: false,
        update: false,
        refresh: false,
        verbose: false,
        force: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("initial".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
    })
    .await;

    test::ensure_file("tracked.txt", Some("updated"));
    test::ensure_file("new.txt", Some("untracked"));

    commit::execute(CommitArgs {
        message: Some("with -a".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: true,
    })
    .await;

    let head_id = Head::current_commit().await.unwrap();
    let commit: Commit = load_object(&head_id).unwrap();
    assert_eq!(commit.message.trim(), "with -a");
    let tree: Tree = load_object(&commit.tree_id).unwrap();
    let entries = tree.get_plain_items();
    let tracked_blob_hash = calc_file_blob_hash("tracked.txt").unwrap();
    let tracked_entry = entries
        .iter()
        .find(|(path, _)| path == &std::path::PathBuf::from("tracked.txt"))
        .expect("tracked file stored in commit");
    assert_eq!(tracked_entry.1, tracked_blob_hash);
    assert!(
        entries
            .iter()
            .all(|(path, _)| path != &std::path::PathBuf::from("new.txt")),
        "untracked files should not be auto-staged by -a"
    );
}

#[tokio::test]
#[serial]
async fn test_commit_with_all_flag_records_deletions() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    test::ensure_file("keep.txt", Some("keep"));
    add::execute(AddArgs {
        pathspec: vec!["keep.txt".into()],
        all: false,
        update: false,
        refresh: false,
        verbose: false,
        force: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("baseline".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
    })
    .await;

    std::fs::remove_file("keep.txt").unwrap();
    test::ensure_file("new_untracked.txt", Some("left alone"));

    commit::execute(CommitArgs {
        message: Some("remove tracked".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: true,
    })
    .await;

    let head_id = Head::current_commit().await.unwrap();
    let commit: Commit = load_object(&head_id).unwrap();
    assert_eq!(commit.message.trim(), "remove tracked");
    let tree: Tree = load_object(&commit.tree_id).unwrap();
    let entries = tree.get_plain_items();
    assert!(
        entries
            .iter()
            .all(|(path, _)| path != &std::path::PathBuf::from("keep.txt")),
        "deleted tracked files should be removed from commit"
    );
    assert!(
        entries
            .iter()
            .all(|(path, _)| path != &std::path::PathBuf::from("new_untracked.txt")),
        "new untracked files should still be absent"
    );
}

#[tokio::test]
#[serial]
/// Verifies commit and amend operations in a SHA-256 repository.
async fn test_commit_sha256() {
    let temp_path = tempdir().unwrap();
    test::setup_clean_testing_env_in(temp_path.path());
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Initialize a repository with SHA-256 object format
    init(InitArgs {
        bare: false,
        initial_branch: Some("main".to_string()),
        repo_directory: temp_path.path().to_str().unwrap().to_string(),
        quiet: true,
        template: None,
        shared: None,
        object_format: Some("sha256".to_string()),
    })
    .await
    .unwrap();

    // Create and add a file
    test::ensure_file("a.txt", Some("hello sha256"));
    add::execute(AddArgs {
        pathspec: vec!["a.txt".to_string()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
    })
    .await;

    // Create the first commit
    commit::execute(CommitArgs {
        message: Some("first sha256 commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: true,
        all: false,
    })
    .await;

    // Verify the commit hash is SHA-256 (64 hex characters)
    let head_commit = Head::current_commit().await.expect("HEAD missing");
    assert_eq!(
        head_commit.to_string().len(),
        64,
        "Commit hash should be SHA-256"
    );

    // Amend the commit
    commit::execute(CommitArgs {
        message: Some("amended sha256 commit".to_string()),
        file: None,
        allow_empty: true, // allow_empty is needed for amend if no new changes are staged
        conventional: false,
        amend: true,
        signoff: false,
        disable_pre: true,
        all: false,
    })
    .await;

    // Verify the amended commit hash is also SHA-256
    let amended_commit = Head::current_commit().await.expect("Amended HEAD missing");
    assert_eq!(
        amended_commit.to_string().len(),
        64,
        "Amended commit hash should be SHA-256"
    );
    assert_ne!(
        head_commit, amended_commit,
        "Amend should create a new commit"
    );
}
