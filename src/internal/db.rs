//! Database utilities for establishing SQLite connections, managing per-test connection pools, creating schemas, and exposing pooled handles.

use std::{
    io,
    io::{Error as IOError, ErrorKind},
    path::Path,
};

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbConn, DbErr, Schema,
    Statement, TransactionError, TransactionTrait,
};

use crate::{internal::model::*, utils::path};

// #[cfg(not(test))]
// use tokio::sync::OnceCell;

/// Establish a connection to the database.
///  - `db_path` is the path to the SQLite database file.
/// - Returns a `DatabaseConnection` if successful, or an `IOError` if the database file does not exist.
#[allow(dead_code)]
pub async fn establish_connection(db_path: &str) -> Result<DatabaseConnection, IOError> {
    if !Path::new(db_path).exists() {
        return Err(IOError::new(
            ErrorKind::NotFound,
            "Database file does not exist.",
        ));
    }

    let conn = connect_database(db_path).await?;
    ensure_ai_projection_schema(&conn)
        .await
        .map_err(|err| IOError::other(format!("Failed to ensure AI projection schema: {err}")))?;
    Ok(conn)
}
// #[cfg(not(test))]
// static DB_CONN: OnceCell<DbConn> = OnceCell::const_new();

// /// Get global database connection instance (singleton)
// #[cfg(not(test))]
// pub async fn get_db_conn_instance() -> &'static DbConn {
//     DB_CONN
//         .get_or_init(|| async { get_db_conn().await.unwrap() })
//         .await
// }

// #[cfg(test)]
// #[cfg(test)]
use std::collections::HashMap;
//#[cfg(test)]
//use std::ops::Deref;
// #[cfg(test)]
use std::path::PathBuf;

use once_cell::sync::Lazy;
// #[cfg(test)]
use tokio::sync::Mutex;

// Shared SQLite connections cached by database path.
static TEST_DB_CONNECTIONS: Lazy<Mutex<HashMap<PathBuf, DbConn>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

async fn get_or_init_db_conn_instance(db_path: PathBuf) -> io::Result<DbConn> {
    let mut connections = TEST_DB_CONNECTIONS.lock().await;

    if !db_path.exists() {
        connections.remove(&db_path);
        return Err(IOError::new(
            ErrorKind::NotFound,
            format!("Database file does not exist: {}", db_path.display()),
        ));
    }

    if let Some(conn) = connections.get(&db_path) {
        return Ok(conn.clone());
    }
    drop(connections);

    let conn = get_db_conn_for_path(&db_path).await?;

    let mut connections = TEST_DB_CONNECTIONS.lock().await;
    if let Some(existing) = connections.get(&db_path) {
        return Ok(existing.clone());
    }
    connections.insert(db_path, conn.clone());
    Ok(conn)
}

/// Get global database connection instance (singleton per SQLite file).
///
/// TODO(error): migrate legacy call sites to `get_db_conn_instance_for_path`
/// and make this convenience wrapper return `io::Result` instead of panicking.
pub async fn get_db_conn_instance() -> DbConn {
    let db_path = path::database();
    get_db_conn_instance_for_path(&db_path)
        .await
        .unwrap_or_else(|err| panic!("Failed to open database {}: {}", db_path.display(), err))
}

/// Get a shared database connection instance for an explicit SQLite file path.
pub async fn get_db_conn_instance_for_path(db_path: &Path) -> io::Result<DbConn> {
    get_or_init_db_conn_instance(db_path.to_path_buf()).await
}

/// Drop a cached shared connection for an explicit SQLite file path.
pub async fn reset_db_conn_instance_for_path(db_path: &Path) {
    let mut connections = TEST_DB_CONNECTIONS.lock().await;
    let removed = connections.remove(db_path);
    drop(connections);

    if let Some(conn) = removed
        && let Err(err) = conn.close().await
    {
        tracing::warn!(
            db_path = %db_path.display(),
            error = %err,
            "Failed to close cached database connection during reset"
        );
    }
}

async fn get_db_conn_for_path(db_path: &Path) -> io::Result<DatabaseConnection> {
    let db_path = db_path.to_str().ok_or_else(|| {
        IOError::new(
            ErrorKind::InvalidData,
            format!("Database path is not valid UTF-8: {}", db_path.display()),
        )
    })?;
    establish_connection(db_path).await
}

/// create table according to the Model
#[deprecated]
#[allow(dead_code)]
async fn setup_database_model(conn: &DatabaseConnection) -> Result<(), TransactionError<DbErr>> {
    // start a transaction
    conn.transaction::<_, _, DbErr>(|txn| {
        Box::pin(async move {
            let backend = txn.get_database_backend();
            let schema = Schema::new(backend);

            // reference table
            let table_create_statement = schema.create_table_from_entity(reference::Entity);
            txn.execute(backend.build(&table_create_statement)).await?;

            // config_section table
            let table_create_statement = schema.create_table_from_entity(config::Entity);
            txn.execute(backend.build(&table_create_statement)).await?;

            Ok(())
        })
    })
    .await
}

const BOOTSTRAP_SQL: &str = include_str!("../../sql/sqlite_20260309_init.sql");
const AI_PROJECTION_SCHEMA_START: &str = "-- BEGIN AI PROJECTION SCHEMA";
const AI_PROJECTION_SCHEMA_END: &str = "-- END AI PROJECTION SCHEMA";

/// create table using the SQLite bootstrap schema
async fn setup_database_sql(conn: &DatabaseConnection) -> Result<(), TransactionError<DbErr>> {
    conn.transaction::<_, _, DbErr>(|txn| {
        Box::pin(async move {
            let backend = txn.get_database_backend();

            // `include_str!` will expand the file while compiling, so `.sql` is not needed after that
            txn.execute(Statement::from_string(backend, BOOTSTRAP_SQL))
                .await?;
            Ok(())
        })
    })
    .await
}

fn ai_projection_sql() -> io::Result<&'static str> {
    let start = BOOTSTRAP_SQL
        .find(AI_PROJECTION_SCHEMA_START)
        .ok_or_else(|| {
            IOError::new(
                ErrorKind::InvalidData,
                format!("Bootstrap schema is missing marker: {AI_PROJECTION_SCHEMA_START}"),
            )
        })?;
    let start = start + AI_PROJECTION_SCHEMA_START.len();
    let end = BOOTSTRAP_SQL[start..]
        .find(AI_PROJECTION_SCHEMA_END)
        .ok_or_else(|| {
            IOError::new(
                ErrorKind::InvalidData,
                format!("Bootstrap schema is missing marker: {AI_PROJECTION_SCHEMA_END}"),
            )
        })?;
    let sql = BOOTSTRAP_SQL[start..start + end].trim();
    if sql.is_empty() {
        return Err(IOError::new(
            ErrorKind::InvalidData,
            "Bootstrap schema AI projection section is empty.",
        ));
    }

    Ok(sql)
}

async fn sqlite_schema_contains(
    conn: &DatabaseConnection,
    entry_type: &str,
    name: &str,
) -> Result<bool, DbErr> {
    let backend = conn.get_database_backend();
    let stmt = Statement::from_sql_and_values(
        backend,
        "SELECT 1 FROM sqlite_master WHERE type = ? AND name = ? LIMIT 1",
        [entry_type.into(), name.into()],
    );
    let row = conn.query_one(stmt).await?;
    Ok(row.is_some())
}

async fn ensure_ai_projection_schema(conn: &DatabaseConnection) -> Result<(), IOError> {
    if !sqlite_schema_contains(conn, "table", "object_index")
        .await
        .map_err(|err| IOError::other(format!("Failed to inspect core schema: {err}")))?
    {
        let backend = conn.get_database_backend();
        conn.execute(Statement::from_string(backend, BOOTSTRAP_SQL))
            .await
            .map_err(|err| IOError::other(format!("Failed to bootstrap SQLite schema: {err}")))?;
        return Ok(());
    }

    let has_ai_table = sqlite_schema_contains(conn, "table", "ai_index_intent_context_frame")
        .await
        .map_err(|err| IOError::other(format!("Failed to inspect AI schema: {err}")))?;
    let has_ai_index = sqlite_schema_contains(conn, "index", "uq_ai_thread_intent_intent")
        .await
        .map_err(|err| IOError::other(format!("Failed to inspect AI schema: {err}")))?;

    if has_ai_table && has_ai_index {
        return Ok(());
    }

    let backend = conn.get_database_backend();
    conn.execute(Statement::from_string(backend, ai_projection_sql()?))
        .await
        .map_err(|err| IOError::other(format!("Failed to apply AI projection schema: {err}")))?;
    Ok(())
}

async fn connect_database(db_path: &str) -> io::Result<DatabaseConnection> {
    let mut option = ConnectOptions::new(format!("sqlite://{db_path}"));
    option.sqlx_logging(false); // TODO use better option
    Database::connect(option)
        .await
        .map_err(|err| IOError::other(format!("Database connection error: {err:?}")))
}

/// Create a new SQLite database file at the specified path.
/// **should only be called in init or test**
/// - `db_path` is the path to the SQLite database file.
/// - Returns `Ok(())` if the database file was created and the schema was set up successfully.
/// - Returns an `IOError` if the database file already exists, or if there was an error creating the file or setting up the schema.
#[allow(dead_code)]
pub async fn create_database(db_path: &str) -> io::Result<DatabaseConnection> {
    if Path::new(db_path).exists() {
        return Err(IOError::new(
            ErrorKind::AlreadyExists,
            "Database file already exists.",
        ));
    }

    std::fs::File::create(db_path)
        .map_err(|err| IOError::other(format!("Failed to create database file: {err:?}")))?;

    // Connect to the new database and set up the schema.
    match connect_database(db_path).await {
        Ok(conn) => {
            setup_database_sql(&conn)
                .await
                .map_err(|err| IOError::other(format!("Failed to setup database: {err:?}")))?;
            Ok(conn)
        }
        _ => Err(IOError::other("Failed to connect to new database.")),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, sync::Arc};

    use sea_orm::{
        ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, EntityTrait, QueryFilter, Set,
    };
    use tests::{object_index, reference::ConfigKind};
    use tokio::sync::Barrier;

    use super::*;

    /// TestDbPath is a helper struct create and delete test database file
    struct TestDbPath(String);
    impl Drop for TestDbPath {
        fn drop(&mut self) {
            if Path::new(&self.0).exists() {
                let _ = fs::remove_file(&self.0);
            }
        }
    }
    impl TestDbPath {
        async fn new(name: &str) -> Self {
            let mut db_path = std::env::temp_dir();
            db_path.push("test_db");
            if !db_path.exists() {
                let _ = fs::create_dir_all(&db_path);
            }
            db_path.push(name);
            let db_path_str = db_path.to_str().unwrap().to_string();
            if db_path.exists() {
                let _ = fs::remove_file(&db_path);
            }
            let rt = TestDbPath(db_path_str);
            create_database(rt.0.as_str()).await.unwrap();
            rt
        }
    }

    #[tokio::test]
    async fn test_create_database() {
        let mut db_path_buf = std::env::temp_dir();
        db_path_buf.push("test_create_database.db");
        let db_path = db_path_buf.to_str().unwrap();

        if Path::new(db_path).exists() {
            fs::remove_file(db_path).unwrap();
        }
        let conn = create_database(db_path).await.unwrap();
        assert!(Path::new(db_path).exists());

        let result = create_database(db_path).await;
        assert!(result.is_err());

        conn.close().await.unwrap();
        fs::remove_file(db_path).unwrap();
    }

    #[tokio::test]
    async fn test_insert_config() {
        // insert into config_entry & config_section, check foreign key constraint
        let test_db = TestDbPath::new("test_insert_config.db").await;
        let db_path = test_db.0.as_str();

        let conn = establish_connection(db_path).await.unwrap();
        // test insert config without name
        {
            let entries = [
                ("repositoryformatversion", "0"),
                ("filemode", "true"),
                ("bare", "false"),
                ("logallrefupdates", "true"),
            ];
            for (key, value) in entries.iter() {
                let entry = config::ActiveModel {
                    configuration: Set("core".to_string()),
                    name: Set(None),
                    key: Set(key.to_string()),
                    value: Set(value.to_string()),
                    ..Default::default()
                };
                let config = entry.save(&conn).await.unwrap();
                assert_eq!(config.key.unwrap(), key.to_string());
            }
            let result = config::Entity::find().all(&conn).await.unwrap();
            assert_eq!(result.len(), entries.len(), "config_section count is not 1");
        }
        // test insert config with name
        {
            let entry = config::ActiveModel {
                id: NotSet,
                configuration: Set("remote".to_string()),
                name: Set(Some("origin".to_string())),
                key: Set("url".to_string()),
                value: Set("https://localhost".to_string()),
            };
            let config = entry.save(&conn).await.unwrap();
            assert_ne!(config.id.unwrap(), 0);
        }

        // test search config
        {
            let result = config::Entity::find()
                .filter(config::Column::Configuration.eq("core"))
                .all(&conn)
                .await
                .unwrap();
            assert_eq!(result.len(), 4, "config_section count is not 5");
        }
    }

    #[tokio::test]
    async fn test_insert_reference() {
        // insert into reference, check foreign key constraint
        let test_db = TestDbPath::new("test_insert_reference.db").await;
        let db_path = test_db.0.as_str();

        let conn = establish_connection(db_path).await.unwrap();
        // test insert reference
        let entries = [
            (Some("master"), ConfigKind::Head, None, None), // attached head
            (None, ConfigKind::Head, Some("2019"), None),   // detached head
            (Some("master"), ConfigKind::Branch, Some("2019"), None), // local branch
            (Some("release1"), ConfigKind::Tag, Some("2019"), None), // tag (remote tag store same as local tag)
            (
                Some("main"),
                ConfigKind::Head,
                None,
                Some("origin".to_string()),
            ), // remote head
            (
                Some("main"),
                ConfigKind::Branch,
                Some("a"),
                Some("origin".to_string()),
            ),
        ];
        for (name, kind, commit, remote) in entries.iter() {
            let entry = reference::ActiveModel {
                name: Set(name.map(|s| s.to_string())),
                kind: Set(kind.clone()),
                commit: Set(commit.map(|s| s.to_string())),
                remote: Set(remote.clone()),
                ..Default::default()
            };
            let reference_entry = entry.save(&conn).await.unwrap();
            assert_eq!(reference_entry.name.unwrap(), name.map(|s| s.to_string()));
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_reference_check() {
        // test reference check
        let test_db = TestDbPath::new("test_reference_check.db").await;
        let db_path = test_db.0.as_str();

        let conn = establish_connection(db_path).await.unwrap();

        // test `remote`` can't be ''
        let entry = reference::ActiveModel {
            name: Set(Some("master".to_string())),
            kind: Set(ConfigKind::Head),
            commit: Set(Some("2019922235".to_string())),
            remote: Set(Some("".to_string())),
            ..Default::default()
        };
        let result = entry.save(&conn).await;
        assert!(
            result.is_err(),
            "reference check `remote` can't be '' failed"
        );

        // test `name`` can't be ''
        let entry = reference::ActiveModel {
            name: Set(Some("".to_string())),
            kind: Set(ConfigKind::Head),
            commit: Set(Some("2019922235".to_string())),
            remote: Set(Some("origin".to_string())),
            ..Default::default()
        };
        let result = entry.save(&conn).await;
        assert!(result.is_err(), "reference check `name` can't be '' failed");

        // test `remote` must be None for tag
        let entry = reference::ActiveModel {
            name: Set(Some("master".to_string())),
            kind: Set(ConfigKind::Tag),
            commit: Set(Some("2019922235".to_string())),
            remote: Set(Some("origin".to_string())),
            ..Default::default()
        };
        let result = entry.save(&conn).await;
        assert!(
            result.is_err(),
            "reference check `remote` must be None for tag failed"
        );

        // test (`name`, `type`) can't be duplicated when `remote` is None
        let entry = reference::ActiveModel {
            name: Set(Some("test_branch".to_string())),
            kind: Set(ConfigKind::Branch),
            ..Default::default()
        };
        let result = entry.clone().save(&conn).await;
        assert!(result.is_ok());
        let result = entry.save(&conn).await;
        assert!(result.is_err(), "reference check duplicated failed");

        // test (`name`, `type`) can't be duplicated when `remote` is not None
        let entry = reference::ActiveModel {
            name: Set(Some("test_branch".to_string())),
            kind: Set(ConfigKind::Branch),
            remote: Set(Some("origin".to_string())),
            ..Default::default()
        };
        let result = entry.clone().save(&conn).await;
        assert!(result.is_ok()); // not duplicated because remote is different
        let result = entry.save(&conn).await;
        assert!(result.is_err(), "reference check duplicated failed");
    }

    #[tokio::test]
    async fn test_object_index_crud() {
        // Test CRUD operations on object_index table
        let test_db = TestDbPath::new("test_object_index_crud.db").await;
        let db_path = test_db.0.as_str();

        let conn = establish_connection(db_path).await.unwrap();

        // Test insert
        let repo_id = "test-repo-uuid-1234";
        let obj_hash = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
        let entry = object_index::ActiveModel {
            o_id: Set(obj_hash.to_string()),
            o_type: Set("blob".to_string()),
            o_size: Set(0),
            repo_id: Set(repo_id.to_string()),
            created_at: Set(chrono::Utc::now().timestamp()),
            is_synced: Set(0),
            ..Default::default()
        };
        let result = entry.save(&conn).await;
        assert!(result.is_ok(), "Failed to insert object_index");

        // Test query by repo_id
        let results = object_index::Entity::find()
            .filter(object_index::Column::RepoId.eq(repo_id))
            .all(&conn)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].o_id, obj_hash);
        assert_eq!(results[0].o_type, "blob");
        assert_eq!(results[0].is_synced, 0);

        // Test update is_synced
        let mut active: object_index::ActiveModel = results[0].clone().into();
        active.is_synced = Set(1);
        let updated = active.update(&conn).await.unwrap();
        assert_eq!(updated.is_synced, 1);

        // Test query unsynced objects
        let unsynced = object_index::Entity::find()
            .filter(object_index::Column::RepoId.eq(repo_id))
            .filter(object_index::Column::IsSynced.eq(0))
            .all(&conn)
            .await
            .unwrap();
        assert_eq!(
            unsynced.len(),
            0,
            "Should have no unsynced objects after update"
        );

        // Test unique constraint on (repo_id, o_id)
        let duplicate_entry = object_index::ActiveModel {
            o_id: Set(obj_hash.to_string()),
            o_type: Set("tree".to_string()),
            o_size: Set(100),
            repo_id: Set(repo_id.to_string()),
            created_at: Set(chrono::Utc::now().timestamp()),
            is_synced: Set(0),
            ..Default::default()
        };
        let result = duplicate_entry.insert(&conn).await;
        assert!(
            result.is_err(),
            "Should fail due to unique constraint on o_id"
        );

        // Test insert different object types
        let types = ["tree", "commit", "tag"];
        for (i, obj_type) in types.iter().enumerate() {
            let entry = object_index::ActiveModel {
                o_id: Set(format!("hash_{i}_{obj_type}")),
                o_type: Set(obj_type.to_string()),
                o_size: Set((i * 100) as i64),
                repo_id: Set(repo_id.to_string()),
                created_at: Set(chrono::Utc::now().timestamp()),
                is_synced: Set(0),
                ..Default::default()
            };
            entry.insert(&conn).await.unwrap();
        }

        // Verify all objects in repo
        let all_objects = object_index::Entity::find()
            .filter(object_index::Column::RepoId.eq(repo_id))
            .all(&conn)
            .await
            .unwrap();
        assert_eq!(all_objects.len(), 4, "Should have 4 objects total");
    }

    #[tokio::test]
    async fn test_establish_connection_backfills_ai_projection_tables() {
        let mut db_path_buf = std::env::temp_dir();
        db_path_buf.push("test_ai_projection_backfill.db");
        let db_path = db_path_buf.to_str().unwrap();

        if Path::new(db_path).exists() {
            fs::remove_file(db_path).unwrap();
        }

        fs::File::create(db_path).unwrap();

        let conn = establish_connection(db_path).await.unwrap();
        let backend = conn.get_database_backend();
        let stmt = Statement::from_sql_and_values(
            backend,
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
            ["ai_thread".into()],
        );
        let row = conn.query_one(stmt).await.unwrap();

        assert!(row.is_some(), "expected ai_thread table to exist");

        conn.close().await.unwrap();
        fs::remove_file(db_path).unwrap();
    }

    #[test]
    fn test_ai_projection_sql_only_contains_ai_schema() {
        let ai_sql = ai_projection_sql().unwrap();

        assert!(ai_sql.contains("CREATE TABLE IF NOT EXISTS `ai_thread`"));
        assert!(ai_sql.contains("CREATE TABLE IF NOT EXISTS `ai_scheduler_state`"));
        assert!(!ai_sql.contains("CREATE TABLE IF NOT EXISTS `config`"));
        assert!(!ai_sql.contains("CREATE TABLE IF NOT EXISTS `reference`"));
        assert!(!ai_sql.contains("CREATE TABLE IF NOT EXISTS `object_index`"));
    }

    #[tokio::test]
    async fn test_establish_connection_backfills_ai_projection_schema_for_core_only_db() {
        let mut db_path_buf = std::env::temp_dir();
        db_path_buf.push("test_ai_projection_backfill_core_only.db");
        let db_path = db_path_buf.to_str().unwrap();

        if Path::new(db_path).exists() {
            fs::remove_file(db_path).unwrap();
        }

        fs::File::create(db_path).unwrap();

        let conn = connect_database(db_path).await.unwrap();
        let core_sql_end = BOOTSTRAP_SQL.find(AI_PROJECTION_SCHEMA_START).unwrap();
        let core_sql = BOOTSTRAP_SQL[..core_sql_end].trim();
        let backend = conn.get_database_backend();
        conn.execute(Statement::from_string(backend, core_sql))
            .await
            .unwrap();
        conn.close().await.unwrap();

        let conn = establish_connection(db_path).await.unwrap();
        let backend = conn.get_database_backend();

        let ai_stmt = Statement::from_sql_and_values(
            backend,
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
            ["ai_thread".into()],
        );
        let ai_row = conn.query_one(ai_stmt).await.unwrap();
        assert!(ai_row.is_some(), "expected ai_thread table to exist");

        let core_stmt = Statement::from_sql_and_values(
            backend,
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
            ["object_index".into()],
        );
        let core_row = conn.query_one(core_stmt).await.unwrap();
        assert!(
            core_row.is_some(),
            "expected object_index table to remain present"
        );

        conn.close().await.unwrap();
        fs::remove_file(db_path).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_get_db_conn_instance_for_path_caches_requested_path_under_race() {
        let test_db =
            TestDbPath::new("test_get_db_conn_instance_for_path_reuses_under_race.db").await;
        let db_path = PathBuf::from(&test_db.0);

        reset_db_conn_instance_for_path(&db_path).await;

        let barrier = Arc::new(Barrier::new(8));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let barrier = Arc::clone(&barrier);
            let db_path = db_path.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                get_db_conn_instance_for_path(&db_path).await
            }));
        }

        for task in tasks {
            let conn = task.await.unwrap().unwrap();
            let backend = conn.get_database_backend();
            let stmt = Statement::from_sql_and_values(backend, "SELECT 1", []);
            let row = conn.query_one(stmt).await.unwrap();
            assert!(row.is_some());
        }

        let connections = TEST_DB_CONNECTIONS.lock().await;
        let cached = connections.get(&db_path);
        assert!(cached.is_some());
        assert_eq!(
            connections.keys().filter(|path| *path == &db_path).count(),
            1
        );
    }

    #[tokio::test]
    async fn test_reset_db_conn_instance_for_path_drops_cached_connection() {
        let test_db = TestDbPath::new("test_reset_db_conn_instance_for_path.db").await;
        let db_path = PathBuf::from(&test_db.0);

        let _conn = get_db_conn_instance_for_path(&db_path).await.unwrap();
        {
            let connections = TEST_DB_CONNECTIONS.lock().await;
            assert!(connections.contains_key(&db_path));
        }

        reset_db_conn_instance_for_path(&db_path).await;

        let connections = TEST_DB_CONNECTIONS.lock().await;
        assert!(!connections.contains_key(&db_path));
    }
}
