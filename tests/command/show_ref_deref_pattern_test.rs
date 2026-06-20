use libra::internal::{branch::Branch, head::Head, tag as internal_tag};

use super::*;

fn head_id(repo: &std::path::Path) -> String {
    let output = run_libra_command(&["rev-parse", "HEAD"], repo);
    assert_cli_success(&output, "rev-parse HEAD");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn create_annotated_tag(repo: &std::path::Path, name: &str) -> String {
    let _guard = ChangeDirGuard::new(repo);
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    runtime
        .block_on(internal_tag::create(
            name,
            Some("release notes".to_string()),
            false,
            false,
        ))
        .expect("failed to create annotated tag")
        .target
        .to_string()
}

fn create_lightweight_tag(repo: &std::path::Path, name: &str) {
    let _guard = ChangeDirGuard::new(repo);
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    runtime
        .block_on(internal_tag::create(name, None, false, false))
        .expect("failed to create lightweight tag");
}

#[test]
fn test_show_ref_dereference_annotated_tag() {
    let repo = create_committed_repo_via_cli();
    let peeled = head_id(repo.path());
    let tag_object = create_annotated_tag(repo.path(), "v1.0");

    let output = run_libra_command(
        &["show-ref", "--dereference", "--tags", "v1.0"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --dereference --tags v1.0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();

    assert_eq!(lines.len(), 2, "unexpected output: {stdout}");
    assert_eq!(lines[0], format!("{tag_object} refs/tags/v1.0"));
    assert_eq!(lines[1], format!("{peeled} refs/tags/v1.0^{{}}"));
}

#[test]
fn test_show_ref_dereference_hash_only() {
    let repo = create_committed_repo_via_cli();
    let peeled = head_id(repo.path());
    let tag_object = create_annotated_tag(repo.path(), "v1.0");

    let output = run_libra_command(
        &["show-ref", "--hash", "--dereference", "--tags", "v1.0"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --hash --dereference --tags v1.0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();

    assert_eq!(lines, vec![tag_object.as_str(), peeled.as_str()]);
}

#[test]
fn test_show_ref_dereference_lightweight_tag_has_single_line() {
    let repo = create_committed_repo_via_cli();
    create_lightweight_tag(repo.path(), "v1.0");

    let output = run_libra_command(
        &["show-ref", "--dereference", "--tags", "v1.0"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --dereference lightweight tag");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();

    assert_eq!(lines.len(), 1, "unexpected output: {stdout}");
    assert!(lines[0].ends_with(" refs/tags/v1.0"));
    assert!(!stdout.contains("^{}"));
}

#[test]
fn test_show_ref_pattern_matches_path_segment_suffix() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    runtime.block_on(async {
        let head_hash = Head::current_commit()
            .await
            .expect("expected HEAD commit")
            .to_string();
        Branch::update_branch("main-2", &head_hash, None)
            .await
            .expect("failed to create branch");
    });

    let output = run_libra_command(&["show-ref", "--heads", "main"], repo.path());
    assert_cli_success(&output, "show-ref --heads main");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("refs/heads/main"),
        "unexpected output: {stdout}"
    );
    assert!(
        !stdout.contains("refs/heads/main-2"),
        "substring-only pattern matching leaked main-2: {stdout}"
    );
}
