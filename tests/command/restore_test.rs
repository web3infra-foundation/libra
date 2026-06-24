//! Tests restore command paths for worktree and index targets along with pathspec handling.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use libra::{
    internal::{branch::TRACES_BRANCH, db::get_db_conn_instance, head::Head, model::reference},
    utils::test::ChangeDirGuard,
};
use sea_orm::{ActiveModelTrait, Set};

use super::*;

#[test]
#[serial]
fn test_restore_cli_outside_repository_returns_fatal_128() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["restore", "."], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fatal: not a libra repository"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
#[serial]
fn test_restore_source_head_unborn_returns_error_without_falling_back() {
    let repo = tempdir().expect("failed to create repository root");
    init_repo_via_cli(repo.path());
    std::fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to write tracked file");

    let output = run_libra_command(&["restore", "--source", "HEAD", "tracked.txt"], repo.path());
    assert_eq!(output.status.code(), Some(128));

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("fatal: failed to resolve checkout source"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.exit_code, 128);

    let content = std::fs::read_to_string(repo.path().join("tracked.txt"))
        .expect("failed to read tracked file");
    assert_eq!(content, "modified\n");
}

#[test]
#[serial]
fn test_restore_missing_pathspec_returns_cli_invalid_target() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["restore", "missing.txt"], repo.path());
    assert_eq!(output.status.code(), Some(129));

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("fatal: pathspec 'missing.txt' did not match any files"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.exit_code, 129);
}

#[test]
#[serial]
fn test_restore_pathspec_from_file_restores_listed_paths() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();

    std::fs::write(p.join("a.txt"), "committed-a\n").unwrap();
    std::fs::write(p.join("b.txt"), "committed-b\n").unwrap();
    assert_cli_success(&run_libra_command(&["add", "a.txt", "b.txt"], p), "add a/b");
    assert_cli_success(
        &run_libra_command(&["commit", "-m", "add a/b", "--no-verify"], p),
        "commit a/b",
    );

    // Dirty both files, then list them in a pathspec file.
    std::fs::write(p.join("a.txt"), "dirty-a\n").unwrap();
    std::fs::write(p.join("b.txt"), "dirty-b\n").unwrap();
    std::fs::write(p.join("specs.txt"), "a.txt\nb.txt\n").unwrap();

    let output = run_libra_command(&["restore", "--pathspec-from-file", "specs.txt"], p);
    assert_cli_success(&output, "restore --pathspec-from-file");

    assert_eq!(
        std::fs::read_to_string(p.join("a.txt")).unwrap(),
        "committed-a\n",
        "a.txt should be restored from the pathspec file"
    );
    assert_eq!(
        std::fs::read_to_string(p.join("b.txt")).unwrap(),
        "committed-b\n",
        "b.txt should be restored from the pathspec file"
    );
}

#[tokio::test]
#[serial]
async fn test_restore_source_does_not_fall_back_from_unborn_branch_to_hash_prefix() {
    let repo = create_committed_repo_via_cli();
    let _guard = ChangeDirGuard::new(repo.path());

    let head_commit = Head::current_commit()
        .await
        .expect("expected committed repository");
    let branch_name = head_commit.to_string()[..7].to_string();

    let db = get_db_conn_instance().await;
    reference::ActiveModel {
        name: Set(Some(branch_name.clone())),
        kind: Set(reference::ConfigKind::Branch),
        commit: Set(None),
        remote: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("failed to insert unborn branch row");

    std::fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to modify tracked file");

    let output = run_libra_command(
        &["restore", "--source", &branch_name, "tracked.txt"],
        repo.path(),
    );
    assert!(
        !output.status.success(),
        "restore unexpectedly succeeded: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let content = std::fs::read_to_string(repo.path().join("tracked.txt"))
        .expect("failed to read tracked file");
    assert_eq!(
        content, "modified\n",
        "restore should not overwrite from hash fallback"
    );
}

// ── Positive paths: worktree / staged / JSON / confirmation ─────────────

#[test]
#[serial]
fn test_restore_worktree_overwrites_modification_with_committed_blob() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to modify tracked file");

    let output = run_libra_command(&["restore", "tracked.txt"], repo.path());
    assert_cli_success(&output, "restore from index should succeed");

    let restored = std::fs::read_to_string(repo.path().join("tracked.txt"))
        .expect("failed to read restored file");
    assert_eq!(
        restored, "tracked\n",
        "worktree restore should reset content to the indexed blob"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Updated 1 path(s) from the index"),
        "expected confirmation message, got stdout: {stdout}"
    );
}

#[test]
#[serial]
fn test_restore_staged_resets_index_entry_to_head() {
    let repo = create_committed_repo_via_cli();

    std::fs::write(repo.path().join("tracked.txt"), "staged change\n")
        .expect("failed to update tracked file");
    let add = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add, "add should stage the tracked change");

    let restore = run_libra_command(&["restore", "--staged", "tracked.txt"], repo.path());
    assert_cli_success(&restore, "restore --staged should succeed");

    let stdout = String::from_utf8_lossy(&restore.stdout);
    assert!(
        stdout.contains("Updated 1 path(s) from HEAD"),
        "expected confirmation message naming HEAD source, got stdout: {stdout}"
    );

    let status = run_libra_command(&["status", "--json"], repo.path());
    assert_cli_success(&status, "status --json should succeed after staged restore");
    let report = parse_json_stdout(&status);
    let staged = report["data"]["staged"]
        .as_object()
        .expect("status data should expose staged");
    let staged_total = ["new", "modified", "deleted"]
        .iter()
        .map(|key| {
            staged
                .get(*key)
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0)
        })
        .sum::<usize>();
    assert_eq!(
        staged_total, 0,
        "after restore --staged, no staged entries should remain (got {staged:?})"
    );

    let worktree = std::fs::read_to_string(repo.path().join("tracked.txt"))
        .expect("failed to read worktree file");
    assert_eq!(
        worktree, "staged change\n",
        "restore --staged must not touch the worktree copy"
    );
}

#[test]
#[serial]
fn test_restore_json_envelope_reports_restored_files() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to modify tracked file");

    let output = run_libra_command(&["restore", "--json", "tracked.txt"], repo.path());
    assert_cli_success(&output, "restore --json should succeed");

    let envelope = parse_json_stdout(&output);
    assert_eq!(envelope["ok"], Value::Bool(true));
    assert_eq!(envelope["command"], Value::String("restore".to_string()));

    let data = &envelope["data"];
    assert_eq!(data["worktree"], Value::Bool(true));
    assert_eq!(data["staged"], Value::Bool(false));
    assert!(
        data["source"].is_null(),
        "default restore (no --source) should leave source as null, got: {}",
        data["source"]
    );

    let restored = data["restored_files"]
        .as_array()
        .expect("restored_files should be an array");
    assert_eq!(
        restored.len(),
        1,
        "expected exactly one restored file, got: {restored:?}"
    );
    assert_eq!(
        restored[0],
        Value::String("tracked.txt".to_string()),
        "expected tracked.txt in restored_files"
    );

    let deleted = data["deleted_files"]
        .as_array()
        .expect("deleted_files should be an array");
    assert!(
        deleted.is_empty(),
        "no deletions expected, got: {deleted:?}"
    );

    let restored_content = std::fs::read_to_string(repo.path().join("tracked.txt"))
        .expect("failed to read restored file");
    assert_eq!(restored_content, "tracked\n");
}

#[test]
#[serial]
fn test_restore_quiet_suppresses_confirmation_but_still_restores() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to modify tracked file");

    let output = run_libra_command(&["--quiet", "restore", "tracked.txt"], repo.path());
    assert_cli_success(&output, "quiet restore should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.is_empty(),
        "quiet mode should produce no stdout, got: {stdout}"
    );

    let restored = std::fs::read_to_string(repo.path().join("tracked.txt"))
        .expect("failed to read restored file");
    assert_eq!(
        restored, "tracked\n",
        "quiet mode must still perform the restore"
    );
}

// ── Locked-branch guard ─────────────────────────────────────────────────

#[test]
#[serial]
fn test_restore_source_refuses_locked_intent_branch() {
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to modify tracked file");

    let output = run_libra_command(
        &["restore", "--source", "intent", "tracked.txt"],
        repo.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(128),
        "locked-branch restore should exit 128 (fatal)"
    );

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("refusing to restore from locked branch 'intent'"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.exit_code, 128);

    let content = std::fs::read_to_string(repo.path().join("tracked.txt"))
        .expect("failed to read tracked file");
    assert_eq!(
        content, "modified\n",
        "locked-source guard must not modify the worktree"
    );
}

#[test]
#[serial]
fn test_restore_source_refuses_locked_branch_with_revision_suffix() {
    // is_locked_revision strips `~1` / `^` / `@{0}` so users cannot
    // end-run the guard with `traces~1` or similar.
    let repo = create_committed_repo_via_cli();
    std::fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to modify tracked file");

    let output = run_libra_command(
        &["restore", "--source", "traces~1", "tracked.txt"],
        repo.path(),
    );
    assert_eq!(
        output.status.code(),
        Some(128),
        "locked-branch restore with revision suffix should still exit 128"
    );

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert!(
        human.contains("refusing to restore from locked branch 'traces~1'"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
}

#[tokio::test]
#[serial]
async fn test_restore_worktree_refuses_ai_managed_current_branch() {
    let repo = create_committed_repo_via_cli();
    {
        let _guard = ChangeDirGuard::new(repo.path());
        Head::update_result(Head::Branch(TRACES_BRANCH.to_string()), None)
            .await
            .expect("point HEAD at traces");
    }
    std::fs::write(repo.path().join("tracked.txt"), "modified\n")
        .expect("failed to modify tracked file");

    let output = run_libra_command(&["restore", "tracked.txt"], repo.path());

    assert_eq!(output.status.code(), Some(128));
    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CONFLICT-002");
    assert!(
        human.contains("refusing to restore worktree while on locked branch 'traces'"),
        "unexpected stderr: {human}"
    );
    let content = std::fs::read_to_string(repo.path().join("tracked.txt"))
        .expect("failed to read tracked file");
    assert_eq!(
        content, "modified\n",
        "locked-current-branch guard must not modify the worktree"
    );
}

#[test]
fn restore_no_progress_flag_is_accepted_noop() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("r.txt"), "modified\n").unwrap();
    // `--no-progress` is accepted and a no-op: Libra's restore renders no
    // progress meter, so the restore proceeds and reverts the file.
    let out = run_libra_command(&["restore", "--no-progress", "r.txt"], p);
    assert_cli_success(&out, "restore --no-progress r.txt");
}

#[test]
fn restore_no_overlay_flag_is_accepted_noop() {
    let repo = create_committed_repo_via_cli();
    let p = repo.path();
    std::fs::write(p.join("r.txt"), "modified\n").unwrap();
    // `--no-overlay` is accepted and a no-op: Libra's restore is never in
    // overlay mode (it already matches `--no-overlay`, the Git default), so the
    // restore proceeds normally.
    let out = run_libra_command(&["restore", "--no-overlay", "r.txt"], p);
    assert_cli_success(&out, "restore --no-overlay r.txt");
}
