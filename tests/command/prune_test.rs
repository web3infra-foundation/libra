//! Integration tests for the `prune` command.
//!
//! **Layer:** L1 — deterministic, no external dependencies.

use std::{
    fs,
    path::{Path, PathBuf},
};

use git_internal::{
    hash::{HashKind, ObjectHash, set_hash_kind_for_test},
    internal::object::{
        blob::Blob,
        commit::Commit,
        signature::{Signature, SignatureType},
        tag::Tag,
        tree::{Tree, TreeItem, TreeItemMode},
        types::ObjectType,
    },
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serial_test::serial;
use tempfile::tempdir;

use super::*;

/// Resolve the current HEAD commit hash using the CLI.
fn head_commit_hash(repo: &Path) -> String {
    let output = run_libra_command(&["log", "--pretty=%H", "-n", "1"], repo);
    assert_cli_success(&output, "failed to read HEAD commit hash");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Return true if the loose object file exists on disk.
fn object_exists(repo: &Path, hash: &str) -> bool {
    loose_object_path(repo, hash).exists()
}

/// Create a loose blob that is not referenced by any refs, index, or reflog.
fn create_unreachable_blob(repo: &Path, content: &str) -> ObjectHash {
    let _guard = ChangeDirGuard::new(repo);
    let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
    let blob = Blob::from_content(content);
    save_object(&blob, &blob.id).expect("failed to save blob");
    blob.id
}

/// Resolve the repository id stored in local config.
fn repo_id(repo: &Path) -> String {
    let output = run_libra_command(&["config", "--get", "libra.repoid"], repo);
    assert_cli_success(&output, "failed to read libra.repoid");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Enable the repository cloud-backup config path used by prune protections.
fn enable_cloud_backup(repo: &Path) {
    let output = run_libra_command(
        &["config", "set", "vault.env.LIBRA_STORAGE_TYPE", "r2"],
        repo,
    );
    assert_cli_success(&output, "failed to set cloud storage type");
}

/// Insert an object_index row for the provided object.
fn insert_object_index_row(repo: &Path, hash: &ObjectHash, repo_id: &str, is_synced: i32) {
    let _guard = ChangeDirGuard::new(repo);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let db_conn = libra::internal::db::get_db_conn_instance().await;
        let model = libra::internal::model::object_index::ActiveModel {
            o_id: Set(hash.to_string()),
            o_type: Set("blob".to_string()),
            o_size: Set(1),
            repo_id: Set(repo_id.to_string()),
            created_at: Set(1),
            is_synced: Set(is_synced),
            ..Default::default()
        };
        model.insert(&db_conn).await.expect("insert object_index");
    });
}

/// Return whether an object_index row exists for the provided object and repo.
fn object_index_row_exists(repo: &Path, hash: &ObjectHash, repo_id: &str) -> bool {
    let _guard = ChangeDirGuard::new(repo);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let db_conn = libra::internal::db::get_db_conn_instance().await;
        libra::internal::model::object_index::Entity::find()
            .filter(libra::internal::model::object_index::Column::OId.eq(hash.to_string()))
            .filter(libra::internal::model::object_index::Column::RepoId.eq(repo_id))
            .one(&db_conn)
            .await
            .expect("query object_index")
            .is_some()
    })
}

/// Create an unreachable commit (and its tree/blob) stored as loose objects.
fn create_unreachable_commit(repo: &Path, label: &str) -> (ObjectHash, ObjectHash, ObjectHash) {
    let _guard = ChangeDirGuard::new(repo);
    let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
    let blob = Blob::from_content_bytes(format!("{label}-blob").into_bytes());
    save_object(&blob, &blob.id).expect("failed to save blob");

    let tree = Tree::from_tree_items(vec![TreeItem::new(
        TreeItemMode::Blob,
        blob.id,
        "file.txt".to_string(),
    )])
    .expect("failed to build tree");
    save_object(&tree, &tree.id).expect("failed to save tree");

    let commit = Commit::from_tree_id(tree.id, Vec::new(), label);
    save_object(&commit, &commit.id).expect("failed to save commit");

    (commit.id, tree.id, blob.id)
}

/// Write a minimal v2 pack index file that list_idx_objects can parse.
fn write_fake_idx_v2(repo: &Path, hashes: &[ObjectHash]) -> PathBuf {
    let pack_dir = repo
        .join(libra::utils::util::ROOT_DIR)
        .join("objects")
        .join("pack");
    fs::create_dir_all(&pack_dir).expect("failed to create pack dir");
    let idx_path = pack_dir.join("pack-test-v2.idx");

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0xFF, 0x74, 0x4F, 0x63]);
    bytes.extend_from_slice(&2u32.to_be_bytes());
    bytes.extend_from_slice(&vec![0u8; 255 * 4]);
    bytes.extend_from_slice(&(hashes.len() as u32).to_be_bytes());
    for hash in hashes {
        bytes.extend_from_slice(hash.as_ref());
    }

    fs::write(&idx_path, bytes).expect("failed to write v2 idx");
    idx_path
}

/// Write a minimal v1 pack index file that list_idx_objects can parse.
fn write_fake_idx_v1(repo: &Path, hashes: &[ObjectHash]) -> PathBuf {
    let pack_dir = repo
        .join(libra::utils::util::ROOT_DIR)
        .join("objects")
        .join("pack");
    fs::create_dir_all(&pack_dir).expect("failed to create pack dir");
    let idx_path = pack_dir.join("pack-test-v1.idx");

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&vec![0u8; 255 * 4]);
    bytes.extend_from_slice(&(hashes.len() as u32).to_be_bytes());
    for (idx, hash) in hashes.iter().enumerate() {
        bytes.extend_from_slice(&(idx as u32).to_be_bytes());
        bytes.extend_from_slice(hash.as_ref());
    }

    fs::write(&idx_path, bytes).expect("failed to write v1 idx");
    idx_path
}

/// Insert a reflog entry that references the provided object hash.
fn insert_reflog_entry(repo: &Path, new_oid: &ObjectHash) {
    let _guard = ChangeDirGuard::new(repo);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let db_conn = libra::internal::db::get_db_conn_instance().await;
        let model = libra::internal::model::reflog::ActiveModel {
            ref_name: Set("HEAD".to_string()),
            old_oid: Set("0000000000000000000000000000000000000000".to_string()),
            new_oid: Set(new_oid.to_string()),
            timestamp: Set(1),
            committer_name: Set("tester".to_string()),
            committer_email: Set("tester@example.com".to_string()),
            action: Set("commit".to_string()),
            message: Set("test reflog".to_string()),
            ..Default::default()
        };
        model.insert(&db_conn).await.expect("insert reflog");
    });
}

/// Write merge recovery metadata that references the provided object.
fn write_merge_state(repo: &Path, target: &ObjectHash) {
    let state_path = repo.join(".libra").join("merge-state.json");
    fs::write(
        state_path,
        format!(r#"{{"orig_head":null,"target":"{target}","base":null}}"#),
    )
    .expect("write merge-state.json");
}

/// Insert an invalid reference row to exercise parse errors in prune.
fn insert_invalid_reference(repo: &Path, name: &str, commit: &str) {
    let _guard = ChangeDirGuard::new(repo);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let db_conn = libra::internal::db::get_db_conn_instance().await;
        let model = libra::internal::model::reference::ActiveModel {
            name: Set(Some(name.to_string())),
            kind: Set(libra::internal::model::reference::ConfigKind::Branch),
            commit: Set(Some(commit.to_string())),
            remote: Set(None),
            ..Default::default()
        };
        model.insert(&db_conn).await.expect("insert invalid ref");
    });
}

/// Insert an annotated tag reference pointing at a tag object.
fn insert_tag_reference(repo: &Path, tag_name: &str, tag_object: &ObjectHash) {
    let _guard = ChangeDirGuard::new(repo);
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let db_conn = libra::internal::db::get_db_conn_instance().await;
        let model = libra::internal::model::reference::ActiveModel {
            name: Set(Some(format!("refs/tags/{tag_name}"))),
            kind: Set(libra::internal::model::reference::ConfigKind::Tag),
            commit: Set(Some(tag_object.to_string())),
            remote: Set(None),
            ..Default::default()
        };
        model.insert(&db_conn).await.expect("insert tag ref");
    });
}

/// Create an annotated tag object that points to a commit and store its ref.
fn create_annotated_tag(repo: &Path, commit: &ObjectHash, tag_name: &str) -> ObjectHash {
    let _guard = ChangeDirGuard::new(repo);
    let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
    let tag = Tag::new(
        *commit,
        ObjectType::Commit,
        tag_name.to_string(),
        Signature {
            signature_type: SignatureType::Tagger,
            name: "tester".to_string(),
            email: "tester@example.com".to_string(),
            timestamp: 1,
            timezone: "+0000".to_string(),
        },
        "annotated tag".to_string(),
    );
    save_object(&tag, &tag.id).expect("failed to save tag object");
    insert_tag_reference(repo, tag_name, &tag.id);
    tag.id
}

// ---------------------------------------------------------------------------
// Basic Functionality Tests (>= 4 required)
// ---------------------------------------------------------------------------

#[test]
#[serial]
/// Tests prune on an empty repository succeeds.
fn test_prune_empty_repo_succeeds() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune on empty repo should succeed");
}

#[test]
#[serial]
/// Tests prune keeps reachable HEAD commit objects.
fn test_prune_keeps_head_commit_object() {
    let repo = create_committed_repo_via_cli();
    let head = head_commit_hash(repo.path());
    assert!(object_exists(repo.path(), &head));

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed on healthy repo");
    assert!(object_exists(repo.path(), &head));
}

#[test]
#[serial]
/// Tests prune removes an unreachable loose blob.
fn test_prune_removes_unreachable_blob() {
    let repo = create_committed_repo_via_cli();
    let blob = create_unreachable_blob(repo.path(), "orphan-blob-a");
    let blob_path = loose_object_path(repo.path(), &blob.to_string());
    assert!(blob_path.exists());

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");
    assert!(!blob_path.exists());
}

#[test]
#[serial]
/// Tests direct prune preserves cloud-pending objects until backup sync completes.
fn test_prune_keeps_unsynced_cloud_backup_object() {
    let repo = create_committed_repo_via_cli();
    enable_cloud_backup(repo.path());
    let repo_id = repo_id(repo.path());
    let blob = create_unreachable_blob(repo.path(), "cloud-pending");
    let blob_path = loose_object_path(repo.path(), &blob.to_string());
    insert_object_index_row(repo.path(), &blob, &repo_id, 0);

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");

    assert!(
        blob_path.exists(),
        "unsynced object_index rows should protect loose objects from prune"
    );
}

#[test]
#[serial]
/// Tests direct prune removes local object_index rows after deleting synced garbage.
fn test_prune_removes_synced_object_index_row() {
    let repo = create_committed_repo_via_cli();
    let repo_id = repo_id(repo.path());
    let blob = create_unreachable_blob(repo.path(), "synced-garbage");
    let blob_path = loose_object_path(repo.path(), &blob.to_string());
    insert_object_index_row(repo.path(), &blob, &repo_id, 1);
    assert!(object_index_row_exists(repo.path(), &blob, &repo_id));

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");

    assert!(!blob_path.exists());
    assert!(
        !object_index_row_exists(repo.path(), &blob, &repo_id),
        "synced object_index row should be removed after local prune"
    );
}

#[test]
#[serial]
/// Tests --dry-run reports prunable objects without deleting them.
fn test_prune_dry_run_reports_without_deleting() {
    let repo = create_committed_repo_via_cli();
    let blob = create_unreachable_blob(repo.path(), "orphan-blob-b");
    let blob_path = loose_object_path(repo.path(), &blob.to_string());

    let output = run_libra_command(&["prune", "--dry-run"], repo.path());
    assert_cli_success(&output, "prune --dry-run should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&format!("{blob} blob")));
    assert!(blob_path.exists());
}

#[test]
#[serial]
/// Tests --verbose reports removed objects.
fn test_prune_verbose_reports_deletions() {
    let repo = create_committed_repo_via_cli();
    let blob = create_unreachable_blob(repo.path(), "orphan-blob-c");

    let output = run_libra_command(&["prune", "--verbose"], repo.path());
    assert_cli_success(&output, "prune --verbose should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&format!("{blob} blob")));
}

#[test]
#[serial]
fn test_prune_keeps_tagged_commit_object() {
    let repo = create_committed_repo_via_cli();
    let (commit, tree, blob) = create_unreachable_commit(repo.path(), "tagged-commit");
    let tag = create_annotated_tag(repo.path(), &commit, "v1.2.3");

    assert!(object_exists(repo.path(), &commit.to_string()));
    assert!(object_exists(repo.path(), &tree.to_string()));
    assert!(object_exists(repo.path(), &blob.to_string()));

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should keep annotated tag targets");

    assert!(object_exists(repo.path(), &tag.to_string()));
    assert!(object_exists(repo.path(), &commit.to_string()));
    assert!(object_exists(repo.path(), &tree.to_string()));
    assert!(object_exists(repo.path(), &blob.to_string()));
}

// ---------------------------------------------------------------------------
// Boundary Condition Tests (>= 8 required)
// ---------------------------------------------------------------------------

#[test]
#[serial]
/// Tests --expire 0 keeps recent unreachable objects.
fn test_prune_expire_epoch_keeps_unreachable_blob() {
    let repo = create_committed_repo_via_cli();
    let blob = create_unreachable_blob(repo.path(), "orphan-blob-expire-0");
    let blob_path = loose_object_path(repo.path(), &blob.to_string());

    let output = run_libra_command(&["prune", "--expire", "0"], repo.path());
    assert_cli_success(&output, "prune --expire 0 should succeed");
    assert!(blob_path.exists());
}

#[test]
#[serial]
/// Tests relative --expire keeps recent objects (not expired).
fn test_prune_expire_relative_keeps_recent_blob() {
    let repo = create_committed_repo_via_cli();
    let blob = create_unreachable_blob(repo.path(), "orphan-blob-expire-rel");
    let blob_path = loose_object_path(repo.path(), &blob.to_string());

    let output = run_libra_command(&["prune", "--expire", "1 day ago"], repo.path());
    assert_cli_success(&output, "prune --expire relative should succeed");
    assert!(blob_path.exists());
}

#[test]
#[serial]
/// Tests future --expire prunes unreachable objects.
fn test_prune_expire_future_prunes_unreachable_blob() {
    let repo = create_committed_repo_via_cli();
    let blob = create_unreachable_blob(repo.path(), "orphan-blob-expire-future");
    let blob_path = loose_object_path(repo.path(), &blob.to_string());

    let output = run_libra_command(&["prune", "--expire", "2999-01-01"], repo.path());
    assert_cli_success(&output, "prune --expire future should succeed");
    assert!(!blob_path.exists());
}

#[test]
#[serial]
/// Tests prune ignores non-hex object directories.
fn test_prune_ignores_non_hex_object_directory() {
    let repo = create_committed_repo_via_cli();
    let junk_dir = repo.path().join(".libra").join("objects").join("zz");
    fs::create_dir_all(&junk_dir).expect("create junk dir");
    let junk_file = junk_dir.join("junk");
    fs::write(&junk_file, b"junk").expect("write junk");

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");
    assert!(junk_file.exists());
}

#[test]
#[serial]
/// Tests prune ignores non-file entries in hex prefix directories.
fn test_prune_ignores_non_file_entries_in_hex_dir() {
    let repo = create_committed_repo_via_cli();
    let hex_dir = repo.path().join(".libra").join("objects").join("aa");
    fs::create_dir_all(&hex_dir).expect("create hex dir");
    let subdir = hex_dir.join("subdir");
    fs::create_dir_all(&subdir).expect("create subdir");

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");
    assert!(subdir.exists());
}

#[test]
#[serial]
/// Tests index-tracked blobs are preserved by prune.
fn test_prune_keeps_index_reachable_blob() {
    let repo = create_committed_repo_via_cli();
    fs::write(repo.path().join("staged.txt"), "index-only\n").unwrap();
    let add = run_libra_command(&["add", "staged.txt"], repo.path());
    assert_cli_success(&add, "failed to stage file");

    let _guard = ChangeDirGuard::new(repo.path());
    let _hash_guard = set_hash_kind_for_test(HashKind::Sha1);
    let blob = Blob::from_content("index-only\n");
    let blob_path = loose_object_path(repo.path(), &blob.id.to_string());

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");
    assert!(blob_path.exists());
}

#[test]
#[serial]
/// Tests reflog-reachable commits are preserved by prune.
fn test_prune_keeps_reflog_reachable_commit() {
    let repo = create_committed_repo_via_cli();
    let (commit, _, _) = create_unreachable_commit(repo.path(), "reflog-commit");
    insert_reflog_entry(repo.path(), &commit);

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");
    assert!(object_exists(repo.path(), &commit.to_string()));
}

#[test]
#[serial]
/// Tests merge-state roots are preserved by direct prune.
fn test_prune_keeps_merge_state_commit() {
    let repo = create_committed_repo_via_cli();
    let (commit, tree, blob) = create_unreachable_commit(repo.path(), "merge-state-commit");
    write_merge_state(repo.path(), &commit);

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should preserve merge-state roots");

    assert!(object_exists(repo.path(), &commit.to_string()));
    assert!(object_exists(repo.path(), &tree.to_string()));
    assert!(object_exists(repo.path(), &blob.to_string()));
}

#[test]
#[serial]
/// Tests multiple head arguments keep their objects reachable.
fn test_prune_multiple_heads_keep_manual_commit() {
    let repo = create_committed_repo_via_cli();
    let (commit_a, _, _) = create_unreachable_commit(repo.path(), "head-a");
    let (commit_b, _, _) = create_unreachable_commit(repo.path(), "head-b");

    let output = run_libra_command(
        &["prune", &commit_a.to_string(), &commit_b.to_string()],
        repo.path(),
    );
    assert_cli_success(&output, "prune with heads should succeed");
    assert!(object_exists(repo.path(), &commit_a.to_string()));
    assert!(object_exists(repo.path(), &commit_b.to_string()));
}

#[test]
#[serial]
/// Tests tag names are accepted as head arguments.
fn test_prune_head_arg_tag_is_accepted() {
    let repo = create_committed_repo_via_cli();
    let tag = run_libra_command(&["tag", "v1.0"], repo.path());
    assert_cli_success(&tag, "failed to create tag");

    let output = run_libra_command(&["prune", "v1.0"], repo.path());
    assert_cli_success(&output, "prune should accept tag head");
}

#[test]
#[serial]
/// Tests orphan v2 pack indexes do not trigger pruning of reachable loose copies.
fn test_prune_orphan_packed_v2_keeps_reachable_duplicate() {
    let repo = create_committed_repo_via_cli();
    let head = head_commit_hash(repo.path());
    assert!(object_exists(repo.path(), &head));

    write_fake_idx_v2(
        repo.path(),
        &[ObjectHash::from_bytes(&hex::decode(&head).unwrap()).unwrap()],
    );

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");
    assert!(object_exists(repo.path(), &head));
}

#[test]
#[serial]
/// Tests orphan v1 pack indexes do not trigger pruning of reachable loose copies.
fn test_prune_orphan_packed_v1_keeps_reachable_duplicate() {
    let repo = create_committed_repo_via_cli();
    let head = head_commit_hash(repo.path());
    assert!(object_exists(repo.path(), &head));

    write_fake_idx_v1(
        repo.path(),
        &[ObjectHash::from_bytes(&hex::decode(&head).unwrap()).unwrap()],
    );

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");
    assert!(object_exists(repo.path(), &head));
}

#[test]
#[serial]
/// Tests prune removes a batch of unreachable loose blobs.
fn test_prune_large_unreachable_batch() {
    let repo = create_committed_repo_via_cli();
    let mut blobs = Vec::new();
    for idx in 0..25 {
        let blob = create_unreachable_blob(repo.path(), &format!("batch-{idx}"));
        blobs.push(blob);
    }

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");
    for blob in blobs {
        assert!(!object_exists(repo.path(), &blob.to_string()));
    }
}

#[test]
#[serial]
/// Tests prune removes empty prefix directories after deleting objects.
fn test_prune_removes_empty_prefix_dir() {
    let repo = tempdir().unwrap();
    init_repo_via_cli(repo.path());
    let blob = create_unreachable_blob(repo.path(), "orphan-for-prefix");
    let blob_str = blob.to_string();
    let prefix_dir = repo
        .path()
        .join(".libra")
        .join("objects")
        .join(&blob_str[..2]);
    assert!(prefix_dir.exists());

    let output = run_libra_command(&["prune"], repo.path());
    assert_cli_success(&output, "prune should succeed");
    assert!(!prefix_dir.exists());
}

#[test]
#[serial]
/// Tests --verbose prints nothing when there are no prunable objects.
fn test_prune_verbose_no_prunable_outputs_nothing() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["prune", "--verbose"], repo.path());
    assert_cli_success(&output, "prune should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.trim().is_empty());
}

// ---------------------------------------------------------------------------
// Error Handling Tests (>= 8 required)
// ---------------------------------------------------------------------------

#[test]
#[serial]
/// Tests prune outside a repository returns a fatal error.
fn test_prune_outside_repository_fails() {
    let temp = tempdir().unwrap();
    let output = run_libra_command(&["prune"], temp.path());
    assert_eq!(output.status.code(), Some(128));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fatal"));
}

#[test]
#[serial]
/// Tests prune rejects invalid --expire formats.
fn test_prune_invalid_expire_format_fails() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["prune", "--expire", "not-a-date"], repo.path());
    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid date"));
}

#[test]
#[serial]
/// Tests prune rejects negative --expire timestamps.
fn test_prune_negative_expire_fails() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["prune", "--expire", "-1"], repo.path());
    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument"));
}

#[test]
#[serial]
/// Tests prune rejects invalid head arguments.
fn test_prune_invalid_head_fails() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["prune", "not-a-ref"], repo.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Not a valid object name"));
}

#[test]
#[serial]
/// Tests prune fails when detached HEAD points to a missing object.
fn test_prune_detached_head_missing_object_fails() {
    let repo = create_committed_repo_via_cli();

    fs::write(repo.path().join("second.txt"), "second\n").unwrap();
    let add = run_libra_command(&["add", "second.txt"], repo.path());
    assert_cli_success(&add, "failed to add second.txt");
    let commit = run_libra_command(&["commit", "-m", "second", "--no-verify"], repo.path());
    assert_cli_success(&commit, "failed to create second commit");

    let log_output = run_libra_command(&["log", "--pretty=%H"], repo.path());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    let first_commit = stdout.lines().nth(1).unwrap().trim();

    let detach = run_libra_command(&["switch", "--detach", first_commit], repo.path());
    assert_cli_success(&detach, "failed to detach HEAD");

    let first_path = loose_object_path(repo.path(), first_commit);
    fs::remove_file(&first_path).expect("remove detached head object");

    let output = run_libra_command(&["prune"], repo.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing object"));
}

#[test]
#[serial]
/// Tests -- stops option parsing and invalid heads still fail.
fn test_prune_double_dash_invalid_head_fails() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["prune", "--", "--not-a-ref"], repo.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Not a valid object name"));
}

#[test]
#[serial]
/// Tests prune fails when a ref points to a missing object.
fn test_prune_missing_ref_object_fails() {
    let repo = create_committed_repo_via_cli();
    let head = head_commit_hash(repo.path());
    let head_path = loose_object_path(repo.path(), &head);
    fs::remove_file(&head_path).expect("remove head object");

    let output = run_libra_command(&["prune"], repo.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("points to missing object"));
}

#[test]
#[serial]
/// Tests prune fails on invalid ref OIDs stored in the database.
fn test_prune_invalid_ref_oid_fails() {
    let repo = create_committed_repo_via_cli();
    insert_invalid_reference(repo.path(), "invalid-ref", "zzzz");

    let output = run_libra_command(&["prune"], repo.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid ref oid"));
}

#[test]
#[serial]
/// Tests prune reports a usage error when --expire is missing its value.
fn test_prune_expire_missing_value_fails() {
    let repo = create_committed_repo_via_cli();
    let output = run_libra_command(&["prune", "--expire"], repo.path());
    assert_eq!(output.status.code(), Some(129));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("a value is required"));
}

#[test]
#[serial]
/// Tests prune rejects full hashes that do not exist in the repo.
fn test_prune_missing_full_hash_head_fails() {
    let repo = create_committed_repo_via_cli();
    let head = head_commit_hash(repo.path());
    let mut chars: Vec<char> = head.chars().collect();
    chars[0] = if chars[0] != 'a' { 'a' } else { 'b' };
    let missing = chars.into_iter().collect::<String>();
    assert!(!object_exists(repo.path(), &missing));

    let output = run_libra_command(&["prune", &missing], repo.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing object"));
}
