//! Transaction wrapper contract for operation-level audit logging.
//!
//! Commit 1 introduces only stable wrapper-facing types that are required by
//! A-5: metadata, snapshot scope, wrapper result, and stage-specific errors.
//! Execution logic is intentionally deferred to later commits.

use thiserror::Error;

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

#[cfg(test)]
mod tests {
    use super::{OperationError, OperationMeta, OperationScope};

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
}
