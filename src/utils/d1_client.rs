//! Cloudflare D1 REST API client for database backup and synchronization.
//!
//! This module provides a client for interacting with Cloudflare D1 database
//! via the REST API. It supports executing SQL statements, querying data,
//! and batch operations for efficient cloud backup.
//!
//! Authentication uses a Cloudflare account ID, API token, and D1 database ID,
//! resolved through [`resolve_env`](crate::internal::config::resolve_env) so they
//! can come from local repo `vault.env.*` config, global config, or process env.
//! The default client speaks HTTPS to `api.cloudflare.com`; the constructor enforces
//! `https_only(true)`. Tests can inject a different API base URL through
//! [`D1Client::new_with_api_base_url`].
//!
//! Schema management is conservative: [`D1Client::ensure_object_index_table`]
//! migrates an older single-column unique index into a composite `(repo_id, o_id)`
//! unique index when it detects the legacy shape, so users upgrading from older
//! Libra versions do not need to drop their D1 backup database manually.

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

use crate::{
    internal::config::resolve_env,
    utils::backoff::{RetryOutcome, RetryPolicy, parse_retry_after, retry_idempotent},
};

const DEFAULT_D1_API_BASE_URL: &str = "https://api.cloudflare.com/client/v4";

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

/// Build a redacted, human-readable message from a reqwest transport error.
///
/// Deliberately avoids the reqwest `Debug` output (which is verbose and can
/// embed request internals) and never includes the full URL — only the failure
/// class and the host, which is safe to log.
fn redact_request_error(err: &reqwest::Error) -> String {
    let host = err
        .url()
        .and_then(|url| url.host_str())
        .unwrap_or("<unknown host>");
    if err.is_timeout() {
        format!("HTTP request to D1 host {host} timed out")
    } else if err.is_connect() {
        format!("failed to connect to D1 host {host}")
    } else {
        format!("HTTP request to D1 host {host} failed")
    }
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

/// CAS update input for `publish_sites.latest_revision_oid`.
pub struct PublishSiteLatestUpdate<'a> {
    pub site_id: &'a str,
    pub default_ref: Option<&'a str>,
    pub latest_revision_oid: Option<&'a str>,
    pub next_refs_generation: i64,
    pub expected_refs_generation: i64,
    pub updated_at: &'a str,
    pub force: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PublishSiteLatestUpdateResult {
    Updated,
    Conflict,
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
    api_base_url: Url,
}

impl D1Client {
    /// Create a new D1 client from environment variables, using
    /// [`resolve_env`](crate::internal::config::resolve_env) so that
    /// vault-stored secrets are picked up automatically.
    ///
    /// Resolution order per variable:
    /// 1. Local vault config (`vault.env.<VAR>`)
    /// 2. Global vault config (`~/.libra/config.db`)
    /// 3. System environment variable (`std::env::var`)
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
                message: format!(
                    "{name} is not configured; set vault.env.{name} with `libra config set \
                     vault.env.{name} <value>` or export {name}"
                ),
            }),
            Err(err) => Err(D1Error {
                code: resolution_error_code,
                message: format!("failed to resolve {name} from vault config or env: {err}"),
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
        // INVARIANT: the default Cloudflare API base URL is a checked-in constant.
        let api_base_url = Url::parse(DEFAULT_D1_API_BASE_URL)
            .expect("DEFAULT_D1_API_BASE_URL must be a valid URL");
        Self::with_api_base_url(account_id, api_token, database_id, api_base_url, true)
    }

    /// Create a D1 client that targets a custom Cloudflare-compatible API base URL.
    ///
    /// This seam is intended for local tests that provide a mock D1 endpoint. The
    /// supplied base URL should point at the Cloudflare API root, e.g.
    /// `http://127.0.0.1:8787/client/v4`; the client appends
    /// `/accounts/<account>/d1/database/<db>/query`.
    pub fn new_with_api_base_url(
        account_id: String,
        api_token: String,
        database_id: String,
        api_base_url: &str,
    ) -> Result<Self, D1Error> {
        let api_base_url = Url::parse(api_base_url).map_err(|e| D1Error {
            code: 2005,
            message: format!("Invalid API base URL: {}", e),
        })?;
        Ok(Self::with_api_base_url(
            account_id,
            api_token,
            database_id,
            api_base_url,
            false,
        ))
    }

    fn with_api_base_url(
        account_id: String,
        api_token: String,
        database_id: String,
        api_base_url: Url,
        https_only: bool,
    ) -> Self {
        let mut builder = Client::builder();
        if https_only {
            builder = builder.https_only(true);
        }
        let client = builder.build().unwrap_or_else(|_| Client::new());
        Self {
            client,
            account_id,
            api_token,
            database_id,
            api_base_url,
        }
    }

    /// Build the per-request `/query` endpoint URL for this account/database.
    ///
    /// Boundary conditions:
    /// - Returns a `D1Error` with code `2005` when appending the account/database
    ///   path to the configured API base URL fails.
    fn api_url(&self) -> Result<Url, D1Error> {
        let mut url = self.api_base_url.clone();
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        url = url
            .join(&format!(
                "accounts/{}/d1/database/{}/query",
                self.account_id, self.database_id
            ))
            .map_err(|e| D1Error {
                code: 2005,
                message: format!("Invalid API URL: {}", e),
            })?;

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

        // D1 writes here are all UPSERTs (`INSERT OR REPLACE` / `ON CONFLICT DO
        // UPDATE`) and reads are pure `SELECT`s, so replaying a statement the
        // server never executed is safe. We therefore retry only when the
        // request provably did not mutate state: a connection that never
        // completed, or an `HTTP 429`/`503` rejection. `Retry-After` is honoured
        // and clamped by the policy. See `docs/development/gap/lore.md` §0.2.
        let policy = RetryPolicy::default();
        let client = &self.client;
        let token = &self.api_token;
        let url_ref = &url;
        let statement_ref = &statement;

        retry_idempotent(&policy, move |_attempt| async move {
            let send_result = client
                .post(url_ref.clone())
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", "application/json")
                .json(statement_ref)
                .send()
                .await;

            let response = match send_result {
                Ok(response) => response,
                Err(err) => {
                    // Never surface the reqwest `Debug`/full URL (which can leak
                    // request internals); report the failure class and host only.
                    let message = redact_request_error(&err);
                    // A connection-level failure means the request never reached
                    // the server, so retrying cannot double-apply a write.
                    if err.is_connect() {
                        return RetryOutcome::Retry {
                            retry_after: None,
                            last_err: D1Error {
                                code: 2001,
                                message,
                            },
                        };
                    }
                    return RetryOutcome::Done(Err(D1Error {
                        code: 2001,
                        message,
                    }));
                }
            };

            let status = response.status();
            // 429/503 mean the server rate-limited or was unavailable and did
            // NOT execute the statement, so retrying is safe.
            if matches!(status.as_u16(), 429 | 503) {
                let retry_after = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_retry_after);
                return RetryOutcome::Retry {
                    retry_after,
                    last_err: D1Error {
                        code: i32::from(status.as_u16()),
                        message: format!(
                            "D1 API rate-limited or unavailable (HTTP {})",
                            status.as_u16()
                        ),
                    },
                };
            }

            let body = match response.text().await {
                Ok(body) => body,
                Err(err) => {
                    // reqwest's Display can embed the full request URL; route it
                    // through the same host-only redactor as the send path.
                    return RetryOutcome::Done(Err(D1Error {
                        code: 2002,
                        message: redact_request_error(&err),
                    }));
                }
            };

            if !status.is_success() {
                // Do NOT echo the response body: it can carry SQL fragments,
                // identifiers, or backend detail that must not reach logs/errors.
                return RetryOutcome::Done(Err(D1Error {
                    code: i32::from(status.as_u16()),
                    message: format!("D1 API error (HTTP {})", status.as_u16()),
                }));
            }

            let d1_response: D1Response<serde_json::Value> = match serde_json::from_str(&body) {
                Ok(parsed) => parsed,
                Err(err) => {
                    // Report the parse error only, not the raw body.
                    return RetryOutcome::Done(Err(D1Error {
                        code: 2003,
                        message: format!("Failed to parse D1 response: {}", err),
                    }));
                }
            };

            if !d1_response.success {
                let error_msg = d1_response
                    .errors
                    .first()
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "Unknown D1 error".to_string());
                return RetryOutcome::Done(Err(D1Error {
                    code: d1_response.errors.first().map(|e| e.code).unwrap_or(3000),
                    message: error_msg,
                }));
            }

            RetryOutcome::Done(
                d1_response
                    .result
                    .and_then(|r| r.into_iter().next())
                    .ok_or_else(|| D1Error {
                        code: 3001,
                        message: "Empty result from D1".to_string(),
                    }),
            )
        })
        .await
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

    /// Look up a repository row by stable `repo_id`.
    ///
    /// Cloudflare clone restore uses the `publish_sites.repo_id`
    /// binding to verify that the backing backup metadata exists
    /// before it starts creating a local repository.
    pub async fn find_repository(&self, repo_id: &str) -> Result<Option<RepositoryRow>, D1Error> {
        let sql =
            "SELECT repo_id, name, created_at, updated_at FROM repositories WHERE repo_id = ?1";
        let rows: Vec<RepositoryRow> = self
            .query(sql, Some(vec![serde_json::json!(repo_id)]))
            .await?;
        Ok(rows.into_iter().next())
    }

    // ── CEX-EntireIO §10.2: agent_session / agent_checkpoint mirroring ──

    /// Create the `agent_session` table on the D1 side.
    ///
    /// Mirrors a subset of the local SQLite schema (see
    /// `sql/migrations/2026050303_agent_capture.sql`) — only the columns
    /// `libra cloud sync` needs to round-trip a session listing on a fresh
    /// machine. The `agent_kind` CHECK matches the local CHECK so a
    /// future widening migration on either side stays in lock-step.
    ///
    /// **Intentional divergences from the local schema** (operators
    /// debugging cloud-vs-local drift should know about these). Each
    /// bullet names the responsible team and the planned revisit window
    /// so a future operator can chase the right thread:
    ///
    /// - **No FK to `ai_thread(thread_id)`**. D1 does not host the
    ///   `ai_thread` table; `thread_id` is always NULL in v1
    ///   (`docs/development/commands/_general.md` §11.3). **Owner**: cloud-sync
    ///   path (this module). **Revisit**: Phase 4 migration that
    ///   replicates `ai_thread` to D1; until then, treat any non-NULL
    ///   `thread_id` rows as a local-only join key.
    /// - **No `ON DELETE CASCADE`** between session and checkpoint.
    ///   D1 typically does not enforce FKs, so cascades would be a
    ///   no-op even if declared. Orphan-row reconciliation is the
    ///   caller's responsibility — `libra agent clean` handles the
    ///   local side, and a future Phase 3 follow-up will add the D1
    ///   side.
    /// - **No payload size cap on `metadata_json` / `redaction_report`**.
    ///   D1 has its own row-size cap; we rely on the local
    ///   `Redactor::DEFAULT_RULES` keeping these blobs small in
    ///   practice. **Owner**: redaction module
    ///   (`observed_agents::redaction`). **Revisit**: Phase 4 if D1
    ///   row-size violations are observed in production sync logs;
    ///   Phase 3 already telemeters bytes_redacted via the report so
    ///   the trigger condition is observable from the agent_session
    ///   row itself.
    ///
    /// Idempotent — safe to call on every backup.
    pub async fn ensure_agent_session_table(&self) -> Result<(), D1Error> {
        let sql = r#"
            CREATE TABLE IF NOT EXISTS agent_session (
                session_id TEXT NOT NULL,
                repo_id TEXT NOT NULL,
                agent_kind TEXT NOT NULL CHECK(agent_kind IN (
                    'claude_code', 'cursor', 'codex', 'gemini',
                    'opencode', 'copilot', 'factory_ai'
                )),
                provider_session_id TEXT NOT NULL,
                state TEXT NOT NULL,
                working_dir TEXT NOT NULL,
                worktree_id TEXT,
                parent_commit TEXT,
                parent_session_id TEXT,
                metadata_json TEXT NOT NULL DEFAULT '{}',
                redaction_report TEXT NOT NULL DEFAULT '{}',
                started_at INTEGER NOT NULL,
                last_event_at INTEGER NOT NULL,
                stopped_at INTEGER,
                schema_version INTEGER NOT NULL DEFAULT 1,
                synced_at INTEGER NOT NULL,
                PRIMARY KEY (repo_id, session_id)
            )
        "#;
        self.execute(sql, None).await?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS idx_d1_agent_session_repo ON agent_session (repo_id)",
            None,
        )
        .await?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS idx_d1_agent_session_kind \
             ON agent_session (repo_id, agent_kind)",
            None,
        )
        .await?;
        Ok(())
    }

    /// Create the `agent_checkpoint` table on the D1 side.
    ///
    /// As with `agent_session`, this mirrors the local-side schema minus
    /// the FK constraint (`ON DELETE CASCADE` from session → checkpoint
    /// would require D1 to enforce FKs, which the host typically does
    /// not). Cleanup of orphan checkpoints is therefore the caller's
    /// responsibility — `libra agent clean` handles this on the local
    /// side; D1 garbage rows would persist until a future
    /// `libra cloud sync` reconciliation.
    ///
    /// Idempotent — safe to call on every backup.
    pub async fn ensure_agent_checkpoint_table(&self) -> Result<(), D1Error> {
        let sql = r#"
            CREATE TABLE IF NOT EXISTS agent_checkpoint (
                checkpoint_id TEXT NOT NULL,
                repo_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                parent_checkpoint_id TEXT,
                scope TEXT NOT NULL CHECK(scope IN ('temporary','committed','subagent')),
                parent_commit TEXT,
                tree_oid TEXT NOT NULL,
                metadata_blob_oid TEXT NOT NULL,
                traces_commit TEXT NOT NULL,
                tool_use_id TEXT,
                subagent_session_id TEXT,
                description TEXT,
                created_at INTEGER NOT NULL,
                synced_at INTEGER NOT NULL,
                PRIMARY KEY (repo_id, checkpoint_id)
            )
        "#;
        self.execute(sql, None).await?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS idx_d1_agent_checkpoint_session \
             ON agent_checkpoint (repo_id, session_id, created_at)",
            None,
        )
        .await?;
        self.execute(
            "CREATE INDEX IF NOT EXISTS idx_d1_agent_checkpoint_scope \
             ON agent_checkpoint (repo_id, scope)",
            None,
        )
        .await?;
        Ok(())
    }

    /// Upsert one `agent_session` row keyed by `(repo_id, session_id)`.
    ///
    /// On conflict the latest local view wins — `last_event_at`,
    /// `stopped_at`, `state`, and `redaction_report` are overwritten so
    /// repeated `libra cloud sync` runs converge to whatever the local
    /// SQLite has now.
    ///
    /// `synced_at` is stamped server-side via `strftime('%s', 'now')` so
    /// multi-machine clock skew between Libra clients does not poison the
    /// observability column. Codex Phase-3.5 review #Q3.
    pub async fn upsert_agent_session(
        &self,
        repo_id: &str,
        row: &AgentSessionRow,
    ) -> Result<(), D1Error> {
        let sql = r#"
            INSERT INTO agent_session (
                session_id, repo_id, agent_kind, provider_session_id, state, working_dir,
                worktree_id, parent_commit, parent_session_id, metadata_json,
                redaction_report, started_at, last_event_at, stopped_at, schema_version,
                synced_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                CAST(strftime('%s', 'now') AS INTEGER)
            )
            ON CONFLICT(repo_id, session_id) DO UPDATE SET
                state = excluded.state,
                working_dir = excluded.working_dir,
                worktree_id = excluded.worktree_id,
                parent_commit = excluded.parent_commit,
                parent_session_id = excluded.parent_session_id,
                metadata_json = excluded.metadata_json,
                redaction_report = excluded.redaction_report,
                last_event_at = excluded.last_event_at,
                stopped_at = excluded.stopped_at,
                schema_version = excluded.schema_version,
                synced_at = CAST(strftime('%s', 'now') AS INTEGER)
        "#;
        let params = vec![
            serde_json::json!(row.session_id),
            serde_json::json!(repo_id),
            serde_json::json!(row.agent_kind),
            serde_json::json!(row.provider_session_id),
            serde_json::json!(row.state),
            serde_json::json!(row.working_dir),
            serde_json::json!(row.worktree_id),
            serde_json::json!(row.parent_commit),
            serde_json::json!(row.parent_session_id),
            serde_json::json!(row.metadata_json),
            serde_json::json!(row.redaction_report),
            serde_json::json!(row.started_at),
            serde_json::json!(row.last_event_at),
            serde_json::json!(row.stopped_at),
            serde_json::json!(row.schema_version),
        ];
        self.execute(sql, Some(params)).await?;
        Ok(())
    }

    /// Read every `agent_session` row for a repo. Used by
    /// `libra cloud restore` to repopulate the local catalog on a fresh
    /// machine — paired with [`Self::upsert_agent_session`] so a
    /// round-trip preserves shape.
    pub async fn list_agent_sessions(
        &self,
        repo_id: &str,
    ) -> Result<Vec<AgentSessionRow>, D1Error> {
        let sql = r#"
            SELECT session_id, agent_kind, provider_session_id, state, working_dir,
                   worktree_id, parent_commit, parent_session_id, metadata_json,
                   redaction_report, started_at, last_event_at, stopped_at, schema_version
            FROM agent_session
            WHERE repo_id = ?1
        "#;
        self.query(sql, Some(vec![serde_json::json!(repo_id)]))
            .await
    }

    /// Read every `agent_checkpoint` row for a repo. Used by
    /// `libra cloud restore` together with
    /// [`Self::list_agent_sessions`].
    pub async fn list_agent_checkpoints(
        &self,
        repo_id: &str,
    ) -> Result<Vec<AgentCheckpointRow>, D1Error> {
        let sql = r#"
            SELECT checkpoint_id, session_id, parent_checkpoint_id, scope, parent_commit,
                   tree_oid, metadata_blob_oid, traces_commit, tool_use_id,
                   subagent_session_id, description, created_at
            FROM agent_checkpoint
            WHERE repo_id = ?1
            ORDER BY created_at ASC
        "#;
        self.query(sql, Some(vec![serde_json::json!(repo_id)]))
            .await
    }

    /// Upsert one `agent_checkpoint` row keyed by `(repo_id, checkpoint_id)`.
    ///
    /// `synced_at` is stamped server-side via `strftime('%s', 'now')` for
    /// the same reason as [`Self::upsert_agent_session`].
    pub async fn upsert_agent_checkpoint(
        &self,
        repo_id: &str,
        row: &AgentCheckpointRow,
    ) -> Result<(), D1Error> {
        let sql = r#"
            INSERT INTO agent_checkpoint (
                checkpoint_id, repo_id, session_id, parent_checkpoint_id, scope,
                parent_commit, tree_oid, metadata_blob_oid, traces_commit, tool_use_id,
                subagent_session_id, description, created_at, synced_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                CAST(strftime('%s', 'now') AS INTEGER)
            )
            ON CONFLICT(repo_id, checkpoint_id) DO UPDATE SET
                session_id = excluded.session_id,
                parent_checkpoint_id = excluded.parent_checkpoint_id,
                scope = excluded.scope,
                parent_commit = excluded.parent_commit,
                tree_oid = excluded.tree_oid,
                metadata_blob_oid = excluded.metadata_blob_oid,
                traces_commit = excluded.traces_commit,
                tool_use_id = excluded.tool_use_id,
                subagent_session_id = excluded.subagent_session_id,
                description = excluded.description,
                created_at = excluded.created_at,
                synced_at = CAST(strftime('%s', 'now') AS INTEGER)
        "#;
        let params = vec![
            serde_json::json!(row.checkpoint_id),
            serde_json::json!(repo_id),
            serde_json::json!(row.session_id),
            serde_json::json!(row.parent_checkpoint_id),
            serde_json::json!(row.scope),
            serde_json::json!(row.parent_commit),
            serde_json::json!(row.tree_oid),
            serde_json::json!(row.metadata_blob_oid),
            serde_json::json!(row.traces_commit),
            serde_json::json!(row.tool_use_id),
            serde_json::json!(row.subagent_session_id),
            serde_json::json!(row.description),
            serde_json::json!(row.created_at),
        ];
        self.execute(sql, Some(params)).await?;
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────
    // Phase 2 (publish.md) — D1 publish schema + upsert/list.
    //
    // The publish schema source-of-truth lives at
    // `sql/publish/0001_publish.sql` + later migrations under
    // `sql/publish/`. `ensure_publish_schema` reads every `*.sql`
    // file via `include_str!` and applies them in numeric order;
    // each statement is run individually because D1's REST `execute`
    // does not accept multi-statement payloads.
    //
    // Upsert/list helpers below are the typed access surface the
    // CLI snapshot builder (Phase 3) and the publish CLI (Phase 4)
    // call into. They never `println!`/`eprintln!` — the caller
    // owns user-facing output.
    // ─────────────────────────────────────────────────────────────

    /// Apply every publish schema migration in `sql/publish/` to the
    /// remote D1 database. Idempotent — every migration uses
    /// `CREATE TABLE IF NOT EXISTS` / `CREATE TRIGGER IF NOT
    /// EXISTS` / `DROP TRIGGER IF EXISTS` so repeat calls converge
    /// to the same state. Phase 6+7 reviewers (passes 6 + 11)
    /// pinned the migration chain via the
    /// `publish_schema_contract_worker_mirror_is_byte_equal` test;
    /// the strings below MUST stay byte-equal mirrors of the on-
    /// disk SQL files, which is enforced by `include_str!`.
    pub async fn ensure_publish_schema(&self) -> Result<(), D1Error> {
        // Order matches numeric migration prefix.
        let migrations: &[(&str, &str)] = &[
            (
                "0001_publish.sql",
                include_str!("../../sql/publish/0001_publish.sql"),
            ),
            (
                "0002_publish_digest_check.sql",
                include_str!("../../sql/publish/0002_publish_digest_check.sql"),
            ),
            (
                "0003_publish_max_preview_trigger_replace.sql",
                include_str!("../../sql/publish/0003_publish_max_preview_trigger_replace.sql"),
            ),
            (
                "0004_publish_refs_index.sql",
                include_str!("../../sql/publish/0004_publish_refs_index.sql"),
            ),
        ];
        for (label, sql) in migrations {
            for statement in split_sql_statements(sql) {
                self.execute(&statement, None).await.map_err(|e| D1Error {
                    code: e.code,
                    message: format!(
                        "publish migration {label} failed at statement {statement:?}: {}",
                        e.message
                    ),
                })?;
            }
        }
        Ok(())
    }

    /// Insert or update a `publish_sites` row.
    ///
    /// `default_ref` and `latest_revision_oid` may be NULL on first
    /// insert (the chicken-and-egg insert order described in
    /// `sql/publish/0001_publish.sql`); update them in a follow-up
    /// call once the refs/revisions exist.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_publish_site(&self, row: &PublishSiteRow) -> Result<(), D1Error> {
        let sql = r#"
            INSERT INTO publish_sites (
                site_id, repo_id, clone_domain, slug, display_origin,
                name, visibility, status, worker_name, default_ref,
                latest_revision_oid, refs_generation, max_preview_bytes,
                schema_version, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            ON CONFLICT(site_id) DO UPDATE SET
                repo_id = excluded.repo_id,
                clone_domain = excluded.clone_domain,
                slug = excluded.slug,
                display_origin = excluded.display_origin,
                name = excluded.name,
                visibility = excluded.visibility,
                status = excluded.status,
                worker_name = excluded.worker_name,
                default_ref = excluded.default_ref,
                latest_revision_oid = excluded.latest_revision_oid,
                refs_generation = excluded.refs_generation,
                max_preview_bytes = excluded.max_preview_bytes,
                schema_version = excluded.schema_version,
                updated_at = excluded.updated_at
        "#;
        let params = vec![
            serde_json::json!(row.site_id),
            serde_json::json!(row.repo_id),
            serde_json::json!(row.clone_domain),
            serde_json::json!(row.slug),
            serde_json::json!(row.display_origin),
            serde_json::json!(row.name),
            serde_json::json!(row.visibility),
            serde_json::json!(row.status),
            serde_json::json!(row.worker_name),
            serde_json::json!(row.default_ref),
            serde_json::json!(row.latest_revision_oid),
            serde_json::json!(row.refs_generation),
            serde_json::json!(row.max_preview_bytes),
            serde_json::json!(row.schema_version),
            serde_json::json!(row.created_at),
            serde_json::json!(row.updated_at),
        ];
        self.execute(sql, Some(params)).await?;
        Ok(())
    }

    /// CAS-update a site's latest/default publish pointers.
    ///
    /// Without `force`, the update only applies when the stored
    /// `refs_generation` matches `expected_refs_generation`; a
    /// zero-row update is reported as [`PublishSiteLatestUpdateResult::Conflict`]
    /// so callers can surface a clear retry-or-`--force` error. With
    /// `force`, the generation guard is intentionally omitted.
    pub async fn update_publish_site_latest(
        &self,
        update: PublishSiteLatestUpdate<'_>,
    ) -> Result<PublishSiteLatestUpdateResult, D1Error> {
        let D1Statement { sql, params } = publish_site_latest_update_statement(&update);
        let result = self.execute(&sql, params).await?;
        Ok(publish_site_latest_update_result_from_changes(
            result.meta.and_then(|meta| meta.changes).unwrap_or(0),
        ))
    }

    /// Insert or update a `publish_sync_runs` row.
    pub async fn upsert_publish_sync_run(&self, row: &PublishSyncRunRow) -> Result<(), D1Error> {
        let sql = r#"
            INSERT INTO publish_sync_runs (
                sync_run_id, site_id, status, started_at, finished_at,
                refs_count, revision_count, file_count, ai_object_count,
                ai_bundle_count, warnings_json, error_message,
                cli_version, schema_version
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(sync_run_id) DO UPDATE SET
                status = excluded.status,
                finished_at = excluded.finished_at,
                refs_count = excluded.refs_count,
                revision_count = excluded.revision_count,
                file_count = excluded.file_count,
                ai_object_count = excluded.ai_object_count,
                ai_bundle_count = excluded.ai_bundle_count,
                warnings_json = excluded.warnings_json,
                error_message = excluded.error_message,
                schema_version = excluded.schema_version
        "#;
        let params = vec![
            serde_json::json!(row.sync_run_id),
            serde_json::json!(row.site_id),
            serde_json::json!(row.status),
            serde_json::json!(row.started_at),
            serde_json::json!(row.finished_at),
            serde_json::json!(row.refs_count),
            serde_json::json!(row.revision_count),
            serde_json::json!(row.file_count),
            serde_json::json!(row.ai_object_count),
            serde_json::json!(row.ai_bundle_count),
            serde_json::json!(row.warnings_json),
            serde_json::json!(row.error_message),
            serde_json::json!(row.cli_version),
            serde_json::json!(row.schema_version),
        ];
        self.execute(sql, Some(params)).await?;
        Ok(())
    }

    /// Insert or update a `publish_revisions` row.
    pub async fn upsert_publish_revision(&self, row: &PublishRevisionRow) -> Result<(), D1Error> {
        let sql = r#"
            INSERT INTO publish_revisions (
                site_id, revision_oid, status, code_manifest_key, ai_index_key,
                file_count, ai_object_count, ai_bundle_count, redaction_mode,
                redaction_rules_version, sync_run_id, schema_version,
                created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(site_id, revision_oid) DO UPDATE SET
                status = excluded.status,
                code_manifest_key = excluded.code_manifest_key,
                ai_index_key = excluded.ai_index_key,
                file_count = excluded.file_count,
                ai_object_count = excluded.ai_object_count,
                ai_bundle_count = excluded.ai_bundle_count,
                redaction_mode = excluded.redaction_mode,
                redaction_rules_version = excluded.redaction_rules_version,
                sync_run_id = excluded.sync_run_id,
                schema_version = excluded.schema_version,
                updated_at = excluded.updated_at
        "#;
        let params = vec![
            serde_json::json!(row.site_id),
            serde_json::json!(row.revision_oid),
            serde_json::json!(row.status),
            serde_json::json!(row.code_manifest_key),
            serde_json::json!(row.ai_index_key),
            serde_json::json!(row.file_count),
            serde_json::json!(row.ai_object_count),
            serde_json::json!(row.ai_bundle_count),
            serde_json::json!(row.redaction_mode),
            serde_json::json!(row.redaction_rules_version),
            serde_json::json!(row.sync_run_id),
            serde_json::json!(row.schema_version),
            serde_json::json!(row.created_at),
            serde_json::json!(row.updated_at),
        ];
        self.execute(sql, Some(params)).await?;
        Ok(())
    }

    /// Insert or update a `publish_refs` row.
    pub async fn upsert_publish_ref(&self, row: &PublishRefRow) -> Result<(), D1Error> {
        let sql = r#"
            INSERT INTO publish_refs (
                site_id, ref_name, ref_type, short_name, target_oid,
                revision_oid, is_default, sync_run_id, schema_version, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(site_id, ref_name) DO UPDATE SET
                ref_type = excluded.ref_type,
                short_name = excluded.short_name,
                target_oid = excluded.target_oid,
                revision_oid = excluded.revision_oid,
                is_default = excluded.is_default,
                sync_run_id = excluded.sync_run_id,
                schema_version = excluded.schema_version,
                updated_at = excluded.updated_at
        "#;
        let params = vec![
            serde_json::json!(row.site_id),
            serde_json::json!(row.ref_name),
            serde_json::json!(row.ref_type),
            serde_json::json!(row.short_name),
            serde_json::json!(row.target_oid),
            serde_json::json!(row.revision_oid),
            serde_json::json!(row.is_default),
            serde_json::json!(row.sync_run_id),
            serde_json::json!(row.schema_version),
            serde_json::json!(row.updated_at),
        ];
        self.execute(sql, Some(params)).await?;
        Ok(())
    }

    /// Delete stale `publish_refs` rows left by older all-refs sync runs.
    ///
    /// Callers run this only after a full sync has upserted every current
    /// local branch/tag ref with `current_sync_run_id` and after the site
    /// latest/default CAS succeeds. That order prevents deleting the previous
    /// default ref while `publish_sites.default_ref` still points at it.
    pub async fn delete_publish_refs_for_other_sync_runs(
        &self,
        site_id: &str,
        current_sync_run_id: &str,
    ) -> Result<i64, D1Error> {
        let D1Statement { sql, params } =
            delete_publish_refs_for_other_sync_runs_statement(site_id, current_sync_run_id);
        let result = self.execute(&sql, params).await?;
        Ok(result.meta.and_then(|meta| meta.changes).unwrap_or(0))
    }

    /// Insert or update a `publish_files` row.
    pub async fn upsert_publish_file(&self, row: &PublishFileRow) -> Result<(), D1Error> {
        let sql = r#"
            INSERT INTO publish_files (
                site_id, revision_oid, path, display_mode, content_sha256,
                r2_key, size_bytes, language, schema_version
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(site_id, revision_oid, path) DO UPDATE SET
                display_mode = excluded.display_mode,
                content_sha256 = excluded.content_sha256,
                r2_key = excluded.r2_key,
                size_bytes = excluded.size_bytes,
                language = excluded.language,
                schema_version = excluded.schema_version
        "#;
        let params = vec![
            serde_json::json!(row.site_id),
            serde_json::json!(row.revision_oid),
            serde_json::json!(row.path),
            serde_json::json!(row.display_mode),
            serde_json::json!(row.content_sha256),
            serde_json::json!(row.r2_key),
            serde_json::json!(row.size_bytes),
            serde_json::json!(row.language),
            serde_json::json!(row.schema_version),
        ];
        self.execute(sql, Some(params)).await?;
        Ok(())
    }

    /// Insert or update a `publish_ai_objects` row.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_publish_ai_object(&self, row: &PublishAiObjectRow) -> Result<(), D1Error> {
        let sql = r#"
            INSERT INTO publish_ai_objects (
                site_id, revision_oid, object_type, object_id, layer,
                r2_key, redaction_mode, payload_sha256, schema_version, created_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(site_id, revision_oid, object_type, object_id) DO UPDATE SET
                layer = excluded.layer,
                r2_key = excluded.r2_key,
                redaction_mode = excluded.redaction_mode,
                payload_sha256 = excluded.payload_sha256,
                schema_version = excluded.schema_version
        "#;
        let params = vec![
            serde_json::json!(row.site_id),
            serde_json::json!(row.revision_oid),
            serde_json::json!(row.object_type),
            serde_json::json!(row.object_id),
            serde_json::json!(row.layer),
            serde_json::json!(row.r2_key),
            serde_json::json!(row.redaction_mode),
            serde_json::json!(row.payload_sha256),
            serde_json::json!(row.schema_version),
            serde_json::json!(row.created_at),
        ];
        self.execute(sql, Some(params)).await?;
        Ok(())
    }

    /// List AI object rows for one published revision.
    pub async fn list_publish_ai_objects(
        &self,
        site_id: &str,
        revision_oid: &str,
    ) -> Result<Vec<PublishAiObjectRow>, D1Error> {
        let sql = r#"
            SELECT site_id, revision_oid, object_type, object_id, layer,
                   r2_key, redaction_mode, payload_sha256, schema_version, created_at
              FROM publish_ai_objects
             WHERE site_id = ?1 AND revision_oid = ?2
             ORDER BY layer, object_type, object_id
        "#;
        self.query(
            sql,
            Some(vec![
                serde_json::json!(site_id),
                serde_json::json!(revision_oid),
            ]),
        )
        .await
    }

    /// List AI version rows for one published revision.
    pub async fn list_publish_ai_versions(
        &self,
        site_id: &str,
        revision_oid: &str,
    ) -> Result<Vec<PublishAiVersionRow>, D1Error> {
        let sql = r#"
            SELECT site_id, ai_version_id, revision_oid, bundle_key, bundle_sha256,
                   object_count, redaction_mode, redaction_rules_version,
                   schema_version, created_at
              FROM publish_ai_versions
             WHERE site_id = ?1 AND revision_oid = ?2
             ORDER BY created_at, ai_version_id
        "#;
        self.query(
            sql,
            Some(vec![
                serde_json::json!(site_id),
                serde_json::json!(revision_oid),
            ]),
        )
        .await
    }

    /// Insert or update a `publish_ai_versions` row.
    pub async fn upsert_publish_ai_version(
        &self,
        row: &PublishAiVersionRow,
    ) -> Result<(), D1Error> {
        let sql = r#"
            INSERT INTO publish_ai_versions (
                site_id, ai_version_id, revision_oid, bundle_key, bundle_sha256,
                object_count, redaction_mode, redaction_rules_version,
                schema_version, created_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(site_id, ai_version_id) DO UPDATE SET
                revision_oid = excluded.revision_oid,
                bundle_key = excluded.bundle_key,
                bundle_sha256 = excluded.bundle_sha256,
                object_count = excluded.object_count,
                redaction_mode = excluded.redaction_mode,
                redaction_rules_version = excluded.redaction_rules_version,
                schema_version = excluded.schema_version
        "#;
        let params = vec![
            serde_json::json!(row.site_id),
            serde_json::json!(row.ai_version_id),
            serde_json::json!(row.revision_oid),
            serde_json::json!(row.bundle_key),
            serde_json::json!(row.bundle_sha256),
            serde_json::json!(row.object_count),
            serde_json::json!(row.redaction_mode),
            serde_json::json!(row.redaction_rules_version),
            serde_json::json!(row.schema_version),
            serde_json::json!(row.created_at),
        ];
        self.execute(sql, Some(params)).await?;
        Ok(())
    }

    /// List all `publish_refs` rows for one site.
    pub async fn list_publish_refs(&self, site_id: &str) -> Result<Vec<PublishRefRow>, D1Error> {
        let sql = "SELECT site_id, ref_name, ref_type, short_name, target_oid, \
                          revision_oid, is_default, sync_run_id, schema_version, updated_at \
                   FROM publish_refs WHERE site_id = ?1 \
                   ORDER BY ref_type, short_name";
        self.query(sql, Some(vec![serde_json::json!(site_id)]))
            .await
    }

    /// Find one revision row by `(site_id, revision_oid)`, regardless
    /// of status. Used for state inspection (publish status command);
    /// the Worker side filters `status = 'published'` separately so
    /// in-progress `syncing` rows never leak into reads.
    ///
    /// Codex Phase 2 P3 (closed): the earlier name was
    /// `find_publish_revision` which implied a published-only filter
    /// the SQL didn't have. Renamed to `find_publish_revision_any`
    /// to make the broader semantic explicit; new
    /// `find_published_revision` carries the published filter.
    pub async fn find_publish_revision_any(
        &self,
        site_id: &str,
        revision_oid: &str,
    ) -> Result<Option<PublishRevisionRow>, D1Error> {
        let sql = "SELECT site_id, revision_oid, status, code_manifest_key, ai_index_key, \
                          file_count, ai_object_count, ai_bundle_count, redaction_mode, \
                          redaction_rules_version, sync_run_id, schema_version, \
                          created_at, updated_at \
                   FROM publish_revisions WHERE site_id = ?1 AND revision_oid = ?2";
        let rows: Vec<PublishRevisionRow> = self
            .query(
                sql,
                Some(vec![
                    serde_json::json!(site_id),
                    serde_json::json!(revision_oid),
                ]),
            )
            .await?;
        Ok(rows.into_iter().next())
    }

    /// Find one revision row that is in `status = 'published'`.
    /// Mirror of the Worker-side semantic: in-flight `syncing` rows
    /// are invisible.
    pub async fn find_published_revision(
        &self,
        site_id: &str,
        revision_oid: &str,
    ) -> Result<Option<PublishRevisionRow>, D1Error> {
        let sql = "SELECT site_id, revision_oid, status, code_manifest_key, ai_index_key, \
                          file_count, ai_object_count, ai_bundle_count, redaction_mode, \
                          redaction_rules_version, sync_run_id, schema_version, \
                          created_at, updated_at \
                   FROM publish_revisions \
                   WHERE site_id = ?1 AND revision_oid = ?2 AND status = 'published'";
        let rows: Vec<PublishRevisionRow> = self
            .query(
                sql,
                Some(vec![
                    serde_json::json!(site_id),
                    serde_json::json!(revision_oid),
                ]),
            )
            .await?;
        Ok(rows.into_iter().next())
    }

    /// Find one publish_sites row by site_id.
    pub async fn find_publish_site(
        &self,
        site_id: &str,
    ) -> Result<Option<PublishSiteRow>, D1Error> {
        let sql = publish_site_select_sql("site_id = ?1");
        let rows: Vec<PublishSiteRow> = self
            .query(&sql, Some(vec![serde_json::json!(site_id)]))
            .await?;
        Ok(rows.into_iter().next())
    }

    /// Resolve a publish site by its clone-domain slug.
    ///
    /// The schema pins `(clone_domain, slug)` as unique, so callers
    /// receive at most one row. Slug lookup is the human-facing
    /// `libra+cloud://<clone-domain>/<slug>` path.
    pub async fn find_publish_site_by_clone_slug(
        &self,
        clone_domain: &str,
        slug: &str,
    ) -> Result<Option<PublishSiteRow>, D1Error> {
        let sql = publish_site_select_sql("clone_domain = ?1 AND slug = ?2");
        let rows: Vec<PublishSiteRow> = self
            .query(
                &sql,
                Some(vec![
                    serde_json::json!(clone_domain),
                    serde_json::json!(slug),
                ]),
            )
            .await?;
        Ok(rows.into_iter().next())
    }

    /// Resolve a publish site by its stable repo id.
    ///
    /// The `repo/<repo_id>` URL form survives slug renames because
    /// the schema pins `(clone_domain, repo_id)` as unique.
    pub async fn find_publish_site_by_clone_repo_id(
        &self,
        clone_domain: &str,
        repo_id: &str,
    ) -> Result<Option<PublishSiteRow>, D1Error> {
        let sql = publish_site_select_sql("clone_domain = ?1 AND repo_id = ?2");
        let rows: Vec<PublishSiteRow> = self
            .query(
                &sql,
                Some(vec![
                    serde_json::json!(clone_domain),
                    serde_json::json!(repo_id),
                ]),
            )
            .await?;
        Ok(rows.into_iter().next())
    }
}

fn publish_site_select_sql(where_clause: &str) -> String {
    format!(
        "SELECT site_id, repo_id, clone_domain, slug, display_origin, \
                name, visibility, status, worker_name, default_ref, \
                latest_revision_oid, refs_generation, max_preview_bytes, \
                schema_version, created_at, updated_at \
         FROM publish_sites WHERE {where_clause} LIMIT 1"
    )
}

fn publish_site_latest_update_statement(update: &PublishSiteLatestUpdate<'_>) -> D1Statement {
    let mut sql = "UPDATE publish_sites \
                   SET default_ref = ?2, latest_revision_oid = ?3, refs_generation = ?4, \
                       updated_at = ?5 \
                   WHERE site_id = ?1"
        .to_string();
    let mut params = vec![
        serde_json::json!(update.site_id),
        serde_json::json!(update.default_ref),
        serde_json::json!(update.latest_revision_oid),
        serde_json::json!(update.next_refs_generation),
        serde_json::json!(update.updated_at),
    ];
    if !update.force {
        sql.push_str(" AND refs_generation = ?6");
        params.push(serde_json::json!(update.expected_refs_generation));
    }
    D1Statement {
        sql,
        params: Some(params),
    }
}

fn publish_site_latest_update_result_from_changes(changes: i64) -> PublishSiteLatestUpdateResult {
    if changes > 0 {
        PublishSiteLatestUpdateResult::Updated
    } else {
        PublishSiteLatestUpdateResult::Conflict
    }
}

fn delete_publish_refs_for_other_sync_runs_statement(
    site_id: &str,
    current_sync_run_id: &str,
) -> D1Statement {
    D1Statement {
        sql: "DELETE FROM publish_refs WHERE site_id = ?1 AND sync_run_id != ?2".to_string(),
        params: Some(vec![
            serde_json::json!(site_id),
            serde_json::json!(current_sync_run_id),
        ]),
    }
}

/// Split a multi-statement SQL script into individual statements.
///
/// SQLite's REST `execute` accepts one statement per call, but the
/// publish migrations are written as multi-statement files for
/// readability. This helper splits on top-level `;` boundaries
/// while ignoring `;` inside string literals (`'…'`) and inside
/// SQL `CREATE TRIGGER … BEGIN …; …; END;` blocks. Line comments
/// (`--…\n`) and block comments (`/* … */`) are stripped so the
/// final statement count stays stable across `cargo +nightly fmt`
/// reflow.
///
/// Codex Phase 2 P1 (closed): the earlier draft processed `;`
/// before flushing the running `prev_word` into the BEGIN/END
/// counter, so `END;` collapsed an entire trigger block into a
/// single multi-statement payload. The fix flushes the pending
/// keyword on EVERY non-alphanumeric boundary (whitespace,
/// punctuation, end-of-input) before checking for `;`.
fn split_sql_statements(input: &str) -> Vec<String> {
    let mut statements: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut depth_begin_end: i32 = 0;
    let mut prev_word = String::new();

    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        // Strip line comments outside of string literals.
        if !in_string && ch == '-' && chars.peek() == Some(&'-') {
            for next_ch in chars.by_ref() {
                if next_ch == '\n' {
                    current.push('\n');
                    break;
                }
            }
            flush_keyword(&mut prev_word, &mut depth_begin_end);
            continue;
        }
        // Strip block comments outside of string literals.
        if !in_string && ch == '/' && chars.peek() == Some(&'*') {
            // Consume the leading '*'.
            chars.next();
            // Walk until we see the closing '*/'.
            while let Some(next_ch) = chars.next() {
                if next_ch == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
            current.push(' ');
            flush_keyword(&mut prev_word, &mut depth_begin_end);
            continue;
        }

        if ch == '\'' && !in_string {
            in_string = true;
            current.push(ch);
            flush_keyword(&mut prev_word, &mut depth_begin_end);
            continue;
        }
        if ch == '\'' && in_string {
            // Handle SQL `''` escape inside a string. Codex Phase 2
            // P1 (closed): use `if let Some(...)` instead of
            // `chars.next().unwrap()` so the splitter never
            // panics on a malformed input mid-stream.
            if chars.peek() == Some(&'\'') {
                current.push(ch);
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
                continue;
            }
            in_string = false;
            current.push(ch);
            flush_keyword(&mut prev_word, &mut depth_begin_end);
            continue;
        }

        if !in_string && ch.is_alphanumeric() {
            prev_word.push(ch.to_ascii_lowercase());
        } else if !in_string {
            // Codex Phase 2 P1 (closed): flush the pending keyword
            // on any non-alphanumeric boundary BEFORE we check
            // whether the current char is `;`. This means `END;`
            // increments the depth-tracker for the `END` token,
            // closing the BEGIN/END block, and the subsequent `;`
            // check sees `depth_begin_end == 0` so the trigger
            // statement closes correctly.
            flush_keyword(&mut prev_word, &mut depth_begin_end);
        }

        if ch == ';' && !in_string && depth_begin_end == 0 {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                statements.push(trimmed);
            }
            current.clear();
            continue;
        }

        current.push(ch);
    }
    // Flush any trailing keyword at end-of-input so a file that
    // ends with `END` (no trailing semicolon) still updates the
    // depth tracker before we capture the final statement.
    flush_keyword(&mut prev_word, &mut depth_begin_end);
    let trailing = current.trim().to_string();
    if !trailing.is_empty() {
        statements.push(trailing);
    }
    statements
}

/// Apply the BEGIN/END nesting effect of the keyword that just
/// ended at a word boundary, then clear the buffer.
fn flush_keyword(prev_word: &mut String, depth_begin_end: &mut i32) {
    match prev_word.as_str() {
        "begin" => *depth_begin_end += 1,
        "end" if *depth_begin_end > 0 => {
            *depth_begin_end -= 1;
        }
        _ => {}
    }
    prev_word.clear();
}

/// Local view of a `publish_sites` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishSiteRow {
    pub site_id: String,
    pub repo_id: String,
    pub clone_domain: String,
    pub slug: String,
    pub display_origin: String,
    pub name: String,
    pub visibility: String,
    pub status: String,
    pub worker_name: String,
    pub default_ref: Option<String>,
    pub latest_revision_oid: Option<String>,
    pub refs_generation: i64,
    pub max_preview_bytes: i64,
    pub schema_version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Local view of a `publish_revisions` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishRevisionRow {
    pub site_id: String,
    pub revision_oid: String,
    pub status: String,
    pub code_manifest_key: Option<String>,
    pub ai_index_key: Option<String>,
    pub file_count: i64,
    pub ai_object_count: i64,
    pub ai_bundle_count: i64,
    pub redaction_mode: String,
    pub redaction_rules_version: String,
    pub sync_run_id: String,
    pub schema_version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Local view of a `publish_refs` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishRefRow {
    pub site_id: String,
    pub ref_name: String,
    pub ref_type: String,
    pub short_name: String,
    pub target_oid: String,
    pub revision_oid: String,
    pub is_default: i64,
    pub sync_run_id: String,
    pub schema_version: i64,
    pub updated_at: String,
}

/// Local view of a `publish_files` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishFileRow {
    pub site_id: String,
    pub revision_oid: String,
    pub path: String,
    pub display_mode: String,
    pub content_sha256: Option<String>,
    pub r2_key: Option<String>,
    pub size_bytes: i64,
    pub language: Option<String>,
    pub schema_version: i64,
}

/// Local view of a `publish_ai_objects` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishAiObjectRow {
    pub site_id: String,
    pub revision_oid: String,
    pub object_type: String,
    pub object_id: String,
    pub layer: String,
    pub r2_key: String,
    pub redaction_mode: String,
    pub payload_sha256: String,
    pub schema_version: i64,
    pub created_at: String,
}

/// Local view of a `publish_ai_versions` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishAiVersionRow {
    pub site_id: String,
    pub ai_version_id: String,
    pub revision_oid: String,
    pub bundle_key: String,
    pub bundle_sha256: String,
    pub object_count: i64,
    pub redaction_mode: String,
    pub redaction_rules_version: String,
    pub schema_version: i64,
    pub created_at: String,
}

/// Local view of a `publish_sync_runs` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishSyncRunRow {
    pub sync_run_id: String,
    pub site_id: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub refs_count: i64,
    pub revision_count: i64,
    pub file_count: i64,
    pub ai_object_count: i64,
    pub ai_bundle_count: i64,
    pub warnings_json: String,
    pub error_message: Option<String>,
    pub cli_version: String,
    pub schema_version: i64,
}

/// Local view of an `agent_session` row prepared for D1 mirroring.
///
/// Field set is the same as the local SQLite schema (see
/// `sql/migrations/2026050303_agent_capture.sql`) minus the optional
/// `id`/auto-increment surrogate columns. The cloud caller (`command::cloud`)
/// builds these from a SELECT and hands them to
/// [`D1Client::upsert_agent_session`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSessionRow {
    pub session_id: String,
    pub agent_kind: String,
    pub provider_session_id: String,
    pub state: String,
    pub working_dir: String,
    pub worktree_id: Option<String>,
    pub parent_commit: Option<String>,
    pub parent_session_id: Option<String>,
    pub metadata_json: String,
    pub redaction_report: String,
    pub started_at: i64,
    pub last_event_at: i64,
    pub stopped_at: Option<i64>,
    pub schema_version: i64,
}

/// Local view of an `agent_checkpoint` row prepared for D1 mirroring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCheckpointRow {
    pub checkpoint_id: String,
    pub session_id: String,
    pub parent_checkpoint_id: Option<String>,
    pub scope: String,
    /// Nullable per the `2026050501` follow-up migration.
    pub parent_commit: Option<String>,
    pub tree_oid: String,
    pub metadata_blob_oid: String,
    pub traces_commit: String,
    pub tool_use_id: Option<String>,
    pub subagent_session_id: Option<String>,
    pub description: Option<String>,
    pub created_at: i64,
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
        utils::test::{ChangeDirGuard, ScopedEnvVar, setup_with_new_libra_in},
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

    #[test]
    fn d1_client_default_api_url_uses_cloudflare_query_endpoint() {
        let client = D1Client::new(
            "account-123".to_string(),
            "token-123".to_string(),
            "database-123".to_string(),
        );

        let url = client.api_url().expect("default D1 API URL should parse");

        assert_eq!(
            url.as_str(),
            "https://api.cloudflare.com/client/v4/accounts/account-123/d1/database/database-123/query"
        );
    }

    #[test]
    fn d1_client_custom_api_base_url_uses_query_endpoint() {
        let client = D1Client::new_with_api_base_url(
            "account-123".to_string(),
            "token-123".to_string(),
            "database-123".to_string(),
            "http://127.0.0.1:8787/client/v4",
        )
        .expect("custom D1 API base URL should parse");

        let url = client.api_url().expect("custom D1 API URL should parse");

        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:8787/client/v4/accounts/account-123/d1/database/database-123/query"
        );
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

    /// Scenario: D1 credentials follow the same priority chain as the rest of
    /// the secret surface (docs/development/commands/config.md 12-Factor rule):
    /// process env > local vault > global vault. Local vault is the fallback
    /// when env is unset. Mirrors v0.17.906's resolve_env_for_target_process_
    /// env_overrides_local_vault fix.
    #[test]
    #[serial]
    fn d1_client_from_env_process_env_overrides_local_config() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());
        let _account = ScopedEnvVar::set("LIBRA_D1_ACCOUNT_ID", "account-from-env");
        let _token = ScopedEnvVar::set("LIBRA_D1_API_TOKEN", "token-from-env");
        let _database = ScopedEnvVar::set("LIBRA_D1_DATABASE_ID", "db-from-env");

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

        // env wins.
        let client = rt
            .block_on(D1Client::from_env())
            .expect("D1Client::from_env should pick up env values when present");
        assert_eq!(client.account_id, "account-from-env");
        assert_eq!(client.api_token, "token-from-env");
        assert_eq!(client.database_id, "db-from-env");

        // …and vault is the fallback when env is unset.
        drop(_account);
        drop(_token);
        drop(_database);
        let _account = ClearedEnvVarGuard::new("LIBRA_D1_ACCOUNT_ID");
        let _token = ClearedEnvVarGuard::new("LIBRA_D1_API_TOKEN");
        let _database = ClearedEnvVarGuard::new("LIBRA_D1_DATABASE_ID");
        let client = rt
            .block_on(D1Client::from_env())
            .expect("D1Client::from_env should fall back to vault");
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

    /// Codex Phase 2 P3 + pass-1 P1 (closed): pin the SQL splitter
    /// behaviour so the publish migrations apply one statement at a
    /// time without breaking on `BEGIN…END` blocks (used by the
    /// trigger migrations), `--` line comments, or `/* */` block
    /// comments. The previous draft of the splitter incorrectly
    /// processed `;` before flushing the running keyword, so `END;`
    /// collapsed an entire trigger block into a single multi-
    /// statement payload.
    #[test]
    fn split_sql_statements_handles_triggers_and_comments() {
        let sql = r#"
            -- Header comment, ignored.
            /* Block comment with ; and BEGIN inside, also ignored. */
            CREATE TABLE IF NOT EXISTS foo (id INTEGER);
            CREATE TRIGGER IF NOT EXISTS foo_guard
                BEFORE INSERT ON foo
                FOR EACH ROW
                WHEN NEW.id < 0
            BEGIN
                SELECT RAISE(ABORT, 'id must be >= 0');
            END;
            CREATE TRIGGER IF NOT EXISTS foo_guard_update
                BEFORE UPDATE ON foo
                FOR EACH ROW
                WHEN NEW.id < 0
            BEGIN
                SELECT RAISE(ABORT, 'id must be >= 0');
            END;
            CREATE INDEX IF NOT EXISTS foo_id ON foo (id);
        "#;
        let stmts = split_sql_statements(sql);
        assert_eq!(stmts.len(), 4, "got: {stmts:?}");
        assert!(stmts[0].starts_with("CREATE TABLE IF NOT EXISTS foo"));
        assert!(stmts[1].contains("CREATE TRIGGER IF NOT EXISTS foo_guard"));
        assert!(stmts[1].contains("END"));
        assert!(stmts[2].contains("CREATE TRIGGER IF NOT EXISTS foo_guard_update"));
        assert!(stmts[2].contains("END"));
        assert!(stmts[3].starts_with("CREATE INDEX IF NOT EXISTS foo_id"));
    }

    /// Pin the splitter against single-quoted literals containing
    /// semicolons (e.g. `RAISE(ABORT, 'must be > 0; restart')`).
    /// A naive splitter would chop the literal in two.
    #[test]
    fn split_sql_statements_preserves_quoted_semicolons() {
        let sql = r#"
            SELECT 'one; two; three' AS phrase;
            SELECT 1;
        "#;
        let stmts = split_sql_statements(sql);
        assert_eq!(stmts.len(), 2, "got: {stmts:?}");
        assert!(stmts[0].contains("'one; two; three'"));
        assert_eq!(stmts[1], "SELECT 1");
    }

    /// The publish migrations under `sql/publish/` must split into a
    /// non-empty statement list and the BEGIN/END trigger blocks must
    /// not be chopped. Acts as a smoke test for `ensure_publish_schema`.
    #[test]
    fn publish_migrations_split_cleanly() {
        let sql_0001 = include_str!("../../sql/publish/0001_publish.sql");
        let sql_0002 = include_str!("../../sql/publish/0002_publish_digest_check.sql");
        let sql_0003 =
            include_str!("../../sql/publish/0003_publish_max_preview_trigger_replace.sql");
        let sql_0004 = include_str!("../../sql/publish/0004_publish_refs_index.sql");
        for (label, sql) in [
            ("0001", sql_0001),
            ("0002", sql_0002),
            ("0003", sql_0003),
            ("0004", sql_0004),
        ] {
            let stmts = split_sql_statements(sql);
            assert!(!stmts.is_empty(), "{label} produced no statements");
            for (idx, stmt) in stmts.iter().enumerate() {
                let begin_count = stmt
                    .split_whitespace()
                    .filter(|w| w.eq_ignore_ascii_case("BEGIN"))
                    .count();
                let end_count = stmt
                    .split_whitespace()
                    .filter(|w| w.eq_ignore_ascii_case("END"))
                    .count();
                assert_eq!(
                    begin_count, end_count,
                    "{label} statement #{idx} has unbalanced BEGIN/END:\n{stmt}",
                );
            }
        }
    }

    #[test]
    fn cloud_clone_domain_resolve_test_uses_unique_slug_and_repo_id_lookups() {
        let slug_sql = publish_site_select_sql("clone_domain = ?1 AND slug = ?2");
        assert!(
            slug_sql.contains("FROM publish_sites WHERE clone_domain = ?1 AND slug = ?2 LIMIT 1"),
            "slug lookup must use the clone-domain unique key: {slug_sql}"
        );
        assert!(
            slug_sql.contains("site_id, repo_id, clone_domain, slug"),
            "slug lookup must return the restore identity columns: {slug_sql}"
        );
        assert!(
            slug_sql.contains("default_ref"),
            "slug lookup must return default ref metadata: {slug_sql}"
        );
        assert!(
            slug_sql.contains("latest_revision_oid"),
            "slug lookup must return latest revision metadata: {slug_sql}"
        );
        assert!(
            slug_sql.contains("refs_generation"),
            "slug lookup must return refs generation metadata: {slug_sql}"
        );

        let repo_sql = publish_site_select_sql("clone_domain = ?1 AND repo_id = ?2");
        assert!(
            repo_sql
                .contains("FROM publish_sites WHERE clone_domain = ?1 AND repo_id = ?2 LIMIT 1"),
            "repo lookup must use the slug-rename-proof unique key: {repo_sql}"
        );
        assert!(
            !repo_sql.contains("slug = ?2"),
            "repo lookup must not depend on the current slug: {repo_sql}"
        );
    }

    #[test]
    fn publish_latest_cas_update_requires_generation_match_unless_forced() {
        let guarded = PublishSiteLatestUpdate {
            site_id: "site-1",
            default_ref: Some("refs/heads/main"),
            latest_revision_oid: Some("abcdef0123456789abcdef0123456789abcdef01"),
            next_refs_generation: 12,
            expected_refs_generation: 11,
            updated_at: "2026-05-13T12:00:00Z",
            force: false,
        };
        let guarded_statement = publish_site_latest_update_statement(&guarded);
        assert!(
            guarded_statement
                .sql
                .contains("WHERE site_id = ?1 AND refs_generation = ?6"),
            "non-force update must use refs_generation as the CAS guard: {}",
            guarded_statement.sql
        );
        assert_eq!(guarded_statement.params.as_ref().expect("params").len(), 6);
        assert_eq!(
            guarded_statement.params.as_ref().expect("params").last(),
            Some(&serde_json::json!(11))
        );

        let forced = PublishSiteLatestUpdate {
            force: true,
            ..guarded
        };
        let forced_statement = publish_site_latest_update_statement(&forced);
        assert!(
            !forced_statement.sql.contains("refs_generation = ?6"),
            "force update must bypass the stale-generation guard: {}",
            forced_statement.sql
        );
        assert_eq!(forced_statement.params.as_ref().expect("params").len(), 5);
    }

    #[test]
    fn publish_latest_cas_update_reports_conflict_on_zero_changes() {
        assert_eq!(
            publish_site_latest_update_result_from_changes(1),
            PublishSiteLatestUpdateResult::Updated
        );
        assert_eq!(
            publish_site_latest_update_result_from_changes(0),
            PublishSiteLatestUpdateResult::Conflict
        );
    }

    #[test]
    fn publish_ref_stale_delete_scopes_to_site_and_current_sync_run() {
        let statement = delete_publish_refs_for_other_sync_runs_statement("site-1", "sync-current");
        assert_eq!(
            statement.sql,
            "DELETE FROM publish_refs WHERE site_id = ?1 AND sync_run_id != ?2"
        );
        let params = statement.params.expect("delete statement must bind params");
        assert_eq!(
            params,
            vec![
                serde_json::json!("site-1"),
                serde_json::json!("sync-current")
            ]
        );
    }

    /// Codex Phase 2 P2 (closed): the `ensure_publish_schema`
    /// migration list is hardcoded via `include_str!` because Rust
    /// has no built-in directory glob at compile time. This test
    /// reads the on-disk `sql/publish/` directory and asserts every
    /// `*.sql` file is present in the hardcoded list, so a future
    /// `0005_*.sql` cannot ship without an explicit code change.
    #[test]
    fn publish_migration_list_matches_disk() {
        use std::path::PathBuf;
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest_dir.join("sql/publish");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .expect("read sql/publish/")
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                if path.extension().and_then(|s| s.to_str()) == Some("sql") {
                    path.file_name()?.to_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect();
        on_disk.sort();
        let expected: Vec<&str> = vec![
            "0001_publish.sql",
            "0002_publish_digest_check.sql",
            "0003_publish_max_preview_trigger_replace.sql",
            "0004_publish_refs_index.sql",
        ];
        assert_eq!(
            on_disk.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            expected,
            "sql/publish/ contents drifted from `ensure_publish_schema` include_str! list; \
             update both lists together",
        );
    }

    /// Codex Phase 2 P1 (regression): the 0002 trigger migration
    /// MUST split into one statement per trigger, not a single
    /// multi-trigger payload. The earlier splitter collapsed every
    /// `END;` because the `;` cleared the keyword buffer before the
    /// `END` was processed at a word boundary.
    #[test]
    fn publish_0002_splits_one_statement_per_trigger() {
        let sql = include_str!("../../sql/publish/0002_publish_digest_check.sql");
        let stmts = split_sql_statements(sql);
        // 0002 ships eight triggers (max_preview INSERT/UPDATE +
        // three sha256 columns × INSERT/UPDATE). Pin the count so a
        // future drift surfaces here instead of in CI under D1.
        assert_eq!(stmts.len(), 8, "0002 stmts: {stmts:?}");
        for (idx, stmt) in stmts.iter().enumerate() {
            assert!(
                stmt.contains("CREATE TRIGGER IF NOT EXISTS"),
                "0002 statement #{idx} is not a trigger:\n{stmt}",
            );
            assert!(
                stmt.ends_with("END") || stmt.contains("END"),
                "0002 statement #{idx} should close with END:\n{stmt}",
            );
        }
    }
}
