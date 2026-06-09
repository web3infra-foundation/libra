//! SeaORM entity definition for reflog entries that record ref transitions with actor metadata and messages.
//!
//! 记录引用转换以及行为者元数据和消息的 reflog 条目的 SeaORM 实体定义。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "reflog")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub id: i64,
    pub ref_name: String,
    pub old_oid: String,
    pub new_oid: String,
    pub timestamp: i64,
    pub committer_name: String,
    pub committer_email: String,
    pub action: String,
    pub message: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
