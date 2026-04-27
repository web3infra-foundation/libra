//! Transaction wrapper contract for operation-level audit logging.
//!
//! Commit 1 introduces only stable wrapper-facing types that are required by
//! A-5: metadata, snapshot scope, wrapper result, and stage-specific errors.
//! Commit 2 adds transaction skeleton execution (begin -> business -> commit)
//! without snapshot capture/persistence.

use std::{
    future::Future,
    pin::Pin,
};

use chrono::Utc;
use sea_orm::{
    DatabaseConnection, DatabaseTransaction, DbErr, TransactionTrait,
};
use thiserror::Error;
use uuid::Uuid;

use crate::internal::db::get_db_conn_instance;

/// Required command metadata captured by `with_operation_log`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationMeta {
    pub command_name: String,
    pub description: String,
    pub actor: String,
    pub repo_id: String,
    pub args_digest: Option<String>,
}

impl OperationMeta {
    /// Validate required fields before entering transaction orchestration.
    pub fn validate(&self) -> Result<(), OperationError> {
        if self.command_name.trim().is_empty() {
            return Err(OperationError::validation(
                "command_name must not be empty",
            ));
        }
        if self.description.trim().is_empty() {
            return Err(OperationError::validation("description must not be empty"));
        }
        if self.actor.trim().is_empty() {
            return Err(OperationError::validation("actor must not be empty"));
        }
        if self.repo_id.trim().is_empty() {
            return Err(OperationError::validation("repo_id must not be empty"));
        }

        Ok(())
    }
}

/// Controls which parts of the final repository view should be captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationScope {
    pub include_refs: bool,
    pub include_workspace: bool,
    pub include_remote_tracking: bool,
}

impl Default for OperationScope {
    fn default() -> Self {
        Self {
            include_refs: true,
            include_workspace: true,
            include_remote_tracking: false,
        }
    }
}

/// Wrapper return shape: business result and operation identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationResult<T> {
    pub payload: T,
    pub op_id: String,
    pub view_id: String,
    pub end_ts: i64,
}

/// Stage-specific failures for with_operation_log.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OperationError {
    #[error("invalid operation metadata: {0}")]
    Validation(String),
    #[error("failed to begin operation transaction: {0}")]
    Begin(String),
    #[error("operation business write failed: {0}")]
    Business(String),
    #[error("failed to capture operation snapshot: {0}")]
    Snapshot(String),
    #[error("failed to persist operation record: {0}")]
    Persist(String),
    #[error("failed to commit operation transaction: {0}")]
    Commit(String),
    #[error("failed to rollback operation transaction: {0}")]
    Rollback(String),
}

impl OperationError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    pub fn begin(message: impl Into<String>) -> Self {
        Self::Begin(message.into())
    }

    pub fn business(message: impl Into<String>) -> Self {
        Self::Business(message.into())
    }

    pub fn snapshot(message: impl Into<String>) -> Self {
        Self::Snapshot(message.into())
    }

    pub fn persist(message: impl Into<String>) -> Self {
        Self::Persist(message.into())
    }

    pub fn commit(message: impl Into<String>) -> Self {
        Self::Commit(message.into())
    }

    pub fn rollback(message: impl Into<String>) -> Self {
        Self::Rollback(message.into())
    }
}

/// Execute one business write closure in a transaction and return operation ids.
///
/// Commit 2 scope:
/// 1. Validate metadata.
/// 2. Begin transaction.
/// 3. Execute business closure.
/// 4. Commit on success, rollback on business failure.
///
/// Snapshot capture and operation graph persistence are added in later commits.
pub async fn with_operation_log<T, F>(
    meta: OperationMeta,
    scope: OperationScope,
    operation: F,
) -> Result<OperationResult<T>, OperationError>
where
    for<'b> F: FnOnce(
        &'b DatabaseTransaction,
    ) -> Pin<Box<dyn Future<Output = Result<T, DbErr>> + Send + 'b>>,
    F: Send + 'static,
{
    let db = get_db_conn_instance().await;
    with_operation_log_with_conn(&db, meta, scope, operation).await
}

/// Same as [`with_operation_log`] but uses caller-provided database connection.
///
/// This helper is designed for tests and advanced internal callers.
pub async fn with_operation_log_with_conn<T, F>(
    db: &DatabaseConnection,
    meta: OperationMeta,
    _scope: OperationScope,
    operation: F,
) -> Result<OperationResult<T>, OperationError>
where
    for<'b> F: FnOnce(
        &'b DatabaseTransaction,
    ) -> Pin<Box<dyn Future<Output = Result<T, DbErr>> + Send + 'b>>,
    F: Send + 'static,
{
    meta.validate()?;

    let op_id = Uuid::now_v7().to_string();
    let view_id = Uuid::now_v7().to_string();

    let txn = db.begin().await.map_err(|err| {
        OperationError::begin(format!(
            "failed to open operation transaction for command '{}': {err}",
            meta.command_name
        ))
    })?;

    let payload = match operation(&txn).await {
        Ok(payload) => payload,
        Err(err) => {
            txn.rollback().await.map_err(|rollback_err| {
                OperationError::rollback(format!(
                    "business step failed with '{err}', and rollback also failed: {rollback_err}"
                ))
            })?;
            return Err(OperationError::business(format!(
                "command '{}' business write failed: {err}",
                meta.command_name
            )));
        }
    };

    txn.commit().await.map_err(|err| {
        OperationError::commit(format!(
            "failed to commit operation transaction for command '{}': {err}",
            meta.command_name
        ))
    })?;

    Ok(OperationResult {
        payload,
        op_id,
        view_id,
        end_ts: Utc::now().timestamp(),
    })
}

#[cfg(test)]
mod tests {
    use sea_orm::{
        ConnectionTrait, Database, DbBackend, DbErr, Statement,
    };

    use super::{
        OperationError, OperationMeta, OperationScope, with_operation_log_with_conn,
    };

    fn valid_meta() -> OperationMeta {
        OperationMeta {
            command_name: "commit".to_string(),
            description: "record snapshot".to_string(),
            actor: "alice".to_string(),
            repo_id: "repo_1".to_string(),
            args_digest: Some("sha256:abcd".to_string()),
        }
    }

    #[test]
    fn meta_validation_rejects_empty_fields() {
        let mut meta = valid_meta();
        meta.command_name = " ".to_string();
        assert!(matches!(meta.validate(), Err(OperationError::Validation(_))));

        let mut meta = valid_meta();
        meta.repo_id = " ".to_string();
        assert!(matches!(meta.validate(), Err(OperationError::Validation(_))));
    }

    #[test]
    fn scope_default_matches_a5_contract() {
        let scope = OperationScope::default();
        assert!(scope.include_refs);
        assert!(scope.include_workspace);
        assert!(!scope.include_remote_tracking);
    }

    #[tokio::test]
    async fn with_operation_log_returns_payload_and_ids_on_success() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        let result = with_operation_log_with_conn(
            &db,
            valid_meta(),
            OperationScope::default(),
            |_txn| Box::pin(async move { Ok::<_, DbErr>("ok".to_string()) }),
        )
        .await
        .unwrap();

        assert_eq!(result.payload, "ok");
        assert!(!result.op_id.is_empty());
        assert!(!result.view_id.is_empty());
        assert!(result.end_ts > 0);
    }

    #[tokio::test]
    async fn with_operation_log_rolls_back_on_business_failure() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute(Statement::from_string(
            DbBackend::Sqlite,
            "CREATE TABLE tx_probe (id INTEGER PRIMARY KEY)".to_string(),
        ))
        .await
        .unwrap();

        let error = with_operation_log_with_conn(
            &db,
            valid_meta(),
            OperationScope::default(),
            |txn| {
                Box::pin(async move {
                    txn.execute(Statement::from_string(
                        DbBackend::Sqlite,
                        "INSERT INTO tx_probe(id) VALUES(1)".to_string(),
                    ))
                    .await?;
                    Err::<(), DbErr>(DbErr::Custom("boom".to_string()))
                })
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(error, OperationError::Business(_)));

        let row = db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) FROM tx_probe".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        let count: i64 = row.try_get_by_index(0).unwrap_or_default();
        assert_eq!(count, 0);
    }
}
