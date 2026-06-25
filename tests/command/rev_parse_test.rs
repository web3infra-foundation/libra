//! Integration tests for `rev-parse` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use super::*;

/// Assert that `rev-parse --show-toplevel` prints the expected repository root.
///
/// Test coverage: the three direct worktree/storage-dir tests below pass temp
/// paths through both `/var` and canonical `/private/var` spellings on macOS,
/// while the symlink case verifies that an entered storage symlink still maps to
/// the canonical worktree root.
fn assert_show_toplevel_stdout_eq(output: &std::process::Output, expected: &std::path::Path) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let actual = std::path::PathBuf::from(stdout.trim());
    assert_eq!(
        actual
            .canonicalize()
            .expect("failed to canonicalize rev-parse output path"),
        expected
            .canonicalize()
            .expect("failed to canonicalize expected repo path")
    );
}

#[test]
fn test_rev_parse_head_resolves_commit() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-parse HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value = stdout.trim();
    assert_eq!(value.len(), 40, "expected full hash, got: {value}");
    assert!(value.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_rev_parse_short_head_returns_non_ambiguous_hash() {
    let repo = create_committed_repo_via_cli();

    let full = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&full, "rev-parse HEAD (full)");
    let full_hash = String::from_utf8_lossy(&full.stdout).trim().to_string();

    let output = run_libra_command(&["rev-parse", "--short", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-parse --short HEAD");

    let short_hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        short_hash.len() >= 7,
        "expected abbreviated hash, got: {short_hash}"
    );
    assert!(short_hash.len() <= full_hash.len());
    assert!(full_hash.starts_with(&short_hash));

    let resolved = run_libra_command(&["rev-parse", short_hash.as_str()], repo.path());
    assert_cli_success(&resolved, "rev-parse <short-hash>");
    assert_eq!(String::from_utf8_lossy(&resolved.stdout).trim(), full_hash);
}

#[test]
fn test_rev_parse_abbrev_ref_head_returns_branch_name() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["rev-parse", "--abbrev-ref", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-parse --abbrev-ref HEAD");

    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_remote_tracking_ref_returns_short_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch(
        "refs/remotes/origin/main",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .expect("failed to create remote-tracking ref");

    let output = run_libra_command(&["rev-parse", "--abbrev-ref", "origin/main"], repo.path());
    assert_cli_success(&output, "rev-parse --abbrev-ref origin/main");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "origin/main"
    );
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_multi_segment_remote_tracking_ref_returns_short_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch(
        "refs/remotes/upstream/origin/main",
        &head.to_string(),
        Some("upstream/origin"),
    )
    .await
    .expect("failed to create multi-segment remote-tracking ref");

    let output = run_libra_command(
        &["rev-parse", "--abbrev-ref", "upstream/origin/main"],
        repo.path(),
    );
    assert_cli_success(&output, "rev-parse --abbrev-ref upstream/origin/main");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "upstream/origin/main"
    );
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_lowercase_head_resolves_branch_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch("head", &head.to_string(), None)
        .await
        .expect("failed to create lowercase head branch");

    let output = run_libra_command(&["rev-parse", "--abbrev-ref", "head"], repo.path());
    assert_cli_success(&output, "rev-parse --abbrev-ref head");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "head");
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_refs_heads_returns_short_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let output = run_libra_command(
        &["rev-parse", "--abbrev-ref", "refs/heads/main"],
        repo.path(),
    );
    assert_cli_success(&output, "rev-parse --abbrev-ref refs/heads/main");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_refs_remotes_returns_short_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch(
        "refs/remotes/origin/main",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .expect("failed to create remote-tracking ref");

    let output = run_libra_command(
        &["rev-parse", "--abbrev-ref", "refs/remotes/origin/main"],
        repo.path(),
    );
    assert_cli_success(&output, "rev-parse --abbrev-ref refs/remotes/origin/main");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "origin/main"
    );
}

#[tokio::test]
#[serial]
async fn test_rev_parse_abbrev_ref_prefers_exact_local_refs_remotes_name() {
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch("refs/remotes/origin/main", &head.to_string(), None)
        .await
        .expect("failed to create local branch named like remote-tracking ref");

    let output = run_libra_command(
        &["rev-parse", "--abbrev-ref", "refs/remotes/origin/main"],
        repo.path(),
    );
    assert_cli_success(
        &output,
        "rev-parse --abbrev-ref exact local refs/remotes/origin/main",
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "refs/remotes/origin/main"
    );
}

#[test]
fn test_rev_parse_show_toplevel_repo_named_storage_dir_returns_repo_root() {
    let parent = tempdir().expect("failed to create parent directory");
    let repo_path = parent.path().join(libra::utils::util::ROOT_DIR);
    init_repo_via_cli(&repo_path);

    let output = run_libra_command(&["rev-parse", "--show-toplevel"], &repo_path);
    assert_cli_success(
        &output,
        "rev-parse --show-toplevel from repo root named .libra",
    );

    // Scenario: a repository whose worktree itself is named `.libra` must not
    // be mistaken for the internal storage directory.
    assert_show_toplevel_stdout_eq(&output, &repo_path);
}

#[test]
fn test_rev_parse_show_toplevel_returns_repo_root() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["rev-parse", "--show-toplevel"], repo.path());
    assert_cli_success(&output, "rev-parse --show-toplevel from repo root");

    // Scenario: the normal worktree-root invocation returns the root path,
    // allowing platform-specific tempdir symlinks to differ only in spelling.
    assert_show_toplevel_stdout_eq(&output, repo.path());
}

#[test]
fn test_rev_parse_show_toplevel_from_storage_dir_returns_repo_root() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());
    let storage = repo.path().join(libra::utils::util::ROOT_DIR);

    let output = run_libra_command(&["rev-parse", "--show-toplevel"], &storage);
    assert_cli_success(&output, "rev-parse --show-toplevel from .libra");

    // Scenario: entering the physical `.libra` storage directory reports the
    // enclosing worktree root rather than the storage path itself.
    assert_show_toplevel_stdout_eq(&output, repo.path());
}

#[cfg(unix)]
#[test]
fn test_rev_parse_show_toplevel_from_symlinked_storage_dir_returns_repo_root() {
    use std::os::unix::fs::symlink;

    let temp_root = tempdir().expect("failed to create temp root");
    let repo = temp_root.path().join("repo");
    init_repo_via_cli(&repo);

    let storage = repo.join(libra::utils::util::ROOT_DIR);
    let storage_link = temp_root.path().join("storage-link");
    symlink(&storage, &storage_link).expect("failed to create storage symlink");

    let output = run_libra_command(&["rev-parse", "--show-toplevel"], &storage_link);
    assert_cli_success(&output, "rev-parse --show-toplevel from symlinked .libra");

    // Scenario: a symlink pointing at `.libra` is resolved back to the real
    // worktree root, matching Git's behavior for storage-directory traversal.
    assert_show_toplevel_stdout_eq(&output, &repo);
}

#[test]
fn test_rev_parse_show_toplevel_in_bare_repo_returns_work_tree_error() {
    let repo = tempdir().expect("failed to create repository root");
    let bare_repo = repo.path().join("repo.git");

    let init_output = run_libra_command(
        &["init", "--bare", "repo.git", "--vault", "false"],
        repo.path(),
    );
    assert_cli_success(&init_output, "init bare repo for rev-parse test");

    let output = run_libra_command(&["rev-parse", "--show-toplevel"], &bare_repo);
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert!(stderr.contains("this operation must be run in a work tree"));
    assert_eq!(report.error_code, "LBR-REPO-003");
}

#[test]
fn test_rev_parse_show_toplevel_rejects_spec() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["rev-parse", "--show-toplevel", "HEAD"], repo.path());

    assert!(!output.status.success(), "command unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("unexpected argument"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_rev_parse_invalid_target_returns_cli_error_code() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["rev-parse", "badref"], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("not a valid object name: 'badref'"));
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[test]
fn test_rev_parse_verify_resolves_single_object() {
    let repo = create_committed_repo_via_cli();
    let verify = run_libra_command(&["rev-parse", "--verify", "HEAD"], repo.path());
    assert_cli_success(&verify, "rev-parse --verify HEAD");
    let plain = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_eq!(
        String::from_utf8_lossy(&verify.stdout).trim(),
        String::from_utf8_lossy(&plain.stdout).trim(),
        "--verify should print the same hash as a plain resolve"
    );
}

#[test]
fn test_rev_parse_verify_unresolvable_exits_128() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(
        &["rev-parse", "--verify", "definitely-not-a-ref"],
        repo.path(),
    );
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Needed a single revision"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn test_rev_parse_verify_quiet_unresolvable_exits_1_silently() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["--quiet", "rev-parse", "--verify", "nope"], repo.path());
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "quiet --verify must print nothing"
    );
    assert!(
        output.stderr.is_empty(),
        "quiet --verify must not print a diagnostic, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_rev_parse_default_used_when_no_spec() {
    let repo = create_committed_repo_via_cli();
    let with_default = run_libra_command(&["rev-parse", "--default", "HEAD"], repo.path());
    assert_cli_success(&with_default, "rev-parse --default HEAD");
    let head = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_eq!(
        String::from_utf8_lossy(&with_default.stdout).trim(),
        String::from_utf8_lossy(&head.stdout).trim(),
        "--default should resolve to HEAD when no SPEC is given"
    );
}

#[test]
fn test_rev_parse_is_inside_work_tree_true() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["rev-parse", "--is-inside-work-tree"], repo.path());
    assert_cli_success(&output, "rev-parse --is-inside-work-tree");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "true");
}

#[test]
fn test_rev_parse_git_dir_points_at_libra_dir() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["rev-parse", "--git-dir"], repo.path());
    assert_cli_success(&output, "rev-parse --git-dir");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().contains(".libra"),
        "git-dir should point at the .libra dir, got {stdout}"
    );
}

#[test]
fn test_rev_parse_rejects_tag_object_that_points_to_tree() {
    let repo = create_committed_repo_via_cli();
    let tag_id = create_non_commit_tag_object(repo.path());

    let output = run_libra_command(&["rev-parse", tag_id.as_str()], repo.path());
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(stderr.contains("not a valid object name"));
    assert!(stderr.contains("tag points to tree"));
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[test]
fn test_rev_parse_json_returns_envelope() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--json", "rev-parse", "HEAD"], repo.path());
    assert_cli_success(&output, "json rev-parse HEAD");

    let json = parse_json_stdout(&output);
    assert_eq!(json["ok"], true);
    assert_eq!(json["command"], "rev-parse");
    assert_eq!(json["data"]["mode"], "resolve");
    assert_eq!(json["data"]["input"], "HEAD");
    assert!(json["data"]["value"].as_str().is_some());
}

#[test]
fn test_rev_parse_machine_returns_single_json_line() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--machine", "rev-parse", "HEAD"], repo.path());
    assert_cli_success(&output, "machine rev-parse HEAD");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line, got: {stdout}"
    );

    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("expected JSON");
    assert_eq!(parsed["command"], "rev-parse");
    assert_eq!(parsed["data"]["mode"], "resolve");
}

/// `libra rev-parse --help` surfaces the EXAMPLES banner so users see
/// the four mutually-exclusive modes (resolve / --short / --abbrev-ref
/// / --show-toplevel) plus the JSON variant for agents. Cross-cutting
/// `--help` EXAMPLES rollout per `docs/development/commands/_general.md` item B.
#[test]
fn test_rev_parse_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for rev-parse --help");
    let output = run_libra_command(&["rev-parse", "--help"], repo.path());
    assert!(
        output.status.success(),
        "rev-parse --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "rev-parse --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra rev-parse HEAD",
        "libra rev-parse main~3",
        "libra rev-parse --short HEAD",
        "libra rev-parse --abbrev-ref HEAD",
        "libra rev-parse --show-toplevel",
        "libra rev-parse --json HEAD",
    ] {
        assert!(
            stdout.contains(invocation),
            "rev-parse --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}

#[test]
fn test_rev_parse_show_prefix_at_root() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["rev-parse", "--show-prefix"], repo.path());
    assert_cli_success(&output, "rev-parse --show-prefix");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "",
        "show-prefix at repo root should be empty"
    );
}

#[test]
fn test_rev_parse_show_prefix_in_subdir() {
    let repo = create_committed_repo_via_cli();
    let subdir = repo.path().join("src");
    std::fs::create_dir_all(&subdir).expect("create subdir");
    let output = run_libra_command(&["rev-parse", "--show-prefix"], &subdir);
    assert_cli_success(&output, "rev-parse --show-prefix in subdir");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "src/",
        "show-prefix in subdir should be 'src/'"
    );
}

#[test]
fn test_rev_parse_show_cdup_at_root() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["rev-parse", "--show-cdup"], repo.path());
    assert_cli_success(&output, "rev-parse --show-cdup");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "", "show-cdup at repo root should be empty");
}

#[test]
fn test_rev_parse_show_cdup_in_subdir() {
    let repo = create_committed_repo_via_cli();
    let subdir = repo.path().join("a").join("b");
    std::fs::create_dir_all(&subdir).expect("create subdir");
    let output = run_libra_command(&["rev-parse", "--show-cdup"], &subdir);
    assert_cli_success(&output, "rev-parse --show-cdup in subdir");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "../../",
        "show-cdup in a/b should be '../../'"
    );
}

#[test]
fn test_rev_parse_short_with_length() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["rev-parse", "--short=8", "HEAD"], repo.path());
    assert_cli_success(&output, "rev-parse --short=8 HEAD");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim().len(), 8, "short=8 should produce 8-char hash");
}

#[test]
fn test_rev_parse_is_inside_git_dir() {
    let repo = create_committed_repo_via_cli();

    // From the worktree root: not inside the .libra directory.
    let out = run_libra_command(&["rev-parse", "--is-inside-git-dir"], repo.path());
    assert_cli_success(&out, "rev-parse --is-inside-git-dir from worktree");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "false");

    // From inside the .libra directory: true (Libra's GIT_DIR equivalent).
    let libra_dir = repo.path().join(".libra");
    let out = run_libra_command(&["rev-parse", "--is-inside-git-dir"], &libra_dir);
    assert_cli_success(&out, "rev-parse --is-inside-git-dir from .libra");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "true");
}

#[test]
fn test_rev_parse_absolute_git_dir_is_canonical_absolute() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    let out = run_libra_command(&["rev-parse", "--absolute-git-dir"], p);
    assert_cli_success(&out, "rev-parse --absolute-git-dir");
    let abs = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        std::path::Path::new(&abs).is_absolute(),
        "absolute path: {abs:?}"
    );
    assert!(abs.ends_with(".libra"), "points at .libra: {abs:?}");

    // In Libra `--git-dir` is already absolute, so the two coincide.
    let gd = run_libra_command(&["rev-parse", "--git-dir"], p);
    assert_cli_success(&gd, "rev-parse --git-dir");
    assert_eq!(
        abs,
        String::from_utf8_lossy(&gd.stdout).trim(),
        "--absolute-git-dir matches --git-dir"
    );

    // Mutually exclusive with --git-dir.
    let both = run_libra_command(&["rev-parse", "--absolute-git-dir", "--git-dir"], p);
    assert!(!both.status.success(), "conflicting flags rejected");
}

#[test]
fn test_rev_parse_sq_single_quotes_resolved_object() {
    let repo = create_committed_repo_via_cli();

    let plain = run_libra_command(&["rev-parse", "HEAD"], repo.path());
    assert_cli_success(&plain, "rev-parse HEAD");
    let hash = String::from_utf8_lossy(&plain.stdout).trim().to_string();

    // `--sq` single-quotes the resolved object name.
    let sq = run_libra_command(&["rev-parse", "--sq", "HEAD"], repo.path());
    assert_cli_success(&sq, "rev-parse --sq HEAD");
    let quoted = String::from_utf8_lossy(&sq.stdout).trim().to_string();
    assert_eq!(quoted, format!("'{hash}'"), "expected single-quoted hash");

    // `--sq` does not quote the repository-query modes (matches Git).
    let toplevel = run_libra_command(&["rev-parse", "--sq", "--show-toplevel"], repo.path());
    assert_cli_success(&toplevel, "rev-parse --sq --show-toplevel");
    let path = String::from_utf8_lossy(&toplevel.stdout).trim().to_string();
    assert!(
        !path.starts_with('\'') && !path.ends_with('\''),
        "query modes must not be shell-quoted: {path:?}"
    );
}

#[test]
fn test_rev_parse_symbolic_full_name() {
    // `--symbolic-full-name` resolves a spec to its full ref name (refs/heads,
    // refs/tags, or HEAD's branch), prints nothing for a valid non-ref object, and
    // fails with exit 128 for an unresolvable name — matching git rev-parse.
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    let head_branch = String::from_utf8_lossy(
        &run_libra_command(&["rev-parse", "--abbrev-ref", "HEAD"], p).stdout,
    )
    .trim()
    .to_string();
    let full = format!("refs/heads/{head_branch}");

    let out = |args: &[&str]| {
        let o = run_libra_command(args, p);
        (
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
            o.status.code(),
        )
    };

    // HEAD -> its branch's full name.
    let (v, code) = out(&["rev-parse", "--symbolic-full-name", "HEAD"]);
    assert_eq!(code, Some(0));
    assert_eq!(v, full, "HEAD resolves to its full branch ref");

    // A bare branch name -> refs/heads/<name>.
    let (v, _) = out(&["rev-parse", "--symbolic-full-name", &head_branch]);
    assert_eq!(v, full);

    // refs/heads/<name> is returned verbatim.
    let (v, _) = out(&["rev-parse", "--symbolic-full-name", &full]);
    assert_eq!(v, full);

    // A tag -> refs/tags/<name>.
    assert_cli_success(&run_libra_command(&["tag", "v9.9"], p), "create tag v9.9");
    let (v, code) = out(&["rev-parse", "--symbolic-full-name", "v9.9"]);
    assert_eq!(code, Some(0));
    assert_eq!(v, "refs/tags/v9.9");

    // A valid object that is not a ref (the commit SHA) prints nothing, exit 0 —
    // byte-exact empty stdout (not even a trailing newline).
    let sha = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD"], p).stdout)
        .trim()
        .to_string();
    let commit_out = run_libra_command(&["rev-parse", "--symbolic-full-name", &sha], p);
    assert_eq!(commit_out.status.code(), Some(0));
    assert!(
        commit_out.stdout.is_empty(),
        "a non-ref commit object emits no bytes: {:?}",
        String::from_utf8_lossy(&commit_out.stdout)
    );

    // A raw tree object id (not a ref) also prints nothing, exit 0.
    let tree_sha =
        String::from_utf8_lossy(&run_libra_command(&["cat-file", "-p", "HEAD"], p).stdout)
            .lines()
            .find_map(|l| l.strip_prefix("tree ").map(|s| s.trim().to_string()))
            .expect("HEAD commit lists a tree");
    let tree_out = run_libra_command(&["rev-parse", "--symbolic-full-name", &tree_sha], p);
    assert_eq!(tree_out.status.code(), Some(0));
    assert!(
        tree_out.stdout.is_empty(),
        "a raw tree object id emits no bytes: {:?}",
        String::from_utf8_lossy(&tree_out.stdout)
    );

    // An unresolvable spec fails with exit 128 (git's "ambiguous argument").
    let (_, code) = out(&["rev-parse", "--symbolic-full-name", "definitely-not-a-ref"]);
    assert_eq!(code, Some(128), "unresolvable spec exits 128");

    // A malformed revision expression the strict parser rejects is unresolvable
    // (exit 128) — it must NOT be permissively re-resolved to empty/exit 0.
    let (_, code) = out(&["rev-parse", "--symbolic-full-name", "HEAD^garbage"]);
    assert_eq!(code, Some(128), "malformed peel/navigation spec exits 128");

    // Detached HEAD -> "HEAD".
    assert_cli_success(
        &run_libra_command(&["checkout", &sha], p),
        "detach HEAD at the commit",
    );
    let (v, code) = out(&["rev-parse", "--symbolic-full-name", "HEAD"]);
    assert_eq!(code, Some(0));
    assert_eq!(v, "HEAD", "detached HEAD resolves to literal HEAD");
}

#[tokio::test]
#[serial]
async fn test_rev_parse_symbolic_full_name_remote_tracking_ref() {
    // A remote-tracking spec resolves to its full `refs/remotes/<remote>/<branch>`.
    let repo = tempdir().expect("failed to create repository root");
    test::setup_with_new_libra_in(repo.path()).await;
    let _guard = ChangeDirGuard::new(repo.path());

    commit::execute(CommitArgs {
        message: Some("base".to_string()),
        allow_empty: true,
        disable_pre: true,
        no_verify: false,
        ..Default::default()
    })
    .await;

    let head = Head::current_commit().await.expect("expected HEAD commit");
    Branch::update_branch(
        "refs/remotes/origin/main",
        &head.to_string(),
        Some("origin"),
    )
    .await
    .expect("failed to create remote-tracking ref");

    let output = run_libra_command(
        &["rev-parse", "--symbolic-full-name", "origin/main"],
        repo.path(),
    );
    assert_cli_success(&output, "rev-parse --symbolic-full-name origin/main");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "refs/remotes/origin/main"
    );
}
