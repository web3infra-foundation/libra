//! SeaORM entity for the selected execution/test plan pair.
//!
//! 选定的执行/测试计划对的 SeaORM 实体。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "ai_scheduler_selected_plan")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub thread_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub plan_id: String,
    pub ordinal: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
