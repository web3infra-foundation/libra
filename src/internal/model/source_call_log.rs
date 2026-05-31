//! SeaORM entity for the [`source_call_log`](../../../../sql/migrations/2026052301_source_call_log.sql)
//! table — persistent telemetry of every external Source / MCP /
//! OpenAPI call routed through [`SourcePool`].
//!
//! [`source_call_log`] 表的 SeaORM 实体 - 通过 [`SourcePool`] 路由的每个外部源/MCP/OpenAPI 调用的持久遥测。
//!
//! Migrations land via `sql/migrations/2026052301_source_call_log{,_down}.sql`
//! and the entity is registered in [`crate::internal::model`].
//!
//! Producer is [`crate::internal::ai::sources::SourceCallLog`] (today
//! the in-memory `Mutex<Vec<SourceCallRecord>>` shape from v0.16.x);
//! v0.17.800 adds the on-disk shape so a session crash no longer
//! drops the audit trail.

use sea_orm::entity::prelude::*;

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
