//! Integration tests for `rev-list` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use git_internal::hash::{HashKind, set_hash_kind_for_test};

use super::*;

fn create_two_commit_repo_with_direct_tip_update(timestamp_offset: usize) -> tempfile::TempDir {
    let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
    let repo = create_committed_repo_via_cli();
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    runtime.block_on(async {
        let _guard = ChangeDirGuard::new(repo.path());
        let parent_id = Head::current_commit().await.expect("expected HEAD commit");
        let parent: Commit = load_object(&parent_id).expect("failed to load parent commit");
        let mut author = parent.author.clone();
        let mut committer = parent.committer.clone();
        author.timestamp = parent.committer.timestamp + timestamp_offset;
        committer.timestamp = parent.committer.timestamp + timestamp_offset;
        let commit = Commit::new(author, committer, parent.tree_id, vec![parent_id], "second");
        save_object(&commit, &commit.id).expect("failed to save second commit");
        Branch::update_branch("main", &commit.id.to_string(), None)
            .await
            .expect("failed to update main branch");
    });

    repo
}

#[test]
fn test_rev_list_defaults_to_head() {
    let repo = create_committed_repo_via_cli();

    let implicit = run_libra_command(&["rev-list"], repo.path());
    assert_cli_success(&implicit, "rev-list");

    let explicit = run_libra_command(&["rev-list", "HEAD"], repo.path());
    assert_cli_success(&explicit, "rev-list HEAD");

    assert_eq!(implicit.stdout, explicit.stdout);
}

#[test]
fn test_rev_list_head_lists_reachable_commits_newest_first() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);

    let head = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&head, "rev-parse HEAD");
    let head_hash = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let parent = run_libra_command(&["rev-parse", "HEAD~1"], repo.path());
    assert_cli_success(&parent, "rev-parse HEAD~1");
    let parent_hash = String::from_utf8_lossy(&parent.stdout).trim().to_string();

    let output = run_libra_command(&["rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-list HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines, vec![head_hash.as_str(), parent_hash.as_str()]);
}

#[test]
fn test_rev_list_preserves_traversal_order_for_equal_timestamps() {
    let repo = create_two_commit_repo_with_direct_tip_update(0);

    let head = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&head, "rev-parse HEAD");
    let head_hash = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let parent = run_libra_command(&["rev-parse", "HEAD~1"], repo.path());
    assert_cli_success(&parent, "rev-parse HEAD~1");
    let parent_hash = String::from_utf8_lossy(&parent.stdout).trim().to_string();

    let output = run_libra_command(&["rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-list HEAD with equal timestamps");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines, vec![head_hash.as_str(), parent_hash.as_str()]);
}

#[test]
fn test_rev_list_supports_revision_navigation() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);

    let parent = run_libra_command(&["rev-parse", "HEAD~1"], repo.path());
    assert_cli_success(&parent, "rev-parse HEAD~1");
    let parent_hash = String::from_utf8_lossy(&parent.stdout).trim().to_string();

    let output = run_libra_command(&["rev-list", "HEAD~1"], repo.path());
    assert_cli_success(&output, "rev-list HEAD~1");

    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), parent_hash);
}

#[test]
fn test_rev_list_max_count_and_skip_limit_visible_output() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);

    let full = run_libra_command(&["rev-list", "HEAD"], repo.path());
    assert_cli_success(&full, "rev-list HEAD");
    let full_stdout = String::from_utf8_lossy(&full.stdout);
    let full_lines = full_stdout.lines().collect::<Vec<_>>();
    assert_eq!(full_lines.len(), 2, "expected two commits: {full_stdout}");

    let limited = run_libra_command(&["rev-list", "--max-count", "1", "HEAD"], repo.path());
    assert_cli_success(&limited, "rev-list --max-count 1 HEAD");
    let limited_stdout = String::from_utf8_lossy(&limited.stdout);
    assert_eq!(
        limited_stdout.lines().collect::<Vec<_>>(),
        vec![full_lines[0]]
    );

    let short_limited = run_libra_command(&["rev-list", "-n", "1", "HEAD"], repo.path());
    assert_cli_success(&short_limited, "rev-list -n 1 HEAD");
    assert_eq!(short_limited.stdout, limited.stdout);

    let skipped = run_libra_command(
        &["rev-list", "--skip", "1", "--max-count", "1", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&skipped, "rev-list --skip 1 --max-count 1 HEAD");
    let skipped_stdout = String::from_utf8_lossy(&skipped.stdout);
    assert_eq!(
        skipped_stdout.lines().collect::<Vec<_>>(),
        vec![full_lines[1]]
    );
}

#[test]
fn test_rev_list_count_reports_filtered_commit_count() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);

    let all = run_libra_command(&["rev-list", "--count", "HEAD"], repo.path());
    assert_cli_success(&all, "rev-list --count HEAD");
    assert_eq!(String::from_utf8_lossy(&all.stdout).trim(), "2");

    let limited = run_libra_command(
        &[
            "rev-list",
            "--count",
            "--skip",
            "1",
            "--max-count",
            "1",
            "HEAD",
        ],
        repo.path(),
    );
    assert_cli_success(&limited, "rev-list --count --skip 1 --max-count 1 HEAD");
    assert_eq!(String::from_utf8_lossy(&limited.stdout).trim(), "1");
}

#[test]
fn test_rev_list_parents_and_timestamp_output_match_git_order() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (head_id, parent_id, head_timestamp, parent_timestamp) = runtime.block_on(async {
        let _guard = ChangeDirGuard::new(repo.path());
        let head_id = Head::current_commit().await.expect("expected HEAD commit");
        let head_commit: Commit = load_object(&head_id).expect("failed to load HEAD commit");
        let parent_id = head_commit.parent_commit_ids[0];
        let parent_commit: Commit = load_object(&parent_id).expect("failed to load parent commit");
        (
            head_id.to_string(),
            parent_id.to_string(),
            head_commit.committer.timestamp,
            parent_commit.committer.timestamp,
        )
    });

    let parents = run_libra_command(&["rev-list", "--parents", "HEAD"], repo.path());
    assert_cli_success(&parents, "rev-list --parents HEAD");
    let parents_stdout = String::from_utf8_lossy(&parents.stdout);
    assert_eq!(
        parents_stdout
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        vec![format!("{head_id} {parent_id}"), parent_id.clone()]
    );

    let timestamp = run_libra_command(&["rev-list", "--timestamp", "HEAD"], repo.path());
    assert_cli_success(&timestamp, "rev-list --timestamp HEAD");
    let timestamp_stdout = String::from_utf8_lossy(&timestamp.stdout);
    assert_eq!(
        timestamp_stdout
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        vec![
            format!("{head_timestamp} {head_id}"),
            format!("{parent_timestamp} {parent_id}"),
        ]
    );

    let combined = run_libra_command(
        &["rev-list", "--timestamp", "--parents", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&combined, "rev-list --timestamp --parents HEAD");
    let combined_stdout = String::from_utf8_lossy(&combined.stdout);
    assert_eq!(
        combined_stdout
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        vec![
            format!("{head_timestamp} {head_id} {parent_id}"),
            format!("{parent_timestamp} {parent_id}"),
        ]
    );

    let count = run_libra_command(
        &["rev-list", "--count", "--parents", "--timestamp", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&count, "rev-list --count --parents --timestamp HEAD");
    assert_eq!(String::from_utf8_lossy(&count.stdout).trim(), "2");
}

#[test]
fn test_rev_list_invalid_target_returns_cli_error_code() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["rev-list", "badref"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("not a valid object name: 'badref'"));
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[test]
fn test_rev_list_rejects_tag_object_that_points_to_tree() {
    let repo = create_committed_repo_via_cli();
    let tag_id = create_non_commit_tag_object(repo.path());

    let output = run_libra_command(&["rev-list", tag_id.as_str()], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("not a valid object name"));
    assert!(stderr.contains("tag points to tree"));
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[tokio::test]
#[serial]
async fn test_rev_list_accepts_fully_qualified_remote_tracking_ref() {
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

    let output = run_libra_command(&["rev-list", "refs/remotes/origin/main"], repo.path());
    assert_cli_success(&output, "rev-list refs/remotes/origin/main");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        head.to_string()
    );
}

#[test]
fn test_rev_list_json_returns_envelope() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);

    let output = run_libra_command(&["--json", "rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "json rev-list HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "rev-list");
    assert_eq!(json["data"]["input"], "HEAD");
    assert_eq!(json["data"]["total"], 2);
    assert_eq!(json["data"]["commits"].as_array().map(Vec::len), Some(2));
}

#[test]
fn test_rev_list_json_entries_preserve_commit_ids_and_metadata() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (head_id, parent_id, head_timestamp) = runtime.block_on(async {
        let _guard = ChangeDirGuard::new(repo.path());
        let head_id = Head::current_commit().await.expect("expected HEAD commit");
        let head_commit: Commit = load_object(&head_id).expect("failed to load HEAD commit");
        (
            head_id.to_string(),
            head_commit.parent_commit_ids[0].to_string(),
            head_commit.committer.timestamp,
        )
    });

    let output = run_libra_command(
        &["--json", "rev-list", "--parents", "--timestamp", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&output, "json rev-list --parents --timestamp HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["commits"][0], head_id);
    assert_eq!(json["data"]["entries"][0]["commit"], head_id);
    assert_eq!(json["data"]["entries"][0]["parents"][0], parent_id);
    assert_eq!(json["data"]["entries"][0]["timestamp"], head_timestamp);
}

#[test]
fn test_rev_list_machine_returns_single_json_line() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--machine", "rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "machine rev-list HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line, got: {stdout}"
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("expected JSON");
    assert_eq!(parsed["command"], "rev-list");
    assert_eq!(parsed["data"]["input"], "HEAD");
}

#[test]
fn test_rev_list_quiet_suppresses_stdout() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--quiet", "rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "quiet rev-list HEAD");

    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
}

/// `libra rev-list --help` surfaces the EXAMPLES banner so users see
/// the default HEAD walk, an explicit branch walk, a relative ref walk,
/// the JSON variant, and the quiet form without reading the design doc.
/// Cross-cutting `--help` EXAMPLES rollout per
/// `docs/development/commands/_general.md` item B.
#[test]
fn test_rev_list_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for rev-list --help");
    let output = run_libra_command(&["rev-list", "--help"], repo.path());
    assert!(
        output.status.success(),
        "rev-list --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "rev-list --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra rev-list",
        "libra rev-list --count HEAD",
        "libra rev-list -n 5 HEAD",
        "libra rev-list --parents HEAD",
        "libra rev-list --timestamp HEAD",
        "libra rev-list main",
        "libra rev-list HEAD~5",
        "libra rev-list --json HEAD",
        "libra rev-list --quiet HEAD",
    ] {
        assert!(
            stdout.contains(invocation),
            "rev-list --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}
