use super::*;

#[test]
fn ls_remote_local_path_lists_head_and_branch_outside_repo() {
    let remote = create_committed_repo_via_cli();
    let outside = tempdir().expect("failed to create outside cwd");

    let remote_path = remote.path().to_string_lossy().to_string();
    let output = run_libra_command(&["ls-remote", &remote_path], outside.path());
    assert_cli_success(
        &output,
        "ls-remote local path should succeed outside a repo",
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\tHEAD"),
        "expected HEAD in ls-remote output, got: {stdout}"
    );
    assert!(
        stdout.contains("\trefs/heads/main"),
        "expected main branch in ls-remote output, got: {stdout}"
    );
}

#[test]
fn ls_remote_resolves_configured_remote_name() {
    let remote = create_committed_repo_via_cli();
    let local = create_committed_repo_via_cli();
    let remote_path = remote.path().to_string_lossy().to_string();

    let output = run_libra_command(&["remote", "add", "origin", &remote_path], local.path());
    assert_cli_success(&output, "remote add should succeed");

    let output = run_libra_command(&["ls-remote", "origin"], local.path());
    assert_cli_success(&output, "ls-remote origin should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\trefs/heads/main"),
        "expected configured remote main branch, got: {stdout}"
    );
}

#[test]
fn ls_remote_heads_pattern_filters_refs() {
    let remote = create_committed_repo_via_cli();
    let outside = tempdir().expect("failed to create outside cwd");
    let remote_path = remote.path().to_string_lossy().to_string();

    let output = run_libra_command(
        &["ls-remote", "--heads", &remote_path, "main"],
        outside.path(),
    );
    assert_cli_success(&output, "ls-remote --heads main should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\trefs/heads/main"),
        "expected main branch in filtered output, got: {stdout}"
    );
    assert!(
        !stdout.contains("\tHEAD"),
        "--heads should not include HEAD, got: {stdout}"
    );
}

#[test]
fn ls_remote_json_reports_entries() {
    let remote = create_committed_repo_via_cli();
    let outside = tempdir().expect("failed to create outside cwd");
    let remote_path = remote.path().to_string_lossy().to_string();

    let output = run_libra_command(
        &["--json=compact", "ls-remote", &remote_path],
        outside.path(),
    );
    assert_cli_success(&output, "json ls-remote should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "ls-remote");
    let entries = json["data"]["entries"]
        .as_array()
        .expect("entries should be an array");
    assert!(
        entries
            .iter()
            .any(|entry| entry["refname"] == "refs/heads/main"),
        "expected refs/heads/main in entries: {json}"
    );
}
