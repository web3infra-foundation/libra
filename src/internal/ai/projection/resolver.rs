//! Read-side projection resolver for code runtime resume and diagnostics.

use anyhow::{Context, Result};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ProjectionRebuilder, SchedulerState, SchedulerStateRepository, ThreadId, ThreadProjection,
};
use crate::internal::ai::runtime::contracts::{ProjectionFreshness, WorkflowPhase};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadBundle {
    pub thread: ThreadProjection,
    pub scheduler: SchedulerState,
    pub freshness: ProjectionFreshness,
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
