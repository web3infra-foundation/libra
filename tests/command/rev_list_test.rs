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

#[path = "rev_list_output_test.rs"]
mod rev_list_output_test;

#[path = "rev_list_parent_filter_test.rs"]
mod rev_list_parent_filter_test;

#[path = "rev_list_range_test.rs"]
mod rev_list_range_test;

#[path = "rev_list_date_filter_test.rs"]
mod rev_list_date_filter_test;

#[path = "rev_list_first_parent_test.rs"]
mod rev_list_first_parent_test;

#[path = "rev_list_author_filter_test.rs"]
mod rev_list_author_filter_test;
#[path = "rev_list_cherry_filter_test.rs"]
mod rev_list_cherry_filter_test;
#[path = "rev_list_cherry_shorthand_test.rs"]
mod rev_list_cherry_shorthand_test;
#[path = "rev_list_children_test.rs"]
mod rev_list_children_test;
#[path = "rev_list_committer_filter_test.rs"]
mod rev_list_committer_filter_test;
#[path = "rev_list_grep_filter_test.rs"]
mod rev_list_grep_filter_test;
#[path = "rev_list_path_filter_test.rs"]
mod rev_list_path_filter_test;

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
fn test_rev_list_reverse_outputs_oldest_first() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);
    let p = repo.path();
    let head = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD"], p).stdout)
        .trim()
        .to_string();
    let parent = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD~1"], p).stdout)
        .trim()
        .to_string();

    // --reverse flips the default newest-first order to oldest-first.
    let out = run_libra_command(&["rev-list", "--reverse", "HEAD"], p);
    assert_cli_success(&out, "rev-list --reverse HEAD");
    let lines = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![parent.clone(), head.clone()],
        "reverse = oldest first"
    );

    // Commit limiting is applied BEFORE the reverse: `-n 1` selects the newest
    // (head); reversing a single commit is still head. (Were it reverse-first,
    // the output would be the parent.)
    let out2 = run_libra_command(&["rev-list", "-n", "1", "--reverse", "HEAD"], p);
    assert_cli_success(&out2, "rev-list -n 1 --reverse HEAD");
    let lines2 = String::from_utf8_lossy(&out2.stdout)
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        lines2,
        vec![head.clone()],
        "limit-then-reverse keeps the newest"
    );
}

#[test]
fn test_rev_list_all_includes_every_ref() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    let rev = |spec: &str| {
        String::from_utf8_lossy(&run_libra_command(&["rev-parse", spec], p).stdout)
            .trim()
            .to_string()
    };
    let c1 = rev("HEAD");

    // A divergent commit on branch `other`.
    assert_cli_success(&run_libra_command(&["branch", "other"], p), "branch other");
    assert_cli_success(&run_libra_command(&["switch", "other"], p), "switch other");
    std::fs::write(p.join("other.txt"), "o\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "other.txt"], p), "add other");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c2", "--no-verify"], p),
        "commit c2",
    );
    let c2 = rev("HEAD");

    // Advance main with its own commit.
    assert_cli_success(&run_libra_command(&["switch", "main"], p), "switch main");
    std::fs::write(p.join("main.txt"), "m\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "main.txt"], p), "add main");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c3", "--no-verify"], p),
        "commit c3",
    );
    let c3 = rev("HEAD");

    // `rev-list main` sees main's history but not the `other` branch tip.
    let main_only = run_libra_command(&["rev-list", "main"], p);
    assert_cli_success(&main_only, "rev-list main");
    let s = String::from_utf8_lossy(&main_only.stdout).into_owned();
    assert!(s.contains(&c3) && s.contains(&c1), "main has c3+c1: {s:?}");
    assert!(!s.contains(&c2), "main must not see other's c2: {s:?}");

    // `rev-list --all` includes every ref tip's history.
    let all = run_libra_command(&["rev-list", "--all"], p);
    assert_cli_success(&all, "rev-list --all");
    let s_all = String::from_utf8_lossy(&all.stdout).into_owned();
    assert!(
        s_all.contains(&c1) && s_all.contains(&c2) && s_all.contains(&c3),
        "--all must include c1, c2 (other), and c3 (main): {s_all:?}"
    );
}

#[test]
fn test_rev_list_all_includes_tag_only_commits() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    let rev = |spec: &str| {
        String::from_utf8_lossy(&run_libra_command(&["rev-parse", spec], p).stdout)
            .trim()
            .to_string()
    };
    let c1 = rev("HEAD");

    // Commit c2 and tag it, then reset main back to c1 so c2 is reachable ONLY
    // via the tag (not from any branch).
    std::fs::write(p.join("t.txt"), "t\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "t.txt"], p), "add t");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c2", "--no-verify"], p),
        "commit c2",
    );
    let c2 = rev("HEAD");
    assert_cli_success(&run_libra_command(&["tag", "mytag"], p), "tag mytag at c2");
    assert_cli_success(
        &run_libra_command(&["reset", "--hard", &c1], p),
        "reset main to c1",
    );

    // main no longer reaches c2.
    let main_only = run_libra_command(&["rev-list", "main"], p);
    assert_cli_success(&main_only, "rev-list main");
    assert!(
        !String::from_utf8_lossy(&main_only.stdout).contains(&c2),
        "main must not reach the tag-only commit c2"
    );

    // --all reaches c2 through the tag (seeded by object id, not name).
    let all = run_libra_command(&["rev-list", "--all"], p);
    assert_cli_success(&all, "rev-list --all");
    let s = String::from_utf8_lossy(&all.stdout).into_owned();
    assert!(
        s.contains(&c1) && s.contains(&c2),
        "--all must include the tag-only commit c2: {s:?}"
    );
}

#[test]
fn test_rev_list_all_on_unborn_repo_is_empty() {
    // `--all` supplies the ref set as input; with no refs the output is empty
    // (exit 0), not a fallback to an unborn HEAD error.
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let out = run_libra_command(&["rev-list", "--all"], repo.path());
    assert_cli_success(&out, "rev-list --all on an unborn repo");
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "no refs -> empty output: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn test_rev_list_all_includes_detached_head_commit() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    let rev = |spec: &str| {
        String::from_utf8_lossy(&run_libra_command(&["rev-parse", spec], p).stdout)
            .trim()
            .to_string()
    };
    let c1 = rev("HEAD");

    // Detach HEAD, then make a commit reachable only from the detached HEAD.
    assert_cli_success(
        &run_libra_command(&["switch", "--detach", &c1], p),
        "detach HEAD",
    );
    std::fs::write(p.join("d.txt"), "d\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "d.txt"], p), "add d");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "detached", "--no-verify"], p),
        "commit detached",
    );
    let cdet = rev("HEAD");
    assert_ne!(cdet, c1, "detached commit must advance HEAD");

    // main does not reach the detached commit.
    let main_only = run_libra_command(&["rev-list", "main"], p);
    assert_cli_success(&main_only, "rev-list main");
    assert!(
        !String::from_utf8_lossy(&main_only.stdout).contains(&cdet),
        "main must not reach the detached commit"
    );

    // --all includes the detached HEAD commit (Git seeds --all with HEAD too).
    let all = run_libra_command(&["rev-list", "--all"], p);
    assert_cli_success(&all, "rev-list --all");
    assert!(
        String::from_utf8_lossy(&all.stdout).contains(&cdet),
        "--all must include the detached HEAD commit: {}",
        String::from_utf8_lossy(&all.stdout)
    );
}

#[test]
fn test_rev_list_date_order_matches_default_ordering() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);
    let p = repo.path();
    // --date-order is accepted and produces Libra's default committer-date
    // (newest-first) ordering.
    let default = run_libra_command(&["rev-list", "HEAD"], p);
    assert_cli_success(&default, "rev-list HEAD");
    let dated = run_libra_command(&["rev-list", "--date-order", "HEAD"], p);
    assert_cli_success(&dated, "rev-list --date-order HEAD");
    assert_eq!(
        String::from_utf8_lossy(&default.stdout),
        String::from_utf8_lossy(&dated.stdout),
        "--date-order matches the default ordering"
    );
}
