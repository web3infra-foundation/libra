use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement, Value};
use uuid::Uuid;

use crate::internal::ai::completion::CompletionUsageSummary;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageContext {
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub agent_run_id: Option<String>,
    pub run_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub request_kind: String,
    pub intent: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UsageRecorder {
    conn: DatabaseConnection,
}

impl UsageRecorder {
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

    pub async fn record_optional_summary(
        &self,
        context: &UsageContext,
        summary: Option<&CompletionUsageSummary>,
        wall_clock_ms: Option<u64>,
    ) -> Result<(), DbErr> {
        let Some(summary) = summary else {
            return Ok(());
        };
        self.record_summary(context, summary, wall_clock_ms).await
    }

    pub async fn record_summary(
        &self,
        context: &UsageContext,
        summary: &CompletionUsageSummary,
        wall_clock_ms: Option<u64>,
    ) -> Result<(), DbErr> {
        self.record_summary_with_tool_count(context, summary, wall_clock_ms, 0)
            .await
    }

    pub async fn record_summary_with_tool_count(
        &self,
        context: &UsageContext,
        summary: &CompletionUsageSummary,
        wall_clock_ms: Option<u64>,
        tool_call_count: u64,
    ) -> Result<(), DbErr> {
        if summary.is_zero() {
            return Ok(());
        }
        self.insert_row(UsageInsert {
            context,
            summary: Some(summary),
            wall_clock_ms: wall_clock_ms.unwrap_or(0),
            tool_call_count,
            usage_estimated: false,
            success: true,
            error_kind: None,
        })
        .await
    }

    pub async fn record_missing_usage(
        &self,
        context: &UsageContext,
        wall_clock_ms: Option<u64>,
        tool_call_count: u64,
    ) -> Result<(), DbErr> {
        self.insert_row(UsageInsert {
            context,
            summary: None,
            wall_clock_ms: wall_clock_ms.unwrap_or(0),
            tool_call_count,
            usage_estimated: true,
            success: true,
            error_kind: None,
        })
        .await
    }

    pub async fn record_failure(
        &self,
        context: &UsageContext,
        error_kind: &str,
        wall_clock_ms: Option<u64>,
    ) -> Result<(), DbErr> {
        self.insert_row(UsageInsert {
            context,
            summary: None,
            wall_clock_ms: wall_clock_ms.unwrap_or(0),
            tool_call_count: 0,
            usage_estimated: false,
            success: false,
            error_kind: Some(error_kind),
        })
        .await
    }

    pub async fn prune_before(&self, cutoff_rfc3339: &str) -> Result<u64, DbErr> {
        let backend = self.conn.get_database_backend();
        let result = self
            .conn
            .execute(Statement::from_sql_and_values(
                backend,
                "DELETE FROM agent_usage_stats \
                 WHERE COALESCE(started_at, created_at) < ?",
                vec![cutoff_rfc3339.to_string().into()],
            ))
            .await?;
        Ok(result.rows_affected())
    }

    async fn insert_row(&self, input: UsageInsert<'_>) -> Result<(), DbErr> {
        let now = Utc::now().to_rfc3339();
        let summary = input.summary.cloned().unwrap_or_default();
        let total_tokens = summary.total_tokens.unwrap_or_else(|| {
            summary
                .input_tokens
                .saturating_add(summary.output_tokens)
                .saturating_add(summary.reasoning_tokens.unwrap_or(0))
        });
        let cost_micro_dollars = cost_micro_dollars(summary.cost_usd);
        let backend = self.conn.get_database_backend();
        self.conn
            .execute(Statement::from_sql_and_values(
                backend,
                "INSERT INTO agent_usage_stats \
                 (id, session_id, thread_id, agent_run_id, run_id, provider, model, request_kind, intent, prompt_tokens, completion_tokens, cached_tokens, reasoning_tokens, total_tokens, tool_call_count, wall_clock_ms, provider_latency_ms, cost_estimate_micro_dollars, cost_usd, usage_estimated, started_at, finished_at, success, error_kind, schema_version, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                vec![
                    Uuid::new_v4().to_string().into(),
                    input.context.session_id.clone().into(),
                    input.context.thread_id.clone().into(),
                    input.context.agent_run_id.clone().into(),
                    input.context.run_id.clone().into(),
                    input.context.provider.clone().into(),
                    input.context.model.clone().into(),
                    input.context.request_kind.clone().into(),
                    input.context.intent.clone().into(),
                    u64_to_i64_value(summary.input_tokens),
                    u64_to_i64_value(summary.output_tokens),
                    u64_to_i64_value(summary.cached_tokens.unwrap_or(0)),
                    u64_to_i64_value(summary.reasoning_tokens.unwrap_or(0)),
                    u64_to_i64_value(total_tokens),
                    u64_to_i64_value(input.tool_call_count),
                    u64_to_i64_value(input.wall_clock_ms),
                    Value::BigInt(None),
                    optional_i64_value(cost_micro_dollars),
                    summary.cost_usd.into(),
                    bool_to_i64_value(input.usage_estimated),
                    now.clone().into(),
                    now.clone().into(),
                    bool_to_i64_value(input.success),
                    input.error_kind.map(str::to_string).into(),
                    1_i64.into(),
                    now.into(),
                ],
            ))
            .await?;
        Ok(())
    }
}

struct UsageInsert<'a> {
    context: &'a UsageContext,
    summary: Option<&'a CompletionUsageSummary>,
    wall_clock_ms: u64,
    tool_call_count: u64,
    usage_estimated: bool,
    success: bool,
    error_kind: Option<&'a str>,
}

fn u64_to_i64_value(value: u64) -> Value {
    i64::try_from(value).unwrap_or(i64::MAX).into()
}

fn optional_i64_value(value: Option<i64>) -> Value {
    value.into()
}

fn bool_to_i64_value(value: bool) -> Value {
    i64::from(value).into()
}

fn cost_micro_dollars(cost_usd: Option<f64>) -> Option<i64> {
    let cost = cost_usd?;
    if !cost.is_finite() || cost < 0.0 {
        return None;
    }
    let micro_dollars = cost * 1_000_000.0;
    if micro_dollars > i64::MAX as f64 {
        None
    } else {
        Some(micro_dollars.round() as i64)
    }
}
