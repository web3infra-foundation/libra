//! CEX-16 usage stats persistence and aggregation tests.

use libra::internal::{
    ai::{
        completion::CompletionUsageSummary,
        usage::{UsageContext, UsageQuery, UsageQueryFilter, UsageRecorder},
    },
    db::migration::run_builtin_migrations,
};
use sea_orm::{ConnectionTrait, Database, Statement};

fn usage_context(provider: &str, model: &str) -> UsageContext {
    UsageContext {
        session_id: Some("session-1".to_string()),
        thread_id: Some("thread-1".to_string()),
        agent_run_id: None,
        run_id: Some("run-1".to_string()),
        provider: provider.to_string(),
        model: model.to_string(),
        request_kind: "completion".to_string(),
        intent: Some("feature".to_string()),
    }
}

#[tokio::test]
async fn usage_recorder_persists_and_aggregates_by_model() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("openai", "gpt-test");

    recorder
        .record_summary_with_tool_count(
            &context,
            &CompletionUsageSummary {
                input_tokens: 10,
                output_tokens: 5,
                cached_tokens: Some(2),
                reasoning_tokens: Some(1),
                total_tokens: Some(16),
                cost_usd: Some(0.25),
            },
            Some(1200),
            2,
        )
        .await
        .expect("record first usage");
    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 7,
                output_tokens: 3,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(10),
                cost_usd: None,
            },
            Some(800),
        )
        .await
        .expect("record second usage");

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate usage");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].provider, "openai");
    assert_eq!(rows[0].model, "gpt-test");
    assert_eq!(rows[0].request_count, 2);
    assert_eq!(rows[0].prompt_tokens, 17);
    assert_eq!(rows[0].completion_tokens, 8);
    assert_eq!(rows[0].cached_tokens, 2);
    assert_eq!(rows[0].reasoning_tokens, 1);
    assert_eq!(rows[0].total_tokens, 26);
    assert_eq!(rows[0].tool_call_count, 2);
    assert_eq!(rows[0].wall_clock_ms, 2000);
    assert_eq!(rows[0].cost_usd, Some(0.25));
    assert_eq!(rows[0].cost_estimate_micro_dollars, Some(250_000));
    assert_eq!(rows[0].failed_count, 0);
}

#[tokio::test]
async fn usage_recorder_ignores_absent_or_zero_usage() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("ollama", "local-test");

    recorder
        .record_optional_summary(&context, None, Some(40))
        .await
        .expect("missing usage is tolerated");
    recorder
        .record_summary(&context, &CompletionUsageSummary::default(), Some(40))
        .await
        .expect("zero usage is tolerated");

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate usage");
    assert!(rows.is_empty());
}

#[tokio::test]
async fn usage_recorder_records_missing_usage_and_failures() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("gemini", "gemini-test");

    recorder
        .record_missing_usage(&context, Some(250), 1)
        .await
        .expect("record estimated zero-token usage");
    recorder
        .record_failure(&context, "provider_error", Some(750))
        .await
        .expect("record failed usage");

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate usage");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].request_count, 2);
    assert_eq!(rows[0].total_tokens, 0);
    assert_eq!(rows[0].tool_call_count, 1);
    assert_eq!(rows[0].wall_clock_ms, 1000);
    assert_eq!(rows[0].failed_count, 1);
}

#[tokio::test]
async fn usage_query_filter_excludes_failures_by_default_when_requested() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("openai", "gpt-test");

    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 4,
                output_tokens: 6,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(10),
                cost_usd: None,
            },
            Some(100),
        )
        .await
        .expect("record success");
    recorder
        .record_failure(&context, "provider_error", Some(900))
        .await
        .expect("record failure");

    let success_rows = UsageQuery::new(conn.clone())
        .aggregate_by_model_filtered(&UsageQueryFilter {
            include_failed: false,
            ..UsageQueryFilter::default()
        })
        .await
        .expect("aggregate successes");
    assert_eq!(success_rows[0].request_count, 1);
    assert_eq!(success_rows[0].failed_count, 0);
    assert_eq!(success_rows[0].wall_clock_ms, 100);

    let all_rows = UsageQuery::new(conn)
        .aggregate_by_model_filtered(&UsageQueryFilter {
            include_failed: true,
            ..UsageQueryFilter::default()
        })
        .await
        .expect("aggregate all rows");
    assert_eq!(all_rows[0].request_count, 2);
    assert_eq!(all_rows[0].failed_count, 1);
    assert_eq!(all_rows[0].wall_clock_ms, 1000);
}

#[tokio::test]
async fn usage_recorder_prunes_rows_before_cutoff() {
    let conn = Database::connect("sqlite::memory:")
        .await
        .expect("connect sqlite");
    run_builtin_migrations(&conn).await.expect("run migrations");
    let recorder = UsageRecorder::new(conn.clone());
    let context = usage_context("openai", "gpt-test");

    recorder
        .record_summary(
            &context,
            &CompletionUsageSummary {
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: None,
                reasoning_tokens: None,
                total_tokens: Some(2),
                cost_usd: None,
            },
            Some(10),
        )
        .await
        .expect("record usage");

    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE agent_usage_stats SET started_at = ?, created_at = ?",
        vec![
            "2020-01-01T00:00:00+00:00".into(),
            "2020-01-01T00:00:00+00:00".into(),
        ],
    ))
    .await
    .expect("age usage row");

    let deleted = recorder
        .prune_before("2021-01-01T00:00:00+00:00")
        .await
        .expect("prune old rows");
    assert_eq!(deleted, 1);

    let rows = UsageQuery::new(conn)
        .aggregate_by_model()
        .await
        .expect("aggregate usage");
    assert!(rows.is_empty());
}
