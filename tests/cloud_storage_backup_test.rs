//! Cloud backup and storage tests covering D1 metadata, R2 object storage, and full sync/restore workflows.
//!
//! Three concentric circles of coverage live here:
//! 1. Mock tests (`mock_*`) exercise `RemoteStorage`, `TieredStorage`, and `search`
//!    against `object_store::memory::InMemory` — fully deterministic.
//! 2. Configuration-error tests (`cloud_*_fails_without_*`) shell out to the real
//!    binary with one half of the cloud env vars deliberately missing, asserting
//!    we surface a precise actionable error mentioning the missing variable.
//! 3. Live cloud tests (`d1_*`, `r2_*`, `cloud_full_workflow_end_to_end`,
//!    `cloud_sync_name_conflict`) hit production Cloudflare D1 + R2.
//!
//! **Layer:** Mock + error-path tests are L1. Live tests are L3 — require
//! `LIBRA_D1_ACCOUNT_ID` and/or `LIBRA_STORAGE_ENDPOINT`. Skipped silently when
//! unset. Live tests use `#[serial(cloud_live)]` to avoid trampling each other
//! on shared D1/R2 resources.

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

/// Read an env var or panic with a pointer to the file header for setup instructions.
/// Used inside live-cloud tests after the gate condition has already confirmed the
/// variable is set, so a panic here genuinely indicates a partial cloud config.
fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("Missing required env var: {name}. See tests/cloud_storage_backup_test.rs header for setup.")
    })
}

/// Build a Cloudflare D1 client from `LIBRA_D1_*` env vars. Callers must have already
/// gated on `LIBRA_D1_ACCOUNT_ID` being set (see the live-cloud tests).
fn d1_client_from_env() -> D1Client {
    D1Client::new(
        required_env("LIBRA_D1_ACCOUNT_ID"),
        required_env("LIBRA_D1_API_TOKEN"),
        required_env("LIBRA_D1_DATABASE_ID"),
    )
}

/// Build a `RemoteStorage` pointing at the configured S3-compatible bucket, scoped to
/// `repo_id` so tests cannot trample each other's objects in shared infrastructure.
/// Defaults `LIBRA_STORAGE_REGION` to "auto" because R2 is region-less.
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

/// Initialize a new Libra repo in a temp dir using the actual binary, with a fully
/// isolated HOME / XDG_CONFIG_HOME / USERPROFILE so global user config cannot leak
/// in. Returns the `TempDir` (must stay alive — drop removes the on-disk repo).
fn init_repo() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    let home = dir.path().join(".home");
    let config_home = home.join(".config");
    std::fs::create_dir_all(&config_home).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(dir.path())
        .args(["init"])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home)
        .output()
        .unwrap();
    assert!(output.status.success());
    dir
}

/// Scenario: store a single blob through `RemoteStorage` backed by an in-memory
/// `object_store`, then exist-check and re-fetch it. Smoke-tests the
/// `Storage::put`/`exist`/`get` contract for the remote backend.
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

/// Scenario: when constructed with `new_with_prefix("repo-a")`, every put writes
/// under `repo-a/objects/...`. Pins the per-repo prefix isolation contract that the
/// cloud backup workflow depends on for multi-tenant safety.
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

/// Scenario: with a 10-byte threshold, a 3-byte blob and a 15-byte blob both end up
/// in local storage (small objects are stored permanently, large objects are LRU
/// cached locally) and the large blob remains retrievable through the tier
/// abstraction. Pins the dual-write semantics the production tiered backend relies
/// on.
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

/// Scenario: insert a blob with a known hex prefix and verify `search` returns a
/// match for full and partial prefixes (`"aabb"`, `"a"`) and an empty result for a
/// non-matching prefix (`"ccdd"`). Guards the prefix-search contract that the
/// `cloud restore` flow uses.
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

/// Scenario: invoke `libra cloud sync` with D1 env vars present but R2 absent and
/// confirm the binary exits non-zero with an error mentioning both
/// "Cloud backup requires D1 + R2 configuration" and the specific missing variable
/// `LIBRA_STORAGE_ENDPOINT`. Pins the actionable-error contract for partial config.
#[test]
fn cloud_sync_fails_without_r2_env() {
    let dir = init_repo();
    let home = dir.path().join(".home");
    let config_home = home.join(".config");
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(dir.path())
        .args(["cloud", "sync"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home)
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

/// Scenario: same as the sync variant but for `cloud restore` — when D1 is set and
/// R2 is missing, the binary surfaces "Cloud backup requires D1 + R2 configuration"
/// plus `LIBRA_STORAGE_ENDPOINT` so the user knows which variable to set.
#[test]
fn cloud_restore_fails_without_r2_env() {
    let dir = init_repo();
    let home = dir.path().join(".home");
    let config_home = home.join(".config");
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(dir.path())
        .args(["cloud", "restore", "--repo-id", "test-repo"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home)
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

/// Scenario: invoke `libra cloud sync` with R2 env vars present but D1 absent and
/// confirm the error mentions both "Cloud backup requires D1 + R2 configuration"
/// and the specific missing variable `LIBRA_D1_ACCOUNT_ID`.
#[test]
fn cloud_sync_fails_without_d1_env() {
    let dir = init_repo();
    let home = dir.path().join(".home");
    let config_home = home.join(".config");
    let output = Command::new(env!("CARGO_BIN_EXE_libra"))
        .current_dir(dir.path())
        .args(["cloud", "sync"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home)
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

/// Scenario: live D1 smoke test — submit `SELECT 1` to confirm the API token,
/// account ID, and database ID are wired correctly. Skipped silently when
/// `LIBRA_D1_ACCOUNT_ID` is unset.
#[tokio::test]
#[serial(cloud_live)]
async fn d1_connection() {
    if std::env::var("LIBRA_D1_ACCOUNT_ID").map_or(true, |v| v.is_empty()) {
        eprintln!("skipped (LIBRA_D1_ACCOUNT_ID not set)");
        return;
    }
    let client = d1_client_from_env();
    let result = client.execute("SELECT 1 as test", None).await;
    assert!(result.is_ok(), "D1 connection failed: {:?}", result.err());
}

/// Scenario: call `ensure_object_index_table` against live D1. Verifies the DDL
/// the cloud backup layer issues is accepted by the real database and is idempotent
/// (the test runs against a possibly-already-existing table). Skipped without D1
/// credentials.
#[tokio::test]
#[serial(cloud_live)]
async fn d1_ensure_table() {
    if std::env::var("LIBRA_D1_ACCOUNT_ID").map_or(true, |v| v.is_empty()) {
        eprintln!("skipped (LIBRA_D1_ACCOUNT_ID not set)");
        return;
    }
    let client = d1_client_from_env();
    let result = client.ensure_object_index_table().await;
    assert!(result.is_ok(), "Failed to create table: {:?}", result.err());
}

/// Scenario: against live D1, upsert one object index row using a timestamp-suffixed
/// hash and confirm `get_object_indexes` returns it. The timestamp suffix avoids
/// collisions across test runs that share the same D1 instance. Skipped without D1
/// credentials.
#[tokio::test]
#[serial(cloud_live)]
async fn d1_upsert_and_query() {
    if std::env::var("LIBRA_D1_ACCOUNT_ID").map_or(true, |v| v.is_empty()) {
        eprintln!("skipped (LIBRA_D1_ACCOUNT_ID not set)");
        return;
    }
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

/// Scenario: against live D1, execute three INSERT statements via the batch API and
/// confirm all three rows land. Pins the contract that `cloud sync` relies on when
/// pushing many object-index entries in one round trip. Skipped without D1
/// credentials.
#[tokio::test]
#[serial(cloud_live)]
async fn d1_batch() {
    if std::env::var("LIBRA_D1_ACCOUNT_ID").map_or(true, |v| v.is_empty()) {
        eprintln!("skipped (LIBRA_D1_ACCOUNT_ID not set)");
        return;
    }
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

/// Scenario: against live R2 (or any S3-compatible endpoint), put a blob, confirm
/// existence, and read it back. The content is timestamp-suffixed so concurrent or
/// repeated runs do not collide. Skipped without `LIBRA_STORAGE_ENDPOINT`.
#[tokio::test]
#[serial(cloud_live)]
async fn r2_connection_basic() {
    if std::env::var("LIBRA_STORAGE_ENDPOINT").map_or(true, |v| v.is_empty()) {
        eprintln!("skipped (LIBRA_STORAGE_ENDPOINT not set)");
        return;
    }
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

/// Scenario: end-to-end cloud backup against live D1 + R2. Two repos with distinct
/// `repo_id`s and `cloud.name`s commit a shared text file (intentionally same
/// content to test object dedup) plus a binary file (only in repo A). After
/// `cloud sync`, both R2 prefixes contain the shared blob (cross-repo dedup is NOT
/// enforced) and the binary is in repo A only. Restore both repos into fresh dirs:
/// repo A by `--repo-id` and repo B by `--name`, confirming both restore mechanisms.
/// The restored repo A's binary file is present in repo A's restore but NOT in repo
/// B's restore — proving repo isolation. Finally `libra config --get libra.repoid`
/// confirms the per-repo config also restored. The test configures a local author
/// identity in each isolated repo so it does not depend on the developer's global
/// `~/.libra/config.db`. Skipped without both D1 and R2 envs.
#[tokio::test]
#[serial(cloud_live)]
async fn cloud_full_workflow_end_to_end() {
    if std::env::var("LIBRA_D1_ACCOUNT_ID").map_or(true, |v| v.is_empty())
        || std::env::var("LIBRA_STORAGE_ENDPOINT").map_or(true, |v| v.is_empty())
    {
        eprintln!("skipped (LIBRA_D1_ACCOUNT_ID or LIBRA_STORAGE_ENDPOINT not set)");
        return;
    }
    // Setup - Initialize two separate local repos
    let repo_a_dir = init_repo();
    let repo_b_dir = init_repo();
    let repo_a_path = repo_a_dir.path();
    let repo_b_path = repo_b_dir.path();

    // Generate unique repo IDs for isolation test
    let repo_id_a = format!("test-repo-a-{}", Uuid::new_v4());
    let repo_id_b = format!("test-repo-b-{}", Uuid::new_v4());

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
        let home = dir.join(".home");
        let config_home = home.join(".config");
        std::fs::create_dir_all(&config_home).expect("failed to create isolated HOME");

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
        cmd.current_dir(dir)
            .args(args)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("USERPROFILE", &home);
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

    // Configure local commit identities. The test isolates HOME/XDG_CONFIG_HOME per
    // repo, so relying on a developer's global config would make the live cloud gate
    // fail before it reaches the D1/R2 behavior under test.
    for repo in [repo_a_path, repo_b_path] {
        run_libra(repo, &["config", "--local", "user.name", "Libra Test"]);
        run_libra(
            repo,
            &["config", "--local", "user.email", "libra@example.com"],
        );
        run_libra(repo, &["config", "--local", "vault.signing", "false"]);
    }

    // Set repo IDs using local scope
    // libra config expects: libra config --local libra.repoid <value>
    run_libra(
        repo_a_path,
        &["config", "--local", "libra.repoid", &repo_id_a],
    );
    run_libra(
        repo_b_path,
        &["config", "--local", "libra.repoid", &repo_id_b],
    );

    // Set cloud names for testing name-based restore
    let name_a = format!("end-to-end-test-a-{}", Uuid::new_v4());
    let name_b = format!("end-to-end-test-b-{}", Uuid::new_v4());
    run_libra(repo_a_path, &["config", "--local", "cloud.name", &name_a]);
    run_libra(repo_b_path, &["config", "--local", "cloud.name", &name_b]);

    // Create content in Repo A
    let file_a = repo_a_path.join("file_a.txt");
    std::fs::write(&file_a, "Content from Repo A").unwrap();

    // Add a binary file to test non-text content
    let bin_file_a = repo_a_path.join("logo.bin");
    let bin_content = vec![0u8, 15, 255, 10, 42]; // Simple binary signature
    std::fs::write(&bin_file_a, &bin_content).unwrap();

    run_libra(repo_a_path, &["add", "."]);
    run_libra(repo_a_path, &["commit", "-m", "Commit A"]);

    // Create content in Repo B (Same content -> Same Hash, Different Repo)
    let file_b = repo_b_path.join("file_b.txt");
    std::fs::write(&file_b, "Content from Repo A").unwrap(); // Intentionally same content
    run_libra(repo_b_path, &["add", "."]);
    run_libra(repo_b_path, &["commit", "-m", "Commit B (Same Content)"]);

    // Cloud Sync both repos
    run_libra(repo_a_path, &["cloud", "sync"]);
    run_libra(repo_b_path, &["cloud", "sync"]);

    // Verification (Direct D1/R2 check)
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
    use git_internal::internal::object::types::ObjectType;
    let blob_hash = git_internal::hash::ObjectHash::from_type_and_data(
        ObjectType::Blob,
        "Content from Repo A".as_bytes(),
    );
    let bin_hash = git_internal::hash::ObjectHash::from_type_and_data(
        ObjectType::Blob,
        &[0u8, 15, 255, 10, 42],
    );

    let blob_id_from_d1 = blob_hash.to_string();
    let bin_blob_id = bin_hash.to_string();

    // Verify D1 has these objects
    assert!(
        idx_a.iter().any(|idx| idx.o_id == blob_id_from_d1),
        "Repo A should have the text blob in D1"
    );
    assert!(
        idx_a.iter().any(|idx| idx.o_id == bin_blob_id),
        "Repo A should have the binary blob in D1"
    );

    assert!(
        r2_a.exist(&blob_hash).await,
        "Text Blob {} should be in Repo A storage",
        blob_hash
    );
    assert!(
        r2_a.exist(&bin_hash).await,
        "Binary Blob {} should be in Repo A storage",
        bin_hash
    );
    assert!(
        r2_b.exist(&blob_hash).await,
        "Text Blob {} should be in Repo B storage",
        blob_hash
    );

    // Restore Scenarios

    // Restore Repo A using ID (Legacy/Explicit ID method)
    let restore_dir_a = tempdir().unwrap();
    let restore_path_a = restore_dir_a.path();

    // Init empty
    let restore_home_a = restore_path_a.join(".home");
    let restore_config_a = restore_home_a.join(".config");
    std::fs::create_dir_all(&restore_config_a).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(restore_path_a)
        .args(["init"])
        .env("HOME", &restore_home_a)
        .env("XDG_CONFIG_HOME", &restore_config_a)
        .env("USERPROFILE", &restore_home_a);
    cmd.output().unwrap();

    // Restore from Cloud using Repo A's ID
    let mut restore_cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    restore_cmd
        .current_dir(restore_path_a)
        .args(["cloud", "restore", "--repo-id", &repo_id_a])
        .env("HOME", &restore_home_a)
        .env("XDG_CONFIG_HOME", &restore_config_a)
        .env("USERPROFILE", &restore_home_a);
    for (k, v) in &envs {
        restore_cmd.env(k, v);
    }
    let out = restore_cmd.output().unwrap();
    assert!(
        out.status.success(),
        "Restore A (by ID) failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Check if objects are in `.libra/objects`
    let objects_path_a = restore_path_a.join(".libra/objects");
    let local_store_a = LocalStorage::new(objects_path_a);
    assert!(
        local_store_a.exist(&blob_hash).await,
        "Restored repo A should have the text blob {}",
        blob_hash
    );
    assert!(
        local_store_a.exist(&bin_hash).await,
        "Restored repo A should have the binary blob {}",
        bin_hash
    );

    // Verify config was restored (repoid)
    // We can check by running `libra config --get libra.repoid`
    let config_out = run_libra(restore_path_a, &["config", "--get", "libra.repoid"]);
    let config_val = String::from_utf8_lossy(&config_out.stdout)
        .trim()
        .to_string();
    assert_eq!(
        config_val, repo_id_a,
        "Restored repo should have correct repo_id in config"
    );

    // Restore Repo B using Name (New method)
    let restore_dir_b = tempdir().unwrap();
    let restore_path_b = restore_dir_b.path();

    // Init empty
    let restore_home_b = restore_path_b.join(".home");
    let restore_config_b = restore_home_b.join(".config");
    std::fs::create_dir_all(&restore_config_b).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(restore_path_b)
        .args(["init"])
        .env("HOME", &restore_home_b)
        .env("XDG_CONFIG_HOME", &restore_config_b)
        .env("USERPROFILE", &restore_home_b);
    cmd.output().unwrap();

    // Restore from Cloud using Repo B's Name
    let mut restore_cmd_b = Command::new(env!("CARGO_BIN_EXE_libra"));
    restore_cmd_b
        .current_dir(restore_path_b)
        .args(["cloud", "restore", "--name", &name_b])
        .env("HOME", &restore_home_b)
        .env("XDG_CONFIG_HOME", &restore_config_b)
        .env("USERPROFILE", &restore_home_b);
    for (k, v) in &envs {
        restore_cmd_b.env(k, v);
    }
    let out_b = restore_cmd_b.output().unwrap();
    assert!(
        out_b.status.success(),
        "Restore B (by Name) failed: {}",
        String::from_utf8_lossy(&out_b.stderr)
    );

    // Check if objects are in `.libra/objects`
    let objects_path_b = restore_path_b.join(".libra/objects");
    let local_store_b = LocalStorage::new(objects_path_b);
    assert!(
        local_store_b.exist(&blob_hash).await,
        "Restored repo B should have the blob {}",
        blob_hash
    );

    // Verify binary blob (Repo A only) is NOT present
    assert!(
        !local_store_b.exist(&bin_hash).await,
        "Restored repo B should NOT have the binary blob {}",
        bin_hash
    );

    // Verify config (repoid)
    let config_out_b = run_libra(restore_path_b, &["config", "--get", "libra.repoid"]);
    let config_val_b = String::from_utf8_lossy(&config_out_b.stdout)
        .trim()
        .to_string();
    assert_eq!(
        config_val_b, repo_id_b,
        "Restored repo B should have correct repo_id"
    );
}

/// Scenario: two distinct repos request the same `cloud.name`. The first sync wins
/// and registers the name; the second sync must fail with a message mentioning
/// "already taken by another repository". Pins the cloud-name uniqueness contract
/// — the runtime cannot allow two repos to share a public-facing name. Skipped
/// without both D1 and R2 envs.
#[tokio::test]
#[serial(cloud_live)]
async fn cloud_sync_name_conflict() {
    if std::env::var("LIBRA_D1_ACCOUNT_ID").map_or(true, |v| v.is_empty())
        || std::env::var("LIBRA_STORAGE_ENDPOINT").map_or(true, |v| v.is_empty())
    {
        eprintln!("skipped (LIBRA_D1_ACCOUNT_ID or LIBRA_STORAGE_ENDPOINT not set)");
        return;
    }
    let repo_a = init_repo();
    let repo_b = init_repo();
    let cloud_name = format!("conflict-test-{}", Uuid::new_v4());

    // Repo A
    run_libra_cmd(
        repo_a.path(),
        &["config", "--local", "cloud.name", &cloud_name],
    );
    let file_a = repo_a.path().join("a.txt");
    std::fs::write(&file_a, "A").unwrap();
    run_libra_cmd(repo_a.path(), &["add", "."]);
    run_libra_cmd(repo_a.path(), &["commit", "-m", "A"]);
    let out_a = run_libra_cmd(repo_a.path(), &["cloud", "sync"]);
    assert!(
        out_a.status.success(),
        "Repo A sync failed: {}",
        String::from_utf8_lossy(&out_a.stderr)
    );

    // Repo B
    run_libra_cmd(
        repo_b.path(),
        &["config", "--local", "cloud.name", &cloud_name],
    );
    let file_b = repo_b.path().join("b.txt");
    std::fs::write(&file_b, "B").unwrap();
    run_libra_cmd(repo_b.path(), &["add", "."]);
    run_libra_cmd(repo_b.path(), &["commit", "-m", "B"]);
    let out_b = run_libra_cmd(repo_b.path(), &["cloud", "sync"]);

    assert!(
        !out_b.status.success(),
        "Repo B sync should fail due to name conflict"
    );
    let stderr = String::from_utf8_lossy(&out_b.stderr);
    assert!(
        stderr.contains("already taken by another repository"),
        "Error message mismatch: {}",
        stderr
    );
}

/// Spawn the real Libra binary with isolated HOME/XDG paths and the full set of
/// cloud env vars wired in. Used by the live-cloud workflow tests so each repo can
/// execute commands with a fresh global config but shared cloud credentials.
/// Panics if any required cloud env var is missing — callers must already have
/// gated on the live-cloud condition before invoking this.
fn run_libra_cmd(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    let home = dir.join(".home");
    let config_home = home.join(".config");
    std::fs::create_dir_all(&config_home).expect("failed to create isolated HOME");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_libra"));
    cmd.current_dir(dir)
        .args(args)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("USERPROFILE", &home);

    let env_vars = [
        "LIBRA_D1_ACCOUNT_ID",
        "LIBRA_D1_API_TOKEN",
        "LIBRA_D1_DATABASE_ID",
        "LIBRA_STORAGE_ENDPOINT",
        "LIBRA_STORAGE_BUCKET",
        "LIBRA_STORAGE_ACCESS_KEY",
        "LIBRA_STORAGE_SECRET_KEY",
    ];

    for var in env_vars {
        let val =
            std::env::var(var).unwrap_or_else(|_| panic!("Missing required env var: {}", var));
        cmd.env(var, val);
    }

    if std::env::var("LIBRA_STORAGE_REGION").map_or(true, |v| v.is_empty()) {
        cmd.env("LIBRA_STORAGE_REGION", "auto");
    } else {
        cmd.env(
            "LIBRA_STORAGE_REGION",
            std::env::var("LIBRA_STORAGE_REGION").unwrap(),
        );
    }

    cmd.output().expect("Failed to execute libra")
}
