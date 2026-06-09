//! SeaORM entity for the [`source_call_log`](../../../../sql/migrations/2026052301_source_call_log.sql)
//! table — persistent telemetry of every external Source / MCP /
//! OpenAPI call routed through [`SourcePool`].
//!
//! Migrations land via `sql/migrations/2026052301_source_call_log{,_down}.sql`
//! and the entity is registered in [`crate::internal::model`].
//!
//! Producer is [`crate::internal::ai::sources::SourceCallLog`] (today
//! the in-memory `Mutex<Vec<SourceCallRecord>>` shape from v0.16.x);
//! v0.17.800 adds the on-disk shape so a session crash no longer
//! drops the audit trail.

use std::collections::HashMap;

use sea_orm::{ConnectionTrait, Statement, entity::prelude::*};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "source_call_log")]
pub struct Model {
    /// UUID-shaped primary key minted at row creation. Distinct from
    /// `tool_call_id` so the same tool call can produce multiple
    /// retry rows (each with its own primary key).
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// Session that issued the source call. Indexed for
    /// `libra usage report --by=source` future query.
    pub session_id: String,
    /// `SourcePool` slug ("mcp:git-tools", "openapi:weather", etc.).
    pub source_slug: String,
    /// Public tool name the caller asked for (post-prefix).
    pub tool_name: String,
    /// Internal registered name (pre-prefix). Same as `tool_name`
    /// for sources that don't apply a prefix; distinct for sources
    /// whose CapabilityManifest renames their exported tools.
    pub registered_tool_name: String,
    /// Caller's tool call id (typically the LLM-supplied
    /// `tool_call_xyz` token). Indexed for cross-row lookup.
    pub tool_call_id: String,
    /// Owning sub-agent run id (CEX-S2-14 trace chain) when the call came from a
    /// sub-agent's tool loop; NULL for main-session source calls. Indexed for
    /// the `thread → agent_run_id → tool_call_id → source_call` trace query.
    pub agent_run_id: Option<String>,
    /// Optional vault/env reference for the credential used.
    pub credential_ref: Option<String>,
    /// Round-trip latency in milliseconds. None when the call
    /// short-circuited (denied by manifest, schema rejection, etc.).
    pub latency_ms: Option<i64>,
    /// Bytes sent (request body + arguments).
    pub input_bytes: i64,
    /// Bytes received (response body).
    pub output_bytes: i64,
    /// Estimated cost in micro-dollars. None when the source has
    /// no pricing model.
    pub cost_estimate_micros: Option<i64>,
    /// Approval decision string: "auto" / "human-once" /
    /// "human-always" / "denied". None when no approval gate fired.
    pub approval_decision: Option<String>,
    /// Per-source state namespace key used to isolate writable
    /// state between sources (today: `source_slug` itself; future
    /// per-tenant namespaces would extend this).
    pub state_namespace: String,
    /// 1 for success, 0 for failure. SQLite booleans land as
    /// integers; SeaORM round-trips them through i64.
    pub success: i64,
    /// RFC3339 timestamp at row creation.
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

/// Count persisted Source Pool / MCP / OpenAPI calls grouped by the owning
/// sub-agent run (CEX-S2-16 agent-pane "source calls" column). Rows whose
/// `agent_run_id` is NULL — main-session calls not attributed to a sub-agent
/// run — are excluded. The map is keyed by exactly the string the trace chain
/// wrote (`AgentRunId.0.to_string()`, v0.17.1254) so a caller can look up a
/// run's count by `run.id.0.to_string()`.
///
/// Read-only and side-effect free; an empty table yields an empty map.
pub async fn count_by_agent_run<C: ConnectionTrait>(
    conn: &C,
) -> Result<HashMap<String, u64>, DbErr> {
    let backend = conn.get_database_backend();
    let rows = conn
        .query_all(Statement::from_string(
            backend,
            "SELECT agent_run_id, COUNT(*) AS cnt \
             FROM source_call_log \
             WHERE agent_run_id IS NOT NULL \
             GROUP BY agent_run_id"
                .to_string(),
        ))
        .await?;

    let mut counts = HashMap::with_capacity(rows.len());
    for row in rows {
        let run_id: String = row.try_get_by_index(0)?;
        // SQLite COUNT(*) returns an i64; clamp defensively before the cast.
        let count: i64 = row.try_get_by_index(1)?;
        counts.insert(run_id, count.max(0) as u64);
    }
    Ok(counts)
}

/// Count persisted source calls for a single sub-agent run (CEX-S2-16 MCP
/// `libra://agents/runs/{id}/budget` `source_call_count`). `agent_run_id` is the
/// `AgentRunId.0.to_string()` the trace chain wrote (v0.17.1254). Returns 0 when
/// no rows match. Read-only.
pub async fn count_for_run<C: ConnectionTrait>(conn: &C, agent_run_id: &str) -> Result<u64, DbErr> {
    let backend = conn.get_database_backend();
    let row = conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT COUNT(*) FROM source_call_log WHERE agent_run_id = ?",
            [sea_orm::Value::from(agent_run_id.to_string())],
        ))
        .await?;
    match row {
        // SQLite COUNT(*) returns an i64; clamp defensively before the cast.
        Some(row) => Ok(row.try_get_by_index::<i64>(0)?.max(0) as u64),
        None => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, Database};

    use super::*;
    use crate::internal::db::migration::run_builtin_migrations;

    fn row(id: usize, agent_run_id: Option<&str>) -> ActiveModel {
        ActiveModel {
            id: Set(format!("call-{id}")),
            session_id: Set("sess".to_string()),
            source_slug: Set("mcp:git".to_string()),
            tool_name: Set("status".to_string()),
            registered_tool_name: Set("status".to_string()),
            tool_call_id: Set(format!("tc-{id}")),
            agent_run_id: Set(agent_run_id.map(str::to_string)),
            credential_ref: Set(None),
            latency_ms: Set(None),
            input_bytes: Set(0),
            output_bytes: Set(0),
            cost_estimate_micros: Set(None),
            approval_decision: Set(None),
            state_namespace: Set("mcp:git".to_string()),
            success: Set(1),
            created_at: Set("2026-06-02T00:00:00Z".to_string()),
        }
    }

    #[tokio::test]
    async fn count_by_agent_run_groups_per_run_and_skips_null() {
        let conn = Database::connect("sqlite::memory:").await.expect("db");
        run_builtin_migrations(&conn).await.expect("migrations");

        // Two calls for run-A, one for run-B, one main-session (NULL run).
        for (i, run) in [Some("run-A"), Some("run-A"), Some("run-B"), None]
            .into_iter()
            .enumerate()
        {
            row(i, run).insert(&conn).await.expect("insert");
        }

        let counts = count_by_agent_run(&conn).await.expect("count");
        assert_eq!(counts.get("run-A").copied(), Some(2));
        assert_eq!(counts.get("run-B").copied(), Some(1));
        assert_eq!(
            counts.len(),
            2,
            "rows with a NULL agent_run_id (main-session calls) are excluded",
        );
    }

    #[tokio::test]
    async fn count_by_agent_run_empty_table_yields_empty_map() {
        let conn = Database::connect("sqlite::memory:").await.expect("db");
        run_builtin_migrations(&conn).await.expect("migrations");
        let counts = count_by_agent_run(&conn).await.expect("count");
        assert!(counts.is_empty());
    }

    #[tokio::test]
    async fn count_for_run_counts_only_the_targeted_run() {
        let conn = Database::connect("sqlite::memory:").await.expect("db");
        run_builtin_migrations(&conn).await.expect("migrations");

        for (i, run) in [Some("run-A"), Some("run-A"), Some("run-B"), None]
            .into_iter()
            .enumerate()
        {
            row(i, run).insert(&conn).await.expect("insert");
        }

        assert_eq!(count_for_run(&conn, "run-A").await.expect("count"), 2);
        assert_eq!(count_for_run(&conn, "run-B").await.expect("count"), 1);
        // An unknown run id counts zero, never errors.
        assert_eq!(count_for_run(&conn, "run-Z").await.expect("count"), 0);
    }
}
