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
    ///
    /// Multipliers are 1024-based (KiB/MiB/GiB) to mirror `git config --int`
    /// behaviour. Returns `Ok(None)` for missing keys, `Err` for unparseable
    /// values, with the same `<REDACTED>` policy as [`Self::get_bool_with_conn`].
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

    /// Pool-acquiring counterpart of [`Self::rename_remote_with_conn`].
    pub async fn rename_remote(old: &str, new: &str) -> Result<()> {
        let db = get_db_conn_instance().await;
        Self::rename_remote_with_conn(&db, old, new).await
    }
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

/// Resolve user identity values from config and environment while preserving
/// the source boundary between the two.
///
/// The returned [`UserIdentitySources`] keeps config-derived and env-derived
/// values in separate fields so callers (notably `libra commit`) can apply
/// `user.useConfigOnly` semantics — refusing to fall back to env vars when
/// the user has explicitly opted into config-only identity.
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
        config.save(db).await.unwrap();
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
            .unwrap()
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
        match Config::get_current_remote_with_conn(db).await.unwrap() {
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

    /// Legacy "list every remote" helper. Deprecated; prefer
    /// [`ConfigKv::all_remote_configs_with_conn`].
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
            .unwrap();
        remote.map(|r| RemoteConfig {
            name: r.name.unwrap(),
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
