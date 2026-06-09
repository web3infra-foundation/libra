//! Scheduler projection types for the Libra runtime layer.
//!
//! These projections capture mutable execution state derived from immutable
//! `Plan`, `Task`, `Run`, and `ContextFrame` history. They represent the
//! scheduler's current selection, active work, and live context window without
//! rewriting the underlying snapshot or event objects.
//!
//! The domain types stay alongside `SchedulerStateRepository` so the in-memory
//! scheduler contract and SQLite persistence contract evolve together.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeZone, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    EntityTrait, QueryFilter, QueryOrder, TransactionTrait, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::thread::ThreadId;
use crate::internal::model::{
    ai_live_context_window, ai_scheduler_plan_head, ai_scheduler_selected_plan, ai_scheduler_state,
};

/// Current scheduler view for one thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerState {
    /// Thread whose execution view this scheduler row belongs to.
    pub thread_id: ThreadId,
    /// Canonical Plan head currently selected for UI and execution decisions.
    pub selected_plan_id: Option<Uuid>,
    /// Ordered selected plan set. In the target runtime this is fixed as
    /// `[execution, test]`; `selected_plan_id` remains as a compatibility field.
    #[serde(default)]
    pub selected_plan_ids: Vec<PlanHeadRef>,
    /// Active Plan leaves that still exist in the current planning frontier.
    #[serde(default)]
    pub current_plan_heads: Vec<PlanHeadRef>,
    /// Task currently emphasized by the scheduler or UI, if any.
    pub active_task_id: Option<Uuid>,
    /// Live Run attempt currently executing within the thread, if any.
    pub active_run_id: Option<Uuid>,
    /// Ordered visible context frames that form the live working set.
    #[serde(default)]
    pub live_context_window: Vec<LiveContextFrameRef>,
    /// Optional projection-only scheduler hints or implementation metadata.
    pub metadata: Option<Value>,
    /// Last time Libra updated the scheduler projection.
    pub updated_at: DateTime<Utc>,
    /// Projection revision maintained for scheduler updates.
    pub version: i64,
}

/// Reference to one currently active Plan head in the scheduler frontier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanHeadRef {
    /// Plan snapshot that remains active in the current frontier.
    pub plan_id: Uuid,
    /// Stable order of the head within the projected frontier list.
    pub ordinal: i64,
}

/// One entry in the scheduler's live context window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveContextFrameRef {
    /// ContextFrame event currently exposed to the active runtime window.
    pub context_frame_id: Uuid,
    /// Stable position of the frame within the visible window.
    pub position: i64,
    /// Phase or subsystem that introduced the frame into the window.
    pub source_kind: LiveContextSourceKind,
    /// Optional reason the frame is pinned instead of being freely evicted.
    pub pin_kind: Option<LiveContextPinKind>,
    /// Time at which the frame entered the projected live window.
    pub inserted_at: DateTime<Utc>,
}

/// Source category for a frame in the live context window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveContextSourceKind {
    /// Frame came from Intent analysis during Phase 0.
    IntentAnalysis,
    /// Frame was added while building or revising a Plan.
    Planning,
    /// Frame was produced during task execution or tool use.
    Execution,
    /// Frame was added during validation, audit, or review work.
    Validation,
    /// Frame was inserted manually outside the automated workflow phases.
    Manual,
}

/// Pin reason for a live context frame that should remain visible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveContextPinKind {
    /// Seed material that anchors the initial working context.
    Seed,
    /// Checkpoint material preserved across execution transitions.
    Checkpoint,
    /// Manual operator pin that should survive normal window churn.
    Manual,
    /// System-level pin reserved for mandatory runtime context.
    System,
}

impl LiveContextSourceKind {
    fn as_row_value(&self) -> &'static str {
        match self {
            Self::IntentAnalysis => "intent_analysis",
            Self::Planning => "planning",
            Self::Execution => "execution",
            Self::Validation => "validation",
            Self::Manual => "manual",
        }
    }

    fn from_row_value(value: &str, thread_id: ThreadId) -> Result<Self> {
        match value {
            "intent_analysis" => Ok(Self::IntentAnalysis),
            "planning" => Ok(Self::Planning),
            "execution" => Ok(Self::Execution),
            "validation" => Ok(Self::Validation),
            "manual" => Ok(Self::Manual),
            other => bail!("Invalid live context source_kind '{other}' for thread {thread_id}"),
        }
    }
}

impl LiveContextPinKind {
    fn as_row_value(&self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::Checkpoint => "checkpoint",
            Self::Manual => "manual",
            Self::System => "system",
        }
    }

    fn from_row_value(value: &str, thread_id: ThreadId) -> Result<Self> {
        match value {
            "seed" => Ok(Self::Seed),
            "checkpoint" => Ok(Self::Checkpoint),
            "manual" => Ok(Self::Manual),
            "system" => Ok(Self::System),
            other => bail!("Invalid live context pin_kind '{other}' for thread {thread_id}"),
        }
    }
}

/// Repository for per-thread scheduler projection state.
impl SchedulerState {
    /// Ordinal convention: `current_plan_heads[ordinal=0]` is the
    /// **execution** plan head. Matches the ordering applied by
    /// `apply_scheduler_mutation(SetCurrentPlanHeads)` (Wave 1B v0.17.589)
    /// and by `SelectedPlanSet::ordered_ids` in the contracts surface.
    pub const EXECUTION_HEAD_ORDINAL: i64 = 0;

    /// Ordinal convention: `current_plan_heads[ordinal=1]` is the
    /// **test** plan head.
    pub const TEST_HEAD_ORDINAL: i64 = 1;

    /// The execution plan head, if `current_plan_heads` carries an entry
    /// at [`EXECUTION_HEAD_ORDINAL`](Self::EXECUTION_HEAD_ORDINAL).
    pub fn execution_head(&self) -> Option<&PlanHeadRef> {
        self.current_plan_heads
            .iter()
            .find(|head| head.ordinal == Self::EXECUTION_HEAD_ORDINAL)
    }

    /// The test plan head, if `current_plan_heads` carries an entry at
    /// [`TEST_HEAD_ORDINAL`](Self::TEST_HEAD_ORDINAL).
    pub fn test_head(&self) -> Option<&PlanHeadRef> {
        self.current_plan_heads
            .iter()
            .find(|head| head.ordinal == Self::TEST_HEAD_ORDINAL)
    }

    /// `true` when no task or run is currently active. Phase 2 dispatch
    /// uses this as the "ready to start the next task" precondition.
    pub fn is_idle(&self) -> bool {
        self.active_task_id.is_none() && self.active_run_id.is_none()
    }

    /// `true` when the thread has been seeded with at least one selected
    /// plan. Mutations that require a seeded thread (e.g.
    /// `StartStage`, `MarkTaskActive`) should fail-closed when this
    /// returns `false`.
    pub fn is_seeded(&self) -> bool {
        self.selected_plan_id.is_some() || !self.selected_plan_ids.is_empty()
    }
}

#[derive(Clone)]
pub struct SchedulerStateRepository {
    db: DatabaseConnection,
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerStateCasError {
    #[error("scheduler state for thread {thread_id} does not exist")]
    Missing { thread_id: ThreadId },
    #[error(
        "scheduler state version conflict for thread {thread_id}: expected {expected}, actual {actual:?}"
    )]
    VersionConflict {
        thread_id: ThreadId,
        expected: i64,
        actual: Option<i64>,
    },
    #[error(transparent)]
    Storage(#[from] anyhow::Error),
}

impl SchedulerStateRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn load(&self, thread_id: ThreadId) -> Result<Option<SchedulerState>> {
        Self::load_with_conn(&self.db, thread_id).await
    }

    pub async fn insert_initial(&self, state: &SchedulerState) -> Result<()> {
        let txn = self
            .db
            .begin()
            .await
            .context("Failed to start scheduler state insert transaction")?;
        insert_state_with_conn(&txn, state).await?;
        replace_child_rows_with_conn(&txn, state).await?;
        txn.commit()
            .await
            .context("Failed to commit scheduler state insert")?;
        Ok(())
    }

    pub async fn compare_and_swap(
        &self,
        expected_version: i64,
        next: &SchedulerState,
    ) -> Result<(), SchedulerStateCasError> {
        if next.version <= expected_version {
            return Err(SchedulerStateCasError::Storage(anyhow::anyhow!(
                "scheduler state CAS for thread {} must advance version above expected {}",
                next.thread_id,
                expected_version
            )));
        }

        let txn = self
            .db
            .begin()
            .await
            .context("Failed to start scheduler state CAS transaction")?;
        let thread_id = next.thread_id.to_string();
        let result = ai_scheduler_state::Entity::update_many()
            .col_expr(
                ai_scheduler_state::Column::SelectedPlanId,
                Expr::value(next.selected_plan_id.map(|id| id.to_string())),
            )
            .col_expr(
                ai_scheduler_state::Column::ActiveTaskId,
                Expr::value(next.active_task_id.map(|id| id.to_string())),
            )
            .col_expr(
                ai_scheduler_state::Column::ActiveRunId,
                Expr::value(next.active_run_id.map(|id| id.to_string())),
            )
            .col_expr(
                ai_scheduler_state::Column::MetadataJson,
                Expr::value(metadata_to_row(next.metadata.as_ref(), next.thread_id)?),
            )
            .col_expr(
                ai_scheduler_state::Column::Version,
                Expr::value(next.version),
            )
            .col_expr(
                ai_scheduler_state::Column::UpdatedAt,
                Expr::value(next.updated_at.timestamp()),
            )
            .filter(ai_scheduler_state::Column::ThreadId.eq(thread_id.clone()))
            .filter(ai_scheduler_state::Column::Version.eq(expected_version))
            .exec(&txn)
            .await
            .with_context(|| {
                format!(
                    "Failed to CAS scheduler state {} from version {}",
                    next.thread_id, expected_version
                )
            })?;

        if result.rows_affected != 1 {
            let actual = ai_scheduler_state::Entity::find_by_id(thread_id)
                .one(&txn)
                .await
                .with_context(|| {
                    format!(
                        "Failed to load scheduler state {} after CAS conflict",
                        next.thread_id
                    )
                })?
                .map(|row| row.version);
            return match actual {
                Some(actual) => Err(SchedulerStateCasError::VersionConflict {
                    thread_id: next.thread_id,
                    expected: expected_version,
                    actual: Some(actual),
                }),
                None => Err(SchedulerStateCasError::Missing {
                    thread_id: next.thread_id,
                }),
            };
        }

        replace_child_rows_with_conn(&txn, next).await?;
        txn.commit()
            .await
            .context("Failed to commit scheduler state CAS")?;
        Ok(())
    }

    pub(crate) async fn load_with_conn<C: ConnectionTrait>(
        db: &C,
        thread_id: ThreadId,
    ) -> Result<Option<SchedulerState>> {
        let Some(state) = ai_scheduler_state::Entity::find_by_id(thread_id.to_string())
            .one(db)
            .await
            .with_context(|| format!("Failed to query scheduler state for thread {thread_id}"))?
        else {
            return Ok(None);
        };

        let thread_id_text = thread_id.to_string();
        let current_plan_heads = ai_scheduler_plan_head::Entity::find()
            .filter(ai_scheduler_plan_head::Column::ThreadId.eq(thread_id_text.clone()))
            .order_by_asc(ai_scheduler_plan_head::Column::Ordinal)
            .all(db)
            .await
            .with_context(|| format!("Failed to load scheduler plan heads for {thread_id}"))?
            .into_iter()
            .map(|row| {
                Ok(PlanHeadRef {
                    plan_id: parse_uuid(&row.plan_id, thread_id, "plan_head.plan_id")?,
                    ordinal: row.ordinal,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mut selected_plan_ids = ai_scheduler_selected_plan::Entity::find()
            .filter(ai_scheduler_selected_plan::Column::ThreadId.eq(thread_id_text.clone()))
            .order_by_asc(ai_scheduler_selected_plan::Column::Ordinal)
            .all(db)
            .await
            .with_context(|| format!("Failed to load selected scheduler plans for {thread_id}"))?
            .into_iter()
            .map(|row| {
                Ok(PlanHeadRef {
                    plan_id: parse_uuid(&row.plan_id, thread_id, "selected_plan.plan_id")?,
                    ordinal: row.ordinal,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let selected_plan_id = state
            .selected_plan_id
            .as_deref()
            .map(|raw| parse_uuid(raw, thread_id, "selected_plan_id"))
            .transpose()?;
        if selected_plan_ids.is_empty()
            && let Some(plan_id) = selected_plan_id
        {
            selected_plan_ids.push(PlanHeadRef {
                plan_id,
                ordinal: 0,
            });
        }

        let live_context_window = ai_live_context_window::Entity::find()
            .filter(ai_live_context_window::Column::ThreadId.eq(thread_id_text))
            .order_by_asc(ai_live_context_window::Column::Position)
            .all(db)
            .await
            .with_context(|| format!("Failed to load live context window for {thread_id}"))?
            .into_iter()
            .map(|row| {
                Ok(LiveContextFrameRef {
                    context_frame_id: parse_uuid(
                        &row.context_frame_id,
                        thread_id,
                        "context_frame_id",
                    )?,
                    position: row.position,
                    source_kind: LiveContextSourceKind::from_row_value(
                        &row.source_kind,
                        thread_id,
                    )?,
                    pin_kind: row
                        .pin_kind
                        .as_deref()
                        .map(|value| LiveContextPinKind::from_row_value(value, thread_id))
                        .transpose()?,
                    inserted_at: timestamp_from_row(row.inserted_at, thread_id, "inserted_at")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Some(SchedulerState {
            thread_id,
            selected_plan_id,
            selected_plan_ids,
            current_plan_heads,
            active_task_id: state
                .active_task_id
                .as_deref()
                .map(|raw| parse_uuid(raw, thread_id, "active_task_id"))
                .transpose()?,
            active_run_id: state
                .active_run_id
                .as_deref()
                .map(|raw| parse_uuid(raw, thread_id, "active_run_id"))
                .transpose()?,
            live_context_window,
            metadata: metadata_from_row(state.metadata_json.as_deref(), thread_id)?,
            updated_at: timestamp_from_row(state.updated_at, thread_id, "updated_at")?,
            version: state.version,
        }))
    }
}

async fn insert_state_with_conn<C: ConnectionTrait>(db: &C, state: &SchedulerState) -> Result<()> {
    ai_scheduler_state::ActiveModel {
        thread_id: Set(state.thread_id.to_string()),
        selected_plan_id: Set(state.selected_plan_id.map(|id| id.to_string())),
        active_task_id: Set(state.active_task_id.map(|id| id.to_string())),
        active_run_id: Set(state.active_run_id.map(|id| id.to_string())),
        metadata_json: Set(metadata_to_row(state.metadata.as_ref(), state.thread_id)?),
        version: Set(state.version),
        updated_at: Set(state.updated_at.timestamp()),
    }
    .insert(db)
    .await
    .with_context(|| {
        format!(
            "Failed to insert scheduler state row for thread {}",
            state.thread_id
        )
    })?;
    Ok(())
}

async fn replace_child_rows_with_conn<C: ConnectionTrait>(
    db: &C,
    state: &SchedulerState,
) -> Result<()> {
    let thread_id = state.thread_id.to_string();

    ai_scheduler_plan_head::Entity::delete_many()
        .filter(ai_scheduler_plan_head::Column::ThreadId.eq(thread_id.clone()))
        .exec(db)
        .await
        .with_context(|| {
            format!(
                "Failed to clear scheduler plan heads for thread {}",
                state.thread_id
            )
        })?;
    ai_scheduler_selected_plan::Entity::delete_many()
        .filter(ai_scheduler_selected_plan::Column::ThreadId.eq(thread_id.clone()))
        .exec(db)
        .await
        .with_context(|| {
            format!(
                "Failed to clear selected scheduler plans for thread {}",
                state.thread_id
            )
        })?;
    ai_live_context_window::Entity::delete_many()
        .filter(ai_live_context_window::Column::ThreadId.eq(thread_id.clone()))
        .exec(db)
        .await
        .with_context(|| {
            format!(
                "Failed to clear live context window for thread {}",
                state.thread_id
            )
        })?;

    for plan_head in &state.current_plan_heads {
        ai_scheduler_plan_head::ActiveModel {
            thread_id: Set(thread_id.clone()),
            plan_id: Set(plan_head.plan_id.to_string()),
            ordinal: Set(plan_head.ordinal),
        }
        .insert(db)
        .await
        .with_context(|| {
            format!(
                "Failed to insert scheduler plan head {} for thread {}",
                plan_head.plan_id, state.thread_id
            )
        })?;
    }

    for selected_plan in &state.selected_plan_ids {
        ai_scheduler_selected_plan::ActiveModel {
            thread_id: Set(thread_id.clone()),
            plan_id: Set(selected_plan.plan_id.to_string()),
            ordinal: Set(selected_plan.ordinal),
        }
        .insert(db)
        .await
        .with_context(|| {
            format!(
                "Failed to insert selected scheduler plan {} for thread {}",
                selected_plan.plan_id, state.thread_id
            )
        })?;
    }

    for frame in &state.live_context_window {
        ai_live_context_window::ActiveModel {
            thread_id: Set(thread_id.clone()),
            context_frame_id: Set(frame.context_frame_id.to_string()),
            position: Set(frame.position),
            source_kind: Set(frame.source_kind.as_row_value().to_string()),
            pin_kind: Set(frame
                .pin_kind
                .as_ref()
                .map(|pin| pin.as_row_value().to_string())),
            inserted_at: Set(frame.inserted_at.timestamp()),
        }
        .insert(db)
        .await
        .with_context(|| {
            format!(
                "Failed to insert live context frame {} for thread {}",
                frame.context_frame_id, state.thread_id
            )
        })?;
    }

    Ok(())
}

fn parse_uuid(raw: &str, thread_id: ThreadId, field: &str) -> Result<Uuid> {
    Uuid::parse_str(raw)
        .with_context(|| format!("Invalid {field} UUID in scheduler state {thread_id}: {raw}"))
}

fn timestamp_from_row(raw: i64, thread_id: ThreadId, field: &str) -> Result<DateTime<Utc>> {
    Utc.timestamp_opt(raw, 0)
        .single()
        .with_context(|| format!("Invalid {field} timestamp in scheduler state {thread_id}: {raw}"))
}

fn metadata_to_row(metadata: Option<&Value>, thread_id: ThreadId) -> Result<Option<String>> {
    metadata
        .map(serde_json::to_string)
        .transpose()
        .with_context(|| format!("Failed to serialize scheduler metadata for {thread_id}"))
}

fn metadata_from_row(raw: Option<&str>, thread_id: ThreadId) -> Result<Option<Value>> {
    raw.map(serde_json::from_str)
        .transpose()
        .with_context(|| format!("Failed to parse scheduler metadata for {thread_id}"))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::{PlanHeadRef, SchedulerState, SchedulerStateCasError};

    fn empty_state(thread_id: Uuid) -> SchedulerState {
        SchedulerState {
            thread_id,
            selected_plan_id: None,
            selected_plan_ids: vec![],
            current_plan_heads: vec![],
            active_task_id: None,
            active_run_id: None,
            live_context_window: vec![],
            metadata: None,
            updated_at: Utc::now(),
            version: 0,
        }
    }

    /// `execution_head()` / `test_head()` must find entries by ordinal
    /// regardless of `Vec` ordering — the scheduler emits them sorted by
    /// ordinal today, but callers should not depend on positional access.
    #[test]
    fn head_helpers_locate_by_ordinal_not_position() {
        let mut state = empty_state(Uuid::new_v4());
        let exec_id = Uuid::new_v4();
        let test_id = Uuid::new_v4();
        // Deliberately reversed-order vector so positional access at [0]
        // would point at the test head — verifying the helpers don't
        // accidentally read `[0]`.
        state.current_plan_heads = vec![
            PlanHeadRef {
                plan_id: test_id,
                ordinal: SchedulerState::TEST_HEAD_ORDINAL,
            },
            PlanHeadRef {
                plan_id: exec_id,
                ordinal: SchedulerState::EXECUTION_HEAD_ORDINAL,
            },
        ];

        assert_eq!(
            state.execution_head().map(|h| h.plan_id),
            Some(exec_id),
            "execution_head() must locate the entry with ordinal=0",
        );
        assert_eq!(
            state.test_head().map(|h| h.plan_id),
            Some(test_id),
            "test_head() must locate the entry with ordinal=1",
        );
    }

    /// Empty `current_plan_heads` must return `None` from both helpers —
    /// callers will branch on these before issuing a Phase 2 dispatch.
    #[test]
    fn head_helpers_return_none_when_no_heads() {
        let state = empty_state(Uuid::new_v4());
        assert!(state.execution_head().is_none());
        assert!(state.test_head().is_none());
    }

    /// `is_idle()` is the "ready to start the next task" predicate:
    /// returns `true` only when both `active_task_id` and `active_run_id`
    /// are `None`.
    #[test]
    fn is_idle_requires_both_active_task_and_run_to_be_none() {
        let mut state = empty_state(Uuid::new_v4());
        assert!(state.is_idle(), "fresh state must be idle");

        state.active_task_id = Some(Uuid::new_v4());
        assert!(!state.is_idle(), "non-None active_task_id must clear idle");

        state.active_task_id = None;
        state.active_run_id = Some(Uuid::new_v4());
        assert!(!state.is_idle(), "non-None active_run_id must clear idle");

        state.active_task_id = Some(Uuid::new_v4());
        state.active_run_id = Some(Uuid::new_v4());
        assert!(!state.is_idle(), "both set must clear idle");
    }

    /// `is_seeded()` is `true` when either `selected_plan_id` is set or
    /// `selected_plan_ids` has at least one entry — matching the
    /// convention in `apply_scheduler_mutation(SetCurrentPlanHeads)`
    /// which mirrors `selected_plan_id` to the execution head while also
    /// populating `selected_plan_ids`.
    #[test]
    fn is_seeded_requires_either_legacy_or_ordered_selection() {
        let mut state = empty_state(Uuid::new_v4());
        assert!(!state.is_seeded(), "fresh state must not be seeded");

        state.selected_plan_id = Some(Uuid::new_v4());
        assert!(
            state.is_seeded(),
            "legacy selected_plan_id alone must flag as seeded",
        );

        state.selected_plan_id = None;
        state.selected_plan_ids = vec![PlanHeadRef {
            plan_id: Uuid::new_v4(),
            ordinal: 0,
        }];
        assert!(
            state.is_seeded(),
            "non-empty selected_plan_ids alone must flag as seeded",
        );
    }

    /// The ordinal constants must stay aligned with the convention used
    /// by `apply_scheduler_mutation(SetCurrentPlanHeads)` — execution at
    /// 0, test at 1. Pinning the values here protects against accidental
    /// re-ordering that would silently invert head dispatch.
    #[test]
    fn head_ordinals_are_pinned_constants() {
        assert_eq!(SchedulerState::EXECUTION_HEAD_ORDINAL, 0);
        assert_eq!(SchedulerState::TEST_HEAD_ORDINAL, 1);
    }

    #[test]
    fn scheduler_state_cas_error_display_pins_owned_variants() {
        let thread_id = Uuid::nil();
        assert_eq!(
            SchedulerStateCasError::Missing { thread_id }.to_string(),
            format!("scheduler state for thread {thread_id} does not exist"),
        );
        assert_eq!(
            SchedulerStateCasError::VersionConflict {
                thread_id,
                expected: 7,
                actual: Some(9),
            }
            .to_string(),
            format!(
                "scheduler state version conflict for thread {thread_id}: \
                 expected 7, actual Some(9)",
            ),
        );
        assert_eq!(
            SchedulerStateCasError::VersionConflict {
                thread_id,
                expected: 7,
                actual: None,
            }
            .to_string(),
            format!(
                "scheduler state version conflict for thread {thread_id}: \
                 expected 7, actual None",
            ),
        );
    }
}
