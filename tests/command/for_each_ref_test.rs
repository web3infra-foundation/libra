//! Integration tests for `libra for-each-ref`.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{fs, io::Write};

use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Create a repo, add a file and commit with the given message.
async fn setup_repo_with_commit(temp: &tempfile::TempDir) {
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = ChangeDirGuard::new(temp.path());

    let mut f = fs::File::create("a.txt").unwrap();
    writeln!(f, "hello").unwrap();

    add::execute(AddArgs {
        pathspec: vec!["a.txt".into()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        pathspec_from_file: None,
        pathspec_file_nul: false,
    })
    .await;

    commit::execute(CommitArgs {
        message: Some("initial".into()),
        ..Default::default()
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_lists_heads() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;

    let output = run_libra_command(&["for-each-ref", "--heads"], temp.path());
    assert_cli_success(&output, "for-each-ref --heads should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("refs/heads/main"),
        "expected refs/heads/main in output, got: {stdout}"
    );
}

#[test]
fn test_for_each_ref_contains_filter() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    std::fs::write(p.join("f1.txt"), "1\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f1.txt"], p), "add f1");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c1", "--no-verify"], p),
        "commit c1",
    );
    // `old` points at c1 and never advances.
    assert_cli_success(&run_libra_command(&["branch", "old"], p), "branch old");

    std::fs::write(p.join("f2.txt"), "2\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f2.txt"], p), "add f2");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c2", "--no-verify"], p),
        "commit c2",
    );

    let head = run_libra_command(&["rev-parse", "HEAD"], p);
    let c2 = String::from_utf8_lossy(&head.stdout).trim().to_string();

    // Only main (at c2) contains c2; `old` (at c1) does not.
    let out = run_libra_command(&["for-each-ref", "--heads", "--contains", &c2], p);
    assert_cli_success(&out, "for-each-ref --contains c2");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("refs/heads/main"),
        "main should contain c2: {stdout}"
    );
    assert!(
        !stdout.contains("refs/heads/old"),
        "old should NOT contain c2: {stdout}"
    );
}

#[test]
fn test_for_each_ref_merged_filter() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    std::fs::write(p.join("f1.txt"), "1\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f1.txt"], p), "add f1");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c1", "--no-verify"], p),
        "commit c1",
    );
    let c1 = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD"], p).stdout)
        .trim()
        .to_string();
    // `old` points at c1 and never advances.
    assert_cli_success(&run_libra_command(&["branch", "old"], p), "branch old");
    // An annotated tag at c1: its entry peels to the commit for reachability.
    assert_cli_success(
        &run_libra_command(&["tag", "-m", "anno", "atag"], p),
        "annotated tag atag at c1",
    );

    std::fs::write(p.join("f2.txt"), "2\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "f2.txt"], p), "add f2");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "c2", "--no-verify"], p),
        "commit c2",
    );
    let c2 = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD"], p).stdout)
        .trim()
        .to_string();
    // A lightweight tag at c2, used to exercise fully-qualified ref targets.
    assert_cli_success(
        &run_libra_command(&["tag", "lw"], p),
        "lightweight tag lw at c2",
    );

    // --merged=c2: both main (c2) and old (c1) are reachable from c2.
    let out = run_libra_command(&["for-each-ref", "--heads", "--merged", &c2], p);
    assert_cli_success(&out, "for-each-ref --merged c2");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("refs/heads/main") && stdout.contains("refs/heads/old"),
        "both main and old should be merged into c2: {stdout}"
    );

    // --no-merged=c1: main (c2) is not reachable from c1; old (c1) is.
    let out = run_libra_command(&["for-each-ref", "--heads", "--no-merged", &c1], p);
    assert_cli_success(&out, "for-each-ref --no-merged c1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("refs/heads/main"),
        "main should NOT be merged into c1: {stdout}"
    );
    assert!(
        !stdout.contains("refs/heads/old"),
        "old should be merged into c1 and thus excluded: {stdout}"
    );

    // Annotated tag peeling: atag (at c1) is reachable from c2, so --merged=c2
    // includes it; --no-merged=c1 excludes it (c1 is merged into c1).
    let out = run_libra_command(&["for-each-ref", "--tags", "--merged", &c2], p);
    assert_cli_success(&out, "for-each-ref --tags --merged c2");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("refs/tags/atag"),
        "annotated tag atag should be merged into c2: {stdout}"
    );

    let out = run_libra_command(&["for-each-ref", "--tags", "--no-merged", &c1], p);
    assert_cli_success(&out, "for-each-ref --tags --no-merged c1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("refs/tags/atag"),
        "annotated tag atag (c1) should be merged into c1 and excluded: {stdout}"
    );

    // The merge TARGET may itself be an annotated tag name; it peels to its
    // commit (atag -> c1), so only refs reachable from c1 are "merged".
    let out = run_libra_command(&["for-each-ref", "--heads", "--merged", "atag"], p);
    assert_cli_success(&out, "for-each-ref --heads --merged atag");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("refs/heads/old"),
        "old (c1) should be merged into atag (c1): {stdout}"
    );
    assert!(
        !stdout.contains("refs/heads/main"),
        "main (c2) should NOT be merged into atag (c1): {stdout}"
    );

    // Fully-qualified ref targets must resolve too (no regression vs the legacy
    // resolver): --contains refs/tags/lw (lw -> c2) keeps only refs containing c2.
    let out = run_libra_command(
        &["for-each-ref", "--heads", "--contains", "refs/tags/lw"],
        p,
    );
    assert_cli_success(&out, "for-each-ref --contains refs/tags/lw");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("refs/heads/main") && !stdout.contains("refs/heads/old"),
        "only main should contain c2 (refs/tags/lw): {stdout}"
    );

    // Namespace disambiguation: a branch named `atag` at c2 collides with the
    // annotated tag `atag` at c1. `--merged refs/tags/atag` must resolve the TAG
    // (c1), not the branch (c2) — otherwise main (c2) would be reported merged.
    assert_cli_success(
        &run_libra_command(&["branch", "atag"], p),
        "branch atag at c2 (collides with tag atag)",
    );
    let out = run_libra_command(
        &["for-each-ref", "--heads", "--merged", "refs/tags/atag"],
        p,
    );
    assert_cli_success(&out, "for-each-ref --merged refs/tags/atag");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("refs/heads/old") && !stdout.contains("refs/heads/main"),
        "refs/tags/atag must resolve the TAG (c1), not branch atag (c2): {stdout}"
    );

    // The branch-namespace counterpart: refs/heads/atag must resolve the BRANCH
    // (c2), so main (c2) IS reported merged — proving both directions.
    let out = run_libra_command(
        &["for-each-ref", "--heads", "--merged", "refs/heads/atag"],
        p,
    );
    assert_cli_success(&out, "for-each-ref --merged refs/heads/atag");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("refs/heads/main") && stdout.contains("refs/heads/old"),
        "refs/heads/atag must resolve the BRANCH (c2), so main is merged: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_merged_resolves_remote_tracking_namespace() {
    use libra::internal::branch::Branch;

    let temp = tempdir().unwrap();
    test::setup_with_new_libra_in(temp.path()).await;
    let _guard = test::ChangeDirGuard::new(temp.path());
    let p = temp.path();

    // c1 then c2 on main.
    std::fs::write("a.txt", "1\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["a.txt".into()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        pathspec_from_file: None,
        pathspec_file_nul: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("c1".into()),
        no_verify: true,
        ..Default::default()
    })
    .await;
    let c1 = String::from_utf8_lossy(&run_libra_command(&["rev-parse", "HEAD"], p).stdout)
        .trim()
        .to_string();

    std::fs::write("a.txt", "2\n").unwrap();
    add::execute(AddArgs {
        pathspec: vec!["a.txt".into()],
        all: false,
        update: false,
        refresh: false,
        force: false,
        verbose: false,
        dry_run: false,
        ignore_errors: false,
        pathspec_from_file: None,
        pathspec_file_nul: false,
    })
    .await;
    commit::execute(CommitArgs {
        message: Some("c2".into()),
        no_verify: true,
        ..Default::default()
    })
    .await;

    // Remote-tracking origin/main at c1, stored under the full ref name with the
    // `remote` column set — exactly as `libra fetch` persists it — plus a
    // colliding LOCAL branch literally named `refs/remotes/origin/main` at c2.
    Branch::update_branch("refs/remotes/origin/main", &c1, Some("origin"))
        .await
        .expect("create remote-tracking origin/main");
    assert_cli_success(
        &run_libra_command(&["branch", "refs/remotes/origin/main"], p),
        "create colliding local branch",
    );

    // `--no-merged refs/remotes/origin/main` must resolve the REMOTE-tracking ref
    // (c1), so main (c2) is NOT merged into c1 and is listed. If the local shadow
    // (c2) were used instead, main would be excluded.
    let out = run_libra_command(
        &[
            "for-each-ref",
            "--heads",
            "--no-merged",
            "refs/remotes/origin/main",
        ],
        p,
    );
    assert_cli_success(&out, "for-each-ref --no-merged refs/remotes/origin/main");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("refs/heads/main"),
        "refs/remotes/origin/main must resolve the remote-tracking ref (c1), not \
         the colliding local branch (c2); main should be listed: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_format_and_json() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;

    let output = run_libra_command(
        &["--json", "for-each-ref", "--heads", "--format=%(refname)"],
        temp.path(),
    );
    assert_cli_success(&output, "for-each-ref --json should succeed");

    let json = parse_json_stdout(&output);
    assert_eq!(json["command"], "for-each-ref");
    let entries = json["data"].as_array().expect("data should be an array");
    assert!(
        entries
            .iter()
            .any(|entry| entry["refname"] == "refs/heads/main"),
        "expected refs/heads/main in JSON output"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_sort_and_count() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;

    let output = run_libra_command(&["for-each-ref", "--count=1"], temp.path());
    assert_cli_success(&output, "for-each-ref --count should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected exactly one line, got: {stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_points_at_matches_direct_and_peeled_tag_targets() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;

    let lightweight = run_libra_command(&["tag", "lw"], temp.path());
    assert_cli_success(&lightweight, "tag lw should succeed");
    let annotated = run_libra_command(&["tag", "-m", "annotated", "ann"], temp.path());
    assert_cli_success(&annotated, "tag -m ann should succeed");

    let head_output = run_libra_command(
        &[
            "for-each-ref",
            "--points-at",
            "HEAD",
            "--format=%(refname) %(objecttype)",
        ],
        temp.path(),
    );
    assert_cli_success(&head_output, "for-each-ref --points-at HEAD should succeed");
    let head_stdout = String::from_utf8_lossy(&head_output.stdout);
    assert!(
        head_stdout.contains("refs/heads/main commit"),
        "expected main branch in --points-at HEAD output, got: {head_stdout}"
    );
    assert!(
        head_stdout.contains("refs/tags/lw commit"),
        "expected lightweight tag in --points-at HEAD output, got: {head_stdout}"
    );
    assert!(
        head_stdout.contains("refs/tags/ann tag"),
        "expected annotated tag in --points-at HEAD output, got: {head_stdout}"
    );

    let tag_object_output = run_libra_command(
        &["for-each-ref", "--points-at", "ann", "--format=%(refname)"],
        temp.path(),
    );
    assert_cli_success(
        &tag_object_output,
        "for-each-ref --points-at ann should succeed",
    );
    let tag_stdout = String::from_utf8_lossy(&tag_object_output.stdout);
    assert_eq!(
        tag_stdout.trim(),
        "refs/tags/ann",
        "expected only annotated tag object ref, got: {tag_stdout}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_unknown_sort_rejects() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;

    let output = run_libra_command(&["for-each-ref", "--sort=unknown"], temp.path());
    assert!(
        !output.status.success(),
        "expected failure for unsupported sort key"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unsupported for-each-ref sort key"),
        "got: {stderr}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_sort_version_refname() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;
    let p = temp.path();
    for v in ["v1.10", "v1.9", "v1.2", "v2.0", "v1.10.1"] {
        run_libra_command(&["tag", v], p);
    }

    let output = run_libra_command(
        &[
            "for-each-ref",
            "--sort=version:refname",
            "--format=%(refname)",
        ],
        p,
    );
    assert_cli_success(&output, "for-each-ref --sort=version:refname");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let tags: Vec<&str> = stdout
        .lines()
        .filter(|l| l.contains("refs/tags/v"))
        .collect();
    let pos = |needle: &str| {
        tags.iter()
            .position(|l| l.ends_with(needle))
            .unwrap_or_else(|| panic!("missing {needle} in {tags:?}"))
    };
    // Numeric ordering: v1.9 must come before v1.10 (lexical sort gets this wrong).
    assert!(pos("v1.2") < pos("v1.9"), "v1.2 before v1.9: {tags:?}");
    assert!(
        pos("v1.9") < pos("v1.10"),
        "v1.9 before v1.10 (numeric, not lexical): {tags:?}"
    );
    assert!(
        pos("v1.10") < pos("v1.10.1"),
        "v1.10 before v1.10.1: {tags:?}"
    );
    assert!(
        pos("v1.10.1") < pos("v2.0"),
        "v1.10.1 before v2.0: {tags:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_format_short_atoms() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;
    let p = temp.path();
    run_libra_command(&["branch", "feature-x"], p);

    // %(refname:short) strips the refs/heads/ namespace.
    let out = run_libra_command(
        &[
            "for-each-ref",
            "--heads",
            "--format=%(refname) => %(refname:short)",
        ],
        p,
    );
    assert_cli_success(&out, "for-each-ref %(refname:short)");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.contains("refs/heads/main => main"),
        "short refname for main: {s:?}"
    );
    assert!(
        s.contains("refs/heads/feature-x => feature-x"),
        "short refname for feature-x: {s:?}"
    );

    // %(objectname:short) is the 7-char prefix of %(objectname).
    let out = run_libra_command(
        &[
            "for-each-ref",
            "--heads",
            "--format=%(objectname) %(objectname:short)",
        ],
        p,
    );
    assert_cli_success(&out, "for-each-ref %(objectname:short)");
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next().unwrap_or("");
    let parts: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(parts.len(), 2, "expected full + short hash: {line:?}");
    assert_eq!(parts[1].len(), 7, "short hash should be 7 chars: {line:?}");
    assert!(
        parts[0].starts_with(parts[1]),
        "short hash must prefix the full hash: {line:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_head_marker_atom() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await; // checked out on main
    let p = temp.path();
    run_libra_command(&["branch", "feature-x"], p);

    // %(HEAD) is `*` for the current branch and a space otherwise.
    let out = run_libra_command(
        &[
            "for-each-ref",
            "--heads",
            "--format=%(HEAD)%(refname:short)",
        ],
        p,
    );
    assert_cli_success(&out, "for-each-ref %(HEAD)");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.lines().any(|l| l == "*main"),
        "current branch should be marked with *: {s:?}"
    );
    assert!(
        s.lines().any(|l| l == " feature-x"),
        "non-current branch should get a space: {s:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_upstream_atom() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;
    let p = temp.path();
    run_libra_command(&["branch", "feature-y"], p); // no upstream

    // Configure main's upstream tracking ref (origin/main).
    assert_cli_success(
        &run_libra_command(&["config", "branch.main.remote", "origin"], p),
        "config branch.main.remote",
    );
    assert_cli_success(
        &run_libra_command(&["config", "branch.main.merge", "refs/heads/main"], p),
        "config branch.main.merge",
    );

    let out = run_libra_command(
        &[
            "for-each-ref",
            "--heads",
            "--format=%(refname:short)|%(upstream)|%(upstream:short)",
        ],
        p,
    );
    assert_cli_success(&out, "for-each-ref %(upstream)");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.lines()
            .any(|l| l == "main|refs/remotes/origin/main|origin/main"),
        "configured upstream atoms for main: {s:?}"
    );
    assert!(
        s.lines().any(|l| l == "feature-y||"),
        "branch without upstream has empty %(upstream): {s:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_push_atom() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;
    let p = temp.path();

    // Read main's `%(push)` / `%(push:short)` line.
    let push_line = || -> String {
        let out = run_libra_command(
            &[
                "for-each-ref",
                "--heads",
                "--format=%(refname:short)|%(push)|%(push:short)",
            ],
            p,
        );
        assert_cli_success(&out, "for-each-ref %(push)");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find(|line| line.starts_with("main|"))
            .unwrap_or("")
            .to_string()
    };

    // With no remote config, %(push) is empty.
    assert_eq!(push_line(), "main||", "no remote config → empty push");

    // With only branch.main.remote, the push ref equals the upstream ref.
    assert_cli_success(
        &run_libra_command(&["config", "branch.main.remote", "origin"], p),
        "branch.main.remote",
    );
    assert_cli_success(
        &run_libra_command(&["config", "branch.main.merge", "refs/heads/main"], p),
        "branch.main.merge",
    );
    assert_eq!(
        push_line(),
        "main|refs/remotes/origin/main|origin/main",
        "push falls back to branch remote"
    );

    // With BOTH remote.pushDefault and branch.main.pushRemote set, pushRemote wins
    // (pins the full pushRemote > pushDefault > remote order).
    assert_cli_success(
        &run_libra_command(&["config", "remote.pushDefault", "pdef"], p),
        "remote.pushDefault",
    );
    assert_cli_success(
        &run_libra_command(&["config", "branch.main.pushRemote", "fork"], p),
        "branch.main.pushRemote",
    );
    assert_eq!(
        push_line(),
        "main|refs/remotes/fork/main|fork/main",
        "pushRemote overrides both pushDefault and remote"
    );

    // With pushRemote unset, remote.pushDefault applies (over branch.main.remote).
    assert_cli_success(
        &run_libra_command(&["config", "--unset", "branch.main.pushRemote"], p),
        "unset pushRemote",
    );
    assert_eq!(
        push_line(),
        "main|refs/remotes/pdef/main|pdef/main",
        "pushDefault applies before branch remote"
    );

    // The lowercase Git-config variable form (`pushremote`) is honored too.
    assert_cli_success(
        &run_libra_command(&["config", "branch.main.pushremote", "lower"], p),
        "branch.main.pushremote (lowercase)",
    );
    assert_eq!(
        push_line(),
        "main|refs/remotes/lower/main|lower/main",
        "lowercase pushremote variable is honored"
    );

    // Variable names are case-insensitive. In the (anomalous) case where two
    // case-variant rows coexist, resolution is deterministic: the most recently
    // inserted variant wins. Inserting a fresh camelCase row now takes
    // precedence over the earlier lowercase value.
    assert_cli_success(
        &run_libra_command(&["config", "branch.main.pushRemote", "camel2"], p),
        "branch.main.pushRemote (re-set, newest)",
    );
    assert_eq!(
        push_line(),
        "main|refs/remotes/camel2/main|camel2/main",
        "most recently inserted case variant wins"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_subject_atom() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await; // commits with subject "initial"
    let p = temp.path();

    // %(subject) renders the first line of the commit message.
    let out = run_libra_command(
        &[
            "for-each-ref",
            "--heads",
            "--format=%(refname:short)|%(subject)",
        ],
        p,
    );
    assert_cli_success(&out, "for-each-ref %(subject)");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.lines().any(|l| l == "main|initial"),
        "subject for main's commit should be 'initial': {s:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_subject_with_percent_paren_is_literal() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;
    let p = temp.path();
    // A commit whose subject itself contains `%(` must NOT be re-parsed as a
    // format atom nor trip the unknown-atom error.
    std::fs::write(p.join("x.txt"), "x\n").unwrap();
    run_libra_command(&["add", "x.txt"], p);
    run_libra_command(&["commit", "-m", "fix %(weird) thing", "--no-verify"], p);

    let out = run_libra_command(&["for-each-ref", "--heads", "--format=%(subject)"], p);
    assert_cli_success(&out, "for-each-ref %(subject) with %( in subject");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.lines().any(|l| l == "fix %(weird) thing"),
        "subject containing %( must render literally: {s:?}"
    );

    // A genuinely unknown atom still errors.
    let bad = run_libra_command(&["for-each-ref", "--heads", "--format=%(bogus)"], p);
    assert!(
        !bad.status.success(),
        "unknown atom should fail: {}",
        String::from_utf8_lossy(&bad.stderr)
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_author_committer_atoms() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;
    let p = temp.path();

    let out = run_libra_command(
        &[
            "for-each-ref",
            "--heads",
            "--format=%(authorname)|%(authoremail)|%(committername)|%(committeremail)",
        ],
        p,
    );
    assert_cli_success(&out, "for-each-ref author/committer atoms");
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next().unwrap_or("");
    let f: Vec<&str> = line.split('|').collect();
    assert_eq!(f.len(), 4, "four author/committer fields: {line:?}");
    assert!(
        !f[0].is_empty(),
        "authorname non-empty for a commit ref: {line:?}"
    );
    assert!(
        f[1].starts_with('<') && f[1].ends_with('>'),
        "authoremail is angle-bracketed: {line:?}"
    );
    assert!(!f[2].is_empty(), "committername non-empty: {line:?}");
    assert!(
        f[3].starts_with('<') && f[3].ends_with('>'),
        "committeremail is angle-bracketed: {line:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_tagger_atoms() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;
    let p = temp.path();
    // Create an annotated tag (`-m` implies annotated; it carries a tagger).
    run_libra_command(&["tag", "-m", "release one", "v1"], p);

    // %(taggername)/%(taggeremail) populated for the annotated tag.
    let out = run_libra_command(
        &[
            "for-each-ref",
            "--tags",
            "--format=%(taggername)|%(taggeremail)",
        ],
        p,
    );
    assert_cli_success(&out, "for-each-ref tagger atoms");
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next().unwrap_or("");
    let f: Vec<&str> = line.split('|').collect();
    assert_eq!(f.len(), 2, "two tagger fields: {line:?}");
    assert!(
        !f[0].is_empty(),
        "taggername non-empty for annotated tag: {line:?}"
    );
    assert!(
        f[1].starts_with('<') && f[1].ends_with('>'),
        "taggeremail is angle-bracketed: {line:?}"
    );

    // For a commit (branch) ref, tagger atoms are empty.
    let out = run_libra_command(
        &[
            "for-each-ref",
            "--heads",
            "--format=[%(taggername)][%(taggeremail)]",
        ],
        p,
    );
    assert_cli_success(&out, "for-each-ref tagger on commit");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        s.lines().any(|l| l == "[][]"),
        "tagger atoms empty for a commit ref: {s:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_date_atoms() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;
    let p = temp.path();

    // %(committerdate)/%(authordate) render in Git's default date format
    // (`Day Mon DD HH:MM:SS YYYY +ZZZZ`), in UTC (consistent with `libra log`).
    let out = run_libra_command(
        &[
            "for-each-ref",
            "--heads",
            "--format=%(authordate)|%(committerdate)",
        ],
        p,
    );
    assert_cli_success(&out, "for-each-ref date atoms");
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next().unwrap_or("");
    let (adate, cdate) = line.split_once('|').unwrap_or(("", ""));
    // Default format ends with a `+ZZZZ` zone and contains a 4-digit year.
    for d in [adate, cdate] {
        assert!(
            d.contains("+0000"),
            "date renders in UTC default format: {line:?}"
        );
        assert!(d.contains("20"), "date contains a year: {line:?}");
    }
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_refname_lstrip_rstrip() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await; // refs/heads/main
    let p = temp.path();
    let f = |spec: &str| {
        let fmt = format!("--format=%(refname:{spec})");
        let out = run_libra_command(&["for-each-ref", "--heads", &fmt], p);
        assert_cli_success(&out, spec);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    };
    assert_eq!(
        f("lstrip=1"),
        "heads/main",
        "lstrip=1 drops 1 leading component"
    );
    assert_eq!(f("lstrip=2"), "main", "lstrip=2 drops 2 leading components");
    assert_eq!(f("lstrip=-1"), "main", "lstrip=-1 keeps the last component");
    assert_eq!(
        f("rstrip=1"),
        "refs/heads",
        "rstrip=1 drops 1 trailing component"
    );
    assert_eq!(
        f("rstrip=-1"),
        "refs",
        "rstrip=-1 keeps the first component"
    );
    assert_eq!(f("lstrip=5"), "", "lstrip beyond depth yields empty");
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_contents_and_body_atoms() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;
    let p = temp.path();
    // A commit with a subject and a body paragraph (single message with a
    // blank-line separator).
    std::fs::write(p.join("x.txt"), "x\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "x.txt"], p), "add x.txt");
    // `--cleanup=verbatim` preserves the blank line separating subject/body
    // (the default cleanup would collapse it).
    assert_cli_success(
        &run_libra_command(
            &[
                "commit",
                "-m",
                "the subject\n\nthe body",
                "--cleanup=verbatim",
                "--no-verify",
            ],
            p,
        ),
        "commit subject+body",
    );

    let field = |spec: &str| {
        let fmt = format!("--format=[%({spec})]");
        let out = run_libra_command(&["for-each-ref", "--heads", &fmt], p);
        assert_cli_success(&out, spec);
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // %(contents:subject) is the subject only.
    let subj = field("contents:subject");
    assert!(subj.contains("the subject"), "subject present: {subj:?}");
    assert!(
        !subj.contains("the body"),
        "subject excludes body: {subj:?}"
    );
    // %(body) is the body only.
    let body = field("body");
    assert!(body.contains("the body"), "body present: {body:?}");
    assert!(
        !body.contains("the subject"),
        "body excludes subject: {body:?}"
    );
    // %(contents) has both.
    let contents = field("contents");
    assert!(
        contents.contains("the subject") && contents.contains("the body"),
        "contents has subject and body: {contents:?}"
    );
}

#[tokio::test]
#[serial]
async fn test_for_each_ref_objectname_short_n() {
    let temp = tempdir().unwrap();
    setup_repo_with_commit(&temp).await;
    let p = temp.path();

    let full = {
        let out = run_libra_command(&["for-each-ref", "--heads", "--format=%(objectname)"], p);
        assert_cli_success(&out, "objectname");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    };
    let field = |spec: &str| {
        let fmt = format!("--format=%(objectname:short={spec})");
        let out = run_libra_command(&["for-each-ref", "--heads", &fmt], p);
        assert_cli_success(&out, spec);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    };
    let s10 = field("10");
    assert_eq!(s10.len(), 10, "short=10 yields 10 chars: {s10:?}");
    assert!(
        full.starts_with(&s10),
        "short=10 is a prefix of the full oid"
    );
    let s4 = field("4");
    assert_eq!(s4.len(), 4, "short=4 yields 4 chars: {s4:?}");
    // N beyond the hash length yields the full oid.
    let big = field("64");
    assert_eq!(big, full, "short=64 yields the full oid for sha1");
}

#[test]
fn test_for_each_ref_exclude_filter() {
    use super::{assert_cli_success, create_committed_repo_via_cli, run_libra_command};

    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    for b in ["feature-a", "feature-b", "release-x"] {
        assert_cli_success(&run_libra_command(&["branch", b], p), "branch");
    }

    // Without --exclude, all heads are listed.
    let all = run_libra_command(&["for-each-ref", "--heads"], p);
    assert_cli_success(&all, "for-each-ref --heads");
    let all_s = String::from_utf8_lossy(&all.stdout);
    assert!(
        all_s.contains("feature-a") && all_s.contains("release-x"),
        "all heads listed: {all_s}"
    );

    // --exclude drops refs whose name matches the pattern (applied after includes).
    let ex = run_libra_command(&["for-each-ref", "--heads", "--exclude", "feature"], p);
    assert_cli_success(&ex, "for-each-ref --exclude");
    let ex_s = String::from_utf8_lossy(&ex.stdout);
    assert!(
        !ex_s.contains("feature-a") && !ex_s.contains("feature-b"),
        "feature refs excluded: {ex_s}"
    );
    assert!(
        ex_s.contains("release-x") && ex_s.contains("main"),
        "non-matching refs kept: {ex_s}"
    );
}
