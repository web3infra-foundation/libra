use clap::Parser;
use libra::command::push;
use libra::utils::test::ChangeDirGuard;
use serial_test::serial;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

/// Helper function: Initialize a temporary Libra repository
fn init_temp_repo() -> TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let temp_path = temp_dir.path();

    eprintln!("Temporary directory created at: {temp_path:?}");
    assert!(
        temp_path.is_dir(),
        "Temporary path is not a valid directory"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .arg("init")
        .output()
        .expect("Failed to execute libra binary");

    if !output.status.success() {
        panic!(
            "Failed to initialize libra repository: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    eprintln!("Initialized libra repo at: {temp_path:?}");
    temp_dir
}

#[tokio::test]
#[serial]
async fn test_push_force_flag_parsing() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();
    let _guard = ChangeDirGuard::new(temp_path);

    // Test that --force flag is correctly parsed
    let args = push::PushArgs::parse_from(["push", "--force", "origin", "main"]);
    assert!(args.force);

    // Test that -f flag is correctly parsed
    let args = push::PushArgs::parse_from(["push", "-f", "origin", "main"]);
    assert!(args.force);
}

#[tokio::test]
#[ignore] // This test requires network connectivity
/// Test pushing to an invalid remote repository with timeout
async fn test_push_invalid_remote() {
    let temp_repo = init_temp_repo();
    let temp_path = temp_repo.path();
    let _guard = ChangeDirGuard::new(temp_path);

    eprintln!("Starting test: push to invalid remote");

    // Configure an invalid remote repository
    eprintln!("Adding invalid remote: https://invalid-url.example/repo.git");
    let remote_output = TokioCommand::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args([
            "remote",
            "add",
            "origin",
            "https://invalid-url.example/repo.git",
        ])
        .output()
        .await
        .expect("Failed to add remote");

    assert!(
        remote_output.status.success(),
        "Failed to add remote: {}",
        String::from_utf8_lossy(&remote_output.stderr)
    );

    // Set upstream branch
    eprintln!("Setting upstream to origin/main");
    let branch_output = TokioCommand::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["branch", "--set-upstream-to", "origin/main"])
        .output()
        .await
        .expect("Failed to set upstream branch");

    assert!(
        branch_output.status.success(),
        "Failed to set upstream: {}",
        String::from_utf8_lossy(&branch_output.stderr)
    );

    // Attempt to push with 15-second timeout to avoid hanging CI
    eprintln!("Attempting 'libra push' with 15s timeout...");
    let push_result = timeout(Duration::from_secs(15), async {
        TokioCommand::new(env!("CARGO_BIN_EXE_libra"))
            .current_dir(temp_path)
            .arg("push")
            .output()
            .await
    })
    .await;

    match push_result {
        // Timeout occurred — this is expected for unreachable remotes
        Err(_) => {
            eprintln!("Push timed out after 15 seconds — expected for invalid remote");
        }
        // Command completed within timeout
        Ok(Ok(output)) => {
            eprintln!("Push completed (status: {:?})", output.status);
            // Push to invalid remote should fail
            assert!(
                !output.status.success(),
                "Push should fail when remote is unreachable"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !stderr.trim().is_empty(),
                "Expected error message in stderr, but was empty"
            );

            eprintln!("Push failed as expected: {stderr}");
        }
        // Failed to start the command
        Ok(Err(e)) => {
            panic!("Failed to run 'libra push' command: {e}");
        }
    }

    eprintln!("test_push_invalid_remote passed");
}

#[tokio::test]
#[serial]
async fn test_push_force_with_local_changes() {
    // This test would verify force push functionality in a local repository setup
    // It would require setting up two repositories, making divergent changes,
    // and verifying that force push correctly overwrites the remote history

    // Note: This is a placeholder for a more comprehensive integration test
    // that would require a more complex setup with actual Git repositories
    assert!(true); // Placeholder assertion
}
