//! Config storage helpers backed by sea-orm.
//!
//! Two APIs exist side-by-side:
//!
//! 1. [`ConfigKv`] (preferred) — flat dotted keys like `remote.origin.url` stored
//!    in the `config_kv` table, with per-row encryption support and a richer
//!    set of CRUD primitives (`set`, `add`, `unset`, `unset_all`, regex/prefix
//!    queries). All new code should use this API.
//! 2. [`Config`] (deprecated) — three-column form `(configuration, name, key)`
//!    stored in the legacy `config` table. Retained for backwards-compatible
//!    repos that have not yet migrated.
//!
//! Both APIs follow the same `*_with_conn` transaction-safety convention used
//! by [`crate::internal::branch`]: callers inside an open transaction must use
//! the `_with_conn` variants to avoid acquiring a second pool connection
//! (which deadlocks under SQLite's writer-serialisation).
//!
//! Cross-cutting helpers in this module:
//! - [`resolve_env`] / [`resolve_env_for_target`]: cascading env-var resolution
//!   (process env > local repo config > global config).
//! - [`is_sensitive_key`] / [`is_vault_internal_key`]: heuristics that drive the
//!   encrypt-by-default policy in `libra config`.
//! - [`encrypt_value`] / [`decrypt_value`]: thin wrappers over the vault module.

use std::{collections::HashSet, mem::swap, path::Path};

use anyhow::{Context, Result, anyhow};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, ModelTrait,
    QueryFilter, QueryOrder, entity::ActiveModelTrait,
};

use crate::{
    internal::{
        db::{get_db_conn_instance, get_db_conn_instance_for_path},
        head::Head,
        model::{
            config::{self, ActiveModel, Model},
            config_kv,
        },
        vault::{decrypt_token, encrypt_token, load_unseal_key_for_scope},
    },
    utils::util::{DATABASE, try_get_storage_path},
};

// ─────────────────────────────────────────────────────────────────────────────
// ConfigKv — new flat key/value API backed by the `config_kv` table
// ─────────────────────────────────────────────────────────────────────────────

/// One row from the `config_kv` table, decoded for application use.
///
/// `encrypted == true` means `value` is hex-encoded ciphertext that must be
/// decrypted via [`decrypt_value`] before display. The encrypt flag is stored
/// as INTEGER (0/1) in SQLite; this struct normalises it to `bool`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigKvEntry {
    /// Dotted config key, e.g. `remote.origin.url` or `vault.env.GEMINI_API_KEY`.
    pub key: String,
    /// Either plaintext or hex ciphertext depending on `encrypted`.
    pub value: String,
    /// `true` when `value` is hex-encoded ciphertext.
    pub encrypted: bool,
}

impl ConfigKvEntry {
    /// Convert a sea-orm row into the public [`ConfigKvEntry`] shape.
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
/// Marker struct; all methods are associated functions. Calling a method
/// without `_with_conn` acquires its own connection — do **not** call those
/// from inside a `db.transaction(|txn| { ... })` block (deadlock).
pub struct ConfigKv;

impl ConfigKv {
    // ── Core CRUD (_with_conn) ───────────────────────────────────────────

    /// Get the last value for a key (last-one-wins for multi-value keys).
    ///
    /// Boundary conditions:
    /// - Returns `Ok(None)` if no row exists.
    /// - When multiple rows share the key (multi-value config like
    ///   `remote.origin.fetch`), the row with the highest `id` wins,
    ///   matching git's "last write" rule.
    /// - The returned value is *not* decrypted; callers must inspect
    ///   `encrypted` and call [`decrypt_value`] themselves.
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

    /// Get all values for a key (preserves insertion order via ascending `id`).
    ///
    /// Used by multi-value keys (e.g. `remote.origin.fetch` may have several
    /// refspec entries). Returns an empty `Vec` when no rows match.
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
    ///
    /// Returns `Ok(0)` when no rows exist. Used by callers that need to decide
    /// between `set` (single-value) and `add` (multi-value) semantics.
    pub async fn count_values_with_conn<C: ConnectionTrait>(db: &C, key: &str) -> Result<usize> {
        let rows = config_kv::Entity::find()
            .filter(config_kv::Column::Key.eq(key))
            .all(db)
            .await
            .context("failed to count config_kv entries")?;
        Ok(rows.len())
    }

    /// Set a config value (upsert).
    ///
    /// Functional scope:
    /// - If exactly one row exists for `key`, updates it in place.
    /// - If no row exists, inserts a fresh row.
    /// - When the existing row is encrypted but `encrypted == false` is
    ///   passed, the encryption flag is *inherited* (preserved). This avoids
    ///   accidentally downgrading a sensitive value to plaintext.
    ///
    /// Boundary conditions:
    /// - Returns `Err` if multiple rows already exist for `key` — the caller
    ///   must explicitly `unset_all` first or use `add`. Mirrors `git config`'s
    ///   exit code 5.
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
    ///
    /// Boundary conditions:
    /// - First-write (no rows yet) is always accepted with the requested flag.
    /// - Returns `Err` when mixing plaintext and encrypted values would result.
    ///   This is a hard invariant of `config_kv`; callers cannot opt out.
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
    ///
    /// Boundary conditions: returns `Err` if multiple rows match — caller must
    /// use [`Self::unset_all_with_conn`] explicitly to remove every row.
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
    /// Returns the number of rows deleted (0 if none matched).
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

    /// List all config entries, sorted by key.
    ///
    /// Useful for `libra config --list`. Encrypted values are returned as
    /// hex ciphertext; the CLI is responsible for redaction.
    pub async fn list_all_with_conn<C: ConnectionTrait>(db: &C) -> Result<Vec<ConfigKvEntry>> {
        let rows = config_kv::Entity::find()
            .order_by_asc(config_kv::Column::Key)
            .all(db)
            .await
            .context("failed to list config_kv")?;
        Ok(rows.iter().map(ConfigKvEntry::from_model).collect())
    }

    /// Get all entries whose key starts with the given prefix.
    ///
    /// Used by domain helpers (`all_remote_configs`, etc.) to scope searches
    /// without having to enumerate every section name. Empty prefix returns
    /// all rows in key order.
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
    ///
    /// Boundary conditions:
    /// - Returns `Err` for invalid regex syntax.
    /// - SQLite has no native `REGEXP`, so we fetch every row and filter in
    ///   Rust. Acceptable cost given config tables are small.
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
    // Each of these pairs with a `*_with_conn` variant above. They acquire
    // a connection from the global pool; do not call them inside a
    // `db.transaction(|txn| { ... })` block — that deadlocks. Use the
    // `_with_conn` variant instead.

    /// Pool-acquiring counterpart of [`Self::get_with_conn`].
    pub async fn get(key: &str) -> Result<Option<ConfigKvEntry>> {
        let db = get_db_conn_instance().await;
        Self::get_with_conn(&db, key).await
    }

    /// Pool-acquiring counterpart of [`Self::get_all_with_conn`].
    pub async fn get_all(key: &str) -> Result<Vec<ConfigKvEntry>> {
        let db = get_db_conn_instance().await;
        Self::get_all_with_conn(&db, key).await
    }

    /// Pool-acquiring counterpart of [`Self::set_with_conn`].
    pub async fn set(key: &str, value: &str, encrypted: bool) -> Result<()> {
        let db = get_db_conn_instance().await;
        Self::set_with_conn(&db, key, value, encrypted).await
    }

    /// Pool-acquiring counterpart of [`Self::add_with_conn`].
    pub async fn add(key: &str, value: &str, encrypted: bool) -> Result<()> {
        let db = get_db_conn_instance().await;
        Self::add_with_conn(&db, key, value, encrypted).await
    }

    /// Pool-acquiring counterpart of [`Self::unset_with_conn`].
    pub async fn unset(key: &str) -> Result<usize> {
        let db = get_db_conn_instance().await;
        Self::unset_with_conn(&db, key).await
    }

    /// Pool-acquiring counterpart of [`Self::unset_all_with_conn`].
    pub async fn unset_all(key: &str) -> Result<usize> {
        let db = get_db_conn_instance().await;
        Self::unset_all_with_conn(&db, key).await
    }

    /// Pool-acquiring counterpart of [`Self::list_all_with_conn`].
    pub async fn list_all() -> Result<Vec<ConfigKvEntry>> {
        let db = get_db_conn_instance().await;
        Self::list_all_with_conn(&db).await
    }

    /// Pool-acquiring counterpart of [`Self::get_by_prefix_with_conn`].
    pub async fn get_by_prefix(prefix: &str) -> Result<Vec<ConfigKvEntry>> {
        let db = get_db_conn_instance().await;
        Self::get_by_prefix_with_conn(&db, prefix).await
    }

    // ── Type helpers ─────────────────────────────────────────────────────

    /// Get a boolean config value. Normalises `true/yes/on/1` -> `true`,
    /// `false/no/off/0` -> `false`.
    ///
    /// Boundary conditions:
    /// - Returns `Ok(None)` when the key is absent.
    /// - Returns `Err` if the value is present but does not match any of the
    ///   recognised tokens.
    /// - Encrypted values display as `<REDACTED>` in the error message so
    ///   ciphertext is not echoed back to the user.
    pub async fn get_bool_with_conn<C: ConnectionTrait>(db: &C, key: &str) -> Result<Option<bool>> {
        let entry = Self::get_with_conn(db, key).await?;
        match entry {
            None => Ok(None),
            Some(e) => parse_config_bool(&e.value).map(Some).ok_or_else(|| {
                anyhow!(
                    "invalid value '{}' for key '{}': expected bool (true/false)",
                    if e.encrypted { "<REDACTED>" } else { &e.value },
                    key
                )
            }),
        }
    }

    /// Get an integer config value. Supports `k`/`m`/`g` suffixes.
    ///
    /// Multipliers are 1024-based (KiB/MiB/GiB) to mirror `git config --int`
    /// behaviour. Returns `Ok(None)` for missing keys, `Err` for unparseable
    /// values, with the same `<REDACTED>` policy as [`Self::get_bool_with_conn`].
    pub async fn get_int_with_conn<C: ConnectionTrait>(db: &C, key: &str) -> Result<Option<i64>> {
        let entry = Self::get_with_conn(db, key).await?;
        match entry {
            None => Ok(None),
            // Delegate to the overflow-checked pure parser; preserve the
            // `<REDACTED>` policy so ciphertext is never echoed on error.
            Some(e) => parse_config_int(&e.value).map(Some).map_err(|err| {
                anyhow!(
                    "invalid value '{}' for key '{}': {err}",
                    if e.encrypted { "<REDACTED>" } else { &e.value },
                    key
                )
            }),
        }
    }

    // ── Domain helpers (replace old Config methods) ──────────────────────

    /// Get the value of `remote.<remote>.url`.
    ///
    /// Returns a user-friendly `fatal:` error when the key is absent —
    /// commands like `push`/`fetch` rely on this exact message format.
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

    /// Pool-acquiring counterpart of [`Self::get_remote_url_with_conn`].
    pub async fn get_remote_url(remote: &str) -> Result<String> {
        let db = get_db_conn_instance().await;
        Self::get_remote_url_with_conn(&db, remote).await
    }

    /// Get remote name for a branch from `branch.<branch>.remote`.
    ///
    /// Returns `Ok(None)` for branches that have no upstream configured.
    pub async fn get_remote_with_conn<C: ConnectionTrait>(
        db: &C,
        branch: &str,
    ) -> Result<Option<String>> {
        let key = format!("branch.{branch}.remote");
        Ok(Self::get_with_conn(db, &key).await?.map(|e| e.value))
    }

    /// Pool-acquiring counterpart of [`Self::get_remote_with_conn`].
    pub async fn get_remote(branch: &str) -> Result<Option<String>> {
        let db = get_db_conn_instance().await;
        Self::get_remote_with_conn(&db, branch).await
    }

    /// Get remote for the current HEAD branch.
    ///
    /// Boundary conditions:
    /// - Returns `Ok(None)` when HEAD points to a valid branch but no upstream.
    /// - Returns `Err` when HEAD is detached, since "the current branch's
    ///   remote" is undefined in that state.
    pub async fn get_current_remote_with_conn<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Option<String>> {
        match Head::current_with_conn(db).await {
            Head::Branch(name) => Self::get_remote_with_conn(db, &name).await,
            Head::Detached(_) => Err(anyhow!("fatal: HEAD is detached, cannot get remote")),
        }
    }

    /// Pool-acquiring counterpart of [`Self::get_current_remote_with_conn`].
    pub async fn get_current_remote() -> Result<Option<String>> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_with_conn(&db).await
    }

    /// Get remote URL for the current HEAD branch.
    ///
    /// Returns `Ok(None)` when no upstream is configured. Returns `Err` if
    /// the upstream is set to a remote that itself has no `url` configured
    /// — this is treated as repository corruption.
    pub async fn get_current_remote_url_with_conn<C: ConnectionTrait>(
        db: &C,
    ) -> Result<Option<String>> {
        match Self::get_current_remote_with_conn(db).await? {
            Some(remote) => Ok(Some(Self::get_remote_url_with_conn(db, &remote).await?)),
            None => Ok(None),
        }
    }

    /// Pool-acquiring counterpart of [`Self::get_current_remote_url_with_conn`].
    pub async fn get_current_remote_url() -> Result<Option<String>> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_url_with_conn(&db).await
    }

    /// Enumerate every configured remote and its URL.
    ///
    /// Discovery rule: walks rows under the `remote.` prefix, treating any
    /// key of the form `remote.<name>.url` as a remote definition. Other keys
    /// (`fetch`, `push`, etc.) are ignored here. Returns each remote at most
    /// once, preserving discovery order.
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

    /// Pool-acquiring counterpart of [`Self::all_remote_configs_with_conn`].
    pub async fn all_remote_configs() -> Result<Vec<RemoteConfig>> {
        let db = get_db_conn_instance().await;
        Self::all_remote_configs_with_conn(&db).await
    }

    /// Get a specific remote's config (`Ok(None)` when no `remote.<name>.url`).
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

    /// Pool-acquiring counterpart of [`Self::remote_config_with_conn`].
    pub async fn remote_config(name: &str) -> Result<Option<RemoteConfig>> {
        let db = get_db_conn_instance().await;
        Self::remote_config_with_conn(&db, name).await
    }

    /// Get branch tracking configuration (the upstream remote and merge ref).
    ///
    /// Boundary conditions:
    /// - Returns `Ok(None)` when either `branch.<name>.remote` or
    ///   `branch.<name>.merge` is missing. Both must be set together for
    ///   tracking to be valid.
    /// - The returned `merge` field has `refs/heads/` stripped if present so
    ///   callers can compare it directly against short branch names.
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

    /// Pool-acquiring counterpart of [`Self::branch_config_with_conn`].
    pub async fn branch_config(name: &str) -> Result<Option<BranchConfig>> {
        let db = get_db_conn_instance().await;
        Self::branch_config_with_conn(&db, name).await
    }

    /// Remove all config entries for a remote, including its SSH credentials.
    ///
    /// Cascading deletes:
    /// 1. Every `remote.<name>.*` row.
    /// 2. Every `vault.ssh.<name>.*` row (private keys, host fingerprints).
    ///
    /// Boundary condition: returns `Err("fatal: No such remote ...")` when the
    /// `remote.<name>.*` namespace is empty. The SSH cleanup never errors on
    /// its own — orphan vault rows are tolerated.
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

    /// Pool-acquiring counterpart of [`Self::remove_remote_with_conn`].
    pub async fn remove_remote(name: &str) -> Result<()> {
        let db = get_db_conn_instance().await;
        Self::remove_remote_with_conn(&db, name).await
    }

    /// Rename a remote, updating all related config entries atomically.
    ///
    /// Performs three cascading rewrites:
    /// 1. `remote.<old>.*` keys are renamed to `remote.<new>.*`.
    /// 2. Any `branch.*.remote = <old>` value is updated to `<new>`.
    /// 3. `vault.ssh.<old>.*` SSH key namespace is renamed to
    ///    `vault.ssh.<new>.*` so credentials follow the rename.
    ///
    /// Boundary conditions:
    /// - Returns `Err` if `<old>` does not exist or `<new>` already exists,
    ///   matching git's "fatal: ..." error format.
    /// - This function is *not* atomic across rewrites. Wrap in a sea-orm
    ///   transaction (and call this `_with_conn` variant with `txn`) when
    ///   atomicity matters.
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
        let ssh_old_prefix = format!("vault.ssh.{old}.");
        let ssh_new_prefix = format!("vault.ssh.{new}.");
        let existing_target_ssh_entries = config_kv::Entity::find()
            .filter(config_kv::Column::Key.starts_with(&ssh_new_prefix))
            .all(db)
            .await
            .context("failed to query target SSH key entries for rename")?;
        if !existing_target_ssh_entries.is_empty() {
            return Err(anyhow!(
                "fatal: SSH key namespace for remote '{new}' already exists"
            ));
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

    /// Pool-acquiring counterpart of [`Self::rename_remote_with_conn`].
    pub async fn rename_remote(old: &str, new: &str) -> Result<()> {
        let db = get_db_conn_instance().await;
        Self::rename_remote_with_conn(&db, old, new).await
    }

    // ── Value-filtered multi-value mutations (Git --value/--replace-all) ──
    // These operate by row `id` inside a caller-provided transaction so that
    // duplicate identical values are never mis-deleted and any error leaves the
    // store unchanged. They must NOT begin/commit — the command layer owns the
    // transaction (see `command/config.rs`).

    /// Replace the values of `key` that match `filter` (or *all* values when
    /// `filter` is `None`) with a single `new` value.
    ///
    /// Git `--replace-all` / `set --all` semantics:
    /// - matching rows are deleted (by `id`) and one `new` row is inserted;
    /// - when nothing matches, `new` is still inserted (add-if-no-match), so the
    ///   call is idempotent and never fails on an empty match set;
    /// - `new` is written with the caller-provided `encrypted` flag — the caller
    ///   is responsible for encrypting `new` and for honouring
    ///   same-key-same-state.
    ///
    /// When `enforce_single` is `true`, a match set larger than one row is a hard
    /// error (no mutation) so the caller can map it to Git's "multiple values"
    /// exit code 5. The caller must already be inside a transaction.
    pub async fn replace_matching_with_conn<C: ConnectionTrait>(
        db: &C,
        key: &str,
        new: &str,
        encrypted: bool,
        filter: Option<&ValueFilter>,
        enforce_single: bool,
    ) -> Result<ReplaceOutcome> {
        let rows = config_kv::Entity::find()
            .filter(config_kv::Column::Key.eq(key))
            .order_by_asc(config_kv::Column::Id)
            .all(db)
            .await
            .context("failed to query config_kv for replace")?;

        let matched_ids: Vec<i64> = match filter {
            Some(f) => rows
                .iter()
                .filter(|r| f.matches(&r.value))
                .map(|r| r.id)
                .collect(),
            None => rows.iter().map(|r| r.id).collect(),
        };

        if enforce_single && matched_ids.len() > 1 {
            return Err(anyhow!(
                "cannot set '{}': {} values exist for this key",
                key,
                matched_ids.len()
            ));
        }

        for id in &matched_ids {
            config_kv::Entity::delete_by_id(*id)
                .exec(db)
                .await
                .context("failed to delete config_kv entry during replace")?;
        }

        let entry = config_kv::ActiveModel {
            key: Set(key.to_owned()),
            value: Set(new.to_owned()),
            encrypted: Set(if encrypted { 1 } else { 0 }),
            ..Default::default()
        };
        entry
            .save(db)
            .await
            .context("failed to insert replacement config_kv entry")?;

        Ok(ReplaceOutcome {
            matched: matched_ids.len(),
            inserted: true,
        })
    }

    /// Delete the values of `key` that match `filter`.
    ///
    /// Returns the number of rows deleted. When `enforce_single` is `true`, a
    /// match set larger than one row is a hard error (no deletion) so the caller
    /// can map it to exit code 5 (Git's default `--unset` ambiguity guard). The
    /// caller must already be inside a transaction.
    pub async fn unset_matching_with_conn<C: ConnectionTrait>(
        db: &C,
        key: &str,
        filter: &ValueFilter,
        enforce_single: bool,
    ) -> Result<usize> {
        let rows = config_kv::Entity::find()
            .filter(config_kv::Column::Key.eq(key))
            .order_by_asc(config_kv::Column::Id)
            .all(db)
            .await
            .context("failed to query config_kv for filtered unset")?;

        let matched_ids: Vec<i64> = rows
            .iter()
            .filter(|r| filter.matches(&r.value))
            .map(|r| r.id)
            .collect();

        if enforce_single && matched_ids.len() > 1 {
            return Err(anyhow!(
                "cannot unset '{}': {} values exist for this key",
                key,
                matched_ids.len()
            ));
        }

        for id in &matched_ids {
            config_kv::Entity::delete_by_id(*id)
                .exec(db)
                .await
                .context("failed to delete config_kv entry during filtered unset")?;
        }
        Ok(matched_ids.len())
    }

    // ── Generic section operations (Git rename-section / remove-section) ──
    // These operate on a dotted prefix (`section + "."`) inside a
    // caller-provided transaction. Matching is performed in Rust (exact
    // `starts_with`) rather than SQL `LIKE` so that `_` in a section name is
    // never treated as a wildcard. They do NOT reuse the remote-specific
    // rename/remove helpers and therefore have no branch/SSH cascade side
    // effects. Encryption flags and ciphertext are preserved verbatim.

    /// Rename every key under `old_prefix` to `new_prefix` (key rewrite only).
    ///
    /// Returns the number of keys moved. Returns `Ok(0)` when the source section
    /// is empty (caller maps to exit 5 "does not exist"). Returns `Err` with a
    /// message containing "already exists" when any key already exists under
    /// `new_prefix` — in that case nothing is changed (the caller rolls the
    /// transaction back). Caller must already be inside a transaction.
    pub async fn rename_section_with_conn<C: ConnectionTrait>(
        db: &C,
        old_prefix: &str,
        new_prefix: &str,
    ) -> Result<u64> {
        let all_rows = config_kv::Entity::find()
            .order_by_asc(config_kv::Column::Id)
            .all(db)
            .await
            .context("failed to query config_kv for section rename")?;

        let source: Vec<&config_kv::Model> = all_rows
            .iter()
            .filter(|m| m.key.starts_with(old_prefix))
            .collect();
        if source.is_empty() {
            return Ok(0);
        }
        if all_rows.iter().any(|m| m.key.starts_with(new_prefix)) {
            return Err(anyhow!("target section already exists"));
        }

        let mut moved = 0u64;
        for row in source {
            let suffix = &row.key[old_prefix.len()..];
            let new_key = format!("{new_prefix}{suffix}");
            let mut active: config_kv::ActiveModel = row.clone().into();
            active.key = Set(new_key);
            active
                .update(db)
                .await
                .context("failed to rewrite config key during section rename")?;
            moved += 1;
        }
        Ok(moved)
    }

    /// Delete every key under `section_prefix`. Returns the number of rows
    /// deleted (0 → caller maps to exit 5 "does not exist"). Caller must already
    /// be inside a transaction.
    pub async fn remove_section_with_conn<C: ConnectionTrait>(
        db: &C,
        section_prefix: &str,
    ) -> Result<u64> {
        let all_rows = config_kv::Entity::find()
            .all(db)
            .await
            .context("failed to query config_kv for section removal")?;
        let ids: Vec<i64> = all_rows
            .iter()
            .filter(|m| m.key.starts_with(section_prefix))
            .map(|m| m.id)
            .collect();
        for id in &ids {
            config_kv::Entity::delete_by_id(*id)
                .exec(db)
                .await
                .context("failed to delete config_kv entry during section removal")?;
        }
        Ok(ids.len() as u64)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Git-compatible value filtering and key validation
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum byte length accepted for a user-supplied config regex pattern.
///
/// Bounds ReDoS exposure: the `regex` crate is already linear-time (NFA), but an
/// unbounded pattern still costs compile time and memory. Patterns over this
/// limit are rejected as invalid (the CLI maps that to exit code 6).
pub const MAX_CONFIG_REGEX_LEN: usize = 4096;

/// Reject regex patterns that exceed [`MAX_CONFIG_REGEX_LEN`] bytes.
pub fn validate_config_regex_pattern(pattern: &str) -> Result<()> {
    if pattern.len() > MAX_CONFIG_REGEX_LEN {
        anyhow::bail!("regex pattern is too long (max {MAX_CONFIG_REGEX_LEN} bytes)");
    }
    Ok(())
}

/// Outcome of [`ConfigKv::replace_matching_with_conn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplaceOutcome {
    /// Number of pre-existing rows that matched the filter and were deleted.
    pub matched: usize,
    /// Whether the new value row was inserted (always `true` today).
    pub inserted: bool,
}

/// A compiled Git-compatible value filter for multi-value `--value` matching.
///
/// Construct with [`ValueFilter::compile`], which validates and compiles up
/// front so an invalid pattern fails *before* any DB access. Matching runs
/// against the *stored* value — for encrypted rows that is the hex ciphertext,
/// so sensitive values are never decrypted merely to be filtered.
#[derive(Debug, Clone)]
pub struct ValueFilter {
    kind: ValueFilterKind,
}

#[derive(Debug, Clone)]
enum ValueFilterKind {
    /// `--fixed-value`: literal string equality (case-sensitive; leading `!`
    /// is an ordinary character).
    Literal(String),
    /// Regex match; `negate` is set when the pattern began with a single `!`.
    Regex { re: regex::Regex, negate: bool },
}

impl ValueFilter {
    /// Compile a value filter.
    ///
    /// - `fixed`: treat `pattern` as a literal string (Git `--fixed-value`); a
    ///   leading `!` is an ordinary character and `ignore_case` is ignored.
    /// - otherwise `pattern` is a regex; a single leading `!` negates the match
    ///   (Git value-pattern negation), and `ignore_case` makes the regex
    ///   case-insensitive.
    ///
    /// Returns `Err` for an over-long (>4 KiB) or syntactically invalid regex;
    /// the CLI maps that to exit code 6.
    pub fn compile(pattern: &str, fixed: bool, ignore_case: bool) -> Result<Self> {
        if fixed {
            return Ok(Self {
                kind: ValueFilterKind::Literal(pattern.to_string()),
            });
        }
        validate_config_regex_pattern(pattern)?;
        let (negate, body) = match pattern.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, pattern),
        };
        let re = regex::RegexBuilder::new(body)
            .case_insensitive(ignore_case)
            .build()
            .map_err(|e| anyhow!("invalid value regex '{}': {}", body, e))?;
        Ok(Self {
            kind: ValueFilterKind::Regex { re, negate },
        })
    }

    /// Test whether a stored value matches this filter.
    pub fn matches(&self, stored: &str) -> bool {
        match &self.kind {
            ValueFilterKind::Literal(needle) => stored == needle,
            ValueFilterKind::Regex { re, negate } => re.is_match(stored) ^ negate,
        }
    }
}

/// Pure value-filter predicate, primarily for unit tests.
///
/// Compiles a fresh [`ValueFilter`] each call; production read/mutation paths
/// compile once and reuse [`ValueFilter::matches`].
pub fn matches_value_filter(
    stored: &str,
    pattern: &str,
    fixed: bool,
    ignore_case: bool,
) -> Result<bool> {
    Ok(ValueFilter::compile(pattern, fixed, ignore_case)?.matches(stored))
}

/// Parse a Git-style boolean (`true/yes/on/1`, `false/no/off/0`), case-insensitive.
/// Returns `None` for any other token.
pub fn parse_config_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// Parse a Git-style integer with an optional case-insensitive `k`/`m`/`g`
/// (1024-based) suffix. Multiplication is overflow-checked: an out-of-range
/// product returns `Err` rather than panicking (debug) or wrapping (release),
/// so `--type=int` on a huge value maps cleanly to an exit-2 error.
pub fn parse_config_int(value: &str) -> Result<i64> {
    let s = value.trim().to_ascii_lowercase();
    let (num_str, multiplier) = if let Some(stripped) = s.strip_suffix('k') {
        (stripped, 1024i64)
    } else if let Some(stripped) = s.strip_suffix('m') {
        (stripped, 1024 * 1024)
    } else if let Some(stripped) = s.strip_suffix('g') {
        (stripped, 1024 * 1024 * 1024)
    } else {
        (s.as_str(), 1i64)
    };
    let n: i64 = num_str.parse().map_err(|_| anyhow!("expected integer"))?;
    n.checked_mul(multiplier)
        .ok_or_else(|| anyhow!("integer value out of range (i64 overflow)"))
}

/// Validate a user-typed config key for the flat `config_kv` store.
///
/// Permissive by design: underscores, digits, camelCase, and any number of
/// dotted segments are all legal — real keys like `vault.env.GEMINI_API_KEY`
/// and `cloud.clone_domains.<domain>.account_id` depend on it. Only genuine
/// malformations are rejected. This is NOT applied to `--import` (Git
/// subsections legitimately contain `/`, `@`, `:` bytes).
pub fn validate_key_syntax(key: &str) -> Result<()> {
    if !key.contains('.') {
        anyhow::bail!("invalid key '{key}': must contain a section (e.g. section.name)");
    }
    if key.starts_with('.') || key.ends_with('.') || key.contains("..") {
        anyhow::bail!("invalid key '{key}': empty section or name component");
    }
    if key
        .chars()
        .any(|c| c == '\0' || c.is_control() || c.is_whitespace())
    {
        anyhow::bail!("invalid key '{key}': contains NUL, control, or whitespace characters");
    }
    Ok(())
}

/// Validate a section name for `rename-section` / `remove-section`.
///
/// A section need not itself contain a dot (e.g. a bare `core`), but must not be
/// empty, have leading/trailing/consecutive dots, or contain NUL / control /
/// whitespace / `*` wildcard characters.
pub fn validate_section_syntax(section: &str) -> Result<()> {
    if section.is_empty() {
        anyhow::bail!("invalid section: section name must not be empty");
    }
    if section.starts_with('.') || section.ends_with('.') || section.contains("..") {
        anyhow::bail!("invalid section '{section}': empty section/name component");
    }
    if section
        .chars()
        .any(|c| c == '\0' || c.is_control() || c.is_whitespace() || c == '*')
    {
        anyhow::bail!(
            "invalid section '{section}': contains NUL, control, whitespace, or wildcard characters"
        );
    }
    Ok(())
}

/// Returns `true` if a section name / key sits in a protected vault namespace
/// that generic `rename-section` / `remove-section` must refuse.
///
/// Neither [`is_sensitive_key`] nor [`is_vault_internal_key`] covers the whole
/// `vault.*` namespace, so this predicate exists specifically for section
/// mutations: any `vault` / `vault.*` section, or any vault-internal key, is
/// protected. (Ordinary auto-encrypted keys outside `vault.*` may still be moved
/// by a generic section rename; their ciphertext travels with the key name.)
pub fn is_protected_vault_section(key_or_prefix: &str) -> bool {
    let lower = key_or_prefix.to_ascii_lowercase();
    lower == "vault" || lower.starts_with("vault.") || is_vault_internal_key(&lower)
}

// ─────────────────────────────────────────────────────────────────────────────
// Environment variable resolution
// ─────────────────────────────────────────────────────────────────────────────

/// Decrypt a hex-encoded ciphertext using the vault unseal key for the given scope.
///
/// `scope` should be `"local"` (current repo's `.libra/libra.db`) or `"global"`
/// (`~/.libra/config.db`). Returns `Err` if the vault for that scope is sealed
/// or the ciphertext is malformed.
pub async fn decrypt_value(hex_ciphertext: &str, scope: &str) -> Result<String> {
    let unseal_key = load_unseal_key_for_scope(scope)
        .await
        .ok_or_else(|| anyhow!("vault not initialized for {scope} scope — cannot decrypt value"))?;
    decrypt_value_with_unseal_key(hex_ciphertext, &unseal_key)
}

/// Decrypt a value using the unseal key tied to a specific local target.
///
/// Used when the resolution chain points at a non-default repository (for
/// example when `libra config --file path/to/db get`). Returns `Err` if the
/// requested vault is sealed or has no unseal key.
async fn decrypt_value_for_local_target(
    hex_ciphertext: &str,
    local_target: LocalIdentityTarget<'_>,
) -> Result<String> {
    let unseal_key = match local_target {
        LocalIdentityTarget::CurrentRepo => {
            crate::internal::vault::load_unseal_key_for_scope("local").await
        }
        LocalIdentityTarget::ExplicitDb(db_path) => {
            crate::internal::vault::load_unseal_key_for_db_path(db_path).await
        }
        LocalIdentityTarget::None => None,
    }
    .ok_or_else(|| anyhow!("vault not initialized for local scope — cannot decrypt value"))?;

    decrypt_value_with_unseal_key(hex_ciphertext, &unseal_key)
}

/// Hex-decode `hex_ciphertext` and pass the bytes to [`decrypt_token`].
///
/// Centralised here so that scope-aware decrypt paths share the same hex
/// parsing and error wrapping.
fn decrypt_value_with_unseal_key(hex_ciphertext: &str, unseal_key: &[u8]) -> Result<String> {
    let ciphertext =
        hex::decode(hex_ciphertext).context("failed to decode encrypted config value hex")?;
    decrypt_token(unseal_key, &ciphertext)
}

/// Encrypt a value using the vault unseal key for the given scope.
/// Returns the hex-encoded ciphertext.
///
/// Used by `libra config set`/`add` when the key is sensitive
/// (see [`is_sensitive_key`]) or `--encrypted` was passed.
pub async fn encrypt_value(value: &str, scope: &str) -> Result<String> {
    let unseal_key = load_unseal_key_for_scope(scope)
        .await
        .ok_or_else(|| anyhow!("vault not initialized for {scope} scope — cannot encrypt value"))?;
    let ciphertext = encrypt_token(&unseal_key, value.as_bytes())?;
    Ok(hex::encode(ciphertext))
}

/// Resolve an environment variable by priority chain.
///
/// Functional scope:
/// 1. System environment variable (`std::env::var`)
/// 2. Local config (`vault.env.<name>` in `.libra/libra.db`)
/// 3. Global config (`vault.env.<name>` in `~/.libra/config.db`)
///
/// Boundary conditions:
/// - `name` is the raw env var name (e.g. `"GEMINI_API_KEY"`).
/// - Returns `Ok(None)` only when *all three* sources are exhausted.
/// - Returns `Err` if a vault/DB query fails (a hard error — not the same
///   as "not configured").
pub async fn resolve_env(name: &str) -> Result<Option<String>> {
    resolve_env_for_target(name, LocalIdentityTarget::CurrentRepo).await
}

/// Synchronous wrapper around [`resolve_env`] for call sites that cannot become
/// async (e.g. sync constructors inside otherwise-async pipelines, or
/// closures threaded through `Fn(&str) -> Option<String>` lookup helpers).
///
/// Functional scope:
/// - Checks `std::env::var(name)` first — the common fast path that does not
///   need a tokio runtime.
/// - When the env var is unset, spawns a private std-thread that owns a
///   single-purpose tokio runtime, drives the async [`resolve_env_for_target`]
///   call against [`LocalIdentityTarget::CurrentRepo`], and returns the
///   resolved value to the caller. This mirrors the pattern in
///   `src/utils/client_storage.rs::resolve_env_sync` and is intentionally
///   isolated from any caller-owned tokio runtime.
///
/// Returns `Ok(None)` only when the process env, the local repo's
/// `.libra/libra.db`, and the global `~/.libra/config.db` all lack the value.
/// Returns `Err` when the worker thread crashed before sending OR when the
/// underlying async resolver returned an error (e.g. corrupt SQLite,
/// schema-mismatch propagation that the vault-init fix in v0.17.515 did not
/// downgrade — those still bubble up here so storage / provider init paths
/// can surface "Run `libra db upgrade`" hints rather than silently treating a
/// vault-configured key as missing).
///
/// Prefer the async [`resolve_env`] when the caller is already inside an
/// async context — that avoids the per-call thread spawn.
pub fn resolve_env_sync(name: &str) -> anyhow::Result<Option<String>> {
    if let Ok(val) = std::env::var(name) {
        return Ok(Some(val));
    }

    let owned = name.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<Option<String>> {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|err| anyhow::anyhow!("failed to create tokio runtime: {err}"))?;
            runtime.block_on(resolve_env_for_target(
                &owned,
                LocalIdentityTarget::CurrentRepo,
            ))
        })();
        let _ = tx.send(result);
    });
    rx.recv()
        .map_err(|_| anyhow::anyhow!("resolve_env_sync worker for '{name}' exited unexpectedly"))?
}

/// Required-value wrapper over [`resolve_env_sync`]: returns `Ok(value)`
/// when the variable is set in the process env, the local repo's
/// `.libra/libra.db`, or the global `~/.libra/config.db`, and a single
/// actionable error otherwise. Provider clients use this for the
/// API-key class of variables where missing means the provider cannot
/// initialise.
pub fn resolve_required_env_sync(name: &str) -> anyhow::Result<String> {
    match resolve_env_sync(name)? {
        Some(value) => Ok(value),
        None => Err(anyhow::anyhow!(
            "environment variable `{name}` is not set — export it or store it in libra config (`libra config set vault.env.{name} <value>`)"
        )),
    }
}

/// Optional-value wrapper over [`resolve_env_sync`]. Identical to
/// [`resolve_env_sync`]; provided as a named alias so callers can
/// document at the call site that the variable is optional and
/// `Ok(None)` is the success path.
pub fn resolve_optional_env_sync(name: &str) -> anyhow::Result<Option<String>> {
    resolve_env_sync(name)
}

/// Resolve an environment variable using an explicit local config target.
///
/// Same priority chain as [`resolve_env`] but lets callers point at a
/// non-default repo (e.g. when running `libra config --file ...`). The local
/// scope can also be skipped entirely with [`LocalIdentityTarget::None`].
pub async fn resolve_env_for_target(
    name: &str,
    local_target: LocalIdentityTarget<'_>,
) -> Result<Option<String>> {
    // 1. System environment variable — per-process override (12-Factor)
    if let Ok(val) = std::env::var(name) {
        return Ok(Some(val));
    }

    let vault_key = format!("vault.env.{name}");

    // 2. Local config (vault.env.*)
    if let Some(value) = local_env_value_for_target(local_target, &vault_key).await? {
        return Ok(Some(value));
    }

    // 3. Global config — lowest priority
    global_env_value(name, &vault_key).await
}

/// Resolve the global config database path.
///
/// Boundary conditions:
/// - `LIBRA_CONFIG_GLOBAL_DB` env var wins (used by integration tests to
///   sandbox a global config without touching `$HOME`).
/// - Falls back to `~/.libra/config.db`. Returns `None` if no home directory
///   can be discovered (rare, but possible inside containers).
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
    /// `user.name` from local-then-global config (encrypted values are
    /// transparently decrypted before populating this field).
    pub config_name: Option<String>,
    /// `user.email` from local-then-global config.
    pub config_email: Option<String>,
    /// First non-empty value from the env var list (`GIT_COMMITTER_NAME`,
    /// `GIT_AUTHOR_NAME`, `LIBRA_COMMITTER_NAME`).
    pub env_name: Option<String>,
    /// First non-empty value from the env var list (`GIT_COMMITTER_EMAIL`,
    /// `GIT_AUTHOR_EMAIL`, `EMAIL`, `LIBRA_COMMITTER_EMAIL`).
    pub env_email: Option<String>,
}

/// Which local repository, if any, should participate in config resolution.
///
/// Used as a parameter to [`resolve_env_for_target`] and friends so callers
/// can bypass the implicit "discover from cwd" lookup when needed (tests,
/// `--file path` flags).
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
///
/// Whitespace-only values are treated as empty so users can clear an env
/// var by setting it to a single space.
pub fn env_first_non_empty(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

/// Read a config value for the given target using local-first, then global.
///
/// Encrypted values are transparently decrypted via the appropriate vault.
/// Returns `Ok(None)` when both local and global are absent or empty.
pub async fn read_cascaded_config_value(
    local_target: LocalIdentityTarget<'_>,
    key: &str,
) -> Result<Option<String>> {
    if let Some(value) = local_config_value_for_target(local_target, key).await? {
        return Ok(Some(value));
    }
    global_config_value(key).await
}

/// Read a config value for the given target using local-first, then global, and
/// decrypt encrypted entries with the matching vault.
///
/// Use this for non-env config keys whose names still trigger sensitive-key
/// encryption, for example credential/profile selectors that are stored through
/// `libra config set`.
pub async fn read_cascaded_config_value_decrypted(
    local_target: LocalIdentityTarget<'_>,
    key: &str,
) -> Result<Option<String>> {
    if let Some(value) = local_config_decrypted_value_for_target(local_target, key).await? {
        return Ok(Some(value));
    }
    global_config_decrypted_value(key).await
}

async fn local_config_decrypted_value_for_target(
    local_target: LocalIdentityTarget<'_>,
    key: &str,
) -> Result<Option<String>> {
    let Some(entry) = local_config_entry_for_target(local_target, key).await? else {
        return Ok(None);
    };

    let value = if entry.encrypted {
        decrypt_value_for_local_target(&entry.value, local_target)
            .await
            .context(format!("failed to decrypt {key} from local config"))?
    } else {
        entry.value
    };
    Ok(trim_non_empty_config_value(value))
}

async fn global_config_decrypted_value(key: &str) -> Result<Option<String>> {
    let Some(db_path) = global_config_path() else {
        return Ok(None);
    };
    if !db_path.exists() {
        return Ok(None);
    }

    let Some(entry) = read_config_entry_from_db_path(&db_path, key).await? else {
        return Ok(None);
    };
    let value = if entry.encrypted {
        decrypt_value(&entry.value, "global")
            .await
            .context(format!("failed to decrypt {key} from global config"))?
    } else {
        entry.value
    };
    Ok(trim_non_empty_config_value(value))
}

fn trim_non_empty_config_value(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Resolve user identity values from config and environment while preserving
/// the source boundary between the two.
///
/// The returned [`UserIdentitySources`] keeps config-derived and env-derived
/// values in separate fields so callers (notably `libra commit`) can apply
/// `user.useConfigOnly` semantics — refusing to fall back to env vars when
/// the user has explicitly opted into config-only identity.
///
/// Failures while reading the config DB (missing file, stale schema, locked
/// SQLite) are downgraded to `tracing::warn!` + `None` rather than hard
/// errors. Identity is auxiliary at vault-init time (the caller falls back
/// to env vars or hard-coded defaults), and at `commit` time the missing
/// value still surfaces as a clear `IdentityMissing` error — so a corrupted
/// `~/.libra/config.db` no longer blocks `libra init` / `libra clone`.
pub async fn resolve_user_identity_sources(
    local_target: LocalIdentityTarget<'_>,
) -> Result<UserIdentitySources> {
    Ok(UserIdentitySources {
        config_name: read_identity_field_with_warning(local_target, "user.name").await,
        config_email: read_identity_field_with_warning(local_target, "user.email").await,
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

async fn read_identity_field_with_warning(
    local_target: LocalIdentityTarget<'_>,
    key: &str,
) -> Option<String> {
    match read_cascaded_config_value(local_target, key).await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(
                key = key,
                error = %format!("{error:#}"),
                "failed to read identity field from config; treating as unset"
            );
            None
        }
    }
}

/// Read a `vault.env.*` entry from the local target, decrypting if needed.
///
/// Boundary condition: encrypted entries with no available unseal key
/// produce `Err`. A missing row produces `Ok(None)`.
async fn local_env_value_for_target(
    local_target: LocalIdentityTarget<'_>,
    vault_key: &str,
) -> Result<Option<String>> {
    let Some(entry) = local_config_entry_for_target(local_target, vault_key).await? else {
        return Ok(None);
    };

    if entry.encrypted {
        let plaintext = decrypt_value_for_local_target(&entry.value, local_target)
            .await
            .context(format!("failed to decrypt {vault_key}"))?;
        return Ok(Some(plaintext));
    }

    Ok(Some(entry.value))
}

/// Resolve the storage path for the given local target and read a single key.
///
/// Returns `Ok(None)` when the target's `.libra/libra.db` does not exist
/// (pre-init repos) or [`LocalIdentityTarget::None`] is selected.
async fn local_config_entry_for_target(
    local_target: LocalIdentityTarget<'_>,
    key: &str,
) -> Result<Option<ConfigKvEntry>> {
    match local_target {
        LocalIdentityTarget::CurrentRepo => {
            let storage = crate::utils::util::try_get_storage_path(None)
                .context("failed to resolve current repository storage")?;
            let db_path = storage.join(crate::utils::util::DATABASE);
            read_config_entry_from_db_path(&db_path, key).await
        }
        LocalIdentityTarget::ExplicitDb(db_path) => {
            read_config_entry_from_db_path(db_path, key).await
        }
        LocalIdentityTarget::None => Ok(None),
    }
}

/// Look up a `vault.env.<name>` value from the global config DB.
///
/// Returns `Ok(None)` if the global DB does not exist (user has never
/// configured global settings). Otherwise behaves like
/// [`local_env_value_for_target`].
async fn global_env_value(name: &str, vault_key: &str) -> Result<Option<String>> {
    let Some(global_path) = global_config_path() else {
        return Ok(None);
    };
    if !global_path.exists() {
        return Ok(None);
    }

    let Some(entry) = read_config_entry_from_db_path(&global_path, vault_key).await? else {
        return Ok(None);
    };

    if entry.encrypted {
        let plaintext = decrypt_value(&entry.value, "global")
            .await
            .context(format!(
                "failed to decrypt vault.env.{name} from global config"
            ))?;
        return Ok(Some(plaintext));
    }

    Ok(Some(entry.value))
}

/// Read a (non-vault) config value scoped to the given local target.
///
/// Used by [`read_cascaded_config_value`]; differs from
/// [`local_env_value_for_target`] in that it skips vault decryption and
/// trims whitespace-only values to `None`.
async fn local_config_value_for_target(
    local_target: LocalIdentityTarget<'_>,
    key: &str,
) -> Result<Option<String>> {
    match local_target {
        LocalIdentityTarget::CurrentRepo => {
            let storage = try_get_storage_path(None)
                .context("failed to resolve current repository storage")?;
            let db_path = storage.join(DATABASE);
            read_config_value_from_db_path(&db_path, key).await
        }
        LocalIdentityTarget::ExplicitDb(db_path) => {
            read_config_value_from_db_path(db_path, key).await
        }
        LocalIdentityTarget::None => Ok(None),
    }
}

/// Read a single key from the global config DB, returning `Ok(None)` if no
/// global DB exists or the key is missing.
async fn global_config_value(key: &str) -> Result<Option<String>> {
    let Some(db_path) = global_config_path() else {
        return Ok(None);
    };
    if !db_path.exists() {
        return Ok(None);
    }
    read_config_value_from_db_path(&db_path, key).await
}

/// Read a config value from `db_path`, trimming whitespace and treating empty
/// strings as missing. Used for non-vault keys where surrounding whitespace
/// is almost certainly a typo.
async fn read_config_value_from_db_path(db_path: &Path, key: &str) -> Result<Option<String>> {
    let entry = read_config_entry_from_db_path(db_path, key).await?;
    Ok(entry.and_then(|entry| {
        let trimmed = entry.value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }))
}

/// Open the SQLite DB at `db_path` and read a single `config_kv` entry.
///
/// Returns `Ok(None)` when the file does not exist (so callers can probe
/// optional config locations cheaply). Errors are wrapped with the path so
/// the user can diagnose `permission denied`/`schema mismatch` problems.
async fn read_config_entry_from_db_path(
    db_path: &Path,
    key: &str,
) -> Result<Option<ConfigKvEntry>> {
    if !db_path.exists() {
        return Ok(None);
    }

    let conn = get_db_conn_instance_for_path(db_path)
        .await
        .with_context(|| format!("failed to open config database '{}'", db_path.display()))?;
    ConfigKv::get_with_conn(&conn, key).await.with_context(|| {
        format!(
            "failed to query '{key}' from config database '{}'",
            db_path.display()
        )
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Sensitive key detection
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` if the key holds sensitive material that should be
/// encrypted and redacted by default.
///
/// Detection rules (applied case-insensitively):
/// 1. `vault.env.*` — every entry under the env vault namespace.
/// 2. Anything ending in `.privkey` — SSH/PGP private keys.
/// 3. Hardcoded vault internals (`vault.unsealkey`, `vault.roottoken`).
/// 4. Substring match on the *last* dotted segment (after stripping `_`/`-`):
///    `secret`, `token`, `password`, `credential`, `privatekey`, `accesskey`,
///    `apikey`, `secretkey`.
/// 5. Explicit exemption: keys ending in `pubkey` / `publickey` are treated
///    as non-sensitive even though they contain `key`.
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
///
/// Vault internals (unseal key, root token, repo private key) must remain
/// encrypted at all times. The CLI consults this predicate before honouring
/// `--reveal` or `--plaintext` flags.
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
//
// The methods below are retained for backwards compatibility with the original
// three-column `config` table. New code should use [`ConfigKv`] instead, which
// supports encryption and richer multi-value semantics.
//
// Many of these legacy helpers `unwrap()` on storage errors. That's deliberate
// for the deprecation period: once a migration is complete the table will be
// dropped, and surfacing failures loudly is preferable to silent fallback.

/// Marker type for the deprecated three-column config API. Use [`ConfigKv`].
#[deprecated(note = "use ConfigKv instead")]
pub struct Config;

/// Internal helper: lets us treat both `DatabaseConnection` and
/// `&DatabaseConnection` uniformly when wiring legacy `Config::*` methods.
/// Avoids extra clones inside the deprecated layer.
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

/// Resolved view of a `remote.<name>.*` section.
///
/// Carries only the bare minimum needed by `push`/`fetch`/`clone` flows; the
/// raw URL is whatever the user typed (no scheme normalisation).
#[derive(Clone, Debug)]
pub struct RemoteConfig {
    /// Remote alias, e.g. `origin`.
    pub name: String,
    /// Fetch URL exactly as configured.
    pub url: String,
}
/// Resolved view of `branch.<name>.{remote,merge}` for upstream tracking.
///
/// `merge` is normalised to a short branch name (no `refs/heads/` prefix).
#[allow(dead_code)]
pub struct BranchConfig {
    /// Local branch name.
    pub name: String,
    /// Upstream branch name (e.g. `main`), already stripped of `refs/heads/`.
    pub merge: String,
    /// Upstream remote alias (e.g. `origin`).
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
    /// Insert a row into the legacy `config` table without checking for
    /// existing entries. Panics on storage errors — this is the deprecated
    /// path; new code should call [`ConfigKv::add`] / [`ConfigKv::set`].
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
        // INVARIANT (deprecated lossy API): storage failures here are
        // unrecoverable for this legacy path. ConfigKv::add / ConfigKv::set
        // surface the same failure as a typed error.
        config
            .save(db)
            .await
            .expect("legacy Config::insert_with_conn: DB save failed");
    }

    /// Update an existing config row's value. Panics if no matching row
    /// exists. Deprecated; prefer [`ConfigKv::set`].
    pub async fn update_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
        value: &str,
    ) -> Model {
        // INVARIANT (deprecated lossy API): callers must have verified the
        // (configuration, name, key) tuple exists before calling. The
        // SeaORM `find().one()` returns `Result<Option<Model>, DbErr>`, so
        // the outer .expect() surfaces query failures and the inner
        // .expect() surfaces the missing-row case. Both are unrecoverable
        // for this legacy path; ConfigKv::set replaces the whole sequence
        // with an upsert and explicit errors.
        let mut config: ActiveModel = config::Entity::find()
            .filter(config::Column::Configuration.eq(configuration))
            .filter(match name {
                Some(str) => config::Column::Name.eq(str),
                None => config::Column::Name.is_null(),
            })
            .filter(config::Column::Key.eq(key))
            .one(db)
            .await
            .expect("legacy Config::update_with_conn: DB query failed")
            .expect("legacy Config::update_with_conn: target config row missing (use ConfigKv::set for upsert semantics)")
            .into();
        config.value = Set(value.to_owned());
        config
            .update(db)
            .await
            .expect("legacy Config::update_with_conn: DB update failed")
    }

    /// Internal: list every legacy row matching `(configuration, name, key)`.
    /// Used by `get*`/`get_all*` and the delete pipeline.
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
            .expect("legacy Config::query_with_conn: DB query failed")
    }

    /// Get the first matching value (insertion order). Returns `None` for
    /// missing keys. Deprecated; prefer [`ConfigKv::get`].
    pub async fn get_with_conn<C: ConnectionTrait>(
        db: &C,
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Option<String> {
        let values = Self::query_with_conn(db, configuration, name, key).await;
        values.first().map(|c| c.value.to_owned())
    }

    /// Legacy `branch.<branch>.remote` lookup. Deprecated;
    /// prefer [`ConfigKv::get_remote_with_conn`].
    pub async fn get_remote_with_conn<C: ConnectionTrait>(db: &C, branch: &str) -> Option<String> {
        Config::get_with_conn(db, "branch", Some(branch), "remote").await
    }

    /// Legacy upstream-remote lookup. Returns `Err(())` (note: unit error,
    /// not anyhow) when HEAD is detached. Deprecated; prefer
    /// [`ConfigKv::get_current_remote_with_conn`].
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

    /// Legacy fetch-URL lookup. **Panics** when the URL is missing — this
    /// pre-dates the structured error path and is preserved for compatibility
    /// only. Deprecated; prefer [`ConfigKv::get_remote_url_with_conn`].
    pub async fn get_remote_url_with_conn<C: ConnectionTrait>(db: &C, remote: &str) -> String {
        match Config::get_with_conn(db, "remote", Some(remote), "url").await {
            Some(url) => url,
            None => panic!("fatal: No URL configured for remote '{remote}'."),
        }
    }

    /// Legacy "URL of the current branch's upstream" lookup.
    pub async fn get_current_remote_url_with_conn<C: ConnectionTrait>(db: &C) -> Option<String> {
        // INVARIANT (deprecated lossy API): `get_current_remote_with_conn`
        // returns Err(()) only when HEAD is detached, after already
        // printing a `fatal: HEAD is detached, cannot get remote` message
        // to stderr. The legacy contract is to panic in that case rather
        // than silently treat it as "no remote"; callers that need
        // graceful handling should use `ConfigKv::get_current_remote_url_with_conn`.
        match Config::get_current_remote_with_conn(db)
            .await
            .expect("legacy Config::get_current_remote_url_with_conn: HEAD is detached")
        {
            Some(remote) => Some(Config::get_remote_url_with_conn(db, &remote).await),
            None => None,
        }
    }

    /// Legacy multi-value getter. Returns every `value` for the matching
    /// triple in insertion order. Deprecated.
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

    /// Legacy `git config --list` equivalent: emits `(dotted_key, value)`
    /// pairs for every row in the table. Deprecated.
    pub async fn list_all_with_conn<C: ConnectionTrait>(db: &C) -> Vec<(String, String)> {
        config::Entity::find()
            .all(db)
            .await
            .expect("legacy Config::list_all_with_conn: DB query failed")
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

    /// Delete one or all matching legacy config rows.
    ///
    /// Boundary conditions:
    /// - `valuepattern` filters by substring match against the row's value.
    /// - `delete_all = false` stops after the first deletion (mirrors
    ///   `git config --unset`).
    /// - Returns the underlying `DbErr` on failure; rows already deleted
    ///   before the failure remain deleted (no implicit transaction).
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

    /// Legacy "remove every `remote.<name>.*` row" helper. Returns
    /// `Err(String)` (note: not anyhow) when the remote does not exist.
    pub async fn remove_remote_with_conn<C: ConnectionTrait>(
        db: &C,
        name: &str,
    ) -> Result<(), String> {
        let remote = config::Entity::find()
            .filter(config::Column::Configuration.eq("remote"))
            .filter(config::Column::Name.eq(name))
            .all(db)
            .await
            .expect("legacy Config::remove_remote_with_conn: DB query failed");
        if remote.is_empty() {
            return Err(format!("fatal: No such remote: {name}"));
        }
        for r in remote {
            let r: ActiveModel = r.into();
            r.delete(db)
                .await
                .expect("legacy Config::remove_remote_with_conn: DB delete failed");
        }
        Ok(())
    }

    /// Legacy remote-rename helper. Performs the same cascade as
    /// [`ConfigKv::rename_remote_with_conn`] but without the SSH key
    /// rewrite (the legacy table has no vault namespace).
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
            .expect("legacy Config::rename_remote_with_conn: DB query failed");

        // Update remote.<name>.* entries to point at the new name.
        for entry in remote_entries {
            let mut active: ActiveModel = entry.into();
            active.name = Set(Some(new.to_owned()));
            active
                .update(db)
                .await
                .expect("legacy Config::rename_remote_with_conn: DB update failed");
        }

        let branch_entries = config::Entity::find()
            .filter(config::Column::Configuration.eq("branch"))
            .filter(config::Column::Key.eq("remote"))
            .filter(config::Column::Value.eq(old))
            .all(db)
            .await
            .expect("legacy Config::rename_remote_with_conn: DB query failed");

        // Repoint branch.*.remote values that referenced the old remote.
        for entry in branch_entries {
            let mut active: ActiveModel = entry.into();
            active.value = Set(new.to_owned());
            active
                .update(db)
                .await
                .expect("legacy Config::rename_remote_with_conn: DB update failed");
        }

        Ok(())
    }

    /// Legacy "list every remote" helper. Deprecated; prefer
    /// [`ConfigKv::all_remote_configs_with_conn`].
    pub async fn all_remote_configs_with_conn<C: ConnectionTrait>(db: &C) -> Vec<RemoteConfig> {
        let remotes = config::Entity::find()
            .filter(config::Column::Configuration.eq("remote"))
            .all(db)
            .await
            .expect("legacy Config::all_remote_configs_with_conn: DB query failed");
        // INVARIANT: rows with configuration='remote' always have a non-NULL
        // `name` column (the remote name itself is required by every Libra
        // write path). External tampering could violate this, in which case
        // the deprecated lossy API panics; ConfigKv::all_remote_configs_with_conn
        // surfaces the same condition as a typed error.
        let remote_names = remotes
            .iter()
            .map(|remote| {
                remote
                    .name
                    .as_ref()
                    .expect("legacy remote row missing 'name' column")
                    .clone()
            })
            .collect::<HashSet<String>>();

        remote_names
            .iter()
            .map(|name| {
                let url = remotes
                    .iter()
                    .find(|remote| {
                        remote
                            .name
                            .as_ref()
                            .expect("legacy remote row missing 'name' column")
                            == name
                    })
                    .expect("remote_names was built from the same `remotes` slice; name must match")
                    .value
                    .to_owned();
                RemoteConfig {
                    name: name.to_owned(),
                    url,
                }
            })
            .collect()
    }

    /// Legacy single-remote lookup. Returns `None` when missing.
    pub async fn remote_config_with_conn<C: ConnectionTrait>(
        db: &C,
        name: &str,
    ) -> Option<RemoteConfig> {
        let remote = config::Entity::find()
            .filter(config::Column::Configuration.eq("remote"))
            .filter(config::Column::Name.eq(name))
            .one(db)
            .await
            .expect("legacy Config::remote_config_with_conn: DB query failed");
        remote.map(|r| RemoteConfig {
            // INVARIANT: matched by `Column::Name.eq(name)` above; the row's
            // `name` column is guaranteed non-NULL.
            name: r.name.expect("legacy remote row missing 'name' column"),
            url: r.value,
        })
    }

    /// Legacy branch-tracking lookup.
    ///
    /// Boundary conditions:
    /// - Returns `None` when the branch has no rows in the legacy table.
    /// - Asserts there are exactly two rows (`merge` + `remote`). Earlier
    ///   versions of Libra always wrote both together; a different count
    ///   indicates external tampering.
    /// - The `merge` field is normalised by stripping `refs/heads/` (the
    ///   leading 11 bytes); see the `[11..]` slice below.
    pub async fn branch_config_with_conn<C: ConnectionTrait>(
        db: &C,
        name: &str,
    ) -> Option<BranchConfig> {
        let config_entries = config::Entity::find()
            .filter(config::Column::Configuration.eq("branch"))
            .filter(config::Column::Name.eq(name))
            .all(db)
            .await
            .expect("legacy Config::branch_config_with_conn: DB query failed");
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

    /// Pool-acquiring counterpart of [`Self::insert_with_conn`]. Deprecated.
    pub async fn insert(configuration: &str, name: Option<&str>, key: &str, value: &str) {
        let db = get_db_conn_instance().await;
        Self::insert_with_conn(&db, configuration, name, key, value).await;
    }

    /// Update one configuration entry in database using given configuration, name, key and value.
    pub async fn update(configuration: &str, name: Option<&str>, key: &str, value: &str) -> Model {
        let db = get_db_conn_instance().await;
        Self::update_with_conn(&db, configuration, name, key, value).await
    }

    /// Get one configuration value (legacy table). Deprecated.
    pub async fn get(configuration: &str, name: Option<&str>, key: &str) -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_with_conn(&db, configuration, name, key).await
    }

    /// Get remote repo name by branch name (legacy).
    /// - Returns `None` when `branch.<name>.remote` is unset; callers usually
    ///   need to `branch --set-upstream` first.
    pub async fn get_remote(branch: &str) -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_remote_with_conn(&db, branch).await
    }

    /// Get remote repo name of current branch (legacy).
    /// Returns `Err(())` when HEAD is detached.
    pub async fn get_current_remote() -> Result<Option<String>, ()> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_with_conn(&db).await
    }

    /// Pool-acquiring counterpart of [`Self::get_remote_url_with_conn`].
    /// Panics when no URL is configured (legacy behaviour).
    pub async fn get_remote_url(remote: &str) -> String {
        let db = get_db_conn_instance().await;
        Self::get_remote_url_with_conn(&db, remote).await
    }

    /// Returns `None` if no remote is set on the current branch.
    pub async fn get_current_remote_url() -> Option<String> {
        let db = get_db_conn_instance().await;
        Self::get_current_remote_url_with_conn(&db).await
    }

    /// Get all configuration values (legacy multi-value reader).
    /// e.g. `remote.origin.fetch` may have multiple entries.
    pub async fn get_all(configuration: &str, name: Option<&str>, key: &str) -> Vec<String> {
        let db = get_db_conn_instance().await;
        Self::get_all_with_conn(&db, configuration, name, key).await
    }

    /// Get literally all the entries in database without any filtering.
    pub async fn list_all() -> Vec<(String, String)> {
        let db = get_db_conn_instance().await;
        Self::list_all_with_conn(&db).await
    }

    /// Delete one or all configuration entries using given key and value pattern.
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

    /// Remove every row matching the given `(configuration, name, key)` triple.
    pub async fn remove(
        configuration: &str,
        name: Option<&str>,
        key: &str,
    ) -> Result<(), sea_orm::DbErr> {
        Self::remove_config(configuration, name, key, None, true).await
    }

    // NOTE: `remove_by_section` was once contemplated as a `--remove-section`
    // implementation but never landed; new section-wide deletion goes through
    // [`ConfigKv::get_by_prefix`] + per-row delete.

    /// Pool-acquiring counterpart of [`Self::remove_remote_with_conn`].
    pub async fn remove_remote(name: &str) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::remove_remote_with_conn(&db, name).await
    }

    /// Pool-acquiring counterpart of [`Self::rename_remote_with_conn`].
    pub async fn rename_remote(old: &str, new: &str) -> Result<(), String> {
        let db = get_db_conn_instance().await;
        Self::rename_remote_with_conn(&db, old, new).await
    }

    /// Pool-acquiring counterpart of [`Self::all_remote_configs_with_conn`].
    pub async fn all_remote_configs() -> Vec<RemoteConfig> {
        let db = get_db_conn_instance().await;
        Self::all_remote_configs_with_conn(&db).await
    }

    /// Pool-acquiring counterpart of [`Self::remote_config_with_conn`].
    pub async fn remote_config(name: &str) -> Option<RemoteConfig> {
        let db = get_db_conn_instance().await;
        Self::remote_config_with_conn(&db, name).await
    }

    /// Pool-acquiring counterpart of [`Self::branch_config_with_conn`].
    pub async fn branch_config(name: &str) -> Option<BranchConfig> {
        let db = get_db_conn_instance().await;
        Self::branch_config_with_conn(&db, name).await
    }
}

#[cfg(test)]
mod config_kv_tests {
    use sea_orm::TransactionTrait;
    use tempfile::TempDir;

    use super::*;
    use crate::internal::db::create_database;

    /// Create a fresh, isolated `config_kv`-backed SQLite database. The returned
    /// `TempDir` must be kept alive for the duration of the test.
    async fn new_kv_db() -> (TempDir, DatabaseConnection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config_kv_test.db");
        let conn = create_database(&path.to_string_lossy())
            .await
            .expect("create test config db");
        (dir, conn)
    }

    // ── matches_value_filter (pure) ─────────────────────────────────────

    #[test]
    fn value_filter_regex_substring_and_anchors() {
        assert!(matches_value_filter("axb", "a.b", false, false).unwrap());
        assert!(matches_value_filter("main-2", "^main", false, false).unwrap());
        assert!(!matches_value_filter("dev", "^main$", false, false).unwrap());
    }

    #[test]
    fn value_filter_fixed_is_literal_equality() {
        // `a.b` literal matches only the exact value, not `axb`.
        assert!(matches_value_filter("a.b", "a.b", true, false).unwrap());
        assert!(!matches_value_filter("axb", "a.b", true, false).unwrap());
        // A regex metacharacter like `[` is a plain literal under fixed mode.
        assert!(matches_value_filter("[", "[", true, false).unwrap());
    }

    #[test]
    fn value_filter_negation_and_fixed_bang_literal() {
        // Non-fixed `!` negates the trailing regex.
        assert!(matches_value_filter("drop", "!^keep$", false, false).unwrap());
        assert!(!matches_value_filter("keep", "!^keep$", false, false).unwrap());
        // `!` with an empty body matches everything, so negation matches nothing.
        assert!(!matches_value_filter("anything", "!", false, false).unwrap());
        // Under fixed mode the leading `!` is an ordinary character.
        assert!(matches_value_filter("!main", "!main", true, false).unwrap());
        assert!(!matches_value_filter("main", "!main", true, false).unwrap());
    }

    #[test]
    fn value_filter_ignore_case_affects_regex_only() {
        assert!(matches_value_filter("MAIN", "^main$", false, true).unwrap());
        // Fixed mode ignores `ignore_case`: literal stays case-sensitive.
        assert!(!matches_value_filter("main", "MAIN", true, true).unwrap());
    }

    #[test]
    fn value_filter_rejects_invalid_and_overlong_regex() {
        assert!(matches_value_filter("x", "[", false, false).is_err());
        let long = "a".repeat(MAX_CONFIG_REGEX_LEN + 1);
        assert!(ValueFilter::compile(&long, false, false).is_err());
        // Over-long is fine in fixed mode (no regex compiled).
        assert!(ValueFilter::compile(&long, true, false).is_ok());
    }

    // ── validate_key_syntax ─────────────────────────────────────────────

    #[test]
    fn key_syntax_accepts_real_non_classic_keys() {
        for key in [
            "user.name",
            "vault.env.GEMINI_API_KEY",
            "vault.env.LIBRA_D1_ACCOUNT_ID",
            "cloud.clone_domains.example.com.account_id",
            "core.bigFileThreshold",
            "user.useConfigOnly",
            "custom.api_token",
            "sec.key.123",
            "vault.ssh.origin.privkey",
        ] {
            assert!(validate_key_syntax(key).is_ok(), "should accept '{key}'");
        }
    }

    #[test]
    fn key_syntax_rejects_malformations() {
        for key in [
            "invalid_key", // no dot
            ".foo",        // leading dot
            "foo.",        // trailing dot
            "a..b",        // empty component
            "a b.c",       // whitespace
            "a.\tc",       // control/whitespace
            "a.\u{0}b",    // NUL
        ] {
            assert!(validate_key_syntax(key).is_err(), "should reject '{key:?}'");
        }
    }

    // ── value-filtered mutations (transaction-safe, by row id) ──────────

    #[tokio::test]
    async fn replace_matching_enforce_single_leaves_source_unchanged() {
        let (_dir, db) = new_kv_db().await;
        ConfigKv::add_with_conn(&db, "remote.origin.fetch", "main", false)
            .await
            .unwrap();
        ConfigKv::add_with_conn(&db, "remote.origin.fetch", "dev", false)
            .await
            .unwrap();

        // A default (enforce_single) replace over a 2-value match set must fail
        // without mutating anything.
        let txn = db.begin().await.unwrap();
        let err = ConfigKv::replace_matching_with_conn(
            &txn,
            "remote.origin.fetch",
            "NEW",
            false,
            None,
            true,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("values exist"), "got: {err}");
        drop(txn); // rollback

        let rows = ConfigKv::get_all_with_conn(&db, "remote.origin.fetch")
            .await
            .unwrap();
        assert_eq!(
            rows.len(),
            2,
            "source values must be unchanged after conflict"
        );
    }

    #[tokio::test]
    async fn replace_matching_all_collapses_to_single_value() {
        let (_dir, db) = new_kv_db().await;
        for v in ["main", "dev", "main"] {
            ConfigKv::add_with_conn(&db, "remote.origin.fetch", v, false)
                .await
                .unwrap();
        }
        let txn = db.begin().await.unwrap();
        let outcome = ConfigKv::replace_matching_with_conn(
            &txn,
            "remote.origin.fetch",
            "NEW",
            false,
            None,
            false,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        assert_eq!(outcome.matched, 3);
        let rows = ConfigKv::get_all_with_conn(&db, "remote.origin.fetch")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value, "NEW");
    }

    #[tokio::test]
    async fn replace_matching_filter_no_match_inserts_new() {
        let (_dir, db) = new_kv_db().await;
        ConfigKv::add_with_conn(&db, "remote.origin.fetch", "main", false)
            .await
            .unwrap();
        let filter = ValueFilter::compile("^missing$", false, false).unwrap();
        let txn = db.begin().await.unwrap();
        let outcome = ConfigKv::replace_matching_with_conn(
            &txn,
            "remote.origin.fetch",
            "NEW",
            false,
            Some(&filter),
            false,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        assert_eq!(outcome.matched, 0);
        let mut values: Vec<String> = ConfigKv::get_all_with_conn(&db, "remote.origin.fetch")
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.value)
            .collect();
        values.sort();
        assert_eq!(values, vec!["NEW".to_string(), "main".to_string()]);
    }

    #[tokio::test]
    async fn unset_matching_removes_only_matching_rows() {
        let (_dir, db) = new_kv_db().await;
        for v in ["keep", "drop1", "drop2"] {
            ConfigKv::add_with_conn(&db, "remote.origin.fetch", v, false)
                .await
                .unwrap();
        }
        // Remove everything that is NOT `keep`.
        let filter = ValueFilter::compile("!^keep$", false, false).unwrap();
        let txn = db.begin().await.unwrap();
        let removed =
            ConfigKv::unset_matching_with_conn(&txn, "remote.origin.fetch", &filter, false)
                .await
                .unwrap();
        txn.commit().await.unwrap();
        assert_eq!(removed, 2);
        let rows = ConfigKv::get_all_with_conn(&db, "remote.origin.fetch")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value, "keep");
    }

    #[tokio::test]
    async fn value_filter_matches_ciphertext_for_encrypted_rows() {
        // Sensitive/encrypted rows store hex ciphertext; the filter runs against
        // that stored form, never the plaintext, when not revealed.
        let (_dir, db) = new_kv_db().await;
        ConfigKv::add_with_conn(&db, "vault.env.SECRET", "deadbeef", true)
            .await
            .unwrap();
        let filter = ValueFilter::compile("^deadbeef$", false, false).unwrap();
        let txn = db.begin().await.unwrap();
        let removed = ConfigKv::unset_matching_with_conn(&txn, "vault.env.SECRET", &filter, true)
            .await
            .unwrap();
        txn.commit().await.unwrap();
        assert_eq!(removed, 1, "filter should match on stored ciphertext");
    }

    // ── section operations ──────────────────────────────────────────────

    #[tokio::test]
    async fn rename_section_moves_dotted_prefix_only() {
        let (_dir, db) = new_kv_db().await;
        ConfigKv::add_with_conn(&db, "remote.origin.url", "ssh://x", false)
            .await
            .unwrap();
        ConfigKv::add_with_conn(&db, "remote.origin.fetch", "+a", false)
            .await
            .unwrap();
        // Decoy that shares a prefix substring but not the dotted boundary.
        ConfigKv::add_with_conn(&db, "remote.originator.url", "keep", false)
            .await
            .unwrap();

        let txn = db.begin().await.unwrap();
        let moved = ConfigKv::rename_section_with_conn(&txn, "remote.origin.", "remote.upstream.")
            .await
            .unwrap();
        txn.commit().await.unwrap();
        assert_eq!(moved, 2);

        assert!(
            ConfigKv::get_with_conn(&db, "remote.upstream.url")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            ConfigKv::get_with_conn(&db, "remote.origin.url")
                .await
                .unwrap()
                .is_none()
        );
        // The originator decoy must be untouched.
        assert_eq!(
            ConfigKv::get_with_conn(&db, "remote.originator.url")
                .await
                .unwrap()
                .unwrap()
                .value,
            "keep"
        );
    }

    #[tokio::test]
    async fn rename_section_conflict_leaves_source_unchanged() {
        let (_dir, db) = new_kv_db().await;
        ConfigKv::add_with_conn(&db, "remote.origin.url", "src", false)
            .await
            .unwrap();
        ConfigKv::add_with_conn(&db, "remote.upstream.url", "dst", false)
            .await
            .unwrap();

        let txn = db.begin().await.unwrap();
        let err = ConfigKv::rename_section_with_conn(&txn, "remote.origin.", "remote.upstream.")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "got: {err}");
        drop(txn); // rollback

        assert_eq!(
            ConfigKv::get_with_conn(&db, "remote.origin.url")
                .await
                .unwrap()
                .unwrap()
                .value,
            "src",
            "source must be unchanged after conflict"
        );
    }

    #[tokio::test]
    async fn rename_section_preserves_encrypted_flag_and_value() {
        let (_dir, db) = new_kv_db().await;
        // Auto-encrypted-looking row (encrypted flag set, hex ciphertext value).
        ConfigKv::add_with_conn(&db, "my.api.token", "deadbeef", true)
            .await
            .unwrap();
        let txn = db.begin().await.unwrap();
        let moved = ConfigKv::rename_section_with_conn(&txn, "my.", "yours.")
            .await
            .unwrap();
        txn.commit().await.unwrap();
        assert_eq!(moved, 1);
        let moved_row = ConfigKv::get_with_conn(&db, "yours.api.token")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(moved_row.value, "deadbeef", "ciphertext preserved verbatim");
        assert!(moved_row.encrypted, "encrypted flag preserved");
    }

    #[tokio::test]
    async fn remove_section_deletes_prefix_and_reports_count() {
        let (_dir, db) = new_kv_db().await;
        ConfigKv::add_with_conn(&db, "branch.main.remote", "origin", false)
            .await
            .unwrap();
        ConfigKv::add_with_conn(&db, "branch.main.merge", "refs/heads/main", false)
            .await
            .unwrap();
        ConfigKv::add_with_conn(&db, "branch.dev.remote", "origin", false)
            .await
            .unwrap();

        let txn = db.begin().await.unwrap();
        let removed = ConfigKv::remove_section_with_conn(&txn, "branch.main.")
            .await
            .unwrap();
        txn.commit().await.unwrap();
        assert_eq!(removed, 2);
        assert!(
            ConfigKv::get_with_conn(&db, "branch.dev.remote")
                .await
                .unwrap()
                .is_some(),
            "sibling section must survive"
        );
    }

    #[test]
    fn protected_vault_section_predicate() {
        for protected in [
            "vault",
            "vault.env",
            "vault.ssh.origin",
            "vault.gpg",
            "x.privkey",
        ] {
            assert!(
                is_protected_vault_section(protected),
                "{protected} should be protected"
            );
        }
        for ok in ["remote.origin", "branch.main", "my.api", "core"] {
            assert!(
                !is_protected_vault_section(ok),
                "{ok} should not be protected"
            );
        }
    }

    #[test]
    fn section_syntax_validation() {
        for ok in [
            "remote.origin",
            "branch.main",
            "core",
            "cloud.clone_domains",
        ] {
            assert!(validate_section_syntax(ok).is_ok(), "{ok} should be valid");
        }
        for bad in ["", ".x", "x.", "a..b", "a b", "re*mote"] {
            assert!(
                validate_section_syntax(bad).is_err(),
                "{bad:?} should be invalid"
            );
        }
    }

    // ── typed-value parsers ─────────────────────────────────────────────

    #[test]
    fn parse_int_suffixes_and_overflow() {
        assert_eq!(parse_config_int("1k").unwrap(), 1024);
        assert_eq!(parse_config_int("2M").unwrap(), 2 * 1024 * 1024);
        assert_eq!(parse_config_int("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_config_int("-3").unwrap(), -3);
        assert!(parse_config_int("nope").is_err());
        // Overflow must be caught, not panic (debug) or wrap (release).
        assert!(parse_config_int("9223372036854775807g").is_err());
    }

    #[test]
    fn parse_bool_tokens() {
        for t in ["true", "YES", "On", "1"] {
            assert_eq!(parse_config_bool(t), Some(true));
        }
        for f in ["false", "no", "OFF", "0"] {
            assert_eq!(parse_config_bool(f), Some(false));
        }
        assert_eq!(parse_config_bool("maybe"), None);
    }

    #[tokio::test]
    async fn get_int_overflow_does_not_panic() {
        let (_dir, db) = new_kv_db().await;
        ConfigKv::add_with_conn(&db, "core.size", "9223372036854775807g", false)
            .await
            .unwrap();
        assert!(
            ConfigKv::get_int_with_conn(&db, "core.size").await.is_err(),
            "overflow must be a recoverable Err, never a panic"
        );
    }
}
