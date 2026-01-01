//! Tests rebase command applying commits onto new bases and handling conflicts.

#![cfg(test)]
use std::fs;

use libra::command::rebase::{RebaseArgs, execute};
use libra::common_utils::parse_commit_msg;
use serial_test::serial;
use tempfile::tempdir;

use super::*;

fn commit_messages_from_head(start: &ObjectHash, max: usize) -> Vec<String> {
    let mut messages = Vec::new();
    let mut current = Some(*start);
    while let Some(hash) = current {
        let commit = load_object::<Commit>(&hash).unwrap();
        let (message, _) = parse_commit_msg(&commit.message);
        messages.push(message.trim().to_string());

        current = commit.parent_commit_ids.first().copied();
        if messages.len() >= max {
            break;
        }
    }
    messages
}

#[tokio::test]
#[serial]
async fn test_basic_rebase() {
    use libra::internal::head::Head;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // 1. Create initial commits on master
    fs::write(temp_path.path().join("file.txt"), "content1").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("C1: Add file.txt on master".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    fs::write(temp_path.path().join("file.txt"), "content1\ncontent2").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("C2: Modify file.txt on master".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 2. Create and switch to feature branch
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    // 3. Create commits on feature branch
    fs::write(temp_path.path().join("feature_a.txt"), "featureA").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["feature_a.txt".to_string()],
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
        message: Some("F1: Add feature_a.txt on feature branch".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    fs::write(temp_path.path().join("feature_b.txt"), "featureB").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["feature_b.txt".to_string()],
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
        message: Some("F2: Add feature_b.txt on feature branch".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 4. Switch back to master and make it diverge
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("master_only.txt"), "master_change").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["master_only.txt".to_string()],
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
        message: Some("C3: Add master_only.txt on master".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 5. Switch back to feature and perform rebase
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    // 6. Verify the rebase result
    // Check that all files exist after rebase
    assert!(temp_path.path().join("file.txt").exists());
    assert!(temp_path.path().join("feature_a.txt").exists());
    assert!(temp_path.path().join("feature_b.txt").exists());
    assert!(temp_path.path().join("master_only.txt").exists());

    // Check file contents
    assert_eq!(
        fs::read_to_string(temp_path.path().join("file.txt")).unwrap(),
        "content1\ncontent2"
    );
    assert_eq!(
        fs::read_to_string(temp_path.path().join("feature_a.txt")).unwrap(),
        "featureA"
    );
    assert_eq!(
        fs::read_to_string(temp_path.path().join("feature_b.txt")).unwrap(),
        "featureB"
    );
    assert_eq!(
        fs::read_to_string(temp_path.path().join("master_only.txt")).unwrap(),
        "master_change"
    );

    let head_commit = Head::current_commit().await.expect("expected HEAD commit");
    let messages = commit_messages_from_head(&head_commit, 5);
    assert_eq!(
        messages,
        vec![
            "F2: Add feature_b.txt on feature branch",
            "F1: Add feature_a.txt on feature branch",
            "C3: Add master_only.txt on master",
            "C2: Modify file.txt on master",
            "C1: Add file.txt on master"
        ]
    );
}

#[tokio::test]
#[serial]
async fn test_rebase_already_up_to_date() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create commits on master
    fs::write(temp_path.path().join("file1.txt"), "content1").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file1.txt".to_string()],
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
        message: Some("First commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    fs::write(temp_path.path().join("file2.txt"), "content2").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file2.txt".to_string()],
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
        message: Some("Second commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Create feature branch from current master (no divergence)
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    // Try to rebase feature onto master (should be up to date)
    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    // Should complete without errors (already up to date)
}

#[tokio::test]
#[serial]
async fn test_rebase_abort_when_no_rebase_in_progress() {
    use libra::command::rebase::RebaseState;
    use libra::internal::head::Head;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create initial commit on master
    fs::write(temp_path.path().join("file.txt"), "base content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("Initial commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Create feature branch and make a commit
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("feature.txt"), "feature content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["feature.txt".to_string()],
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
        message: Some("Feature commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Switch back to master and make a conflicting commit
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("master.txt"), "master content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["master.txt".to_string()],
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
        message: Some("Master commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Switch back to feature
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    // Start rebase
    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    let head_before_abort = Head::current_commit().await.expect("expected HEAD commit");
    let messages_before_abort = commit_messages_from_head(&head_before_abort, 3);
    assert_eq!(
        messages_before_abort,
        vec!["Feature commit", "Master commit", "Initial commit"]
    );

    // Rebase should complete (no conflict in this case)
    // But let's test abort when no rebase is in progress
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: false,
        abort: true,
        skip: false,
    })
    .await;

    let head_after_abort = Head::current_commit().await.expect("expected HEAD commit");
    assert_eq!(
        head_after_abort, head_before_abort,
        "Abort without rebase should not move HEAD"
    );
    let messages_after_abort = commit_messages_from_head(&head_after_abort, 3);
    assert_eq!(messages_after_abort, messages_before_abort);

    // Should handle gracefully (no rebase in progress)
    assert!(!RebaseState::is_in_progress().await.expect("failed to query rebase state"));
}

#[tokio::test]
#[serial]
async fn test_rebase_continue_no_rebase() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create initial commit
    fs::write(temp_path.path().join("file.txt"), "content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("Initial commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Try to continue when no rebase is in progress
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: true,
        abort: false,
        skip: false,
    })
    .await;

    // Should handle gracefully (outputs error message)
}

#[tokio::test]
#[serial]
async fn test_rebase_skip_no_rebase() {
    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Create initial commit
    fs::write(temp_path.path().join("file.txt"), "content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("Initial commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Try to skip when no rebase is in progress
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: false,
        abort: false,
        skip: true,
    })
    .await;

    // Should handle gracefully (outputs error message)
}

#[tokio::test]
#[serial]
async fn test_rebase_with_conflict_and_abort() {
    use libra::command::rebase::RebaseState;
    use libra::internal::head::Head;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // 1. Create initial commit on master with a file
    fs::write(temp_path.path().join("conflict.txt"), "base content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Base commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 2. Create feature branch and modify the file
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    fs::write(
        temp_path.path().join("conflict.txt"),
        "feature modification",
    )
    .unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Feature modifies conflict.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 3. Switch to master and make a conflicting modification
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("conflict.txt"), "master modification").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Master modifies conflict.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 4. Switch back to feature and attempt rebase (should conflict)
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    // 5. Rebase should be in progress (conflict should have stopped it)
    assert!(
        RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Expected conflict to stop rebase"
    );

    // Verify conflict markers in file
    let content = fs::read_to_string(temp_path.path().join("conflict.txt")).unwrap();
    assert!(
        content.contains("<<<<<<<") || content.contains("=======") || content.contains(">>>>>>>"),
        "Expected conflict markers in file"
    );

    // 6. Abort the rebase
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: false,
        abort: true,
        skip: false,
    })
    .await;

    // 7. Verify rebase is no longer in progress
    assert!(
        !RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Rebase should not be in progress after abort"
    );

    // 8. Verify we're back on feature branch
    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, "feature", "Should be back on feature branch"),
        _ => panic!("Should be on a branch after abort"),
    }

    // 9. Verify file content is restored to feature branch version
    let restored_content = fs::read_to_string(temp_path.path().join("conflict.txt")).unwrap();
    assert_eq!(
        restored_content, "feature modification",
        "File should be restored to feature branch content after abort"
    );

    let head_commit = Head::current_commit().await.expect("expected HEAD commit");
    let messages = commit_messages_from_head(&head_commit, 2);
    assert_eq!(
        messages,
        vec!["Feature modifies conflict.txt", "Base commit"]
    );
}

#[tokio::test]
#[serial]
async fn test_rebase_binary_conflict_skips_markers() {
    use libra::command::rebase::RebaseState;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    let file_path = temp_path.path().join("binary.bin");
    let base_bytes = vec![0x00, 0xFF, 0x01, 0x02];
    let feature_bytes = vec![0x10, 0xFF, 0x20, 0x21];
    let master_bytes = vec![0x30, 0xFF, 0x40, 0x41];

    // 1. Base commit on master with binary content
    fs::write(&file_path, &base_bytes).unwrap();
    add::execute(AddArgs {
        pathspec: vec!["binary.bin".to_string()],
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
        message: Some("Base binary".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 2. Feature branch modifies binary content
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;
    fs::write(&file_path, &feature_bytes).unwrap();
    add::execute(AddArgs {
        pathspec: vec!["binary.bin".to_string()],
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
        message: Some("Feature binary".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 3. Master modifies binary content differently
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;
    fs::write(&file_path, &master_bytes).unwrap();
    add::execute(AddArgs {
        pathspec: vec!["binary.bin".to_string()],
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
        message: Some("Master binary".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 4. Rebase feature onto master (should conflict)
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;
    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    assert!(
        RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Expected conflict to stop rebase"
    );

    // Binary conflict should not get marker text; keep feature bytes as-is.
    let current = fs::read(&file_path).unwrap();
    assert_eq!(current, feature_bytes, "Binary content should be unchanged");

    // Cleanup: abort rebase
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: false,
        abort: true,
        skip: false,
    })
    .await;
    assert!(
        !RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Rebase should not be in progress after abort"
    );
}

#[tokio::test]
#[serial]
async fn test_rebase_with_conflict_and_skip() {
    use libra::command::rebase::RebaseState;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // 1. Create initial commit on master
    fs::write(temp_path.path().join("conflict.txt"), "base content").unwrap();
    fs::write(temp_path.path().join("other.txt"), "other base").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string(), "other.txt".to_string()],
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
        message: Some("Base commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 2. Create feature branch with two commits
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    // First feature commit - will conflict
    fs::write(
        temp_path.path().join("conflict.txt"),
        "feature modification",
    )
    .unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Feature commit 1 - conflicts".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Second feature commit - no conflict
    fs::write(
        temp_path.path().join("feature_only.txt"),
        "feature only content",
    )
    .unwrap();
    add::execute(AddArgs {
        pathspec: vec!["feature_only.txt".to_string()],
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
        message: Some("Feature commit 2 - no conflict".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 3. Switch to master and make a conflicting change
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("conflict.txt"), "master modification").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Master modifies conflict.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 4. Switch back to feature and attempt rebase
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    // 5. Rebase should stop due to conflict; skip the conflicting commit
    assert!(
        RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Expected conflict to stop rebase"
    );

    execute(RebaseArgs {
        upstream: None,
        continue_rebase: false,
        abort: false,
        skip: true,
    })
    .await;

    // After skip, rebase should complete and apply the non-conflicting commit
    assert!(
        !RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Rebase should complete after skip"
    );
    assert!(
        temp_path.path().join("feature_only.txt").exists(),
        "feature_only.txt should exist after skip and continue"
    );
}

#[tokio::test]
#[serial]
async fn test_rebase_with_conflict_and_continue() {
    use libra::command::rebase::RebaseState;
    use libra::internal::head::Head;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // 1. Create initial commit on master
    fs::write(temp_path.path().join("conflict.txt"), "base content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Base commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 2. Create feature branch and modify the file
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    fs::write(
        temp_path.path().join("conflict.txt"),
        "feature modification",
    )
    .unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Feature modifies conflict.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 3. Switch to master and make a conflicting modification
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("conflict.txt"), "master modification").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Master modifies conflict.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 4. Switch back to feature and attempt rebase
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    // 5. Rebase should stop due to conflict, resolve it and continue
    assert!(
        RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Expected conflict to stop rebase"
    );

    // Resolve the conflict by writing merged content
    fs::write(
        temp_path.path().join("conflict.txt"),
        "merged content from both branches",
    )
    .unwrap();

    // Stage the resolved file
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    })
    .await;

    // Continue the rebase
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: true,
        abort: false,
        skip: false,
    })
    .await;

    // Verify rebase completed
    assert!(
        !RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Rebase should be complete after continue"
    );

    // Verify the merged content
    let final_content = fs::read_to_string(temp_path.path().join("conflict.txt")).unwrap();
    assert_eq!(
        final_content, "merged content from both branches",
        "File should contain merged content"
    );

    // Verify we're on feature branch
    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, "feature", "Should be on feature branch"),
        _ => panic!("Should be on a branch after rebase"),
    }
}

#[tokio::test]
#[serial]
async fn test_rebase_multiple_commits_partial_conflict() {
    use libra::command::rebase::RebaseState;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // 1. Create initial commit on master
    fs::write(temp_path.path().join("file1.txt"), "base1").unwrap();
    fs::write(temp_path.path().join("file2.txt"), "base2").unwrap();
    fs::write(temp_path.path().join("file3.txt"), "base3").unwrap();
    add::execute(AddArgs {
        pathspec: vec![
            "file1.txt".to_string(),
            "file2.txt".to_string(),
            "file3.txt".to_string(),
        ],
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
        message: Some("Base commit with 3 files".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 2. Create feature branch with 3 commits
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    // Commit 1: modify file1 (will conflict)
    fs::write(temp_path.path().join("file1.txt"), "feature1").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file1.txt".to_string()],
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
        message: Some("F1: modify file1".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Commit 2: add new file (no conflict)
    fs::write(
        temp_path.path().join("new_feature.txt"),
        "new feature content",
    )
    .unwrap();
    add::execute(AddArgs {
        pathspec: vec!["new_feature.txt".to_string()],
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
        message: Some("F2: add new_feature.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Commit 3: modify file3 (no conflict)
    fs::write(temp_path.path().join("file3.txt"), "feature3").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file3.txt".to_string()],
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
        message: Some("F3: modify file3".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 3. Switch to master and make conflicting change to file1
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("file1.txt"), "master1").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file1.txt".to_string()],
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
        message: Some("M1: modify file1".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 4. Switch back to feature and attempt rebase
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    // 5. Handle conflicts - skip the first conflicting commit
    assert!(
        RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Expected conflict to stop rebase"
    );

    // Skip the conflicting commit (F1)
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: false,
        abort: false,
        skip: true,
    })
    .await;

    // The remaining commits (F2 and F3) should apply without conflict
    assert!(
        !RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Rebase should complete after skip"
    );

    // Verify file1 has master's content (since we skipped feature's change)
    let file1_content = fs::read_to_string(temp_path.path().join("file1.txt")).unwrap();
    assert_eq!(
        file1_content, "master1",
        "file1 should have master's content after skip"
    );

    // Verify new_feature.txt exists (from F2)
    assert!(
        temp_path.path().join("new_feature.txt").exists(),
        "new_feature.txt should exist from commit F2"
    );

    // Verify file3 has feature's content (from F3)
    let file3_content = fs::read_to_string(temp_path.path().join("file3.txt")).unwrap();
    assert_eq!(
        file3_content, "feature3",
        "file3 should have feature's content from F3"
    );
}

#[tokio::test]
#[serial]
async fn test_rebase_state_persistence() {
    use libra::command::rebase::RebaseState;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // 1. Create initial commit
    fs::write(temp_path.path().join("file.txt"), "base").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("Base".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 2. Create feature branch with conflicting change
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("file.txt"), "feature").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("Feature".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 3. Create conflicting change on master
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("file.txt"), "master").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("Master".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // 4. Start rebase
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    // 5. Check state persistence
    assert!(
        RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Expected conflict to stop rebase"
    );

    // Verify legacy state files are not created
    let rebase_dir = temp_path.path().join(".libra/rebase-merge");
    assert!(
        !rebase_dir.exists(),
        "legacy rebase-merge directory should not be created"
    );

    // Load and verify state
    let state = RebaseState::load().await.expect("Should be able to load state");
    assert_eq!(state.head_name, "feature", "head_name should be 'feature'");
    assert!(
        state.stopped_sha.is_some(),
        "stopped_sha should be set during conflict"
    );

    // Clean up - abort the rebase
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: false,
        abort: true,
        skip: false,
    })
    .await;

    // Verify state is cleaned up
    assert!(
        !RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Rebase state should be cleaned up after abort"
    );
    assert!(
        !rebase_dir.exists(),
        "legacy rebase-merge directory should not exist after abort"
    );
}

#[tokio::test]
#[serial]
async fn test_rebase_fast_forward_branch_behind() {
    use libra::internal::head::Head;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Initial commit on master
    fs::write(temp_path.path().join("file.txt"), "base").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("Base commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Create feature branch at the same commit
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    // Advance master by one commit
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("file.txt"), "master-advance").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("Advance master".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    let master_head = Head::current_commit().await.unwrap();

    // Rebase feature onto master (fast-forward)
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, "feature", "Should be on feature branch"),
        _ => panic!("Should be on a branch after fast-forward"),
    }

    let feature_head = Head::current_commit().await.unwrap();
    assert_eq!(
        feature_head, master_head,
        "Feature should fast-forward to master"
    );

    let content = fs::read_to_string(temp_path.path().join("file.txt")).unwrap();
    assert_eq!(content, "master-advance");
}

#[tokio::test]
#[serial]
async fn test_rebase_fast_forward_blocks_dirty_workdir() {
    use libra::internal::head::Head;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Base commit on master
    fs::write(temp_path.path().join("file.txt"), "base").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("Base commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Create feature branch at base
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    // Advance master by one commit
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("file.txt"), "master-advance").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".to_string()],
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
        message: Some("Advance master".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Switch to feature and introduce a dirty tracked file
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    let feature_head = Head::current_commit().await.unwrap();
    fs::write(temp_path.path().join("file.txt"), "local-modification").unwrap();

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, "feature", "Should stay on feature branch"),
        _ => panic!("Should be on a branch after failed fast-forward"),
    }

    let feature_head_after = Head::current_commit().await.unwrap();
    assert_eq!(
        feature_head_after, feature_head,
        "Feature should not move with dirty workdir"
    );

    let content = fs::read_to_string(temp_path.path().join("file.txt")).unwrap();
    assert_eq!(content, "local-modification");
}

#[tokio::test]
#[serial]
async fn test_rebase_fast_forward_blocks_untracked_overwrite() {
    use libra::internal::head::Head;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Base commit on master
    fs::write(temp_path.path().join("base.txt"), "base").unwrap();
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
        message: Some("Base commit".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Create feature branch at base
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    // Advance master with a new file that will conflict with untracked
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("new.txt"), "master-content").unwrap();
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
    commit::execute(CommitArgs {
        message: Some("Add new.txt".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Switch to feature and create untracked file that would be overwritten
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    let feature_head = Head::current_commit().await.unwrap();
    fs::write(temp_path.path().join("new.txt"), "local-untracked").unwrap();

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    match Head::current().await {
        Head::Branch(name) => assert_eq!(name, "feature", "Should stay on feature branch"),
        _ => panic!("Should be on a branch after failed fast-forward"),
    }

    let feature_head_after = Head::current_commit().await.unwrap();
    assert_eq!(
        feature_head_after, feature_head,
        "Feature should not move when untracked would be overwritten"
    );

    let content = fs::read_to_string(temp_path.path().join("new.txt")).unwrap();
    assert_eq!(content, "local-untracked");
}

#[tokio::test]
#[serial]
async fn test_rebase_conflict_preserves_non_conflicting_workdir() {
    use libra::command::rebase::RebaseState;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Base commit on master
    fs::write(temp_path.path().join("conflict.txt"), "base").unwrap();
    fs::write(temp_path.path().join("clean.txt"), "base-clean").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string(), "clean.txt".to_string()],
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
        message: Some("Base".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Feature commit modifies both files (conflict + clean)
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("conflict.txt"), "feature-conflict").unwrap();
    fs::write(temp_path.path().join("clean.txt"), "feature-clean").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string(), "clean.txt".to_string()],
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
        message: Some("Feature changes".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Master conflicting change
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("conflict.txt"), "master-conflict").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Master conflict".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Rebase feature onto master; should stop with conflict
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    assert!(
        RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Expected conflict to stop rebase"
    );

    let clean_content = fs::read_to_string(temp_path.path().join("clean.txt")).unwrap();
    assert_eq!(
        clean_content, "feature-clean",
        "Non-conflicting file should be updated in workdir"
    );

    // Clean up
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: false,
        abort: true,
        skip: false,
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_rebase_continue_requires_resolution() {
    use libra::command::rebase::RebaseState;
    use libra::internal::head::Head;

    let temp_path = tempdir().unwrap();
    test::setup_with_new_libra_in(temp_path.path()).await;
    let _guard = ChangeDirGuard::new(temp_path.path());

    // Base commit on master
    fs::write(temp_path.path().join("conflict.txt"), "base").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Base".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Feature commit
    switch::execute(SwitchArgs {
        branch: None,
        create: Some("feature".to_string()),
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("conflict.txt"), "feature").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Feature".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Master conflict
    switch::execute(SwitchArgs {
        branch: Some("master".to_string()),
        create: None,
        detach: false,
    })
    .await;

    fs::write(temp_path.path().join("conflict.txt"), "master").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["conflict.txt".to_string()],
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
        message: Some("Master".to_string()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    })
    .await;

    // Start rebase
    switch::execute(SwitchArgs {
        branch: Some("feature".to_string()),
        create: None,
        detach: false,
    })
    .await;

    execute(RebaseArgs {
        upstream: Some("master".to_string()),
        continue_rebase: false,
        abort: false,
        skip: false,
    })
    .await;

    assert!(
        RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Expected conflict to stop rebase"
    );

    let head_before = Head::current_commit().await.unwrap();

    // Continue without resolving conflicts
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: true,
        abort: false,
        skip: false,
    })
    .await;

    assert!(
        RebaseState::is_in_progress().await.expect("failed to query rebase state"),
        "Rebase should remain in progress after unresolved continue"
    );
    let head_after = Head::current_commit().await.unwrap();
    assert_eq!(head_before, head_after, "HEAD should not move");

    // Clean up
    execute(RebaseArgs {
        upstream: None,
        continue_rebase: false,
        abort: true,
        skip: false,
    })
    .await;
}
