//! Branch store utilities to find/create/update/delete branch refs in the database with transaction-safe helpers and commit resolution.

use std::{str::FromStr, time::Duration};

use git_internal::hash::ObjectHash;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait,
    QueryFilter,
};
use tokio::time::sleep;

use crate::internal::{db::get_db_conn_instance, model::reference};

pub const DEFAULT_BRANCH: &str = "main";
pub const INTENT_BRANCH: &str = "intent";

pub fn is_locked_branch(name: &str) -> bool {
    name == DEFAULT_BRANCH || name == INTENT_BRANCH
}

#[derive(Debug)]
pub struct Branch {
    pub name: String,
    pub commit: ObjectHash,
    pub remote: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BranchStoreError {
    #[error("failed to query branch storage: {0}")]
    Query(String),
    #[error("stored branch reference '{name}' is corrupt: {detail}")]
    Corrupt { name: String, detail: String },
    #[error("branch '{0}' not found")]
    NotFound(String),
    #[error("failed to delete branch '{name}': {detail}")]
    Delete { name: String, detail: String },
}

fn log_branch_store_error(context: &str, error: &BranchStoreError) {
    tracing::error!("{context}: {error}");
}

fn branch_from_model(model: reference::Model) -> Result<Option<Branch>, BranchStoreError> {
    let Some(name) = model.name.clone() else {
        return Err(BranchStoreError::Corrupt {
            name: "<unknown>".to_string(),
            detail: "missing name field".to_string(),
        });
    };
    let Some(commit_str) = model.commit.as_ref() else {
        return Ok(None);
    };
    let commit = ObjectHash::from_str(commit_str).map_err(|e| BranchStoreError::Corrupt {
        name: name.clone(),
        detail: e.to_string(),
    })?;
    Ok(Some(Branch {
        name,
        commit,
        remote: model.remote.clone(),
    }))
}

//  `_with_conn` version of the helper function
async fn query_reference_with_conn<C>(
    db: &C,
    branch_name: &str,
    remote: Option<&str>,
) -> Result<Option<reference::Model>, DbErr>
where
    C: ConnectionTrait,
{
    reference::Entity::find()
        .filter(reference::Column::Name.eq(branch_name))
        .filter(reference::Column::Kind.eq(reference::ConfigKind::Branch))
        .filter(match remote {
            Some(remote) => reference::Column::Remote.eq(remote),
            None => reference::Column::Remote.is_null(),
        })
        .one(db)
        .await
}

const SQLITE_BUSY_MAX_RETRIES: usize = 15;
const SQLITE_BUSY_RETRY_BASE_MS: u64 = 100;

fn is_sqlite_busy(err: &DbErr) -> bool {
    let message = err.to_string();
    message.contains("database is locked") || message.contains("database schema is locked")
}

/*
 * =================================================================================
 * NOTE: Transaction Safety Pattern (`_with_conn`)
 * =================================================================================
 *
 * This module follows the `_with_conn` pattern for transaction safety.
 *
 * - Public functions (e.g., `find_branch`, `update_branch`) acquire a new database
 *   connection from the pool and are suitable for single, non-transactional operations.
 *
 * - `*_with_conn` variants (e.g., `find_branch_with_conn`, `update_branch_with_conn`)
 *   accept an existing connection or transaction handle (`&C where C: ConnectionTrait`).
 *
 * **WARNING**: To use these functions within a database transaction (e.g., inside
 * a `db.transaction(|txn| { ... })` block), you MUST call the `*_with_conn`
 * variant, passing the transaction handle `txn`. Calling a public version from
 * inside a transaction will try to acquire a second connection from the pool,
 * leading to a deadlock.
 *
 * Correct Usage (in a transaction): `Branch::update_branch_with_conn(txn, ...).await;`
 * Incorrect Usage (in a transaction): `Branch::update_branch(...).await;` // DEADLOCK!
 */
impl Branch {
    pub async fn list_branches_result_with_conn<C>(
        db: &C,
        remote: Option<&str>,
    ) -> Result<Vec<Self>, BranchStoreError>
    where
        C: ConnectionTrait,
    {
        let branches = reference::Entity::find()
            .filter(reference::Column::Kind.eq(reference::ConfigKind::Branch))
            .filter(match remote {
                Some(remote) => reference::Column::Remote.eq(remote),
                None => reference::Column::Remote.is_null(),
            })
            .all(db)
            .await
            .map_err(|err| BranchStoreError::Query(err.to_string()))?;

        let mut resolved = Vec::new();
        for branch in branches {
            if let Some(branch) = branch_from_model(branch)? {
                resolved.push(branch);
            }
        }
        Ok(resolved)
    }

    /// Lossy compatibility wrapper. Prefer `list_branches_result_with_conn` in
    /// production paths so storage failures are not downgraded to an empty list.
    pub async fn list_branches_with_conn<C>(db: &C, remote: Option<&str>) -> Vec<Self>
    where
        C: ConnectionTrait,
    {
        match Self::list_branches_result_with_conn(db, remote).await {
            Ok(branches) => branches,
            Err(error) => {
                log_branch_store_error("failed to list branches", &error);
                Vec::new()
            }
        }
    }

    /// Lossy compatibility wrapper. Prefer `list_branches_result` in production
    /// paths so storage failures are not downgraded to an empty list.
    pub async fn list_branches(remote: Option<&str>) -> Vec<Self> {
        let db_conn = get_db_conn_instance().await;
        Self::list_branches_with_conn(&db_conn, remote).await
    }

    pub async fn list_branches_result(remote: Option<&str>) -> Result<Vec<Self>, BranchStoreError> {
        let db_conn = get_db_conn_instance().await;
        Self::list_branches_result_with_conn(&db_conn, remote).await
    }

    /// Lossy compatibility wrapper. Prefer `exists_result_with_conn` in
    /// production paths so storage failures are not downgraded to `false`.
    pub async fn exists_with_conn<C>(db: &C, branch_name: &str) -> bool
    where
        C: ConnectionTrait,
    {
        let branch = Self::find_branch_with_conn(db, branch_name, None).await;
        branch.is_some()
    }

    /// Lossy compatibility wrapper that only checks **local** branches
    /// (hardcodes `remote = None`). Prefer `exists_result` in production paths
    /// so storage failures are not downgraded to `false` and a remote scope can
    /// be specified.
    pub async fn exists(branch_name: &str) -> bool {
        let db_conn = get_db_conn_instance().await;
        Self::exists_with_conn(&db_conn, branch_name).await
    }

    pub async fn exists_result_with_conn<C>(
        db: &C,
        branch_name: &str,
        remote: Option<&str>,
    ) -> Result<bool, BranchStoreError>
    where
        C: ConnectionTrait,
    {
        query_reference_with_conn(db, branch_name, remote)
            .await
            .map(|branch| branch.is_some())
            .map_err(|err| BranchStoreError::Query(err.to_string()))
    }

    pub async fn exists_result(
        branch_name: &str,
        remote: Option<&str>,
    ) -> Result<bool, BranchStoreError> {
        let db_conn = get_db_conn_instance().await;
        Self::exists_result_with_conn(&db_conn, branch_name, remote).await
    }

    /// Lossy compatibility wrapper. Prefer `find_branch_result_with_conn` in
    /// production paths so storage failures are not downgraded to `None`.
    pub async fn find_branch_with_conn<C>(
        db: &C,
        branch_name: &str,
        remote: Option<&str>,
    ) -> Option<Self>
    where
        C: ConnectionTrait,
    {
        match Self::find_branch_result_with_conn(db, branch_name, remote).await {
            Ok(branch) => branch,
            Err(error) => {
                log_branch_store_error(
                    &format!(
                        "failed to resolve branch lookup for '{}'{}",
                        branch_name,
                        remote
                            .map(|name| format!(" on remote '{name}'"))
                            .unwrap_or_default()
                    ),
                    &error,
                );
                None
            }
        }
    }

    /// Lossy compatibility wrapper. Prefer `find_branch_result` in production
    /// paths so storage failures are not downgraded to `None`.
    pub async fn find_branch(branch_name: &str, remote: Option<&str>) -> Option<Self> {
        let db_conn = get_db_conn_instance().await;
        Self::find_branch_with_conn(&db_conn, branch_name, remote).await
    }

    pub async fn find_branch_result_with_conn<C>(
        db: &C,
        branch_name: &str,
        remote: Option<&str>,
    ) -> Result<Option<Self>, BranchStoreError>
    where
        C: ConnectionTrait,
    {
        let branch = query_reference_with_conn(db, branch_name, remote)
            .await
            .map_err(|err| BranchStoreError::Query(err.to_string()))?;
        match branch {
            Some(branch) => branch_from_model(branch),
            None => Ok(None),
        }
    }

    pub async fn find_branch_result(
        branch_name: &str,
        remote: Option<&str>,
    ) -> Result<Option<Self>, BranchStoreError> {
        let db_conn = get_db_conn_instance().await;
        Self::find_branch_result_with_conn(&db_conn, branch_name, remote).await
    }

    //  `_with_conn` version for `search_branch`
    pub async fn search_branch_with_conn<C>(db: &C, branch_name: &str) -> Vec<Self>
    where
        C: ConnectionTrait,
    {
        match Self::search_branch_result_with_conn(db, branch_name).await {
            Ok(branches) => branches,
            Err(error) => {
                log_branch_store_error(
                    &format!("failed to search branches matching '{branch_name}'"),
                    &error,
                );
                Vec::new()
            }
        }
    }

    pub async fn search_branch_result_with_conn<C>(
        db: &C,
        branch_name: &str,
    ) -> Result<Vec<Self>, BranchStoreError>
    where
        C: ConnectionTrait,
    {
        let mut branch_name_str = branch_name.to_string();
        let mut remote = String::new();

        let mut branches = vec![];
        if let Some(branch) = Self::find_branch_result_with_conn(db, &branch_name_str, None).await?
        {
            branches.push(branch)
        }

        while let Some(index) = branch_name_str.find('/') {
            let (remote_segment, remainder) = branch_name_str.split_at(index);
            let remainder =
                remainder
                    .strip_prefix('/')
                    .ok_or_else(|| BranchStoreError::Corrupt {
                        name: branch_name.to_string(),
                        detail: format!("failed to split branch search path '{branch_name_str}'"),
                    })?;
            if !remote.is_empty() {
                remote += "/";
            }
            remote += remote_segment;
            branch_name_str = remainder.to_string();
            if let Some(branch) =
                Self::find_branch_result_with_conn(db, &branch_name_str, Some(&remote)).await?
            {
                branches.push(branch);
            }
        }
        Ok(branches)
    }

    /// search branch with full name, return vec of branches
    /// e.g. `origin/sub/master/feature` may means `origin/sub/master` + `feature` or `origin/sub` + `master/feature`
    /// so we need to search all possible branches
    pub async fn search_branch(branch_name: &str) -> Vec<Self> {
        let db_conn = get_db_conn_instance().await;
        Self::search_branch_with_conn(&db_conn, branch_name).await
    }

    pub async fn search_branch_result(branch_name: &str) -> Result<Vec<Self>, BranchStoreError> {
        let db_conn = get_db_conn_instance().await;
        Self::search_branch_result_with_conn(&db_conn, branch_name).await
    }

    //  `_with_conn` version for `update_branch`
    pub async fn update_branch_with_conn<C>(
        db: &C,
        branch_name: &str,
        commit_hash: &str,
        remote: Option<&str>,
    ) -> Result<(), DbErr>
    where
        C: ConnectionTrait,
    {
        for attempt in 0..=SQLITE_BUSY_MAX_RETRIES {
            let branch = match query_reference_with_conn(db, branch_name, remote).await {
                Ok(branch) => branch,
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                    continue;
                }
                Err(err) => return Err(err),
            };

            let write_result = match branch {
                Some(branch) => {
                    let mut branch: reference::ActiveModel = branch.into();
                    branch.commit = Set(Some(commit_hash.to_owned()));
                    branch.update(db).await.map(|_| ())
                }
                None => reference::ActiveModel {
                    name: Set(Some(branch_name.to_owned())),
                    kind: Set(reference::ConfigKind::Branch),
                    commit: Set(Some(commit_hash.to_owned())),
                    remote: Set(remote.map(|s| s.to_owned())),
                    ..Default::default()
                }
                .insert(db)
                .await
                .map(|_| ()),
            };

            match write_result {
                Ok(()) => return Ok(()),
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                }
                Err(err) => return Err(err),
            }
        }
        unreachable!("sqlite retry loop must return")
    }

    pub async fn update_branch(
        branch_name: &str,
        commit_hash: &str,
        remote: Option<&str>,
    ) -> Result<(), DbErr> {
        let db_conn = get_db_conn_instance().await;
        Self::update_branch_with_conn(&db_conn, branch_name, commit_hash, remote).await
    }

    // `_with_conn` version for `delete_branch`
    pub async fn delete_branch_with_conn<C>(db: &C, branch_name: &str, remote: Option<&str>)
    where
        C: ConnectionTrait,
    {
        if let Err(error) = Self::delete_branch_result_with_conn(db, branch_name, remote).await {
            log_branch_store_error(
                &format!(
                    "failed to delete branch '{}'{}",
                    branch_name,
                    remote
                        .map(|name| format!(" on remote '{name}'"))
                        .unwrap_or_default()
                ),
                &error,
            );
        }
    }

    pub async fn delete_branch(branch_name: &str, remote: Option<&str>) {
        let db_conn = get_db_conn_instance().await;
        Self::delete_branch_with_conn(&db_conn, branch_name, remote).await
    }

    pub async fn delete_branch_result_with_conn<C>(
        db: &C,
        branch_name: &str,
        remote: Option<&str>,
    ) -> Result<(), BranchStoreError>
    where
        C: ConnectionTrait,
    {
        let branch = query_reference_with_conn(db, branch_name, remote)
            .await
            .map_err(|err| BranchStoreError::Query(err.to_string()))?;
        let Some(branch) = branch else {
            return Err(BranchStoreError::NotFound(branch_name.to_string()));
        };
        let branch: reference::ActiveModel = branch.into();
        branch
            .delete(db)
            .await
            .map(|_| ())
            .map_err(|err| BranchStoreError::Delete {
                name: branch_name.to_string(),
                detail: err.to_string(),
            })
    }

    pub async fn delete_branch_result(
        branch_name: &str,
        remote: Option<&str>,
    ) -> Result<(), BranchStoreError> {
        let db_conn = get_db_conn_instance().await;
        Self::delete_branch_result_with_conn(&db_conn, branch_name, remote).await
    }
}

#[cfg(test)]
mod tests {
    use git_internal::hash::{HashKind, get_hash_kind, set_hash_kind_for_test};
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::utils::test;

    #[tokio::test]
    #[serial]
    async fn test_search_branch() {
        let _guard = set_hash_kind_for_test(HashKind::Sha256);
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let commit_hash = ObjectHash::zero_str(get_hash_kind()).to_string();
        Branch::update_branch("upstream/origin/master", &commit_hash, None)
            .await
            .unwrap(); // should match
        Branch::update_branch("origin/master", &commit_hash, Some("upstream"))
            .await
            .unwrap(); // should match
        Branch::update_branch("master", &commit_hash, Some("upstream/origin"))
            .await
            .unwrap(); // should match
        Branch::update_branch("feature", &commit_hash, Some("upstream/origin/master"))
            .await
            .unwrap(); // should not match

        let branches = Branch::search_branch("upstream/origin/master").await;
        assert_eq!(branches.len(), 3);
    }
}
