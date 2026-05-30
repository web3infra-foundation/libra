//! Integration tests for the `libra usage` command surface.
//!
//! **Layer:** L1 — deterministic, no external dependencies. Covers the
//! cross-cutting `--help` EXAMPLES rollout from
//! `docs/improvement/README.md` item B for the AI usage reporting
//! command.

use libra::{
    internal::{
        ai::{
            completion::CompletionUsageSummary,
            usage::{UsageContext, UsageRecorder},
        },
        db::get_db_conn_instance_for_path,
    },
    utils::util::DATABASE,
};

use super::*;

async fn seed_usage_report_rows(repo: &std::path::Path) {
    let init = run_libra_command(&["init"], repo);
    assert_cli_success(&init, "init repo for usage report test");

    let conn = get_db_conn_instance_for_path(&repo.join(".libra").join(DATABASE))
        .await
        .expect("open repo db");
    let recorder = UsageRecorder::new(conn);
    let summary = CompletionUsageSummary {
        input_tokens: 100,
        output_tokens: 50,
        cached_tokens: Some(10),
        reasoning_tokens: Some(5),
        total_tokens: Some(155),
        cost_usd: Some(0.001),
    };

    for _ in 0..2 {
        recorder
            .record_summary(
                &usage_context(Some("planner"), "openai", "gpt-4o"),
                &summary,
                Some(120),
            )
            .await
            .expect("record planner usage");
    }
    recorder
        .record_summary(
            &usage_context(Some("reviewer"), "deepseek", "deepseek-chat"),
            &summary,
            Some(80),
        )
        .await
        .expect("record reviewer usage");
}

fn usage_context(agent_name: Option<&str>, provider: &str, model: &str) -> UsageContext {
    UsageContext {
        session_id: Some("usage-test-session".to_string()),
        thread_id: None,
        agent_run_id: None,
        run_id: None,
        provider: provider.to_string(),
        model: model.to_string(),
        request_kind: "chat".to_string(),
        intent: None,
        agent_name: agent_name.map(str::to_string),
    }
}

/// `libra usage --help` surfaces the EXAMPLES banner so users see the
/// canonical invocation per sub-command (`report` / `prune`) plus
/// common filter combinations (`--since`, `--session`, `--thread`,
/// `--include-failed`, csv format, `--retention-days`) without having
/// to read the design doc.
#[test]
fn test_usage_help_lists_examples_banner() {
    let repo = tempdir().expect("tempdir for usage --help");
    let output = run_libra_command(&["usage", "--help"], repo.path());
    assert!(
        output.status.success(),
        "usage --help should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "usage --help should include EXAMPLES banner, stdout: {stdout}"
    );
    for invocation in [
        "libra usage report",
        "libra usage report --by agent",
        "libra usage report --by agent-provider-model",
        "libra usage report --since 24h",
        "libra usage report --since 7d --include-failed",
        "libra usage report --session",
        "libra usage report --thread",
        "libra usage report --format csv",
        "libra usage --json report",
        "libra usage prune",
        "libra usage prune --retention-days 30",
    ] {
        assert!(
            stdout.contains(invocation),
            "usage --help should include `{invocation}`, stdout: {stdout}"
        );
    }
}

#[tokio::test]
async fn test_usage_report_by_agent_json_groups_agent_dimension() {
    let repo = tempdir().expect("tempdir for usage report --by agent");
    seed_usage_report_rows(repo.path()).await;

    let output = run_libra_command(&["--json", "usage", "report", "--by", "agent"], repo.path());
    assert_cli_success(&output, "usage report --by agent");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["by"], "agent");
    let rows = json["data"]["rows"].as_array().expect("rows array");
    let planner = rows
        .iter()
        .find(|row| row["agent_name"].as_str() == Some("planner"))
        .expect("planner aggregate row");

    assert_eq!(planner["request_count"], 2);
    assert_eq!(planner["provider"], "");
    assert_eq!(planner["model"], "");
}

#[tokio::test]
async fn test_usage_report_by_agent_human_and_csv_render_agent_columns() {
    let repo = tempdir().expect("tempdir for usage report --by agent render");
    seed_usage_report_rows(repo.path()).await;

    let human = run_libra_command(&["usage", "report", "--by", "agent"], repo.path());
    assert_cli_success(&human, "usage report --by agent human");
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(
        stdout.contains("planner\trequests=2"),
        "human output should include planner aggregate: {stdout}"
    );

    let csv = run_libra_command(
        &["usage", "report", "--by", "agent", "--format", "csv"],
        repo.path(),
    );
    assert_cli_success(&csv, "usage report --by agent csv");
    let stdout = String::from_utf8_lossy(&csv.stdout);
    assert!(
        stdout.starts_with("agent_name,requests,failed,"),
        "csv output should start with agent header: {stdout}"
    );
    assert!(
        stdout.contains("planner,2,0,200,100"),
        "csv output should include planner totals: {stdout}"
    );
}

#[tokio::test]
async fn test_usage_report_by_agent_provider_model_json_keeps_model_dimension() {
    let repo = tempdir().expect("tempdir for usage report --by agent-provider-model");
    seed_usage_report_rows(repo.path()).await;

    let output = run_libra_command(
        &["--json", "usage", "report", "--by", "agent-provider-model"],
        repo.path(),
    );
    assert_cli_success(&output, "usage report --by agent-provider-model");
    let json = parse_json_stdout(&output);
    assert_eq!(json["data"]["by"], "agent-provider-model");
    let rows = json["data"]["rows"].as_array().expect("rows array");
    let planner = rows
        .iter()
        .find(|row| {
            row["agent_name"].as_str() == Some("planner")
                && row["provider"].as_str() == Some("openai")
                && row["model"].as_str() == Some("gpt-4o")
        })
        .expect("planner/openai/gpt-4o aggregate row");

    assert_eq!(planner["request_count"], 2);
}
