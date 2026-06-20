use super::*;

struct PathFilterRepo {
    repo: tempfile::TempDir,
    base_id: String,
    a_id: String,
    b_id: String,
}

fn create_path_filter_repo() -> PathFilterRepo {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());
    configure_identity_via_cli(repo.path());

    fs::write(repo.path().join("a.txt"), "base\n").expect("failed to write a.txt");
    fs::write(repo.path().join("b.txt"), "base\n").expect("failed to write b.txt");
    let output = run_libra_command(&["add", "a.txt", "b.txt"], repo.path());
    assert_cli_success(&output, "add base files");
    let output = run_libra_command(&["commit", "-m", "base", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit base");
    let base_id = head_commit_id(repo.path());

    fs::write(repo.path().join("a.txt"), "base\na-only\n").expect("failed to update a.txt");
    let output = run_libra_command(&["add", "a.txt"], repo.path());
    assert_cli_success(&output, "add a.txt change");
    let output = run_libra_command(&["commit", "-m", "update a", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit a change");
    let a_id = head_commit_id(repo.path());

    fs::write(repo.path().join("b.txt"), "base\nb-only\n").expect("failed to update b.txt");
    let output = run_libra_command(&["add", "b.txt"], repo.path());
    assert_cli_success(&output, "add b.txt change");
    let output = run_libra_command(&["commit", "-m", "update b", "--no-verify"], repo.path());
    assert_cli_success(&output, "commit b change");
    let b_id = head_commit_id(repo.path());

    PathFilterRepo {
        repo,
        base_id,
        a_id,
        b_id,
    }
}

fn head_commit_id(repo: &Path) -> String {
    let output = run_libra_command(&["rev-parse", "HEAD"], repo);
    assert_cli_success(&output, "rev-parse HEAD");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn stdout_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn test_rev_list_path_filter_limits_commits_after_separator() {
    let graph = create_path_filter_repo();

    let output = run_libra_command(&["rev-list", "HEAD", "--", "a.txt"], graph.repo.path());
    assert_cli_success(&output, "rev-list HEAD -- a.txt");

    assert_eq!(
        stdout_lines(&output),
        vec![graph.a_id.clone(), graph.base_id.clone()]
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(&graph.b_id),
        "path-limited rev-list must exclude commits that only touched b.txt"
    );
}

#[test]
fn test_rev_list_json_includes_pathspec_filter() {
    let graph = create_path_filter_repo();

    let output = run_libra_command(
        &["--json", "rev-list", "HEAD", "--", "b.txt"],
        graph.repo.path(),
    );
    assert_cli_success(&output, "--json rev-list HEAD -- b.txt");
    let json = parse_json_stdout(&output);

    assert_eq!(json["data"]["pathspecs"], serde_json::json!(["b.txt"]));
    assert_eq!(
        json["data"]["commits"],
        serde_json::json!([graph.b_id, graph.base_id])
    );
}
