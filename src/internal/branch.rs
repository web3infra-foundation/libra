use std::str::FromStr;

use git_internal::hash::SHA1;

use crate::internal::db::{DbConnection, get_db_conn_instance};
use crate::internal::model::reference::{ConfigKind, Model};

#[derive(Debug)]
pub struct Branch {
    pub name: String,
    pub commit: SHA1,
    pub remote: Option<String>,
}

//  `_with_conn` version of the helper function
async fn query_reference_with_conn(
    db: &DbConnection,
    branch_name: &str,
    remote: Option<&str>,
) -> Option<Model> {
    let mut rows = match remote {
        Some(r) => {
            let sql = "SELECT * FROM reference WHERE name = ?1 AND kind = ?2 AND remote = ?3";
            db.query(
                sql,
                turso::params![branch_name, ConfigKind::Branch.as_str(), r],
            )
            .await
            .unwrap()
        }
        None => {
            let sql = "SELECT * FROM reference WHERE name = ?1 AND kind = ?2 AND remote IS NULL";
            db.query(
                sql,
                turso::params![branch_name, ConfigKind::Branch.as_str()],
            )
            .await
            .unwrap()
        }
    };

    rows.next()
        .await
        .unwrap()
        .map(|row| Model::from_row(&row).unwrap())
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
 *   accept an existing connection handle (`&DbConnection`).
 *
 * **WARNING**: To use these functions within a database transaction (e.g., inside
 * a `db.transaction(|txn| { ... })` block), you MUST call the `*_with_conn`
 * variant, passing the connection handle. Calling a public version from
 * inside a transaction will try to acquire a second connection from the pool,
 * leading to a deadlock.
 *
 * Correct Usage (in a transaction): `Branch::update_branch_with_conn(db, ...).await;`
 * Incorrect Usage (in a transaction): `Branch::update_branch(...).await;` // DEADLOCK!
 */
impl Branch {
    //  `_with_conn` version for `list_branches`
    pub async fn list_branches_with_conn(db: &DbConnection, remote: Option<&str>) -> Vec<Self> {
        let mut rows = match remote {
            Some(r) => {
                let sql = "SELECT * FROM reference WHERE kind = ?1 AND remote = ?2";
                db.query(sql, turso::params![ConfigKind::Branch.as_str(), r])
                    .await
                    .unwrap()
            }
            None => {
                let sql = "SELECT * FROM reference WHERE kind = ?1 AND remote IS NULL";
                db.query(sql, turso::params![ConfigKind::Branch.as_str()])
                    .await
                    .unwrap()
            }
        };
        let mut branches = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let branch_model = Model::from_row(&row).unwrap();
            branches.push(Branch {
                name: branch_model.name.clone().unwrap(),
                commit: SHA1::from_str(branch_model.commit.as_ref().unwrap()).unwrap(),
                remote: branch_model.remote.clone(),
            });
        }
        branches
    }

    /// list all remote branches
    pub async fn list_branches(remote: Option<&str>) -> Vec<Self> {
        let db_conn = get_db_conn_instance().await;
        Self::list_branches_with_conn(db_conn.as_ref(), remote).await
    }

    //  `_with_conn` version for `exists`
    pub async fn exists_with_conn(db: &DbConnection, branch_name: &str) -> bool {
        let branch = Self::find_branch_with_conn(db, branch_name, None).await;
        branch.is_some()
    }

    /// is the branch exists
    pub async fn exists(branch_name: &str) -> bool {
        let db_conn = get_db_conn_instance().await;
        Self::exists_with_conn(db_conn.as_ref(), branch_name).await
    }

    //  `_with_conn` version for `find_branch`
    pub async fn find_branch_with_conn(
        db: &DbConnection,
        branch_name: &str,
        remote: Option<&str>,
    ) -> Option<Self> {
        let branch = query_reference_with_conn(db, branch_name, remote).await;
        match branch {
            Some(branch) => Some(Branch {
                name: branch.name.as_ref().unwrap().clone(),
                commit: SHA1::from_str(branch.commit.as_ref().unwrap()).unwrap(),
                remote: branch.remote.clone(),
            }),
            None => None,
        }
    }

    /// get the branch by name
    pub async fn find_branch(branch_name: &str, remote: Option<&str>) -> Option<Self> {
        let db_conn = get_db_conn_instance().await;
        Self::find_branch_with_conn(db_conn.as_ref(), branch_name, remote).await
    }

    //  `_with_conn` version for `search_branch`
    pub async fn search_branch_with_conn(db: &DbConnection, branch_name: &str) -> Vec<Self> {
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
        Self::search_branch_with_conn(db_conn.as_ref(), branch_name).await
    }

    //  `_with_conn` version for `update_branch`
    pub async fn update_branch_with_conn(
        db: &DbConnection,
        branch_name: &str,
        commit_hash: &str,
        remote: Option<&str>,
    ) {
        let branch = query_reference_with_conn(db, branch_name, remote).await;

        match branch {
            Some(branch_model) => {
                let sql = "UPDATE reference SET `commit` = ?1 WHERE id = ?2";
                db.execute(sql, turso::params![commit_hash, branch_model.id])
                    .await
                    .unwrap();
            }
            None => {
                let sql =
                    "INSERT INTO reference (name, kind, `commit`, remote) VALUES (?1, ?2, ?3, ?4)";
                db.execute(
                    sql,
                    turso::params![
                        branch_name,
                        ConfigKind::Branch.as_str(),
                        commit_hash,
                        remote
                    ],
                )
                .await
                .unwrap();
            }
        }
    }

    pub async fn update_branch(branch_name: &str, commit_hash: &str, remote: Option<&str>) {
        let db_conn = get_db_conn_instance().await;
        Self::update_branch_with_conn(db_conn.as_ref(), branch_name, commit_hash, remote).await
    }

    // `_with_conn` version for `delete_branch`
    pub async fn delete_branch_with_conn(
        db: &DbConnection,
        branch_name: &str,
        remote: Option<&str>,
    ) {
        let branch = query_reference_with_conn(db, branch_name, remote)
            .await
            .unwrap();
        let sql = "DELETE FROM reference WHERE id = ?1";
        db.execute(sql, turso::params![branch.id]).await.unwrap();
    }

    pub async fn delete_branch(branch_name: &str, remote: Option<&str>) {
        let db_conn = get_db_conn_instance().await;
        Self::delete_branch_with_conn(db_conn.as_ref(), branch_name, remote).await
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::test;
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    #[serial]
    async fn test_search_branch() {
        let temp_path = tempdir().unwrap();
        test::setup_with_new_libra_in(temp_path.path()).await;
        let _guard = test::ChangeDirGuard::new(temp_path.path());

        let commit_hash = SHA1::default().to_string();
        Branch::update_branch("upstream/origin/master", &commit_hash, None).await; // should match
        Branch::update_branch("origin/master", &commit_hash, Some("upstream")).await; // should match
        Branch::update_branch("master", &commit_hash, Some("upstream/origin")).await; // should match
        Branch::update_branch("feature", &commit_hash, Some("upstream/origin/master")).await; // should not match

        let branches = Branch::search_branch("upstream/origin/master").await;
        assert_eq!(branches.len(), 3);
    }
}
