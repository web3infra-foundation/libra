use libra::command::add::{self, AddArgs};
use libra::command::commit::{self, CommitArgs};
use libra::command::config::{self, ConfigArgs};
use libra::command::tag::{self, TagArgs};
use libra::internal::tag as internal_tag;
use libra::utils::test::{setup_with_new_libra_in, ChangeDirGuard};
use serial_test::serial;
use tempfile::tempdir;
use std::collections::HashSet;

// Test helpers and utilities for tag tests.
// These helpers work with the internal tag API (`internal::tag`) rather than the CLI
// because some tests need to create tags directly and inspect internal objects.

async fn setup_user_identity() {
    // Configure a predictable user identity for annotated tag creation
    config::execute(ConfigArgs {
        key: Some("user.name".into()),
        valuepattern: Some("Test User".into()),
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        name_only: false,
        default: None,
    }).await;
    config::execute(ConfigArgs {
        key: Some("user.email".into()),
        valuepattern: Some("test@example.com".into()),
        add: true,
        get: false,
        get_all: false,
        unset: false,
        unset_all: false,
        list: false,
        name_only: false,
        default: None,
    }).await;
}

/// Return the full ref name for a tag (e.g. "refs/tags/v1.0").
fn ref_name(tag: &str) -> String {
    format!("refs/tags/{tag}")
}

/// List tag names returned by `internal_tag::list()`.
/// `internal_tag::list()` returns bare tag names (without the "refs/tags/" prefix).
async fn list_tag_refs() -> Vec<String> {
    internal_tag::list()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.name)
        .collect()
}

/// Find a tag by name.
/// Accepts either a full ref ("refs/tags/<name>") or a bare name ("<name>").
async fn get_tag_by_name(full_ref: &str) -> Option<internal_tag::Tag> {
    // Support both full ref (refs/tags/...) and bare tag name
    let search = full_ref.strip_prefix("refs/tags/").unwrap_or(full_ref);
    internal_tag::list()
        .await
        .ok()?
        .into_iter()
        .find(|t| t.name == search)
}

/// Returns true if a tag with the given bare name exists.
async fn tag_exists(name: &str) -> bool {
    let full = ref_name(name);
    get_tag_by_name(&full).await.is_some()
}

/// Read the object id the tag points to (as a string), if present.
async fn read_tag_oid(name: &str) -> Option<String> {
    let full = ref_name(name);
    let tag = get_tag_by_name(&full).await?;

    match &tag.object {
        internal_tag::TagObject::Commit(c) => Some(c.id.to_string()),
        internal_tag::TagObject::Tag(t) => Some(t.id.to_string()),
        internal_tag::TagObject::Tree(tr) => Some(tr.id.to_string()),
        internal_tag::TagObject::Blob(b) => Some(b.id.to_string()),
    }
}



/// Return a set of bare tag names currently present (no refs/tags/ prefix).
async fn list_tag_names() -> HashSet<String> {
    list_tag_refs()
        .await
        .into_iter()
        .collect()
}

/// Assert the tag exists; provide helpful failure message.
async fn assert_tag_exists(name: &str) {
    assert!(tag_exists(name).await, "Tag does not exist: {}", name);
}

/// Assert the tag is absent; provide helpful failure message.
async fn assert_tag_absent(name: &str) {
    assert!(!tag_exists(name).await, "Tag still exists: {}", name);
}

// Test cases

#[tokio::test]
#[serial]
async fn test_basic_tag_creation() {
    // Create an isolated temporary repository and ensure a commit exists.
    let temp = tempdir().unwrap();
    setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());
    setup_user_identity().await;

    std::fs::write("file.txt", "content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".into()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    }).await;

    commit::execute(CommitArgs {
        message: Some("Initial commit".into()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    }).await;

    // Create a lightweight tag that points to HEAD commit.
    internal_tag::create("v1.0.0", None, false).await.unwrap();

    // Verify tag presence and that we can read the pointed object id.
    assert_tag_exists("v1.0.0").await;
    assert!(read_tag_oid("v1.0.0").await.is_some(), "Should be able to read tag OID");
}

#[tokio::test]
#[serial]
async fn test_tag_with_message() {
    // Create a tag with an annotation message (annotated tag) and verify presence.
    let temp = tempdir().unwrap();
    setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());
    setup_user_identity().await;

    std::fs::write("file.txt", "content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".into()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    }).await;

    commit::execute(CommitArgs {
        message: Some("Commit with message".into()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    }).await;

    // Annotated tag creation (includes tagger and message fields internally).
    internal_tag::create("v1.0.1", Some("Release v1.0.1".into()), false).await.unwrap();

    assert_tag_exists("v1.0.1").await;
    assert!(read_tag_oid("v1.0.1").await.is_some());
}

#[tokio::test]
#[serial]
async fn test_force_tag() {
    // Verify that forcing a tag replaces the ref target.
    let temp = tempdir().unwrap();
    setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());
    setup_user_identity().await;

    std::fs::write("file.txt", "v1").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".into()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    }).await;
    commit::execute(CommitArgs {
        message: Some("First".into()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    }).await;

    internal_tag::create("v1.0", Some("Initial".into()), false).await.unwrap();
    assert_tag_exists("v1.0").await;
    let before = read_tag_oid("v1.0").await;

    std::fs::write("file.txt", "v2").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".into()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    }).await;
    commit::execute(CommitArgs {
        message: Some("Second".into()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    }).await;

    // Use CLI path for force update to exercise both CLI and internal logic.
    tag::execute(TagArgs {
        name: Some("v1.0".into()),
        list: false,
        delete: false,
        message: Some("Updated".into()),
        force: true,
    }).await;
    let after = read_tag_oid("v1.0").await;
    assert!(before.is_some() && after.is_some() && before != after, "force update should change OID");
}

#[tokio::test]
#[serial]
async fn test_list_tags() {
    // Verify listing returns created tag names.
    let temp = tempdir().unwrap();
    setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());
    setup_user_identity().await;

    std::fs::write("file.txt", "content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".into()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    }).await;
    commit::execute(CommitArgs {
        message: Some("Base".into()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    }).await;

    internal_tag::create("v1.0.0", None, false).await.unwrap();
    internal_tag::create("v2.0.0", None, false).await.unwrap();

    let names = list_tag_names().await;
    assert!(names.contains("v1.0.0"));
    assert!(names.contains("v2.0.0"));
}

#[tokio::test]
#[serial]
async fn test_delete_tag() {
    // Verify delete removes the tag ref.
    let temp = tempdir().unwrap();
    setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());
    setup_user_identity().await;

    std::fs::write("file.txt", "content").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["file.txt".into()],
        all: false,
        update: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        refresh: false,
        force: false,
    }).await;
    commit::execute(CommitArgs {
        message: Some("Delete base".into()),
        file: None,
        allow_empty: false,
        conventional: false,
        amend: false,
        signoff: false,
        disable_pre: false,
        all: false,
    }).await;

    internal_tag::create("to-delete", None, false).await.unwrap();
    assert_tag_exists("to-delete").await;

    tag::execute(TagArgs {
        name: Some("to-delete".into()),
        list: false,
        delete: true,
        message: None,
        force: false,
    }).await;
    assert_tag_absent("to-delete").await;
}
