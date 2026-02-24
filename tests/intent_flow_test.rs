use std::sync::Arc;

use git_internal::internal::object::{
    intent::{Intent, IntentStatus},
    task::Task,
    types::ActorRef,
};
use libra::{
    internal::ai::history::HistoryManager,
    utils::{storage::local::LocalStorage, storage_ext::StorageExt, test},
};
use tempfile::tempdir;

/// Integration test: Intent and Task objects share the single AI branch (refs/libra/intent).
///
/// Verifies:
/// 1. `init_branch()` creates the AI ref at startup.
/// 2. Intent objects are stored and retrievable from the AI branch.
/// 3. Task objects share the same AI branch.
/// 4. Both object types coexist under a single `refs/libra/intent` ref.
#[tokio::test]
async fn test_intent_flow() {
    // 1. Setup Storage and Repo Environment
    let dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(dir.path());

    test::setup_with_new_libra_in(dir.path()).await;

    let libra_dir = dir.path().join(".libra");
    let objects_dir = libra_dir.join("objects");

    let storage = Arc::new(LocalStorage::new(objects_dir));

    // Single AI History Manager — all AI objects go here
    let ai_history = HistoryManager::new(storage.clone(), libra_dir.clone());

    // 2. Verify init creates the AI ref (called during `libra init`)
    let ai_ref_path = libra_dir.join("refs/libra/intent");
    assert!(
        ai_ref_path.exists(),
        "AI ref file should be created during init at {:?}",
        ai_ref_path
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

    // 4. Create Child Intent
    let mut child_intent =
        Intent::new(actor.clone(), "Sub-goal: Move Intent struct to libra").expect("child intent");

    child_intent.set_parent(Some(root_intent.header().object_id()));
    child_intent.set_status(IntentStatus::Active);

    let child_hash = storage
        .put_tracked(&child_intent, &ai_history)
        .await
        .unwrap();
    println!("Stored Child Intent: {}", child_hash);

    // 5. Verify Retrieval
    let loaded_child: Intent = storage.get_json(&child_hash).await.unwrap();
    assert_eq!(
        loaded_child.header().object_id(),
        child_intent.header().object_id()
    );
    assert_eq!(
        loaded_child.parent(),
        Some(root_intent.header().object_id())
    );
    assert_eq!(loaded_child.prompt(), child_intent.prompt());
    assert_eq!(loaded_child.header().created_by().id(), "jackie");
    assert_eq!(loaded_child.status(), Some(&IntentStatus::Active));

    // 6. Create a Task object on the SAME branch
    let task = Task::new(ActorRef::human("me").unwrap(), "Main Task", None).unwrap();

    storage.put_tracked(&task, &ai_history).await.unwrap();

    // 7. Verify both Intent and Task coexist on the single AI branch
    let intents = ai_history.list_objects("intent").await.unwrap();
    assert_eq!(intents.len(), 2, "Should have 2 intents on AI branch");

    let tasks = ai_history.list_objects("task").await.unwrap();
    assert_eq!(tasks.len(), 1, "Should have 1 task on same AI branch");

    // 8. Verify HEAD is a single ref
    let head_hash = ai_history.resolve_history_head().await.unwrap().unwrap();
    println!("AI Branch HEAD: {}", head_hash);
}
