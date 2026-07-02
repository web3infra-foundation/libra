//! Unified scoped metadata KV store (lore.md §1.5) — the SINGLE owner API for
//! the `metadata_kv` table. Branch metadata (and future scopes) live here;
//! repo-scope metadata intentionally lives in `config_kv` under the
//! `metadata.*` namespace (see [`REPO_METADATA_PREFIX`]) so `libra config`
//! tooling keeps working on it.
//!
//! `protect` / `archive` / `lineage.*` are KEYS in this store, never separate
//! tables. Nothing enforces them yet: enforcement lands once, in the future
//! branch-policy layer (lore.md 1.13), which reads through
//! [`MetadataKv::is_protected_with_conn`] / [`MetadataKv::is_archived_with_conn`]
//! inside its authoritative transaction. The truthy parse is FAIL-CLOSED — a
//! garbage value counts as protected — so a corrupted value can never silently
//! disable protection when enforcement arrives.
//!
//! Every core operation ships a `_with_conn` variant (transaction-safe,
//! matching the `ConfigKv` convention) plus a pool-acquiring wrapper.

use anyhow::{Context, Result};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder, sea_query::OnConflict,
};

use crate::internal::{db::get_db_conn_instance, model::metadata_kv};

/// Repo-scope metadata namespace inside `config_kv` (lore.md §1.5: repo =
/// config_kv). `libra metadata --repo <key>` reads/writes `metadata.<key>`
/// through `ConfigKv`; `libra config` operating on the same keys is an
/// intended dual surface.
pub const REPO_METADATA_PREFIX: &str = "metadata.";

/// Well-known branch-metadata key: branch protection (recorded now, enforced
/// by the future branch-policy layer, lore.md 1.13).
pub const KEY_PROTECT: &str = "protect";
/// Well-known branch-metadata key: branch archival.
pub const KEY_ARCHIVE: &str = "archive";
/// Well-known branch-metadata key prefix: branch lineage records.
pub const LINEAGE_PREFIX: &str = "lineage.";

/// Maximum metadata key length in bytes.
pub const MAX_KEY_LEN: usize = 256;
/// Maximum metadata value length in bytes (text values in v1).
pub const MAX_VALUE_LEN: usize = 1024 * 1024;

/// The metadata scope. v1 supports `Branch`; the `scope` column is TEXT so
/// future scopes (worktree, …; revision/file metadata use trailers/side-trees
/// per lore.md 1.10, not this table) need no table rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataScope {
    Branch,
}

impl MetadataScope {
    pub fn as_str(self) -> &'static str {
        match self {
            MetadataScope::Branch => "branch",
        }
    }
}

/// A single metadata entry as read back from the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataEntry {
    pub scope: String,
    pub target: String,
    pub key: String,
    pub value: String,
    pub value_type: String,
}

impl MetadataEntry {
    fn from_model(model: &metadata_kv::Model) -> Self {
        Self {
            scope: model.scope.clone(),
            target: model.target.clone(),
            key: model.key.clone(),
            value: model.value.clone(),
            value_type: model.value_type.clone(),
        }
    }
}

/// Validate a metadata key: non-empty, ≤ [`MAX_KEY_LEN`] bytes, no whitespace
/// or control characters (keys are exact, case-sensitive identifiers).
pub fn validate_key(key: &str) -> std::result::Result<(), String> {
    if key.is_empty() {
        return Err("metadata key must not be empty".to_string());
    }
    if key.len() > MAX_KEY_LEN {
        return Err(format!("metadata key exceeds {MAX_KEY_LEN} bytes: '{key}'"));
    }
    if key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "metadata key must not contain whitespace or control characters: '{key}'"
        ));
    }
    Ok(())
}

/// Validate a metadata value: ≤ [`MAX_VALUE_LEN`] bytes. The empty string is
/// legal and distinct from an absent key.
pub fn validate_value(value: &str) -> std::result::Result<(), String> {
    if value.len() > MAX_VALUE_LEN {
        return Err(format!(
            "metadata value exceeds {} bytes ({} given)",
            MAX_VALUE_LEN,
            value.len()
        ));
    }
    Ok(())
}

/// Whether a recorded flag value counts as SET, parsed FAIL-CLOSED: the
/// explicit falsy spellings (`false`/`0`/`no`/`off`, case-insensitive,
/// trimmed) count as off; EVERYTHING else — including garbage — counts as on,
/// so a corrupted value can never silently disable protection.
fn truthy_fail_closed(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "false" | "0" | "no" | "off"
    )
}

/// The single owner API for the `metadata_kv` table.
pub struct MetadataKv;

impl MetadataKv {
    /// Get one entry, or `None` when absent.
    pub async fn get_with_conn<C: ConnectionTrait>(
        db: &C,
        scope: MetadataScope,
        target: &str,
        key: &str,
    ) -> Result<Option<MetadataEntry>> {
        let row = metadata_kv::Entity::find()
            .filter(metadata_kv::Column::Scope.eq(scope.as_str()))
            .filter(metadata_kv::Column::Target.eq(target))
            .filter(metadata_kv::Column::Key.eq(key))
            .one(db)
            .await
            .context("failed to query metadata_kv")?;
        Ok(row.as_ref().map(MetadataEntry::from_model))
    }

    /// Pool-acquiring counterpart of [`Self::get_with_conn`].
    pub async fn get(
        scope: MetadataScope,
        target: &str,
        key: &str,
    ) -> Result<Option<MetadataEntry>> {
        let db = get_db_conn_instance().await;
        Self::get_with_conn(&db, scope, target, key).await
    }

    /// Upsert one entry atomically (`INSERT … ON CONFLICT DO UPDATE` on the
    /// `(scope, target, key)` unique key — no find-then-insert race). Returns
    /// the PREVIOUS value when the key already existed.
    pub async fn set_with_conn<C: ConnectionTrait>(
        db: &C,
        scope: MetadataScope,
        target: &str,
        key: &str,
        value: &str,
    ) -> Result<Option<String>> {
        let previous = Self::get_with_conn(db, scope, target, key)
            .await?
            .map(|entry| entry.value);
        let now = Utc::now().to_rfc3339();
        let active = metadata_kv::ActiveModel {
            scope: Set(scope.as_str().to_string()),
            target: Set(target.to_string()),
            key: Set(key.to_string()),
            value: Set(value.to_string()),
            value_type: Set("text".to_string()),
            created_at: Set(now.clone()),
            updated_at: Set(now),
            ..Default::default()
        };
        let on_conflict = OnConflict::columns([
            metadata_kv::Column::Scope,
            metadata_kv::Column::Target,
            metadata_kv::Column::Key,
        ])
        .update_columns([
            metadata_kv::Column::Value,
            metadata_kv::Column::ValueType,
            metadata_kv::Column::UpdatedAt,
        ])
        .to_owned();
        metadata_kv::Entity::insert(active)
            .on_conflict(on_conflict)
            .exec(db)
            .await
            .context("failed to upsert metadata_kv entry")?;
        Ok(previous)
    }

    /// Pool-acquiring counterpart of [`Self::set_with_conn`].
    pub async fn set(
        scope: MetadataScope,
        target: &str,
        key: &str,
        value: &str,
    ) -> Result<Option<String>> {
        let db = get_db_conn_instance().await;
        Self::set_with_conn(&db, scope, target, key, value).await
    }

    /// Delete one entry; returns whether a row was removed.
    pub async fn unset_with_conn<C: ConnectionTrait>(
        db: &C,
        scope: MetadataScope,
        target: &str,
        key: &str,
    ) -> Result<bool> {
        let result = metadata_kv::Entity::delete_many()
            .filter(metadata_kv::Column::Scope.eq(scope.as_str()))
            .filter(metadata_kv::Column::Target.eq(target))
            .filter(metadata_kv::Column::Key.eq(key))
            .exec(db)
            .await
            .context("failed to delete metadata_kv entry")?;
        Ok(result.rows_affected > 0)
    }

    /// Pool-acquiring counterpart of [`Self::unset_with_conn`].
    pub async fn unset(scope: MetadataScope, target: &str, key: &str) -> Result<bool> {
        let db = get_db_conn_instance().await;
        Self::unset_with_conn(&db, scope, target, key).await
    }

    /// List a target's entries, key-ordered, optionally filtered to a key
    /// prefix.
    pub async fn list_with_conn<C: ConnectionTrait>(
        db: &C,
        scope: MetadataScope,
        target: &str,
        key_prefix: Option<&str>,
    ) -> Result<Vec<MetadataEntry>> {
        let mut query = metadata_kv::Entity::find()
            .filter(metadata_kv::Column::Scope.eq(scope.as_str()))
            .filter(metadata_kv::Column::Target.eq(target));
        if let Some(prefix) = key_prefix {
            query = query.filter(metadata_kv::Column::Key.starts_with(prefix));
        }
        let rows = query
            .order_by_asc(metadata_kv::Column::Key)
            .all(db)
            .await
            .context("failed to list metadata_kv entries")?;
        Ok(rows.iter().map(MetadataEntry::from_model).collect())
    }

    /// Pool-acquiring counterpart of [`Self::list_with_conn`].
    pub async fn list(
        scope: MetadataScope,
        target: &str,
        key_prefix: Option<&str>,
    ) -> Result<Vec<MetadataEntry>> {
        let db = get_db_conn_instance().await;
        Self::list_with_conn(&db, scope, target, key_prefix).await
    }

    /// Delete every entry for a target (branch-delete cascade). Returns the
    /// number of rows removed.
    pub async fn delete_all_for_target_with_conn<C: ConnectionTrait>(
        db: &C,
        scope: MetadataScope,
        target: &str,
    ) -> Result<u64> {
        let result = metadata_kv::Entity::delete_many()
            .filter(metadata_kv::Column::Scope.eq(scope.as_str()))
            .filter(metadata_kv::Column::Target.eq(target))
            .exec(db)
            .await
            .context("failed to cascade-delete metadata_kv entries")?;
        Ok(result.rows_affected)
    }

    /// Move a target's entries to a new target name (branch rename). Any
    /// pre-existing rows under the destination are removed first so the
    /// `(scope, target, key)` unique key cannot abort mid-move (defensive —
    /// the branch CLI refuses to rename onto an existing branch).
    pub async fn rename_target_with_conn<C: ConnectionTrait>(
        db: &C,
        scope: MetadataScope,
        old_target: &str,
        new_target: &str,
    ) -> Result<()> {
        Self::delete_all_for_target_with_conn(db, scope, new_target).await?;
        metadata_kv::Entity::update_many()
            .col_expr(
                metadata_kv::Column::Target,
                sea_orm::sea_query::Expr::value(new_target),
            )
            .filter(metadata_kv::Column::Scope.eq(scope.as_str()))
            .filter(metadata_kv::Column::Target.eq(old_target))
            .exec(db)
            .await
            .context("failed to move metadata_kv entries to the renamed target")?;
        Ok(())
    }

    /// Copy a target's entries to another target (branch copy). Destination
    /// rows are replaced (matching `branch -C`'s overwrite semantics).
    pub async fn copy_target_with_conn<C: ConnectionTrait>(
        db: &C,
        scope: MetadataScope,
        from_target: &str,
        to_target: &str,
    ) -> Result<()> {
        // Self-copy (`branch -C x x`) must be a no-op: clearing the
        // destination first would delete the source rows and then copy
        // nothing, silently losing the metadata.
        if from_target == to_target {
            return Ok(());
        }
        Self::delete_all_for_target_with_conn(db, scope, to_target).await?;
        let entries = Self::list_with_conn(db, scope, from_target, None).await?;
        let now = Utc::now().to_rfc3339();
        for entry in entries {
            let active = metadata_kv::ActiveModel {
                scope: Set(entry.scope),
                target: Set(to_target.to_string()),
                key: Set(entry.key),
                value: Set(entry.value),
                value_type: Set(entry.value_type),
                created_at: Set(now.clone()),
                updated_at: Set(now.clone()),
                ..Default::default()
            };
            active
                .insert(db)
                .await
                .context("failed to copy metadata_kv entry")?;
        }
        Ok(())
    }

    /// Whether the branch is recorded as protected — the read entry the future
    /// branch-policy layer (lore.md 1.13) calls inside its authoritative
    /// transaction. FAIL-CLOSED: any value other than the explicit falsy
    /// spellings counts as protected.
    pub async fn is_protected_with_conn<C: ConnectionTrait>(db: &C, branch: &str) -> Result<bool> {
        Ok(
            Self::get_with_conn(db, MetadataScope::Branch, branch, KEY_PROTECT)
                .await?
                .is_some_and(|entry| truthy_fail_closed(&entry.value)),
        )
    }

    /// Whether the branch is recorded as archived (fail-closed, like
    /// [`Self::is_protected_with_conn`]).
    pub async fn is_archived_with_conn<C: ConnectionTrait>(db: &C, branch: &str) -> Result<bool> {
        Ok(
            Self::get_with_conn(db, MetadataScope::Branch, branch, KEY_ARCHIVE)
                .await?
                .is_some_and(|entry| truthy_fail_closed(&entry.value)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_validation_rejects_empty_whitespace_and_oversize() {
        assert!(validate_key("protect").is_ok());
        assert!(validate_key("lineage.parent").is_ok());
        assert!(validate_key("").is_err());
        assert!(validate_key("has space").is_err());
        assert!(validate_key("has\ttab").is_err());
        assert!(validate_key(&"k".repeat(MAX_KEY_LEN + 1)).is_err());
        assert!(validate_key(&"k".repeat(MAX_KEY_LEN)).is_ok());
    }

    #[test]
    fn value_validation_allows_empty_and_bounds_size() {
        assert!(validate_value("").is_ok());
        assert!(validate_value("v").is_ok());
        assert!(validate_value(&"v".repeat(MAX_VALUE_LEN)).is_ok());
        assert!(validate_value(&"v".repeat(MAX_VALUE_LEN + 1)).is_err());
    }

    #[test]
    fn truthy_parse_is_fail_closed() {
        for on in ["true", "1", "yes", "on", "TRUE", " weird-garbage ", ""] {
            assert!(truthy_fail_closed(on), "{on:?} must count as set");
        }
        for off in ["false", "0", "no", "off", "FALSE", " Off "] {
            assert!(!truthy_fail_closed(off), "{off:?} must count as unset");
        }
    }
}
