use super::*;

fn stdout_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn test_rev_list_parents_and_timestamp_output_match_git_order() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (head_id, parent_id, head_timestamp, parent_timestamp) = runtime.block_on(async {
        let _guard = ChangeDirGuard::new(repo.path());
        let head_id = Head::current_commit().await.expect("expected HEAD commit");
        let head_commit: Commit = load_object(&head_id).expect("failed to load HEAD commit");
        let parent_id = head_commit.parent_commit_ids[0];
        let parent_commit: Commit = load_object(&parent_id).expect("failed to load parent commit");
        (
            head_id.to_string(),
            parent_id.to_string(),
            head_commit.committer.timestamp,
            parent_commit.committer.timestamp,
        )
    });

    let parents = run_libra_command(&["rev-list", "--parents", "HEAD"], repo.path());
    assert_cli_success(&parents, "rev-list --parents HEAD");
    assert_eq!(
        stdout_lines(&parents),
        vec![format!("{head_id} {parent_id}"), parent_id.clone()]
    );

    let timestamp = run_libra_command(&["rev-list", "--timestamp", "HEAD"], repo.path());
    assert_cli_success(&timestamp, "rev-list --timestamp HEAD");
    assert_eq!(
        stdout_lines(&timestamp),
        vec![
            format!("{head_timestamp} {head_id}"),
            format!("{parent_timestamp} {parent_id}"),
        ]
    );

    let combined = run_libra_command(
        &["rev-list", "--timestamp", "--parents", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&combined, "rev-list --timestamp --parents HEAD");
    assert_eq!(
        stdout_lines(&combined),
        vec![
            format!("{head_timestamp} {head_id} {parent_id}"),
            format!("{parent_timestamp} {parent_id}"),
        ]
    );

    let count = run_libra_command(
        &["rev-list", "--count", "--parents", "--timestamp", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&count, "rev-list --count --parents --timestamp HEAD");
    assert_eq!(String::from_utf8_lossy(&count.stdout).trim(), "2");
}

#[test]
fn test_rev_list_json_returns_envelope() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);

    let output = run_libra_command(&["--json", "rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "json rev-list HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "rev-list");
    assert_eq!(json["data"]["input"], "HEAD");
    assert_eq!(json["data"]["total"], 2);
    assert_eq!(json["data"]["commits"].as_array().map(Vec::len), Some(2));
}

#[test]
fn test_rev_list_json_entries_preserve_commit_ids_and_metadata() {
    let repo = create_two_commit_repo_with_direct_tip_update(1);
    let runtime = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let (head_id, parent_id, head_timestamp) = runtime.block_on(async {
        let _guard = ChangeDirGuard::new(repo.path());
        let head_id = Head::current_commit().await.expect("expected HEAD commit");
        let head_commit: Commit = load_object(&head_id).expect("failed to load HEAD commit");
        (
            head_id.to_string(),
            head_commit.parent_commit_ids[0].to_string(),
            head_commit.committer.timestamp,
        )
    });

    let output = run_libra_command(
        &["--json", "rev-list", "--parents", "--timestamp", "HEAD"],
        repo.path(),
    );
    assert_cli_success(&output, "json rev-list --parents --timestamp HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["commits"][0], head_id);
    assert_eq!(json["data"]["entries"][0]["commit"], head_id);
    assert_eq!(json["data"]["entries"][0]["parents"][0], parent_id);
    assert_eq!(json["data"]["entries"][0]["timestamp"], head_timestamp);
}

#[test]
fn test_rev_list_machine_returns_single_json_line() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--machine", "rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "machine rev-list HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line, got: {stdout}"
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("expected JSON");
    assert_eq!(parsed["command"], "rev-list");
    assert_eq!(parsed["data"]["input"], "HEAD");
}

#[test]
fn test_rev_list_quiet_suppresses_stdout() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--quiet", "rev-list", "HEAD"], repo.path());
    assert_cli_success(&output, "quiet rev-list HEAD");

    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        output.stdout
    );
}

/// `libra rev-list --help` surfaces the EXAMPLES banner so users see
/// the default HEAD walk, an explicit branch walk, a relative ref walk,
/// the JSON variant, and the quiet form without reading the design doc.
/// Cross-cutting `--help` EXAMPLES rollout per
/// `docs/development/commands/_general.md` item B.
#[test]
fn test_rev_list_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for rev-list --help");
    let output = run_libra_command(&["rev-list", "--help"], repo.path());
    assert!(
        output.status.success(),
        "rev-list --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "rev-list --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra rev-list",
        "libra rev-list --count HEAD",
        "libra rev-list -n 5 HEAD",
        "libra rev-list --merges HEAD",
        "libra rev-list --max-parents 0 HEAD",
        "libra rev-list --first-parent HEAD",
        "libra rev-list --author alice HEAD",
        "libra rev-list --committer alice HEAD",
        "libra rev-list --grep 'fix' HEAD",
        "libra rev-list --parents HEAD",
        "libra rev-list --timestamp HEAD",
        "libra rev-list main",
        "libra rev-list HEAD~5",
        "libra rev-list --json HEAD",
        "libra rev-list --quiet HEAD",
    ] {
        assert!(
            stdout.contains(invocation),
            "rev-list --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}
