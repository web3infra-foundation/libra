//! Cloudflare D1 REST API client for database backup and synchronization.
//!
//! This module provides a client for interacting with Cloudflare D1 database
//! via the REST API. It supports executing SQL statements, querying data,
//! and batch operations for efficient cloud backup.

use std::env;

use reqwest::Client;
use serde::{Deserialize, Serialize};

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
    /// Create a new D1 client from environment variables
    ///
    /// Required environment variables:
    /// - `LIBRA_D1_ACCOUNT_ID`: Cloudflare Account ID
    /// - `LIBRA_D1_API_TOKEN`: Cloudflare API Token
    /// - `LIBRA_D1_DATABASE_ID`: D1 Database ID
    pub fn from_env() -> Result<Self, D1Error> {
        let account_id = env::var("LIBRA_D1_ACCOUNT_ID").map_err(|_| D1Error {
            code: 1001,
            message: "LIBRA_D1_ACCOUNT_ID environment variable not set".to_string(),
        })?;
        let api_token = env::var("LIBRA_D1_API_TOKEN").map_err(|_| D1Error {
            code: 1002,
            message: "LIBRA_D1_API_TOKEN environment variable not set".to_string(),
        })?;
        let database_id = env::var("LIBRA_D1_DATABASE_ID").map_err(|_| D1Error {
            code: 1003,
            message: "LIBRA_D1_DATABASE_ID environment variable not set".to_string(),
        })?;

        Ok(Self::new(account_id, api_token, database_id))
    }

    /// Create a new D1 client with explicit credentials
    pub fn new(account_id: String, api_token: String, database_id: String) -> Self {
        Self {
            client: Client::new(),
            account_id,
            api_token,
            database_id,
        }
    }

    /// Get the D1 API endpoint URL
    fn api_url(&self) -> String {
        format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/d1/database/{}/query",
            self.account_id, self.database_id
        )
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

        let response = self
            .client
            .post(self.api_url())
            .header("Authorization", format!("Bearer {}", self.api_token))
            .header("Content-Type", "application/json")
            .json(&statement)
            .send()
            .await
            .map_err(|e| D1Error {
                code: 2001,
                message: format!("HTTP request failed: {}", e),
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

                let _ = self
                    .execute(
                        r#"
                            INSERT INTO object_index_v2 (o_id, o_type, o_size, repo_id, created_at, is_synced)
                            SELECT o_id, o_type, o_size, repo_id, created_at, is_synced FROM object_index
                        "#,
                        None,
                    )
                    .await;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
