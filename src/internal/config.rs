//! Config storage helpers backed by SeaORM to insert, update, and retrieve values, manage remote/branch settings, and merge scoped configs.

use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, ModelTrait, QueryFilter,
    entity::ActiveModelTrait,
};

use crate::internal::{
    db::get_db_conn_instance,
    head::Head,
    model::config::{self, ActiveModel, Model},
};

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
 *   accept an existing connection or transaction handle (`&C where C: ConnectionTrait`).
 *
 * **WARNING**: To use these functions within a database transaction (e.g., inside
 * a `db.transaction(|txn| { ... })` block), you MUST call the `*_with_conn`
 * variant, passing the transaction handle `txn`. Calling a public version from
 * inside a transaction will try to acquire a second connection from the pool,
 * leading to a deadlock.
 *
 * Correct Usage (in a transaction): `Config::update_with_conn(txn, ...).await;`
 * Incorrect Usage (in a transaction): `Config::update(...).await;` // DEADLOCK!
 */
impl Config {
    // _with_conn version for insert
    pub async fn insert_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        value: &str,
    ) -> Result<(), DbErr> {
        let config = ActiveModel {
            configuration: Set(configuration.to_owned()),
            name: Set(name.map(|s| s.to_owned())),
            key: Set(key.to_owned()),
            value: Set(value.to_owned()),
            ..Default::default()
        };
        config.save(db).await?;
        Ok(())
    }

    // _with_conn version for update
    pub async fn update_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        value: &str,
    ) -> Result<Model, DbErr> {
        let entry = config::Entity::find()
            .filter(config::Column::Configuration.eq(configuration))
            .filter(match name {
                Some(str) => config::Column::Name.eq(str),
                None => config::Column::Name.is_null(),
            })
            .filter(config::Column::Key.eq(key))
            .one(db)
            .await?
            .ok_or_else(|| {
                DbErr::RecordNotFound(format!(
                    "config entry not found: {}.{}.{}",
                    configuration,
                    name.unwrap_or("<none>"),
                    key
                ))
            })?;
        let mut config: ActiveModel = entry.into();
        config.value = Set(value.to_owned());
        config.update(db).await
    }

    // _with_conn version for query
    async fn query_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Result<Vec<Model>, DbErr> {
        config::Entity::find()
            .filter(config::Column::Configuration.eq(configuration))
            .filter(match name {
                Some(str) => config::Column::Name.eq(str),
                None => config::Column::Name.is_null(),
            })
            .filter(config::Column::Key.eq(key))
            .all(db)
            .await
    }

    // _with_conn version for get
    pub async fn get_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Result<Option<String>, DbErr> {
        let values = Self::query_with_conn(db, configuration, name, key).await?;
        Ok(values.first().map(|c| c.value.to_owned()))
    }

    // _with_conn version for get_remote
    pub async fn get_remote_with_conn<C: ConnectionTrait>(
        db: &C,
        branch: &str,
    ) -> Result<Option<String>, DbErr> {
        Config::get_with_conn(db, "branch", Some(branch), "remote").await
    }

    // _with_conn version for get_current_remote
    pub async fn get_current_remote_with_conn<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Option<String>, ()> {
        match Head::current_with_conn(db).await {
            Head::Branch(name) => Config::get_remote_with_conn(db, &name)
                .await
                .map_err(|_| ()),
            Head::Detached(_) => {
                eprintln!("fatal: HEAD is detached, cannot get remote");
                Err(())
            }
        }
    }

    // _with_conn version for get_remote_url
    pub async fn get_remote_url_with_conn<C: ConnectionTrait>(
        db: &C,
        remote: &str,
    ) -> Result<String, DbErr> {
        Config::get_with_conn(db, "remote", Some(remote), "url")
            .await?
            .ok_or_else(|| {
                DbErr::RecordNotFound(format!("no URL configured for remote '{remote}'"))
            })
    }

    // _with_conn version for get_current_remote_url
    pub async fn get_current_remote_url_with_conn<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Option<String>, DbErr> {
        match Config::get_current_remote_with_conn(db).await {
            Ok(Some(remote)) => Ok(Some(Config::get_remote_url_with_conn(db, &remote).await?)),
            Ok(None) => Ok(None),
            Err(()) => Ok(None),
        }
    }

    // _with_conn version for get_all
    pub async fn get_all_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Result<Vec<String>, DbErr> {
        Ok(Self::query_with_conn(db, configuration, name, key)
            .await?
            .iter()
            .map(|c| c.value.to_owned())
            .collect())
    }

    // _with_conn version for list_all
    pub async fn list_all_with_conn<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Vec<(String, String)>, DbErr> {
        Ok(config::Entity::find()
            .all(db)
            .await?
            .iter()
            .map(|m| {
                (
                    match &m.name {
                        Some(n) => m.configuration.to_owned() + "." + n + "." + &m.key,
                        None => m.configuration.to_owned() + "." + &m.key,
                    },
                    m.value.to_owned(),
                )
            })
            .collect())
    }

    // _with_conn version for remove_config
    pub async fn remove_config_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        valuepattern: Option<&str>,
        delete_all: bool,
    ) -> Result<(), DbErr> {
        let entries: Vec<Model> = Self::query_with_conn(db, configuration, name, key).await?;
        for e in entries {
            match valuepattern {
                Some(vp) => {
                    if e.value.contains(vp) {
                        e.delete(db).await?;
                    } else {
                        continue;
                    }
                }
                None => {
                    e.delete(db).await?;
                }
            };
            if !delete_all {
                break;
            }
        }
        Ok(())
    }

    // _with_conn version for remove_remote
    pub async fn remove_remote_with_conn<C: ConnectionTrait>(
        db: &C,
        name: &str,
    ) -> Result<(), String> {
        let remote = config::Entity::find()
            .filter(config::Column::Configuration.eq("remote"))
            .filter(config::Column::Name.eq(name))
            .all(db)
            .await
            .map_err(|e| format!("failed to query remote '{name}': {e}"))?;
        if remote.is_empty() {
            return Err(format!("fatal: No such remote: {name}"));
        }
        for r in remote {
            let r: ActiveModel = r.into();
            r.delete(db)
                .await
                .map_err(|e| format!("failed to delete remote '{name}': {e}"))?;
        }
        Ok(())
    }

    pub async fn rename_remote_with_conn<C: ConnectionTrait>(
        db: &C,
        old: &str,
        new: &str,
    ) -> Result<(), String> {
        // Ensure the requested rename has a valid source and no conflicts.
        if Self::remote_config_with_conn(db, old)
            .await
            .map_err(|e| format!("failed to look up remote '{old}': {e}"))?
            .is_none()
        {
            return Err(format!("fatal: No such remote: {old}"));
        }
        if Self::remote_config_with_conn(db, new)
            .await
            .map_err(|e| format!("failed to look up remote '{new}': {e}"))?
            .is_some()
        {
            return Err(format!("fatal: remote {new} already exists."));
        }

        let remote_entries = config::Entity::find()
            .filter(config::Column::Configuration.eq("remote"))
            .filter(config::Column::Name.eq(old))
            .all(db)
            .await
            .map_err(|e| format!("failed to query remote entries for '{old}': {e}"))?;

        // Update remote.<name>.* entries to point at the new name.
        for entry in remote_entries {
            let mut active: ActiveModel = entry.into();
            active.name = Set(Some(new.to_owned()));
            active.update(db).await.map_err(|e| {
                format!("failed to rename remote entry from '{old}' to '{new}': {e}")
            })?;
        }

        let branch_entries = config::Entity::find()
            .filter(config::Column::Configuration.eq("branch"))
            .filter(config::Column::Key.eq("remote"))
            .filter(config::Column::Value.eq(old))
            .all(db)
            .await
            .map_err(|e| format!("failed to query branch entries referencing '{old}': {e}"))?;

        // Repoint branch.*.remote values that referenced the old remote.
        for entry in branch_entries {
            let mut active: ActiveModel = entry.into();
            active.value = Set(new.to_owned());
            active.update(db).await.map_err(|e| {
                format!("failed to update branch remote from '{old}' to '{new}': {e}")
            })?;
        }

        Ok(())
    }

    // _with_conn version for all_remote_configs
    pub async fn all_remote_configs_with_conn<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Vec<RemoteConfig>, DbErr> {
        let remotes = config::Entity::find()
            .filter(config::Column::Configuration.eq("remote"))
            .filter(config::Column::Key.eq("url"))
            .all(db)
            .await?;

        Ok(remotes
            .into_iter()
            .filter_map(|entry| {
                let name = entry.name?;
                Some(RemoteConfig {
                    name,
                    url: entry.value,
                })
            })
            .collect())
    }

    // _with_conn version for remote_config
    pub async fn remote_config_with_conn<C: ConnectionTrait>(
        db: &C,
        name: &str,
    ) -> Result<Option<RemoteConfig>, DbErr> {
        let remote = config::Entity::find()
            .filter(config::Column::Configuration.eq("remote"))
            .filter(config::Column::Name.eq(name))
            .filter(config::Column::Key.eq("url"))
            .one(db)
            .await?;
        Ok(remote.map(|r| RemoteConfig {
            name: r.name.unwrap_or_default(),
            url: r.value,
        }))
    }

    // _with_conn version for branch_config
    pub async fn branch_config_with_conn<C: ConnectionTrait>(
        db: &C,
        name: &str,
    ) -> Result<Option<BranchConfig>, DbErr> {
        let config_entries = config::Entity::find()
            .filter(config::Column::Configuration.eq("branch"))
            .filter(config::Column::Name.eq(name))
            .all(db)
            .await?;
        if config_entries.is_empty() {
            Ok(None)
        } else {
            let remote = config_entries
                .iter()
                .find(|entry| entry.key == "remote")
                .map(|entry| entry.value.to_owned());
            let merge = config_entries
                .iter()
                .find(|entry| entry.key == "merge")
                .map(|entry| entry.value.to_owned());
            match (remote, merge) {
                (Some(remote), Some(merge)) => {
                    let merge = merge
                        .strip_prefix("refs/heads/")
                        .unwrap_or(&merge)
                        .to_owned();
                    Ok(Some(BranchConfig {
                        name: name.to_owned(),
                        merge,
                        remote,
                    }))
                }
                _ => Ok(None),
            }
        }
    }

    pub async fn insert(configuration: &str, name: Option<&str>, key: &str, value: &str) {
        let db = get_db_conn_instance().await;
        Self::insert_with_conn(&db, configuration, name, key, value)
            .await
            .expect("failed to insert config entry");
    }

    // Update one configuration entry in database using given configuration, name, key and value
    pub async fn update(configuration: &str, name: Option<&str>, key: &str, value: &str) -> Model {
        let db = get_db_conn_instance().await;
        Self::update_with_conn(&db, configuration, name, key, value)
            .await
            .expect("failed to update config entry")
    }

    /// Get one configuration value
    pub async fn get(configuration: &str, name: Option<&str>, key: &str) -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_with_conn(&db, configuration, name, key)
            .await
            .expect("failed to query config")
    }

    /// Get remote repo name by branch name
    /// - You may need to `[branch::set-upstream]` if return `None`
    pub async fn get_remote(branch: &str) -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_remote_with_conn(&db, branch)
            .await
            .expect("failed to query remote config")
    }

    /// Get remote repo name of current branch
    /// - `Error` if `HEAD` is detached
    pub async fn get_current_remote() -> Result<Option<String>, ()> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_with_conn(&db).await
    }

    pub async fn get_remote_url(remote: &str) -> String {
        let db = get_db_conn_instance().await;
        Self::get_remote_url_with_conn(&db, remote)
            .await
            .expect("failed to get remote URL")
    }

    /// return `None` if no remote is set
    pub async fn get_current_remote_url() -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_url_with_conn(&db)
            .await
            .expect("failed to get current remote URL")
    }

    /// Get all configuration values
    /// - e.g. remote.origin.url can be multiple
    pub async fn get_all(configuration: &str, name: Option<&str>, key: &str) -> Vec<String> {
        let db = get_db_conn_instance().await;
        Self::get_all_with_conn(&db, configuration, name, key)
            .await
            .expect("failed to get all config values")
    }

    /// Get literally all the entries in database without any filtering
    pub async fn list_all() -> Vec<(String, String)> {
        let db = get_db_conn_instance().await;
        Self::list_all_with_conn(&db)
            .await
            .expect("failed to list all config entries")
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
        Self::remove_config_with_conn(&db, configuration, name, key, valuepattern, delete_all)
            .await
            .expect("failed to remove config entry");
    }

    /// Delete all the configuration entries using given configuration field (--remove-section)
    // pub async fn remove_by_section(configuration: &str) {
    //     unimplemented!();
    // }
    pub async fn remove_remote(name: &str) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::remove_remote_with_conn(&db, name).await
    }

    pub async fn rename_remote(old: &str, new: &str) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::rename_remote_with_conn(&db, old, new).await
    }

    pub async fn all_remote_configs() -> Vec<RemoteConfig> {
        let db = get_db_conn_instance().await;
        Self::all_remote_configs_with_conn(&db)
            .await
            .expect("failed to query all remote configs")
    }

    pub async fn remote_config(name: &str) -> Option<RemoteConfig> {
        let db = get_db_conn_instance().await;
        Self::remote_config_with_conn(&db, name)
            .await
            .expect("failed to query remote config")
    }

    pub async fn branch_config(name: &str) -> Option<BranchConfig> {
        let db = get_db_conn_instance().await;
        Self::branch_config_with_conn(&db, name)
            .await
            .expect("failed to query branch config")
    }
}
