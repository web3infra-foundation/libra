//! HEAD management backed by the database, supporting local and remote heads, detached states, and transaction-safe query/update helpers.

use std::{str::FromStr, time::Duration};

use git_internal::hash::ObjectHash;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait,
    QueryFilter,
};
use tokio::time::sleep;

use crate::internal::{
    branch::{Branch, BranchStoreError},
    db::get_db_conn_instance,
    model::reference,
};

#[derive(Debug, Clone)]
pub enum Head {
    Detached(ObjectHash),
    Branch(String),
}

/*
 * =================================================================================
 * NOTE: Transaction Safety Pattern (`_with_conn`)
 * =================================================================================
 *
 * This module follows the `_with_conn` pattern for transaction safety.
 *
 * - Public functions (e.g., `get`, `update`) acquire a new database
 *   connection from the pool and are suitable for single, non-transactional operations.
 *
 * - `*_with_conn` variants (e.g., `get_with_conn`, `update_with_conn`)
 *   accept an existing connection or transaction handle (`&C where C: ConnectionTrait`).
 *
 * **WARNING**: To use these functions within a database transaction (e.g., inside
 * a `db.transaction(|txn| { ... })` block), you MUST call the `*_with_conn`
 * variant, passing the transaction handle `txn`. Calling a public version from
 * inside a transaction will try to acquire a second connection from the pool,
 * leading to a deadlock.
 *
 * Correct Usage (in a transaction): `Head::update_with_conn(txn, ...).await;`
 * Incorrect Usage (in a transaction): `Head::update(...).await;` // DEADLOCK!
 */

impl Head {
    const SQLITE_BUSY_MAX_RETRIES: usize = 15;
    const SQLITE_BUSY_RETRY_BASE_MS: u64 = 100;

    fn is_sqlite_busy(err: &DbErr) -> bool {
        let message = err.to_string();
        message.contains("database is locked") || message.contains("database schema is locked")
    }

    async fn query_local_head_result_with_conn<C>(
        db: &C,
    ) -> Result<reference::Model, BranchStoreError>
    where
        C: ConnectionTrait,
    {
        for attempt in 0..=Self::SQLITE_BUSY_MAX_RETRIES {
            match reference::Entity::find()
                .filter(reference::Column::Kind.eq(reference::ConfigKind::Head))
                .filter(reference::Column::Remote.is_null())
                .one(db)
                .await
            {
                Ok(Some(model)) => return Ok(model),
                Ok(None) => {
                    return Err(BranchStoreError::Corrupt {
                        name: "HEAD".to_string(),
                        detail: "HEAD reference is missing from storage".to_string(),
                    });
                }
                Err(err)
                    if Self::is_sqlite_busy(&err) && attempt < Self::SQLITE_BUSY_MAX_RETRIES =>
                {
                    sleep(Duration::from_millis(
                        Self::SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                }
                Err(err) => return Err(BranchStoreError::Query(err.to_string())),
            }
        }

        unreachable!("sqlite retry loop must return")
    }

    async fn query_local_head_with_conn<C>(db: &C) -> reference::Model
    where
        C: ConnectionTrait,
    {
        for attempt in 0..=Self::SQLITE_BUSY_MAX_RETRIES {
            match reference::Entity::find()
                .filter(reference::Column::Kind.eq(reference::ConfigKind::Head))
                .filter(reference::Column::Remote.is_null())
                .one(db)
                .await
            {
                Ok(Some(model)) => return model,
                Ok(None) => panic!("fatal: storage broken, HEAD not found"),
                Err(err)
                    if Self::is_sqlite_busy(&err) && attempt < Self::SQLITE_BUSY_MAX_RETRIES =>
                {
                    sleep(Duration::from_millis(
                        Self::SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                }
                Err(err) => panic!("fatal: failed to query HEAD: {err}"),
            }
        }

        unreachable!("sqlite retry loop must return")
    }

    async fn query_remote_head_with_conn<C>(db: &C, remote: &str) -> Option<reference::Model>
    where
        C: ConnectionTrait,
    {
        for attempt in 0..=Self::SQLITE_BUSY_MAX_RETRIES {
            match reference::Entity::find()
                .filter(reference::Column::Kind.eq(reference::ConfigKind::Head))
                .filter(reference::Column::Remote.eq(remote))
                .one(db)
                .await
            {
                Ok(model) => return model,
                Err(err)
                    if Self::is_sqlite_busy(&err) && attempt < Self::SQLITE_BUSY_MAX_RETRIES =>
                {
                    sleep(Duration::from_millis(
                        Self::SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                }
                Err(err) => {
                    tracing::error!(
                        remote,
                        error = %err,
                        "Failed to query remote HEAD"
                    );
                    return None;
                }
            }
        }

        None
    }

    pub async fn current_with_conn<C>(db: &C) -> Head
    where
        C: ConnectionTrait,
    {
        let head = Self::query_local_head_with_conn(db).await;
        match head.name {
            Some(name) => Head::Branch(name),
            None => {
                let commit_hash = head.commit.expect("detached head without commit");
                Head::Detached(ObjectHash::from_str(commit_hash.as_str()).unwrap())
            }
        }
    }

    pub async fn current() -> Head {
        let db_conn = get_db_conn_instance().await;
        Self::current_with_conn(&db_conn).await
    }

    pub async fn current_result_with_conn<C>(db: &C) -> Result<Head, BranchStoreError>
    where
        C: ConnectionTrait,
    {
        let head = Self::query_local_head_result_with_conn(db).await?;
        match head.name {
            Some(name) => Ok(Head::Branch(name)),
            None => {
                let commit_hash = head.commit.ok_or_else(|| BranchStoreError::Corrupt {
                    name: "HEAD".to_string(),
                    detail: "detached HEAD is missing commit hash".to_string(),
                })?;
                let commit_hash = ObjectHash::from_str(commit_hash.as_str()).map_err(|error| {
                    BranchStoreError::Corrupt {
                        name: "HEAD".to_string(),
                        detail: format!("invalid detached HEAD commit hash: {error}"),
                    }
                })?;
                Ok(Head::Detached(commit_hash))
            }
        }
    }

    pub async fn current_result() -> Result<Head, BranchStoreError> {
        let db_conn = get_db_conn_instance().await;
        Self::current_result_with_conn(&db_conn).await
    }

    pub async fn remote_current_with_conn<C>(db: &C, remote: &str) -> Option<Head>
    where
        C: ConnectionTrait,
    {
        match Self::query_remote_head_with_conn(db, remote).await {
            Some(head) => Some(match head.name {
                Some(name) => Head::Branch(name),
                None => {
                    let commit_hash = head.commit.expect("detached head without commit");
                    Head::Detached(ObjectHash::from_str(commit_hash.as_str()).unwrap())
                }
            }),
            None => None,
        }
    }

    pub async fn remote_current(remote: &str) -> Option<Head> {
        let db_conn = get_db_conn_instance().await;
        Self::remote_current_with_conn(&db_conn, remote).await
    }

    /// Resolve HEAD to its current commit hash.
    ///
    /// Returns `Ok(None)` when HEAD is an **unborn branch** — i.e. HEAD points
    /// to a branch name that has no row in the reference table yet.  This is the
    /// normal state after `libra init` before the first commit, and mirrors
    /// Git's semantics (HEAD → refs/heads/main, but the ref file does not
    /// exist).  It is **not** corruption; callers should treat `None` as
    /// "no commits yet" (e.g. use a zero OID for reflog entries).
    ///
    /// Actual storage failures (DB query errors, corrupt data) are surfaced as
    /// `Err(BranchStoreError)`.
    pub async fn current_commit_result_with_conn<C>(
        db: &C,
    ) -> Result<Option<ObjectHash>, BranchStoreError>
    where
        C: ConnectionTrait,
    {
        match Self::current_result_with_conn(db).await? {
            Head::Branch(name) => Ok(Branch::find_branch_result_with_conn(db, &name, None)
                .await?
                .map(|branch| branch.commit)),
            Head::Detached(commit_hash) => Ok(Some(commit_hash)),
        }
    }

    pub async fn current_commit_result() -> Result<Option<ObjectHash>, BranchStoreError> {
        let db_conn = get_db_conn_instance().await;
        Self::current_commit_result_with_conn(&db_conn).await
    }

    /// Lossy compatibility wrapper. Prefer `current_commit_result_with_conn`
    /// in production paths so storage failures are not downgraded to `None`.
    pub async fn current_commit_with_conn<C>(db: &C) -> Option<ObjectHash>
    where
        C: ConnectionTrait,
    {
        match Self::current_commit_result_with_conn(db).await {
            Ok(commit) => commit,
            Err(error) => {
                tracing::error!("failed to resolve HEAD commit: {error}");
                None
            }
        }
    }

    /// Lossy compatibility wrapper. Prefer `current_commit_result` in
    /// production paths so storage failures are not downgraded to `None`.
    pub async fn current_commit() -> Option<ObjectHash> {
        let db_conn = get_db_conn_instance().await;
        Self::current_commit_with_conn(&db_conn).await
    }

    pub async fn update_result_with_conn<C>(
        db: &C,
        new_head: Self,
        remote: Option<&str>,
    ) -> Result<(), BranchStoreError>
    where
        C: ConnectionTrait,
    {
        for attempt in 0..=Self::SQLITE_BUSY_MAX_RETRIES {
            let head = match remote {
                Some(remote) => Self::query_remote_head_with_conn(db, remote).await,
                None => Some(Self::query_local_head_result_with_conn(db).await?),
            };

            let write_result = match head {
                Some(head) => {
                    // update
                    let mut head: reference::ActiveModel = head.into();
                    if remote.is_some() {
                        head.remote = Set(remote.map(|s| s.to_owned()));
                    }
                    match &new_head {
                        Head::Detached(commit_hash) => {
                            head.commit = Set(Some(commit_hash.to_string()));
                            head.name = Set(None);
                        }
                        Head::Branch(branch_name) => {
                            head.name = Set(Some(branch_name.clone()));
                            head.commit = Set(None);
                        }
                    }
                    head.update(db).await.map(|_| ())
                }
                None => {
                    let mut head = reference::ActiveModel {
                        kind: Set(reference::ConfigKind::Head),
                        ..Default::default()
                    };
                    if remote.is_some() {
                        head.remote = Set(remote.map(|s| s.to_owned()));
                    }
                    match &new_head {
                        Head::Detached(commit_hash) => {
                            head.commit = Set(Some(commit_hash.to_string()));
                        }
                        Head::Branch(branch_name) => {
                            head.name = Set(Some(branch_name.clone()));
                        }
                    }
                    head.save(db).await.map(|_| ())
                }
            };

            match write_result {
                Ok(()) => return Ok(()),
                Err(err)
                    if Self::is_sqlite_busy(&err) && attempt < Self::SQLITE_BUSY_MAX_RETRIES =>
                {
                    sleep(Duration::from_millis(
                        Self::SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                }
                Err(err) => return Err(BranchStoreError::Query(err.to_string())),
            }
        }

        Err(BranchStoreError::Query(
            "failed to update HEAD reference after sqlite busy retries".to_string(),
        ))
    }

    pub async fn update_with_conn<C>(db: &C, new_head: Self, remote: Option<&str>)
    where
        C: ConnectionTrait,
    {
        if let Err(error) = Self::update_result_with_conn(db, new_head, remote).await {
            if remote.is_none() {
                panic!("fatal: failed to update HEAD reference: {error}");
            }
            tracing::error!(
                remote = ?remote,
                error = %error,
                "Failed to update HEAD reference"
            );
        }
    }

    pub async fn update_result(
        new_head: Self,
        remote: Option<&str>,
    ) -> Result<(), BranchStoreError> {
        let db_conn = get_db_conn_instance().await;
        Self::update_result_with_conn(&db_conn, new_head, remote).await
    }

    // HEAD is unique, update if exists, insert if not
    pub async fn update(new_head: Self, remote: Option<&str>) {
        let db_conn = get_db_conn_instance().await;
        Self::update_with_conn(&db_conn, new_head, remote).await;
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::utils::test::{self, ChangeDirGuard};

    #[tokio::test]
    #[serial]
    async fn current_commit_result_with_conn_returns_corrupt_when_head_row_missing() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = ChangeDirGuard::new(repo.path());

        let db = get_db_conn_instance().await;
        reference::Entity::delete_many()
            .filter(reference::Column::Kind.eq(reference::ConfigKind::Head))
            .filter(reference::Column::Remote.is_null())
            .exec(&db)
            .await
            .unwrap();

        let error = Head::current_commit_result_with_conn(&db)
            .await
            .expect_err("missing HEAD row should surface as corruption");
        assert!(matches!(error, BranchStoreError::Corrupt { .. }));
        assert!(
            error.to_string().contains("HEAD reference is missing"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn current_commit_result_with_conn_returns_corrupt_for_invalid_detached_hash() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = ChangeDirGuard::new(repo.path());

        let db = get_db_conn_instance().await;
        let head = reference::Entity::find()
            .filter(reference::Column::Kind.eq(reference::ConfigKind::Head))
            .filter(reference::Column::Remote.is_null())
            .one(&db)
            .await
            .unwrap()
            .expect("expected HEAD row");
        let mut head: reference::ActiveModel = head.into();
        head.name = Set(None);
        head.commit = Set(Some("not-a-valid-hash".to_string()));
        head.update(&db).await.unwrap();

        let error = Head::current_commit_result_with_conn(&db)
            .await
            .expect_err("invalid detached HEAD hash should surface as corruption");
        assert!(matches!(error, BranchStoreError::Corrupt { .. }));
        assert!(
            error
                .to_string()
                .contains("invalid detached HEAD commit hash"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    #[serial]
    #[should_panic(expected = "fatal: failed to update HEAD reference")]
    async fn update_with_conn_panics_when_local_head_row_missing() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = ChangeDirGuard::new(repo.path());

        let db = get_db_conn_instance().await;
        reference::Entity::delete_many()
            .filter(reference::Column::Kind.eq(reference::ConfigKind::Head))
            .filter(reference::Column::Remote.is_null())
            .exec(&db)
            .await
            .unwrap();

        Head::update_with_conn(&db, Head::Branch("main".to_string()), None).await;
    }
}
