//! SeaORM entity definition for operation workspace pointer snapshots.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "operation_view_workspace")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub view_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub pointer_kind: String,
    pub pointer_value: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
