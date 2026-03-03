use std::{str::FromStr, sync::Arc};

use git_internal::internal::object::{ObjectTrait, blob::Blob};
use libra::utils::storage::{
    Storage, local::LocalStorage, remote::RemoteStorage, tiered::TieredStorage,
};
use object_store::memory::InMemory;
use tempfile::tempdir;

#[tokio::test]
async fn test_mock_remote_storage_with_repo_prefix() {
    let memory_store = Arc::new(InMemory::new());
    let remote_storage = RemoteStorage::new_with_prefix(memory_store, "repo-a".to_string());

    let blob = Blob::from_content("Hello Prefix!");
    let path = remote_storage
        .put(&blob.id, &blob.data, blob.get_type())
        .await
        .expect("Put failed");

    // Verify physical path isolation
    assert!(path.starts_with("repo-a/objects/"));
    assert!(remote_storage.exist(&blob.id).await);

    // Verify retrieval works via the abstraction
    let (data, _) = remote_storage.get(&blob.id).await.unwrap();
    assert_eq!(data, blob.data);
}

#[tokio::test]
async fn test_mock_tiered_storage_logic() {
    // 1. Setup Components
    let memory_store = Arc::new(InMemory::new());
    // Use repo prefix for tiered storage backend to ensure it propagates
    let remote = RemoteStorage::new_with_prefix(memory_store, "repo-tiered".to_string());

    let dir = tempdir().unwrap();
    let local = LocalStorage::new(dir.path().to_path_buf());

    // Threshold = 10 bytes.
    let threshold = 10;
    let disk_usage_limit = 1024; // Enough for test
    let tiered = TieredStorage::new(local.clone(), remote, threshold, disk_usage_limit);

    // 2. Test Small Object (Perma Store)
    let small_content = "123"; // 3 bytes < 10
    let small_blob = Blob::from_content(small_content);
    let path_small = tiered
        .put(&small_blob.id, &small_blob.data, small_blob.get_type())
        .await
        .expect("Put small failed");

    // Check path prefix in returned remote path (tiered.put returns remote path)
    assert!(path_small.starts_with("repo-tiered/objects/"));

    // Check Local (Should exist permanently)
    assert!(
        local.exist(&small_blob.id).await,
        "Small object should be in local storage"
    );

    // 3. Test Large Object (LRU Cache)
    let large_content = "123456789012345"; // 15 bytes > 10
    let large_blob = Blob::from_content(large_content);
    let path_large = tiered
        .put(&large_blob.id, &large_blob.data, large_blob.get_type())
        .await
        .expect("Put large failed");

    assert!(path_large.starts_with("repo-tiered/objects/"));

    // Check Local (Should exist in LRU/Local)
    assert!(
        local.exist(&large_blob.id).await,
        "Large object should be in local storage (cached)"
    );

    // 4. Verify Retrieval
    let (data, _) = tiered.get(&large_blob.id).await.expect("Get large failed");
    assert_eq!(data, large_blob.data);
}

#[tokio::test]
async fn test_mock_remote_search() {
    let memory_store = Arc::new(InMemory::new());
    // Search should work within the prefix
    let remote_storage = RemoteStorage::new_with_prefix(memory_store, "repo-search".to_string());

    // Create "aabbccdd..."
    let hash_str = "aabbccdd12345678901234567890123456789012";
    let hash = git_internal::hash::ObjectHash::from_str(hash_str).unwrap();
    let blob = Blob::from_content("search me");

    remote_storage
        .put(&hash, &blob.data, blob.get_type())
        .await
        .unwrap();

    // Test exact prefix "aabb" -> should match "aa/bb..." inside the repo prefix
    let res = remote_storage.search("aabb").await;
    assert_eq!(res.len(), 1);
    assert_eq!(res[0], hash);

    // Test short prefix "a" -> should match "aa/..."
    let res = remote_storage.search("a").await;
    assert_eq!(res.len(), 1);
    assert_eq!(res[0], hash);

    // Test non-matching
    let res = remote_storage.search("ccdd").await;
    assert!(res.is_empty());
}
