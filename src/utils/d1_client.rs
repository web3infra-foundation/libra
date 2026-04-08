//! Cloudflare D1 REST API client for database backup and synchronization.
//!
//! This module provides a client for interacting with Cloudflare D1 database
//! via the REST API. It supports executing SQL statements, querying data,
//! and batch operations for efficient cloud backup.

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

use crate::internal::config::resolve_env;

/// D1 API response wrapper
#[derive(Debug, Deserialize)]
pub struct D1Response<T> {
    pub success: bool,
    pub errors: Vec<D1Error>,
    pub messages: Vec<D1Message>,
    pub result: Option<Vec<D1QueryResult<T>>>,
}

/// D1 API error
#[derive(Debug, Deserialize)]
pub struct D1Error {
    pub code: i32,
    pub message: String,
}

/// D1 API message
#[derive(Debug, Deserialize)]
pub struct D1Message {
    pub code: Option<i32>,
    pub message: String,
}

/// D1 query result
#[derive(Debug, Deserialize)]
pub struct D1QueryResult<T> {
    pub results: Option<Vec<T>>,
    pub success: bool,
    pub meta: Option<D1Meta>,
}

/// D1 query metadata
#[derive(Debug, Deserialize)]
pub struct D1Meta {
    pub changes: Option<i64>,
    pub duration: Option<f64>,
    pub last_row_id: Option<i64>,
    pub rows_read: Option<i64>,
    pub rows_written: Option<i64>,
}

/// D1 SQL statement for batch execution
#[derive(Debug, Serialize)]
pub struct D1Statement {
    pub sql: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<serde_json::Value>>,
}

/// Cloudflare D1 REST API client
#[derive(Clone)]
pub struct D1Client {
    client: Client,
    account_id: String,
    api_token: String,
    database_id: String,
}

impl D1Client {
    /// Create a new D1 client from environment variables, using
    /// [`resolve_env`](crate::internal::config::resolve_env) so that
    /// vault-stored secrets are picked up automatically.
    ///
    /// Resolution order per variable:
    /// 1. System environment variable (`std::env::var`)
    /// 2. Local vault config (`vault.env.<VAR>`)
    /// 3. Global vault config (`~/.libra/config.db`)
    ///
    /// Required variables:
    /// - `LIBRA_D1_ACCOUNT_ID`: Cloudflare Account ID
    /// - `LIBRA_D1_API_TOKEN`: Cloudflare API Token
    /// - `LIBRA_D1_DATABASE_ID`: D1 Database ID
    pub async fn from_env() -> Result<Self, D1Error> {
        let account_id = Self::resolve_required_env("LIBRA_D1_ACCOUNT_ID", 1001, 1101).await?;
        let api_token = Self::resolve_required_env("LIBRA_D1_API_TOKEN", 1002, 1102).await?;
        let database_id = Self::resolve_required_env("LIBRA_D1_DATABASE_ID", 1003, 1103).await?;

        Ok(Self::new(account_id, api_token, database_id))
    }

    async fn resolve_required_env(
        name: &str,
        missing_code: i32,
        resolution_error_code: i32,
    ) -> Result<String, D1Error> {
        match resolve_env(name).await {
            Ok(Some(value)) if !value.is_empty() => Ok(value),
            Ok(Some(_)) | Ok(None) => Err(D1Error {
                code: missing_code,
                message: format!("{name} not set (env or vault)"),
            }),
            Err(err) => Err(D1Error {
                code: resolution_error_code,
                message: format!("failed to resolve {name} from env or config: {err}"),
            }),
        }
    }

    /// Create a new D1 client with explicit credentials
    pub fn new(account_id: String, api_token: String, database_id: String) -> Self {
        let client = Client::builder()
            .https_only(true)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            account_id,
            api_token,
            database_id,
        }
    }

    /// Get the D1 API endpoint URL
    fn api_url(&self) -> Result<Url, D1Error> {
        let url_str = format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/d1/database/{}/query",
            self.account_id, self.database_id
        );
        let url = Url::parse(&url_str).map_err(|e| D1Error {
            code: 2005,
            message: format!("Invalid API URL: {}", e),
        })?;

        if url.scheme() != "https" {
            return Err(D1Error {
                code: 2006,
                message: "API URL must use HTTPS".to_string(),
            });
        }

        Ok(url)
    }

    /// Execute a single SQL statement
    pub async fn execute(
        &self,
        sql: &str,
        params: Option<Vec<serde_json::Value>>,
    ) -> Result<D1QueryResult<serde_json::Value>, D1Error> {
        let statement = D1Statement {
            sql: sql.to_string(),
            params,
        };

        let url = self.api_url()?;

        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_token))
            .header("Content-Type", "application/json")
            .json(&statement)
            .send()
            .await
            .map_err(|e| D1Error {
                code: 2001,
                message: format!("HTTP request failed: {:?}", e),
            })?;

        let status = response.status();
        let body = response.text().await.map_err(|e| D1Error {
            code: 2002,
            message: format!("Failed to read response body: {}", e),
        })?;

        if !status.is_success() {
            return Err(D1Error {
                code: status.as_u16() as i32,
                message: format!("D1 API error: {}", body),
            });
        }

        let d1_response: D1Response<serde_json::Value> =
            serde_json::from_str(&body).map_err(|e| D1Error {
                code: 2003,
                message: format!("Failed to parse D1 response: {} - body: {}", e, body),
            })?;

        if !d1_response.success {
            let error_msg = d1_response
                .errors
                .first()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "Unknown D1 error".to_string());
            return Err(D1Error {
                code: d1_response.errors.first().map(|e| e.code).unwrap_or(3000),
                message: error_msg,
            });
        }

        d1_response
            .result
            .and_then(|r| r.into_iter().next())
            .ok_or_else(|| D1Error {
                code: 3001,
                message: "Empty result from D1".to_string(),
            })
    }

    /// Query and return typed results
    pub async fn query<T: for<'de> Deserialize<'de>>(
        &self,
        sql: &str,
        params: Option<Vec<serde_json::Value>>,
    ) -> Result<Vec<T>, D1Error> {
        let result = self.execute(sql, params).await?;

        let results = result.results.unwrap_or_default();
        let mut typed_results = Vec::with_capacity(results.len());

        for v in results {
            let t: T = serde_json::from_value(v).map_err(|e| D1Error {
                code: 2004,
                message: format!("Failed to deserialize result row: {}", e),
            })?;
            typed_results.push(t);
        }

        Ok(typed_results)
    }

    /// Execute multiple SQL statements in a batch
    ///
    /// Note: This currently executes statements sequentially as a fallback,
    /// due to potential API compatibility issues with array inputs on the `/query` endpoint.
    pub async fn batch(
        &self,
        statements: Vec<D1Statement>,
    ) -> Result<Vec<D1QueryResult<serde_json::Value>>, D1Error> {
        let mut results = Vec::new();
        for stmt in statements {
            let query_result = self.execute(&stmt.sql, stmt.params).await?;
            results.push(query_result);
        }

        Ok(results)
    }

    /// Create object_index table in D1 if not exists
    pub async fn ensure_object_index_table(&self) -> Result<(), D1Error> {
        #[derive(Deserialize)]
        struct SqlRow {
            sql: Option<String>,
        }

        let create_v2_sql = r#"
            CREATE TABLE IF NOT EXISTS object_index (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                o_id TEXT NOT NULL,
                o_type TEXT NOT NULL,
                o_size INTEGER NOT NULL,
                repo_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                is_synced INTEGER DEFAULT 0,
                UNIQUE(repo_id, o_id)
            )
        "#;

        let existing: Vec<SqlRow> = self
            .query(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='object_index'",
                None,
            )
            .await?;

        if existing.is_empty() {
            self.execute(create_v2_sql, None).await?;
        } else {
            let table_sql = existing[0].sql.clone().unwrap_or_default();
            let has_bad_unique = table_sql.contains("o_id TEXT NOT NULL UNIQUE");
            let has_composite_unique = table_sql.contains("UNIQUE(repo_id, o_id)")
                || table_sql.contains("UNIQUE (repo_id, o_id)");

            if has_bad_unique && !has_composite_unique {
                self.execute("DROP TABLE IF EXISTS object_index_v2", None)
                    .await?;
                self.execute(
                    r#"
                        CREATE TABLE object_index_v2 (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            o_id TEXT NOT NULL,
                            o_type TEXT NOT NULL,
                            o_size INTEGER NOT NULL,
                            repo_id TEXT NOT NULL,
                            created_at INTEGER NOT NULL,
                            is_synced INTEGER DEFAULT 0,
                            UNIQUE(repo_id, o_id)
                        )
                    "#,
                    None,
                )
                .await?;

                self.execute(
                    r#"
                        INSERT INTO object_index_v2 (o_id, o_type, o_size, repo_id, created_at, is_synced)
                        SELECT o_id, o_type, o_size, repo_id, created_at, is_synced FROM object_index
                    "#,
                    None,
                )
                .await?;

                self.execute("DROP TABLE object_index", None).await?;
                self.execute("ALTER TABLE object_index_v2 RENAME TO object_index", None)
                    .await?;
            }

            self.execute(create_v2_sql, None).await?;
        }

        self.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_d1_object_repo_oid ON object_index (repo_id, o_id)",
            None,
        )
        .await?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS idx_d1_object_repo ON object_index (repo_id)",
            None,
        )
        .await?;

        Ok(())
    }

    /// Upsert an object index entry
    pub async fn upsert_object_index(
        &self,
        o_id: &str,
        o_type: &str,
        o_size: i64,
        repo_id: &str,
        created_at: i64,
    ) -> Result<(), D1Error> {
        let sql = r#"
            INSERT INTO object_index (o_id, o_type, o_size, repo_id, created_at, is_synced)
            VALUES (?1, ?2, ?3, ?4, ?5, 1)
            ON CONFLICT(repo_id, o_id) DO UPDATE SET
                o_type = excluded.o_type,
                o_size = excluded.o_size,
                created_at = excluded.created_at,
                is_synced = 1
        "#;
        let params = vec![
            serde_json::json!(o_id),
            serde_json::json!(o_type),
            serde_json::json!(o_size),
            serde_json::json!(repo_id),
            serde_json::json!(created_at),
        ];
        self.execute(sql, Some(params)).await?;
        Ok(())
    }

    /// Get all object indexes for a repo
    pub async fn get_object_indexes(&self, repo_id: &str) -> Result<Vec<ObjectIndexRow>, D1Error> {
        let sql = "SELECT o_id, o_type, o_size, repo_id, created_at, is_synced FROM object_index WHERE repo_id = ?1";
        self.query(sql, Some(vec![serde_json::json!(repo_id)]))
            .await
    }

    /// Create repositories table in D1 if not exists
    pub async fn ensure_repositories_table(&self) -> Result<(), D1Error> {
        let sql = r#"
            CREATE TABLE IF NOT EXISTS repositories (
                repo_id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )
        "#;
        self.execute(sql, None).await?;
        Ok(())
    }

    /// Upsert a repository
    ///
    /// This function handles three cases:
    /// 1. New repository: Inserts a new record.
    /// 2. Existing repository (same repo_id): Updates the name and timestamp.
    /// 3. Name conflict (different repo_id): Returns the existing repository row
    ///    that already owns this `name`.
    ///
    /// Callers that need to enforce unique names per logical repository must
    /// compare the returned `repo_id` with the one they attempted to upsert; if
    /// they differ, a logical name conflict has occurred.
    pub async fn upsert_repository(
        &self,
        repo_id: &str,
        name: &str,
    ) -> Result<RepositoryRow, D1Error> {
        let now = chrono::Utc::now().timestamp();
        // Try to insert or update existing repo_id (renaming project)
        let sql = r#"
            INSERT INTO repositories (repo_id, name, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(repo_id) DO UPDATE SET
                name = excluded.name,
                updated_at = excluded.updated_at
            RETURNING repo_id, name, created_at, updated_at
        "#;
        let params = vec![
            serde_json::json!(repo_id),
            serde_json::json!(name),
            serde_json::json!(now),
            serde_json::json!(now),
        ];

        match self.query(sql, Some(params)).await {
            Ok(rows) => rows.into_iter().next().ok_or_else(|| D1Error {
                code: 3002,
                message: "Failed to upsert repository".to_string(),
            }),
            Err(e) => {
                // Check if error is due to name conflict (UNIQUE constraint on name)
                if e.message
                    .contains("UNIQUE constraint failed: repositories.name")
                    || e.message.contains("SQLITE_CONSTRAINT")
                {
                    // Fetch the existing repository that owns this name
                    let existing_sql = "SELECT repo_id, name, created_at, updated_at FROM repositories WHERE name = ?1";
                    let existing_rows: Vec<RepositoryRow> = self
                        .query(existing_sql, Some(vec![serde_json::json!(name)]))
                        .await?;
                    existing_rows.into_iter().next().ok_or(e)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Get repository ID by name
    pub async fn get_repo_id_by_name(&self, name: &str) -> Result<Option<String>, D1Error> {
        #[derive(Deserialize)]
        struct IdRow {
            repo_id: String,
        }
        let sql = "SELECT repo_id FROM repositories WHERE name = ?1";
        let result: Vec<IdRow> = self.query(sql, Some(vec![serde_json::json!(name)])).await?;
        Ok(result.into_iter().next().map(|r| r.repo_id))
    }
}

/// Object index row from D1
#[derive(Debug, Deserialize, Serialize)]
pub struct ObjectIndexRow {
    pub o_id: String,
    pub o_type: String,
    pub o_size: i64,
    pub repo_id: String,
    pub created_at: i64,
    pub is_synced: i32,
}

/// Repository row from D1
#[derive(Debug, Deserialize, Serialize)]
pub struct RepositoryRow {
    pub repo_id: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[cfg(test)]
mod tests {
    use std::{env, ffi::OsString};

    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        internal::config::ConfigKv,
        utils::test::{ChangeDirGuard, setup_with_new_libra_in},
    };

    struct ClearedEnvVarGuard {
        key: String,
        previous: Option<OsString>,
    }

    impl ClearedEnvVarGuard {
        fn new(key: &str) -> Self {
            let previous = env::var_os(key);
            // SAFETY: unit tests mutate process env in a controlled serial context.
            unsafe {
                env::remove_var(key);
            }
            Self {
                key: key.to_string(),
                previous,
            }
        }
    }

    impl Drop for ClearedEnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: this restores the exact previous value for the same env key.
            unsafe {
                if let Some(value) = &self.previous {
                    env::set_var(&self.key, value);
                } else {
                    env::remove_var(&self.key);
                }
            }
        }
    }

    #[test]
    fn test_d1_statement_serialization() {
        let stmt = D1Statement {
            sql: "SELECT * FROM test WHERE id = ?1".to_string(),
            params: Some(vec![serde_json::json!(1)]),
        };
        let json = serde_json::to_string(&stmt).unwrap();
        assert!(json.contains("SELECT"));
        assert!(json.contains("params"));
    }

    #[test]
    fn test_d1_statement_no_params() {
        let stmt = D1Statement {
            sql: "SELECT * FROM test".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&stmt).unwrap();
        assert!(json.contains("SELECT"));
        assert!(!json.contains("params"));
    }

    #[test]
    #[serial]
    fn d1_client_from_env_reads_values_from_local_config() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());
        let _account = ClearedEnvVarGuard::new("LIBRA_D1_ACCOUNT_ID");
        let _token = ClearedEnvVarGuard::new("LIBRA_D1_API_TOKEN");
        let _database = ClearedEnvVarGuard::new("LIBRA_D1_DATABASE_ID");

        rt.block_on(async {
            ConfigKv::set(
                "vault.env.LIBRA_D1_ACCOUNT_ID",
                "account-from-config",
                false,
            )
            .await
            .unwrap();
            ConfigKv::set("vault.env.LIBRA_D1_API_TOKEN", "token-from-config", false)
                .await
                .unwrap();
            ConfigKv::set("vault.env.LIBRA_D1_DATABASE_ID", "db-from-config", false)
                .await
                .unwrap();
        });

        let client = rt
            .block_on(D1Client::from_env())
            .expect("local config values should initialize D1 client");
        assert_eq!(client.account_id, "account-from-config");
        assert_eq!(client.api_token, "token-from-config");
        assert_eq!(client.database_id, "db-from-config");
    }

    #[test]
    #[serial]
    fn d1_client_from_env_surfaces_global_config_connection_errors() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());
        let _account = ClearedEnvVarGuard::new("LIBRA_D1_ACCOUNT_ID");
        let _token = ClearedEnvVarGuard::new("LIBRA_D1_API_TOKEN");
        let _database = ClearedEnvVarGuard::new("LIBRA_D1_DATABASE_ID");

        let bad_global_dir = tempdir().unwrap();
        let bad_global_db = bad_global_dir.path().join("bad-global.db");
        std::fs::write(&bad_global_db, "not sqlite").unwrap();
        let _global_db =
            crate::utils::test::ScopedEnvVar::set("LIBRA_CONFIG_GLOBAL_DB", &bad_global_db);

        let err = match rt.block_on(D1Client::from_env()) {
            Ok(_) => panic!("global config resolution failure should surface"),
            Err(err) => err,
        };
        assert_eq!(err.code, 1101);
        assert!(
            err.message.contains("failed to connect to global config"),
            "unexpected error: {}",
            err.message
        );
    }
}
