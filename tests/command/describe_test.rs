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
fn test_describe_no_tags_lightweight_only_errors() {
    let repo = create_committed_repo_via_cli();

    // A lightweight tag is ignored without `--tags`, so there is nothing to
    // describe → RepoStateInvalid (128).
    let tag_output = run_libra_command(&["tag", "v-light"], repo.path());
    assert_cli_success(&tag_output, "failed to create lightweight tag");

    let output = run_libra_command(&["describe"], repo.path());
    assert_eq!(output.status.code(), Some(128));
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
}

#[test]
fn test_describe_no_tags_no_always_errors() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["describe"], repo.path());
    assert_eq!(output.status.code(), Some(128));
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
}

#[test]
fn test_describe_exact_match_with_distance_errors() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], repo.path());
    assert_cli_success(&tag_output, "failed to create tag");

    // Advance one commit so HEAD is distance 1 from the tag.
    std::fs::write(repo.path().join("tracked.txt"), "tracked\nnext\n")
        .expect("failed to update tracked file");
    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add_output, "failed to stage updated tracked file");
    let commit_output = run_libra_command(&["commit", "-m", "next", "--no-verify"], repo.path());
    assert_cli_success(&commit_output, "failed to create second commit");

    let output = run_libra_command(&["describe", "--exact-match"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(128),
        "exact-match at distance>0 should fail with 128: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
}

#[test]
fn test_describe_exact_match_at_tag_succeeds() {
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], repo.path());
    assert_cli_success(&tag_output, "failed to create tag");

    let output = run_libra_command(&["describe", "--exact-match", "--json"], repo.path());
    assert_cli_success(&output, "exact-match at the tag should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0");
    assert_eq!(json["data"]["distance"], 0);
}

#[test]
fn test_describe_first_parent_json_succeeds() {
    // First-parent traversal on a linear history reduces to the normal walk.
    let repo = create_committed_repo_via_cli();

    let tag_output = run_libra_command(&["tag", "-m", "Release v1.0", "v1.0"], repo.path());
    assert_cli_success(&tag_output, "failed to create tag");

    std::fs::write(repo.path().join("tracked.txt"), "tracked\nnext\n")
        .expect("failed to update tracked file");
    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add_output, "failed to stage updated tracked file");
    let commit_output = run_libra_command(&["commit", "-m", "next", "--no-verify"], repo.path());
    assert_cli_success(&commit_output, "failed to create second commit");

    let output = run_libra_command(&["describe", "--first-parent", "--json"], repo.path());
    assert_cli_success(&output, "describe --first-parent --json should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["tag"], "v1.0");
    assert_eq!(json["data"]["distance"], 1);
}

#[test]
fn test_describe_negative_abbrev_rejected_by_clap() {
    let repo = create_committed_repo_via_cli();

    // `--abbrev=-1` cannot parse as `usize`; clap rejects it. Libra maps clap
    // parse errors to 129 (classify_parse_error), not clap's native 2.
    let output = run_libra_command(&["describe", "--abbrev=-1"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "negative --abbrev should be rejected at parse time: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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

/// A clean worktree adds no `--dirty` suffix.
#[test]
fn test_describe_dirty_clean_worktree_no_suffix() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    let output = run_libra_command(&["describe", "--dirty", "--json"], repo.path());
    assert_cli_success(&output, "describe --dirty on clean worktree should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0");
    assert_eq!(json["data"]["dirty"], false);
    assert!(json["data"]["dirty_suffix"].is_null());
}

/// An unstaged modification to a tracked file marks the worktree dirty.
#[test]
fn test_describe_dirty_unstaged_tracked_modification() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    std::fs::write(repo.path().join("tracked.txt"), "tracked\nmodified\n")
        .expect("failed to modify tracked file");

    let output = run_libra_command(&["describe", "--dirty", "--json"], repo.path());
    assert_cli_success(
        &output,
        "describe --dirty with unstaged change should succeed",
    );
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0-dirty");
    assert_eq!(json["data"]["dirty"], true);
    assert_eq!(json["data"]["dirty_suffix"], "-dirty");
}

/// A staged modification marks the worktree dirty.
#[test]
fn test_describe_dirty_staged_modification() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    std::fs::write(repo.path().join("tracked.txt"), "tracked\nstaged\n")
        .expect("failed to modify tracked file");
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], repo.path()),
        "stage tracked.txt",
    );

    let output = run_libra_command(&["describe", "--dirty", "--json"], repo.path());
    assert_cli_success(
        &output,
        "describe --dirty with staged change should succeed",
    );
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0-dirty");
    assert_eq!(json["data"]["dirty"], true);
}

/// Only untracked files present → the worktree is NOT dirty (no suffix).
#[test]
fn test_describe_dirty_only_untracked_is_clean() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    std::fs::write(repo.path().join("untracked.txt"), "brand new\n")
        .expect("failed to create untracked file");

    let output = run_libra_command(&["describe", "--dirty", "--json"], repo.path());
    assert_cli_success(
        &output,
        "describe --dirty with only untracked should succeed",
    );
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0");
    assert_eq!(json["data"]["dirty"], false);
    assert!(json["data"]["dirty_suffix"].is_null());
}

/// `--dirty=<mark>` uses the custom suffix.
#[test]
fn test_describe_dirty_custom_suffix() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    std::fs::write(repo.path().join("tracked.txt"), "tracked\nmodified\n")
        .expect("failed to modify tracked file");

    let output = run_libra_command(&["describe", "--dirty=-modified", "--json"], repo.path());
    assert_cli_success(&output, "describe --dirty=-modified should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0-modified");
    assert_eq!(json["data"]["dirty"], true);
    assert_eq!(json["data"]["dirty_suffix"], "-modified");
}

/// `--dirty` is a read-only probe: it must not touch tracked-file mtimes.
#[test]
fn test_describe_dirty_readonly_does_not_touch_mtime() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    let tracked = repo.path().join("tracked.txt");
    std::fs::write(&tracked, "tracked\nmodified\n").expect("failed to modify tracked file");
    let mtime_before = std::fs::metadata(&tracked)
        .expect("metadata before")
        .modified()
        .expect("mtime before");

    let output = run_libra_command(&["describe", "--dirty"], repo.path());
    assert_cli_success(&output, "describe --dirty should succeed");

    let mtime_after = std::fs::metadata(&tracked)
        .expect("metadata after")
        .modified()
        .expect("mtime after");
    assert_eq!(
        mtime_before, mtime_after,
        "describe --dirty must not rewrite tracked-file mtime"
    );
}

/// Advance `tracked.txt` by one commit so the history grows for `--contains`
/// distance assertions.
fn advance_commit(repo: &tempfile::TempDir, content: &str) {
    std::fs::write(repo.path().join("tracked.txt"), content).expect("failed to write tracked file");
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], repo.path()),
        "failed to stage advance commit",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "advance", "--no-verify"], repo.path()),
        "failed to create advance commit",
    );
}

/// `--contains` reports `<tag>~<offset>` for a tag whose history includes the target.
#[test]
fn test_describe_contains_returns_tag_offset() {
    let repo = create_committed_repo_via_cli(); // C1
    advance_commit(&repo, "two\n"); // C2
    advance_commit(&repo, "three\n"); // C3 (HEAD)
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    // v1.0 (at C3) contains HEAD~2 (= C1) at topological distance 2.
    let output = run_libra_command(&["describe", "--contains", "HEAD~2", "--json"], repo.path());
    assert_cli_success(&output, "describe --contains HEAD~2 should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0~2");
    assert_eq!(json["data"]["contains_offset"], 2);
    assert_eq!(json["data"]["ref_kind"], "tag");
    assert_eq!(json["data"]["ref_name"], "v1.0");
    assert_eq!(json["data"]["tag"], "v1.0");
}

/// `--contains` output uses the `tag~N` form and never the default `-g<hash>` form.
#[test]
fn test_describe_contains_format_excludes_ghash() {
    let repo = create_committed_repo_via_cli();
    advance_commit(&repo, "two\n");
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    let output = run_libra_command(&["describe", "--contains", "HEAD~1"], repo.path());
    assert_cli_success(&output, "describe --contains should succeed");
    let result = String::from_utf8_lossy(&output.stdout);
    assert_eq!(result.trim(), "v1.0~1");
    assert!(
        !result.contains("-g"),
        "contains output must not include -g<hash>: {result:?}"
    );
}

/// `--contains` matches lightweight tags by default (Git's `describe --contains`).
#[test]
fn test_describe_contains_includes_lightweight_tag_by_default() {
    let repo = create_committed_repo_via_cli();
    advance_commit(&repo, "two\n");
    assert_cli_success(
        &run_libra_command(&["tag", "lw1.0"], repo.path()),
        "lightweight tag",
    );

    let output = run_libra_command(&["describe", "--contains", "HEAD~1", "--json"], repo.path());
    assert_cli_success(&output, "describe --contains lightweight should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["ref_name"], "lw1.0");
    assert_eq!(json["data"]["contains_offset"], 1);
}

/// `--contains --exact-match` requires the containing ref to point directly at the
/// target (offset 0); a positive offset is a run-time failure (128), matching the
/// non-`--contains` exact-match path and Git.
#[test]
fn test_describe_contains_exact_match_nonzero_offset_fails() {
    let repo = create_committed_repo_via_cli(); // C1
    advance_commit(&repo, "two\n"); // C2 (HEAD)
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    // v1.0 (at C2) contains HEAD~1 (= C1) at distance 1, so exact-match must reject it.
    let output = run_libra_command(
        &["describe", "--contains", "--exact-match", "HEAD~1"],
        repo.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(128),
        "--contains --exact-match at offset>0 should fail with 128: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
}

/// `--contains --exact-match` succeeds when a ref points directly at the target,
/// emitting the bare ref name with no `~N` suffix.
#[test]
fn test_describe_contains_exact_match_offset_zero_succeeds() {
    let repo = create_committed_repo_via_cli(); // C1
    advance_commit(&repo, "two\n"); // C2 (HEAD)
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    // v1.0 points directly at HEAD (offset 0), so exact-match succeeds.
    let output = run_libra_command(
        &["describe", "--contains", "--exact-match", "HEAD", "--json"],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "describe --contains --exact-match at the ref should succeed",
    );
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["result"], "v1.0");
    assert_eq!(json["data"]["contains_offset"], 0);
    assert_eq!(json["data"]["exact_match"], true);
}

/// `--candidates=N` (N>=1) still returns the topologically nearest tag.
#[test]
fn test_describe_candidates_limit_picks_nearest() {
    let repo = create_committed_repo_via_cli(); // C1
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "old"], repo.path()),
        "tag old",
    );
    advance_commit(&repo, "two\n"); // C2
    advance_commit(&repo, "three\n"); // C3
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "new"], repo.path()),
        "tag new",
    );
    advance_commit(&repo, "four\n"); // C4 (HEAD)

    let output = run_libra_command(&["describe", "--candidates", "5", "--json"], repo.path());
    assert_cli_success(&output, "describe --candidates=5 should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["tag"], "new");
    assert_eq!(json["data"]["distance"], 1);
}

/// `--candidates=0` is rejected as a usage error (129).
#[test]
fn test_describe_candidates_zero_rejected() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );

    let output = run_libra_command(&["describe", "--candidates", "0"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(129),
        "--candidates=0 should be rejected: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
}

/// `describe.maxCandidates` config is consulted (and does not panic); the nearest
/// tag is still returned.
#[test]
fn test_describe_max_candidates_config_override() {
    let repo = create_committed_repo_via_cli();
    assert_cli_success(
        &run_libra_command(
            &["config", "set", "describe.maxCandidates", "3"],
            repo.path(),
        ),
        "set describe.maxCandidates",
    );
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );
    advance_commit(&repo, "two\n");

    let output = run_libra_command(&["describe", "--json"], repo.path());
    assert_cli_success(&output, "describe with config maxCandidates should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["tag"], "v1.0");
    assert_eq!(json["data"]["distance"], 1);
}

/// `--contains --all` searches branch heads; a branch whose history includes the
/// target is reported as `heads/<name>` with `ref_kind="head"`.
#[test]
fn test_describe_contains_with_all_finds_branch() {
    let repo = create_committed_repo_via_cli(); // C1
    advance_commit(&repo, "two\n"); // C2 (HEAD on main)
    // No tags. Branch "aaa" at HEAD sorts before "main" for a deterministic result.
    assert_cli_success(
        &run_libra_command(&["branch", "aaa"], repo.path()),
        "branch aaa",
    );

    let output = run_libra_command(
        &["describe", "--contains", "--all", "HEAD~1", "--json"],
        repo.path(),
    );
    assert_cli_success(&output, "describe --contains --all should succeed");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["ref_kind"], "head");
    assert_eq!(json["data"]["ref_name"], "heads/aaa");
    assert_eq!(json["data"]["contains_offset"], 1);
    assert!(json["data"]["tag"].is_null());
}

/// `--contains` with no containing ref and no `--always` fails with 128.
#[test]
fn test_describe_contains_no_match_errors() {
    let repo = create_committed_repo_via_cli(); // C1, no tags

    let output = run_libra_command(&["describe", "--contains", "HEAD"], repo.path());
    assert_eq!(
        output.status.code(),
        Some(128),
        "--contains with no tag should fail with 128: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-003");
}

/// At equal distance, a tag wins over a branch (deterministic kind priority).
#[test]
fn test_describe_contains_tiebreak_deterministic() {
    let repo = create_committed_repo_via_cli(); // C1
    advance_commit(&repo, "two\n"); // C2 (HEAD)
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "rel", "v1.0"], repo.path()),
        "tag v1.0",
    );
    assert_cli_success(
        &run_libra_command(&["branch", "zzz"], repo.path()),
        "branch zzz",
    );

    // Tag and branches all contain HEAD~1 at distance 1; tag kind wins.
    let output = run_libra_command(
        &["describe", "--contains", "--all", "HEAD~1", "--json"],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "describe --contains --all tie-break should succeed",
    );
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["ref_kind"], "tag");
    assert_eq!(json["data"]["ref_name"], "v1.0");
    assert_eq!(json["data"]["tag"], "v1.0");
}
