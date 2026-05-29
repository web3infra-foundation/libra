//! Read-side projection resolver for code runtime resume and diagnostics.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    IntentContextFrameIndexRow, IntentPlanIndexRow, IntentTaskIndexRow, PlanStepTaskIndexRow,
    ProjectionRebuilder, RunEventIndexRow, RunPatchSetIndexRow, SchedulerState,
    SchedulerStateRepository, TaskRunIndexRow, ThreadId, ThreadProjection,
};
use crate::internal::{
    ai::runtime::contracts::{ProjectionFreshness, WorkflowPhase},
    model::{
        ai_index_intent_context_frame, ai_index_intent_plan, ai_index_intent_task,
        ai_index_plan_step_task, ai_index_run_event, ai_index_run_patchset, ai_index_task_run,
    },
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadBundle {
    pub thread: ThreadProjection,
    pub scheduler: SchedulerState,
    pub freshness: ProjectionFreshness,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadQueryIndexes {
    pub thread_id: ThreadId,
    pub freshness: ProjectionFreshness,
    pub diagnostics: Vec<QueryIndexDiagnostic>,
    pub intent_plan_index: Vec<IntentPlanIndexRow>,
    pub intent_task_index: Vec<IntentTaskIndexRow>,
    pub plan_step_task_index: Vec<PlanStepTaskIndexRow>,
    pub task_run_index: Vec<TaskRunIndexRow>,
    pub run_event_index: Vec<RunEventIndexRow>,
    pub run_patchset_index: Vec<RunPatchSetIndexRow>,
    pub intent_context_frame_index: Vec<IntentContextFrameIndexRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryIndexDiagnostic {
    pub code: String,
    pub index_name: String,
    pub subject_id: Uuid,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeBundle {
    pub thread: ThreadProjection,
    pub scheduler: SchedulerState,
    pub freshness: ProjectionFreshness,
    pub phase_at_resume: WorkflowPhase,
    pub resume_reason: ResumeReason,
    pub resume_actions: Vec<ResumeAction>,
}

impl ResumeBundle {
    pub fn from_thread_bundle(bundle: ThreadBundle) -> Self {
        let phase_at_resume = infer_resume_phase(&bundle.thread, &bundle.scheduler);
        let (resume_reason, resume_actions) =
            resume_contract(bundle.freshness, phase_at_resume, &bundle.scheduler);

        Self {
            thread: bundle.thread,
            scheduler: bundle.scheduler,
            freshness: bundle.freshness,
            phase_at_resume,
            resume_reason,
            resume_actions,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeReason {
    FreshThread,
    InterruptedRun,
    ProjectionStale,
    ProjectionUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeAction {
    ReopenIntentReview,
    ReopenPlanningReview,
    ResumeScheduler,
    RequeueInterruptedRun,
    TriggerTargetedRebuild,
    OpenReadOnly,
    BlockAutomaticResume,
}

#[derive(Clone)]
pub struct ProjectionResolver {
    db: DatabaseConnection,
    scheduler: SchedulerStateRepository,
}

impl ProjectionResolver {
    pub fn new(db: DatabaseConnection) -> Self {
        Self {
            scheduler: SchedulerStateRepository::new(db.clone()),
            db,
        }
    }

    pub async fn load_thread_bundle(&self, thread_id: ThreadId) -> Result<Option<ThreadBundle>> {
        let Some(thread) = ThreadProjection::find_by_id(&self.db, thread_id)
            .await
            .with_context(|| format!("Failed to resolve thread projection {thread_id}"))?
        else {
            return Ok(None);
        };

        let scheduler = match self.scheduler.load(thread_id).await? {
            Some(scheduler) => scheduler,
            None => {
                return Ok(Some(ThreadBundle {
                    scheduler: empty_scheduler(thread_id),
                    thread,
                    freshness: ProjectionFreshness::StaleReadOnly,
                }));
            }
        };

        Ok(Some(ThreadBundle {
            thread,
            scheduler,
            freshness: ProjectionFreshness::Fresh,
        }))
    }

    pub async fn load_query_indexes(
        &self,
        thread_id: ThreadId,
    ) -> Result<Option<ThreadQueryIndexes>> {
        let Some(bundle) = self.load_thread_bundle(thread_id).await? else {
            return Ok(None);
        };

        self.load_query_indexes_for_bundle(&bundle).await.map(Some)
    }

    pub async fn load_or_rebuild_query_indexes(
        &self,
        thread_id: ThreadId,
        rebuilder: &ProjectionRebuilder<'_>,
    ) -> Result<Option<ThreadQueryIndexes>> {
        let existing = self.load_query_indexes(thread_id).await?;
        if existing.as_ref().is_some_and(|indexes| {
            indexes.freshness == ProjectionFreshness::Fresh && indexes.diagnostics.is_empty()
        }) {
            return Ok(existing);
        }

        match rebuilder.materialize_thread(&self.db, thread_id).await {
            Ok(Some(_)) => self.load_query_indexes(thread_id).await,
            Ok(None) => Ok(existing),
            Err(error) => {
                if let Some(mut indexes) = existing {
                    indexes.freshness = ProjectionFreshness::Unavailable;
                    indexes.diagnostics.push(query_index_diagnostic(
                        "query_index_rebuild_failed",
                        "all",
                        thread_id,
                        format!(
                            "Targeted query-index rebuild failed for thread {thread_id}: {error:#}"
                        ),
                    ));
                    Ok(Some(indexes))
                } else {
                    Err(error.context(format!(
                        "Failed to rebuild missing query indexes for thread {thread_id}"
                    )))
                }
            }
        }
    }

    async fn load_query_indexes_for_bundle(
        &self,
        bundle: &ThreadBundle,
    ) -> Result<ThreadQueryIndexes> {
        let thread_id = bundle.thread.thread_id;
        let intent_ids = thread_intent_ids(bundle);

        let intent_plan_index = if intent_ids.is_empty() {
            Vec::new()
        } else {
            ai_index_intent_plan::Entity::find()
                .filter(ai_index_intent_plan::Column::IntentId.is_in(strings(&intent_ids)))
                .order_by_asc(ai_index_intent_plan::Column::IntentId)
                .order_by_asc(ai_index_intent_plan::Column::PlanId)
                .all(&self.db)
                .await
                .with_context(|| {
                    format!("Failed to load intent-plan query index rows for thread {thread_id}")
                })?
                .into_iter()
                .map(|row| intent_plan_index_from_model(row, thread_id))
                .collect::<Result<Vec<_>>>()?
        };

        let intent_task_index = if intent_ids.is_empty() {
            Vec::new()
        } else {
            ai_index_intent_task::Entity::find()
                .filter(ai_index_intent_task::Column::IntentId.is_in(strings(&intent_ids)))
                .order_by_asc(ai_index_intent_task::Column::IntentId)
                .order_by_asc(ai_index_intent_task::Column::TaskId)
                .all(&self.db)
                .await
                .with_context(|| {
                    format!("Failed to load intent-task query index rows for thread {thread_id}")
                })?
                .into_iter()
                .map(|row| intent_task_index_from_model(row, thread_id))
                .collect::<Result<Vec<_>>>()?
        };

        let mut plan_ids = intent_plan_index
            .iter()
            .map(|row| row.plan_id)
            .collect::<BTreeSet<_>>();
        plan_ids.extend(bundle.scheduler.selected_plan_id);
        plan_ids.extend(
            bundle
                .scheduler
                .selected_plan_ids
                .iter()
                .map(|plan| plan.plan_id),
        );
        plan_ids.extend(
            bundle
                .scheduler
                .current_plan_heads
                .iter()
                .map(|plan| plan.plan_id),
        );

        let plan_step_task_index = if plan_ids.is_empty() {
            Vec::new()
        } else {
            ai_index_plan_step_task::Entity::find()
                .filter(ai_index_plan_step_task::Column::PlanId.is_in(strings(&plan_ids)))
                .order_by_asc(ai_index_plan_step_task::Column::PlanId)
                .order_by_asc(ai_index_plan_step_task::Column::StepId)
                .order_by_asc(ai_index_plan_step_task::Column::TaskId)
                .all(&self.db)
                .await
                .with_context(|| {
                    format!("Failed to load plan-step-task query index rows for thread {thread_id}")
                })?
                .into_iter()
                .map(|row| plan_step_task_index_from_model(row, thread_id))
                .collect::<Result<Vec<_>>>()?
        };

        let mut task_ids = intent_task_index
            .iter()
            .map(|row| row.task_id)
            .collect::<BTreeSet<_>>();
        task_ids.extend(plan_step_task_index.iter().map(|row| row.task_id));
        task_ids.extend(bundle.scheduler.active_task_id);
        if intent_ids.is_empty() {
            task_ids.insert(thread_id);
        }

        let task_run_index = if task_ids.is_empty() {
            Vec::new()
        } else {
            ai_index_task_run::Entity::find()
                .filter(ai_index_task_run::Column::TaskId.is_in(strings(&task_ids)))
                .order_by_asc(ai_index_task_run::Column::TaskId)
                .order_by_asc(ai_index_task_run::Column::RunId)
                .all(&self.db)
                .await
                .with_context(|| {
                    format!("Failed to load task-run query index rows for thread {thread_id}")
                })?
                .into_iter()
                .map(|row| task_run_index_from_model(row, thread_id))
                .collect::<Result<Vec<_>>>()?
        };

        let mut run_ids = task_run_index
            .iter()
            .map(|row| row.run_id)
            .collect::<BTreeSet<_>>();
        run_ids.extend(bundle.scheduler.active_run_id);

        let run_event_index = if run_ids.is_empty() {
            Vec::new()
        } else {
            ai_index_run_event::Entity::find()
                .filter(ai_index_run_event::Column::RunId.is_in(strings(&run_ids)))
                .order_by_asc(ai_index_run_event::Column::RunId)
                .order_by_asc(ai_index_run_event::Column::EventId)
                .all(&self.db)
                .await
                .with_context(|| {
                    format!("Failed to load run-event query index rows for thread {thread_id}")
                })?
                .into_iter()
                .map(|row| run_event_index_from_model(row, thread_id))
                .collect::<Result<Vec<_>>>()?
        };

        let run_patchset_index = if run_ids.is_empty() {
            Vec::new()
        } else {
            ai_index_run_patchset::Entity::find()
                .filter(ai_index_run_patchset::Column::RunId.is_in(strings(&run_ids)))
                .order_by_asc(ai_index_run_patchset::Column::RunId)
                .order_by_asc(ai_index_run_patchset::Column::Sequence)
                .order_by_asc(ai_index_run_patchset::Column::PatchsetId)
                .all(&self.db)
                .await
                .with_context(|| {
                    format!("Failed to load run-patchset query index rows for thread {thread_id}")
                })?
                .into_iter()
                .map(|row| run_patchset_index_from_model(row, thread_id))
                .collect::<Result<Vec<_>>>()?
        };

        let intent_context_frame_index = if intent_ids.is_empty() {
            Vec::new()
        } else {
            ai_index_intent_context_frame::Entity::find()
                .filter(
                    ai_index_intent_context_frame::Column::IntentId.is_in(strings(&intent_ids)),
                )
                .order_by_asc(ai_index_intent_context_frame::Column::IntentId)
                .order_by_asc(ai_index_intent_context_frame::Column::RelationKind)
                .order_by_asc(ai_index_intent_context_frame::Column::ContextFrameId)
                .all(&self.db)
                .await
                .with_context(|| {
                    format!(
                        "Failed to load intent-context-frame query index rows for thread {thread_id}"
                    )
                })?
                .into_iter()
                .map(|row| intent_context_frame_index_from_model(row, thread_id))
                .collect::<Result<Vec<_>>>()?
        };

        let diagnostics = query_index_diagnostics(
            bundle,
            &intent_plan_index,
            &intent_task_index,
            &plan_step_task_index,
            &task_run_index,
        );

        Ok(ThreadQueryIndexes {
            thread_id,
            freshness: bundle.freshness,
            diagnostics,
            intent_plan_index,
            intent_task_index,
            plan_step_task_index,
            task_run_index,
            run_event_index,
            run_patchset_index,
            intent_context_frame_index,
        })
    }

    pub async fn load_or_rebuild_thread_bundle(
        &self,
        thread_id: ThreadId,
        rebuilder: &ProjectionRebuilder<'_>,
    ) -> Result<Option<ThreadBundle>> {
        let existing = self.load_thread_bundle(thread_id).await?;
        if existing
            .as_ref()
            .is_some_and(|bundle| bundle.freshness == ProjectionFreshness::Fresh)
        {
            return Ok(existing);
        }

        match rebuilder.materialize_thread(&self.db, thread_id).await {
            Ok(Some(_)) => self.load_thread_bundle(thread_id).await,
            Ok(None) => Ok(existing),
            Err(error) => {
                if let Some(mut bundle) = existing {
                    bundle.freshness = ProjectionFreshness::Unavailable;
                    Ok(Some(bundle))
                } else {
                    Err(error.context(format!(
                        "Failed to rebuild missing projection for thread {thread_id}"
                    )))
                }
            }
        }
    }

    pub async fn load_for_resume(
        &self,
        thread_id: ThreadId,
        rebuilder: &ProjectionRebuilder<'_>,
    ) -> Result<Option<ResumeBundle>> {
        Ok(self
            .load_or_rebuild_thread_bundle(thread_id, rebuilder)
            .await?
            .map(ResumeBundle::from_thread_bundle))
    }
}

fn thread_intent_ids(bundle: &ThreadBundle) -> BTreeSet<Uuid> {
    let mut ids = bundle
        .thread
        .intents
        .iter()
        .map(|intent| intent.intent_id)
        .collect::<BTreeSet<_>>();
    ids.extend(bundle.thread.current_intent_id);
    ids.extend(bundle.thread.latest_intent_id);
    ids
}

fn query_index_diagnostics(
    bundle: &ThreadBundle,
    intent_plan_index: &[IntentPlanIndexRow],
    intent_task_index: &[IntentTaskIndexRow],
    plan_step_task_index: &[PlanStepTaskIndexRow],
    task_run_index: &[TaskRunIndexRow],
) -> Vec<QueryIndexDiagnostic> {
    let mut diagnostics = Vec::new();
    let intent_ids = thread_intent_ids(bundle);
    let plan_ids = scheduler_plan_ids(&bundle.scheduler);

    if !intent_ids.is_empty() {
        for plan_id in plan_ids {
            let indexed = intent_plan_index
                .iter()
                .any(|row| row.plan_id == plan_id && intent_ids.contains(&row.intent_id));
            if !indexed {
                diagnostics.push(query_index_diagnostic(
                    "missing_intent_plan_index",
                    "ai_index_intent_plan",
                    plan_id,
                    format!(
                        "Scheduler references plan {plan_id}, but no intent-plan query index row links it to thread {}",
                        bundle.thread.thread_id
                    ),
                ));
            }
        }
    }

    if let Some(active_task_id) = bundle.scheduler.active_task_id {
        let indexed = intent_task_index
            .iter()
            .any(|row| row.task_id == active_task_id)
            || plan_step_task_index
                .iter()
                .any(|row| row.task_id == active_task_id);
        if !indexed {
            diagnostics.push(query_index_diagnostic(
                "missing_active_task_index",
                "ai_index_intent_task",
                active_task_id,
                format!(
                    "Scheduler active task {active_task_id} is not reachable from intent-task or plan-step-task query indexes"
                ),
            ));
        }
    }

    if let Some(active_run_id) = bundle.scheduler.active_run_id {
        let indexed = task_run_index.iter().any(|row| row.run_id == active_run_id);
        if !indexed {
            diagnostics.push(query_index_diagnostic(
                "missing_active_run_index",
                "ai_index_task_run",
                active_run_id,
                format!(
                    "Scheduler active run {active_run_id} is not reachable from task-run query indexes"
                ),
            ));
        }
    }

    diagnostics
}

fn scheduler_plan_ids(scheduler: &SchedulerState) -> BTreeSet<Uuid> {
    let mut plan_ids = BTreeSet::new();
    plan_ids.extend(scheduler.selected_plan_id);
    plan_ids.extend(scheduler.selected_plan_ids.iter().map(|plan| plan.plan_id));
    plan_ids.extend(scheduler.current_plan_heads.iter().map(|plan| plan.plan_id));
    plan_ids
}

fn query_index_diagnostic(
    code: &'static str,
    index_name: &'static str,
    subject_id: Uuid,
    message: String,
) -> QueryIndexDiagnostic {
    QueryIndexDiagnostic {
        code: code.to_string(),
        index_name: index_name.to_string(),
        subject_id,
        message,
    }
}

fn strings(ids: &BTreeSet<Uuid>) -> Vec<String> {
    ids.iter().map(Uuid::to_string).collect()
}

fn parse_uuid_field(raw: &str, thread_id: ThreadId, field: &str) -> Result<Uuid> {
    Uuid::parse_str(raw).with_context(|| {
        format!("Invalid {field} UUID in query index rows for thread {thread_id}: {raw}")
    })
}

fn parse_optional_uuid_field(
    raw: Option<&str>,
    thread_id: ThreadId,
    field: &str,
) -> Result<Option<Uuid>> {
    raw.map(|value| parse_uuid_field(value, thread_id, field))
        .transpose()
}

fn timestamp_from_index_row(raw: i64, thread_id: ThreadId, field: &str) -> Result<DateTime<Utc>> {
    Utc.timestamp_opt(raw, 0).single().with_context(|| {
        format!("Invalid {field} timestamp in query index rows for thread {thread_id}: {raw}")
    })
}

fn intent_plan_index_from_model(
    row: ai_index_intent_plan::Model,
    thread_id: ThreadId,
) -> Result<IntentPlanIndexRow> {
    Ok(IntentPlanIndexRow {
        intent_id: parse_uuid_field(&row.intent_id, thread_id, "intent_plan.intent_id")?,
        plan_id: parse_uuid_field(&row.plan_id, thread_id, "intent_plan.plan_id")?,
        created_at: timestamp_from_index_row(row.created_at, thread_id, "intent_plan.created_at")?,
    })
}

fn intent_task_index_from_model(
    row: ai_index_intent_task::Model,
    thread_id: ThreadId,
) -> Result<IntentTaskIndexRow> {
    Ok(IntentTaskIndexRow {
        intent_id: parse_uuid_field(&row.intent_id, thread_id, "intent_task.intent_id")?,
        task_id: parse_uuid_field(&row.task_id, thread_id, "intent_task.task_id")?,
        parent_task_id: parse_optional_uuid_field(
            row.parent_task_id.as_deref(),
            thread_id,
            "intent_task.parent_task_id",
        )?,
        origin_step_id: parse_optional_uuid_field(
            row.origin_step_id.as_deref(),
            thread_id,
            "intent_task.origin_step_id",
        )?,
        created_at: timestamp_from_index_row(row.created_at, thread_id, "intent_task.created_at")?,
    })
}

fn plan_step_task_index_from_model(
    row: ai_index_plan_step_task::Model,
    thread_id: ThreadId,
) -> Result<PlanStepTaskIndexRow> {
    Ok(PlanStepTaskIndexRow {
        plan_id: parse_uuid_field(&row.plan_id, thread_id, "plan_step_task.plan_id")?,
        step_id: parse_uuid_field(&row.step_id, thread_id, "plan_step_task.step_id")?,
        task_id: parse_uuid_field(&row.task_id, thread_id, "plan_step_task.task_id")?,
        created_at: timestamp_from_index_row(
            row.created_at,
            thread_id,
            "plan_step_task.created_at",
        )?,
    })
}

fn task_run_index_from_model(
    row: ai_index_task_run::Model,
    thread_id: ThreadId,
) -> Result<TaskRunIndexRow> {
    Ok(TaskRunIndexRow {
        task_id: parse_uuid_field(&row.task_id, thread_id, "task_run.task_id")?,
        run_id: parse_uuid_field(&row.run_id, thread_id, "task_run.run_id")?,
        is_latest: row.is_latest,
        created_at: timestamp_from_index_row(row.created_at, thread_id, "task_run.created_at")?,
    })
}

fn run_event_index_from_model(
    row: ai_index_run_event::Model,
    thread_id: ThreadId,
) -> Result<RunEventIndexRow> {
    Ok(RunEventIndexRow {
        run_id: parse_uuid_field(&row.run_id, thread_id, "run_event.run_id")?,
        event_id: parse_uuid_field(&row.event_id, thread_id, "run_event.event_id")?,
        event_kind: row.event_kind,
        is_latest: row.is_latest,
        created_at: timestamp_from_index_row(row.created_at, thread_id, "run_event.created_at")?,
    })
}

fn run_patchset_index_from_model(
    row: ai_index_run_patchset::Model,
    thread_id: ThreadId,
) -> Result<RunPatchSetIndexRow> {
    Ok(RunPatchSetIndexRow {
        run_id: parse_uuid_field(&row.run_id, thread_id, "run_patchset.run_id")?,
        patchset_id: parse_uuid_field(&row.patchset_id, thread_id, "run_patchset.patchset_id")?,
        sequence: row.sequence,
        is_latest: row.is_latest,
        created_at: timestamp_from_index_row(row.created_at, thread_id, "run_patchset.created_at")?,
    })
}

fn intent_context_frame_index_from_model(
    row: ai_index_intent_context_frame::Model,
    thread_id: ThreadId,
) -> Result<IntentContextFrameIndexRow> {
    Ok(IntentContextFrameIndexRow {
        intent_id: parse_uuid_field(&row.intent_id, thread_id, "intent_context_frame.intent_id")?,
        context_frame_id: parse_uuid_field(
            &row.context_frame_id,
            thread_id,
            "intent_context_frame.context_frame_id",
        )?,
        relation_kind: row.relation_kind,
        created_at: timestamp_from_index_row(
            row.created_at,
            thread_id,
            "intent_context_frame.created_at",
        )?,
    })
}

fn infer_resume_phase(thread: &ThreadProjection, scheduler: &SchedulerState) -> WorkflowPhase {
    if scheduler.active_task_id.is_some()
        || scheduler.active_run_id.is_some()
        || scheduler.selected_plan_id.is_some()
        || !scheduler.selected_plan_ids.is_empty()
    {
        WorkflowPhase::Execution
    } else if thread.current_intent_id.is_some() || thread.latest_intent_id.is_some() {
        WorkflowPhase::Planning
    } else {
        WorkflowPhase::Intent
    }
}

fn resume_contract(
    freshness: ProjectionFreshness,
    phase_at_resume: WorkflowPhase,
    scheduler: &SchedulerState,
) -> (ResumeReason, Vec<ResumeAction>) {
    match freshness {
        ProjectionFreshness::Fresh
            if scheduler.active_task_id.is_some() || scheduler.active_run_id.is_some() =>
        {
            (
                ResumeReason::InterruptedRun,
                vec![
                    ResumeAction::ResumeScheduler,
                    ResumeAction::RequeueInterruptedRun,
                ],
            )
        }
        ProjectionFreshness::Fresh => {
            let action = match phase_at_resume {
                WorkflowPhase::Intent => ResumeAction::ReopenIntentReview,
                WorkflowPhase::Planning => ResumeAction::ReopenPlanningReview,
                WorkflowPhase::Execution | WorkflowPhase::Validation | WorkflowPhase::Decision => {
                    ResumeAction::ResumeScheduler
                }
            };
            (ResumeReason::FreshThread, vec![action])
        }
        ProjectionFreshness::StaleReadOnly => (
            ResumeReason::ProjectionStale,
            vec![
                ResumeAction::TriggerTargetedRebuild,
                ResumeAction::OpenReadOnly,
            ],
        ),
        ProjectionFreshness::Unavailable => (
            ResumeReason::ProjectionUnavailable,
            vec![ResumeAction::BlockAutomaticResume],
        ),
    }
}

/// Construct a stale-marker `SchedulerState` for a thread whose
/// projection row is missing from the scheduler table.
///
/// The returned state has version 0 (so subsequent CAS writes know
/// they're starting fresh) and every active/selected field cleared.
/// Exposed at `pub(crate)` so the empty-shape contract is testable
/// without exercising the full DB load path.
pub(crate) fn empty_scheduler(thread_id: Uuid) -> SchedulerState {
    SchedulerState {
        thread_id,
        selected_plan_id: None,
        selected_plan_ids: Vec::new(),
        current_plan_heads: Vec::new(),
        active_task_id: None,
        active_run_id: None,
        live_context_window: Vec::new(),
        metadata: None,
        updated_at: chrono::Utc::now(),
        version: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `empty_scheduler(thread_id)` must produce a fully-cleared
    /// SchedulerState with version=0. Pin the "stale marker" shape
    /// so the read-side branch in `load_thread_bundle` (which returns
    /// this when the scheduler row is missing) keeps its contract.
    #[test]
    fn empty_scheduler_produces_fully_cleared_state_with_version_zero() {
        let thread_id = ThreadId::nil();
        let state = empty_scheduler(thread_id);

        assert_eq!(state.thread_id, thread_id);
        assert!(state.selected_plan_id.is_none());
        assert!(state.selected_plan_ids.is_empty());
        assert!(state.current_plan_heads.is_empty());
        assert!(state.active_task_id.is_none());
        assert!(state.active_run_id.is_none());
        assert!(state.live_context_window.is_empty());
        assert!(state.metadata.is_none());
        assert_eq!(state.version, 0);
    }

    /// `empty_scheduler` threads through the supplied thread_id —
    /// callers identify the stale row by this id alone.
    #[test]
    fn empty_scheduler_uses_supplied_thread_id() {
        let nil = ThreadId::nil();
        let other = Uuid::new_v4();
        assert_eq!(empty_scheduler(nil).thread_id, nil);
        assert_eq!(empty_scheduler(other).thread_id, other);
        assert_ne!(empty_scheduler(nil).thread_id, other);
    }

    /// `empty_scheduler` must always set `version = 0`, regardless of
    /// the input thread_id. Subsequent CAS writes use this to
    /// detect "this is a fresh row, not an update".
    #[test]
    fn empty_scheduler_always_sets_version_zero() {
        for raw in [Uuid::nil(), Uuid::new_v4(), Uuid::new_v4()] {
            assert_eq!(
                empty_scheduler(raw).version,
                0,
                "version must be 0 for thread {raw}",
            );
        }
    }

    /// `empty_scheduler` `updated_at` must be a non-zero timestamp
    /// (it reads `Utc::now()` at construction). Without this guard,
    /// a future refactor that uses `Default::default()` for
    /// `DateTime<Utc>` could silently emit the Unix epoch.
    #[test]
    fn empty_scheduler_updated_at_is_not_epoch() {
        let state = empty_scheduler(Uuid::nil());
        let epoch = chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).expect("valid epoch");
        assert_ne!(
            state.updated_at, epoch,
            "updated_at must reflect now, not the Unix epoch",
        );
    }

    /// `ProjectionResolver` is `Clone` so the runtime can hand
    /// independent handles to the orchestrator's observer + the read
    /// path. Verified via static type-system check; constructing a
    /// real `DatabaseConnection` would require sqlite setup beyond
    /// the scope of this unit test.
    #[test]
    fn projection_resolver_is_clone() {
        fn assert_clone<T: Clone>() {}
        assert_clone::<ProjectionResolver>();
    }

    /// `ThreadBundle` derives `Clone` + `PartialEq`. Verified via
    /// type-system check; the struct's `thread` and `scheduler`
    /// fields wrap heavy types whose construction in tests would
    /// require DB fixtures.
    #[test]
    fn thread_bundle_derives_clone_and_partial_eq() {
        fn assert_clone_eq<T: Clone + PartialEq>() {}
        assert_clone_eq::<ThreadBundle>();
    }
}
