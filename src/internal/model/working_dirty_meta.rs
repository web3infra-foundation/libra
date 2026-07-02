//! SeaORM entity for the single-row `working_dirty_meta` freshness record
//! (lore.md 1.1). All access goes through `internal::dirty::DirtyCache`.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "working_dirty_meta")]
pub struct Model {
    /// Always 1 (single row, enforced by a CHECK constraint).
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    /// `fresh` / `stale`.
    pub state: String,
    /// Hex of the index file's trailing content checksum at scan time
    /// (width follows the active hash kind); `absent` when no index existed.
    pub index_fingerprint: Option<String>,
    /// HEAD commit at scan time — the staged snapshot keys on BOTH.
    pub head_oid: Option<String>,
    pub scanned_at: Option<String>,
    pub scan_lock_pid: Option<i64>,
    pub scan_lock_at: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
