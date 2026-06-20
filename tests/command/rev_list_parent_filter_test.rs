use git_internal::hash::{HashKind, set_hash_kind_for_test};

use super::*;

struct ParentFilterRepo {
    repo: tempfile::TempDir,
    root_id: String,
    main_id: String,
    side_id: String,
    merge_id: String,
}

fn create_parent_filter_repo() -> ParentFilterRepo {
    let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
    let repo = create_committed_repo_via_cli();
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (root_id, main_id, side_id, merge_id) = runtime.block_on(async {
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
            "main side",
        );
        save_object(&main, &main.id).expect("failed to save main commit");
        let main_hash = main.id;

        let mut side_author = root.author.clone();
        let mut side_committer = root.committer.clone();
        side_author.timestamp = root.committer.timestamp + 2;
        side_committer.timestamp = root.committer.timestamp + 2;
        let side = Commit::new(
            side_author,
            side_committer,
            tree_id,
            vec![root_hash],
            "side branch",
        );
        save_object(&side, &side.id).expect("failed to save side commit");
        let side_hash = side.id;

        let mut merge_author = root.author.clone();
        let mut merge_committer = root.committer.clone();
        merge_author.timestamp = root.committer.timestamp + 3;
        merge_committer.timestamp = root.committer.timestamp + 3;
        let merge = Commit::new(
            merge_author,
            merge_committer,
            tree_id,
            vec![main_hash, side_hash],
            "merge side",
        );
        save_object(&merge, &merge.id).expect("failed to save merge commit");
        Branch::update_branch("main", &merge.id.to_string(), None)
            .await
            .expect("failed to update main branch");

        (
            root_hash.to_string(),
            main_hash.to_string(),
            side_hash.to_string(),
            merge.id.to_string(),
        )
    });

    ParentFilterRepo {
        repo,
        root_id,
        main_id,
        side_id,
        merge_id,
    }
}

fn stdout_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn test_rev_list_parent_count_filters_match_git_shapes() {
    let graph = create_parent_filter_repo();

    let merges = run_libra_command(&["rev-list", "--merges", "HEAD"], graph.repo.path());
    assert_cli_success(&merges, "rev-list --merges HEAD");
    assert_eq!(stdout_lines(&merges), vec![graph.merge_id.clone()]);

    let no_merges = run_libra_command(&["rev-list", "--no-merges", "HEAD"], graph.repo.path());
    assert_cli_success(&no_merges, "rev-list --no-merges HEAD");
    assert_eq!(
        stdout_lines(&no_merges),
        vec![
            graph.side_id.clone(),
            graph.main_id.clone(),
            graph.root_id.clone(),
        ]
    );

    let min_parents = run_libra_command(
        &["rev-list", "--min-parents", "1", "HEAD"],
        graph.repo.path(),
    );
    assert_cli_success(&min_parents, "rev-list --min-parents 1 HEAD");
    assert_eq!(
        stdout_lines(&min_parents),
        vec![
            graph.merge_id.clone(),
            graph.side_id.clone(),
            graph.main_id.clone(),
        ]
    );

    let max_parents = run_libra_command(
        &["rev-list", "--max-parents", "0", "HEAD"],
        graph.repo.path(),
    );
    assert_cli_success(&max_parents, "rev-list --max-parents 0 HEAD");
    assert_eq!(stdout_lines(&max_parents), vec![graph.root_id.clone()]);

    let single_parent = run_libra_command(
        &[
            "rev-list",
            "--min-parents",
            "1",
            "--max-parents",
            "1",
            "HEAD",
        ],
        graph.repo.path(),
    );
    assert_cli_success(
        &single_parent,
        "rev-list --min-parents 1 --max-parents 1 HEAD",
    );
    assert_eq!(
        stdout_lines(&single_parent),
        vec![graph.side_id.clone(), graph.main_id.clone()]
    );

    let intersection = run_libra_command(
        &["rev-list", "--merges", "--no-merges", "HEAD"],
        graph.repo.path(),
    );
    assert_cli_success(&intersection, "rev-list --merges --no-merges HEAD");
    assert!(stdout_lines(&intersection).is_empty());

    let count = run_libra_command(
        &["rev-list", "--count", "--merges", "HEAD"],
        graph.repo.path(),
    );
    assert_cli_success(&count, "rev-list --count --merges HEAD");
    assert_eq!(String::from_utf8_lossy(&count.stdout).trim(), "1");
}

#[test]
fn test_rev_list_json_includes_parent_count_filters() {
    let graph = create_parent_filter_repo();

    let output = run_libra_command(
        &[
            "--json",
            "rev-list",
            "--min-parents",
            "1",
            "--max-parents",
            "1",
            "HEAD",
        ],
        graph.repo.path(),
    );
    assert_cli_success(
        &output,
        "json rev-list --min-parents 1 --max-parents 1 HEAD",
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["commits"][0], graph.side_id);
    assert_eq!(json["data"]["commits"][1], graph.main_id);
    assert_eq!(json["data"]["total"], 2);
    assert_eq!(json["data"]["min_parents"], 1);
    assert_eq!(json["data"]["max_parents"], 1);
}
