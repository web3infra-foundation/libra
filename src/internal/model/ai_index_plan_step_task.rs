//! SeaORM entity for the plan step -> task reverse index.
//!
//! 计划步骤 -> 任务反向索引的 SeaORM 实体。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "ai_index_plan_step_task")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub plan_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub task_id: String,
    pub step_id: String,
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
