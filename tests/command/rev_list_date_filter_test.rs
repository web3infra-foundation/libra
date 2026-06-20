use git_internal::hash::{HashKind, set_hash_kind_for_test};

use super::*;

struct DateFilterRepo {
    repo: tempfile::TempDir,
    root_id: String,
    middle_id: String,
    tip_id: String,
    middle_ts: usize,
    tip_ts: usize,
}

fn create_date_filter_repo() -> DateFilterRepo {
    let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
    let repo = create_committed_repo_via_cli();
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (root_id, middle_id, tip_id, middle_ts, tip_ts) = runtime.block_on(async {
        let _guard = ChangeDirGuard::new(repo.path());
        let root_hash = Head::current_commit().await.expect("expected HEAD commit");
        let root: Commit = load_object(&root_hash).expect("failed to load root commit");
        let tree_id = root.tree_id;
        let middle_ts = root.committer.timestamp + 10;
        let tip_ts = root.committer.timestamp + 20;

        let mut middle_author = root.author.clone();
        let mut middle_committer = root.committer.clone();
        middle_author.timestamp = middle_ts;
        middle_committer.timestamp = middle_ts;
        let middle = Commit::new(
            middle_author,
            middle_committer,
            tree_id,
            vec![root_hash],
            "middle",
        );
        save_object(&middle, &middle.id).expect("failed to save middle commit");

        let mut tip_author = root.author.clone();
        let mut tip_committer = root.committer.clone();
        tip_author.timestamp = tip_ts;
        tip_committer.timestamp = tip_ts;
        let tip = Commit::new(tip_author, tip_committer, tree_id, vec![middle.id], "tip");
        save_object(&tip, &tip.id).expect("failed to save tip commit");
        Branch::update_branch("main", &tip.id.to_string(), None)
            .await
            .expect("failed to update main branch");

        (
            root_hash.to_string(),
            middle.id.to_string(),
            tip.id.to_string(),
            middle_ts,
            tip_ts,
        )
    });

    DateFilterRepo {
        repo,
        root_id,
        middle_id,
        tip_id,
        middle_ts,
        tip_ts,
    }
}

fn stdout_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn test_rev_list_since_until_filter_by_committer_timestamp() {
    let graph = create_date_filter_repo();
    let middle_ts = graph.middle_ts.to_string();
    let tip_ts = graph.tip_ts.to_string();

    let since = run_libra_command(
        &["rev-list", "--since", &middle_ts, "HEAD"],
        graph.repo.path(),
    );
    assert_cli_success(&since, "rev-list --since <middle-ts> HEAD");
    assert_eq!(
        stdout_lines(&since),
        vec![graph.tip_id.clone(), graph.middle_id.clone()]
    );

    let after = run_libra_command(
        &["rev-list", "--after", &middle_ts, "HEAD"],
        graph.repo.path(),
    );
    assert_cli_success(&after, "rev-list --after <middle-ts> HEAD");
    assert_eq!(after.stdout, since.stdout);

    let until = run_libra_command(
        &["rev-list", "--until", &middle_ts, "HEAD"],
        graph.repo.path(),
    );
    assert_cli_success(&until, "rev-list --until <middle-ts> HEAD");
    assert_eq!(
        stdout_lines(&until),
        vec![graph.middle_id.clone(), graph.root_id.clone()]
    );

    let before = run_libra_command(
        &["rev-list", "--before", &middle_ts, "HEAD"],
        graph.repo.path(),
    );
    assert_cli_success(&before, "rev-list --before <middle-ts> HEAD");
    assert_eq!(before.stdout, until.stdout);

    let window = run_libra_command(
        &[
            "rev-list",
            "--since",
            &middle_ts,
            "--until",
            &tip_ts,
            "--skip",
            "1",
            "--max-count",
            "1",
            "HEAD",
        ],
        graph.repo.path(),
    );
    assert_cli_success(
        &window,
        "rev-list --since <middle-ts> --until <tip-ts> --skip 1 --max-count 1 HEAD",
    );
    assert_eq!(stdout_lines(&window), vec![graph.middle_id.clone()]);
}

#[test]
fn test_rev_list_invalid_since_returns_cli_usage_error() {
    let graph = create_date_filter_repo();

    let output = run_libra_command(
        &["rev-list", "--since", "not-a-date", "HEAD"],
        graph.repo.path(),
    );
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("invalid --since date"));
    assert_eq!(report.error_code, "LBR-CLI-002");
}
