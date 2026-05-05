//! Integration tests for the `2026050303_agent_capture` migration.
//!
//! See `docs/improvement/entire.md` (section 4.4) for the acceptance criteria
//! these tests pin: fresh-DB up, legacy-DB compatibility, and `up → down → up`
//! idempotency.

use libra::internal::db::migration::{MigrationRunner, builtin_migrations};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, ExecResult, Statement,
};
use tempfile::TempDir;

const LEGACY_BOOTSTRAP_SQL: &str = include_str!("../sql/sqlite_20260309_init.sql");

fn fresh_db_url() -> (TempDir, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("agent_capture.db");
    std::fs::File::create(&path).expect("touch sqlite file");
    let url = format!("sqlite://{}", path.display());
    (dir, url)
}

async fn connect(url: &str) -> DatabaseConnection {
    let mut opts = ConnectOptions::new(url.to_string());
    opts.sqlx_logging(false);
    Database::connect(opts).await.expect("connect")
}

async fn table_exists(conn: &DatabaseConnection, name: &str) -> bool {
    let backend = conn.get_database_backend();
    conn.query_one(Statement::from_sql_and_values(
        backend,
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1",
        [name.into()],
    ))
    .await
    .expect("query")
    .is_some()
}

async fn index_exists(conn: &DatabaseConnection, name: &str) -> bool {
    let backend = conn.get_database_backend();
    conn.query_one(Statement::from_sql_and_values(
        backend,
        "SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ? LIMIT 1",
        [name.into()],
    ))
    .await
    .expect("query")
    .is_some()
}

fn registered_runner() -> MigrationRunner {
    let mut runner = MigrationRunner::new();
    for migration in builtin_migrations() {
        runner
            .register(migration)
            .expect("builtin migrations must register clean");
    }
    runner
}

/// Replay the legacy bootstrap SQL the way `establish_connection` does on
/// first-time install. Statements are executed individually because the
/// driver only accepts one DDL per `execute` call.
async fn run_legacy_bootstrap(conn: &DatabaseConnection) {
    let backend = conn.get_database_backend();
    for raw in LEGACY_BOOTSTRAP_SQL.split(';') {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let _: ExecResult = conn
            .execute(Statement::from_string(backend, trimmed.to_string()))
            .await
            .unwrap_or_else(|e| panic!("legacy bootstrap stmt failed: {trimmed}\n{e}"));
    }
}

#[tokio::test]
async fn agent_capture_creates_tables_and_indexes() {
    let (_dir, url) = fresh_db_url();
    let conn = connect(&url).await;
    let runner = registered_runner();
    let applied = runner.run_pending(&conn).await.expect("run_pending");
    assert!(applied.contains(&2026050303));

    assert!(table_exists(&conn, "agent_session").await);
    assert!(table_exists(&conn, "agent_checkpoint").await);
    assert!(index_exists(&conn, "idx_agent_session_provider").await);
    assert!(index_exists(&conn, "idx_agent_session_active").await);
    assert!(index_exists(&conn, "idx_agent_session_thread").await);
    assert!(index_exists(&conn, "idx_agent_checkpoint_session").await);
    assert!(index_exists(&conn, "idx_agent_checkpoint_scope").await);
}

#[tokio::test]
async fn agent_capture_run_pending_is_idempotent() {
    let (_dir, url) = fresh_db_url();
    let conn = connect(&url).await;
    let runner = registered_runner();

    let first = runner.run_pending(&conn).await.expect("run_pending #1");
    assert!(first.contains(&2026050303));

    let second = runner.run_pending(&conn).await.expect("run_pending #2");
    assert!(
        second.is_empty(),
        "second run must apply no migrations; got {second:?}"
    );

    // Tables still present after the no-op pass.
    assert!(table_exists(&conn, "agent_session").await);
    assert!(table_exists(&conn, "agent_checkpoint").await);
}

#[tokio::test]
async fn agent_capture_rollback_drops_tables_and_indexes_only() {
    let (_dir, url) = fresh_db_url();
    let conn = connect(&url).await;
    let runner = registered_runner();
    runner.run_pending(&conn).await.expect("run_pending");

    let rolled_back = runner
        .rollback_to(&conn, 2026050302)
        .await
        .expect("rollback_to(2026050302)");
    assert_eq!(rolled_back, vec![2026050303]);

    // agent_capture artifacts gone.
    assert!(!table_exists(&conn, "agent_session").await);
    assert!(!table_exists(&conn, "agent_checkpoint").await);
    assert!(!index_exists(&conn, "idx_agent_session_provider").await);
    assert!(!index_exists(&conn, "idx_agent_session_active").await);
    assert!(!index_exists(&conn, "idx_agent_session_thread").await);
    assert!(!index_exists(&conn, "idx_agent_checkpoint_session").await);
    assert!(!index_exists(&conn, "idx_agent_checkpoint_scope").await);

    // Earlier migrations remain intact.
    assert!(table_exists(&conn, "automation_log").await);
    assert!(table_exists(&conn, "agent_usage_stats").await);
    assert_eq!(
        runner.current_version(&conn).await.unwrap(),
        Some(2026050302)
    );
}

#[tokio::test]
async fn agent_capture_up_down_up_round_trip() {
    let (_dir, url) = fresh_db_url();
    let conn = connect(&url).await;
    let runner = registered_runner();

    runner.run_pending(&conn).await.expect("up #1");
    runner
        .rollback_to(&conn, 2026050302)
        .await
        .expect("rollback");

    let applied_again = runner.run_pending(&conn).await.expect("up #2");
    assert!(applied_again.contains(&2026050303));
    assert!(table_exists(&conn, "agent_session").await);
    assert!(table_exists(&conn, "agent_checkpoint").await);
}

#[tokio::test]
async fn agent_capture_compatible_with_legacy_bootstrap() {
    let (_dir, url) = fresh_db_url();
    let conn = connect(&url).await;

    // Simulate a database that was first created by the legacy bootstrap
    // SQL — `run_pending` must apply cleanly on top of it.
    run_legacy_bootstrap(&conn).await;

    let runner = registered_runner();
    let applied = runner
        .run_pending(&conn)
        .await
        .expect("run_pending on legacy bootstrap");
    assert!(applied.contains(&2026050303));

    assert!(table_exists(&conn, "agent_session").await);
    assert!(table_exists(&conn, "agent_checkpoint").await);
}

/// Inserting a row whose `state` is outside the allowed set must fail because
/// the migration declares a CHECK constraint. This pins that the constraint
/// is applied — not silently dropped during DDL execution.
#[tokio::test]
async fn agent_capture_session_state_check_constraint_rejects_invalid() {
    let (_dir, url) = fresh_db_url();
    let conn = connect(&url).await;
    let runner = registered_runner();
    runner.run_pending(&conn).await.expect("run_pending");

    let backend = conn.get_database_backend();
    let res = conn
        .execute(Statement::from_string(
            backend,
            "INSERT INTO agent_session ( \
                session_id, agent_kind, provider_session_id, state, working_dir, \
                started_at, last_event_at \
             ) VALUES ('s1', 'claude_code', 'p1', 'bogus', '/', 0, 0)"
                .to_string(),
        ))
        .await;
    assert!(
        res.is_err(),
        "CHECK constraint on agent_session.state must reject 'bogus'"
    );
}
