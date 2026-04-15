//! Phase B projection resolver and scheduler repository contract tests.

use chrono::{DateTime, Utc};
use git_internal::internal::object::types::ActorRef;
use libra::internal::ai::{
    projection::{
        LiveContextFrameRef, LiveContextPinKind, LiveContextSourceKind, PlanHeadRef,
        ProjectionResolver, SchedulerState, SchedulerStateCasError, SchedulerStateRepository,
        ThreadIntentLinkReason, ThreadIntentRef, ThreadParticipant, ThreadParticipantRole,
        ThreadProjection,
    },
    runtime::contracts::ProjectionFreshness,
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
use serde_json::json;
use uuid::Uuid;

const BOOTSTRAP_SQL: &str = include_str!("../sql/sqlite_20260309_init.sql");

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

fn ts(seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(seconds, 0).unwrap()
}

fn id(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

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

#[tokio::test]
async fn scheduler_repository_loads_selected_plan_set_and_enforces_cas() {
    let db = setup_db().await;
    let thread_id = id("11111111-1111-4111-8111-111111111111");
    sample_thread(thread_id).create(&db).await.unwrap();

    let repo = SchedulerStateRepository::new(db.clone());
    repo.insert_initial(&sample_scheduler(thread_id)).await.unwrap();

    let loaded = repo.load(thread_id).await.unwrap().expect("scheduler state");
    assert_eq!(loaded.version, 1);
    assert_eq!(loaded.selected_plan_ids.len(), 2);
    assert_eq!(loaded.live_context_window[0].source_kind, LiveContextSourceKind::Planning);

    let mut next = loaded.clone();
    next.version = 2;
    next.active_task_id = Some(id("66666666-6666-4666-8666-666666666666"));
    repo.compare_and_swap(1, &next).await.unwrap();

    let after = repo.load(thread_id).await.unwrap().expect("updated scheduler state");
    assert_eq!(after.version, 2);
    assert_eq!(after.active_task_id, next.active_task_id);

    let stale = repo.compare_and_swap(1, &next).await.expect_err("stale CAS must fail");
    assert!(matches!(
        stale,
        SchedulerStateCasError::VersionConflict {
            expected: 1,
            actual: Some(2),
            ..
        }
    ));
}

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
