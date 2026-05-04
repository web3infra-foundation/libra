use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement, Value};
use serde::Serialize;

#[derive(Clone)]
pub struct UsageQuery {
    conn: DatabaseConnection,
}

impl UsageQuery {
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

    pub async fn aggregate_by_model(&self) -> Result<Vec<UsageAggregate>, DbErr> {
        self.aggregate_by_model_filtered(&UsageQueryFilter::default())
            .await
    }

    pub async fn aggregate_by_model_filtered(
        &self,
        filter: &UsageQueryFilter,
    ) -> Result<Vec<UsageAggregate>, DbErr> {
        let backend = self.conn.get_database_backend();
        let (where_sql, values) = usage_where_clause(filter);
        let rows = self
            .conn
            .query_all(Statement::from_sql_and_values(
                backend,
                format!(
                    "SELECT provider, model, COUNT(*), \
                        COALESCE(SUM(prompt_tokens), 0), \
                        COALESCE(SUM(completion_tokens), 0), \
                        COALESCE(SUM(cached_tokens), 0), \
                        COALESCE(SUM(reasoning_tokens), 0), \
                        COALESCE(SUM(total_tokens), 0), \
                        COALESCE(SUM(tool_call_count), 0), \
                        COALESCE(SUM(wall_clock_ms), 0), \
                        SUM(cost_usd), \
                        SUM(cost_estimate_micro_dollars), \
                        COALESCE(SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END), 0) \
                 FROM agent_usage_stats \
                 {where_sql} \
                 GROUP BY provider, model \
                 ORDER BY provider, model"
                ),
                values,
            ))
            .await?;

        rows.into_iter().map(decode_aggregate).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageQueryFilter {
    pub since: Option<String>,
    pub until: Option<String>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub include_failed: bool,
}

impl Default for UsageQueryFilter {
    fn default() -> Self {
        Self {
            since: None,
            until: None,
            session_id: None,
            thread_id: None,
            include_failed: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UsageAggregate {
    pub provider: String,
    pub model: String,
    pub request_count: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
    pub tool_call_count: u64,
    pub wall_clock_ms: u64,
    pub cost_usd: Option<f64>,
    pub cost_estimate_micro_dollars: Option<u64>,
    pub failed_count: u64,
}

fn decode_aggregate(row: sea_orm::QueryResult) -> Result<UsageAggregate, DbErr> {
    let provider: String = row.try_get_by_index(0)?;
    let model: String = row.try_get_by_index(1)?;
    let request_count: i64 = row.try_get_by_index(2)?;
    let prompt_tokens: i64 = row.try_get_by_index(3)?;
    let completion_tokens: i64 = row.try_get_by_index(4)?;
    let cached_tokens: i64 = row.try_get_by_index(5)?;
    let reasoning_tokens: i64 = row.try_get_by_index(6)?;
    let total_tokens: i64 = row.try_get_by_index(7)?;
    let tool_call_count: i64 = row.try_get_by_index(8)?;
    let wall_clock_ms: i64 = row.try_get_by_index(9)?;
    let cost_usd: Option<f64> = row.try_get_by_index(10)?;
    let cost_estimate_micro_dollars: Option<i64> = row.try_get_by_index(11)?;
    let failed_count: i64 = row.try_get_by_index(12)?;

    Ok(UsageAggregate {
        provider,
        model,
        request_count: non_negative_u64(request_count),
        prompt_tokens: non_negative_u64(prompt_tokens),
        completion_tokens: non_negative_u64(completion_tokens),
        cached_tokens: non_negative_u64(cached_tokens),
        reasoning_tokens: non_negative_u64(reasoning_tokens),
        total_tokens: non_negative_u64(total_tokens),
        tool_call_count: non_negative_u64(tool_call_count),
        wall_clock_ms: non_negative_u64(wall_clock_ms),
        cost_usd,
        cost_estimate_micro_dollars: cost_estimate_micro_dollars.map(non_negative_u64),
        failed_count: non_negative_u64(failed_count),
    })
}

fn non_negative_u64(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(0)
}

fn usage_where_clause(filter: &UsageQueryFilter) -> (String, Vec<Value>) {
    let mut clauses = Vec::new();
    let mut values = Vec::new();

    if let Some(since) = filter.since.as_ref() {
        clauses.push("COALESCE(started_at, created_at) >= ?");
        values.push(since.clone().into());
    }
    if let Some(until) = filter.until.as_ref() {
        clauses.push("COALESCE(started_at, created_at) <= ?");
        values.push(until.clone().into());
    }
    if let Some(session_id) = filter.session_id.as_ref() {
        clauses.push("session_id = ?");
        values.push(session_id.clone().into());
    }
    if let Some(thread_id) = filter.thread_id.as_ref() {
        clauses.push("thread_id = ?");
        values.push(thread_id.clone().into());
    }
    if !filter.include_failed {
        clauses.push("success = 1");
    }

    if clauses.is_empty() {
        (String::new(), values)
    } else {
        (format!("WHERE {}", clauses.join(" AND ")), values)
    }
}
