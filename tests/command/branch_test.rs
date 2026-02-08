//! Tests branch subcommand for creation, listing, deletion, and switching logic.

#![cfg(test)]

use libra::internal::config::Config;
use serial_test::serial;
use tempfile::tempdir;

use super::*;
#[tokio::test]
#[serial]
/// Tests core branch management functionality including creation and listing.
/// Verifies branches can be created from specific commits.
async fn test_branch() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let commit_args = CommitArgs {
        message: Some("first".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(commit_args).await;
    let first_commit_id = Branch::find_branch("master", None).await.unwrap().commit;

    let commit_args = CommitArgs {
        message: Some("second".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(commit_args).await;
    let second_commit_id = Branch::find_branch("master", None).await.unwrap().commit;

    {
        // create branch with first commit
        let first_branch_name = "first_branch".to_string();
        let args = BranchArgs {
            new_branch: Some(first_branch_name.clone()),
            commit_hash: Some(first_commit_id.to_string()),
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
        };
        execute(args).await;

        // check branch exist
        match Head::current().await {
            Head::Branch(current_branch) => {
                assert_ne!(current_branch, first_branch_name)
            }
            _ => panic!("should be branch"),
        };

        let first_branch = Branch::find_branch(&first_branch_name, None).await.unwrap();
        assert_eq!(first_branch.commit, first_commit_id);
        assert_eq!(first_branch.name, first_branch_name);
    }

    {
        // create second branch with current branch
        let second_branch_name = "second_branch".to_string();
        let args = BranchArgs {
            new_branch: Some(second_branch_name.clone()),
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
        };
        execute(args).await;
        let second_branch = Branch::find_branch(&second_branch_name, None)
            .await
            .unwrap();
        assert_eq!(second_branch.commit, second_commit_id);
        assert_eq!(second_branch.name, second_branch_name);
    }

    // show current branch
    println!("show current branch");
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        show_current: true,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
    };
    execute(args).await;

    // list branches
    println!("list branches");
    // execute(BranchArgs::parse_from([""])).await; // default list
}

#[tokio::test]
#[serial]
/// Tests branch creation using remote branches as starting points.
/// Verifies that local branches can be created from remote branch references.
async fn test_create_branch_from_remote() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    let args = CommitArgs {
        message: Some("first".to_string()),
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
    commit::execute(args).await;
    let hash = Head::current_commit().await.unwrap();
    Branch::update_branch("master", &hash.to_string(), Some("origin")).await; // create remote branch
    assert!(get_target_commit("origin/master").await.is_ok());

    let args = BranchArgs {
        new_branch: Some("test_new".to_string()),
        commit_hash: Some("origin/master".into()),
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
    };
    execute(args).await;

    let branch = Branch::find_branch("test_new", None)
        .await
        .expect("branch create failed found");
    assert_eq!(branch.commit, hash);
}

#[tokio::test]
#[serial]
/// Tests the behavior of creating a branch with an invalid name.
async fn test_invalid_branch_name() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    let args = CommitArgs {
        message: Some("first".to_string()),
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
    commit::execute(args).await;

    let args = BranchArgs {
        new_branch: Some("@{mega}".to_string()),
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
    };
    execute(args).await;

    let branch = Branch::find_branch("@{mega}", None).await;
    assert!(branch.is_none(), "invalid branch should not be created");
}

#[tokio::test]
#[serial]
/// Tests branch renaming functionality.
/// Verifies that branches can be renamed and HEAD is updated when renaming current branch.
async fn test_branch_rename() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    // Create initial commit
    let args = CommitArgs {
        message: Some("first".to_string()),
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
    commit::execute(args).await;
    let commit_id_1 = Head::current_commit().await.unwrap();

    // Create a test branch
    let args = BranchArgs {
        new_branch: Some("old_name".to_string()),
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
    };
    execute(args).await;

    // Verify old branch exists
    let old_branch = Branch::find_branch("old_name", None).await;
    assert!(old_branch.is_some(), "old branch should exist");
    assert_eq!(old_branch.unwrap().commit, commit_id_1);

    // Rename branch from old_name to new_name
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        show_current: false,
        rename: vec!["old_name".to_string(), "new_name".to_string()],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
    };
    execute(args).await;

    // Verify old branch no longer exists
    let old_branch = Branch::find_branch("old_name", None).await;
    assert!(
        old_branch.is_none(),
        "old branch should not exist after rename"
    );

    // Verify new branch exists with same commit
    let new_branch = Branch::find_branch("new_name", None).await;
    assert!(new_branch.is_some(), "new branch should exist");
    assert_eq!(new_branch.unwrap().commit, commit_id_1);
}

#[tokio::test]
#[serial]
/// Tests renaming the current branch.
/// Verifies that HEAD is updated when renaming the current branch.
async fn test_rename_current_branch() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    // Create initial commit
    let args = CommitArgs {
        message: Some("first".to_string()),
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
    commit::execute(args).await;
    let commit_id = Head::current_commit().await.unwrap();

    // Verify we're on master branch
    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, "master"),
        _ => panic!("should be on a branch"),
    }

    // Rename current branch (master) to main using single argument
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        show_current: false,
        rename: vec!["main".to_string()],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
    };
    execute(args).await;

    // Verify HEAD is now on 'main'
    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, "main"),
        _ => panic!("should be on a branch"),
    }

    // Verify old branch no longer exists
    let old_branch = Branch::find_branch("master", None).await;
    assert!(
        old_branch.is_none(),
        "master branch should not exist after rename"
    );

    // Verify new branch exists with same commit
    let new_branch = Branch::find_branch("main", None).await;
    assert!(new_branch.is_some(), "main branch should exist");
    assert_eq!(new_branch.unwrap().commit, commit_id);
}

#[tokio::test]
#[serial]
/// Tests that renaming to an existing branch name fails.
async fn test_rename_to_existing_branch() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    // Create initial commit
    let args = CommitArgs {
        message: Some("first".to_string()),
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
    commit::execute(args).await;

    // Create two branches
    let args = BranchArgs {
        new_branch: Some("branch1".to_string()),
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
    };
    execute(args).await;

    let args = BranchArgs {
        new_branch: Some("branch2".to_string()),
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
    };
    execute(args).await;

    // Try to rename branch1 to branch2 (should fail)
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        show_current: false,
        rename: vec!["branch1".to_string(), "branch2".to_string()],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
    };
    execute(args).await;

    // Verify both branches still exist
    assert!(Branch::find_branch("branch1", None).await.is_some());
    assert!(Branch::find_branch("branch2", None).await.is_some());
}

#[tokio::test]
#[serial]
/// Tests listing all branches (local + remote).
async fn test_list_all_branches() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    // Create initial commit
    let args = CommitArgs {
        message: Some("initial commit".to_string()),
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
    commit::execute(args).await;

    // Create local branch
    let args = BranchArgs {
        new_branch: Some("feature_branch".to_string()),
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
    };
    execute(args).await;

    Config::insert(
        "remote",
        Some("origin"),
        "url",
        "https://example.com/repo.git",
    )
    .await;

    // Create remote branch
    let hash = Head::current_commit().await.unwrap();
    Branch::update_branch("remote_branch", &hash.to_string(), Some("origin")).await;

    // Test -a parameter - just call execute, don't try to capture output
    let args = BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: None,
        set_upstream_to: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: true,
        contains: vec![],
        no_contains: vec![],
    };
    execute(args).await; // This will print to stdout, which is fine for tests

    // Verify branches exist
    assert!(
        Branch::find_branch("master", None).await.is_some()
            || Branch::find_branch("main", None).await.is_some()
    );
    assert!(Branch::find_branch("feature_branch", None).await.is_some());
    assert!(
        Branch::find_branch("remote_branch", Some("origin"))
            .await
            .is_some()
    );
}

#[tokio::test]
#[serial]
/// Tests safe delete (branch -d) functionality
/// Verifies that -d refuses to delete unmerged branches but allows merged ones
async fn test_branch_delete_safe() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create first commit on master
    let commit_args = CommitArgs {
        message: Some("initial commit".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(commit_args).await;

    // Create a feature branch
    execute(BranchArgs {
        new_branch: Some("feature".to_string()),
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

    // Switch to feature branch and make a commit
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    let commit_args = CommitArgs {
        message: Some("feature work".to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };
    commit::execute(commit_args).await;

    // Switch back to master
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    // Try to delete feature branch with -d (should fail - not merged)
    execute(BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: Some("feature".to_string()),
        set_upstream_to: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
    })
    .await;

    // Feature branch should still exist
    assert!(Branch::find_branch("feature", None).await.is_some());

    // Now merge feature into master
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    // Fast-forward merge (just update master to feature's commit)
    let feature_commit = Branch::find_branch("feature", None).await.unwrap().commit;
    Branch::update_branch("master", &feature_commit.to_string(), None).await;

    // Now try -d again (should succeed - fully merged)
    execute(BranchArgs {
        new_branch: None,
        commit_hash: None,
        list: false,
        delete: None,
        delete_safe: Some("feature".to_string()),
        set_upstream_to: None,
        show_current: false,
        rename: vec![],
        remotes: false,
        all: false,
        contains: vec![],
        no_contains: vec![],
    })
    .await;

    // Feature branch should be deleted
    assert!(Branch::find_branch("feature", None).await.is_none());
}

#[tokio::test]
#[serial]
/// Comprehensive tests for `branch --contains` and `branch --no-contains` filters.
///
/// Builds a classic divergent branch topology:
///
/// ```text
///   master:  base ← m1 ← m2
///             ↖
///   dev:        d1 ← d2
/// ```
///
/// Where:
/// - `base`: common ancestor, reachable from both branches
/// - `m1`, `m2`: commits unique to master
/// - `d1`, `d2`: commits unique to dev (d1 branches from base, d2 extends d1)
///
/// Tests cover:
/// 1. Single filters (`--contains` or `--no-contains` alone)
/// 2. Combined filters (`--contains` AND `--no-contains`)
/// 3. Multiple values (OR semantics for `--contains`, AND for `--no-contains`)
async fn test_branch_contains_commit_filter() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());
    test::init_debug_logger();

    let main_branch = match Head::current().await {
        Head::Branch(name) => name,
        _ => panic!("expected to start on a branch"),
    };

    let make_commit = |msg: &str| CommitArgs {
        message: Some(msg.to_string()),
        file: None,
        allow_empty: true,
        conventional: false,
        amend: false,
        no_edit: false,
        signoff: false,
        disable_pre: true,
        all: false,
        no_verify: false,
        author: None,
    };

    // ================================================================
    //  Build commit graph: divergent branches with shared ancestor
    // ================================================================

    // Common ancestor
    commit::execute(make_commit("base")).await;
    let base = Head::current_commit().await.unwrap().to_string();

    // Create dev branch and add two commits
    execute(BranchArgs {
        new_branch: Some("dev".to_string()),
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

    switch::execute(SwitchArgs {
        branch: Some("dev".to_string()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    commit::execute(make_commit("d1")).await;
    let d1 = Head::current_commit().await.unwrap().to_string();

    commit::execute(make_commit("d2")).await;
    let d2 = Head::current_commit().await.unwrap().to_string();

    // Return to main branch and add two commits
    switch::execute(SwitchArgs {
        branch: Some(main_branch.clone()),
        create: None,
        detach: false,
        track: false,
    })
    .await;

    commit::execute(make_commit("m1")).await;
    let m1 = Head::current_commit().await.unwrap().to_string();

    commit::execute(make_commit("m2")).await;
    let m2 = Head::current_commit().await.unwrap().to_string();

    // ── Helper: filter and return sorted branch names ──
    let run_filter = |contains: &[&str], no_contains: &[&str]| {
        let contains: Vec<String> = contains.iter().map(|s| s.to_string()).collect();
        let no_contains: Vec<String> = no_contains.iter().map(|s| s.to_string()).collect();
        async move {
            let mut branches = Branch::list_branches(None).await;
            filter_branches(&mut branches, &contains, &no_contains).await;
            let mut names: Vec<String> = branches.into_iter().map(|b| b.name).collect();
            names.sort();
            names
        }
    };

    let sorted = |names: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        v.sort();
        v
    };

    // ================================================================
    //  Test single `--contains` filter
    // ================================================================

    // Common ancestor is in both branches
    assert_eq!(
        run_filter(&[&base], &[]).await,
        sorted(&[&main_branch, "dev"]),
        "`--contains base` should match both branches"
    );

    // Branch-specific commits
    assert_eq!(
        run_filter(&[&d1], &[]).await,
        sorted(&["dev"]),
        "`--contains d1` should match only dev"
    );

    assert_eq!(
        run_filter(&[&d2], &[]).await,
        sorted(&["dev"]),
        "`--contains d2` (tip of dev) should match only dev"
    );

    assert_eq!(
        run_filter(&[&m1], &[]).await,
        sorted(&[&main_branch]),
        "`--contains m1` should match only master"
    );

    assert_eq!(
        run_filter(&[&m2], &[]).await,
        sorted(&[&main_branch]),
        "`--contains m2` (tip of master) should match only master"
    );

    // ================================================================
    //  Test single `--no-contains` filter
    // ================================================================

    // Excluding common ancestor filters out everything
    assert_eq!(
        run_filter(&[], &[&base]).await,
        sorted(&[]),
        "`--no-contains base` should match nothing"
    );

    // Excluding branch-specific commits
    assert_eq!(
        run_filter(&[], &[&d1]).await,
        sorted(&[&main_branch]),
        "`--no-contains d1` should match only master"
    );

    assert_eq!(
        run_filter(&[], &[&m1]).await,
        sorted(&["dev"]),
        "`--no-contains m1` should match only dev"
    );

    // ================================================================
    //  Test multiple `--contains` (OR semantics)
    // ================================================================

    // Any branch containing d1 OR m1
    assert_eq!(
        run_filter(&[&d1, &m1], &[]).await,
        sorted(&[&main_branch, "dev"]),
        "`--contains d1 --contains m1` should match both (OR)"
    );

    // Any branch containing d2 OR m2 (both tips)
    assert_eq!(
        run_filter(&[&d2, &m2], &[]).await,
        sorted(&[&main_branch, "dev"]),
        "`--contains d2 --contains m2` should match both (OR)"
    );

    // ================================================================
    //  Test multiple `--no-contains` (AND semantics)
    // ================================================================

    // Branches excluding both d1 AND m1 → none (each branch has one)
    assert_eq!(
        run_filter(&[], &[&d1, &m1]).await,
        sorted(&[]),
        "`--no-contains d1 --no-contains m1` should match nothing (each branch has one)"
    );

    // ================================================================
    //  Test combined `--contains` and `--no-contains`
    // ================================================================

    // Branches with base but not m1 → dev
    assert_eq!(
        run_filter(&[&base], &[&m1]).await,
        sorted(&["dev"]),
        "`--contains base --no-contains m1` should match dev"
    );

    // Branches with base but not d1 → master
    assert_eq!(
        run_filter(&[&base], &[&d1]).await,
        sorted(&[&main_branch]),
        "`--contains base --no-contains d1` should match master"
    );

    // Branches with base but not m2 → dev
    assert_eq!(
        run_filter(&[&base], &[&m2]).await,
        sorted(&["dev"]),
        "`--contains base --no-contains m2` should match dev"
    );

    // Branches with d1 OR m1, but not d2 → only master (dev is excluded by d2)
    assert_eq!(
        run_filter(&[&d1, &m1], &[&d2]).await,
        sorted(&[&main_branch]),
        "`--contains d1 --contains m1 --no-contains d2` should match master"
    );

    // Branches with d1 OR m1, but not m2 → only dev (master is excluded by m2)
    assert_eq!(
        run_filter(&[&d1, &m1], &[&m2]).await,
        sorted(&["dev"]),
        "`--contains d1 --contains m1 --no-contains m2` should match dev"
    );

    // ================================================================
    //  Test edge cases
    // ================================================================

    // Chain dependency: d2 contains d1, so `--contains d1 --no-contains d2` → empty
    assert_eq!(
        run_filter(&[&d1], &[&d2]).await,
        sorted(&[]),
        "`--contains d1 --no-contains d2` should match nothing (d2 contains d1)"
    );

    // Similarly for master chain
    assert_eq!(
        run_filter(&[&m1], &[&m2]).await,
        sorted(&[]),
        "`--contains m1 --no-contains m2` should match nothing (m2 contains m1)"
    );

    // Branches with base but excluding both tips → none
    assert_eq!(
        run_filter(&[&base], &[&d2, &m2]).await,
        sorted(&[]),
        "`--contains base --no-contains d2 --no-contains m2` should match nothing"
    );
}
