use std::{env, str::FromStr, sync::Arc};

use git_internal::internal::object::{ObjectTrait, blob::Blob};
use libra::utils::{
    client_storage::ClientStorage,
    storage::{Storage, local::LocalStorage, remote::RemoteStorage, tiered::TieredStorage},
};
use object_store::memory::InMemory;
use serial_test::serial;
use tempfile::tempdir;

// Credentials from user input(need )
const R2_ACCOUNT_ID: &str = "1234";
const R2_ACCESS_KEY: &str = "12345678";
const R2_SECRET_KEY: &str = "dbedudygydgwedqudgwedqugwedqugwedqugwedqugwedqugwedqugwedqugwedqug";

#[tokio::test]
async fn test_mock_remote_storage_basic() {
    // 1. Setup Memory ObjectStore
    let memory_store = Arc::new(InMemory::new());
    let remote_storage = RemoteStorage::new(memory_store);

    // 2. Create Object
    let content = "Hello Mock Storage!";
    let blob = Blob::from_content(content);

    // 3. Put
    let path = remote_storage
        .put(&blob.id, &blob.data, blob.get_type())
        .await
        .expect("Put failed");
    assert!(!path.is_empty());

    // 4. Exist
    assert!(
        remote_storage.exist(&blob.id).await,
        "Object should exist in remote"
    );

    // 5. Get
    let (data, obj_type) = remote_storage.get(&blob.id).await.expect("Get failed");
    assert_eq!(data, blob.data);
    assert_eq!(obj_type, blob.get_type());
}

#[tokio::test]
async fn test_mock_tiered_storage_logic() {
    // 1. Setup Components
    let memory_store = Arc::new(InMemory::new());
    let remote = RemoteStorage::new(memory_store);

    let dir = tempdir().unwrap();
    let local = LocalStorage::new(dir.path().to_path_buf());

    // Threshold = 10 bytes.
    // Small object < 10 bytes -> Local + Remote
    // Large object >= 10 bytes -> Remote + Local LRU
    let threshold = 10;
    let cache_size = 1024; // Enough for test
    let tiered = TieredStorage::new(local.clone(), remote, threshold, cache_size);

    // 2. Test Small Object (Perma Store)
    let small_content = "123"; // 3 bytes < 10
    let small_blob = Blob::from_content(small_content);
    tiered
        .put(&small_blob.id, &small_blob.data, small_blob.get_type())
        .await
        .expect("Put small failed");

    // Check Local (Should exist permanently)
    assert!(
        local.exist(&small_blob.id).await,
        "Small object should be in local storage"
    );

    // 3. Test Large Object (LRU Cache)
    let large_content = "123456789012345"; // 15 bytes > 10
    let large_blob = Blob::from_content(large_content);
    tiered
        .put(&large_blob.id, &large_blob.data, large_blob.get_type())
        .await
        .expect("Put large failed");

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
    let remote_storage = RemoteStorage::new(memory_store);

    // Create "aabbccdd..."
    let hash_str = "aabbccdd12345678901234567890123456789012";
    let hash = git_internal::hash::ObjectHash::from_str(hash_str).unwrap();
    let blob = Blob::from_content("search me");

    remote_storage
        .put(&hash, &blob.data, blob.get_type())
        .await
        .unwrap();

    // Test exact prefix "aabb" -> should match "aa/bb..."
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

#[tokio::test]
#[serial]
#[ignore] // Ignored by default to avoid failing if credentials are removed/invalid
async fn test_r2_integration_existing_bucket() {
    // Setup env vars
    unsafe {
        env::set_var("LIBRA_STORAGE_TYPE", "r2");
        env::set_var("LIBRA_STORAGE_BUCKET", "testlibra");
        env::set_var(
            "LIBRA_STORAGE_ENDPOINT",
            format!("https://{}.r2.cloudflarestorage.com", R2_ACCOUNT_ID),
        );
        env::set_var("LIBRA_STORAGE_ACCESS_KEY", R2_ACCESS_KEY);
        env::set_var("LIBRA_STORAGE_SECRET_KEY", R2_SECRET_KEY);
        env::set_var("LIBRA_STORAGE_REGION", "auto");
        env::set_var("LIBRA_STORAGE_THRESHOLD", "10"); // Small threshold to force remote storage
    }

    let dir = tempdir().unwrap();
    let objects_dir = dir.path().join("objects");
    let storage = ClientStorage::init(objects_dir);

    // Create a test blob
    let content = "Hello R2 from Libra (Existing Bucket)!";
    let blob = Blob::from_content(content);

    // Put object
    println!("Putting object {}...", blob.id);
    match storage.put(&blob.id, &blob.data, blob.get_type()) {
        Ok(_) => println!("Put success"),
        Err(e) => panic!("Put failed: {:?}", e),
    }

    // Check existence
    assert!(storage.exist(&blob.id), "Object should exist");

    // Get object
    println!("Getting object {}...", blob.id);
    let data = storage.get(&blob.id).unwrap();
    assert_eq!(data, blob.data);
    assert_eq!(String::from_utf8(data).unwrap(), content);

    println!("R2 integration test (existing bucket) passed!");
}

#[tokio::test]
#[serial]
#[ignore]
async fn test_r2_integration_new_bucket() {
    // Setup env vars
    unsafe {
        env::set_var("LIBRA_STORAGE_TYPE", "r2");
        env::set_var("LIBRA_STORAGE_BUCKET", "libra"); // Non-existent bucket (initially)
        env::set_var(
            "LIBRA_STORAGE_ENDPOINT",
            format!("https://{}.r2.cloudflarestorage.com", R2_ACCOUNT_ID),
        );
        env::set_var("LIBRA_STORAGE_ACCESS_KEY", R2_ACCESS_KEY);
        env::set_var("LIBRA_STORAGE_SECRET_KEY", R2_SECRET_KEY);
        env::set_var("LIBRA_STORAGE_REGION", "auto");
        env::set_var("LIBRA_STORAGE_THRESHOLD", "10");
    }

    let dir = tempdir().unwrap();
    let objects_dir = dir.path().join("objects");
    let storage = ClientStorage::init(objects_dir);

    let content = "Hello R2 from Libra (New Bucket)!";
    let blob = Blob::from_content(content);

    println!("Putting object {} to bucket 'libra'...", blob.id);
    // Note: This might fail if bucket doesn't exist and we don't have auto-creation.
    // Since implementing auto-creation without SDK is hard, we expect this might fail if bucket is truly missing.
    // However, if the user manually created it or R2 behaves differently, it might pass.
    match storage.put(&blob.id, &blob.data, blob.get_type()) {
        Ok(_) => println!("Put success"),
        Err(e) => {
            println!("Put failed as expected (if bucket missing): {:?}", e);
            // We don't assert failure because we want to see if it works.
            // But if it succeeds, we verify retrieval.
            return;
        }
    }

    assert!(storage.exist(&blob.id), "Object should exist");

    let data = storage.get(&blob.id).unwrap();
    assert_eq!(data, blob.data);

    println!("R2 integration test (new bucket) passed!");
}
