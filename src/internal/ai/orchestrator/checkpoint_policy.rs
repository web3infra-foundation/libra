//! Checkpoint policy for deciding when an orchestrated AI run should save durable
//! progress.
//!
//! Boundary: policy reads intent risk and plan state but does not persist records
//! directly. Scheduler and storage-flow tests cover high-risk plans, skipped phases,
//! and final validation checkpoints.

use crate::internal::ai::intentspec::types::IntentSpec;

pub(super) fn checkpoint_on_replan(spec: &IntentSpec) -> bool {
    spec.libra
        .as_ref()
        .and_then(|libra| libra.context_pipeline.as_ref())
        .is_none_or(|pipeline| pipeline.checkpoint_on_replan)
}

pub(super) fn checkpoint_before_replan(spec: &IntentSpec) -> bool {
    spec.libra
        .as_ref()
        .and_then(|libra| libra.decision_policy.as_ref())
        .is_none_or(|policy| policy.checkpoint_before_replan)
}

pub(super) fn dagrs_checkpointing_enabled(spec: &IntentSpec) -> bool {
    checkpoint_on_replan(spec) || checkpoint_before_replan(spec)
}
