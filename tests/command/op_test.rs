//! Integration tests for `libra op` log/show/restore flows.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::path::Path;

use libra::{
    internal::{branch::Branch, db::get_db_conn_instance, head::Head, model::config_kv},
    utils::test::ChangeDirGuard,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
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
fn test_op_log_quiet_suppresses_stdout() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let output = run_libra_command(&["--quiet", "op", "log", "-n", "1"], repo.path());
    assert_cli_success(&output, "quiet op log");
    assert!(output.stdout.is_empty(), "unexpected stdout: {:?}", output.stdout);
}

#[test]
fn test_op_log_zero_inputs_normalize_to_default_page_shape() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let json = run_json_op(repo.path(), &["log", "-n", "0", "--page", "0"]);
    assert_eq!(json["data"]["page"], Value::from(1));
    assert_eq!(json["data"]["per_page"], Value::from(50));
    assert_eq!(json["data"]["total"], Value::from(1));
    assert_eq!(json["data"]["operations"].as_array().expect("operations").len(), 1);
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
    assert_eq!(page_one["data"]["total"], Value::from(3));
    assert_eq!(page_two["data"]["page"], Value::from(2));
    assert_eq!(page_two["data"]["per_page"], Value::from(1));
    assert_eq!(page_two["data"]["total"], Value::from(3));

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
fn test_op_log_json_command_filter_preserves_filtered_total_across_pages() {
    let repo = create_committed_repo_via_cli();

    for branch_name in ["feature", "topic", "release"] {
        let output = run_libra_command(&["branch", branch_name], repo.path());
        assert_cli_success(&output, branch_name);
    }

    let target_op_id = latest_operation_id(repo.path());
    let restore = run_json_op(repo.path(), &["restore", &target_op_id]);
    assert_eq!(restore["data"]["action"], "restore");

    let page_one = run_json_op(
        repo.path(),
        &["log", "-n", "2", "--page", "1", "--command", "branch"],
    );
    let page_two = run_json_op(
        repo.path(),
        &["log", "-n", "2", "--page", "2", "--command", "branch"],
    );
    let restore_only = run_json_op(
        repo.path(),
        &["log", "-n", "10", "--page", "1", "--command", "op restore"],
    );

    assert_eq!(page_one["data"]["total"], Value::from(3));
    assert_eq!(page_one["data"]["page"], Value::from(1));
    assert_eq!(page_one["data"]["per_page"], Value::from(2));
    assert_eq!(
        page_one["data"]["operations"].as_array().expect("operations").len(),
        2
    );
    assert_eq!(page_one["data"]["operations"][0]["command_name"], "branch");
    assert_eq!(page_one["data"]["operations"][1]["command_name"], "branch");
    assert!(page_one["data"]["operations"][0]["description"]
        .as_str()
        .expect("expected description")
        .contains("create branch release"));
    assert!(page_one["data"]["operations"][1]["description"]
        .as_str()
        .expect("expected description")
        .contains("create branch topic"));

    assert_eq!(page_two["data"]["total"], Value::from(3));
    assert_eq!(page_two["data"]["page"], Value::from(2));
    assert_eq!(page_two["data"]["per_page"], Value::from(2));
    assert_eq!(
        page_two["data"]["operations"].as_array().expect("operations").len(),
        1
    );
    assert_eq!(page_two["data"]["operations"][0]["command_name"], "branch");
    assert!(page_two["data"]["operations"][0]["description"]
        .as_str()
        .expect("expected description")
        .contains("create branch feature"));

    assert_eq!(restore_only["data"]["total"], Value::from(1));
    assert_eq!(
        restore_only["data"]["operations"][0]["command_name"],
        "op restore"
    );
}

#[test]
fn test_op_log_json_command_filter_with_no_matches_returns_empty_page() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let json = run_json_op(repo.path(), &["log", "-n", "10", "--command", "merge"]);
    assert_eq!(json["data"]["page"], Value::from(1));
    assert_eq!(json["data"]["per_page"], Value::from(10));
    assert_eq!(json["data"]["total"], Value::from(0));
    assert!(json["data"]["operations"]
        .as_array()
        .expect("operations")
        .is_empty());
}

#[test]
fn test_op_log_json_whitespace_command_filter_is_treated_as_unfiltered() {
    let repo = create_committed_repo_via_cli();

    for branch_name in ["feature", "topic"] {
        let output = run_libra_command(&["branch", branch_name], repo.path());
        assert_cli_success(&output, branch_name);
    }

    let json = run_json_op(repo.path(), &["log", "-n", "10", "--command", "   "]);
    assert_eq!(json["data"]["total"], Value::from(2));
    assert_eq!(json["data"]["operations"].as_array().expect("operations").len(), 2);
}

#[test]
fn test_op_log_human_page_two_uses_filtered_global_index() {
    let repo = create_committed_repo_via_cli();

    for branch_name in ["feature", "topic", "release"] {
        let output = run_libra_command(&["branch", branch_name], repo.path());
        assert_cli_success(&output, branch_name);
    }

    let output = run_libra_command(
        &["op", "log", "-n", "1", "--page", "2", "--command", "branch"],
        repo.path(),
    );
    assert_cli_success(&output, "op log page 2 branch filter");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("shown 1"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("@{1} branch"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("create branch topic"), "unexpected stdout: {stdout}");
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
fn test_op_show_out_of_range_index_reports_invalid_target() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let output = run_libra_command(&["op", "show", "@{99}"], repo.path());
    let (human, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(
        human.contains("fatal: operation index 99 out of range"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.category, "cli");
    assert_eq!(report.exit_code, 129);
    assert_eq!(
        report.hints,
        vec!["use 'libra op log' to see available operations"]
    );
}

#[test]
fn test_op_show_invalid_index_format_reports_invalid_arguments() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let output = run_libra_command(&["op", "show", "@{abc}"], repo.path());
    let (human, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(
        human.contains("fatal: invalid operation index: @{abc}"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-002");
    assert_eq!(report.category, "cli");
    assert_eq!(report.exit_code, 129);
}

#[test]
fn test_op_show_unknown_operation_id_reports_invalid_target() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let output = run_libra_command(&["op", "show", "missing-op-id"], repo.path());
    let (human, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(
        human.contains("fatal: operation 'missing-op-id' not found"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.category, "cli");
    assert_eq!(report.exit_code, 129);
    assert_eq!(
        report.hints,
        vec!["use 'libra op log' to list available operations"]
    );
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

#[test]
fn test_op_restore_dirty_worktree_is_rejected_without_recording_new_operation() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    std::fs::write(repo.path().join("tracked.txt"), "tracked\ndirty change\n")
        .expect("failed to dirty tracked file");

    let before = listed_operation_count(repo.path());
    let output = run_libra_command(&["op", "restore", "@{0}"], repo.path());
    let (human, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert!(
        human.contains("fatal: working tree has uncommitted changes"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CONFLICT-001");
    assert_eq!(report.category, "conflict");
    assert_eq!(report.exit_code, 128);
    assert_eq!(
        report.hints,
        vec!["use --force to restore anyway, or commit/stash changes first"]
    );

    let after = listed_operation_count(repo.path());
    assert_eq!(after, before, "rejected restore must not record a new operation");
}

#[test]
fn test_op_restore_unknown_operation_id_reports_invalid_target() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let output = run_libra_command(&["op", "restore", "missing-op-id"], repo.path());
    let (human, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(129));
    assert!(
        human.contains("fatal: operation 'missing-op-id' not found"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-CLI-003");
    assert_eq!(report.category, "cli");
    assert_eq!(report.exit_code, 129);
}

#[test]
fn test_op_restore_force_allows_dirty_worktree_and_records_new_operation() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    std::fs::write(repo.path().join("tracked.txt"), "tracked\ndirty change\n")
        .expect("failed to dirty tracked file");

    let before = listed_operation_count(repo.path());
    let output = run_libra_command(&["op", "restore", "@{0}", "--force"], repo.path());
    assert_cli_success(&output, "op restore --force");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Restored to operation"), "unexpected stdout: {stdout}");
    assert!(stdout.contains("New operation recorded:"), "unexpected stdout: {stdout}");

    let after = listed_operation_count(repo.path());
    assert_eq!(after, before + 1, "forced restore should record a new operation");
    assert_eq!(run_json_op(repo.path(), &["log", "-n", "1"])["data"]["operations"][0]["command_name"], "op restore");
}

#[tokio::test]
#[serial]
async fn test_op_restore_uses_default_actor_when_user_name_is_missing() {
    let repo = create_committed_repo_via_cli();

    let feature = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&feature, "branch feature");

    let _guard = ChangeDirGuard::new(repo.path());
    let db = get_db_conn_instance().await;
    config_kv::Entity::delete_many()
        .filter(config_kv::Column::Key.eq("user.name"))
        .exec(&db)
        .await
        .expect("failed to delete user.name");

    let restore = run_json_op(repo.path(), &["restore", "@{0}"]);
    assert_eq!(restore["data"]["action"], Value::String("restore".to_string()));

    let latest = run_json_op(repo.path(), &["log", "-n", "1"]);
    assert_eq!(latest["data"]["operations"][0]["actor"], "libra-user");
}

#[tokio::test]
#[serial]
async fn test_op_log_missing_repo_id_reports_repo_corrupt() {
    let repo = create_committed_repo_via_cli();

    let _guard = ChangeDirGuard::new(repo.path());
    let db = get_db_conn_instance().await;
    config_kv::Entity::delete_many()
        .filter(config_kv::Column::Key.eq("libra.repoid"))
        .exec(&db)
        .await
        .expect("failed to delete libra.repoid");

    let output = run_libra_command(&["op", "log"], repo.path());
    let (human, report) = parse_cli_error_stderr(&output.stderr);

    assert_eq!(output.status.code(), Some(128));
    assert!(
        human.contains("fatal: repository id is missing"),
        "unexpected stderr: {human}"
    );
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert_eq!(report.category, "repo");
    assert_eq!(report.exit_code, 128);
    assert_eq!(
        report.hints,
        vec!["run 'libra init' to initialize repository metadata"]
    );
}

#[test]
fn test_op_command_smoke_sequence_passes() {
    let repo = create_committed_repo_via_cli();

    let branch = run_libra_command(&["branch", "feature"], repo.path());
    assert_cli_success(&branch, "branch feature");

    let log = run_libra_command(&["op", "log", "-n", "5"], repo.path());
    assert_cli_success(&log, "op log");

    let show = run_libra_command(&["op", "show", "@{0}"], repo.path());
    assert_cli_success(&show, "op show");

    let dry_run = run_libra_command(&["op", "restore", "@{0}", "--dry-run"], repo.path());
    assert_cli_success(&dry_run, "op restore --dry-run");
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

    let commit_output =
        run_libra_command(&["commit", "-m", "feature update", "--no-verify"], repo.path());
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