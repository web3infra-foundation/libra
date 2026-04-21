//! Phase 0 schema migration tests for AI runtime contract tables.

use libra::internal::db::ensure_ai_runtime_contract_schema;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};

const BOOTSTRAP_SQL: &str = include_str!("../sql/sqlite_20260309_init.sql");

async fn table_exists(db: &DatabaseConnection, table: &str) -> bool {
    let stmt = Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1",
        [table.into()],
    );
    db.query_one(stmt).await.unwrap().is_some()
}

async fn index_exists(db: &DatabaseConnection, index: &str) -> bool {
    let stmt = Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ? LIMIT 1",
        [index.into()],
    );
    db.query_one(stmt).await.unwrap().is_some()
}

#[tokio::test]
async fn fresh_bootstrap_contains_phase0_runtime_contract_tables() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    db.execute(Statement::from_string(
        db.get_database_backend(),
        BOOTSTRAP_SQL,
    ))
    .await
    .unwrap();

    for table in [
        "ai_scheduler_selected_plan",
        "ai_validation_report",
        "ai_risk_score_breakdown",
        "ai_decision_proposal",
        "ai_thread_provider_metadata",
    ] {
        assert!(table_exists(&db, table).await, "missing table {table}");
    }

    assert!(
        index_exists(&db, "idx_ai_scheduler_selected_plan_thread_ordinal").await,
        "missing selected plan ordinal index"
    );
}

#[tokio::test]
async fn deployed_db_runtime_contract_migration_is_idempotent() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    let backend = db.get_database_backend();
    db.execute(Statement::from_string(
        backend,
        r#"
CREATE TABLE object_index (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    o_id TEXT NOT NULL,
    o_type TEXT NOT NULL,
    o_size INTEGER NOT NULL,
    repo_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    is_synced INTEGER DEFAULT 0
);
CREATE TABLE ai_thread (
    thread_id TEXT PRIMARY KEY,
    owner_kind TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE ai_scheduler_state (
    thread_id TEXT PRIMARY KEY,
    selected_plan_id TEXT,
    active_task_id TEXT,
    active_run_id TEXT,
    metadata_json TEXT,
    version INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);
"#,
    ))
    .await
    .unwrap();

    ensure_ai_runtime_contract_schema(&db).await.unwrap();
    ensure_ai_runtime_contract_schema(&db).await.unwrap();

    assert!(table_exists(&db, "ai_scheduler_selected_plan").await);
    assert!(table_exists(&db, "ai_validation_report").await);
    assert!(table_exists(&db, "ai_thread_provider_metadata").await);
    assert!(index_exists(&db, "idx_ai_validation_report_latest").await);
}
