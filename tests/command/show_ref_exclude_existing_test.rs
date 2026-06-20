use super::*;

fn head_hash(repo: &std::path::Path) -> String {
    let output = run_libra_command(&["rev-parse", "HEAD"], repo);
    assert_cli_success(&output, "rev-parse HEAD");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn show_ref_exclude_existing_filters_existing_refs_from_stdin() {
    // Given
    let repo = create_committed_repo_via_cli();
    let head = head_hash(repo.path());
    let stdin = format!(
        "{head} refs/heads/main\n{head} refs/heads/new\nrefs/tags/newtag\n{head} refs/heads/main^{{}}\n"
    );

    // When
    let output =
        run_libra_command_with_stdin(&["show-ref", "--exclude-existing"], repo.path(), &stdin);

    // Then
    assert_cli_success(&output, "show-ref --exclude-existing");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout,
        format!("{head} refs/heads/new\nrefs/tags/newtag\n"),
        "existing refs should be filtered while missing refs preserve their input line"
    );
}

#[test]
fn show_ref_exclude_existing_pattern_filters_ref_prefix() {
    // Given
    let repo = create_committed_repo_via_cli();
    let head = head_hash(repo.path());
    let stdin = format!("{head} refs/heads/new\n{head} refs/tags/newtag\n");

    // When
    let output = run_libra_command_with_stdin(
        &["show-ref", "--exclude-existing=refs/heads"],
        repo.path(),
        &stdin,
    );

    // Then
    assert_cli_success(&output, "show-ref --exclude-existing=refs/heads");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, format!("{head} refs/heads/new\n"));
}

#[test]
fn show_ref_exclude_existing_json_reports_filtered_entries() {
    // Given
    let repo = create_committed_repo_via_cli();
    let head = head_hash(repo.path());
    let stdin = format!("{head} refs/heads/new\n{head} refs/heads/main\n");

    // When
    let output = run_libra_command_with_stdin(
        &["--json", "show-ref", "--exclude-existing"],
        repo.path(),
        &stdin,
    );

    // Then
    assert_cli_success(&output, "show-ref --exclude-existing --json");
    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "show-ref");
    assert_eq!(json["data"]["exclude_existing"], true);
    assert_eq!(
        json["data"]["entries"][0]["line"],
        format!("{head} refs/heads/new")
    );
    assert_eq!(json["data"]["entries"][0]["refname"], "refs/heads/new");
}

#[test]
fn show_ref_exclude_existing_conflicts_with_verify() {
    // Given
    let repo = create_committed_repo_via_cli();

    // When
    let output = run_libra_command(
        &[
            "show-ref",
            "--exclude-existing",
            "--verify",
            "refs/heads/main",
        ],
        repo.path(),
    );

    // Then
    assert!(
        !output.status.success(),
        "--exclude-existing and --verify should conflict"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "expected clap conflict error, got: {stderr}"
    );
}
