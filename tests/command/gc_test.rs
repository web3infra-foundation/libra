//! Integration tests for `gc`, covering reachability pruning, stale pack cleanup,
//! structured output, and CLI error handling.

use std::fs;

use serial_test::serial;
use tempfile::tempdir;

use super::*;

fn write_unreachable_blob(repo: &std::path::Path, name: &str, contents: &str) -> String {
    fs::write(repo.join(name), contents).expect("failed to write blob source");
    let output = run_libra_command(&["hash-object", "-w", name], repo);
    assert_cli_success(&output, "hash-object -w should create loose blob");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
#[serial]
fn gc_prune_now_removes_unreachable_loose_object() {
    let repo = create_committed_repo_via_cli();
    let oid = write_unreachable_blob(repo.path(), "garbage.txt", "garbage\n");
    assert!(loose_object_path(repo.path(), &oid).exists());

    let output = run_libra_command(&["gc", "--prune=now"], repo.path());
    assert_cli_success(&output, "gc --prune=now should succeed");

    assert!(
        !loose_object_path(repo.path(), &oid).exists(),
        "unreachable loose object should be pruned"
    );
}

#[test]
#[serial]
fn gc_dry_run_reports_without_removing_object() {
    let repo = create_committed_repo_via_cli();
    let oid = write_unreachable_blob(repo.path(), "dry-run.txt", "dry\n");

    let output = run_libra_command(&["gc", "--dry-run", "--prune=now"], repo.path());
    assert_cli_success(&output, "gc --dry-run should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Would prune"),
        "dry-run output should report planned pruning: {stdout}"
    );
    assert!(loose_object_path(repo.path(), &oid).exists());
}

#[test]
#[serial]
fn gc_default_prune_keeps_recent_unreachable_object() {
    let repo = create_committed_repo_via_cli();
    let oid = write_unreachable_blob(repo.path(), "recent.txt", "recent\n");

    let output = run_libra_command(&["gc"], repo.path());
    assert_cli_success(&output, "gc default prune should succeed");

    assert!(
        loose_object_path(repo.path(), &oid).exists(),
        "default prune cutoff should keep recent unreachable objects"
    );
}

#[test]
#[serial]
fn gc_no_prune_keeps_unreachable_object_even_with_now_cutoff() {
    let repo = create_committed_repo_via_cli();
    let oid = write_unreachable_blob(repo.path(), "never.txt", "never\n");

    let output = run_libra_command(&["gc", "--no-prune"], repo.path());
    assert_cli_success(&output, "gc --no-prune should succeed");

    assert!(loose_object_path(repo.path(), &oid).exists());
}

#[test]
#[serial]
fn gc_keeps_reachable_tracked_blob() {
    let repo = create_committed_repo_via_cli();
    let hash_output = run_libra_command(&["hash-object", "tracked.txt"], repo.path());
    assert_cli_success(&hash_output, "hash-object should identify tracked blob");
    let tracked_oid = String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string();

    let output = run_libra_command(&["gc", "--prune=now"], repo.path());
    assert_cli_success(&output, "gc should keep reachable objects");

    let cat = run_libra_command(&["cat-file", "-t", &tracked_oid], repo.path());
    assert_cli_success(&cat, "reachable tracked blob should remain readable");
    assert_eq!(String::from_utf8_lossy(&cat.stdout).trim(), "blob");
}

#[test]
#[serial]
fn gc_json_reports_pruned_loose_object() {
    let repo = create_committed_repo_via_cli();
    let oid = write_unreachable_blob(repo.path(), "json-garbage.txt", "json\n");

    let output = run_libra_command(&["--json", "gc", "--prune=now"], repo.path());
    assert_cli_success(&output, "json gc should succeed");
    let json = parse_json_stdout(&output);

    assert_eq!(json["command"], "gc");
    assert_eq!(json["data"]["dry_run"], false);
    assert_eq!(json["data"]["loose_objects"]["pruned"], 1);
    assert_eq!(json["data"]["unreachable_objects"][0]["oid"], oid);
    assert_eq!(json["data"]["unreachable_objects"][0]["action"], "pruned");
}

#[test]
#[serial]
fn gc_json_dry_run_reports_would_prune() {
    let repo = create_committed_repo_via_cli();
    write_unreachable_blob(repo.path(), "json-dry.txt", "json dry\n");

    let output = run_libra_command(&["--json", "gc", "--dry-run", "--prune=now"], repo.path());
    assert_cli_success(&output, "json dry-run gc should succeed");
    let json = parse_json_stdout(&output);

    assert_eq!(json["data"]["loose_objects"]["pruned"], 0);
    assert_eq!(
        json["data"]["unreachable_objects"][0]["action"],
        "would_prune"
    );
}

#[test]
#[serial]
fn gc_prunes_orphan_pack_index() {
    let repo = create_committed_repo_via_cli();
    let pack_dir = repo.path().join(".libra").join("objects").join("pack");
    fs::create_dir_all(&pack_dir).expect("failed to create pack dir");
    let orphan_idx = pack_dir.join("pack-deadbeef.idx");
    fs::write(&orphan_idx, b"orphan").expect("failed to write orphan idx");

    let output = run_libra_command(&["gc", "--prune=now"], repo.path());
    assert_cli_success(&output, "gc should clean orphan pack index");

    assert!(!orphan_idx.exists());
}

#[test]
#[serial]
fn gc_keeps_orphan_pack_index_when_keep_file_exists() {
    let repo = create_committed_repo_via_cli();
    let pack_dir = repo.path().join(".libra").join("objects").join("pack");
    fs::create_dir_all(&pack_dir).expect("failed to create pack dir");
    let orphan_idx = pack_dir.join("pack-deadbeef.idx");
    let keep = pack_dir.join("pack-deadbeef.keep");
    fs::write(&orphan_idx, b"orphan").expect("failed to write orphan idx");
    fs::write(&keep, b"keep").expect("failed to write keep");

    let output = run_libra_command(&["gc", "--prune=now"], repo.path());
    assert_cli_success(&output, "gc should respect pack keep file");

    assert!(orphan_idx.exists());
}

#[test]
#[serial]
fn gc_rejects_invalid_prune_date() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["gc", "--prune=yesterday"], repo.path());
    assert_eq!(output.status.code(), Some(129));

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert!(
        human.contains("invalid prune date"),
        "human stderr should explain invalid date: {human}"
    );
}

#[test]
#[serial]
fn gc_outside_repository_returns_repo_not_found() {
    let dir = tempdir().expect("failed to create tempdir");

    let output = run_libra_command(&["gc"], dir.path());
    assert_eq!(output.status.code(), Some(128));

    let (human, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-001");
    assert!(
        human.contains("not a libra repository"),
        "human stderr should mention missing repository: {human}"
    );
}

#[test]
#[serial]
fn gc_quiet_suppresses_stdout() {
    let repo = create_committed_repo_via_cli();

    let output = run_libra_command(&["--quiet", "gc", "--prune=never"], repo.path());
    assert_cli_success(&output, "quiet gc should succeed");

    assert!(output.stdout.is_empty());
}
