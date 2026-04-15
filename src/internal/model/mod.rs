//! Exposes SeaORM entity modules for config, reference, reflog, and object_index tables used across the internal database layer.

pub mod ai_decision_proposal;
pub mod ai_index_intent_context_frame;
pub mod ai_index_intent_plan;
pub mod ai_index_intent_task;
pub mod ai_index_plan_step_task;
pub mod ai_index_run_event;
pub mod ai_index_run_patchset;
pub mod ai_index_task_run;
pub mod ai_live_context_window;
pub mod ai_risk_score_breakdown;
pub mod ai_scheduler_plan_head;
pub mod ai_scheduler_selected_plan;
pub mod ai_scheduler_state;
pub mod ai_thread;
pub mod ai_thread_intent;
pub mod ai_thread_participant;
pub mod ai_thread_provider_metadata;
pub mod ai_validation_report;
pub mod config;
pub mod config_kv;
pub mod object_index;
pub mod reference;
pub mod reflog;

#[cfg(test)]
mod reference_test;
