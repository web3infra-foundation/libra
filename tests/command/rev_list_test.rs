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
/// `docs/improvement/README.md` item B.
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
        "libra rev-list main",
        "libra rev-list HEAD~5",
        "libra rev-list main..HEAD",
        "libra rev-list -n 10 HEAD",
        "libra rev-list --count HEAD",
        "libra rev-list --json HEAD",
        "libra rev-list --quiet HEAD",
    ] {
        assert!(
            stdout.contains(invocation),
            "rev-list --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}

// ---------------------------------------------------------------------------
// Multi-spec, ranges (^, A..B, A...B), limits, parent filters, and format
// flags. Black-box CLI tests over real branch/merge topologies.
// ---------------------------------------------------------------------------

fn commit_file_revlist(repo: &std::path::Path, file: &str, content: &str, msg: &str) {
    std::fs::write(repo.join(file), content).expect("write file");
    assert_cli_success(&run_libra_command(&["add", file], repo), "add file");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", msg, "--no-verify"], repo),
        "commit file",
    );
}

fn rev_parse_revlist(repo: &std::path::Path, rev: &str) -> String {
    let out = run_libra_command(&["rev-parse", rev], repo);
    assert_cli_success(&out, "rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn rev_list_lines(repo: &std::path::Path, args: &[&str]) -> Vec<String> {
    let mut full: Vec<&str> = vec!["rev-list"];
    full.extend_from_slice(args);
    let out = run_libra_command(&full, repo);
    assert_cli_success(&out, "rev-list");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect()
}

/// main stays at the base commit; feature is two commits ahead.
fn build_diverged_repo() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], p),
        "branch feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], p),
        "checkout feature",
    );
    commit_file_revlist(p, "f1.txt", "f1\n", "f1");
    commit_file_revlist(p, "f2.txt", "f2\n", "f2");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], p),
        "checkout main",
    );
    repo
}

/// HEAD is a 2-parent merge of feature into main.
fn build_merge_repo() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], p),
        "branch feature",
    );
    assert_cli_success(
        &run_libra_command(&["checkout", "feature"], p),
        "checkout feature",
    );
    commit_file_revlist(p, "feature.txt", "feature\n", "feature");
    assert_cli_success(
        &run_libra_command(&["checkout", "main"], p),
        "checkout main",
    );
    commit_file_revlist(p, "mainfile.txt", "main\n", "main change");
    assert_cli_success(
        &run_libra_command(&["merge", "feature"], p),
        "merge feature",
    );
    repo
}

#[test]
fn test_rev_list_excludes_with_caret() {
    let repo = create_committed_repo_via_cli();
    let lines = rev_list_lines(repo.path(), &["HEAD", "^HEAD"]);
    assert!(lines.is_empty(), "HEAD ^HEAD must be empty, got {lines:?}");
}

#[test]
fn test_rev_list_two_dot_range() {
    let repo = build_diverged_repo();
    let p = repo.path();
    let lines = rev_list_lines(p, &["main..feature"]);
    assert_eq!(
        lines.len(),
        2,
        "main..feature = 2 feature-only commits: {lines:?}"
    );
    let f1 = rev_parse_revlist(p, "feature~1");
    let f2 = rev_parse_revlist(p, "feature");
    let base = rev_parse_revlist(p, "main");
    assert!(lines.contains(&f1) && lines.contains(&f2));
    assert!(!lines.contains(&base), "base must be excluded by ^main");
}

#[test]
fn test_rev_list_three_dot_symmetric() {
    let repo = build_diverged_repo();
    let p = repo.path();
    // main == merge base, so the symmetric difference is the feature-only set.
    let lines = rev_list_lines(p, &["main...feature"]);
    assert_eq!(
        lines.len(),
        2,
        "main...feature symmetric difference: {lines:?}"
    );
}

#[test]
fn test_rev_list_multi_spec_union() {
    let repo = build_diverged_repo();
    let p = repo.path();
    // feature reaches base+f1+f2 (3); main reaches base (1); union dedups to 3.
    let lines = rev_list_lines(p, &["feature", "main"]);
    assert_eq!(
        lines.len(),
        3,
        "multi-spec union dedups the shared base: {lines:?}"
    );
}

#[test]
fn test_rev_list_max_count_and_skip() {
    let repo = build_diverged_repo();
    let p = repo.path();
    assert_eq!(rev_list_lines(p, &["-n", "1", "feature"]).len(), 1);
    let all = rev_list_lines(p, &["feature"]);
    let skipped = rev_list_lines(p, &["--skip", "1", "feature"]);
    assert_eq!(skipped.len(), all.len() - 1);
    assert_eq!(skipped[0], all[1], "--skip 1 drops the newest commit");
}

#[test]
fn test_rev_list_count_matches_line_count() {
    let repo = build_diverged_repo();
    let p = repo.path();
    let all = rev_list_lines(p, &["feature"]);
    let out = run_libra_command(&["rev-list", "--count", "feature"], p);
    assert_cli_success(&out, "rev-list --count");
    let count: usize = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .expect("count is a number");
    assert_eq!(count, all.len());
}

#[test]
fn test_rev_list_merges_and_no_merges() {
    let repo = build_merge_repo();
    let p = repo.path();
    let merge = rev_parse_revlist(p, "HEAD");
    assert_eq!(
        rev_list_lines(p, &["--merges", "HEAD"]),
        vec![merge.clone()],
        "--merges keeps only the merge commit",
    );
    let no_merges = rev_list_lines(p, &["--no-merges", "HEAD"]);
    assert!(
        !no_merges.contains(&merge),
        "--no-merges excludes the merge"
    );
    assert!(!no_merges.is_empty());
    assert_eq!(
        rev_list_lines(p, &["--min-parents", "2", "HEAD"]),
        vec![merge],
        "--min-parents 2 is equivalent to --merges",
    );
}

#[test]
fn test_rev_list_parents_and_timestamp_format() {
    let repo = build_merge_repo();
    let p = repo.path();
    let merge = rev_parse_revlist(p, "HEAD");
    let lines = rev_list_lines(p, &["--parents", "HEAD"]);
    let merge_line = lines
        .iter()
        .find(|l| l.starts_with(&merge))
        .expect("merge line present");
    assert_eq!(
        merge_line.split(' ').count(),
        3,
        "merge line is hash + 2 parents: {merge_line}",
    );
    for line in rev_list_lines(p, &["--timestamp", "HEAD"]) {
        let first = line.split(' ').next().unwrap_or_default();
        assert!(first.parse::<u64>().is_ok(), "timestamp prefix: {line}");
    }
}

#[test]
fn test_rev_list_merges_no_merges_conflict() {
    let repo = create_committed_repo_via_cli();
    let out = run_libra_command(
        &["rev-list", "--merges", "--no-merges", "HEAD"],
        repo.path(),
    );
    assert_eq!(out.status.code(), Some(129));
}
