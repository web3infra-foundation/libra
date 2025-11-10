use std::str::FromStr;

use git_internal::hash::SHA1;

use crate::internal::branch::Branch;
use crate::internal::db::{DbConnection, get_db_conn_instance};
use crate::internal::model::reference::{ConfigKind, Model};

#[derive(Debug, Clone)]
pub enum Head {
    Detached(SHA1),
    Branch(String),
}

/*
 * =================================================================================
 * NOTE: Transaction Safety Pattern (`_with_conn`)
 * =================================================================================
 *
 * This module follows the `_with_conn` pattern for transaction safety.
 *
 * - Public functions (e.g., `current`, `update`) acquire a new database
 *   connection from the pool and are suitable for single, non-transactional operations.
 *
 * - `*_with_conn` variants (e.g., `current_with_conn`, `update_with_conn`)
 *   accept an existing connection handle (`&DbConnection`).
 *
 * **WARNING**: To use these functions within a database transaction (e.g., inside
 * a `db.transaction(|txn| { ... })` block), you MUST call the `*_with_conn`
 * variant, passing the connection handle. Calling a public version from
 * inside a transaction will try to acquire a second connection from the pool,
 * leading to a deadlock.
 *
 * Correct Usage (in a transaction): `Head::update_with_conn(db, ...).await;`
 * Incorrect Usage (in a transaction): `Head::update(...).await;` // DEADLOCK!
 */

impl Head {
    async fn query_local_head_with_conn(db: &DbConnection) -> Model {
        let sql = "SELECT * FROM reference WHERE kind = ?1 AND remote IS NULL";
        let mut rows = db
            .query(sql, turso::params![ConfigKind::Head.as_str()])
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .map(|row| Model::from_row(&row).unwrap())
            .expect("fatal: storage broken, HEAD not found")
    }

    async fn query_remote_head_with_conn(db: &DbConnection, remote: &str) -> Option<Model> {
        let sql = "SELECT * FROM reference WHERE kind = ?1 AND remote = ?2";
        let mut rows = db
            .query(sql, turso::params![ConfigKind::Head.as_str(), remote])
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .map(|row| Model::from_row(&row).unwrap())
    }

    pub async fn current_with_conn(db: &DbConnection) -> Head {
        let head = Self::query_local_head_with_conn(db).await;
        match head.name {
            Some(name) => Head::Branch(name),
            None => {
                let commit_hash = head.commit.expect("detached head without commit");
                Head::Detached(SHA1::from_str(commit_hash.as_str()).unwrap())
            }
        }
    }

    pub async fn current() -> Head {
        let db_conn = get_db_conn_instance().await;
        Self::current_with_conn(db_conn.as_ref()).await
    }

    pub async fn remote_current_with_conn(db: &DbConnection, remote: &str) -> Option<Head> {
        match Self::query_remote_head_with_conn(db, remote).await {
            Some(head) => Some(match head.name {
                Some(name) => Head::Branch(name),
                None => {
                    let commit_hash = head.commit.expect("detached head without commit");
                    Head::Detached(SHA1::from_str(commit_hash.as_str()).unwrap())
                }
            }),
            None => None,
        }
    }

    pub async fn remote_current(remote: &str) -> Option<Head> {
        let db_conn = get_db_conn_instance().await;
        Self::remote_current_with_conn(db_conn.as_ref(), remote).await
    }

    pub async fn current_commit_with_conn(db: &DbConnection) -> Option<SHA1> {
        match Self::current_with_conn(db).await {
            Head::Detached(commit_hash) => Some(commit_hash),
            Head::Branch(name) => {
                let branch = Branch::find_branch_with_conn(db, &name, None).await;
                branch.map(|b| b.commit)
            }
        }
    }

    /// get the commit hash of current head, return `None` if no commit
    pub async fn current_commit() -> Option<SHA1> {
        let db_conn = get_db_conn_instance().await;
        Self::current_commit_with_conn(db_conn.as_ref()).await
    }

    pub async fn update_with_conn(db: &DbConnection, new_head: Self, remote: Option<&str>) {
        let head = match remote {
            Some(remote) => Self::query_remote_head_with_conn(db, remote).await,
            None => Some(Self::query_local_head_with_conn(db).await),
        };

        match head {
            Some(head) => {
                // update existing HEAD
                let (name, commit) = match new_head {
                    Head::Detached(commit_hash) => (None::<String>, Some(commit_hash.to_string())),
                    Head::Branch(branch_name) => (Some(branch_name), None::<String>),
                };

                let sql = "UPDATE reference SET name = ?1, `commit` = ?2 WHERE id = ?3";
                db.execute(sql, turso::params![name, commit, head.id])
                    .await
                    .unwrap();
            }
            None => {
                // insert new HEAD
                let (name, commit) = match new_head {
                    Head::Detached(commit_hash) => (None::<String>, Some(commit_hash.to_string())),
                    Head::Branch(branch_name) => (Some(branch_name), None::<String>),
                };

                let sql =
                    "INSERT INTO reference (kind, name, `commit`, remote) VALUES (?1, ?2, ?3, ?4)";
                db.execute(
                    sql,
                    turso::params![ConfigKind::Head.as_str(), name, commit, remote],
                )
                .await
                .unwrap();
            }
        }
    }

    // HEAD is unique, update if exists, insert if not
    pub async fn update(new_head: Self, remote: Option<&str>) {
        let db_conn = get_db_conn_instance().await;
        Self::update_with_conn(db_conn.as_ref(), new_head, remote).await;
    }
}
