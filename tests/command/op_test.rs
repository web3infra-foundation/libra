//! Integration tests for `libra op` log/show/restore flows.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::path::Path;

use libra::{
    internal::{branch::Branch, head::Head},
    utils::test::ChangeDirGuard,
};
use serde_json::Value;

use super::*;

fn run_json_op(repo: &Path, args: &[&str]) -> Value {
    let mut full_args = vec!["--json", "op"];
    full_args.extend_from_slice(args);

    let output = run_libra_command(&full_args, repo);
    assert_cli_success(&output, "op json command should succeed");
    parse_json_stdout(&output)
}

fn latest_operation_id(repo: &Path) -> String {
    run_json_op(repo, &["log", "-n", "1"])["data"]["operations"][0]["op_id"]
        .as_str()
        .expect("expected latest operation id")
        .to_string()
}

fn listed_operation_count(repo: &Path) -> u64 {
    run_json_op(repo, &["log", "-n", "20"])["data"]["total"]
        .as_u64()
        .expect("expected operation count")
}

#[test]
fn test_op_log_json_lists_latest_operations_newest_first() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let topic = run_libra_command(&["branch", "topic"], repo.path());
    assert_cli_success(&topic, "branch topic");

    let json = run_json_op(repo.path(), &["log", "-n", "10"]);
    assert_eq!(json["command"], Value::String("op".to_string()));

    let data = &json["data"];
    assert_eq!(data["action"], Value::String("log".to_string()));
    assert_eq!(data["page"], Value::from(1));
    assert_eq!(data["per_page"], Value::from(10));
    assert_eq!(data["total"], Value::from(2));

    let operations = data["operations"]
        .as_array()
        .expect("expected operations array");
    assert_eq!(operations.len(), 2);
    assert_eq!(operations[0]["command_name"], "branch");
    assert_eq!(operations[0]["status"], "succeeded");
    assert!(operations[0]["description"]
        .as_str()
        .expect("expected description")
        .contains("create branch topic"));
    assert!(operations[1]["description"]
        .as_str()
        .expect("expected description")
        .contains("create branch feature"));
}

#[test]
fn test_op_log_verbose_includes_core_fields() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let output = run_libra_command(&["op", "log", "-n", "1", "--verbose"], repo.path());
    assert_cli_success(&output, "op log --verbose");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("command: branch"), "unexpected stdout: {stdout}");
    assert!(
        stdout.contains("description: create branch feature"),
        "unexpected stdout: {stdout}"
    );
    assert!(stdout.contains("actor: Test User"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("status: succeeded"), "unexpected stdout: {stdout}");
}

#[test]
fn test_op_log_json_page_two_returns_older_operation() {
    let repo = create_committed_repo_via_cli();

    for branch_name in ["feature", "topic", "release"] {
        let output = run_libra_command(&["branch", branch_name], repo.path());
        assert_cli_success(&output, branch_name);
    }

    let page_one = run_json_op(repo.path(), &["log", "-n", "1", "--page", "1"]);
    let page_two = run_json_op(repo.path(), &["log", "-n", "1", "--page", "2"]);

    assert_eq!(page_one["data"]["page"], Value::from(1));
    assert_eq!(page_one["data"]["per_page"], Value::from(1));
    assert_eq!(page_two["data"]["page"], Value::from(2));
    assert_eq!(page_two["data"]["per_page"], Value::from(1));

    let first = &page_one["data"]["operations"][0];
    let second = &page_two["data"]["operations"][0];
    assert_ne!(first["op_id"], second["op_id"]);
    assert!(first["description"]
        .as_str()
        .expect("expected description")
        .contains("create branch release"));
    assert!(second["description"]
        .as_str()
        .expect("expected description")
        .contains("create branch topic"));
}

#[test]
fn test_op_show_json_latest_index_resolves_to_branch_operation() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let json = run_json_op(repo.path(), &["show", "@{0}"]);
    assert_eq!(json["command"], Value::String("op".to_string()));

    let data = &json["data"];
    assert_eq!(data["action"], Value::String("show".to_string()));
    assert_eq!(data["command_name"], "branch");
    assert_eq!(data["actor"], "Test User");
    assert_eq!(data["status"], "succeeded");
    assert!(data["description"]
        .as_str()
        .expect("expected description")
        .contains("create branch feature"));
    assert!(data["op_id"].as_str().expect("expected op id").len() > 8);
    assert!(data["view_id"].as_str().expect("expected view id").len() > 8);
}

#[test]
fn test_op_show_view_human_includes_snapshot_refs() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let output = run_libra_command(&["op", "show", "@{0}", "--view"], repo.path());
    assert_cli_success(&output, "op show --view");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("View Snapshot:"), "unexpected stdout: {stdout}");
    assert!(
        stdout.contains("HEAD: main (branch)"),
        "unexpected stdout: {stdout}"
    );
    assert!(stdout.contains("feature:"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("main:"), "unexpected stdout: {stdout}");
}

#[test]
fn test_op_restore_dry_run_does_not_record_new_operation() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let before = listed_operation_count(repo.path());
    let output = run_libra_command(&["op", "restore", "@{0}", "--dry-run"], repo.path());
    assert_cli_success(&output, "op restore --dry-run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Would restore to operation"),
        "unexpected stdout: {stdout}"
    );

    let after = listed_operation_count(repo.path());
    assert_eq!(after, before, "dry-run must not append a new operation");
}

#[tokio::test]
#[serial]
async fn test_op_restore_json_records_new_operation_and_restores_head_and_branch_ref() {
    let repo = create_committed_repo_via_cli();

    let branch_output = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&branch_output, "branch feature");
    let target_op_id = latest_operation_id(repo.path());

    let _guard = ChangeDirGuard::new(repo.path());
    let base_commit = Head::current_commit()
        .await
        .expect("expected base HEAD commit")
        .to_string();

    let switch_output = run_libra_command(&["switch", "feature"], repo.path());
    assert_cli_success(&switch_output, "switch feature");

    std::fs::write(repo.path().join("tracked.txt"), "tracked\nfeature change\n")
        .expect("failed to update tracked file");

    let add_output = run_libra_command(&["add", "tracked.txt"], repo.path());
    assert_cli_success(&add_output, "add tracked.txt");

    let commit_output = run_libra_command(&["commit", "-m", "feature update", "--no-verify"], repo.path());
    assert_cli_success(&commit_output, "commit feature update");

    let feature_tip_before_restore = Head::current_commit()
        .await
        .expect("expected feature HEAD commit")
        .to_string();
    assert_ne!(feature_tip_before_restore, base_commit);

    let restore_json = run_json_op(repo.path(), &["restore", &target_op_id]);
    assert_eq!(restore_json["command"], Value::String("op".to_string()));
    assert_eq!(restore_json["data"]["action"], Value::String("restore".to_string()));
    assert_eq!(restore_json["data"]["target_op_id"], target_op_id);

    let new_op_id = restore_json["data"]["new_op_id"]
        .as_str()
        .expect("expected new operation id")
        .to_string();
    assert_ne!(new_op_id, target_op_id, "restore must record a new operation");

    match Head::current().await {
        Head::Branch(branch_name) => assert_eq!(branch_name, "main"),
        other => panic!("expected HEAD to restore to main branch, got {other:?}"),
    }

    let restored_head = Head::current_commit()
        .await
        .expect("expected restored HEAD commit")
        .to_string();
    assert_eq!(restored_head, base_commit);

    let feature_branch = Branch::find_branch_result("feature", None)
        .await
        .expect("feature branch lookup should succeed")
        .expect("feature branch should still exist after restore");
    assert_eq!(feature_branch.commit.to_string(), base_commit);

    let latest_log = run_json_op(repo.path(), &["log", "-n", "1"]);
    assert_eq!(latest_log["data"]["operations"][0]["op_id"], new_op_id);
    assert_eq!(latest_log["data"]["operations"][0]["command_name"], "op restore");
}