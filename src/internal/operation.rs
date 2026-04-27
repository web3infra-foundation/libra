//! Operation service skeleton for command-level audit persistence.
//!
//! This module defines stable public types for A-6. Commit 2 introduces the
//! operation main-table base DAO methods while keeping transaction ownership in
//! callers through `*_with_conn` signatures.

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use thiserror::Error;

use crate::internal::model::operation;

/// Stable status of an operation record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl OperationStatus {
    fn as_db_value(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    fn from_db_value(value: &str) -> Result<Self, OperationServiceError> {
        match value {
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "canceled" => Ok(Self::Canceled),
            other => Err(OperationServiceError::Internal(format!(
                "unknown operation status value in storage: {other}"
            ))),
        }
    }
}

/// Immutable operation record payload used by DAO/service boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRecord {
    pub op_id: String,
    pub repo_id: String,
    pub view_id: String,
    pub command_name: String,
    pub description: String,
    pub actor: String,
    pub args_digest: Option<String>,
    pub start_ts: i64,
    pub end_ts: Option<i64>,
    pub status: OperationStatus,
}

/// Generic pagination request for operation list APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationQueryPage {
    pub page: u64,
    pub per_page: u64,
}

impl Default for OperationQueryPage {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: Self::DEFAULT_PER_PAGE,
        }
    }
}

impl OperationQueryPage {
    pub const DEFAULT_PER_PAGE: u64 = 50;
    pub const MAX_PER_PAGE: u64 = 200;

    /// Clamp invalid pagination input into a safe query range.
    pub fn normalized(self) -> Self {
        let page = if self.page == 0 { 1 } else { self.page };
        let per_page = if self.per_page == 0 {
            Self::DEFAULT_PER_PAGE
        } else {
            self.per_page.clamp(1, Self::MAX_PER_PAGE)
        };
        Self { page, per_page }
    }

    pub fn offset(self) -> u64 {
        let normalized = self.normalized();
        (normalized.page - 1) * normalized.per_page
    }
}

/// Generic paginated operation list response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationPage<T> {
    pub items: Vec<T>,
    pub page: u64,
    pub per_page: u64,
    pub total: u64,
}

#[derive(Debug, Error)]
pub enum OperationServiceError {
    #[error("invalid operation argument: {0}")]
    InvalidArgument(String),
    #[error("operation storage error: {0}")]
    Storage(String),
    #[error("operation internal error: {0}")]
    Internal(String),
}

/// Operation service placeholder.
///
/// Commit 2 adds operation main table base DAO methods. Higher-level graph
/// persistence and wrapper orchestration are introduced in later commits.
#[derive(Debug, Default)]
pub struct OperationService;

impl OperationService {
    fn record_from_model(model: operation::Model) -> Result<OperationRecord, OperationServiceError> {
        let status = OperationStatus::from_db_value(&model.status)?;
        Ok(OperationRecord {
            op_id: model.op_id,
            repo_id: model.repo_id,
            view_id: model.view_id,
            command_name: model.command_name,
            description: model.description,
            actor: model.actor,
            args_digest: model.args_digest,
            start_ts: model.start_ts,
            end_ts: model.end_ts,
            status,
        })
    }

    /// Insert one operation main record using an existing connection or transaction.
    pub async fn insert_operation_with_conn<C: ConnectionTrait>(
        db: &C,
        record: &OperationRecord,
    ) -> Result<OperationRecord, OperationServiceError> {
        Self::validate_record(record)?;

        let model = operation::ActiveModel {
            op_id: Set(record.op_id.clone()),
            repo_id: Set(record.repo_id.clone()),
            view_id: Set(record.view_id.clone()),
            command_name: Set(record.command_name.clone()),
            description: Set(record.description.clone()),
            actor: Set(record.actor.clone()),
            args_digest: Set(record.args_digest.clone()),
            start_ts: Set(record.start_ts),
            end_ts: Set(record.end_ts),
            status: Set(record.status.as_db_value().to_string()),
        };

        let inserted = model.insert(db).await.map_err(|err| {
            OperationServiceError::Storage(format!(
                "failed to insert operation '{}' for repository '{}': {err}",
                record.op_id, record.repo_id
            ))
        })?;

        Self::record_from_model(inserted)
    }

    /// Find one operation main record by operation id.
    pub async fn find_operation_by_id_with_conn<C: ConnectionTrait>(
        db: &C,
        op_id: &str,
    ) -> Result<Option<OperationRecord>, OperationServiceError> {
        if op_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "op_id must not be empty".to_string(),
            ));
        }

        let model = operation::Entity::find_by_id(op_id.to_string())
            .one(db)
            .await
            .map_err(|err| {
                OperationServiceError::Storage(format!(
                    "failed to query operation '{}' from storage: {err}",
                    op_id
                ))
            })?;

        model.map(Self::record_from_model).transpose()
    }

    /// List latest operation main records for a repository.
    pub async fn list_operations_by_repo_with_conn<C: ConnectionTrait>(
        db: &C,
        repo_id: &str,
        limit: u64,
    ) -> Result<Vec<OperationRecord>, OperationServiceError> {
        if repo_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "repo_id must not be empty".to_string(),
            ));
        }
        if limit == 0 {
            return Err(OperationServiceError::InvalidArgument(
                "limit must be greater than 0".to_string(),
            ));
        }

        let models = operation::Entity::find()
            .filter(operation::Column::RepoId.eq(repo_id))
            .order_by_desc(operation::Column::EndTs)
            .order_by_desc(operation::Column::StartTs)
            .limit(limit)
            .all(db)
            .await
            .map_err(|err| {
                OperationServiceError::Storage(format!(
                    "failed to list operations for repository '{}': {err}",
                    repo_id
                ))
            })?;

        models
            .into_iter()
            .map(Self::record_from_model)
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn validate_record(record: &OperationRecord) -> Result<(), OperationServiceError> {
        if record.op_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "op_id must not be empty".to_string(),
            ));
        }
        if record.repo_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "repo_id must not be empty".to_string(),
            ));
        }
        if record.command_name.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "command_name must not be empty".to_string(),
            ));
        }
        if record.start_ts < 0 {
            return Err(OperationServiceError::InvalidArgument(
                "start_ts must be a unix timestamp in seconds".to_string(),
            ));
        }
        if let Some(end_ts) = record.end_ts
            && end_ts < record.start_ts
        {
            return Err(OperationServiceError::InvalidArgument(
                "end_ts must be greater than or equal to start_ts".to_string(),
            ));
        }

        Ok(())
    }

    pub fn normalize_query_page(query: OperationQueryPage) -> OperationQueryPage {
        query.normalized()
    }

    pub fn new_page<T>(
        items: Vec<T>,
        query: OperationQueryPage,
        total: u64,
    ) -> OperationPage<T> {
        let normalized = query.normalized();
        OperationPage {
            items,
            page: normalized.page,
            per_page: normalized.per_page,
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::Database;

    use super::{
        OperationQueryPage, OperationRecord, OperationService, OperationServiceError,
        OperationStatus,
    };

    fn sample_record() -> OperationRecord {
        OperationRecord {
            op_id: "op_1".to_string(),
            repo_id: "repo_1".to_string(),
            view_id: "view_1".to_string(),
            command_name: "commit".to_string(),
            description: "commit message".to_string(),
            actor: "alice".to_string(),
            args_digest: Some("sha256:abcd".to_string()),
            start_ts: 100,
            end_ts: Some(120),
            status: OperationStatus::Succeeded,
        }
    }

    #[test]
    fn commit1_normalize_query_page_clamps_to_limits() {
        let normalized = OperationService::normalize_query_page(OperationQueryPage {
            page: 0,
            per_page: 999,
        });
        assert_eq!(normalized.page, 1);
        assert_eq!(normalized.per_page, OperationQueryPage::MAX_PER_PAGE);

        let normalized = OperationService::normalize_query_page(OperationQueryPage {
            page: 3,
            per_page: 20,
        });
        assert_eq!(normalized.page, 3);
        assert_eq!(normalized.per_page, 20);
    }

    #[test]
    fn commit1_validate_record_rejects_invalid_timestamps() {
        let mut record = sample_record();
        record.end_ts = Some(99);

        let error = OperationService::validate_record(&record).unwrap_err();
        assert!(error
            .to_string()
            .contains("end_ts must be greater than or equal to start_ts"));
    }

    #[tokio::test]
    async fn commit2_insert_rejects_invalid_record() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        let mut record = sample_record();
        record.op_id = " ".to_string();

        let error = OperationService::insert_operation_with_conn(&db, &record)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));
    }

    #[tokio::test]
    async fn commit2_find_rejects_empty_op_id() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        let error = OperationService::find_operation_by_id_with_conn(&db, " ")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));
    }

    #[tokio::test]
    async fn commit2_list_rejects_invalid_arguments() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        let error = OperationService::list_operations_by_repo_with_conn(&db, "", 1)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));

        let error = OperationService::list_operations_by_repo_with_conn(&db, "repo_1", 0)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));
    }
}
