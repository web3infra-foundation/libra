//! Integration tests for `rev-parse` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use super::*;

/// Assert that `rev-parse --show-toplevel` prints the expected repository root.
///
/// Test coverage: the three direct worktree/storage-dir tests below pass temp
/// paths through both `/var` and canonical `/private/var` spellings on macOS,
/// while the symlink case verifies that an entered storage symlink still maps to
/// the canonical worktree root.
fn assert_show_toplevel_stdout_eq(output: &std::process::Output, expected: &std::path::Path) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let actual = std::path::PathBuf::from(stdout.trim());
    assert_eq!(
        actual
            .canonicalize()
            .expect("failed to canonicalize rev-parse output path"),
        expected
            .canonicalize()
            .expect("failed to canonicalize expected repo path")
    );
}

#[test]
fn test_rev_parse_head_resolves_commit() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-parse HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value = stdout.trim();
    assert_eq!(value.len(), 40, "expected full hash, got: {value}");
    assert!(value.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_rev_parse_short_head_returns_non_ambiguous_hash() {
    let repo = create_committed_repo_via_cli();

    let full = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&full, "rev-parse HEAD (full)");
    let full_hash = String::from_utf8_lossy(&full.stdout).trim().to_string();

    let output = run_libra_command(&["rev-parse", "--short", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-parse --short HEAD");

    let short_hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        short_hash.len() >= 7,
        "expected abbreviated hash, got: {short_hash}"
    );
    assert!(short_hash.len() <= full_hash.len());
    assert!(full_hash.starts_with(&short_hash));

    let resolved = run_libra_command(&["rev-parse", short_hash.as_str()], repo.path());
    assert_cli_success(&resolved, "rev-parse <short-hash>");
    assert_eq!(String::from_utf8_lossy(&resolved.stdout).trim(), full_hash);
}

#[test]
fn test_rev_parse_abbrev_ref_head_returns_branch_name() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["rev-parse", "--abbrev-ref", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-parse --abbrev-ref HEAD");

    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_remote_tracking_ref_returns_short_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch(
        "refs/remotes/origin/main",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .expect("failed to create remote-tracking ref");

    let output = run_libra_command(&["rev-parse", "--abbrev-ref", "origin/main"], repo.path());
    assert_cli_success(&output, "rev-parse --abbrev-ref origin/main");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "origin/main"
    );
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_multi_segment_remote_tracking_ref_returns_short_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch(
        "refs/remotes/upstream/origin/main",
        &head.to_string(),
        Some("upstream/origin"),
    )
    .await
    .expect("failed to create multi-segment remote-tracking ref");

    let output = run_libra_command(
        &["rev-parse", "--abbrev-ref", "upstream/origin/main"],
        repo.path(),
    );
    assert_cli_success(&output, "rev-parse --abbrev-ref upstream/origin/main");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "upstream/origin/main"
    );
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_lowercase_head_resolves_branch_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch("head", &head.to_string(), None)
        .await
        .expect("failed to create lowercase head branch");

    let output = run_libra_command(&["rev-parse", "--abbrev-ref", "head"], repo.path());
    assert_cli_success(&output, "rev-parse --abbrev-ref head");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "head");
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_refs_heads_returns_short_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let output = run_libra_command(
        &["rev-parse", "--abbrev-ref", "refs/heads/main"],
        repo.path(),
    );
    assert_cli_success(&output, "rev-parse --abbrev-ref refs/heads/main");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_refs_remotes_returns_short_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch(
        "refs/remotes/origin/main",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .expect("failed to create remote-tracking ref");

    let output = run_libra_command(
        &["rev-parse", "--abbrev-ref", "refs/remotes/origin/main"],
        repo.path(),
    );
    assert_cli_success(&output, "rev-parse --abbrev-ref refs/remotes/origin/main");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "origin/main"
    );
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_prefers_exact_local_refs_remotes_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch("refs/remotes/origin/main", &head.to_string(), None)
        .await
        .expect("failed to create local branch named like remote-tracking ref");

    let output = run_libra_command(
        &["rev-parse", "--abbrev-ref", "refs/remotes/origin/main"],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "rev-parse --abbrev-ref exact local refs/remotes/origin/main",
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "refs/remotes/origin/main"
    );
}

#[test]
fn test_rev_parse_show_toplevel_repo_named_storage_dir_returns_repo_root() {
    let parent = tempdir().expect("failed to create parent directory");
    let repo_path = parent.path().join(libra::utils::util::ROOT_DIR);
    init_repo_via_cli(&repo_path);

    let output = run_libra_command(&["rev-parse", "--show-toplevel"], &repo_path);
    assert_cli_success(
        &output,
        "rev-parse --show-toplevel from repo root named .libra",
    );

    // Scenario: a repository whose worktree itself is named `.libra` must not
    // be mistaken for the internal storage directory.
    assert_show_toplevel_stdout_eq(&output, &repo_path);
}

#[test]
fn test_rev_parse_show_toplevel_returns_repo_root() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["rev-parse", "--show-toplevel"], repo.path());
    assert_cli_success(&output, "rev-parse --show-toplevel from repo root");

    // Scenario: the normal worktree-root invocation returns the root path,
    // allowing platform-specific tempdir symlinks to differ only in spelling.
    assert_show_toplevel_stdout_eq(&output, repo.path());
}

#[test]
fn test_rev_parse_show_toplevel_from_storage_dir_returns_repo_root() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());
    let storage = repo.path().join(libra::utils::util::ROOT_DIR);

    let output = run_libra_command(&["rev-parse", "--show-toplevel"], &storage);
    assert_cli_success(&output, "rev-parse --show-toplevel from .libra");

    // Scenario: entering the physical `.libra` storage directory reports the
    // enclosing worktree root rather than the storage path itself.
    assert_show_toplevel_stdout_eq(&output, repo.path());
}

#[cfg(unix)]
#[test]
fn test_rev_parse_show_toplevel_from_symlinked_storage_dir_returns_repo_root() {
    use std::os::unix::fs::symlink;

    let temp_root = tempdir().expect("failed to create temp root");
    let repo = temp_root.path().join("repo");
    init_repo_via_cli(&repo);

    let storage = repo.join(libra::utils::util::ROOT_DIR);
    let storage_link = temp_root.path().join("storage-link");
    symlink(&storage, &storage_link).expect("failed to create storage symlink");

    let output = run_libra_command(&["rev-parse", "--show-toplevel"], &storage_link);
    assert_cli_success(&output, "rev-parse --show-toplevel from symlinked .libra");

    // Scenario: a symlink pointing at `.libra` is resolved back to the real
    // worktree root, matching Git's behavior for storage-directory traversal.
    assert_show_toplevel_stdout_eq(&output, &repo);
}

#[test]
fn test_rev_parse_show_toplevel_in_bare_repo_returns_work_tree_error() {
    let repo = tempdir().expect("failed to create repository root");
    let bare_repo = repo.path().join("repo.git");

    let init_output = run_libra_command(
        &["init", "--bare", "repo.git", "--vault", "false"],
        repo.path(),
    );
    assert_cli_success(&init_output, "init bare repo for rev-parse test");

    let output = run_libra_command(&["rev-parse", "--show-toplevel"], &bare_repo);
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert!(stderr.contains("this operation must be run in a work tree"));
    assert_eq!(report.error_code, "LBR-REPO-003");
}

#[test]
fn test_rev_parse_show_toplevel_rejects_spec() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["rev-parse", "--show-toplevel", "HEAD"], repo.path());

    assert!(!output.status.success(), "command unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("unexpected argument"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_rev_parse_invalid_target_returns_cli_error_code() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["rev-parse", "badref"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("not a valid object name: 'badref'"));
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[test]
fn test_rev_parse_json_returns_envelope() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "rev-parse", "HEAD"], repo.path());
    assert_cli_success(&output, "json rev-parse HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "rev-parse");
    assert_eq!(json["data"]["mode"], "resolve");
    assert_eq!(json["data"]["input"], "HEAD");
    assert!(json["data"]["value"].as_str().is_some());
}

#[test]
fn test_rev_parse_machine_returns_single_json_line() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--machine", "rev-parse", "HEAD"], repo.path());
    assert_cli_success(&output, "machine rev-parse HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line, got: {stdout}"
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("expected JSON");
    assert_eq!(parsed["command"], "rev-parse");
    assert_eq!(parsed["data"]["mode"], "resolve");
}
