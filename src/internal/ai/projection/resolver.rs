//! Read-side projection resolver for code runtime resume and diagnostics.

use anyhow::{Context, Result};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{SchedulerState, SchedulerStateRepository, ThreadId, ThreadProjection};
use crate::internal::ai::runtime::contracts::ProjectionFreshness;

#[derive(Debug, Clone, PartialEq)]
pub struct ThreadBundle {
    pub thread: ThreadProjection,
    pub scheduler: SchedulerState,
    pub freshness: ProjectionFreshness,
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
}

fn empty_scheduler(thread_id: Uuid) -> SchedulerState {
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
