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
fn test_describe_invalid_reference_returns_cli_invalid_target() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["describe", "missing-ref"], repo.path());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-003");
}
