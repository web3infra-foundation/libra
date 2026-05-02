//! Versioned schema migration runner — CEX-12.5 deliverable.
//!
//! Provides a single, reusable abstraction every future persistence-touching
//! CEX (CEX-13b ContextFrame, CEX-15 automation_log, CEX-16
//! `agent_usage_stats`, plus Step 2 `schema_versions` extensions) plugs into,
//! so we don't end up with four separate `CREATE TABLE IF NOT EXISTS` hacks
//! scattered across [`crate::internal::db`].
//!
//! # Concepts
//!
//! - [`Migration`] — one named, versioned schema change. Carries an `up`
//!   forward DDL and an optional `down` rollback DDL. The DDL **must be
//!   idempotent** at the SQL level (`CREATE TABLE IF NOT EXISTS`,
//!   `CREATE INDEX IF NOT EXISTS`) so re-running on a partially-applied
//!   database does not error.
//! - [`MigrationRunner`] — owns the registered migration set and applies
//!   pending migrations in monotonic version order. Tracks applied
//!   migrations in a dedicated `schema_versions` table.
//!
//! # Concurrency model
//!
//! All three operations (`run_pending` / `current_version` / `rollback_to`)
//! run inside a SQLite transaction so a crash mid-migration cannot leave the
//! database in an inconsistent state. SQLite serializes writers; concurrent
//! callers wait on the busy timeout already configured in
//! [`crate::internal::db::establish_connection_with_busy_timeout`].
//!
//! # Backward compatibility
//!
//! Pre-CEX-12.5 databases were initialized via `sqlite_20260309_init.sql`
//! plus the legacy `ensure_*_schema` helpers. CEX-12.5 keeps those paths
//! intact and adds the migration runner on top. The runner sees those
//! databases as "schema_version is empty" and applies any registered
//! migration whose `up` DDL is idempotent against the pre-existing tables.
//! Future CEXes only touch the runner — no new `ensure_*` helpers should be
//! added.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement, TransactionTrait};
use thiserror::Error;

/// One named, versioned schema change.
///
/// `up` is required; `down` is optional and only used by
/// [`MigrationRunner::rollback_to`]. Both DDL bodies are executed verbatim
/// inside the migration transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    /// Monotonic version. Versions must be **strictly increasing** within a
    /// runner; duplicate or out-of-order registrations are rejected at
    /// register time.
    pub version: i64,

    /// Human-readable name shown in the `schema_versions` table and audit
    /// logs. Should match the `<version>_<name>` filename if the migration
    /// is loaded from `sql/migrations/`.
    pub name: &'static str,

    /// Forward DDL. Must be idempotent (use `IF NOT EXISTS` for tables /
    /// indexes; tolerate columns that already exist).
    pub up: &'static str,

    /// Optional rollback DDL for [`MigrationRunner::rollback_to`]. `None`
    /// means the migration is forward-only; calling `rollback_to` past such
    /// a migration returns [`MigrationError::IrreversibleMigration`].
    pub down: Option<&'static str>,
}

/// Errors raised by the migration runner.
#[derive(Debug, Error)]
pub enum MigrationError {
    /// Two registered migrations share the same `version`. The runner does
    /// not auto-resolve this; the caller must rename one.
    #[error("duplicate migration version {version} (existing name: {existing}, new name: {new})")]
    DuplicateVersion {
        version: i64,
        existing: &'static str,
        new: &'static str,
    },

    /// A migration was registered with a version smaller than or equal to
    /// the previous one. The runner requires monotonic registration so
    /// `applied_at` ordering matches version ordering.
    #[error(
        "migration versions must be strictly increasing; got {new_version} ({new_name}) after {prev_version} ({prev_name})"
    )]
    NonMonotonicRegistration {
        prev_version: i64,
        prev_name: &'static str,
        new_version: i64,
        new_name: &'static str,
    },

    /// `rollback_to` reached a migration without a `down` DDL.
    #[error("migration {version} ({name}) has no down DDL — cannot rollback past it")]
    IrreversibleMigration { version: i64, name: &'static str },

    /// `rollback_to(target)` was called but `target` is greater than the
    /// current version (i.e. there's nothing to roll back).
    #[error("rollback target {target} is at or above current version {current}")]
    RollbackTargetNotBelowCurrent { target: i64, current: i64 },

    /// `rollback_to(target)` was called on a database with no applied
    /// migrations. Distinct from [`Self::RollbackTargetNotBelowCurrent`]
    /// (which compares against a real `current` version) so callers — and
    /// future migrations using legitimate negative version numbers — can
    /// distinguish "empty database" from "rollback target too high"
    /// without colliding on a sentinel `current` value.
    #[error("rollback target {target} requested but no migrations are applied")]
    RollbackOnEmptyDatabase { target: i64 },

    /// A SQL operation failed.
    #[error("database error: {0}")]
    Database(#[from] DbErr),

    /// A higher-level wrapper for context-rich failures (e.g.
    /// "could not insert into schema_versions").
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// SQL bootstrap for the `schema_versions` tracking table.
///
/// Idempotent: safe to run on every connect. Stored as a `&'static str` so
/// the runner has a single source of truth.
const SCHEMA_VERSIONS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS `schema_versions` (
    `version` INTEGER PRIMARY KEY,
    `name` TEXT NOT NULL,
    `applied_at` TEXT NOT NULL
);
"#;

/// Versioned schema migration runner.
///
/// Build one with [`MigrationRunner::new`], register migrations via
/// [`MigrationRunner::register`], then call
/// [`MigrationRunner::run_pending`] to apply everything pending against a
/// live `DatabaseConnection`.
///
/// The runner is **registration-time** validated — duplicate versions and
/// non-monotonic insertions error out before any SQL runs.
#[derive(Default, Debug)]
pub struct MigrationRunner {
    migrations: Vec<Migration>,
}

impl MigrationRunner {
    /// Create an empty runner. Callers register migrations explicitly via
    /// [`MigrationRunner::register`] (or [`MigrationRunner::extend`]).
    pub fn new() -> Self {
        Self {
            migrations: Vec::new(),
        }
    }

    /// Register a single migration. Returns
    /// [`MigrationError::DuplicateVersion`] if a migration with the same
    /// version is already registered, or
    /// [`MigrationError::NonMonotonicRegistration`] if `version` is not
    /// strictly greater than the most-recent registered version.
    pub fn register(&mut self, migration: Migration) -> Result<(), MigrationError> {
        if let Some(prev) = self.migrations.last() {
            if migration.version == prev.version {
                return Err(MigrationError::DuplicateVersion {
                    version: migration.version,
                    existing: prev.name,
                    new: migration.name,
                });
            }
            if migration.version <= prev.version {
                return Err(MigrationError::NonMonotonicRegistration {
                    prev_version: prev.version,
                    prev_name: prev.name,
                    new_version: migration.version,
                    new_name: migration.name,
                });
            }
        }
        // Also catch duplicates anywhere earlier in the list (not just
        // adjacent), since callers may register out-of-order then expect
        // the runner to sort. We choose strict-monotonic-only above; this
        // additional sweep is belt-and-braces.
        if let Some(existing) = self
            .migrations
            .iter()
            .find(|m| m.version == migration.version)
        {
            return Err(MigrationError::DuplicateVersion {
                version: migration.version,
                existing: existing.name,
                new: migration.name,
            });
        }
        self.migrations.push(migration);
        Ok(())
    }

    /// Register many migrations in order. Stops at the first error and
    /// returns it; previously-accepted migrations stay in the runner.
    pub fn extend<I>(&mut self, migrations: I) -> Result<(), MigrationError>
    where
        I: IntoIterator<Item = Migration>,
    {
        for migration in migrations {
            self.register(migration)?;
        }
        Ok(())
    }

    /// Number of registered migrations. Diagnostics-only.
    pub fn len(&self) -> usize {
        self.migrations.len()
    }

    /// `true` when no migrations are registered.
    pub fn is_empty(&self) -> bool {
        self.migrations.is_empty()
    }

    /// Highest registered version, or `None` for an empty runner.
    pub fn max_registered_version(&self) -> Option<i64> {
        self.migrations.last().map(|m| m.version)
    }

    /// Read the highest applied version from `schema_versions`. Returns
    /// `Ok(None)` for a fresh database (or one initialized before
    /// CEX-12.5).
    pub async fn current_version(
        &self,
        conn: &DatabaseConnection,
    ) -> Result<Option<i64>, MigrationError> {
        ensure_schema_versions_table(conn).await?;
        let backend = conn.get_database_backend();
        let row = conn
            .query_one(Statement::from_string(
                backend,
                "SELECT MAX(version) FROM schema_versions",
            ))
            .await?;
        let Some(row) = row else { return Ok(None) };
        // SQLite returns NULL for `MAX(version)` when the table is empty;
        // sea-orm surfaces that as `Option<i64>`. We forward the None.
        // CONTRACT (Codex r5 P1#1): decode failures must propagate — a
        // type-drifted `version` column would otherwise silently report
        // "empty registry" and trigger a re-run of every migration.
        let version: Option<i64> = row.try_get_by_index(0).map_err(|err| {
            MigrationError::Database(DbErr::Custom(format!(
                "schema_versions.version decode failed: {err}"
            )))
        })?;
        Ok(version)
    }

    /// Apply every registered migration whose version is greater than the
    /// current applied version. Each migration runs inside its own
    /// transaction, with both the `up` DDL and the `schema_versions` row
    /// insert atomic together.
    ///
    /// Returns the list of versions that were newly applied **by this
    /// call** (empty when the database is already up to date, or when a
    /// concurrent process beat us to every pending migration).
    /// Migrations that lost the race in `INSERT OR IGNORE` are NOT
    /// included in the return value even though their up-DDL ran
    /// idempotently.
    pub async fn run_pending(&self, conn: &DatabaseConnection) -> Result<Vec<i64>, MigrationError> {
        ensure_schema_versions_table(conn).await?;
        let current = self.current_version(conn).await?;
        let mut applied = Vec::new();

        for migration in &self.migrations {
            if let Some(current) = current
                && migration.version <= current
            {
                continue;
            }
            // No pre-flight `migration_already_applied` check here: that
            // would be a TOCTOU race with concurrent processes (Codex r1
            // P1#2). `apply_one_migration` uses `INSERT OR IGNORE` and
            // reports whether this call actually wrote the row.
            let inserted = apply_one_migration(conn, migration).await?;
            if inserted {
                applied.push(migration.version);
            }
        }

        Ok(applied)
    }

    /// Roll the schema back to `target` by running each migration's `down`
    /// DDL in reverse version order. Errors with
    /// [`MigrationError::IrreversibleMigration`] if any migration in the
    /// rollback range has no `down` DDL.
    ///
    /// `target` must be strictly less than the current applied version;
    /// passing the same or a larger value returns
    /// [`MigrationError::RollbackTargetNotBelowCurrent`].
    ///
    /// **Atomicity** (Codex r1 P1#8 fix): the rollback plan is
    /// pre-validated before any `down` DDL runs. If any migration in the
    /// `(target, current]` range is irreversible (no `down` DDL), the
    /// runner returns [`MigrationError::IrreversibleMigration`] **without
    /// having executed any down migration**, so the database stays in a
    /// known good state. Per-migration down DDL still runs in its own
    /// transaction so a SQL-level failure mid-plan rolls back only that
    /// step; surrounding successful down migrations stay applied (and
    /// removed from `schema_versions`). Callers that need full
    /// transactional rollback across multiple versions can wrap the call
    /// in their own SQLite `BEGIN ... COMMIT`.
    ///
    /// **Concurrency** (Codex r5 P1#3 fix): the returned `Vec` lists only
    /// the versions that **this call** rolled back. When two callers race
    /// `rollback_to` against the same database, each version is owned by
    /// exactly one caller (the one whose `DELETE FROM schema_versions`
    /// reports `changes() = 1`); the loser sees `changes() = 0` and skips
    /// the down DDL entirely, so no down DDL ever runs twice for the
    /// same version. The returned `Vec` for the loser may therefore be a
    /// strict subset of `(target, current]`.
    pub async fn rollback_to(
        &self,
        conn: &DatabaseConnection,
        target: i64,
    ) -> Result<Vec<i64>, MigrationError> {
        ensure_schema_versions_table(conn).await?;
        let current = self
            .current_version(conn)
            .await?
            .ok_or(MigrationError::RollbackOnEmptyDatabase { target })?;
        if target >= current {
            return Err(MigrationError::RollbackTargetNotBelowCurrent { target, current });
        }

        // Phase 1: build the plan and pre-validate. Collect every migration
        // in `(target, current]` and bail early if any of them lacks a
        // `down` DDL. This guarantees we never partially roll back a
        // multi-version range only to discover an irreversible step
        // halfway through.
        let mut plan: Vec<(&Migration, &'static str)> = Vec::new();
        for migration in self.migrations.iter().rev() {
            if migration.version <= target {
                break;
            }
            if migration.version > current {
                continue;
            }
            let down = migration
                .down
                .ok_or(MigrationError::IrreversibleMigration {
                    version: migration.version,
                    name: migration.name,
                })?;
            plan.push((migration, down));
        }

        // Phase 2: execute the validated plan in order. The
        // irreversible-migration class of error is no longer possible at
        // this point; only SQL-level failures from the `down` DDL itself
        // can surface here. `apply_down_migration` returns whether THIS
        // call owned the version (won the DELETE race); concurrent
        // rollback loser sees `false` and we skip pushing it to the
        // result Vec — symmetric to `run_pending`'s INSERT OR IGNORE
        // semantics.
        let mut rolled_back = Vec::new();
        for (migration, down) in plan {
            let owned = apply_down_migration(conn, migration.version, down).await?;
            if owned {
                rolled_back.push(migration.version);
            }
        }
        Ok(rolled_back)
    }
}

/// Idempotent DDL for the `schema_versions` table; safe to call on every
/// connect. The runner invokes this before any read or write of the
/// version column.
async fn ensure_schema_versions_table(conn: &DatabaseConnection) -> Result<(), MigrationError> {
    let backend = conn.get_database_backend();
    conn.execute(Statement::from_string(backend, SCHEMA_VERSIONS_DDL))
        .await?;
    Ok(())
}

/// Apply one migration atomically. Returns `true` when this call inserted
/// the version row, `false` when another concurrent process beat us to it
/// (Codex r1 P1#2 fix: replaces the TOCTOU `migration_already_applied`
/// check + plain `INSERT` with a race-free `INSERT OR IGNORE` reading the
/// resulting `changes()` to disambiguate "we wrote it" from "someone else
/// already had").
async fn apply_one_migration(
    conn: &DatabaseConnection,
    migration: &Migration,
) -> Result<bool, MigrationError> {
    let now: DateTime<Utc> = Utc::now();
    let inserted = conn
        .transaction::<_, _, DbErr>(|txn| {
            let version = migration.version;
            let name = migration.name;
            let up = migration.up;
            let applied_at = now.to_rfc3339();
            Box::pin(async move {
                let backend = txn.get_database_backend();
                // Apply the user DDL first; if it fails the schema_versions
                // insert never happens and the transaction rolls back.
                txn.execute(Statement::from_string(backend, up)).await?;
                // `INSERT OR IGNORE` plus `changes()` lets us tell whether
                // we won the race or not without a separate read query.
                // SQLite's `changes()` reports rows changed by the LAST
                // INSERT/UPDATE/DELETE on this connection, so we read it
                // immediately after the insert, still inside the
                // transaction.
                txn.execute(Statement::from_sql_and_values(
                    backend,
                    "INSERT OR IGNORE INTO schema_versions (version, name, applied_at) VALUES (?, ?, ?)",
                    [version.into(), name.into(), applied_at.into()],
                ))
                .await?;
                let row = txn
                    .query_one(Statement::from_string(backend, "SELECT changes()"))
                    .await?
                    .ok_or_else(|| {
                        DbErr::Custom("SELECT changes() returned no row".to_string())
                    })?;
                let changed: i64 = row
                    .try_get_by_index(0)
                    .map_err(|err| DbErr::Custom(format!("changes() decode failed: {err}")))?;
                Ok::<bool, DbErr>(changed > 0)
            })
        })
        .await
        .map_err(|err| match err {
            sea_orm::TransactionError::Connection(db) => MigrationError::Database(db),
            sea_orm::TransactionError::Transaction(db) => MigrationError::Database(db),
        })?;
    Ok(inserted)
}

/// Apply one migration's down DDL atomically. Returns `true` when this
/// call owned the rollback (its `DELETE` removed the row), `false` when
/// another concurrent process beat it to the deletion (Codex r5 P1#3
/// fix: symmetric to `apply_one_migration`'s INSERT OR IGNORE / changes()
/// semantics — DELETE first, then run the down DDL only if we won the
/// race, so the down DDL never executes twice for the same version under
/// concurrent rollback).
async fn apply_down_migration(
    conn: &DatabaseConnection,
    version: i64,
    down: &'static str,
) -> Result<bool, MigrationError> {
    let owned = conn
        .transaction::<_, _, DbErr>(|txn| {
            Box::pin(async move {
                let backend = txn.get_database_backend();
                // DELETE first — the row's presence is our ownership
                // claim for this rollback. SQLite's `changes()` reports
                // the rows affected by the LAST INSERT/UPDATE/DELETE on
                // this connection, so we read it immediately after the
                // delete, still inside the transaction.
                txn.execute(Statement::from_sql_and_values(
                    backend,
                    "DELETE FROM schema_versions WHERE version = ?",
                    [version.into()],
                ))
                .await?;
                let row = txn
                    .query_one(Statement::from_string(backend, "SELECT changes()"))
                    .await?
                    .ok_or_else(|| DbErr::Custom("SELECT changes() returned no row".to_string()))?;
                let changed: i64 = row
                    .try_get_by_index(0)
                    .map_err(|err| DbErr::Custom(format!("changes() decode failed: {err}")))?;
                if changed == 0 {
                    // Another caller already rolled this version back.
                    // Skip the down DDL — running it twice is exactly the
                    // bug this fix prevents.
                    return Ok::<bool, DbErr>(false);
                }
                // We own the rollback for this version. Execute the down
                // DDL inside the same transaction so a SQL failure rolls
                // both the DELETE and the partial DDL back.
                txn.execute(Statement::from_string(backend, down)).await?;
                Ok::<bool, DbErr>(true)
            })
        })
        .await
        .map_err(|err| match err {
            sea_orm::TransactionError::Connection(db) => MigrationError::Database(db),
            sea_orm::TransactionError::Transaction(db) => MigrationError::Database(db),
        })?;
    Ok(owned)
}

/// Returns the canonical migration set the libra runtime registers on every
/// connect. CEX-12.5 ships an empty registry — future CEXes (13b / 15 / 16)
/// add their migrations here in version order. Keeping the set centralised
/// in this function (rather than in `establish_connection`) makes it
/// trivial to test the wiring against an isolated runner.
pub fn builtin_migrations() -> Vec<Migration> {
    Vec::new()
}

/// Convenience: build a runner pre-loaded with [`builtin_migrations`].
///
/// **Returns `Result`**: a future CEX adding a duplicate or non-monotonic
/// version to `builtin_migrations()` would otherwise produce a partial
/// registry without surfacing the registration error. Tests in
/// `tests/db_migration_test.rs` exercise this path so registration mistakes
/// fail fast in CI rather than at first-use of the missing migration.
pub fn builtin_runner() -> Result<MigrationRunner, MigrationError> {
    let mut runner = MigrationRunner::new();
    runner.extend(builtin_migrations())?;
    Ok(runner)
}

/// Run all built-in migrations on the given connection. This is the
/// canonical entry point used by [`crate::internal::db::establish_connection`]
/// (and by tests that want the same wiring as production). Both registry-
/// build errors and per-migration apply errors are surfaced through
/// `anyhow::Error` so the call site can attach its own context.
pub async fn run_builtin_migrations(conn: &DatabaseConnection) -> Result<Vec<i64>> {
    let runner =
        builtin_runner().with_context(|| "failed to build the built-in migration registry")?;
    runner
        .run_pending(conn)
        .await
        .with_context(|| "failed to run built-in schema migrations")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_rejects_duplicate_version() {
        let mut runner = MigrationRunner::new();
        runner
            .register(Migration {
                version: 1,
                name: "first",
                up: "",
                down: None,
            })
            .unwrap();
        let err = runner
            .register(Migration {
                version: 1,
                name: "first_again",
                up: "",
                down: None,
            })
            .unwrap_err();
        assert!(matches!(
            err,
            MigrationError::DuplicateVersion {
                version: 1,
                existing: "first",
                new: "first_again",
            }
        ));
    }

    #[test]
    fn register_rejects_non_monotonic_version() {
        let mut runner = MigrationRunner::new();
        runner
            .register(Migration {
                version: 5,
                name: "later",
                up: "",
                down: None,
            })
            .unwrap();
        let err = runner
            .register(Migration {
                version: 3,
                name: "earlier",
                up: "",
                down: None,
            })
            .unwrap_err();
        assert!(matches!(
            err,
            MigrationError::NonMonotonicRegistration { .. }
        ));
    }

    #[test]
    fn empty_runner_max_registered_version_is_none() {
        let runner = MigrationRunner::new();
        assert_eq!(runner.max_registered_version(), None);
        assert!(runner.is_empty());
        assert_eq!(runner.len(), 0);
    }

    #[test]
    fn builtin_runner_starts_empty() {
        // CEX-12.5 ships the framework with zero registered migrations.
        // When CEX-13b / 15 / 16 land they add their migrations to
        // `builtin_migrations()` and this assertion is updated to the new
        // count.
        let runner = builtin_runner().expect("CEX-12.5 builtin registry must build clean");
        assert_eq!(runner.len(), 0);
        assert!(runner.is_empty());
    }

    #[test]
    fn builtin_runner_propagates_registration_errors() {
        // Codex r1 P1#1 fix regression guard: changing `builtin_runner` to
        // return `Result` (instead of silently dropping registration
        // errors) means a future CEX that introduces a duplicate version
        // is caught at registry-build time rather than at first-use of a
        // missing migration. We synthesise a duplicate inline because
        // `builtin_migrations()` itself is empty at this CEX.
        let mut runner = MigrationRunner::new();
        runner
            .register(Migration {
                version: 1,
                name: "first",
                up: "",
                down: None,
            })
            .unwrap();
        let err = runner
            .extend(vec![Migration {
                version: 1,
                name: "first_again",
                up: "",
                down: None,
            }])
            .unwrap_err();
        assert!(matches!(err, MigrationError::DuplicateVersion { .. }));
    }
}
