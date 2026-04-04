//! Tests for the show command, verifying correct display of commits and tags.
//! Tests use CLI commands via the libra binary.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{path::PathBuf, process::Command};

use git_internal::internal::object::{commit::Commit, tree::Tree};
use libra::{
    command::load_object,
    internal::{db::get_db_conn_instance, head::Head, model::reference},
    utils::{object_ext::TreeExt, output::OutputConfig, test::ChangeDirGuard},
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serial_test::serial;

use super::{
    create_committed_repo_via_cli, loose_object_path, parse_cli_error_stderr, parse_json_stdout,
    run_libra_command,
};

/// Initialize a temporary repository using CLI.
fn init_temp_repo() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let temp_path = temp_dir.path();

    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["init"])
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
        .args(["config", "user.name", "Test User"])
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
        .args(["config", "user.email", "test@example.com"])
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
        .args(["add", filename])
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
        .args(["commit", "-m", message, "--no-verify"])
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
        .args(["tag", tag_name])
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
        .args(["tag", tag_name, "-m", message])
        .output()
        .expect("Failed to create annotated tag");

    if !output.status.success() {
        panic!(
            "Failed to create annotated tag: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_show_cli_badref_returns_cli_exit_code() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["show", "badref"], repo.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("fatal: bad revision 'badref'"));
    assert!(stderr.contains("Error-Code: LBR-CLI-003"));
    assert!(stderr.contains("Hint: use 'libra log --oneline' to see available commits"));
}

#[test]
fn test_show_cli_outside_repository_returns_repo_not_found() {
    let temp = tempfile::tempdir().expect("failed to create tempdir");

    let output = run_libra_command(&["show", "HEAD"], temp.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-001");
    assert_eq!(report.category, "repo");
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn test_show_json_commit_output_includes_type_and_files() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "show", "HEAD"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "show");
    assert_eq!(json["data"]["type"], "commit");
    assert_eq!(json["data"]["subject"], "base");
    assert!(json["data"]["files"].as_array().is_some());
}

#[test]
fn test_show_quiet_suppresses_human_output() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--quiet", "show", "HEAD"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[tokio::test]
#[serial]
async fn test_show_json_commit_refs_are_best_effort_on_corrupt_branch_metadata() {
    let repo = create_committed_repo_via_cli();

    let create_branch = run_libra_command(&["branch", "topic"], repo.path());
    assert!(
        create_branch.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&create_branch.stderr)
    );

    let _guard = ChangeDirGuard::new(repo.path());
    let db = get_db_conn_instance().await;
    let topic = reference::Entity::find()
        .filter(reference::Column::Kind.eq(reference::ConfigKind::Branch))
        .filter(reference::Column::Name.eq("topic"))
        .filter(reference::Column::Remote.is_null())
        .one(&db)
        .await
        .unwrap()
        .expect("expected topic branch row");
    let mut topic: reference::ActiveModel = topic.into();
    topic.commit = Set(Some("not-a-valid-hash".to_string()));
    topic.update(&db).await.unwrap();

    let output = run_libra_command(&["--json", "show", "HEAD"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "show");
    assert_eq!(json["data"]["type"], "commit");
    let refs = json["data"]["refs"]
        .as_array()
        .expect("refs should be an array");
    assert!(
        refs.iter().any(|value| value == "HEAD -> main"),
        "expected HEAD ref to survive best-effort ref collection, got: {refs:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_show_patch_fails_when_commit_blob_is_missing() {
    let repo = create_committed_repo_via_cli();

    let tracked_blob = {
        let _guard = ChangeDirGuard::new(repo.path());
        let head = Head::current_commit().await.expect("expected HEAD commit");
        let commit: Commit = load_object(&head).expect("expected HEAD commit object");
        let tree: Tree = load_object(&commit.tree_id).expect("expected HEAD tree");
        tree.get_plain_items()
            .into_iter()
            .find(|(path, _)| path == &PathBuf::from("tracked.txt"))
            .map(|(_, hash)| hash.to_string())
            .expect("expected tracked.txt blob in HEAD tree")
    };
    std::fs::remove_file(loose_object_path(repo.path(), &tracked_blob))
        .expect("failed to delete committed blob");
    std::fs::write(
        repo.path().join("tracked.txt"),
        "mutated worktree fallback\n",
    )
    .expect("failed to mutate worktree file");

    let output = run_libra_command(&["show", "HEAD"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("failed to load blob object"),
        "expected repo corruption error, got: {stderr}"
    );
}

#[test]
fn test_show_json_annotated_tag_hash_preserves_tag_schema() {
    let repo = init_temp_repo();
    configure_user_identity(repo.path());
    create_commit(repo.path(), "tracked.txt", "tracked\n", "base");
    create_annotated_tag(repo.path(), "v1.0.0", "release notes");

    let show_ref = run_libra_command(&["show-ref", "--tags", "v1.0.0"], repo.path());
    assert!(
        show_ref.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&show_ref.stderr)
    );
    let stdout = String::from_utf8_lossy(&show_ref.stdout);
    let tag_hash = stdout
        .split_whitespace()
        .next()
        .expect("show-ref should return the tag object hash");

    let output = run_libra_command(&["--json", "show", tag_hash], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "show");
    assert_eq!(json["data"]["type"], "tag");
    assert_eq!(json["data"]["tag_name"], "v1.0.0");
    assert_eq!(json["data"]["message"], "release notes");
    assert_eq!(json["data"]["target_type"], "commit");
    assert!(json["data"]["tagger_name"].as_str().is_some());
}

#[test]
fn test_show_hex_like_tag_name_falls_back_to_ref_resolution() {
    let repo = init_temp_repo();
    configure_user_identity(repo.path());
    create_commit(repo.path(), "tracked.txt", "tracked\n", "base");

    let hex_like_tag = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    create_lightweight_tag(repo.path(), hex_like_tag);

    let human_output = run_libra_command(&["show", hex_like_tag], repo.path());
    assert!(
        human_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&human_output.stderr)
    );
    let human_stdout = String::from_utf8_lossy(&human_output.stdout);
    assert!(
        human_stdout.contains("base"),
        "expected human output to resolve the tag ref, got: {human_stdout}"
    );

    let json_output = run_libra_command(&["--json", "show", hex_like_tag], repo.path());
    assert!(
        json_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json_output.stderr)
    );
    let json = parse_json_stdout(&json_output);
    assert_eq!(json["command"], "show");
    assert_eq!(json["data"]["type"], "commit");
    assert_eq!(json["data"]["subject"], "base");
}

#[test]
fn test_show_json_commit_output_respects_pathspec_filters() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "tracked\nupdated\n")
        .expect("failed to update tracked file");
    std::fs::write(repo.path().join("other.txt"), "other\n").expect("failed to create other file");

    let add_output = run_libra_command(&["add", "tracked.txt", "other.txt"], repo.path());
    assert!(
        add_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_output.stderr)
    );

    let commit_output = run_libra_command(&["commit", "-m", "update", "--no-verify"], repo.path());
    assert!(
        commit_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&commit_output.stderr)
    );

    let unfiltered = run_libra_command(&["--json", "show", "HEAD"], repo.path());
    assert!(
        unfiltered.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&unfiltered.stderr)
    );
    let unfiltered_json = parse_json_stdout(&unfiltered);
    assert_eq!(
        unfiltered_json["data"]["files"]
            .as_array()
            .expect("files should be an array")
            .len(),
        2
    );

    let filtered = run_libra_command(&["--json", "show", "HEAD", "tracked.txt"], repo.path());
    assert!(
        filtered.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&filtered.stderr)
    );

    let filtered_json = parse_json_stdout(&filtered);
    let files = filtered_json["data"]["files"]
        .as_array()
        .expect("files should be an array");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "tracked.txt");
    assert_eq!(files[0]["status"], "modified");
}

#[tokio::test]
#[serial]
async fn test_show_tree_output_uses_git_modes_and_types() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let head = Head::current_commit().await.unwrap();
    let commit: Commit = load_object(&head).unwrap();
    let tree_hash = commit.tree_id.to_string();

    let human = run_libra_command(&["show", &tree_hash], repo.path());
    assert!(
        human.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&human.stderr)
    );
    let human_stdout = String::from_utf8_lossy(&human.stdout);
    assert!(
        human_stdout.contains("100644 blob"),
        "expected git tree mode/type in human output, got: {human_stdout}"
    );
    assert!(
        human_stdout.contains("\ttracked.txt"),
        "expected tracked entry in human output, got: {human_stdout}"
    );

    let output = run_libra_command(&["--json", "show", &tree_hash], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "show");
    assert_eq!(json["data"]["type"], "tree");
    assert_eq!(json["data"]["entries"][0]["mode"], "100644");
    assert_eq!(json["data"]["entries"][0]["object_type"], "blob");
    assert_eq!(json["data"]["entries"][0]["name"], "tracked.txt");
}

/// Test that show can display a lightweight tag.
#[tokio::test]
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
        .args(["show", "v1.0-light", "--no-patch"])
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
        .args(["show", "v1.0-annotated", "--no-patch"])
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
        .args(["show", "v0.1.0", "--no-patch"])
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
        .args(["show", "v0.2.0", "--no-patch"])
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
async fn test_show_nonexistent_tag() {
    let temp_dir = init_temp_repo();
    let temp_path = temp_dir.path();

    configure_user_identity(temp_path);
    create_commit(temp_path, "file.txt", "content", "Initial commit");

    // Show a non-existent tag via CLI
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(temp_path)
        .args(["show", "nonexistent-tag"])
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

/// Test that `show::execute_safe` returns a structured `CliError` for an
/// invalid object reference when called through the API.
#[tokio::test]
#[serial]
async fn test_show_execute_safe_bad_ref_returns_cli_error() {
    use libra::{
        command::show::{ShowArgs, execute_safe},
        utils::test::{self, ChangeDirGuard},
    };
    use tempfile::tempdir;

    let temp = tempdir().expect("failed to create temp dir");
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    let args = ShowArgs {
        object: Some("nonexistent_ref_abc123".to_string()),
        no_patch: false,
        oneline: false,
        name_only: false,
        stat: false,
        pathspec: vec![],
    };
    let result = execute_safe(args, &OutputConfig::default()).await;
    assert!(result.is_err(), "execute_safe should fail for bad ref");
    let err = result.unwrap_err();
    assert_eq!(
        err.exit_code(),
        129,
        "bad revision should map to the invalid-target exit code"
    );
    assert_eq!(err.stable_code().as_str(), "LBR-CLI-003");
    assert!(
        err.message().contains("bad revision") || err.message().contains("unknown revision"),
        "error should mention bad revision, got: {}",
        err.message()
    );
}

/// Test that `show::execute_safe` returns a structured `CliError` for an
/// invalid `<rev>:<path>` pattern.
#[tokio::test]
#[serial]
async fn test_show_execute_safe_bad_rev_path_returns_cli_error() {
    use libra::{
        command::show::{ShowArgs, execute_safe},
        utils::test::{self, ChangeDirGuard},
    };
    use tempfile::tempdir;

    let temp = tempdir().expect("failed to create temp dir");
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    let args = ShowArgs {
        object: Some("HEAD:nonexistent_file.txt".to_string()),
        no_patch: false,
        oneline: false,
        name_only: false,
        stat: false,
        pathspec: vec![],
    };
    let result = execute_safe(args, &OutputConfig::default()).await;
    assert!(result.is_err(), "execute_safe should fail for bad rev:path");
}

#[test]
fn test_show_machine_output_is_single_line_json() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--machine", "show", "HEAD"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let non_empty_lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        non_empty_lines.len(),
        1,
        "machine output should be exactly one non-empty line, got: {stdout}"
    );

    let parsed: serde_json::Value =
        serde_json::from_str(non_empty_lines[0]).expect("machine output should be valid JSON");
    assert_eq!(parsed["command"], "show");
    assert_eq!(parsed["data"]["type"], "commit");
}

#[tokio::test]
#[serial]
async fn test_show_json_blob_output_includes_content() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let head = Head::current_commit().await.unwrap();
    let commit: Commit = load_object(&head).unwrap();
    let tree: Tree = load_object(&commit.tree_id).unwrap();
    let blob_hash = tree
        .get_plain_items()
        .into_iter()
        .find(|(path, _)| path == &PathBuf::from("tracked.txt"))
        .map(|(_, hash)| hash.to_string())
        .expect("expected tracked.txt blob");

    let output = run_libra_command(&["--json", "show", &blob_hash], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "show");
    assert_eq!(json["data"]["type"], "blob");
    assert!(!json["data"]["is_binary"].as_bool().unwrap());
    assert!(
        json["data"]["content"].as_str().is_some(),
        "text blob should have content"
    );
    assert!(json["data"]["size"].as_u64().unwrap() > 0);
}

#[test]
fn test_show_json_bad_revision_returns_error() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "show", "nonexistent_ref"], repo.path());
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(129));

    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.exit_code, 129);
    assert!(report.message.contains("bad revision"));
}

#[test]
fn test_show_json_lightweight_tag_resolves_to_commit() {
    let repo = init_temp_repo();
    configure_user_identity(repo.path());
    create_commit(repo.path(), "file.txt", "content\n", "Initial commit");
    create_lightweight_tag(repo.path(), "v0.1");

    let output = run_libra_command(&["--json", "show", "v0.1"], repo.path());
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["type"], "commit");
    assert_eq!(json["data"]["subject"], "Initial commit");
}
