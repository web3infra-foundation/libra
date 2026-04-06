//! CLI-level tests for the `describe` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use super::*;

#[test]
fn test_describe_json_returns_tag_match() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], repo.path());
    assert_cli_success(&tag_output, "failed to create tag for describe test");

    let output = run_libra_command(&["describe", "--json"], repo.path());
    assert_cli_success(&output, "describe --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "describe");
    assert_eq!(json["data"]["result"], "v1.0");
    assert_eq!(json["data"]["tag"], "v1.0");
    assert_eq!(json["data"]["distance"], 0);
    assert_eq!(json["data"]["used_always"], false);
}

#[test]
fn test_describe_tags_json_includes_lightweight_tag() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "v1.0"], repo.path());
    assert_cli_success(
        &tag_output,
        "failed to create lightweight tag for describe test",
    );

    let output = run_libra_command(&["describe", "--tags", "--json"], repo.path());
    assert_cli_success(&output, "describe --tags --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "describe");
    assert_eq!(json["data"]["result"], "v1.0");
    assert_eq!(json["data"]["tag"], "v1.0");
    assert_eq!(json["data"]["distance"], 0);
    assert_eq!(json["data"]["used_always"], false);
}

#[test]
fn test_describe_always_json_without_tags_returns_abbrev_commit() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["describe", "--always", "--json"], repo.path());
    assert_cli_success(&output, "describe --always --json should succeed");

    let json = parse_json_stdout(&output);
    let result = json["data"]["result"]
        .as_str()
        .expect("result should be a string");
    assert_eq!(
        result.len(),
        7,
        "default abbreviated commit length should be 7"
    );
    assert!(json["data"]["tag"].is_null());
    assert_eq!(json["data"]["used_always"], true);
}

#[test]
fn test_describe_always_respects_explicit_abbrev_length() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["describe", "--always", "--abbrev=5", "--json"],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "describe --always --abbrev=5 --json should succeed",
    );

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"].as_str().unwrap().len(), 5);
    assert_eq!(
        json["data"]["abbreviated_commit"].as_str().unwrap().len(),
        5
    );
    assert_eq!(json["data"]["used_always"], true);
}

#[test]
fn test_describe_always_abbrev_zero_returns_full_hash() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(
        &["describe", "--always", "--abbrev=0", "--json"],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "describe --always --abbrev=0 --json should succeed",
    );

    let json = parse_json_stdout(&output);
    let resolved_commit = json["data"]["resolved_commit"]
        .as_str()
        .expect("resolved_commit should be a string");
    assert_eq!(json["data"]["result"], resolved_commit);
    assert_eq!(json["data"]["abbreviated_commit"], resolved_commit);
    assert_eq!(json["data"]["used_always"], true);
}

#[test]
fn test_describe_abbrev_zero_keeps_git_tag_only_output() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], repo.path());
    assert_cli_success(&tag_output, "failed to create tag for describe test");

    std::fs::write(repo.path().join("tracked.txt"), "tracked\nnext\n")
        .expect("failed to update tracked file");
    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add_output, "failed to stage updated tracked file");
    let commit_output = run_libra_command(&["commit", "-m", "next", "--no-verify"], repo.path());
    assert_cli_success(&commit_output, "failed to create second commit");

    let output = run_libra_command(&["describe", "--abbrev=0", "--json"], repo.path());
    assert_cli_success(&output, "describe --abbrev=0 --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0");
    assert_eq!(json["data"]["tag"], "v1.0");
    assert_eq!(json["data"]["distance"], 1);
    assert!(json["data"]["abbreviated_commit"].is_null());
    assert_eq!(json["data"]["used_always"], false);
}

#[test]
fn test_describe_tags_prefers_annotated_tag_over_lightweight_tag() {
    let repo = create_committed_repo_via_cli();

    let lightweight = run_libra_command(&["tag", "v-light"], repo.path());
    assert_cli_success(&lightweight, "failed to create lightweight tag");

    let annotated = run_libra_command(&["tag", "-m", "Release v-ann", "v-ann"], repo.path());
    assert_cli_success(&annotated, "failed to create annotated tag");

    let output = run_libra_command(&["describe", "--tags", "--json"], repo.path());
    assert_cli_success(&output, "describe --tags --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v-ann");
    assert_eq!(json["data"]["tag"], "v-ann");
    assert_eq!(json["data"]["distance"], 0);
}

#[test]
fn test_describe_invalid_reference_returns_cli_invalid_target() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["describe", "missing-ref"], repo.path());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-003");
}
