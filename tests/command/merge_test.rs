//! Tests merge command scenarios including fast-forward handling and conflict reporting.

use std::process::Command;

use libra::{
    internal::{branch::Branch, head::Head},
    utils::test::ChangeDirGuard,
};
use serial_test::serial;

use super::{create_committed_repo_via_cli, run_libra_command};

#[test]
#[serial]
fn test_merge_cli_missing_branch_returns_error_1() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["merge", "no-such"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("error: no-such - not something we can merge"));
}

#[tokio::test]
/// Test fast-forward merge of local branches
async fn test_merge_fast_forward() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["branch", "feature"])
        .output()
        .expect("Failed to create branch");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["checkout", "feature"])
        .output()
        .expect("Failed to checkout branch");

    // Commit changes on the feature branch
    let file_path = temp_path.join("file.txt");
    std::fs::write(&file_path, "Feature content").expect("Failed to write file");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["add", "."])
        .output()
        .expect("Failed to add file");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["commit", "-m", "Add feature content"])
        .output()
        .expect("Failed to commit");

    // Switch back to the main branch and perform fast-forward merge

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["checkout", "main"])
        .output()
        .expect("Failed to checkout main branch");

    let merge_output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["merge", "feature"])
        .output()
        .expect("Failed to merge branch");
    assert!(
        merge_output.status.success(),
        "Fast-forward merge failed: {}",
        String::from_utf8_lossy(&merge_output.stderr)
    );
}

#[tokio::test]
#[serial]
/// Test merging a remote branch
async fn test_merge_remote_branch() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["branch", "feature"])
        .output()
        .expect("Failed to create branch");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["checkout", "feature"])
        .output()
        .expect("Failed to checkout feature branch");

    std::fs::write(temp_path.join("remote.txt"), "Remote content").expect("Failed to write file");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["add", "."])
        .output()
        .expect("Failed to add file");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["commit", "-m", "Add remote content"])
        .output()
        .expect("Failed to commit");

    let _guard = ChangeDirGuard::new(temp_path);
    let feature_commit = Head::current_commit()
        .await
        .expect("feature branch should have a tip");
    Branch::update_branch("feature", &feature_commit.to_string(), Some("origin")).await;

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["checkout", "main"])
        .output()
        .expect("Failed to checkout main branch");

    let merge_output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["merge", "origin/feature"])
        .output()
        .expect("Failed to merge remote branch");
    assert!(
        merge_output.status.success(),
        "Merge remote branch failed: {}",
        String::from_utf8_lossy(&merge_output.stderr)
    );
}

#[tokio::test]
/// Test merging diverged branches without fast-forward support.
async fn test_merge_diverged_branch_returns_fatal_128() {
    let temp_repo = create_committed_repo_via_cli();
    let temp_path = temp_repo.path();

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["branch", "branch1"])
        .output()
        .expect("Failed to create branch");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["checkout", "branch1"])
        .output()
        .expect("Failed to checkout branch");

    // Commit changes on branch1
    let branch1_file = temp_path.join("branch1.txt");
    std::fs::write(&branch1_file, "Branch1 content").expect("Failed to write file");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["add", "."])
        .output()
        .expect("Failed to add file");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["commit", "-m", "Add branch1 content"])
        .output()
        .expect("Failed to commit");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["checkout", "main"])
        .output()
        .expect("Failed to checkout main branch");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["branch", "branch2"])
        .output()
        .expect("Failed to create branch");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["checkout", "branch2"])
        .output()
        .expect("Failed to checkout branch");

    // Commit changes on branch2
    let branch2_file = temp_path.join("branch2.txt");
    std::fs::write(&branch2_file, "Branch2 content").expect("Failed to write file");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["add", "."])
        .output()
        .expect("Failed to add file");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["commit", "-m", "Add branch2 content"])
        .output()
        .expect("Failed to commit");

    Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["checkout", "branch1"])
        .output()
        .expect("Failed to checkout branch1");

    let merge_output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["merge", "branch2"])
        .output()
        .expect("Failed to merge branch");
    assert_eq!(merge_output.status.code(), Some(128));
    assert!(
        String::from_utf8_lossy(&merge_output.stderr)
            .contains("fatal: Not possible to fast-forward merge")
    );
}
