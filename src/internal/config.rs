//! Config storage helpers backed by SeaORM to insert, update, and retrieve values, manage remote/branch settings, and merge scoped configs.
//!
//! ## New `ConfigKv` API
//!
//! The `ConfigKv` struct provides flat key/value access to the `config_kv` table,
//! with optional vault encryption. All new code should use `ConfigKv` instead of
//! the deprecated `Config` struct.

use std::{collections::HashSet, mem::swap, path::Path};

use anyhow::{Context, Result, anyhow};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, ModelTrait,
    QueryFilter, QueryOrder, entity::ActiveModelTrait,
};

use crate::internal::{
    db::{get_db_conn_instance, get_db_conn_instance_for_path},
    head::Head,
    model::{
        config::{self, ActiveModel, Model},
        config_kv,
    },
};

// ─────────────────────────────────────────────────────────────────────────────
// ConfigKv — new flat key/value API backed by the `config_kv` table
// ─────────────────────────────────────────────────────────────────────────────

/// A single entry from the `config_kv` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigKvEntry {
    pub key: String,
    pub value: String,
    pub encrypted: bool,
}

impl ConfigKvEntry {
    fn from_model(m: &config_kv::Model) -> Self {
        Self {
            key: m.key.clone(),
            value: m.value.clone(),
            encrypted: m.encrypted != 0,
        }
    }
}

/// Flat key/value configuration access backed by the `config_kv` table.
///
/// All methods follow the `_with_conn` pattern for transaction safety.
/// See the module-level documentation on [`Config`] for details.
pub struct ConfigKv;

impl ConfigKv {
    // ── Core CRUD (_with_conn) ───────────────────────────────────────────

    /// Get the last value for a key (last-one-wins for multi-value keys).
    pub async fn get_with_conn<C: ConnectionTrait>(
        db: &C,
        key: &str,
    ) -> Result<Option<ConfigKvEntry>> {
        let row = config_kv::Entity::find()
            .filter(config_kv::Column::Key.eq(key))
            .order_by_desc(config_kv::Column::Id)
            .one(db)
            .await
            .context("failed to query config_kv")?;
        Ok(row.as_ref().map(ConfigKvEntry::from_model))
    }

    /// Get all values for a key (preserves insertion order).
    pub async fn get_all_with_conn<C: ConnectionTrait>(
        db: &C,
        key: &str,
    ) -> Result<Vec<ConfigKvEntry>> {
        let rows = config_kv::Entity::find()
            .filter(config_kv::Column::Key.eq(key))
            .order_by_asc(config_kv::Column::Id)
            .all(db)
            .await
            .context("failed to query config_kv")?;
        Ok(rows.iter().map(ConfigKvEntry::from_model).collect())
    }

    /// Count values for a key.
    pub async fn count_values_with_conn<C: ConnectionTrait>(db: &C, key: &str) -> Result<usize> {
        let rows = config_kv::Entity::find()
            .filter(config_kv::Column::Key.eq(key))
            .all(db)
            .await
            .context("failed to count config_kv entries")?;
        Ok(rows.len())
    }

    /// Set a config value (upsert). Errors with exit-code 5 if the key has
    /// multiple values — caller must use `unset_all` first or use `add`.
    pub async fn set_with_conn<C: ConnectionTrait>(
        db: &C,
        key: &str,
        value: &str,
        encrypted: bool,
    ) -> Result<()> {
        let existing = config_kv::Entity::find()
            .filter(config_kv::Column::Key.eq(key))
            .all(db)
            .await
            .context("failed to query config_kv for set")?;

        if existing.len() > 1 {
            return Err(anyhow!(
                "cannot set '{}': {} values exist for this key",
                key,
                existing.len()
            ));
        }

        if let Some(row) = existing.into_iter().next() {
            // Inherit encryption from existing entry if not explicitly set
            let effective_encrypted = encrypted || row.encrypted != 0;
            // Update existing row
            let mut active: config_kv::ActiveModel = row.into();
            active.value = Set(value.to_owned());
            active.encrypted = Set(if effective_encrypted { 1 } else { 0 });
            active
                .update(db)
                .await
                .context("failed to update config_kv")?;
        } else {
            // Insert new row
            let entry = config_kv::ActiveModel {
                key: Set(key.to_owned()),
                value: Set(value.to_owned()),
                encrypted: Set(if encrypted { 1 } else { 0 }),
                ..Default::default()
            };
            entry.save(db).await.context("failed to insert config_kv")?;
        }
        Ok(())
    }

    /// Add a value for a key (allows duplicates, for multi-value keys).
    ///
    /// Enforces same-key-same-state: if existing entries for this key have a
    /// different encryption state, the insert is rejected. If existing entries
    /// are encrypted and `encrypted` is false, the encryption state is
    /// inherited (auto-promoted to encrypted).
    pub async fn add_with_conn<C: ConnectionTrait>(
        db: &C,
        key: &str,
        value: &str,
        encrypted: bool,
    ) -> Result<()> {
        // Check existing entries for encryption state inheritance / conflict
        let existing = config_kv::Entity::find()
            .filter(config_kv::Column::Key.eq(key))
            .all(db)
            .await
            .context("failed to query config_kv for add")?;

        let has_encrypted = existing.iter().any(|e| e.encrypted != 0);
        let has_plaintext = existing.iter().any(|e| e.encrypted == 0);

        // Inherit encryption from existing entries
        let effective_encrypted = encrypted || has_encrypted;

        // Reject mixed encryption states
        if !existing.is_empty()
            && ((effective_encrypted && has_plaintext) || (!effective_encrypted && has_encrypted))
        {
            return Err(anyhow!(
                "cannot mix encrypted and plaintext values for the same key"
            ));
        }

        let entry = config_kv::ActiveModel {
            key: Set(key.to_owned()),
            value: Set(value.to_owned()),
            encrypted: Set(if effective_encrypted { 1 } else { 0 }),
            ..Default::default()
        };
        entry
            .save(db)
            .await
            .context("failed to add config_kv entry")?;
        Ok(())
    }

    /// Delete the first matching entry for a key.
    /// Returns the number of rows deleted (0 or 1).
    pub async fn unset_with_conn<C: ConnectionTrait>(db: &C, key: &str) -> Result<usize> {
        let rows = config_kv::Entity::find()
            .filter(config_kv::Column::Key.eq(key))
            .all(db)
            .await
            .context("failed to query config_kv for unset")?;

        if rows.len() > 1 {
            return Err(anyhow!(
                "cannot unset '{}': {} values exist for this key",
                key,
                rows.len()
            ));
        }

        if let Some(row) = rows.into_iter().next() {
            row.delete(db)
                .await
                .context("failed to delete config_kv entry")?;
            Ok(1)
        } else {
            Ok(0)
        }
    }

    /// Delete all matching entries for a key.
    /// Returns the number of rows deleted.
    pub async fn unset_all_with_conn<C: ConnectionTrait>(db: &C, key: &str) -> Result<usize> {
        let rows = config_kv::Entity::find()
            .filter(config_kv::Column::Key.eq(key))
            .all(db)
            .await
            .context("failed to query config_kv for unset_all")?;

        let count = rows.len();
        for row in rows {
            row.delete(db)
                .await
                .context("failed to delete config_kv entry")?;
        }
        Ok(count)
    }

    /// List all config entries.
    pub async fn list_all_with_conn<C: ConnectionTrait>(db: &C) -> Result<Vec<ConfigKvEntry>> {
        let rows = config_kv::Entity::find()
            .order_by_asc(config_kv::Column::Key)
            .all(db)
            .await
            .context("failed to list config_kv")?;
        Ok(rows.iter().map(ConfigKvEntry::from_model).collect())
    }

    /// Get all entries whose key starts with the given prefix.
    pub async fn get_by_prefix_with_conn<C: ConnectionTrait>(
        db: &C,
        prefix: &str,
    ) -> Result<Vec<ConfigKvEntry>> {
        let rows = config_kv::Entity::find()
            .filter(config_kv::Column::Key.starts_with(prefix))
            .order_by_asc(config_kv::Column::Key)
            .all(db)
            .await
            .context("failed to query config_kv by prefix")?;
        Ok(rows.iter().map(ConfigKvEntry::from_model).collect())
    }

    /// Get all entries whose key matches a regex pattern.
    pub async fn get_regexp_with_conn<C: ConnectionTrait>(
        db: &C,
        pattern: &str,
    ) -> Result<Vec<ConfigKvEntry>> {
        // SQLite doesn't have native regex, so we fetch all and filter in Rust.
        let re = regex::Regex::new(pattern)
            .map_err(|e| anyhow!("invalid regex pattern '{}': {}", pattern, e))?;
        let rows = config_kv::Entity::find()
            .order_by_asc(config_kv::Column::Key)
            .all(db)
            .await
            .context("failed to query config_kv for regexp")?;
        Ok(rows
            .iter()
            .filter(|r| re.is_match(&r.key))
            .map(ConfigKvEntry::from_model)
            .collect())
    }

    // ── Convenience wrappers (acquire DB conn from pool) ─────────────────

    pub async fn get(key: &str) -> Result<Option<ConfigKvEntry>> {
        let db = get_db_conn_instance().await;
        Self::get_with_conn(&db, key).await
    }

    pub async fn get_all(key: &str) -> Result<Vec<ConfigKvEntry>> {
        let db = get_db_conn_instance().await;
        Self::get_all_with_conn(&db, key).await
    }

    pub async fn set(key: &str, value: &str, encrypted: bool) -> Result<()> {
        let db = get_db_conn_instance().await;
        Self::set_with_conn(&db, key, value, encrypted).await
    }

    pub async fn add(key: &str, value: &str, encrypted: bool) -> Result<()> {
        let db = get_db_conn_instance().await;
        Self::add_with_conn(&db, key, value, encrypted).await
    }

    pub async fn unset(key: &str) -> Result<usize> {
        let db = get_db_conn_instance().await;
        Self::unset_with_conn(&db, key).await
    }

    pub async fn unset_all(key: &str) -> Result<usize> {
        let db = get_db_conn_instance().await;
        Self::unset_all_with_conn(&db, key).await
    }

    pub async fn list_all() -> Result<Vec<ConfigKvEntry>> {
        let db = get_db_conn_instance().await;
        Self::list_all_with_conn(&db).await
    }

    pub async fn get_by_prefix(prefix: &str) -> Result<Vec<ConfigKvEntry>> {
        let db = get_db_conn_instance().await;
        Self::get_by_prefix_with_conn(&db, prefix).await
    }

    // ── Type helpers ─────────────────────────────────────────────────────

    /// Get a boolean config value. Normalises `true/yes/on/1` → `true`,
    /// `false/no/off/0` → `false`.
    pub async fn get_bool_with_conn<C: ConnectionTrait>(db: &C, key: &str) -> Result<Option<bool>> {
        let entry = Self::get_with_conn(db, key).await?;
        match entry {
            None => Ok(None),
            Some(e) => {
                let v = e.value.to_ascii_lowercase();
                match v.as_str() {
                    "true" | "yes" | "on" | "1" => Ok(Some(true)),
                    "false" | "no" | "off" | "0" => Ok(Some(false)),
                    _ => Err(anyhow!(
                        "invalid value '{}' for key '{}': expected bool (true/false)",
                        if e.encrypted { "<REDACTED>" } else { &e.value },
                        key
                    )),
                }
            }
        }
    }

    /// Get an integer config value. Supports `k`/`m`/`g` suffixes.
    pub async fn get_int_with_conn<C: ConnectionTrait>(db: &C, key: &str) -> Result<Option<i64>> {
        let entry = Self::get_with_conn(db, key).await?;
        match entry {
            None => Ok(None),
            Some(e) => {
                let s = e.value.trim().to_ascii_lowercase();
                let (num_str, multiplier) = if s.ends_with('k') {
                    (&s[..s.len() - 1], 1024i64)
                } else if s.ends_with('m') {
                    (&s[..s.len() - 1], 1024 * 1024)
                } else if s.ends_with('g') {
                    (&s[..s.len() - 1], 1024 * 1024 * 1024)
                } else {
                    (s.as_str(), 1i64)
                };
                let n: i64 = num_str.parse().map_err(|_| {
                    anyhow!(
                        "invalid value '{}' for key '{}': expected integer",
                        if e.encrypted { "<REDACTED>" } else { &e.value },
                        key
                    )
                })?;
                Ok(Some(n * multiplier))
            }
        }
    }

    // ── Domain helpers (replace old Config methods) ──────────────────────

    /// Get the value of `remote.<remote>.url`.
    pub async fn get_remote_url_with_conn<C: ConnectionTrait>(
        db: &C,
        remote: &str,
    ) -> Result<String> {
        let key = format!("remote.{remote}.url");
        match Self::get_with_conn(db, &key).await? {
            Some(entry) => Ok(entry.value),
            None => Err(anyhow!("fatal: No URL configured for remote '{remote}'.")),
        }
    }

    pub async fn get_remote_url(remote: &str) -> Result<String> {
        let db = get_db_conn_instance().await;
        Self::get_remote_url_with_conn(&db, remote).await
    }

    /// Get remote name for a branch from `branch.<branch>.remote`.
    pub async fn get_remote_with_conn<C: ConnectionTrait>(
        db: &C,
        branch: &str,
    ) -> Result<Option<String>> {
        let key = format!("branch.{branch}.remote");
        Ok(Self::get_with_conn(db, &key).await?.map(|e| e.value))
    }

    pub async fn get_remote(branch: &str) -> Result<Option<String>> {
        let db = get_db_conn_instance().await;
        Self::get_remote_with_conn(&db, branch).await
    }

    /// Get remote for the current HEAD branch.
    pub async fn get_current_remote_with_conn<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Option<String>> {
        match Head::current_with_conn(db).await {
            Head::Branch(name) => Self::get_remote_with_conn(db, &name).await,
            Head::Detached(_) => Err(anyhow!("fatal: HEAD is detached, cannot get remote")),
        }
    }

    pub async fn get_current_remote() -> Result<Option<String>> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_with_conn(&db).await
    }

    /// Get remote URL for the current HEAD branch.
    pub async fn get_current_remote_url_with_conn<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Option<String>> {
        match Self::get_current_remote_with_conn(db).await? {
            Some(remote) => Ok(Some(Self::get_remote_url_with_conn(db, &remote).await?)),
            None => Ok(None),
        }
    }

    pub async fn get_current_remote_url() -> Result<Option<String>> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_url_with_conn(&db).await
    }

    /// Get all remote configs.
    pub async fn all_remote_configs_with_conn<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Vec<RemoteConfig>> {
        let entries = Self::get_by_prefix_with_conn(db, "remote.").await?;
        let mut remote_names: Vec<String> = Vec::new();
        for e in &entries {
            // Parse "remote.<name>.url" to extract <name>
            if let Some(rest) = e.key.strip_prefix("remote.")
                && let Some((name, suffix)) = rest.rsplit_once('.')
                && suffix == "url"
                && !remote_names.contains(&name.to_string())
            {
                remote_names.push(name.to_string());
            }
        }
        let mut configs = Vec::new();
        for name in remote_names {
            let url_key = format!("remote.{name}.url");
            if let Some(entry) = entries.iter().find(|e| e.key == url_key) {
                configs.push(RemoteConfig {
                    name: name.clone(),
                    url: entry.value.clone(),
                });
            }
        }
        Ok(configs)
    }

    pub async fn all_remote_configs() -> Result<Vec<RemoteConfig>> {
        let db = get_db_conn_instance().await;
        Self::all_remote_configs_with_conn(&db).await
    }

    /// Get a specific remote's config.
    pub async fn remote_config_with_conn<C: ConnectionTrait>(
        db: &C,
        name: &str,
    ) -> Result<Option<RemoteConfig>> {
        let url_key = format!("remote.{name}.url");
        match Self::get_with_conn(db, &url_key).await? {
            Some(entry) => Ok(Some(RemoteConfig {
                name: name.to_owned(),
                url: entry.value,
            })),
            None => Ok(None),
        }
    }

    pub async fn remote_config(name: &str) -> Result<Option<RemoteConfig>> {
        let db = get_db_conn_instance().await;
        Self::remote_config_with_conn(&db, name).await
    }

    /// Get branch tracking configuration.
    pub async fn branch_config_with_conn<C: ConnectionTrait>(
        db: &C,
        name: &str,
    ) -> Result<Option<BranchConfig>> {
        let remote_key = format!("branch.{name}.remote");
        let merge_key = format!("branch.{name}.merge");
        let remote = Self::get_with_conn(db, &remote_key).await?;
        let merge = Self::get_with_conn(db, &merge_key).await?;
        match (remote, merge) {
            (Some(r), Some(m)) => {
                let mut merge_val = m.value;
                // Strip refs/heads/ prefix if present
                if let Some(stripped) = merge_val.strip_prefix("refs/heads/") {
                    merge_val = stripped.to_string();
                }
                Ok(Some(BranchConfig {
                    name: name.to_owned(),
                    merge: merge_val,
                    remote: r.value,
                }))
            }
            _ => Ok(None),
        }
    }

    pub async fn branch_config(name: &str) -> Result<Option<BranchConfig>> {
        let db = get_db_conn_instance().await;
        Self::branch_config_with_conn(&db, name).await
    }

    /// Remove all config entries for a remote.
    pub async fn remove_remote_with_conn<C: ConnectionTrait>(db: &C, name: &str) -> Result<()> {
        let prefix = format!("remote.{name}.");
        let entries = config_kv::Entity::find()
            .filter(config_kv::Column::Key.starts_with(&prefix))
            .all(db)
            .await
            .context("failed to query remote entries for removal")?;

        if entries.is_empty() {
            return Err(anyhow!("fatal: No such remote: {name}"));
        }

        for entry in entries {
            entry
                .delete(db)
                .await
                .context("failed to delete remote entry")?;
        }

        // Also clean up SSH keys for this remote
        let ssh_prefix = format!("vault.ssh.{name}.");
        let ssh_entries = config_kv::Entity::find()
            .filter(config_kv::Column::Key.starts_with(&ssh_prefix))
            .all(db)
            .await
            .context("failed to query SSH key entries for removal")?;
        for entry in ssh_entries {
            entry
                .delete(db)
                .await
                .context("failed to delete SSH key entry")?;
        }

        Ok(())
    }

    pub async fn remove_remote(name: &str) -> Result<()> {
        let db = get_db_conn_instance().await;
        Self::remove_remote_with_conn(&db, name).await
    }

    /// Rename a remote, updating all related config entries.
    pub async fn rename_remote_with_conn<C: ConnectionTrait>(
        db: &C,
        old: &str,
        new: &str,
    ) -> Result<()> {
        // Validate source exists and target doesn't
        if Self::remote_config_with_conn(db, old).await?.is_none() {
            return Err(anyhow!("fatal: No such remote: {old}"));
        }
        if Self::remote_config_with_conn(db, new).await?.is_some() {
            return Err(anyhow!("fatal: remote {new} already exists."));
        }

        // Rename remote.old.* → remote.new.*
        let old_prefix = format!("remote.{old}.");
        let new_prefix = format!("remote.{new}.");
        let entries = config_kv::Entity::find()
            .filter(config_kv::Column::Key.starts_with(&old_prefix))
            .all(db)
            .await
            .context("failed to query remote entries for rename")?;
        for entry in entries {
            let new_key = entry.key.replacen(&old_prefix, &new_prefix, 1);
            let mut active: config_kv::ActiveModel = entry.into();
            active.key = Set(new_key);
            active
                .update(db)
                .await
                .context("failed to rename remote entry")?;
        }

        // Update branch.*.remote values that reference the old name
        let branch_entries = Self::get_by_prefix_with_conn(db, "branch.").await?;
        for be in branch_entries {
            if be.key.ends_with(".remote") && be.value == old {
                let rows = config_kv::Entity::find()
                    .filter(config_kv::Column::Key.eq(&be.key))
                    .filter(config_kv::Column::Value.eq(old))
                    .all(db)
                    .await
                    .context("failed to query branch remote entries")?;
                for row in rows {
                    let mut active: config_kv::ActiveModel = row.into();
                    active.value = Set(new.to_owned());
                    active
                        .update(db)
                        .await
                        .context("failed to update branch remote")?;
                }
            }
        }

        // Cascade SSH key rename: vault.ssh.old.* → vault.ssh.new.*
        let ssh_old_prefix = format!("vault.ssh.{old}.");
        let ssh_new_prefix = format!("vault.ssh.{new}.");
        let ssh_entries = config_kv::Entity::find()
            .filter(config_kv::Column::Key.starts_with(&ssh_old_prefix))
            .all(db)
            .await
            .context("failed to query SSH key entries for rename")?;
        for entry in ssh_entries {
            let new_key = entry.key.replacen(&ssh_old_prefix, &ssh_new_prefix, 1);
            let mut active: config_kv::ActiveModel = entry.into();
            active.key = Set(new_key);
            active
                .update(db)
                .await
                .context("failed to rename SSH key entry")?;
        }

        Ok(())
    }

    pub async fn rename_remote(old: &str, new: &str) -> Result<()> {
        let db = get_db_conn_instance().await;
        Self::rename_remote_with_conn(&db, old, new).await
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Environment variable resolution
// ─────────────────────────────────────────────────────────────────────────────

/// Decrypt a hex-encoded ciphertext using the vault unseal key for the given scope.
/// `scope` should be `"local"` or `"global"`.
pub async fn decrypt_value(hex_ciphertext: &str, scope: &str) -> Result<String> {
    let unseal_key = crate::internal::vault::load_unseal_key_for_scope(scope)
        .await
        .ok_or_else(|| anyhow!("vault not initialized for {scope} scope — cannot decrypt value"))?;
    let ciphertext =
        hex::decode(hex_ciphertext).context("failed to decode encrypted config value hex")?;
    crate::internal::vault::decrypt_token(&unseal_key, &ciphertext)
}

/// Encrypt a value using the vault unseal key for the given scope.
/// Returns the hex-encoded ciphertext.
pub async fn encrypt_value(value: &str, scope: &str) -> Result<String> {
    let unseal_key = crate::internal::vault::load_unseal_key_for_scope(scope)
        .await
        .ok_or_else(|| anyhow!("vault not initialized for {scope} scope — cannot encrypt value"))?;
    let ciphertext = crate::internal::vault::encrypt_token(&unseal_key, value.as_bytes())?;
    Ok(hex::encode(ciphertext))
}

/// Resolve an environment variable by priority chain:
/// 1. System environment variable (`std::env::var`)
/// 2. Local config (`vault.env.<name>` in `.libra/libra.db`)
/// 3. Global config (`vault.env.<name>` in `~/.libra/config.db`)
///
/// `name` is the raw env var name (e.g. `"GEMINI_API_KEY"`).
/// Returns `Err` if a vault/DB query fails (not the same as "not configured").
pub async fn resolve_env(name: &str) -> Result<Option<String>> {
    // 1. System environment variable — per-process override (12-Factor)
    if let Ok(val) = std::env::var(name) {
        return Ok(Some(val));
    }

    // 2. Local config (vault.env.*)
    let vault_key = format!("vault.env.{name}");
    match ConfigKv::get(&vault_key).await {
        Ok(Some(entry)) => {
            if entry.encrypted {
                // Decrypt the stored value using local-scope unseal key
                let plaintext = decrypt_value(&entry.value, "local")
                    .await
                    .context(format!("failed to decrypt vault.env.{name}"))?;
                return Ok(Some(plaintext));
            }
            return Ok(Some(entry.value));
        }
        Ok(None) => {}
        Err(e) => {
            return Err(e.context(format!("failed to read '{name}' from local config")));
        }
    }

    // 3. Global config — lowest priority
    if let Some(global_path) = global_config_path()
        && global_path.exists()
    {
        let conn = crate::internal::db::establish_connection(&global_path.to_string_lossy())
            .await
            .with_context(|| {
                format!(
                    "failed to connect to global config '{}'",
                    global_path.display()
                )
            })?;
        match ConfigKv::get_with_conn(&conn, &vault_key).await {
            Ok(Some(entry)) => {
                if entry.encrypted {
                    let plaintext =
                        decrypt_value(&entry.value, "global")
                            .await
                            .context(format!(
                                "failed to decrypt vault.env.{name} from global config"
                            ))?;
                    return Ok(Some(plaintext));
                }
                return Ok(Some(entry.value));
            }
            Ok(None) => {}
            Err(e) => {
                return Err(e.context(format!("failed to read '{name}' from global config")));
            }
        }
    }

    Ok(None)
}

/// Resolve the global config database path.
///
/// Checks `LIBRA_CONFIG_GLOBAL_DB` env var first, then falls back to
/// `~/.libra/config.db`.
fn global_config_path() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("LIBRA_CONFIG_GLOBAL_DB") {
        return Some(std::path::PathBuf::from(p));
    }
    dirs::home_dir().map(|home| home.join(".libra").join("config.db"))
}

/// Identity sources resolved for commands that need name/email defaults.
///
/// `config_*` contains the cascaded local/global result for each field, while
/// `env_*` preserves the environment fallback separately so callers like
/// `commit` can still enforce `user.useConfigOnly`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserIdentitySources {
    pub config_name: Option<String>,
    pub config_email: Option<String>,
    pub env_name: Option<String>,
    pub env_email: Option<String>,
}

/// Which local repository, if any, should participate in config resolution.
#[derive(Debug, Clone, Copy)]
pub enum LocalIdentityTarget<'a> {
    /// Read local config from the current repository discovered from cwd.
    CurrentRepo,
    /// Read local config from an explicit repository database path.
    ExplicitDb(&'a Path),
    /// Skip local scope entirely and only read global/env values.
    None,
}

/// Return the first non-empty environment variable value from `keys`.
pub fn env_first_non_empty(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

/// Read a config value for the given target using local-first, then global.
pub async fn read_cascaded_config_value(
    local_target: LocalIdentityTarget<'_>,
    key: &str,
) -> Result<Option<String>> {
    if let Some(value) = local_config_value_for_target(local_target, key).await? {
        return Ok(Some(value));
    }
    global_config_value(key).await
}

/// Resolve user identity values from config and environment while preserving
/// the source boundary between the two.
pub async fn resolve_user_identity_sources(
    local_target: LocalIdentityTarget<'_>,
) -> Result<UserIdentitySources> {
    Ok(UserIdentitySources {
        config_name: read_cascaded_config_value(local_target, "user.name").await?,
        config_email: read_cascaded_config_value(local_target, "user.email").await?,
        env_name: env_first_non_empty(&[
            "GIT_COMMITTER_NAME",
            "GIT_AUTHOR_NAME",
            "LIBRA_COMMITTER_NAME",
        ]),
        env_email: env_first_non_empty(&[
            "GIT_COMMITTER_EMAIL",
            "GIT_AUTHOR_EMAIL",
            "EMAIL",
            "LIBRA_COMMITTER_EMAIL",
        ]),
    })
}

async fn local_config_value_for_target(
    local_target: LocalIdentityTarget<'_>,
    key: &str,
) -> Result<Option<String>> {
    match local_target {
        LocalIdentityTarget::CurrentRepo => {
            let storage = crate::utils::util::try_get_storage_path(None)
                .context("failed to resolve current repository storage")?;
            let db_path = storage.join(crate::utils::util::DATABASE);
            read_config_value_from_db_path(&db_path, key).await
        }
        LocalIdentityTarget::ExplicitDb(db_path) => {
            read_config_value_from_db_path(db_path, key).await
        }
        LocalIdentityTarget::None => Ok(None),
    }
}

async fn global_config_value(key: &str) -> Result<Option<String>> {
    let Some(db_path) = global_config_path() else {
        return Ok(None);
    };
    if !db_path.exists() {
        return Ok(None);
    }
    read_config_value_from_db_path(&db_path, key).await
}

async fn read_config_value_from_db_path(db_path: &Path, key: &str) -> Result<Option<String>> {
    if !db_path.exists() {
        return Ok(None);
    }

    let conn = get_db_conn_instance_for_path(db_path)
        .await
        .with_context(|| format!("failed to open config database '{}'", db_path.display()))?;
    let entry = ConfigKv::get_with_conn(&conn, key).await.with_context(|| {
        format!(
            "failed to query '{key}' from config database '{}'",
            db_path.display()
        )
    })?;

    Ok(entry.and_then(|entry| {
        let trimmed = entry.value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Sensitive key detection
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` if the key holds sensitive material that should be
/// encrypted and redacted by default.
pub fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();

    // Exact-match vault internals
    if lower.starts_with("vault.env.") {
        return true;
    }
    if lower.ends_with(".privkey") {
        return true;
    }
    if lower == "vault.unsealkey" || lower == "vault.roottoken" || lower == "vault.roottoken_enc" {
        return true;
    }

    // Normalize the last segment: remove `_` and `-`, lowercase
    let last_segment = lower.rsplit('.').next().unwrap_or(&lower);
    let normalized: String = last_segment
        .chars()
        .filter(|c| *c != '_' && *c != '-')
        .collect();

    // Explicit exclusion for public keys
    if normalized.ends_with("pubkey") || normalized.ends_with("publickey") {
        return false;
    }

    // Check for sensitive substrings in the normalized last segment
    const SENSITIVE_SUBSTRINGS: &[&str] = &[
        "secret",
        "token",
        "password",
        "credential",
        "privatekey",
        "accesskey",
        "apikey",
        "secretkey",
    ];
    SENSITIVE_SUBSTRINGS.iter().any(|s| normalized.contains(s))
}

/// Returns `true` if the key is a vault internal credential that cannot
/// be `--reveal`ed or stored with `--plaintext`.
pub fn is_vault_internal_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.ends_with(".privkey")
        || lower == "vault.unsealkey"
        || lower == "vault.roottoken"
        || lower == "vault.roottoken_enc"
}

// ─────────────────────────────────────────────────────────────────────────────
// Legacy Config API (deprecated)
// ─────────────────────────────────────────────────────────────────────────────

#[deprecated(note = "use ConfigKv instead")]
pub struct Config;

trait DatabaseConnectionRef {
    fn as_db_conn_ref(&self) -> &DatabaseConnection;
}

impl DatabaseConnectionRef for DatabaseConnection {
    fn as_db_conn_ref(&self) -> &DatabaseConnection {
        self
    }
}

impl DatabaseConnectionRef for &DatabaseConnection {
    fn as_db_conn_ref(&self) -> &DatabaseConnection {
        self
    }
}

#[derive(Clone, Debug)]
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
#[allow(deprecated)]
impl Config {
    // _with_conn version for insert
    pub async fn insert_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        value: &str,
    ) {
        let config = ActiveModel {
            configuration: Set(configuration.to_owned()),
            name: Set(name.map(|s| s.to_owned())),
            key: Set(key.to_owned()),
            value: Set(value.to_owned()),
            ..Default::default()
        };
        config.save(db).await.unwrap();
    }

    // _with_conn version for update
    pub async fn update_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        value: &str,
    ) -> Model {
        let mut config: ActiveModel = config::Entity::find()
            .filter(config::Column::Configuration.eq(configuration))
            .filter(match name {
                Some(str) => config::Column::Name.eq(str),
                None => config::Column::Name.is_null(),
            })
            .filter(config::Column::Key.eq(key))
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .into();
        config.value = Set(value.to_owned());
        config.update(db).await.unwrap()
    }

    // _with_conn version for query
    async fn query_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Vec<Model> {
        config::Entity::find()
            .filter(config::Column::Configuration.eq(configuration))
            .filter(match name {
                Some(str) => config::Column::Name.eq(str),
                None => config::Column::Name.is_null(),
            })
            .filter(config::Column::Key.eq(key))
            .all(db)
            .await
            .unwrap()
    }

    // _with_conn version for get
    pub async fn get_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Option<String> {
        let values = Self::query_with_conn(db, configuration, name, key).await;
        values.first().map(|c| c.value.to_owned())
    }

    // _with_conn version for get_remote
    pub async fn get_remote_with_conn<C: ConnectionTrait>(db: &C, branch: &str) -> Option<String> {
        Config::get_with_conn(db, "branch", Some(branch), "remote").await
    }

    // _with_conn version for get_current_remote
    pub async fn get_current_remote_with_conn<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Option<String>, ()> {
        match Head::current_with_conn(db).await {
            Head::Branch(name) => Ok(Config::get_remote_with_conn(db, &name).await),
            Head::Detached(_) => {
                eprintln!("fatal: HEAD is detached, cannot get remote");
                Err(())
            }
        }
    }

    // _with_conn version for get_remote_url
    pub async fn get_remote_url_with_conn<C: ConnectionTrait>(db: &C, remote: &str) -> String {
        match Config::get_with_conn(db, "remote", Some(remote), "url").await {
            Some(url) => url,
            None => panic!("fatal: No URL configured for remote '{remote}'."),
        }
    }

    // _with_conn version for get_current_remote_url
    pub async fn get_current_remote_url_with_conn<C: ConnectionTrait>(db: &C) -> Option<String> {
        match Config::get_current_remote_with_conn(db).await.unwrap() {
            Some(remote) => Some(Config::get_remote_url_with_conn(db, &remote).await),
            None => None,
        }
    }

    // _with_conn version for get_all
    pub async fn get_all_with_conn<C: ConnectionTrait>(
        db: &C,
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
    pub async fn list_all_with_conn<C: ConnectionTrait>(db: &C) -> Vec<(String, String)> {
        config::Entity::find()
            .all(db)
            .await
            .unwrap()
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
            .collect()
    }

    // _with_conn version for remove_config
    pub async fn remove_config_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        valuepattern: Option<&str>,
        delete_all: bool,
    ) -> Result<(), sea_orm::DbErr> {
        let entries: Vec<Model> = Self::query_with_conn(db, configuration, name, key).await;
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
            .unwrap();
        if remote.is_empty() {
            return Err(format!("fatal: No such remote: {name}"));
        }
        for r in remote {
            let r: ActiveModel = r.into();
            r.delete(db).await.unwrap();
        }
        Ok(())
    }

    pub async fn rename_remote_with_conn<C: ConnectionTrait>(
        db: &C,
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

        let remote_entries = config::Entity::find()
            .filter(config::Column::Configuration.eq("remote"))
            .filter(config::Column::Name.eq(old))
            .all(db)
            .await
            .unwrap();

        // Update remote.<name>.* entries to point at the new name.
        for entry in remote_entries {
            let mut active: ActiveModel = entry.into();
            active.name = Set(Some(new.to_owned()));
            active.update(db).await.unwrap();
        }

        let branch_entries = config::Entity::find()
            .filter(config::Column::Configuration.eq("branch"))
            .filter(config::Column::Key.eq("remote"))
            .filter(config::Column::Value.eq(old))
            .all(db)
            .await
            .unwrap();

        // Repoint branch.*.remote values that referenced the old remote.
        for entry in branch_entries {
            let mut active: ActiveModel = entry.into();
            active.value = Set(new.to_owned());
            active.update(db).await.unwrap();
        }

        Ok(())
    }

    // _with_conn version for all_remote_configs
    pub async fn all_remote_configs_with_conn<C: ConnectionTrait>(db: &C) -> Vec<RemoteConfig> {
        let remotes = config::Entity::find()
            .filter(config::Column::Configuration.eq("remote"))
            .all(db)
            .await
            .unwrap();
        let remote_names = remotes
            .iter()
            .map(|remote| remote.name.as_ref().unwrap().clone())
            .collect::<HashSet<String>>();

        remote_names
            .iter()
            .map(|name| {
                let url = remotes
                    .iter()
                    .find(|remote| remote.name.as_ref().unwrap() == name)
                    .unwrap()
                    .value
                    .to_owned();
                RemoteConfig {
                    name: name.to_owned(),
                    url,
                }
            })
            .collect()
    }

    // _with_conn version for remote_config
    pub async fn remote_config_with_conn<C: ConnectionTrait>(
        db: &C,
        name: &str,
    ) -> Option<RemoteConfig> {
        let remote = config::Entity::find()
            .filter(config::Column::Configuration.eq("remote"))
            .filter(config::Column::Name.eq(name))
            .one(db)
            .await
            .unwrap();
        remote.map(|r| RemoteConfig {
            name: r.name.unwrap(),
            url: r.value,
        })
    }

    // _with_conn version for branch_config
    pub async fn branch_config_with_conn<C: ConnectionTrait>(
        db: &C,
        name: &str,
    ) -> Option<BranchConfig> {
        let config_entries = config::Entity::find()
            .filter(config::Column::Configuration.eq("branch"))
            .filter(config::Column::Name.eq(name))
            .all(db)
            .await
            .unwrap();
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
        Self::insert_with_conn(&db, configuration, name, key, value).await;
    }

    // Update one configuration entry in database using given configuration, name, key and value
    pub async fn update(configuration: &str, name: Option<&str>, key: &str, value: &str) -> Model {
        let db = get_db_conn_instance().await;
        Self::update_with_conn(&db, configuration, name, key, value).await
    }

    /// Get one configuration value
    pub async fn get(configuration: &str, name: Option<&str>, key: &str) -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_with_conn(&db, configuration, name, key).await
    }

    /// Get remote repo name by branch name
    /// - You may need to `[branch::set-upstream]` if return `None`
    pub async fn get_remote(branch: &str) -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_remote_with_conn(&db, branch).await
    }

    /// Get remote repo name of current branch
    /// - `Error` if `HEAD` is detached
    pub async fn get_current_remote() -> Result<Option<String>, ()> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_with_conn(&db).await
    }

    pub async fn get_remote_url(remote: &str) -> String {
        let db = get_db_conn_instance().await;
        Self::get_remote_url_with_conn(&db, remote).await
    }

    /// return `None` if no remote is set
    pub async fn get_current_remote_url() -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_url_with_conn(&db).await
    }

    /// Get all configuration values
    /// - e.g. remote.origin.url can be multiple
    pub async fn get_all(configuration: &str, name: Option<&str>, key: &str) -> Vec<String> {
        let db = get_db_conn_instance().await;
        Self::get_all_with_conn(&db, configuration, name, key).await
    }

    /// Get literally all the entries in database without any filtering
    pub async fn list_all() -> Vec<(String, String)> {
        let db = get_db_conn_instance().await;
        Self::list_all_with_conn(&db).await
    }

    /// Delete one or all configuration using given key and value pattern
    pub async fn remove_config(
        configuration: &str,
        name: Option<&str>,
        key: &str,
        valuepattern: Option<&str>,
        delete_all: bool,
    ) -> Result<(), sea_orm::DbErr> {
        let db = get_db_conn_instance().await;
        Self::remove_config_with_conn(
            db.as_db_conn_ref(),
            configuration,
            name,
            key,
            valuepattern,
            delete_all,
        )
        .await
    }

    /// Remove all entries matching the given configuration/name/key triple.
    pub async fn remove(
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Result<(), sea_orm::DbErr> {
        Self::remove_config(configuration, name, key, None, true).await
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
        Self::all_remote_configs_with_conn(&db).await
    }

    pub async fn remote_config(name: &str) -> Option<RemoteConfig> {
        let db = get_db_conn_instance().await;
        Self::remote_config_with_conn(&db, name).await
    }

    pub async fn branch_config(name: &str) -> Option<BranchConfig> {
        let db = get_db_conn_instance().await;
        Self::branch_config_with_conn(&db, name).await
    }
}
