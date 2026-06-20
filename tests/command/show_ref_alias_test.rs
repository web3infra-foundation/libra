use super::*;

fn stdout_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn create_repo_with_tag() -> tempfile::TempDir {
    let repo = create_committed_repo_via_cli();
    let tag = run_libra_command(&["tag", "-m", "release notes", "v1.0"], repo.path());
    assert_cli_success(&tag, "tag -m release notes v1.0");
    repo
}

#[test]
fn test_show_ref_branches_alias_matches_heads() {
    let repo = create_committed_repo_via_cli();

    let heads = run_libra_command(&["show-ref", "--heads"], repo.path());
    assert_cli_success(&heads, "show-ref --heads");
    let branches = run_libra_command(&["show-ref", "--branches"], repo.path());
    assert_cli_success(&branches, "show-ref --branches");

    assert_eq!(branches.stdout, heads.stdout);
    assert!(branches.stderr.is_empty());
}

#[test]
fn test_show_ref_branches_alias_combines_with_tags() {
    let repo = create_committed_repo_via_cli();
    let tag = run_libra_command(&["tag", "v1.0"], repo.path());
    assert_cli_success(&tag, "tag v1.0");

    let output = run_libra_command(&["show-ref", "--branches", "--tags"], repo.path());
    assert_cli_success(&output, "show-ref --branches --tags");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("refs/heads/main"),
        "unexpected output: {stdout}"
    );
    assert!(
        stdout.contains("refs/tags/v1.0"),
        "unexpected output: {stdout}"
    );
}

#[test]
fn test_show_ref_branches_alias_filters_json_to_heads() {
    let repo = create_committed_repo_via_cli();
    let tag = run_libra_command(&["tag", "v1.0"], repo.path());
    assert_cli_success(&tag, "tag v1.0");

    let output = run_libra_command(&["--json", "show-ref", "--branches"], repo.path());
    assert_cli_success(&output, "show-ref --json --branches");
    let json = parse_json_stdout(&output);
    let entries = json["data"]["entries"]
        .as_array()
        .expect("entries should be an array");

    assert!(
        entries
            .iter()
            .any(|entry| entry["refname"] == "refs/heads/main"),
        "branch entry should be present: {json}"
    );
    assert!(
        entries.iter().all(|entry| !entry["refname"]
            .as_str()
            .unwrap_or("")
            .starts_with("refs/tags/")),
        "tag entries should be filtered out: {json}"
    );
}

#[test]
fn test_show_ref_no_branches_resets_branches_filter() {
    let repo = create_repo_with_tag();

    let output = run_libra_command(&["show-ref", "--branches", "--no-branches"], repo.path());
    assert_cli_success(&output, "show-ref --branches --no-branches");
    let stdout = stdout_text(&output);

    assert!(
        stdout.contains("refs/heads/main"),
        "branch entry should remain after reset: {stdout}"
    );
    assert!(
        stdout.contains("refs/tags/v1.0"),
        "tag entry should return after reset to default scope: {stdout}"
    );
}

#[test]
fn test_show_ref_no_tags_resets_tags_filter() {
    let repo = create_repo_with_tag();

    let output = run_libra_command(&["show-ref", "--tags", "--no-tags"], repo.path());
    assert_cli_success(&output, "show-ref --tags --no-tags");
    let stdout = stdout_text(&output);

    assert!(
        stdout.contains("refs/heads/main"),
        "branch entry should return after reset to default scope: {stdout}"
    );
    assert!(
        stdout.contains("refs/tags/v1.0"),
        "tag entry should remain after reset: {stdout}"
    );
}

#[test]
fn test_show_ref_no_head_resets_head_flag() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["show-ref", "--head", "--no-head"], repo.path());
    assert_cli_success(&output, "show-ref --head --no-head");
    let stdout = stdout_text(&output);

    assert!(
        !stdout.lines().any(|line| line.ends_with(" HEAD")),
        "HEAD should be omitted after --no-head: {stdout}"
    );
}

#[test]
fn test_show_ref_no_dereference_resets_dereference_flag() {
    let repo = create_repo_with_tag();

    let output = run_libra_command(
        &["show-ref", "--dereference", "--no-dereference", "--tags"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --dereference --no-dereference --tags");
    let stdout = stdout_text(&output);

    assert!(
        !stdout.contains("^{}"),
        "peeled tag entry should be omitted after --no-dereference: {stdout}"
    );
}

#[test]
fn test_show_ref_no_abbrev_resets_abbrev_width() {
    let repo = create_committed_repo_via_cli();
    let head = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&head, "rev-parse HEAD");
    let expected = stdout_text(&head).trim().to_string();

    let output = run_libra_command(
        &["show-ref", "--abbrev=7", "--no-abbrev", "--heads"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --abbrev=7 --no-abbrev --heads");
    let stdout = stdout_text(&output);
    let actual = stdout
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("");

    assert_eq!(actual, expected, "hash should be full-width: {stdout}");
}

#[test]
fn test_show_ref_no_hash_is_git_compatible_hash_only_alias() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["show-ref", "--no-hash", "--heads"], repo.path());
    assert_cli_success(&output, "show-ref --no-hash --heads");
    let stdout = stdout_text(&output);

    assert_eq!(stdout.lines().count(), 1, "unexpected output: {stdout}");
    assert!(
        !stdout.contains("refs/heads/main"),
        "--no-hash should follow Git and behave as hash-only: {stdout}"
    );
}

#[test]
fn test_show_ref_no_verify_resets_verify_mode() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["show-ref", "--verify", "--no-verify", "main"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --verify --no-verify main");
    let stdout = stdout_text(&output);

    assert!(
        stdout.contains("refs/heads/main"),
        "non-exact pattern should work after --no-verify reset: {stdout}"
    );
}

#[test]
fn test_show_ref_no_exists_resets_exists_mode() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["show-ref", "--exists", "--no-exists", "refs/heads/main"],
        repo.path(),
    );
    assert_cli_success(&output, "show-ref --exists --no-exists refs/heads/main");
    let stdout = stdout_text(&output);

    assert!(
        stdout.contains("refs/heads/main"),
        "normal show-ref output should return after --no-exists reset: {stdout}"
    );
}
