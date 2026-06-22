//! SeaORM entity definition for operation view snapshots.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "operation_view")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub view_id: String,
    pub repo_id: String,
    pub head_kind: String,
    pub head_target: String,
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
