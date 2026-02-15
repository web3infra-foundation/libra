use std::sync::Arc;

use libra::{
    internal::ai::{history::HistoryManager, intent::Intent},
    utils::{storage::local::LocalStorage, storage_ext::StorageExt, test},
};
use tempfile::tempdir;

/// Integration test for the Intent parallel branch flow
#[tokio::test]
async fn test_intent_flow() {
    // 1. Setup Storage and Repo Environment
    let dir = tempdir().unwrap();
    let _guard = test::ChangeDirGuard::new(dir.path());

    test::setup_with_new_libra_in(dir.path()).await;

    let libra_dir = dir.path().join(".libra");
    let objects_dir = libra_dir.join("objects");

    let storage = Arc::new(LocalStorage::new(objects_dir));

    // Create dedicated Intent History Manager pointing to refs/libra/intent
    let intent_history =
        HistoryManager::new_with_ref(storage.clone(), libra_dir.clone(), "refs/libra/intent");

    // Create standard History Manager for comparison
    let main_history = HistoryManager::new(storage.clone(), libra_dir.clone());

    // 2. Create Root Intent
    let actor = git_internal::internal::object::types::ActorRef::human("jackie").unwrap();
    let root_intent = Intent::new(
        "Initial high-level goal: Refactor system".to_string(),
        None,
        None,
        Some(actor.clone()),
    );

    // Store and track in Intent History
    let root_hash = storage
        .put_tracked(&root_intent, &intent_history)
        .await
        .unwrap();
    println!("Stored Root Intent: {}", root_hash);

    // 3. Verify Intent Branch Creation
    let intent_ref_path = libra_dir.join("refs/libra/intent");
    assert!(
        intent_ref_path.exists(),
        "Intent ref file should be created at {:?}",
        intent_ref_path
    );

    // Verify Main History is NOT affected (orphan branch separation)
    let main_ref_path = libra_dir.join("refs/libra/history");
    assert!(
        !main_ref_path.exists(),
        "Main history ref should not exist yet"
    );

    // 4. Create Child Intent
    let child_intent = Intent::new(
        "Sub-goal: Move Intent struct to libra".to_string(),
        Some(root_intent.id),
        None,
        Some(actor.clone()),
    );

    let child_hash = storage
        .put_tracked(&child_intent, &intent_history)
        .await
        .unwrap();
    println!("Stored Child Intent: {}", child_hash);

    // 5. Verify Retrieval
    let loaded_child: Intent = storage.get_json(&child_hash).await.unwrap();
    assert_eq!(loaded_child.id, child_intent.id);
    assert_eq!(loaded_child.parent_id, Some(root_intent.id));
    assert_eq!(loaded_child.content, child_intent.content);
    assert_eq!(loaded_child.created_by.unwrap().id(), "jackie");
    use libra::internal::ai::intent::IntentStatus;
    assert_eq!(loaded_child.status, IntentStatus::Active);

    // 6. Verify "Parallel Main Line"
    // The intent ref should point to a commit that includes the child intent
    // We can resolve the head of the intent branch
    let head_hash = intent_history
        .resolve_history_head()
        .await
        .unwrap()
        .unwrap();
    println!("Intent Branch HEAD: {}", head_hash);

    // 7. Verify Main Branch Independence
    // Now create a normal task in main history
    let task = git_internal::internal::object::task::Task::new(
        uuid::Uuid::new_v4(),
        git_internal::internal::object::types::ActorRef::human("me").unwrap(),
        "Main Task",
        None,
    )
    .unwrap();

    storage.put_tracked(&task, &main_history).await.unwrap();

    assert!(main_ref_path.exists(), "Main history should now exist");

    let main_head = main_history.resolve_history_head().await.unwrap().unwrap();
    assert_ne!(
        head_hash, main_head,
        "Intent branch and Main history branch must be distinct"
    );
}
