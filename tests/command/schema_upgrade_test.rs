//! CLI coverage for repository database schema upgrades.

use std::{path::Path, time::Duration};

use libra::internal::db::migration::builtin_runner;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, Statement};
use tempfile::tempdir;

use super::{assert_cli_success, init_repo_via_cli, parse_cli_error_stderr, run_libra_command};

async fn connect_raw_repo_db(repo: &Path) -> DatabaseConnection {
    let db_path = repo.join(".libra").join("libra.db");
    let mut opts = ConnectOptions::new(format!("sqlite://{}", db_path.display()));
    opts.sqlx_logging(false)
        .connect_timeout(Duration::from_secs(5));
    Database::connect(opts)
        .await
        .expect("connect raw repository database")
}

async fn stale_repo_at_approved_permission() -> tempfile::TempDir {
    let repo = tempdir().expect("create repository root");
    init_repo_via_cli(repo.path());

    let conn = connect_raw_repo_db(repo.path()).await;
    let runner = builtin_runner().expect("built-in migration registry");
    runner
        .rollback_to(&conn, 2026050601)
        .await
        .expect("roll back latest migration");
    conn.close().await.expect("close raw connection");
    repo
}

async fn max_schema_version(conn: &DatabaseConnection) -> Option<i64> {
    let row = conn
        .query_one(Statement::from_string(
            conn.get_database_backend(),
            "SELECT MAX(version) FROM schema_versions",
        ))
        .await
        .expect("query schema version")
        .expect("schema version row");
    row.try_get_by_index(0).expect("decode schema version")
}

async fn index_exists(conn: &DatabaseConnection, name: &str) -> bool {
    conn.query_one(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "SELECT 1 FROM sqlite_master WHERE type = ? AND name = ? LIMIT 1",
        ["index".into(), name.into()],
    ))
    .await
    .expect("query sqlite_master")
    .is_some()
}

async fn column_exists(conn: &DatabaseConnection, table: &str, column: &str) -> bool {
    let escaped_table = table.replace('`', "``");
    let rows = conn
        .query_all(Statement::from_string(
            conn.get_database_backend(),
            format!("PRAGMA table_info(`{escaped_table}`)"),
        ))
        .await
        .expect("query table_info");
    rows.iter().any(|row| {
        let name: String = row.try_get_by_index(1).expect("column name");
        name == column
    })
}

#[tokio::test]
async fn db_upgrade_applies_pending_builtin_migrations_to_stale_repo() {
    let repo = stale_repo_at_approved_permission().await;

    let output = run_libra_command(&["db", "upgrade"], repo.path());
    assert_cli_success(&output, "libra db upgrade");

    let conn = connect_raw_repo_db(repo.path()).await;
    let latest = builtin_runner()
        .expect("built-in migration registry")
        .max_registered_version();
    assert_eq!(max_schema_version(&conn).await, latest);
    assert!(
        column_exists(&conn, "agent_usage_stats", "agent_name").await,
        "db upgrade should apply the agent_name migration"
    );
    assert!(
        index_exists(&conn, "idx_agent_usage_stats_agent_name_provider_model").await,
        "db upgrade should recreate the agent_name/provider/model index"
    );
}

#[tokio::test]
async fn normal_command_refuses_stale_schema_and_does_not_upgrade() {
    let repo = stale_repo_at_approved_permission().await;

    let output = run_libra_command(&["status"], repo.path());
    assert_eq!(output.status.code(), Some(128));
    let (stderr, report) = parse_cli_error_stderr(&output.stderr);
    assert_eq!(report.error_code, "LBR-REPO-002");
    assert!(
        stderr.contains("libra db upgrade")
            || report
                .hints
                .iter()
                .any(|hint| hint.contains("libra db upgrade")),
        "stale schema error should point at db upgrade, stderr: {stderr}, hints: {:?}",
        report.hints
    );

    let conn = connect_raw_repo_db(repo.path()).await;
    assert_eq!(max_schema_version(&conn).await, Some(2026050601));
    assert!(
        !column_exists(&conn, "agent_usage_stats", "agent_name").await,
        "normal command preflight must not apply the pending migration"
    );
    assert!(
        !index_exists(&conn, "idx_agent_usage_stats_agent_name_provider_model").await,
        "normal command preflight must not recreate pending indexes"
    );
}
