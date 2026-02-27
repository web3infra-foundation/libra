use std::{process::Command, str::FromStr, sync::Arc};

use git_internal::internal::object::{ObjectTrait, blob::Blob};
use libra::utils::{
    d1_client::{D1Client, D1Statement},
    storage::{Storage, local::LocalStorage, remote::RemoteStorage, tiered::TieredStorage},
};
use object_store::memory::InMemory;
use serial_test::serial;
use tempfile::tempdir;
use uuid::Uuid;

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("Missing required env var: {name}. See tests/cloud_storage_backup_test.rs header for setup.")
    })
}

fn d1_client_from_env() -> D1Client {
    D1Client::new(
        required_env("LIBRA_D1_ACCOUNT_ID"),
        required_env("LIBRA_D1_API_TOKEN"),
        required_env("LIBRA_D1_DATABASE_ID"),
    )
}

fn r2_storage_from_env(repo_id: &str) -> RemoteStorage {
    let endpoint = required_env("LIBRA_STORAGE_ENDPOINT");
    let bucket = required_env("LIBRA_STORAGE_BUCKET");
    let access_key = required_env("LIBRA_STORAGE_ACCESS_KEY");
    let secret_key = required_env("LIBRA_STORAGE_SECRET_KEY");
    let region = std::env::var("LIBRA_STORAGE_REGION").unwrap_or_else(|_| "auto".to_string());

    let s3 = object_store::aws::AmazonS3Builder::new()
        .with_bucket_name(bucket)
        .with_region(region)
        .with_endpoint(endpoint)
        .with_access_key_id(access_key)
        .with_secret_access_key(secret_key)
        .with_virtual_hosted_style_request(false)
        .build()
        .expect("Failed to build S3 client");

    RemoteStorage::new_with_prefix(Arc::new(s3), repo_id.to_string())
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(dir.path())
        .args(["init"])
        .output()
        .unwrap();
    assert!(output.status.success());
    dir
}

#[tokio::test]
async fn mock_remote_storage_basic() {
    let memory_store = Arc::new(InMemory::new());
    let remote_storage = RemoteStorage::new(memory_store);

    let blob = Blob::from_content("Hello Mock Storage!");
    let path = remote_storage
        .put(&blob.id, &blob.data, blob.get_type())
        .await
        .expect("Put failed");
    assert!(!path.is_empty());
    assert!(remote_storage.exist(&blob.id).await);

    let (data, obj_type) = remote_storage.get(&blob.id).await.expect("Get failed");
    assert_eq!(data, blob.data);
    assert_eq!(obj_type, blob.get_type());
}

#[tokio::test]
async fn mock_remote_storage_with_repo_prefix() {
    let memory_store = Arc::new(InMemory::new());
    let remote_storage = RemoteStorage::new_with_prefix(memory_store, "repo-a".to_string());

    let blob = Blob::from_content("Hello Prefix!");
    let path = remote_storage
        .put(&blob.id, &blob.data, blob.get_type())
        .await
        .expect("Put failed");

    assert!(path.starts_with("repo-a/objects/"));
    assert!(remote_storage.exist(&blob.id).await);
}

#[tokio::test]
async fn mock_tiered_storage_logic() {
    let memory_store = Arc::new(InMemory::new());
    let remote = RemoteStorage::new(memory_store);

    let dir = tempdir().unwrap();
    let local = LocalStorage::new(dir.path().to_path_buf());

    let tiered = TieredStorage::new(local.clone(), remote, 10, 1024);

    let small_blob = Blob::from_content("123");
    tiered
        .put(&small_blob.id, &small_blob.data, small_blob.get_type())
        .await
        .expect("Put small failed");
    assert!(local.exist(&small_blob.id).await);

    let large_blob = Blob::from_content("123456789012345");
    tiered
        .put(&large_blob.id, &large_blob.data, large_blob.get_type())
        .await
        .expect("Put large failed");
    assert!(local.exist(&large_blob.id).await);

    let (data, _) = tiered.get(&large_blob.id).await.expect("Get large failed");
    assert_eq!(data, large_blob.data);
}

#[tokio::test]
async fn mock_remote_search() {
    let memory_store = Arc::new(InMemory::new());
    let remote_storage = RemoteStorage::new(memory_store);

    let hash_str = "aabbccdd12345678901234567890123456789012";
    let hash = git_internal::hash::ObjectHash::from_str(hash_str).unwrap();
    let blob = Blob::from_content("search me");
    remote_storage
        .put(&hash, &blob.data, blob.get_type())
        .await
        .unwrap();

    let res = remote_storage.search("aabb").await;
    assert_eq!(res.len(), 1);
    assert_eq!(res[0], hash);

    let res = remote_storage.search("a").await;
    assert_eq!(res.len(), 1);
    assert_eq!(res[0], hash);

    let res = remote_storage.search("ccdd").await;
    assert!(res.is_empty());
}

#[test]
fn cloud_sync_fails_without_r2_env() {
    let dir = init_repo();
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(dir.path())
        .args(["cloud", "sync"])
        .env("LIBRA_D1_ACCOUNT_ID", "test-account")
        .env("LIBRA_D1_API_TOKEN", "test-token")
        .env("LIBRA_D1_DATABASE_ID", "test-db")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Cloud backup requires D1 + R2 configuration"));
    assert!(stderr.contains("LIBRA_STORAGE_ENDPOINT"));
}

#[test]
fn cloud_restore_fails_without_r2_env() {
    let dir = init_repo();
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(dir.path())
        .args(["cloud", "restore", "--repo-id", "test-repo"])
        .env("LIBRA_D1_ACCOUNT_ID", "test-account")
        .env("LIBRA_D1_API_TOKEN", "test-token")
        .env("LIBRA_D1_DATABASE_ID", "test-db")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Cloud backup requires D1 + R2 configuration"));
    assert!(stderr.contains("LIBRA_STORAGE_ENDPOINT"));
}

#[test]
fn cloud_sync_fails_without_d1_env() {
    let dir = init_repo();
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(dir.path())
        .args(["cloud", "sync"])
        .env("LIBRA_STORAGE_ENDPOINT", "https://example.invalid")
        .env("LIBRA_STORAGE_BUCKET", "test-bucket")
        .env("LIBRA_STORAGE_ACCESS_KEY", "test-access")
        .env("LIBRA_STORAGE_SECRET_KEY", "test-secret")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Cloud backup requires D1 + R2 configuration"));
    assert!(stderr.contains("LIBRA_D1_ACCOUNT_ID"));
}

#[tokio::test]
#[serial]
#[ignore]
async fn d1_connection() {
    let client = d1_client_from_env();
    let result = client.execute("SELECT 1 as test", None).await;
    assert!(result.is_ok(), "D1 connection failed: {:?}", result.err());
}

#[tokio::test]
#[serial]
#[ignore]
async fn d1_ensure_table() {
    let client = d1_client_from_env();
    let result = client.ensure_object_index_table().await;
    assert!(result.is_ok(), "Failed to create table: {:?}", result.err());
}

#[tokio::test]
#[serial]
#[ignore]
async fn d1_upsert_and_query() {
    let client = d1_client_from_env();
    client.ensure_object_index_table().await.unwrap();

    let test_hash = format!("test_hash_{}", chrono::Utc::now().timestamp());
    client
        .upsert_object_index(
            &test_hash,
            "blob",
            100,
            "test-repo-id",
            chrono::Utc::now().timestamp(),
        )
        .await
        .unwrap();

    let indexes = client.get_object_indexes("test-repo-id").await.unwrap();
    assert!(indexes.iter().any(|idx| idx.o_id == test_hash));
}

#[tokio::test]
#[serial]
#[ignore]
async fn d1_batch() {
    let client = d1_client_from_env();
    client.ensure_object_index_table().await.unwrap();

    let timestamp = chrono::Utc::now().timestamp();
    let statements: Vec<D1Statement> = (0..3)
        .map(|i| D1Statement {
            sql: "INSERT OR REPLACE INTO object_index (o_id, o_type, o_size, repo_id, created_at, is_synced) VALUES (?1, ?2, ?3, ?4, ?5, ?6)".to_string(),
            params: Some(vec![
                serde_json::json!(format!("batch_test_{}_{}", timestamp, i)),
                serde_json::json!("blob"),
                serde_json::json!(i * 100),
                serde_json::json!("batch-test-repo"),
                serde_json::json!(timestamp),
                serde_json::json!(1),
            ]),
        })
        .collect();

    let result = client.batch(statements).await;
    assert!(result.is_ok(), "Batch operation failed: {:?}", result.err());

    let indexes = client.get_object_indexes("batch-test-repo").await.unwrap();
    let batch_count = indexes
        .iter()
        .filter(|idx| idx.o_id.starts_with(&format!("batch_test_{}", timestamp)))
        .count();
    assert_eq!(batch_count, 3);
}

#[tokio::test]
#[serial]
#[ignore]
async fn r2_connection_basic() {
    let storage = r2_storage_from_env("cloud-backup-test");

    let content = format!("Test content {}", chrono::Utc::now().timestamp());
    let blob = Blob::from_content(&content);

    storage
        .put(&blob.id, &blob.data, blob.get_type())
        .await
        .unwrap();
    assert!(storage.exist(&blob.id).await);

    let (data, obj_type) = storage.get(&blob.id).await.expect("R2 get failed");
    assert_eq!(data, blob.data);
    assert_eq!(obj_type, blob.get_type());
}

#[tokio::test]
#[serial]
#[ignore]
async fn cloud_sync_workflow_r2_then_d1() {
    let d1_client = d1_client_from_env();
    let repo_id = "cloud-sync-test-repo";
    let r2_storage = r2_storage_from_env(repo_id);

    d1_client.ensure_object_index_table().await.unwrap();

    let content = format!("Sync test content {}", chrono::Utc::now().timestamp());
    let blob = Blob::from_content(&content);

    r2_storage
        .put(&blob.id, &blob.data, blob.get_type())
        .await
        .unwrap();
    d1_client
        .upsert_object_index(
            &blob.id.to_string(),
            "blob",
            blob.data.len() as i64,
            repo_id,
            chrono::Utc::now().timestamp(),
        )
        .await
        .unwrap();

    assert!(r2_storage.exist(&blob.id).await);

    let indexes = d1_client.get_object_indexes(repo_id).await.unwrap();
    assert!(indexes.iter().any(|idx| idx.o_id == blob.id.to_string()));
}

#[tokio::test]
#[serial]
#[ignore]
async fn cloud_full_workflow_end_to_end() {
    // Phase 1: Setup - Initialize two separate local repos
    let repo_a_dir = init_repo();
    let repo_b_dir = init_repo();
    let repo_a_path = repo_a_dir.path();
    let repo_b_path = repo_b_dir.path();

    // Generate unique repo IDs for isolation test
    let repo_id_a = format!("test-repo-a-{}", Uuid::new_v4());
    let repo_id_b = format!("test-repo-b-{}", Uuid::new_v4());

    // Configure repos with their IDs (simulate `libra init` behavior or config edit)
    // Here we manually inject them into the config via CLI or just pass them to commands
    // But `cloud sync` reads from config.
    // Let's use `libra config` to set them.

    let envs = [
        ("LIBRA_D1_ACCOUNT_ID", required_env("LIBRA_D1_ACCOUNT_ID")),
        ("LIBRA_D1_API_TOKEN", required_env("LIBRA_D1_API_TOKEN")),
        ("LIBRA_D1_DATABASE_ID", required_env("LIBRA_D1_DATABASE_ID")),
        (
            "LIBRA_STORAGE_ENDPOINT",
            required_env("LIBRA_STORAGE_ENDPOINT"),
        ),
        ("LIBRA_STORAGE_BUCKET", required_env("LIBRA_STORAGE_BUCKET")),
        (
            "LIBRA_STORAGE_ACCESS_KEY",
            required_env("LIBRA_STORAGE_ACCESS_KEY"),
        ),
        (
            "LIBRA_STORAGE_SECRET_KEY",
            required_env("LIBRA_STORAGE_SECRET_KEY"),
        ),
        ("LIBRA_STORAGE_REGION", "auto".to_string()),
    ];

    // Helper to run libra command
    let run_libra = |dir: &std::path::Path, args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
        cmd.current_dir(dir).args(args);
        for (k, v) in &envs {
            cmd.env(k, v);
        }
        let output = cmd.output().expect("Failed to execute libra");
        if !output.status.success() {
            eprintln!("Command failed: libra {}", args.join(" "));
            eprintln!("Stderr: {}", String::from_utf8_lossy(&output.stderr));
            panic!("Command failed");
        }
        output
    };

    // Set repo IDs using local scope
    // libra config expects: libra config --local libra.repoid <value>
    // Format: configuration.name.key = value
    // libra -> configuration, repoid -> key (no name component)
    run_libra(
        repo_a_path,
        &["config", "--local", "libra.repoid", &repo_id_a],
    );
    run_libra(
        repo_b_path,
        &["config", "--local", "libra.repoid", &repo_id_b],
    );

    // Phase 2: Create content in Repo A
    let file_a = repo_a_path.join("file_a.txt");
    std::fs::write(&file_a, "Content from Repo A").unwrap();
    run_libra(repo_a_path, &["add", "."]);
    run_libra(repo_a_path, &["commit", "-m", "Commit A"]);

    // Phase 3: Create content in Repo B (Same content -> Same Hash, Different Repo)
    let file_b = repo_b_path.join("file_b.txt");
    std::fs::write(&file_b, "Content from Repo A").unwrap(); // Intentionally same content
    run_libra(repo_b_path, &["add", "."]);
    run_libra(repo_b_path, &["commit", "-m", "Commit B (Same Content)"]);

    // Phase 4: Cloud Sync both repos
    run_libra(repo_a_path, &["cloud", "sync"]);
    run_libra(repo_b_path, &["cloud", "sync"]);

    // Phase 5: Verification (Direct D1/R2 check)
    let d1 = d1_client_from_env();
    let r2_a = r2_storage_from_env(&repo_id_a);
    let r2_b = r2_storage_from_env(&repo_id_b);

    // Verify D1 indexes exist for both
    let idx_a = d1.get_object_indexes(&repo_id_a).await.unwrap();
    let idx_b = d1.get_object_indexes(&repo_id_b).await.unwrap();

    assert!(!idx_a.is_empty(), "Repo A should have indexes");
    assert!(!idx_b.is_empty(), "Repo B should have indexes");

    // Verify Object Isolation in R2
    // We expect the blob (same hash) to exist in BOTH prefixes
    let mut blob_id_from_d1 = String::new();
    for idx in &idx_a {
        if idx.o_type == "blob" {
            blob_id_from_d1 = idx.o_id.clone();
            break;
        }
    }

    assert!(
        !blob_id_from_d1.is_empty(),
        "Repo A should have a blob in D1 index after sync"
    );
    // If D1 has a blob, use that hash for R2 check
    let check_hash = git_internal::hash::ObjectHash::from_str(&blob_id_from_d1).unwrap();

    assert!(
        r2_a.exist(&check_hash).await,
        "Blob {} should be in Repo A storage",
        check_hash
    );
    assert!(
        r2_b.exist(&check_hash).await,
        "Blob {} should be in Repo B storage",
        check_hash
    );

    // Phase 6: Restore Scenario
    // Simulate a fresh clone for Repo A
    let restore_dir = tempdir().unwrap();
    let restore_path = restore_dir.path();

    // Init empty
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(restore_path).arg("init");
    cmd.output().unwrap();

    // Restore from Cloud using Repo A's ID
    let mut restore_cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    restore_cmd
        .current_dir(restore_path)
        .args(["cloud", "restore", "--repo-id", &repo_id_a]);
    for (k, v) in &envs {
        restore_cmd.env(k, v);
    }
    let out = restore_cmd.output().unwrap();
    assert!(
        out.status.success(),
        "Restore failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Check if objects are in `.libra/objects`
    let objects_path = restore_path.join(".libra/objects");
    let local_store = LocalStorage::new(objects_path.clone());

    let exists = local_store.exist(&check_hash).await;

    assert!(exists, "Restored repo should have the blob {}", check_hash);
}

#[tokio::test]
#[serial]
#[ignore]
async fn cloud_restore_workflow_d1_then_r2() {
    let d1_client = d1_client_from_env();
    let repo_id = "cloud-sync-test-repo";
    let r2_storage = r2_storage_from_env(repo_id);

    let indexes = d1_client.get_object_indexes(repo_id).await.unwrap();
    if indexes.is_empty() {
        return;
    }

    for idx in indexes.iter().take(5) {
        let hash = match git_internal::hash::ObjectHash::from_bytes(
            &hex::decode(&idx.o_id).unwrap_or_default(),
        ) {
            Ok(h) => h,
            Err(_) => continue,
        };

        if let Ok((data, _)) = r2_storage.get(&hash).await {
            let computed = git_internal::hash::ObjectHash::from_type_and_data(
                git_internal::internal::object::types::ObjectType::Blob,
                &data,
            );
            assert_eq!(computed, hash);
        }
    }
}

#[tokio::test]
#[serial]
#[ignore]
async fn multi_repo_isolation_same_object_id() {
    let client = d1_client_from_env();
    client.ensure_object_index_table().await.unwrap();

    let repo_a = "multi-repo-a";
    let repo_b = "multi-repo-b";

    let r2_a = r2_storage_from_env(repo_a);
    let r2_b = r2_storage_from_env(repo_b);

    let blob = Blob::from_content("same object across repos");
    let ts = chrono::Utc::now().timestamp();

    r2_a.put(&blob.id, &blob.data, blob.get_type())
        .await
        .unwrap();
    r2_b.put(&blob.id, &blob.data, blob.get_type())
        .await
        .unwrap();

    client
        .upsert_object_index(
            &blob.id.to_string(),
            "blob",
            blob.data.len() as i64,
            repo_a,
            ts,
        )
        .await
        .unwrap();
    client
        .upsert_object_index(
            &blob.id.to_string(),
            "blob",
            blob.data.len() as i64,
            repo_b,
            ts,
        )
        .await
        .unwrap();

    assert!(r2_a.exist(&blob.id).await);
    assert!(r2_b.exist(&blob.id).await);

    let a = client.get_object_indexes(repo_a).await.unwrap();
    let b = client.get_object_indexes(repo_b).await.unwrap();
    assert!(a.iter().any(|idx| idx.o_id == blob.id.to_string()));
    assert!(b.iter().any(|idx| idx.o_id == blob.id.to_string()));
}
