use std::collections::HashSet;
use std::mem::swap;

use crate::internal::db::{DbConnection, get_db_conn_instance};
use crate::internal::head::Head;
use crate::internal::model::config::Model;

pub struct Config;

#[derive(Clone)]
pub struct RemoteConfig {
    pub name: String,
    pub url: String,
}
#[allow(dead_code)]
pub struct BranchConfig {
    pub name: String,
    pub merge: String,
    pub remote: String,
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
 *   accept an existing connection handle (`&DbConnection`).
 *
 * **WARNING**: To use these functions within a database transaction (e.g., inside
 * a `db.transaction(|txn| { ... })` block), you MUST call the `*_with_conn`
 * variant, passing the connection handle. Calling a public version from
 * inside a transaction will try to acquire a second connection from the pool,
 * leading to a deadlock.
 *
 * Correct Usage (in a transaction): `Config::update_with_conn(db, ...).await;`
 * Incorrect Usage (in a transaction): `Config::update(...).await;` // DEADLOCK!
 */
impl Config {
    // _with_conn version for insert
    pub async fn insert_with_conn(
        db: &DbConnection,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        value: &str,
    ) {
        let sql = "INSERT INTO config (configuration, name, key, value) VALUES (?1, ?2, ?3, ?4)";
        db.execute(sql, turso::params![configuration, name, key, value])
            .await
            .unwrap();
    }

    // _with_conn version for update
    pub async fn update_with_conn(
        db: &DbConnection,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        value: &str,
    ) -> Model {
        match name {
            Some(n) => {
                let sql = "UPDATE config SET value = ?4 WHERE configuration = ?1 AND name = ?2 AND key = ?3";
                db.execute(sql, turso::params![configuration, n, key, value])
                    .await
                    .unwrap();
            }
            None => {
                let sql = "UPDATE config SET value = ?3 WHERE configuration = ?1 AND name IS NULL AND key = ?2";
                db.execute(sql, turso::params![configuration, key, value])
                    .await
                    .unwrap();
            }
        }
        Self::get_with_conn(db, configuration, name, key)
            .await
            .unwrap()
    }

    // _with_conn version for query
    pub async fn query_with_conn(
        db: &DbConnection,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Vec<Model> {
        let mut result = Vec::new();
        match name {
            Some(n) => {
                let sql =
                    "SELECT * FROM config WHERE configuration = ?1 AND name = ?2 AND key = ?3";
                let mut rows = db
                    .query(sql, turso::params![configuration, n, key])
                    .await
                    .unwrap();
                while let Some(row) = rows.next().await.unwrap() {
                    result.push(Model::from_row(&row).unwrap());
                }
            }
            None => {
                let sql =
                    "SELECT * FROM config WHERE configuration = ?1 AND name IS NULL AND key = ?2";
                let mut rows = db
                    .query(sql, turso::params![configuration, key])
                    .await
                    .unwrap();
                while let Some(row) = rows.next().await.unwrap() {
                    result.push(Model::from_row(&row).unwrap());
                }
            }
        }
        result
    }

    // _with_conn version for get
    pub async fn get_with_conn(
        db: &DbConnection,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Option<Model> {
        let configs = Self::query_with_conn(db, configuration, name, key).await;
        configs.into_iter().next()
    }

    // _with_conn version for get_remote
    pub async fn get_remote_with_conn(db: &DbConnection, branch: &str) -> Option<String> {
        Config::get_with_conn(db, "branch", Some(branch), "remote")
            .await
            .map(|m| m.value)
    }

    // _with_conn version for get_current_remote
    pub async fn get_current_remote_with_conn(db: &DbConnection) -> Result<Option<String>, ()> {
        match Head::current_with_conn(db).await {
            Head::Branch(name) => Ok(Config::get_remote_with_conn(db, &name).await),
            Head::Detached(_) => {
                eprintln!("fatal: HEAD is detached, cannot get remote");
                Err(())
            }
        }
    }

    // _with_conn version for get_remote_url
    pub async fn get_remote_url_with_conn(db: &DbConnection, remote: &str) -> String {
        match Config::get_with_conn(db, "remote", Some(remote), "url").await {
            Some(model) => model.value,
            None => panic!("fatal: No URL configured for remote '{remote}'."),
        }
    }

    // _with_conn version for get_current_remote_url
    pub async fn get_current_remote_url_with_conn(db: &DbConnection) -> Option<String> {
        match Config::get_current_remote_with_conn(db).await.unwrap() {
            Some(remote) => Some(Config::get_remote_url_with_conn(db, &remote).await),
            None => None,
        }
    }

    // _with_conn version for get_all
    pub async fn get_all_with_conn(
        db: &DbConnection,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Vec<String> {
        Self::query_with_conn(db, configuration, name, key)
            .await
            .iter()
            .map(|c| c.value.to_owned())
            .collect()
    }

    // _with_conn version for list_all
    pub async fn list_all_with_conn(db: &DbConnection) -> Vec<(String, String)> {
        let sql = "SELECT * FROM config";
        let mut rows = db.query(sql, turso::params![]).await.unwrap();
        let mut result = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let m = Model::from_row(&row).unwrap();
            let key_name = match &m.name {
                Some(n) => m.configuration.to_owned() + "." + n + "." + &m.key,
                None => m.configuration.to_owned() + "." + &m.key,
            };
            result.push((key_name, m.value));
        }
        result
    }

    // _with_conn version for remove_config
    pub async fn remove_config_with_conn(
        db: &DbConnection,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        valuepattern: Option<&str>,
        delete_all: bool,
    ) {
        let entries: Vec<Model> = Self::query_with_conn(db, configuration, name, key).await;
        for e in entries {
            let should_delete = match valuepattern {
                Some(vp) => e.value.contains(vp),
                None => true,
            };
            if should_delete {
                let sql = "DELETE FROM config WHERE id = ?1";
                db.execute(sql, turso::params![e.id]).await.unwrap();
                if !delete_all {
                    break;
                }
            }
        }
    }

    // _with_conn version for remove_remote
    pub async fn remove_remote_with_conn(db: &DbConnection, name: &str) -> Result<(), String> {
        let sql = "SELECT * FROM config WHERE configuration = ?1 AND name = ?2";
        let mut rows = db.query(sql, turso::params!["remote", name]).await.unwrap();
        let mut remote_entries = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            remote_entries.push(Model::from_row(&row).unwrap());
        }

        if remote_entries.is_empty() {
            return Err(format!("fatal: No such remote: {name}"));
        }

        for r in remote_entries {
            let sql = "DELETE FROM config WHERE id = ?1";
            db.execute(sql, turso::params![r.id]).await.unwrap();
        }
        Ok(())
    }

    pub async fn rename_remote_with_conn(
        db: &DbConnection,
        old: &str,
        new: &str,
    ) -> Result<(), String> {
        // Ensure the requested rename has a valid source and no conflicts.
        if Self::remote_config_with_conn(db, old).await.is_none() {
            return Err(format!("fatal: No such remote: {old}"));
        }
        if Self::remote_config_with_conn(db, new).await.is_some() {
            return Err(format!("fatal: remote {new} already exists."));
        }

        // Update remote.<name>.* entries to point at the new name.
        let sql = "UPDATE config SET name = ?1 WHERE configuration = ?2 AND name = ?3";
        db.execute(sql, turso::params![new, "remote", old])
            .await
            .unwrap();

        // Repoint branch.*.remote values that referenced the old remote.
        let sql =
            "UPDATE config SET value = ?1 WHERE configuration = ?2 AND key = ?3 AND value = ?4";
        db.execute(sql, turso::params![new, "branch", "remote", old])
            .await
            .unwrap();

        Ok(())
    }

    // _with_conn version for all_remote_configs
    pub async fn all_remote_configs_with_conn(db: &DbConnection) -> Vec<RemoteConfig> {
        let sql = "SELECT * FROM config WHERE configuration = ?1";
        let mut rows = db.query(sql, turso::params!["remote"]).await.unwrap();
        let mut remotes = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            remotes.push(Model::from_row(&row).unwrap());
        }

        let remote_names: HashSet<String> = remotes
            .iter()
            .filter_map(|remote| remote.name.as_ref().cloned())
            .collect();

        remote_names
            .iter()
            .filter_map(|name| {
                remotes
                    .iter()
                    .find(|remote| remote.name.as_ref().unwrap() == name && remote.key == "url")
                    .map(|remote| RemoteConfig {
                        name: name.to_owned(),
                        url: remote.value.to_owned(),
                    })
            })
            .collect()
    }

    // _with_conn version for remote_config
    pub async fn remote_config_with_conn(db: &DbConnection, name: &str) -> Option<RemoteConfig> {
        let sql = "SELECT * FROM config WHERE configuration = ?1 AND name = ?2 AND key = ?3";
        let mut rows = db
            .query(sql, turso::params!["remote", name, "url"])
            .await
            .unwrap();

        if let Some(row) = rows.next().await.unwrap() {
            let r = Model::from_row(&row).unwrap();
            Some(RemoteConfig {
                name: r.name.unwrap(),
                url: r.value,
            })
        } else {
            None
        }
    }

    // _with_conn version for branch_config
    pub async fn branch_config_with_conn(db: &DbConnection, name: &str) -> Option<BranchConfig> {
        let sql = "SELECT * FROM config WHERE configuration = ?1 AND name = ?2";
        let mut rows = db.query(sql, turso::params!["branch", name]).await.unwrap();
        let mut config_entries = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            config_entries.push(Model::from_row(&row).unwrap());
        }

        if config_entries.is_empty() {
            None
        } else {
            assert_eq!(config_entries.len(), 2);
            // if branch_config[0].key == "merge" {
            //     Some(BranchConfig {
            //         name: name.to_owned(),
            //         merge: branch_config[0].value.clone(),
            //         remote: branch_config[1].value.clone(),
            //     })
            // } else {
            //     Some(BranchConfig {
            //         name: name.to_owned(),
            //         merge: branch_config[1].value.clone(),
            //         remote: branch_config[0].value.clone(),
            //     })
            // }
            let mut branch_config = BranchConfig {
                name: name.to_owned(),
                merge: config_entries[0].value.clone(),
                remote: config_entries[1].value.clone(),
            };
            if config_entries[0].key == "remote" {
                swap(&mut branch_config.merge, &mut branch_config.remote);
            }
            branch_config.merge = branch_config.merge[11..].into(); // cut refs/heads/

            Some(branch_config)
        }
    }

    pub async fn insert(configuration: &str, name: Option<&str>, key: &str, value: &str) {
        let db = get_db_conn_instance().await;
        Self::insert_with_conn(db.as_ref(), configuration, name, key, value).await;
    }

    // Update one configuration entry in database using given configuration, name, key and value
    pub async fn update(configuration: &str, name: Option<&str>, key: &str, value: &str) -> Model {
        let db = get_db_conn_instance().await;
        Self::update_with_conn(db.as_ref(), configuration, name, key, value).await
    }

    /// Get one configuration value
    pub async fn get(configuration: &str, name: Option<&str>, key: &str) -> Option<Model> {
        let db = get_db_conn_instance().await;
        Self::get_with_conn(db.as_ref(), configuration, name, key).await
    }

    /// Get remote repo name by branch name
    /// - You may need to `[branch::set-upstream]` if return `None`
    pub async fn get_remote(branch: &str) -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_remote_with_conn(db.as_ref(), branch).await
    }

    /// Get remote repo name of current branch
    /// - `Error` if `HEAD` is detached
    pub async fn get_current_remote() -> Result<Option<String>, ()> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_with_conn(db.as_ref()).await
    }

    pub async fn get_remote_url(remote: &str) -> String {
        let db = get_db_conn_instance().await;
        Self::get_remote_url_with_conn(db.as_ref(), remote).await
    }

    /// return `None` if no remote is set
    pub async fn get_current_remote_url() -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_url_with_conn(db.as_ref()).await
    }

    /// Get all configuration values
    /// - e.g. remote.origin.url can be multiple
    pub async fn get_all(configuration: &str, name: Option<&str>, key: &str) -> Vec<String> {
        let db = get_db_conn_instance().await;
        Self::get_all_with_conn(db.as_ref(), configuration, name, key).await
    }

    /// Get literally all the entries in database without any filtering
    pub async fn list_all() -> Vec<(String, String)> {
        let db = get_db_conn_instance().await;
        Self::list_all_with_conn(db.as_ref()).await
    }

    /// Delete one or all configuration using given key and value pattern
    pub async fn remove_config(
        configuration: &str,
        name: Option<&str>,
        key: &str,
        valuepattern: Option<&str>,
        delete_all: bool,
    ) {
        let db = get_db_conn_instance().await;
        Self::remove_config_with_conn(
            db.as_ref(),
            configuration,
            name,
            key,
            valuepattern,
            delete_all,
        )
        .await;
    }

    /// Delete all the configuration entries using given configuration field (--remove-section)
    // pub async fn remove_by_section(configuration: &str) {
    //     unimplemented!();
    // }
    pub async fn remove_remote(name: &str) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::remove_remote_with_conn(db.as_ref(), name).await
    }

    pub async fn rename_remote(old: &str, new: &str) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::rename_remote_with_conn(db.as_ref(), old, new).await
    }

    pub async fn all_remote_configs() -> Vec<RemoteConfig> {
        let db = get_db_conn_instance().await;
        Self::all_remote_configs_with_conn(db.as_ref()).await
    }

    pub async fn remote_config(name: &str) -> Option<RemoteConfig> {
        let db = get_db_conn_instance().await;
        Self::remote_config_with_conn(db.as_ref(), name).await
    }

    pub async fn branch_config(name: &str) -> Option<BranchConfig> {
        let db = get_db_conn_instance().await;
        Self::branch_config_with_conn(db.as_ref(), name).await
    }
}
