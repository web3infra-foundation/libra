//! Cloudflare D1 REST API client for database backup and synchronization.
//!
//! This module provides a client for interacting with Cloudflare D1 database
//! via the REST API. It supports executing SQL statements, querying data,
//! and batch operations for efficient cloud backup.
//!
//! Authentication uses a Cloudflare account ID, API token, and D1 database ID,
//! resolved through [`resolve_env`](crate::internal::config::resolve_env) so they
//! can come from process env, local repo `vault.env.*` config, or global config.
//! The client always speaks HTTPS to `api.cloudflare.com`; the constructor enforces
//! `https_only(true)` and the URL builder rejects non-HTTPS schemes defensively.
//!
//! Schema management is conservative: [`D1Client::ensure_object_index_table`]
//! migrates an older single-column unique index into a composite `(repo_id, o_id)`
//! unique index when it detects the legacy shape, so users upgrading from older
//! Libra versions do not need to drop their D1 backup database manually.

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

use crate::internal::config::resolve_env;

/// Top-level wrapper for every Cloudflare D1 API response.
///
/// Cloudflare wraps the actual query results in a `Vec<D1QueryResult>`; even single
/// queries are returned as a one-element vector. When `success == false`, the
/// `errors` vector carries the failure details.
#[derive(Debug, Deserialize)]
pub struct D1Response<T> {
    pub success: bool,
    pub errors: Vec<D1Error>,
    pub messages: Vec<D1Message>,
    pub result: Option<Vec<D1QueryResult<T>>>,
}

/// Error structure used both for Cloudflare's API errors *and* for client-side
/// failures (HTTP, JSON parsing, env resolution). Client-side codes use the 1xxx
/// and 2xxx ranges; Cloudflare's API codes occupy the 3xxx+ range.
#[derive(Debug, Deserialize)]
pub struct D1Error {
    /// Numeric error code. Stable enough to match against in tests.
    pub code: i32,
    /// Human-readable failure message.
    pub message: String,
}

/// Informational message returned by Cloudflare alongside `result` (e.g. retry hints).
#[derive(Debug, Deserialize)]
pub struct D1Message {
    pub code: Option<i32>,
    pub message: String,
}

/// One element of the `result` array returned by D1.
#[derive(Debug, Deserialize)]
pub struct D1QueryResult<T> {
    /// Row values for SELECT statements; `None` for non-SELECT statements.
    pub results: Option<Vec<T>>,
    /// `true` when the individual statement succeeded (an outer `D1Response`
    /// can succeed overall while one statement fails inside).
    pub success: bool,
    pub meta: Option<D1Meta>,
}

/// Per-statement execution metadata returned by D1.
#[derive(Debug, Deserialize)]
pub struct D1Meta {
    pub changes: Option<i64>,
    pub duration: Option<f64>,
    pub last_row_id: Option<i64>,
    pub rows_read: Option<i64>,
    pub rows_written: Option<i64>,
}

/// One SQL statement plus its bound parameters, ready to be sent over the wire.
///
/// Parameters are positional (`?1`, `?2`, ...). `params` is omitted from the JSON
/// body entirely when `None` so the request matches the single-statement shape that
/// the D1 `/query` endpoint accepts.
#[derive(Debug, Serialize)]
pub struct D1Statement {
    pub sql: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<serde_json::Value>>,
}

/// Cloudflare D1 REST API client.
///
/// `Clone` is cheap (HTTP client is `Arc` internally). Construct via
/// [`D1Client::from_env`] in production code so vault-stored credentials are
/// honoured, or [`D1Client::new`] when credentials are already in scope.
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
    ///
    /// Boundary conditions:
    /// - Returns a `D1Error` with codes `1001`/`1002`/`1003` when the corresponding
    ///   variable is unset or empty across all scopes.
    /// - Returns a `D1Error` with codes `1101`/`1102`/`1103` when the underlying
    ///   resolver fails (e.g. corrupt config database). Tests rely on these codes
    ///   to differentiate "missing" from "broken".
    /// - See: `d1_client_from_env_reads_values_from_local_config`,
    ///   `d1_client_from_env_surfaces_global_config_connection_errors`.
    pub async fn from_env() -> Result<Self, D1Error> {
        let account_id = Self::resolve_required_env("LIBRA_D1_ACCOUNT_ID", 1001, 1101).await?;
        let api_token = Self::resolve_required_env("LIBRA_D1_API_TOKEN", 1002, 1102).await?;
        let database_id = Self::resolve_required_env("LIBRA_D1_DATABASE_ID", 1003, 1103).await?;

        Ok(Self::new(account_id, api_token, database_id))
    }

    /// Resolve a required env var, mapping the two failure modes into distinct codes.
    ///
    /// Boundary conditions:
    /// - `Ok(Some(""))` is treated as missing — empty strings are not credentials.
    /// - The two error codes (`missing_code`, `resolution_error_code`) let callers
    ///   distinguish "user forgot to configure" from "configuration store is broken".
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

    /// Create a new D1 client with explicit credentials.
    ///
    /// Functional scope:
    /// - Builds an HTTPS-only `reqwest::Client`. If TLS configuration is unavailable
    ///   (extremely unlikely in production), falls back to a default `Client::new()`
    ///   so the constructor itself never fails.
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

    /// Build the per-request `/query` endpoint URL for this account/database.
    ///
    /// Boundary conditions:
    /// - Returns a `D1Error` with code `2005` when the formatted URL fails to parse
    ///   (would only happen if account_id/database_id contain invalid URL chars,
    ///   which is checked indirectly by Cloudflare's own API).
    /// - Returns a `D1Error` with code `2006` if the parsed scheme is not `https` —
    ///   defensive belt-and-braces in addition to `https_only` on the client.
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

    /// Execute a single SQL statement against the D1 database.
    ///
    /// Functional scope:
    /// - Sends a POST to `/query` with the bearer token and JSON-encoded statement.
    /// - Unwraps Cloudflare's outer `D1Response` and returns the first (and usually
    ///   only) `D1QueryResult` element.
    ///
    /// Boundary conditions:
    /// - Returns `D1Error` codes `2001`/`2002`/`2003` for HTTP, response read, and
    ///   JSON parse failures respectively.
    /// - Returns the raw HTTP status as the error code when D1 responds with non-2xx.
    /// - Returns code `3000` (default) when D1 reports failure with an empty error
    ///   list, otherwise the first element's code.
    /// - Returns code `3001` when the response is well-formed but has no result
    ///   payload — this happens after schema migrations that succeed silently.
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

    /// Query D1 and deserialise each row into `T`.
    ///
    /// Boundary conditions:
    /// - Returns an empty vector when the statement returns zero rows. Callers that
    ///   want to distinguish "no rows" from "no `results` payload" should use
    ///   [`Self::execute`] directly.
    /// - Returns `D1Error` code `2004` if any row fails to deserialise into `T`;
    ///   the message includes the row index implicitly via the underlying
    ///   `serde_json` error.
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

    /// Execute multiple SQL statements in a batch.
    ///
    /// Note: This currently executes statements sequentially as a fallback,
    /// due to potential API compatibility issues with array inputs on the `/query` endpoint.
    ///
    /// Boundary conditions:
    /// - Stops at the first failing statement and returns its error; previously
    ///   committed statements are not rolled back. D1 has no transactional batch
    ///   API for this endpoint, so callers that need atomicity must compose a
    ///   single `BEGIN/COMMIT` SQL string.
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

    /// Create or migrate the `object_index` table on the D1 side.
    ///
    /// Functional scope:
    /// - Creates the table if it does not exist with the new composite UNIQUE
    ///   constraint `(repo_id, o_id)`.
    /// - When an older table exists with `o_id TEXT NOT NULL UNIQUE` (the legacy
    ///   single-tenant shape), copies rows into a new `object_index_v2` table,
    ///   drops the old table, and renames the new one. This is a destructive
    ///   in-place migration but D1 has no transactional DDL, so partial failure
    ///   leaves a `*_v2` table that the next call will re-attempt to consume.
    /// - Always re-runs `CREATE TABLE IF NOT EXISTS` and the supporting indexes so
    ///   missing indexes are healed on every backup.
    ///
    /// Boundary conditions:
    /// - Returns the underlying `D1Error` from any failing statement. There is no
    ///   automatic rollback; an error during migration leaves the database in a
    ///   half-migrated state that is still consistent (the rename is the last step
    ///   and is atomic from D1's perspective).
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

    /// Upsert an `object_index` row keyed by `(repo_id, o_id)`.
    ///
    /// Boundary conditions:
    /// - Sets `is_synced = 1` unconditionally — once Cloudflare accepts the row, the
    ///   client treats the object as synced. Callers must still verify the data
    ///   plane copy if they need stronger guarantees.
    /// - On conflict, the stored `o_type`, `o_size`, and `created_at` are
    ///   overwritten with the values from this call so re-uploads keep the row
    ///   fresh.
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

    /// Fetch every `object_index` row that belongs to `repo_id`.
    ///
    /// Boundary conditions:
    /// - Returns an empty vector for an unknown `repo_id`; this is treated as "no
    ///   prior backup".
    pub async fn get_object_indexes(&self, repo_id: &str) -> Result<Vec<ObjectIndexRow>, D1Error> {
        let sql = "SELECT o_id, o_type, o_size, repo_id, created_at, is_synced FROM object_index WHERE repo_id = ?1";
        self.query(sql, Some(vec![serde_json::json!(repo_id)]))
            .await
    }

    /// Create the `repositories` table on the D1 side if it does not already exist.
    ///
    /// Boundary conditions:
    /// - Idempotent — safe to call on every backup.
    /// - The `name` column is `UNIQUE`; this is what enables the conflict-detection
    ///   path inside [`Self::upsert_repository`].
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

    /// Upsert a repository row, gracefully resolving name conflicts.
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
    ///
    /// Boundary conditions:
    /// - The conflict path string-matches both `UNIQUE constraint failed:
    ///   repositories.name` and the more generic `SQLITE_CONSTRAINT` so that
    ///   wording differences across D1/SQLite versions do not cause a regression.
    /// - Returns `D1Error` code `3002` if the upsert returns no row (D1 docs allow
    ///   `RETURNING` to come back empty in degenerate cases).
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

    /// Look up a repository's `repo_id` by its human-readable name.
    ///
    /// Boundary conditions:
    /// - Returns `Ok(None)` when no row matches; only forwards database-level
    ///   errors as `Err`.
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

/// One row of the `object_index` table.
///
/// Mirrors the on-disk SQLite columns one-to-one so that local and remote rows can
/// be diffed without translation.
#[derive(Debug, Deserialize, Serialize)]
pub struct ObjectIndexRow {
    pub o_id: String,
    pub o_type: String,
    pub o_size: i64,
    pub repo_id: String,
    pub created_at: i64,
    /// `0` when only stored locally; `1` once synced to D1.
    pub is_synced: i32,
}

/// One row of the `repositories` table.
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

    /// RAII guard that removes an env var on construction and restores it on drop.
    /// Local copy of the helper used elsewhere — kept self-contained so the test
    /// module here has no cross-module dependency on `client_storage.rs`.
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

    /// Scenario: a `D1Statement` with parameters must serialise both `sql` and
    /// `params` fields. Pins the wire format so an accidental `serde` rename does
    /// not silently break the Cloudflare API contract.
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

    /// Scenario: a `D1Statement` without parameters must omit the `params` key
    /// entirely. The single-statement `/query` endpoint rejects requests where
    /// `params` is present but null, so omission is required, not optional.
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

    /// Scenario: with all three D1 env vars unset and the local repo config holding
    /// the values, `from_env` should successfully build a client. This is the
    /// happy path users follow when storing credentials in `vault.env.*` rather
    /// than in their shell profile.
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

    /// Scenario: when `LIBRA_CONFIG_GLOBAL_DB` points at a corrupt file, the
    /// resolver should emit a `1101`-coded error rather than silently degrading to
    /// "missing variable". This pins the contract that lets the cloud-backup
    /// command surface actionable errors.
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
            err.message.contains("failed to open config database")
                || err.message.contains("failed to connect to global config"),
            "unexpected error: {}",
            err.message
        );
    }
}
