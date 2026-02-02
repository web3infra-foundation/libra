//! Tests for the show command, verifying correct display of commits and tags.
//! Tests use CLI commands via the libra binary.

use std::process::Command;

use serial_test::serial;

/// Initialize a temporary repository using CLI.
fn init_temp_repo() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let temp_path = temp_dir.path();

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

    temp_dir
}

/// Configure user identity for commits using CLI.
fn configure_user_identity(temp_path: &std::path::Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["config", "user.name", "Test User"])
        .output()
        .expect("Failed to configure user.name");

    if !output.status.success() {
        panic!(
            "Failed to configure user.name: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["config", "user.email", "test@example.com"])
        .output()
        .expect("Failed to configure user.email");

    if !output.status.success() {
        panic!(
            "Failed to configure user.email: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Create a commit with a file using CLI.
fn create_commit(temp_path: &std::path::Path, filename: &str, content: &str, message: &str) {
    // Create file
    std::fs::write(temp_path.join(filename), content).expect("Failed to create file");

    // Add file
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["add", filename])
        .output()
        .expect("Failed to add file");

    if !output.status.success() {
        panic!(
            "Failed to add file: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Commit
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["commit", "-m", message, "--no-verify"])
        .output()
        .expect("Failed to commit");

    if !output.status.success() {
        panic!(
            "Failed to commit: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Create a lightweight tag using CLI.
fn create_lightweight_tag(temp_path: &std::path::Path, tag_name: &str) {
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["tag", tag_name])
        .output()
        .expect("Failed to create lightweight tag");

    if !output.status.success() {
        panic!(
            "Failed to create tag: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Create an annotated tag using CLI.
fn create_annotated_tag(temp_path: &std::path::Path, tag_name: &str, message: &str) {
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["tag", tag_name, "-m", message])
        .output()
        .expect("Failed to create annotated tag");

    if !output.status.success() {
        panic!(
            "Failed to create annotated tag: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Test that show can display a lightweight tag.
#[tokio::test]
#[serial]
async fn test_show_lightweight_tag() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "file.txt", "content", "Initial commit");

    // Create a lightweight tag
    create_lightweight_tag(temp_path, "v1.0-light");

    // Show the tag via CLI
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["show", "v1.0-light", "--no-patch"])
        .output()
        .expect("Failed to execute show command");

    assert!(
        output.status.success(),
        "show command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("commit"),
        "Output should contain 'commit': {}",
        stdout
    );
    assert!(
        stdout.contains("Initial commit"),
        "Output should contain commit message: {}",
        stdout
    );
}

/// Test that show displays an annotated tag with its metadata.
#[tokio::test]
#[serial]
async fn test_show_annotated_tag() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "file.txt", "content", "Initial commit");

    // Create an annotated tag with a message
    create_annotated_tag(temp_path, "v1.0-annotated", "Release v1.0.0");

    // Show the annotated tag via CLI
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["show", "v1.0-annotated", "--no-patch"])
        .output()
        .expect("Failed to execute show command");

    assert!(
        output.status.success(),
        "show command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Annotated tag should show tag info
    assert!(
        stdout.contains("tag"),
        "Output should contain 'tag': {}",
        stdout
    );
    assert!(
        stdout.contains("v1.0-annotated"),
        "Output should contain tag name: {}",
        stdout
    );
    assert!(
        stdout.contains("Release v1.0.0"),
        "Output should contain tag message: {}",
        stdout
    );
    assert!(
        stdout.contains("Test User"),
        "Output should contain tagger name: {}",
        stdout
    );
}

/// Test that show can handle multiple commits with different tags.
#[tokio::test]
#[serial]
async fn test_show_multiple_tags() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "file.txt", "content v1", "Feature one");

    // Create first tag on initial commit
    create_lightweight_tag(temp_path, "v0.1.0");

    // Make second commit
    create_commit(temp_path, "file.txt", "content v2", "Feature two");

    // Create second tag on latest commit
    create_lightweight_tag(temp_path, "v0.2.0");

    // Show first tag via CLI
    let output1 = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["show", "v0.1.0", "--no-patch"])
        .output()
        .expect("Failed to execute show command");

    assert!(
        output1.status.success(),
        "show v0.1.0 failed: {}",
        String::from_utf8_lossy(&output1.stderr)
    );

    let stdout1 = String::from_utf8_lossy(&output1.stdout);
    assert!(
        stdout1.contains("Feature one"),
        "v0.1.0 should show 'Feature one': {}",
        stdout1
    );

    // Show second tag via CLI
    let output2 = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["show", "v0.2.0", "--no-patch"])
        .output()
        .expect("Failed to execute show command");

    assert!(
        output2.status.success(),
        "show v0.2.0 failed: {}",
        String::from_utf8_lossy(&output2.stderr)
    );

    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(
        stdout2.contains("Feature two"),
        "v0.2.0 should show 'Feature two': {}",
        stdout2
    );
}

/// Test that show handles non-existent tags gracefully.
#[tokio::test]
#[serial]
async fn test_show_nonexistent_tag() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "file.txt", "content", "Initial commit");

    // Show a non-existent tag via CLI
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(&["show", "nonexistent-tag"])
        .output()
        .expect("Failed to execute show command");

    // Should fail with error
    assert!(
        !output.status.success(),
        "show command should fail for non-existent tag"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bad revision") || stderr.contains("fatal"),
        "Error output should indicate bad revision: {}",
        stderr
    );
}
