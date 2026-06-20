use super::*;

fn stdout_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn create_grep_filter_repo() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("alpha.txt"), "alpha\n").expect("failed to write alpha fixture");
    let add = run_libra_command(&["add", "alpha.txt"], repo.path());
    assert_cli_success(&add, "add alpha.txt");
    let commit = run_libra_command(&["commit", "-m", "Alpha topic", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit Alpha topic");

    fs::write(repo.path().join("beta.txt"), "beta\n").expect("failed to write beta fixture");
    let add = run_libra_command(&["add", "beta.txt"], repo.path());
    assert_cli_success(&add, "add beta.txt");
    let commit = run_libra_command(&["commit", "-m", "Beta topic", "--no-verify"], repo.path());
    assert_cli_success(&commit, "commit Beta topic");

    repo
}

#[test]
fn test_rev_list_grep_filters_commit_messages_case_sensitive() {
    let repo = create_grep_filter_repo();

    let beta = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&beta, "rev-parse HEAD");
    let beta_id = String::from_utf8_lossy(&beta.stdout).trim().to_string();

    let alpha = run_libra_command(&["rev-parse", "HEAD~1"], repo.path());
    assert_cli_success(&alpha, "rev-parse HEAD~1");
    let alpha_id = String::from_utf8_lossy(&alpha.stdout).trim().to_string();

    let by_regex = run_libra_command(&["rev-list", "--grep", "Alpha.*topic", "HEAD"], repo.path());
    assert_cli_success(&by_regex, "rev-list --grep Alpha.*topic HEAD");
    assert_eq!(stdout_lines(&by_regex), vec![alpha_id.clone()]);

    let by_second_pattern = run_libra_command(
        &["rev-list", "--grep", "Alpha", "--grep", "Beta", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&by_second_pattern, "rev-list with multiple --grep");
    assert_eq!(stdout_lines(&by_second_pattern), vec![beta_id, alpha_id]);

    let case_miss = run_libra_command(
        &["rev-list", "--count", "--grep", "alpha", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&case_miss, "rev-list --count --grep alpha HEAD");
    assert_eq!(String::from_utf8_lossy(&case_miss.stdout).trim(), "0");
}

#[test]
fn test_rev_list_json_includes_grep_filter() {
    let repo = create_grep_filter_repo();

    let output = run_libra_command(
        &["--json", "rev-list", "--grep", "Beta", "--count", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&output, "json rev-list --grep Beta --count HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["total"], 1);
    assert_eq!(json["data"]["grep"], serde_json::json!(["Beta"]));
}

#[test]
fn test_rev_list_grep_rejects_invalid_regex_with_cli_error() {
    let repo = create_grep_filter_repo();

    let output = run_libra_command(&["rev-list", "--grep", "[", "HEAD"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("invalid --grep pattern '['"));
    assert_eq!(report.error_code, "LBR-CLI-002");
}
