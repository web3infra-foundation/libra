//! Phase B projection resolver and scheduler repository contract tests.
//!
//! Pin the contracts that `ProjectionResolver`, `SchedulerStateRepository`, and
//! `ProjectionRebuilder` expose to the AI runtime:
//! - The scheduler repository round-trips a `SelectedPlanSet` and rejects stale
//!   compare-and-swap attempts via `SchedulerStateCasError::VersionConflict`.
//! - When a thread row exists but no scheduler row does, the resolver returns a
//!   `StaleReadOnly` bundle so callers know not to write to it.
//! - When neither row exists, the rebuilder can reconstruct a fresh bundle from the
//!   on-disk history (Intent + Task objects).
//!
//! **Layer:** L1 — uses an in-memory SQLite plus a temp-dir Libra repo. The history
//! rebuild test mutates CWD via `ChangeDirGuard` and is therefore `#[serial]`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use git_internal::internal::object::{intent::Intent, task::Task, types::ActorRef};
use libra::{
    internal::{
        ai::{
            history::HistoryManager,
            projection::{
                LiveContextFrameRef, LiveContextPinKind, LiveContextSourceKind, PlanHeadRef,
                ProjectionRebuilder, ProjectionResolver, SchedulerState, SchedulerStateCasError,
                SchedulerStateRepository, ThreadIntentLinkReason, ThreadIntentRef,
                ThreadParticipant, ThreadParticipantRole, ThreadProjection,
            },
            runtime::contracts::ProjectionFreshness,
        },
        db,
    },
    utils::{storage::local::LocalStorage, storage_ext::StorageExt, test},
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
use serde_json::json;
use serial_test::serial;
use tempfile::tempdir;
use uuid::Uuid;

const BOOTSTRAP_SQL: &str = include_str!("../sql/sqlite_20260309_init.sql");

/// Spin up an in-memory SQLite, run the canonical bootstrap SQL, and return the
/// connection. Used by tests that only need the schema (not a full Libra repo on
/// disk).
async fn setup_db() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    db.execute(Statement::from_string(
        db.get_database_backend(),
        BOOTSTRAP_SQL,
    ))
    .await
    .unwrap();
    db
}

/// Provision a temp-dir Libra repository, switch CWD into it, and wire up the
/// `LocalStorage` + `HistoryManager` + DB connection that the rebuild test exercises.
///
/// Must be called from inside a `#[serial]` test because `ChangeDirGuard` mutates the
/// process-wide CWD. The returned `TempDir` must be held alive for the duration of
/// the test — dropping it removes the on-disk repo.
async fn setup_projection_history() -> (
    tempfile::TempDir,
    Arc<LocalStorage>,
    HistoryManager,
    Arc<DatabaseConnection>,
) {
    let dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(dir.path());
    test::setup_with_new_libra_in(dir.path()).await;

    let libra_dir = dir.path().join(".libra");
    let storage = Arc::new(LocalStorage::new(libra_dir.join("objects")));
    let db_conn = Arc::new(
        db::establish_connection(libra_dir.join("libra.db").to_str().unwrap())
            .await
            .unwrap(),
    );
    let history = HistoryManager::new(storage.clone(), libra_dir, db_conn.clone());
    (dir, storage, history, db_conn)
}

/// Build a deterministic UTC timestamp from a Unix-seconds value for fixtures.
fn ts(seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(seconds, 0).unwrap()
}

/// Parse a hard-coded UUID literal used in fixtures. Panics on malformed input — the
/// literals are author-owned so this is a programming-error fast path.
fn id(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

/// Construct a minimal `ThreadProjection` fixture rooted at `thread_id` with one
/// intent and a single owner participant. Used as the precondition row when testing
/// the scheduler/resolver: the thread must exist before either component reads.
fn sample_thread(thread_id: Uuid) -> ThreadProjection {
    let owner = ActorRef::human("projection-test").unwrap();
    let intent_id = id("22222222-2222-4222-8222-222222222222");
    ThreadProjection {
        thread_id,
        title: Some("Projection resolver test".to_string()),
        owner: owner.clone(),
        participants: vec![ThreadParticipant {
            actor: owner,
            role: ThreadParticipantRole::Owner,
            joined_at: ts(1_700_000_000),
        }],
        current_intent_id: Some(intent_id),
        latest_intent_id: Some(intent_id),
        intents: vec![ThreadIntentRef {
            intent_id,
            ordinal: 0,
            is_head: true,
            linked_at: ts(1_700_000_001),
            link_reason: ThreadIntentLinkReason::Seed,
        }],
        metadata: Some(json!({ "source": "test" })),
        archived: false,
        created_at: ts(1_700_000_000),
        updated_at: ts(1_700_000_002),
        version: 1,
    }
}

/// Construct a minimal `SchedulerState` fixture: two selected plans (an execution
/// plan and a test plan) with the execution plan as the current head, plus one
/// pinned live-context frame. Mirrors the shape the runtime persists at v1.
fn sample_scheduler(thread_id: Uuid) -> SchedulerState {
    let execution_plan_id = id("33333333-3333-4333-8333-333333333333");
    let test_plan_id = id("44444444-4444-4444-8444-444444444444");
    SchedulerState {
        thread_id,
        selected_plan_id: Some(execution_plan_id),
        selected_plan_ids: vec![
            PlanHeadRef {
                plan_id: execution_plan_id,
                ordinal: 0,
            },
            PlanHeadRef {
                plan_id: test_plan_id,
                ordinal: 1,
            },
        ],
        current_plan_heads: vec![PlanHeadRef {
            plan_id: execution_plan_id,
            ordinal: 0,
        }],
        active_task_id: None,
        active_run_id: None,
        live_context_window: vec![LiveContextFrameRef {
            context_frame_id: id("55555555-5555-4555-8555-555555555555"),
            position: 0,
            source_kind: LiveContextSourceKind::Planning,
            pin_kind: Some(LiveContextPinKind::Seed),
            inserted_at: ts(1_700_000_003),
        }],
        metadata: Some(json!({ "ready_queue": [] })),
        updated_at: ts(1_700_000_004),
        version: 1,
    }
}

/// Scenario: insert a `SchedulerState` at version 1, load it back, then perform two
/// compare-and-swap attempts — one valid (1 → 2) and one stale (1 → next, after the
/// row is already at 2). Asserts the load preserves the selected-plan set ordering
/// and the live-context source kind, and that the stale CAS surfaces a
/// `VersionConflict { expected: 1, actual: Some(2) }` so callers can retry the read.
#[tokio::test]
async fn scheduler_repository_loads_selected_plan_set_and_enforces_cas() {
    let db = setup_db().await;
    let thread_id = id("11111111-1111-4111-8111-111111111111");
    sample_thread(thread_id).create(&db).await.unwrap();

    let repo = SchedulerStateRepository::new(db.clone());
    repo.insert_initial(&sample_scheduler(thread_id))
        .await
        .unwrap();

    let loaded = repo
        .load(thread_id)
        .await
        .unwrap()
        .expect("scheduler state");
    assert_eq!(loaded.version, 1);
    assert_eq!(loaded.selected_plan_ids.len(), 2);
    assert_eq!(
        loaded.live_context_window[0].source_kind,
        LiveContextSourceKind::Planning
    );

    let mut next = loaded.clone();
    next.version = 2;
    next.active_task_id = Some(id("66666666-6666-4666-8666-666666666666"));
    repo.compare_and_swap(1, &next).await.unwrap();

    let after = repo
        .load(thread_id)
        .await
        .unwrap()
        .expect("updated scheduler state");
    assert_eq!(after.version, 2);
    assert_eq!(after.active_task_id, next.active_task_id);

    let stale = repo
        .compare_and_swap(1, &next)
        .await
        .expect_err("stale CAS must fail");
    assert!(matches!(
        stale,
        SchedulerStateCasError::VersionConflict {
            expected: 1,
            actual: Some(2),
            ..
        }
    ));
}

/// Scenario: when a thread projection row exists but its scheduler row is missing,
/// `ProjectionResolver::load_thread_bundle` must return a `StaleReadOnly` bundle with
/// an empty selected-plan set rather than an error. This is the contract the runtime
/// relies on to know when callers should not perform writes.
#[tokio::test]
async fn projection_resolver_returns_stale_read_only_when_scheduler_row_is_missing() {
    let db = setup_db().await;
    let thread_id = id("77777777-7777-4777-8777-777777777777");
    sample_thread(thread_id).create(&db).await.unwrap();

    let resolver = ProjectionResolver::new(db);
    let bundle = resolver
        .load_thread_bundle(thread_id)
        .await
        .unwrap()
        .expect("thread bundle");

    assert_eq!(bundle.thread.thread_id, thread_id);
    assert_eq!(bundle.freshness, ProjectionFreshness::StaleReadOnly);
    assert!(bundle.scheduler.selected_plan_ids.is_empty());
}

/// Scenario: when neither projection nor scheduler rows exist but the underlying
/// history has Intent + Task objects, `load_or_rebuild_thread_bundle` reconstructs a
/// `Fresh` bundle from the AI history. Confirms the rebuild path is reachable end to
/// end (storage put_tracked → resolver → rebuilder), and that the rebuilt bundle
/// carries the right thread/intent identity.
///
/// `#[serial]` because `ChangeDirGuard` mutates process CWD.
#[tokio::test]
#[serial]
async fn projection_resolver_rebuilds_missing_thread_projection_from_history() {
    let (_dir, storage, history, db_conn) = setup_projection_history().await;
    let actor = ActorRef::human("projection-rebuild").unwrap();
    let intent = Intent::new(actor.clone(), "Recover missing projection").unwrap();
    storage.put_tracked(&intent, &history).await.unwrap();
    let mut task = Task::new(actor, "Recovered task", None).unwrap();
    task.set_intent(Some(intent.header().object_id()));
    storage.put_tracked(&task, &history).await.unwrap();

    let resolver = ProjectionResolver::new(db_conn.as_ref().clone());
    assert!(
        resolver
            .load_thread_bundle(intent.header().object_id())
            .await
            .unwrap()
            .is_none()
    );

    let rebuilder = ProjectionRebuilder::new(storage.as_ref(), &history);
    let bundle = resolver
        .load_or_rebuild_thread_bundle(intent.header().object_id(), &rebuilder)
        .await
        .unwrap()
        .expect("rebuilt thread bundle");

    assert_eq!(bundle.freshness, ProjectionFreshness::Fresh);
    assert_eq!(bundle.thread.thread_id, intent.header().object_id());
    assert_eq!(bundle.thread.intents.len(), 1);
    assert_eq!(bundle.scheduler.thread_id, intent.header().object_id());
}
