//! Intent specification flow tests covering draft, resolve, validate, and repair stages.
//!
//! Pins the v0.7 contract that Intent, IntentEvent, and Task objects all live on the
//! single AI ref `refs/libra/intent` (now stored in SQLite), share a `HistoryManager`,
//! and round-trip through `LocalStorage::put_tracked` / `get_json`. Covers the parent
//! linkage between root and child intents, the `IntentEventKind::Analyzed` lifecycle
//! marker, and confirms Task and Intent coexist on the same branch.
//!
//! **Layer:** L1 — deterministic, no external dependencies. `#[serial]` because
//! `ChangeDirGuard` mutates the process CWD.

use std::sync::Arc;

use git_internal::internal::object::{
    intent::Intent,
    intent_event::{IntentEvent, IntentEventKind},
    task::Task,
    types::ActorRef,
};
use libra::{
    internal::ai::history::HistoryManager,
    utils::{storage::local::LocalStorage, storage_ext::StorageExt, test},
};
use serial_test::serial;
use tempfile::tempdir;

/// Integration test: Intent and Task objects share the single AI branch (refs/libra/intent).
///
/// Scenario: in a fresh temp-dir Libra repo, walk through the Intent flow end to end.
/// Asserts:
/// 1. `init` does NOT create the legacy on-disk `refs/libra/intent` file (it is in
///    the SQLite DB) and the legacy `refs/libra/history` file is also absent.
/// 2. Storing a root Intent and a child Intent (via `Intent::new_revision_from`)
///    succeeds and the child references the root via `parents()`.
/// 3. `IntentEvent::new` records lifecycle events (`Analyzed`) and round-trips
///    through `get_json`.
/// 4. A Task created via the same `HistoryManager` lands on the same branch — the
///    list filter sees 2 intents, 1 intent event, and 1 task.
/// 5. The AI branch HEAD resolves to a single hash after all writes.
///
/// `#[serial]` because `ChangeDirGuard` mutates process CWD.
#[tokio::test]
#[serial]
async fn test_intent_flow() {
    // 1. Setup Storage and Repo Environment
    let dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(dir.path());

    test::setup_with_new_libra_in(dir.path()).await;

    let libra_dir = dir.path().join(".libra");
    let objects_dir = libra_dir.join("objects");

    let storage = Arc::new(LocalStorage::new(objects_dir));

    // Single AI History Manager — all AI objects go here
    let db_path = libra_dir.join("libra.db");
    let db_conn = Arc::new(
        libra::internal::db::establish_connection(db_path.to_str().unwrap())
            .await
            .unwrap(),
    );
    let ai_history = HistoryManager::new(storage.clone(), libra_dir.clone(), db_conn);

    // 2. Verify init does NOT create the AI ref
    let ai_ref_path = libra_dir.join("refs/libra/intent");
    assert!(
        !ai_ref_path.exists(),
        "AI ref file should NOT be created during init at {:?}, it should be in DB",
        ai_ref_path
    );
    assert!(
        ai_history.resolve_history_head().await.unwrap().is_none(),
        "AI ref should be unborn (no commit) initially"
    );

    // The old history ref should NOT exist
    let old_history_ref = libra_dir.join("refs/libra/history");
    assert!(
        !old_history_ref.exists(),
        "Legacy refs/libra/history should not exist"
    );

    // 3. Create Root Intent
    let actor = ActorRef::human("jackie").unwrap();
    let root_intent = Intent::new(actor.clone(), "Initial high-level goal: Refactor system")
        .expect("root intent");

    let root_hash = storage
        .put_tracked(&root_intent, &ai_history)
        .await
        .unwrap();
    println!("Stored Root Intent: {}", root_hash);

    // 4. Create Child Intent (revision linked to root)
    let child_intent = Intent::new_revision_from(
        actor.clone(),
        "Sub-goal: Move Intent struct to libra",
        &root_intent,
    )
    .expect("child intent");

    let child_hash = storage
        .put_tracked(&child_intent, &ai_history)
        .await
        .unwrap();
    println!("Stored Child Intent: {}", child_hash);

    // 4.1 Record lifecycle via IntentEvent (0.7 model)
    let mut child_event = IntentEvent::new(
        actor.clone(),
        child_intent.header().object_id(),
        IntentEventKind::Analyzed,
    )
    .expect("intent event");
    child_event.set_reason(Some("analysis completed".to_string()));
    let child_event_hash = storage
        .put_tracked(&child_event, &ai_history)
        .await
        .unwrap();
    println!("Stored Child IntentEvent: {}", child_event_hash);

    // 5. Verify Retrieval
    let loaded_child: Intent = storage.get_json(&child_hash).await.unwrap();
    assert_eq!(
        loaded_child.header().object_id(),
        child_intent.header().object_id()
    );
    assert_eq!(loaded_child.parents(), &[root_intent.header().object_id()]);
    assert_eq!(loaded_child.prompt(), child_intent.prompt());
    assert_eq!(loaded_child.header().created_by().id(), "jackie");
    let loaded_event: IntentEvent = storage.get_json(&child_event_hash).await.unwrap();
    assert_eq!(loaded_event.intent_id(), child_intent.header().object_id());
    assert_eq!(loaded_event.kind(), &IntentEventKind::Analyzed);

    // 6. Create a Task object on the SAME branch
    let task = Task::new(ActorRef::human("me").unwrap(), "Main Task", None).unwrap();

    storage.put_tracked(&task, &ai_history).await.unwrap();

    // 7. Verify both Intent and Task coexist on the single AI branch
    let intents = ai_history.list_objects("intent").await.unwrap();
    assert_eq!(intents.len(), 2, "Should have 2 intents on AI branch");
    let intent_events = ai_history.list_objects("intent_event").await.unwrap();
    assert_eq!(
        intent_events.len(),
        1,
        "Should have 1 intent event on AI branch"
    );

    let tasks = ai_history.list_objects("task").await.unwrap();
    assert_eq!(tasks.len(), 1, "Should have 1 task on same AI branch");

    // 8. Verify HEAD is a single ref
    let head_hash = ai_history.resolve_history_head().await.unwrap().unwrap();
    println!("AI Branch HEAD: {}", head_hash);
}
