//! Tests restore command paths for worktree and index targets along with pathspec handling.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use libra::{
    internal::{db::get_db_conn_instance, head::Head, model::reference},
    utils::test::ChangeDirGuard,
};
use sea_orm::{ActiveModelTrait, Set};

use super::*;

#[test]
#[serial]
fn test_restore_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["restore", "."], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
#[serial]
fn test_restore_source_head_unborn_returns_error_without_falling_back() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());
    std::fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to write tracked file");

    let output = run_libra_command(&["restore", "--source", "HEAD", "tracked.txt"], repo.path());
    assert_eq!(output.status.code(), Some(128));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal:"), "unexpected stderr: {stderr}");

    let content = std::fs::read_to_string(repo.path().join("tracked.txt"))
        .expect("failed to read tracked file");
    assert_eq!(content, "modified\n");
}

#[tokio::test]
#[serial]
async fn test_restore_source_does_not_fall_back_from_unborn_branch_to_hash_prefix() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());

    let head_commit = Head::current_commit()
        .await
        .expect("expected committed repository");
    let branch_name = head_commit.to_string()[..7].to_string();

    let db = get_db_conn_instance().await;
    reference::ActiveModel {
        name: Set(Some(branch_name.clone())),
        kind: Set(reference::ConfigKind::Branch),
        commit: Set(None),
        remote: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("failed to insert unborn branch row");

    std::fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to modify tracked file");

    let output = run_libra_command(
        &["restore", "--source", &branch_name, "tracked.txt"],
        repo.path(),
    );
    assert!(
        !output.status.success(),
        "restore unexpectedly succeeded: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let content = std::fs::read_to_string(repo.path().join("tracked.txt"))
        .expect("failed to read tracked file");
    assert_eq!(
        content, "modified\n",
        "restore should not overwrite from hash fallback"
    );
}
