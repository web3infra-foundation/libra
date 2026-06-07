use libra::internal::{branch::Branch, config::ConfigKv, head::Head};

use super::*;

fn head_id(repo: &std::path::Path) -> String {
    let output = run_libra_command(&["rev-parse", "HEAD"], repo);
    assert_cli_success(&output, "rev-parse HEAD");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn test_show_ref_verify_branch_present() {
    let repo = create_committed_repo_via_cli();
    let expected = head_id(repo.path());

    let output = run_libra_command(&["show-ref", "--verify", "refs/heads/main"], repo.path());
    assert_cli_success(&output, "show-ref --verify refs/heads/main");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), format!("{expected} refs/heads/main"));
}

#[test]
fn test_show_ref_verify_hash_only() {
    let repo = create_committed_repo_via_cli();
    let expected = head_id(repo.path());

    let output = run_libra_command(
        &["show-ref", "--hash", "--verify", "refs/heads/main"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --hash --verify refs/heads/main");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}

#[test]
fn test_show_ref_verify_short_name_fails() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["show-ref", "--verify", "main"], repo.path());

    assert_eq!(output.status.code(), Some(128));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("'main' - not a valid ref"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn test_show_ref_verify_quiet_missing_is_silent_exit_1() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["--quiet", "show-ref", "--verify", "refs/heads/nope"],
        repo.path(),
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn test_show_ref_verify_ignores_scope_flags() {
    let repo = create_committed_repo_via_cli();
    let expected = head_id(repo.path());

    let output = run_libra_command(
        &["show-ref", "--verify", "--tags", "refs/heads/main"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --verify ignores --tags");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), format!("{expected} refs/heads/main"));
}

#[test]
fn test_show_ref_verify_remote_tracking_ref() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");

    let expected = runtime.block_on(async {
        let head_hash = Head::current_commit()
            .await
            .expect("expected HEAD commit")
            .to_string();
        ConfigKv::set(
            "remote.origin.url",
            "https://example.invalid/repo.git",
            false,
        )
        .await
        .expect("failed to configure remote");
        Branch::update_branch("main", &head_hash, Some("origin"))
            .await
            .expect("failed to create remote tracking branch");
        head_hash
    });

    let output = run_libra_command(
        &["show-ref", "--verify", "refs/remotes/origin/main"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --verify refs/remotes/origin/main");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        format!("{expected} refs/remotes/origin/main")
    );
}
