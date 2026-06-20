use super::*;

fn stdout_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn create_committer_filter_repo() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();

    let name = run_libra_command(&["config", "user.name", "Committer User"], repo.path());
    assert_cli_success(&name, "configure committer user.name");
    let email = run_libra_command(
        &["config", "user.email", "committer@example.com"],
        repo.path(),
    );
    assert_cli_success(&email, "configure committer user.email");

    fs::write(repo.path().join("committer.txt"), "committer\n")
        .expect("failed to write committer fixture");
    let add = run_libra_command(&["add", "committer.txt"], repo.path());
    assert_cli_success(&add, "add committer.txt");

    let commit = run_libra_command(
        &[
            "commit",
            "-m",
            "other committer",
            "--author",
            "Other Author <other-author@example.com>",
            "--no-verify",
        ],
        repo.path(),
    );
    assert_cli_success(&commit, "commit other committer");

    repo
}

#[test]
fn test_rev_list_committer_filters_by_name_or_email_case_insensitive() {
    let repo = create_committer_filter_repo();

    let tip = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&tip, "rev-parse HEAD");
    let tip_id = String::from_utf8_lossy(&tip.stdout).trim().to_string();

    let by_name = run_libra_command(
        &["rev-list", "--committer", "committer user", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&by_name, "rev-list --committer committer user HEAD");
    assert_eq!(stdout_lines(&by_name), vec![tip_id.clone()]);

    let by_email = run_libra_command(
        &["rev-list", "--committer", "COMMITTER@EXAMPLE.COM", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&by_email, "rev-list --committer COMMITTER@EXAMPLE.COM HEAD");
    assert_eq!(stdout_lines(&by_email), vec![tip_id]);

    let no_match = run_libra_command(
        &["rev-list", "--count", "--committer", "missing", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&no_match, "rev-list --count --committer missing HEAD");
    assert_eq!(String::from_utf8_lossy(&no_match.stdout).trim(), "0");
}

#[test]
fn test_rev_list_json_includes_committer_filter() {
    let repo = create_committer_filter_repo();

    let output = run_libra_command(
        &[
            "--json",
            "rev-list",
            "--committer",
            "committer",
            "--count",
            "HEAD",
        ],
        repo.path(),
    );
    assert_cli_success(&output, "json rev-list --committer committer --count HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["total"], 1);
    assert_eq!(json["data"]["committer"], "committer");
}
