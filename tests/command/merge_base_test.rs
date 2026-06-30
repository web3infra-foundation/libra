//! Integration tests for `libra merge-base` and `diff A...B`.
//!
//! Layer: L1 (deterministic; tempdir + isolated HOME, no network).

use std::{fs, process::Output};

use tempfile::{TempDir, tempdir};

use super::{
    assert_cli_success, create_committed_repo_via_cli, parse_json_stdout, run_libra_command,
    run_libra_command_with_stdin,
};

fn out_trim(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn rev_parse(repo: &TempDir, rev: &str) -> String {
    out_trim(&run_libra_command(&["rev-parse", rev], repo.path()))
}

/// Build a Y-shaped history: a shared base, then divergent commits on the
/// default branch and on `feature`. Returns `(repo, base, default_tip,
/// feature_tip)`.
fn y_shaped_repo() -> (TempDir, String, String, String) {
    let repo = create_committed_repo_via_cli();
    let base = rev_parse(&repo, "HEAD");

    // Branch `feature` at the base.
    assert_cli_success(
        &run_libra_command(&["branch", "feature"], repo.path()),
        "create feature branch",
    );

    // Advance the default branch with one commit.
    fs::write(repo.path().join("tracked.txt"), "default-change\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "tracked.txt"], repo.path()),
        "stage default change",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "default2", "--no-verify"], repo.path()),
        "commit on default branch",
    );
    let default_tip = rev_parse(&repo, "HEAD");

    // Diverge on feature with a different change.
    assert_cli_success(
        &run_libra_command(&["switch", "feature"], repo.path()),
        "switch to feature",
    );
    fs::write(repo.path().join("feature_only.txt"), "feature-change\n").unwrap();
    assert_cli_success(
        &run_libra_command(&["add", "feature_only.txt"], repo.path()),
        "stage feature change",
    );
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "feature2", "--no-verify"], repo.path()),
        "commit on feature",
    );
    let feature_tip = rev_parse(&repo, "HEAD");

    assert_ne!(base, default_tip);
    assert_ne!(base, feature_tip);
    assert_ne!(default_tip, feature_tip);
    (repo, base, default_tip, feature_tip)
}

#[test]
fn merge_base_of_diverged_branches_is_the_base() {
    let (repo, base, default_tip, feature_tip) = y_shaped_repo();
    let out = run_libra_command(&["merge-base", &default_tip, &feature_tip], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "merge-base failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out_trim(&out), base);
}

#[test]
fn is_ancestor_holds_for_base_of_tip() {
    let (repo, base, default_tip, _feature_tip) = y_shaped_repo();
    let yes = run_libra_command(
        &["merge-base", "--is-ancestor", &base, &default_tip],
        repo.path(),
    );
    assert_eq!(
        yes.status.code(),
        Some(0),
        "base should be an ancestor of the tip"
    );
}

#[test]
fn is_ancestor_rejects_diverged_tips() {
    let (repo, _base, default_tip, feature_tip) = y_shaped_repo();
    let no = run_libra_command(
        &["merge-base", "--is-ancestor", &default_tip, &feature_tip],
        repo.path(),
    );
    assert_eq!(
        no.status.code(),
        Some(1),
        "diverged tips are not ancestors of each other"
    );
}

#[test]
fn all_lists_the_single_base_for_a_simple_merge() {
    let (repo, base, default_tip, feature_tip) = y_shaped_repo();
    let out = run_libra_command(
        &["merge-base", "--all", &default_tip, &feature_tip],
        repo.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    let stdout = out_trim(&out);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec![base.as_str()], "a simple Y has one merge base");
}

#[test]
fn json_reports_bases() {
    let (repo, base, default_tip, feature_tip) = y_shaped_repo();
    let out = run_libra_command(
        &["--json", "merge-base", &default_tip, &feature_tip],
        repo.path(),
    );
    assert_eq!(out.status.code(), Some(0));
    let json = parse_json_stdout(&out);
    assert_eq!(
        json["data"]["bases"].as_array().unwrap().len(),
        1,
        "one base in JSON"
    );
    assert_eq!(json["data"]["bases"][0].as_str(), Some(base.as_str()));
}

#[test]
fn unresolvable_commit_is_an_error() {
    let (repo, _base, _d, _f) = y_shaped_repo();
    let out = run_libra_command(&["merge-base", "definitely-not-a-ref", "HEAD"], repo.path());
    assert_eq!(out.status.code(), Some(128), "bad revision exits 128");
}

#[test]
fn wrong_argument_count_is_a_usage_error() {
    let (repo, _base, _d, _f) = y_shaped_repo();
    let out = run_libra_command(&["merge-base", "HEAD"], repo.path());
    assert_eq!(
        out.status.code(),
        Some(128),
        "merge-base needs exactly two commits"
    );
}

/// Write a commit object verbatim (so we can build arbitrary DAGs) and return
/// its object id.
fn craft_commit(repo: &TempDir, tree: &str, parents: &[&str], message: &str) -> String {
    let mut body = format!("tree {tree}\n");
    for parent in parents {
        body.push_str(&format!("parent {parent}\n"));
    }
    body.push_str("author Test <t@e.com> 1700000000 +0000\n");
    body.push_str("committer Test <t@e.com> 1700000000 +0000\n\n");
    body.push_str(message);
    body.push('\n');
    let out = run_libra_command_with_stdin(
        &[
            "hash-object",
            "-t",
            "commit",
            "--literally",
            "-w",
            "--stdin",
        ],
        repo.path(),
        &body,
    );
    assert_cli_success(&out, "craft commit object");
    out_trim(&out)
}

/// In a criss-cross history, both branch points are lowest common ancestors,
/// so `--all` must return both — the case the old first-found walk gets wrong.
///
/// ```text
///        c0
///       /  \
///      a1   b1
///       \\ //   (mA and mB each merge both a1 and b1)
///       mA mB
/// ```
#[test]
fn all_returns_both_bases_for_criss_cross() {
    let repo = tempdir().unwrap();
    assert_cli_success(&run_libra_command(&["init"], repo.path()), "init");
    let tree = out_trim(&run_libra_command(&["write-tree"], repo.path()));

    let c0 = craft_commit(&repo, &tree, &[], "c0");
    let a1 = craft_commit(&repo, &tree, &[&c0], "a1");
    let b1 = craft_commit(&repo, &tree, &[&c0], "b1");
    let m_a = craft_commit(&repo, &tree, &[&a1, &b1], "mA");
    let m_b = craft_commit(&repo, &tree, &[&a1, &b1], "mB");

    let out = run_libra_command(&["merge-base", "--all", &m_a, &m_b], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "criss-cross merge-base failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = out_trim(&out);
    let mut got: Vec<&str> = stdout.lines().collect();
    got.sort_unstable();
    let mut expected = [a1.as_str(), b1.as_str()];
    expected.sort_unstable();
    assert_eq!(
        got, expected,
        "criss-cross --all must return both branch points (a1, b1)"
    );
}

/// `diff A...B` diffs from merge-base(A, B) to B, so it shows only B's changes
/// (feature_only.txt), not A's (the default-branch change to tracked.txt).
#[test]
fn diff_three_dot_uses_merge_base() {
    let (repo, _base, default_tip, feature_tip) = y_shaped_repo();
    let spec = format!("{default_tip}...{feature_tip}");
    let out = run_libra_command(&["diff", &spec], repo.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "diff A...B failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let diff = String::from_utf8_lossy(&out.stdout);
    assert!(
        diff.contains("feature_only.txt"),
        "A...B should include feature's change: {diff}"
    );
    assert!(
        !diff.contains("tracked.txt"),
        "A...B should NOT include the default branch's change (it is on the A side): {diff}"
    );
}
