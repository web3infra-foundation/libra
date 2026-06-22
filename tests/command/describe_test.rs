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
fn test_describe_lowercase_head_resolves_named_ref_not_head() {
    let repo = create_committed_repo_via_cli();

    let first_tag = run_libra_command(&["tag", "-m", "Release v1", "v1"], repo.path());
    assert_cli_success(&first_tag, "failed to create first describe tag");

    let branch_output = run_libra_command(&["branch", "head"], repo.path());
    assert_cli_success(&branch_output, "failed to create lowercase head branch");

    std::fs::write(repo.path().join("tracked.txt"), "tracked\nnext\n")
        .expect("failed to update tracked file");
    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add_output, "failed to stage updated tracked file");
    let commit_output = run_libra_command(&["commit", "-m", "next", "--no-verify"], repo.path());
    assert_cli_success(&commit_output, "failed to create second commit");

    let second_tag = run_libra_command(&["tag", "-m", "Release v2", "v2"], repo.path());
    assert_cli_success(&second_tag, "failed to create second describe tag");

    let current_output = run_libra_command(&["describe", "--json"], repo.path());
    assert_cli_success(&current_output, "describe --json should succeed");
    let current_json = parse_json_stdout(&current_output);
    assert_eq!(current_json["data"]["result"], "v2");

    let output = run_libra_command(&["describe", "head", "--json"], repo.path());
    assert_cli_success(&output, "describe head --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1");
    assert_eq!(json["data"]["tag"], "v1");
    assert_eq!(json["data"]["distance"], 0);
    assert_eq!(json["data"]["exact_match"], true);
}

#[test]
fn test_describe_invalid_reference_returns_cli_invalid_target() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["describe", "missing-ref"], repo.path());
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[test]
fn test_describe_exact_match_fails_after_head_moves_past_tag() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], repo.path());
    assert_cli_success(&tag_output, "failed to create tag for describe test");

    std::fs::write(repo.path().join("tracked.txt"), "tracked\nnext\n")
        .expect("failed to update tracked file");
    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add_output, "failed to stage updated tracked file");
    let commit_output = run_libra_command(&["commit", "-m", "next", "--no-verify"], repo.path());
    assert_cli_success(&commit_output, "failed to create second commit");

    let output = run_libra_command(&["describe", "--exact-match"], repo.path());
    let (human, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert_eq!(report.error_code, "LBR-REPO-003");
    assert!(
        human.contains("no tag exactly matches"),
        "exact-match failure should explain why describe failed: {human}"
    );
}

#[test]
fn test_describe_exact_match_succeeds_on_tagged_head() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], repo.path());
    assert_cli_success(&tag_output, "failed to create tag for describe test");

    let output = run_libra_command(&["describe", "--exact-match", "--json"], repo.path());
    assert_cli_success(&output, "describe --exact-match --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0");
    assert_eq!(json["data"]["exact_match"], true);
    assert_eq!(json["data"]["dirty"], false);
}

#[test]
fn test_describe_dirty_appends_default_suffix_for_tracked_changes() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], repo.path());
    assert_cli_success(&tag_output, "failed to create tag for describe test");
    std::fs::write(repo.path().join("tracked.txt"), "tracked\nchanged\n")
        .expect("failed to dirty tracked file");

    let output = run_libra_command(&["describe", "--dirty", "--json"], repo.path());
    assert_cli_success(&output, "describe --dirty --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0-dirty");
    assert_eq!(json["data"]["dirty"], true);
    assert_eq!(json["data"]["dirty_mark"], "-dirty");
}

#[test]
fn test_describe_dirty_ignores_untracked_files_like_git() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], repo.path());
    assert_cli_success(&tag_output, "failed to create tag for describe test");
    std::fs::write(repo.path().join("untracked.txt"), "untracked\n")
        .expect("failed to write untracked file");

    let output = run_libra_command(&["describe", "--dirty", "--json"], repo.path());
    assert_cli_success(&output, "describe --dirty --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0");
    assert_eq!(json["data"]["dirty"], false);
    assert!(json["data"]["dirty_mark"].is_null());
}

#[test]
fn test_describe_dirty_accepts_custom_suffix() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], repo.path());
    assert_cli_success(&tag_output, "failed to create tag for describe test");
    std::fs::write(repo.path().join("tracked.txt"), "tracked\nchanged\n")
        .expect("failed to dirty tracked file");

    let output = run_libra_command(&["describe", "--dirty=-worktree", "--json"], repo.path());
    assert_cli_success(&output, "describe --dirty=-worktree --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0-worktree");
    assert_eq!(json["data"]["dirty"], true);
    assert_eq!(json["data"]["dirty_mark"], "-worktree");
}

/// `--match` keeps only tags whose name matches the glob; a non-matching tag that
/// would otherwise win the lexicographic tie-break is filtered out.
#[test]
fn test_describe_match_single_glob() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "a-other"], repo.path()),
        "tag a-other",
    );
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    // Without --match the lexicographic tie-break picks "a-other"; --match forces v1.0.
    let output = run_libra_command(&["describe", "--match", "v1.*", "--json"], repo.path());
    assert_cli_success(&output, "describe --match v1.* should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["tag"], "v1.0");
    assert_eq!(json["data"]["result"], "v1.0");
}

/// Multiple `--match` globs are OR-combined: matching any one is enough.
#[test]
fn test_describe_match_multiple_globs_or() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "a-other"], repo.path()),
        "tag a-other",
    );
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    let output = run_libra_command(
        &["describe", "--match", "x*", "--match", "v1.*", "--json"],
        repo.path(),
    );
    assert_cli_success(&output, "describe with two --match globs should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["tag"], "v1.0");
}

/// `--exclude` removes a matched tag even when it would otherwise win.
#[test]
fn test_describe_exclude_filters_matched_tag() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "a-rc"], repo.path()),
        "tag a-rc",
    );
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    // Without exclude the tie-break would pick "a-rc"; --exclude removes it.
    let output = run_libra_command(&["describe", "--exclude", "*rc*", "--json"], repo.path());
    assert_cli_success(&output, "describe --exclude *rc* should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["tag"], "v1.0");
}

/// `--match` and `--exclude` combine: exclude takes precedence over match.
#[test]
fn test_describe_match_and_exclude_combined() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0-rc"], repo.path()),
        "tag v1.0-rc",
    );
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    let output = run_libra_command(
        &["describe", "--match", "v1.*", "--exclude", "*rc*", "--json"],
        repo.path(),
    );
    assert_cli_success(&output, "describe --match + --exclude should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["tag"], "v1.0");
}

/// A glob longer than 256 chars is rejected at the parsing stage (129).
#[test]
fn test_describe_glob_over_256_chars_rejected() {
    let repo = create_committed_repo_via_cli();
    let pattern = "a".repeat(257);
    let output = run_libra_command(&["describe", "--match", &pattern], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "overlong glob should be rejected: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

/// A malformed glob pattern is rejected with a usage error rather than panicking.
#[test]
fn test_describe_invalid_glob_rejected() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["describe", "--match", "v{1"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "invalid glob should be rejected: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

/// `--first-parent` is accepted and produces a normal description on a linear
/// history (where it has no effect beyond the default walk).
#[test]
fn test_describe_first_parent_linear_history() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    let output = run_libra_command(&["describe", "--first-parent", "--json"], repo.path());
    assert_cli_success(&output, "describe --first-parent should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["tag"], "v1.0");
    assert_eq!(json["data"]["result"], "v1.0");
}

#[test]
fn test_describe_candidates_zero_requires_exact_match() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    let tag = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], p);
    assert_cli_success(&tag, "create annotated tag v1.0");

    // HEAD is exactly tagged: `--candidates 0` succeeds with the exact match
    // (Git documents `--candidates 0` as "only exact matches").
    let output = run_libra_command(&["describe", "--candidates", "0"], p);
    assert_cli_success(&output, "describe --candidates 0 on exact tag");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "v1.0");

    // Advance HEAD past the tag.
    std::fs::write(p.join("tracked.txt"), "next\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], p),
        "add tracked.txt",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "next", "--no-verify"], p),
        "commit next",
    );

    // `--candidates 0` now fails (no exact match), exactly like `--exact-match`.
    let output = run_libra_command(&["describe", "--candidates", "0"], p);
    assert!(
        !output.status.success(),
        "describe --candidates 0 should fail when HEAD is not exactly tagged"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no tag exactly matches"),
        "expected exact-match failure: {stderr}"
    );

    // `--candidates 5` (>=1) keeps the normal behavior: the nearest tag.
    let output = run_libra_command(&["describe", "--tags", "--candidates", "5", "--json"], p);
    assert_cli_success(&output, "describe --candidates 5");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["tag"], "v1.0");

    // A non-integer value is rejected at the clap layer.
    let output = run_libra_command(&["describe", "--candidates", "abc"], p);
    assert!(
        !output.status.success(),
        "describe --candidates abc should be rejected"
    );
}
