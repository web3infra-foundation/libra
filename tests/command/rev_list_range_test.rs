use git_internal::hash::{HashKind, set_hash_kind_for_test};

use super::*;

struct RangeRepo {
    repo: tempfile::TempDir,
    root_id: String,
    main_id: String,
    side_id: String,
}

fn create_range_repo() -> RangeRepo {
    let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
    let repo = create_committed_repo_via_cli();
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (root_id, main_id, side_id) = runtime.block_on(async {
        let _guard = ChangeDirGuard::new(repo.path());
        let root_hash = Head::current_commit().await.expect("expected HEAD commit");
        let root: Commit = load_object(&root_hash).expect("failed to load root commit");
        let tree_id = root.tree_id;

        let mut main_author = root.author.clone();
        let mut main_committer = root.committer.clone();
        main_author.timestamp = root.committer.timestamp + 1;
        main_committer.timestamp = root.committer.timestamp + 1;
        let main = Commit::new(
            main_author,
            main_committer,
            tree_id,
            vec![root_hash],
            "main branch",
        );
        save_object(&main, &main.id).expect("failed to save main commit");

        let mut side_author = root.author.clone();
        let mut side_committer = root.committer.clone();
        side_author.timestamp = root.committer.timestamp + 2;
        side_committer.timestamp = root.committer.timestamp + 2;
        let side = Commit::new(
            side_author,
            side_committer,
            tree_id,
            vec![root_hash],
            "feature branch",
        );
        save_object(&side, &side.id).expect("failed to save feature commit");

        Branch::update_branch("main", &main.id.to_string(), None)
            .await
            .expect("failed to update main branch");
        Branch::update_branch("feature", &side.id.to_string(), None)
            .await
            .expect("failed to create feature branch");

        (
            root_hash.to_string(),
            main.id.to_string(),
            side.id.to_string(),
        )
    });

    RangeRepo {
        repo,
        root_id,
        main_id,
        side_id,
    }
}

fn stdout_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn test_rev_list_accepts_multiple_positive_revisions_without_duplicates() {
    let graph = create_range_repo();

    let output = run_libra_command(&["rev-list", "main", "feature"], graph.repo.path());
    assert_cli_success(&output, "rev-list main feature");

    assert_eq!(
        stdout_lines(&output),
        vec![
            graph.side_id.clone(),
            graph.main_id.clone(),
            graph.root_id.clone(),
        ]
    );
}

#[test]
fn test_rev_list_supports_range_and_exclusion_specs() {
    let graph = create_range_repo();

    let dotted = run_libra_command(
        &["rev-list", &format!("{}..feature", graph.root_id)],
        graph.repo.path(),
    );
    assert_cli_success(&dotted, "rev-list root..feature");
    assert_eq!(stdout_lines(&dotted), vec![graph.side_id.clone()]);

    let caret = run_libra_command(
        &["rev-list", &format!("^{}", graph.root_id), "feature"],
        graph.repo.path(),
    );
    assert_cli_success(&caret, "rev-list ^root feature");
    assert_eq!(stdout_lines(&caret), vec![graph.side_id.clone()]);
}

#[test]
fn test_rev_list_supports_symmetric_difference_specs() {
    let graph = create_range_repo();

    let output = run_libra_command(&["rev-list", "main...feature"], graph.repo.path());
    assert_cli_success(&output, "rev-list main...feature");

    assert_eq!(
        stdout_lines(&output),
        vec![graph.side_id.clone(), graph.main_id.clone()]
    );
}

#[test]
fn test_rev_list_parent_filter_reset_aliases_remove_bounds() {
    let graph = create_range_repo();

    let no_min = run_libra_command(
        &[
            "rev-list",
            "--count",
            "--min-parents",
            "1",
            "--no-min-parents",
            "main",
        ],
        graph.repo.path(),
    );
    assert_cli_success(
        &no_min,
        "rev-list --count --min-parents 1 --no-min-parents main",
    );
    assert_eq!(String::from_utf8_lossy(&no_min.stdout).trim(), "2");

    let no_max = run_libra_command(
        &[
            "rev-list",
            "--count",
            "--max-parents",
            "0",
            "--no-max-parents",
            "main",
        ],
        graph.repo.path(),
    );
    assert_cli_success(
        &no_max,
        "rev-list --count --max-parents 0 --no-max-parents main",
    );
    assert_eq!(String::from_utf8_lossy(&no_max.stdout).trim(), "2");
}
