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
    //  `_with_conn` version for `list_branches`
    pub async fn list_branches_with_conn<C>(db: &C, remote: Option<&str>) -> Vec<Self>
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
            .unwrap();

        branches
            .iter()
            .filter_map(|branch| {
                // Skip branches with no commit (unborn/placeholder)
                let commit_str = branch.commit.as_ref()?;
                Some(Branch {
                    name: branch.name.as_ref().unwrap().clone(),
                    commit: ObjectHash::from_str(commit_str).unwrap(),
                    remote: branch.remote.clone(),
                })
            })
            .collect()
    }

    /// list all remote branches
    pub async fn list_branches(remote: Option<&str>) -> Vec<Self> {
        let db_conn = get_db_conn_instance().await;
        Self::list_branches_with_conn(&db_conn, remote).await
    }

    //  `_with_conn` version for `exists`
    pub async fn exists_with_conn<C>(db: &C, branch_name: &str) -> bool
    where
        C: ConnectionTrait,
    {
        let branch = Self::find_branch_with_conn(db, branch_name, None).await;
        branch.is_some()
    }

    /// is the branch exists
    pub async fn exists(branch_name: &str) -> bool {
        let db_conn = get_db_conn_instance().await;
        Self::exists_with_conn(&db_conn, branch_name).await
    }

    //  `_with_conn` version for `find_branch`
    pub async fn find_branch_with_conn<C>(
        db: &C,
        branch_name: &str,
        remote: Option<&str>,
    ) -> Option<Self>
    where
        C: ConnectionTrait,
    {
        let branch = match query_reference_with_conn(db, branch_name, remote).await {
            Ok(branch) => branch,
            Err(err) => {
                eprintln!("fatal: failed to query branch '{branch_name}': {err}");
                return None;
            }
        };
        match branch {
            Some(branch) => {
                // Return None if commit is None (unborn/placeholder)
                let commit_str = branch.commit.as_ref()?;
                Some(Branch {
                    name: branch.name.as_ref().unwrap().clone(),
                    commit: ObjectHash::from_str(commit_str).unwrap(),
                    remote: branch.remote.clone(),
                })
            }
            None => None,
        }
    }

    /// get the branch by name
    pub async fn find_branch(branch_name: &str, remote: Option<&str>) -> Option<Self> {
        let db_conn = get_db_conn_instance().await;
        Self::find_branch_with_conn(&db_conn, branch_name, remote).await
    }

    //  `_with_conn` version for `search_branch`
    pub async fn search_branch_with_conn<C>(db: &C, branch_name: &str) -> Vec<Self>
    where
        C: ConnectionTrait,
    {
        let mut branch_name_str = branch_name.to_string();
        let mut remote = String::new();

        let mut branches = vec![];
        if let Some(branch) = Self::find_branch_with_conn(db, &branch_name_str, None).await {
            branches.push(branch)
        }

        while let Some(index) = branch_name_str.find('/') {
            if !remote.is_empty() {
                remote += "/";
            }
            remote += branch_name_str.get(..index).unwrap();
            branch_name_str = branch_name_str.get(index + 1..).unwrap().to_string();
            // Important: Call the `_with_conn` variant inside the loop
            let branch = Self::find_branch_with_conn(db, &branch_name_str, Some(&remote)).await;
            if let Some(branch) = branch {
                branches.push(branch);
            }
        }
        branches
    }

    /// search branch with full name, return vec of branches
    /// e.g. `origin/sub/master/feature` may means `origin/sub/master` + `feature` or `origin/sub` + `master/feature`
    /// so we need to search all possible branches
    pub async fn search_branch(branch_name: &str) -> Vec<Self> {
        let db_conn = get_db_conn_instance().await;
        Self::search_branch_with_conn(&db_conn, branch_name).await
    }

    //  `_with_conn` version for `update_branch`
    pub async fn update_branch_with_conn<C>(
        db: &C,
        branch_name: &str,
        commit_hash: &str,
        remote: Option<&str>,
    ) where
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
                Err(err) => {
                    eprintln!("fatal: failed to query branch '{branch_name}': {err}");
                    return;
                }
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
                Ok(()) => return,
                Err(err) if is_sqlite_busy(&err) && attempt < SQLITE_BUSY_MAX_RETRIES => {
                    sleep(Duration::from_millis(
                        SQLITE_BUSY_RETRY_BASE_MS * (attempt as u64 + 1),
                    ))
                    .await;
                }
                Err(err) => {
                    eprintln!("fatal: failed to update branch '{branch_name}': {err}");
                    return;
                }
            }
        }
    }

    pub async fn update_branch(branch_name: &str, commit_hash: &str, remote: Option<&str>) {
        let db_conn = get_db_conn_instance().await;
        Self::update_branch_with_conn(&db_conn, branch_name, commit_hash, remote).await
    }

    // `_with_conn` version for `delete_branch`
    pub async fn delete_branch_with_conn<C>(db: &C, branch_name: &str, remote: Option<&str>)
    where
        C: ConnectionTrait,
    {
        let branch = match query_reference_with_conn(db, branch_name, remote).await {
            Ok(branch) => branch,
            Err(err) => {
                eprintln!("fatal: failed to query branch '{branch_name}': {err}");
                return;
            }
        };
        let Some(branch) = branch else {
            eprintln!("fatal: branch '{branch_name}' not found");
            return;
        };
        let branch: reference::ActiveModel = branch.into();
        if let Err(err) = branch.delete(db).await {
            eprintln!("fatal: failed to delete branch '{branch_name}': {err}");
        }
    }

    pub async fn delete_branch(branch_name: &str, remote: Option<&str>) {
        let db_conn = get_db_conn_instance().await;
        Self::delete_branch_with_conn(&db_conn, branch_name, remote).await
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
        Branch::update_branch("upstream/origin/master", &commit_hash, None).await; // should match
        Branch::update_branch("origin/master", &commit_hash, Some("upstream")).await; // should match
        Branch::update_branch("master", &commit_hash, Some("upstream/origin")).await; // should match
        Branch::update_branch("feature", &commit_hash, Some("upstream/origin/master")).await; // should not match

        let branches = Branch::search_branch("upstream/origin/master").await;
        assert_eq!(branches.len(), 3);
    }
}
