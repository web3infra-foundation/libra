use super::*;

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
