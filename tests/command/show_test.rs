//! Tests for the show command, verifying correct display of commits and tags.
//! Tests focus on testing the internal APIs that show uses, rather than CLI output.

use libra::{
    command::{
        add::{self, AddArgs},
        commit::{self, CommitArgs},
        tag::{self, TagArgs},
    },
    internal::tag as internal_tag,
    utils::test::{ChangeDirGuard, setup_with_new_libra_in},
};
use serial_test::serial;
use tempfile::tempdir;

/// Create a new temporary repo, set it as current dir, set up identity, add a file and commit.
/// Returns the TempDir and a ChangeDirGuard so the caller can keep the guard alive for test duration.
async fn setup_repo_with_commit() -> (tempfile::TempDir, ChangeDirGuard) {
    setup_repo_with_commit_with("content", "Initial commit").await
}

/// Same as `setup_repo_with_commit` but allows specifying file content and commit message.
async fn setup_repo_with_commit_with(
    content: &str,
    commit_msg: &str,
) -> (tempfile::TempDir, ChangeDirGuard) {
    let temp = tempdir().unwrap();
    let guard = ChangeDirGuard::new(temp.path());
    setup_with_new_libra_in(temp.path()).await;

    std::fs::write("file.txt", content).unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".into()],
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
        message: Some(commit_msg.into()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: true,
        author: None,
    })
    .await;

    (temp, guard)
}

/// Helper function to create a lightweight tag (no message).
async fn create_lightweight_tag(tag_name: &str) {
    tag::execute(TagArgs {
        name: Some(tag_name.into()),
        list: false,
        delete: false,
        message: None,
        force: false,
        n_lines: None,
    })
    .await;
}

/// Helper function to create an annotated tag (with message using -m).
async fn create_annotated_tag(tag_name: &str, message: &str) {
    tag::execute(TagArgs {
        name: Some(tag_name.into()),
        list: false,
        delete: false,
        message: Some(message.into()),
        force: false,
        n_lines: None,
    })
    .await;
}

/// Test that show can show a tag to its commit.
#[tokio::test]
#[serial]
async fn test_show_lightweight_tag() {
    let (_temp, _guard) = setup_repo_with_commit().await;

    // Create a lightweight tag
    create_lightweight_tag("v1.0-light").await;

    // Use the internal API that show uses to resolve tags
    let result = internal_tag::find_tag_and_commit("v1.0-light").await;

    // find_tag_and_commit returns Result<Option<...>, GitError>
    let Ok(Some((object, commit))) = result else {
        panic!("find_tag_and_commit failed or returned None");
    };

    // Lightweight tag points directly to commit
    assert_eq!(
        object.get_type(),
        git_internal::internal::object::types::ObjectType::Commit
    );
    assert!(
        commit.message.contains("Initial commit"),
        "Should resolve to initial commit"
    );
}

/// Test that show can show an annotated tag and include tag metadata.
#[tokio::test]
#[serial]
async fn test_show_annotated_tag() {
    let (_temp, _guard) = setup_repo_with_commit().await;

    // Create an annotated tag with a message
    create_annotated_tag("v1.0-annotated", "Release v1.0.0").await;

    // Use the internal API that show uses to show tags
    let result = internal_tag::find_tag_and_commit("v1.0-annotated").await;

    let Ok(Some((object, commit))) = result else {
        panic!("show tag and commit failed or returned None");
    };

    // Annotated tag points to a Tag object, not directly to commit
    assert_eq!(
        object.get_type(),
        git_internal::internal::object::types::ObjectType::Tag
    );

    // Check that we can get the commit through the tag
    assert!(
        commit.message.contains("Initial commit"),
        "Should resolve to initial commit"
    );
}

/// Test that show can handle multiple commits with different tags.
#[tokio::test]
#[serial]
async fn test_show_multiple_tags() {
    let (_temp, _guard) = setup_repo_with_commit_with("content v1", "Feature one").await;

    // Create first tag on initial commit
    create_annotated_tag("v0.1.0", "First release").await;

    // Make second commit
    std::fs::write("file.txt", "content v2").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".into()],
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
        message: Some("Feature two".into()),
        file: None,
        allow_empty: false,
        conventional: false,
        no_edit: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
        no_verify: true,
        author: None,
    })
    .await;

    // Create second tag on latest commit
    create_annotated_tag("v0.2.0", "Second release").await;

    // Verify both tags resolve correctly
    let v01_result = internal_tag::find_tag_and_commit("v0.1.0").await;
    let v02_result = internal_tag::find_tag_and_commit("v0.2.0").await;

    let Ok(Some((_, v01_commit))) = v01_result else {
        panic!("v0.1.0 should be found");
    };
    let Ok(Some((_, v02_commit))) = v02_result else {
        panic!("v0.2.0 should be found");
    };

    // Each tag should resolve to its respective commit
    assert!(
        v01_commit.message.contains("Feature one"),
        "v0.1.0 should point to commit with 'Feature one'"
    );
    assert!(
        v02_commit.message.contains("Feature two"),
        "v0.2.0 should point to commit with 'Feature two'"
    );
}

/// Test that show handles non-existent tags gracefully.
#[tokio::test]
#[serial]
async fn test_show_nonexistent_tag() {
    let (_temp, _guard) = setup_repo_with_commit().await;

    // Try to find a non-existent tag
    let result = internal_tag::find_tag_and_commit("nonexistent-tag").await;

    // The result should be Ok(None) for non-existent tag
    let Ok(None) = result else {
        panic!("show tag and commit should return Ok(None) for non-existent tag");
    };
}
