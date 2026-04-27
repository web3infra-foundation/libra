//! Operation service skeleton for command-level audit persistence.
//!
//! This module defines stable public types for A-6 and intentionally keeps
//! database access out of Commit 1. Later commits will add DAO methods with
//! `*_with_conn` variants on top of these contracts.

use thiserror::Error;

/// Stable status of an operation record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Running,
    Succeeded,
    Failed,
    Canceled,
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
/// Commit 1 only defines stable validation and paging helpers. Database-backed
/// methods are added in later A-6 commits.
#[derive(Debug, Default)]
pub struct OperationService;

impl OperationService {
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
    use super::{OperationQueryPage, OperationRecord, OperationService, OperationStatus};

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
    fn normalize_query_page_clamps_to_limits() {
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
    fn validate_record_rejects_invalid_timestamps() {
        let mut record = sample_record();
        record.end_ts = Some(99);

        let error = OperationService::validate_record(&record).unwrap_err();
        assert!(error
            .to_string()
            .contains("end_ts must be greater than or equal to start_ts"));
    }
}