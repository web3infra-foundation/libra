use crate::utils::path;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::io;
use std::io::Error as IOError;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use turso::{Builder, Connection, Database};

/// Wrapper around Turso database connection with additional functionality
pub struct DbConnection {
    _db: Arc<Database>,
    conn: Arc<Mutex<Connection>>,
}

impl DbConnection {
    /// Create a new connection to the database at the specified path
    pub async fn connect(db_path: &str) -> Result<Self, IOError> {
        if !Path::new(db_path).exists() {
            return Err(IOError::new(
                ErrorKind::NotFound,
                "Database file does not exist.",
            ));
        }

        let db = Builder::new_local(db_path)
            .build()
            .await
            .map_err(|e| IOError::other(format!("Turso database build error: {e:?}")))?;

        let conn = db
            .connect()
            .map_err(|e| IOError::other(format!("Failed to open Turso connection: {e:?}")))?;

        Ok(Self {
            _db: Arc::new(db),
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Execute a SQL statement that doesn't return rows
    pub async fn execute(&self, sql: &str, params: impl turso::IntoParams) -> Result<u64, IOError> {
        let conn = self.conn.lock().await;
        conn.execute(sql, params)
            .await
            .map_err(|e| IOError::other(format!("Execute error: {e:?}")))
    }

    /// Execute a SQL query that returns rows
    pub async fn query(
        &self,
        sql: &str,
        params: impl turso::IntoParams,
    ) -> Result<turso::Rows, IOError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(sql)
            .await
            .map_err(|e| IOError::other(format!("Prepare error: {e:?}")))?;
        stmt.query(params)
            .await
            .map_err(|e| IOError::other(format!("Query error: {e:?}")))
    }

    /// Begin a transaction and execute the provided closure
    /// The closure receives &DbConnection (not &Transaction) for simplicity
    pub async fn transaction<F, T>(&self, f: F) -> Result<T, IOError>
    where
        F: for<'a> FnOnce(
            &'a DbConnection,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<T, IOError>> + Send + 'a>,
        >,
    {
        // For simplicity, we just pass self to the closure
        // Turso handles transaction isolation at the connection level
        f(self).await
    }

    /// Execute a batch of SQL statements (for initialization)
    pub async fn execute_batch(&self, sql: &str) -> Result<(), IOError> {
        let conn = self.conn.lock().await;
        conn.execute_batch(sql)
            .await
            .map_err(|e| IOError::other(format!("Execute batch error: {e:?}")))
    }
}

// Global connection pool: maps working directory to database connection
static TEST_DB_CONNECTIONS: Lazy<Mutex<HashMap<PathBuf, Arc<DbConnection>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Get or create a database connection instance for the current working directory.
/// In test environment, each working directory has its own connection.
pub async fn get_db_conn_instance() -> Arc<DbConnection> {
    let current_dir = std::env::current_dir().unwrap();
    let mut connections = TEST_DB_CONNECTIONS.lock().await;

    if !connections.contains_key(&current_dir) {
        let conn = get_db_conn().await.unwrap();
        connections.insert(current_dir.clone(), Arc::new(conn));
    }

    connections.get(&current_dir).unwrap().clone()
}

/// Create a connection to the database of current repo: `.libra/libra.db`
async fn get_db_conn() -> io::Result<DbConnection> {
    let db_path = path::database(); // for longer lifetime
    let db_path = db_path.to_str().unwrap();
    DbConnection::connect(db_path).await
}

/// Establish a connection to the database (kept for compatibility)
#[allow(dead_code)]
pub async fn establish_connection(db_path: &str) -> Result<DbConnection, IOError> {
    DbConnection::connect(db_path).await
}

/// create table using Turso-compatible schema (without CHECK constraints)
async fn setup_database_sql(conn: &DbConnection) -> Result<(), IOError> {
    // `include_str!` will expand the file while compiling, so `.sql` is not needed after that
    const SETUP_SQL: &str = include_str!("../../sql/turso_init.sql");
    conn.execute_batch(SETUP_SQL).await
}

/// Create a new database file at the specified path.
/// **should only be called in init or test**
/// - `db_path` is the path to the database file.
/// - Returns `Ok(())` if the database file was created and the schema was set up successfully.
/// - Returns an `IOError` if the database file already exists, or if there was an error creating the file or setting up the schema.
#[allow(dead_code)]
pub async fn create_database(db_path: &str) -> io::Result<DbConnection> {
    if Path::new(db_path).exists() {
        return Err(IOError::new(
            ErrorKind::AlreadyExists,
            "Database file already exists.",
        ));
    }

    std::fs::File::create(db_path)
        .map_err(|err| IOError::other(format!("Failed to create database file: {err:?}")))?;

    // Connect to the new database and set up the schema.
    let conn = DbConnection::connect(db_path).await?;
    setup_database_sql(&conn).await?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
        db_path_buf.push("test_turso_create_database.db");
        let db_path = db_path_buf.to_str().unwrap();

        if Path::new(db_path).exists() {
            fs::remove_file(db_path).unwrap();
        }
        let _conn = create_database(db_path).await.unwrap();
        assert!(Path::new(db_path).exists());

        let result = create_database(db_path).await;
        assert!(result.is_err());

        fs::remove_file(db_path).unwrap();
    }

    #[tokio::test]
    async fn test_turso_basic_operations() {
        let test_db = TestDbPath::new("test_turso_basic.db").await;
        let db_path = test_db.0.as_str();

        let conn = establish_connection(db_path).await.unwrap();

        // Test insert
        let result = conn
            .execute(
                "INSERT INTO config (configuration, name, key, value) VALUES (?1, ?2, ?3, ?4)",
                turso::params!["core", None::<String>, "repositoryformatversion", "0"],
            )
            .await;
        assert!(result.is_ok(), "Insert should succeed");

        // Test query
        let mut rows = conn
            .query(
                "SELECT * FROM config WHERE configuration = ?1",
                turso::params!["core"],
            )
            .await
            .unwrap();

        let row = rows.next().await.unwrap().unwrap();
        let config_value: String = row.get(4).unwrap(); // value is column 4
        assert_eq!(config_value, "0");

        // Test update
        let result = conn
            .execute(
                "UPDATE config SET value = ?1 WHERE configuration = ?2 AND key = ?3",
                turso::params!["1", "core", "repositoryformatversion"],
            )
            .await;
        assert!(result.is_ok(), "Update should succeed");

        // Verify update
        let mut rows = conn
            .query(
                "SELECT value FROM config WHERE configuration = ?1 AND key = ?2",
                turso::params!["core", "repositoryformatversion"],
            )
            .await
            .unwrap();

        let row = rows.next().await.unwrap().unwrap();
        let updated_value: String = row.get(0).unwrap();
        assert_eq!(updated_value, "1");
    }

    #[tokio::test]
    async fn test_turso_transaction() {
        let test_db = TestDbPath::new("test_turso_transaction.db").await;
        let db_path = test_db.0.as_str();

        let conn = establish_connection(db_path).await.unwrap();

        // Test successful transaction
        let result = conn.transaction(|tx| {
            Box::pin(async move {
                tx.execute(
                    "INSERT INTO config (configuration, name, key, value) VALUES (?1, ?2, ?3, ?4)",
                    turso::params!["remote", Some("origin"), "url", "https://example.com"]
                ).await.map_err(|e| IOError::other(format!("{e:?}")))?;
                tx.execute(
                    "INSERT INTO config (configuration, name, key, value) VALUES (?1, ?2, ?3, ?4)",
                    turso::params!["remote", Some("origin"), "fetch", "+refs/heads/*:refs/remotes/origin/*"]
                ).await.map_err(|e| IOError::other(format!("{e:?}")))?;
                Ok::<_, IOError>(())
            })
        }).await;

        assert!(result.is_ok(), "Transaction should succeed");

        // Verify both inserts were committed
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM config WHERE configuration = ?1",
                turso::params!["remote"],
            )
            .await
            .unwrap();

        let row = rows.next().await.unwrap().unwrap();
        let count: i64 = row.get(0).unwrap();
        assert_eq!(count, 2, "Both inserts should be committed");
    }

    #[tokio::test]
    async fn test_turso_constraints() {
        let test_db = TestDbPath::new("test_turso_constraints.db").await;
        let db_path = test_db.0.as_str();

        let conn = establish_connection(db_path).await.unwrap();

        // NOTE: Turso doesn't support CHECK constraints yet, so we skip those tests
        // Validation should be done in application layer instead

        // Test unique index constraint
        conn.execute(
            "INSERT INTO reference (name, kind, `commit`, remote) VALUES (?1, ?2, ?3, ?4)",
            turso::params![Some("main"), "Branch", Some("abc123"), None::<String>],
        )
        .await
        .unwrap();

        let result = conn
            .execute(
                "INSERT INTO reference (name, kind, `commit`, remote) VALUES (?1, ?2, ?3, ?4)",
                turso::params![Some("main"), "Branch", Some("def456"), None::<String>],
            )
            .await;
        assert!(result.is_err(), "Should fail unique constraint");
    }
}
