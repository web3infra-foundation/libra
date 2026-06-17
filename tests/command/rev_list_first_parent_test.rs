use git_internal::hash::{HashKind, set_hash_kind_for_test};

use super::*;

struct FirstParentRepo {
    repo: tempfile::TempDir,
    root_id: String,
    main_id: String,
    merge_id: String,
}

fn create_first_parent_repo() -> FirstParentRepo {
    let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
    let repo = create_committed_repo_via_cli();
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (root_id, main_id, merge_id) = runtime.block_on(async {
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

        let mut merge_author = root.author.clone();
        let mut merge_committer = root.committer.clone();
        merge_author.timestamp = root.committer.timestamp + 3;
        merge_committer.timestamp = root.committer.timestamp + 3;
        let merge = Commit::new(
            merge_author,
            merge_committer,
            tree_id,
            vec![main_hash, side.id],
            "merge side",
        );
        save_object(&merge, &merge.id).expect("failed to save merge commit");
        Branch::update_branch("main", &merge.id.to_string(), None)
            .await
            .expect("failed to update main branch");

        (
            root_hash.to_string(),
            main_hash.to_string(),
            merge.id.to_string(),
        )
    });

    FirstParentRepo {
        repo,
        root_id,
        main_id,
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
fn test_rev_list_first_parent_follows_mainline_only() {
    let graph = create_first_parent_repo();

    let output = run_libra_command(&["rev-list", "--first-parent", "HEAD"], graph.repo.path());
    assert_cli_success(&output, "rev-list --first-parent HEAD");

    assert_eq!(
        stdout_lines(&output),
        vec![graph.merge_id.clone(), graph.main_id.clone(), graph.root_id]
    );
}

#[test]
fn test_rev_list_json_includes_first_parent_flag() {
    let graph = create_first_parent_repo();

    let output = run_libra_command(
        &["--json", "rev-list", "--first-parent", "--count", "HEAD"],
        graph.repo.path(),
    );
    assert_cli_success(&output, "json rev-list --first-parent --count HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["total"], 3);
    assert_eq!(json["data"]["first_parent"], true);
}
