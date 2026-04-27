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

use crate::internal::model::{operation, operation_parent, operation_view, operation_view_ref};

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

/// Parent edge for operation lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationParentRecord {
    pub op_id: String,
    pub parent_op_id: String,
}

/// Snapshot header for one operation view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationViewRecord {
    pub view_id: String,
    pub repo_id: String,
    pub head_kind: String,
    pub head_target: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationViewRefRecord {
    pub view_id: String,
    pub ref_kind: String,
    pub ref_name: String,
    pub ref_remote: Option<String>,
    pub target_oid: String,
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

    fn parent_from_model(
        model: operation_parent::Model,
    ) -> Result<OperationParentRecord, OperationServiceError> {
        let parent = OperationParentRecord {
            op_id: model.op_id,
            parent_op_id: model.parent_op_id,
        };
        Self::validate_parent_record(&parent)?;
        Ok(parent)
    }

    pub fn validate_parent_record(
        record: &OperationParentRecord,
    ) -> Result<(), OperationServiceError> {
        if record.op_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "op_id must not be empty".to_string(),
            ));
        }
        if record.parent_op_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "parent_op_id must not be empty".to_string(),
            ));
        }
        if record.op_id == record.parent_op_id {
            return Err(OperationServiceError::InvalidArgument(
                "op_id must not equal parent_op_id".to_string(),
            ));
        }
        Ok(())
    }

    /// Insert one operation parent edge.
    pub async fn insert_parent_with_conn<C: ConnectionTrait>(
        db: &C,
        record: &OperationParentRecord,
    ) -> Result<OperationParentRecord, OperationServiceError> {
        Self::validate_parent_record(record)?;

        let model = operation_parent::ActiveModel {
            op_id: Set(record.op_id.clone()),
            parent_op_id: Set(record.parent_op_id.clone()),
        };

        let inserted = model.insert(db).await.map_err(|err| {
            OperationServiceError::Storage(format!(
                "failed to insert operation parent edge ('{}' -> '{}'): {err}",
                record.op_id, record.parent_op_id
            ))
        })?;

        Self::parent_from_model(inserted)
    }

    /// List parent edges of one operation, ordered by parent operation id.
    pub async fn list_parents_with_conn<C: ConnectionTrait>(
        db: &C,
        op_id: &str,
    ) -> Result<Vec<OperationParentRecord>, OperationServiceError> {
        if op_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "op_id must not be empty".to_string(),
            ));
        }

        let models = operation_parent::Entity::find()
            .filter(operation_parent::Column::OpId.eq(op_id))
            .order_by_asc(operation_parent::Column::ParentOpId)
            .all(db)
            .await
            .map_err(|err| {
                OperationServiceError::Storage(format!(
                    "failed to list parent edges for operation '{}': {err}",
                    op_id
                ))
            })?;

        models
            .into_iter()
            .map(Self::parent_from_model)
            .collect::<Result<Vec<_>, _>>()
    }

    fn view_from_model(model: operation_view::Model) -> Result<OperationViewRecord, OperationServiceError> {
        let view = OperationViewRecord {
            view_id: model.view_id,
            repo_id: model.repo_id,
            head_kind: model.head_kind,
            head_target: model.head_target,
            created_at: model.created_at,
        };
        Self::validate_view_record(&view)?;
        Ok(view)
    }

    pub fn validate_view_record(record: &OperationViewRecord) -> Result<(), OperationServiceError> {
        if record.view_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "view_id must not be empty".to_string(),
            ));
        }
        if record.repo_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "repo_id must not be empty".to_string(),
            ));
        }
        if record.head_kind.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "head_kind must not be empty".to_string(),
            ));
        }
        if record.head_target.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "head_target must not be empty".to_string(),
            ));
        }
        if record.created_at < 0 {
            return Err(OperationServiceError::InvalidArgument(
                "created_at must be a unix timestamp in seconds".to_string(),
            ));
        }
        Ok(())
    }

    /// Insert one operation view snapshot.
    pub async fn insert_view_with_conn<C: ConnectionTrait>(
        db: &C,
        record: &OperationViewRecord,
    ) -> Result<OperationViewRecord, OperationServiceError> {
        Self::validate_view_record(record)?;

        let model = operation_view::ActiveModel {
            view_id: Set(record.view_id.clone()),
            repo_id: Set(record.repo_id.clone()),
            head_kind: Set(record.head_kind.clone()),
            head_target: Set(record.head_target.clone()),
            created_at: Set(record.created_at),
        };

        let inserted = model.insert(db).await.map_err(|err| {
            OperationServiceError::Storage(format!(
                "failed to insert operation view '{}' for repository '{}': {err}",
                record.view_id, record.repo_id
            ))
        })?;

        Self::view_from_model(inserted)
    }

    /// Find one operation view by operation id.
    pub async fn find_view_by_operation_with_conn<C: ConnectionTrait>(
        db: &C,
        op_id: &str,
    ) -> Result<Option<OperationViewRecord>, OperationServiceError> {
        if op_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "op_id must not be empty".to_string(),
            ));
        }

        let op_model = operation::Entity::find_by_id(op_id.to_string())
            .one(db)
            .await
            .map_err(|err| {
                OperationServiceError::Storage(format!(
                    "failed to query operation '{}' from storage: {err}",
                    op_id
                ))
            })?;

        let Some(op_model) = op_model else {
            return Ok(None);
        };

        let view_model = operation_view::Entity::find_by_id(op_model.view_id)
            .one(db)
            .await
            .map_err(|err| {
                OperationServiceError::Storage(format!(
                    "failed to query operation view by operation '{}': {err}",
                    op_id
                ))
            })?;

        view_model.map(Self::view_from_model).transpose()
    }

    fn view_ref_from_model(
        model: operation_view_ref::Model,
    ) -> Result<OperationViewRefRecord, OperationServiceError> {
        let record = OperationViewRefRecord {
            view_id: model.view_id,
            ref_kind: model.ref_kind,
            ref_name: model.ref_name,
            ref_remote: if model.ref_remote.is_empty() {
                None
            } else {
                Some(model.ref_remote)
            },
            target_oid: model.target_oid,
        };
        Self::validate_view_ref_record(&record)?;
        Ok(record)
    }

    pub fn validate_view_ref_record(
        record: &OperationViewRefRecord,
    ) -> Result<(), OperationServiceError> {
        if record.view_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "view_id must not be empty".to_string(),
            ));
        }
        if record.ref_kind.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "ref_kind must not be empty".to_string(),
            ));
        }
        if record.ref_name.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "ref_name must not be empty".to_string(),
            ));
        }
        if record.target_oid.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "target_oid must not be empty".to_string(),
            ));
        }
        Ok(())
    }
    pub async fn replace_view_refs_with_conn<C: ConnectionTrait>(
        db: &C,
        view_id: &str,
        refs: &[OperationViewRefRecord],
    ) -> Result<Vec<OperationViewRefRecord>, OperationServiceError> {
        if view_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "view_id must not be empty".to_string(),
            ));
        }
        for record in refs {
            Self::validate_view_ref_record(record)?;
            if record.view_id != view_id {
                return Err(OperationServiceError::InvalidArgument(
                    "all view refs must use the same view_id".to_string(),
                ));
            }
        }
        operation_view_ref::Entity::delete_many()
            .filter(operation_view_ref::Column::ViewId.eq(view_id))
            .exec(db)
            .await
            .map_err(|err| {
                OperationServiceError::Storage(format!(
                    "failed to clear view refs for view '{}': {err}",
                    view_id
                ))
            })?;

        let mut inserted = Vec::with_capacity(refs.len());
        for record in refs {
            let model = operation_view_ref::ActiveModel {
                view_id: Set(record.view_id.clone()),
                ref_kind: Set(record.ref_kind.clone()),
                ref_name: Set(record.ref_name.clone()),
                ref_remote: Set(record.ref_remote.clone().unwrap_or_default()),
                target_oid: Set(record.target_oid.clone()),
            };

            let saved = model.insert(db).await.map_err(|err| {
                OperationServiceError::Storage(format!(
                    "failed to insert view ref '{}:{}' for view '{}': {err}",
                    record.ref_kind, record.ref_name, view_id
                ))
            })?;
            inserted.push(Self::view_ref_from_model(saved)?);
        }
        Ok(inserted)
    }
    pub async fn list_view_refs_with_conn<C: ConnectionTrait>(
        db: &C,
        view_id: &str,
    ) -> Result<Vec<OperationViewRefRecord>, OperationServiceError> {
        if view_id.trim().is_empty() {
            return Err(OperationServiceError::InvalidArgument(
                "view_id must not be empty".to_string(),
            ));
        }
        let models = operation_view_ref::Entity::find()
            .filter(operation_view_ref::Column::ViewId.eq(view_id))
            .order_by_asc(operation_view_ref::Column::RefKind)
            .order_by_asc(operation_view_ref::Column::RefName)
            .order_by_asc(operation_view_ref::Column::RefRemote)
            .all(db)
            .await
            .map_err(|err| {
                OperationServiceError::Storage(format!(
                    "failed to list view refs for view '{}': {err}",
                    view_id
                ))
            })?;

        models
            .into_iter()
            .map(Self::view_ref_from_model)
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
    use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};

    use super::{
        OperationParentRecord, OperationQueryPage, OperationRecord, OperationService,
        OperationServiceError, OperationStatus, OperationViewRecord, OperationViewRefRecord,
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

    #[tokio::test]
    async fn commit3_insert_parent_rejects_invalid_edge() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        let edge = OperationParentRecord {
            op_id: " ".to_string(),
            parent_op_id: "op_0".to_string(),
        };
        let error = OperationService::insert_parent_with_conn(&db, &edge)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));

        let edge = OperationParentRecord {
            op_id: "op_1".to_string(),
            parent_op_id: "op_1".to_string(),
        };
        let error = OperationService::insert_parent_with_conn(&db, &edge)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));
    }

    #[tokio::test]
    async fn commit3_list_parents_rejects_empty_op_id() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        let error = OperationService::list_parents_with_conn(&db, " ")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));
    }

    #[tokio::test]
    async fn commit3_insert_and_list_parents_roundtrip() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        db.execute(Statement::from_string(
            DbBackend::Sqlite,
            r#"
            CREATE TABLE IF NOT EXISTS operation_parent (
                op_id TEXT NOT NULL,
                parent_op_id TEXT NOT NULL,
                PRIMARY KEY (op_id, parent_op_id)
            );
            "#,
        ))
        .await
        .unwrap();

        let p1 = OperationParentRecord {
            op_id: "op_2".to_string(),
            parent_op_id: "op_0".to_string(),
        };
        let p2 = OperationParentRecord {
            op_id: "op_2".to_string(),
            parent_op_id: "op_1".to_string(),
        };
        OperationService::insert_parent_with_conn(&db, &p1)
            .await
            .unwrap();
        OperationService::insert_parent_with_conn(&db, &p2)
            .await
            .unwrap();

        let parents = OperationService::list_parents_with_conn(&db, "op_2")
            .await
            .unwrap();
        assert_eq!(parents.len(), 2);
        assert_eq!(parents[0].parent_op_id, "op_0");
        assert_eq!(parents[1].parent_op_id, "op_1");
    }

    #[tokio::test]
    async fn commit4_insert_view_rejects_invalid_record() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        let view = OperationViewRecord {
            view_id: " ".to_string(),
            repo_id: "repo_1".to_string(),
            head_kind: "branch".to_string(),
            head_target: "main".to_string(),
            created_at: 100,
        };
        let error = OperationService::insert_view_with_conn(&db, &view)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));
    }

    #[tokio::test]
    async fn commit4_find_view_rejects_empty_op_id() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        let error = OperationService::find_view_by_operation_with_conn(&db, " ")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));
    }

    #[test]
    fn commit4_validate_view_accepts_valid_record() {
        let view = OperationViewRecord {
            view_id: "view_10".to_string(),
            repo_id: "repo_1".to_string(),
            head_kind: "branch".to_string(),
            head_target: "main".to_string(),
            created_at: 101,
        };

        OperationService::validate_view_record(&view).unwrap();
    }

    #[tokio::test]
    async fn commit5_replace_view_refs_rejects_invalid_record() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        let refs = vec![OperationViewRefRecord {
            view_id: "view_1".to_string(),
            ref_kind: "branch".to_string(),
            ref_name: "main".to_string(),
            ref_remote: None,
            target_oid: " ".to_string(),
        }];
        let error = OperationService::replace_view_refs_with_conn(&db, "view_1", &refs)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));
    }

    #[tokio::test]
    async fn commit5_list_view_refs_rejects_empty_view_id() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        let error = OperationService::list_view_refs_with_conn(&db, " ")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            OperationServiceError::InvalidArgument(_)
        ));
    }

    #[test]
    fn commit5_validate_view_ref_accepts_valid_record() {
        let record = OperationViewRefRecord {
            view_id: "view_2".to_string(),
            ref_kind: "branch".to_string(),
            ref_name: "main".to_string(),
            ref_remote: None,
            target_oid: "oid-main".to_string(),
        };

        OperationService::validate_view_ref_record(&record).unwrap();
    }
}
