//! Cloud backup command for synchronizing repository data to Cloudflare D1 and R2.
//!
//! This module provides subcommands for:
//! - `libra cloud sync` - Sync local DB to D1, objects to R2
//! - `libra cloud restore` - Restore from D1/R2
//! - `libra cloud status` - Show sync status

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::Arc,
};

use clap::{Parser, Subcommand};
use git_internal::hash::ObjectHash;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Schema, Set,
    sea_query::Expr,
};
use uuid::Uuid;

use crate::{
    cli_error,
    internal::{
        config::Config,
        db,
        model::{object_index, reference},
    },
    utils::{
        d1_client::D1Client,
        error::{CliError, CliResult},
        path,
        storage::{Storage, local::LocalStorage, remote::RemoteStorage},
        util,
    },
};

#[derive(Parser, Debug)]
#[command(about = "Cloud backup and restore operations")]
pub struct CloudArgs {
    #[command(subcommand)]
    pub command: CloudCommand,
}

#[derive(Subcommand, Debug)]
pub enum CloudCommand {
    /// Sync local repository to cloud (D1 + R2)
    Sync(SyncArgs),
    /// Restore repository from cloud
    Restore(RestoreArgs),
    /// Show cloud sync status
    Status(StatusArgs),
}

#[derive(Parser, Debug)]
pub struct SyncArgs {
    /// Force sync all objects, not just unsynced ones
    #[arg(long)]
    pub force: bool,

    /// Batch size for sync operations
    #[arg(long, default_value = "50")]
    pub batch_size: usize,
}

#[derive(Parser, Debug)]
pub struct RestoreArgs {
    /// Repository ID to restore
    #[arg(long, required_unless_present = "name", conflicts_with = "name")]
    pub repo_id: Option<String>,

    /// Repository name to restore
    #[arg(long, required_unless_present = "repo_id", conflicts_with = "repo_id")]
    pub name: Option<String>,

    /// Only restore metadata (object index), not objects
    #[arg(long)]
    pub metadata_only: bool,
}

#[derive(Parser, Debug)]
pub struct StatusArgs {
    /// Show detailed status for each object
    #[arg(long)]
    pub verbose: bool,
}

/// Execute cloud command
pub async fn execute(args: CloudArgs) {
    match args.command {
        CloudCommand::Sync(sync_args) => {
            if let Err(e) = execute_sync(sync_args).await {
                cli_error!(e, "fatal: sync failed");
                std::process::exit(1);
            }
        }
        CloudCommand::Restore(restore_args) => {
            if let Err(e) = execute_restore(restore_args).await {
                cli_error!(e, "fatal: restore failed");
                std::process::exit(1);
            }
        }
        CloudCommand::Status(status_args) => {
            if let Err(e) = execute_status(status_args).await {
                cli_error!(e, "fatal: status check failed");
                std::process::exit(1);
            }
        }
    }
}

/// Thin wrapper for CLI dispatch. Internal errors are still handled via
/// `eprintln!` + `process::exit`.
///
/// # Known limitations
///
/// `execute()` handles errors internally and never propagates them, so this
/// wrapper always returns `Ok(())` even when the cloud command fails.
// TODO: refactor execute() to return CliResult so errors propagate to callers.
pub async fn execute_safe(args: CloudArgs) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    execute(args).await;
    Ok(())
}

/// Execute sync command - uploads objects to R2, indexes to D1, and registers project name
async fn execute_sync(args: SyncArgs) -> Result<(), String> {
    if args.batch_size < 1 {
        return Err("Batch size must be at least 1".to_string());
    }

    println!("Starting cloud sync...");

    validate_cloud_backup_env(false)?;

    // Initialize D1 client
    let d1_client = D1Client::from_env().map_err(|e| format!("D1 client error: {}", e.message))?;

    // Ensure D1 table exists before any operations
    d1_client
        .ensure_object_index_table()
        .await
        .map_err(|e| format!("Failed to create D1 table: {}", e.message))?;

    // Get database connection
    let db_conn = db::get_db_conn_instance().await;

    // Check if object_index table exists locally, create if not
    let builder = db_conn.get_database_backend();
    let schema = Schema::new(builder);
    let stmt = schema
        .create_table_from_entity(object_index::Entity)
        .if_not_exists()
        .to_owned();

    let _ = db_conn.execute(builder.build(&stmt)).await;

    let repo_id = ensure_repo_id().await?;

    // Determine project name from config 'cloud.name' or current directory name
    let project_name = Config::get("cloud", None, "name").await.unwrap_or_else(|| {
        util::working_dir()
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown-project".to_string())
    });

    // Ensure repositories table exists
    d1_client
        .ensure_repositories_table()
        .await
        .map_err(|e| format!("Failed to create repositories table: {}", e.message))?;

    // Upsert repository info
    let repo_row = d1_client
        .upsert_repository(&repo_id, &project_name)
        .await
        .map_err(|e| format!("Failed to upsert repository: {}", e.message))?;

    // Verify repo_id matches (to detect name conflict)
    if repo_row.repo_id != repo_id {
        return Err(format!(
            "Project name '{}' is already taken by another repository (ID: {}). Please choose a different name in cloud.name config.",
            project_name, repo_row.repo_id
        ));
    }

    // Query unsynced objects
    let query = if args.force {
        object_index::Entity::find().filter(object_index::Column::RepoId.eq(&repo_id))
    } else {
        object_index::Entity::find()
            .filter(object_index::Column::RepoId.eq(&repo_id))
            .filter(object_index::Column::IsSynced.eq(0))
    };

    let unsynced_objects = query
        .all(db_conn)
        .await
        .map_err(|e| format!("Database query failed: {}", e))?;

    // Initialize R2 storage
    let r2_storage = create_r2_storage(&repo_id)?;

    if unsynced_objects.is_empty() {
        println!("No objects to sync.");
        sync_metadata(db_conn, &r2_storage)
            .await
            .map_err(|e| format!("Metadata sync failed: {}", e))?;
        return Ok(());
    }

    println!("Found {} objects to sync.", unsynced_objects.len());

    // Initialize local storage for reading objects
    let objects_path = path::objects();
    let local_storage = LocalStorage::new(objects_path);

    let mut synced_count = 0;
    let mut failed_count = 0;

    // Process in batches
    for batch in unsynced_objects.chunks(args.batch_size) {
        for obj in batch {
            let result = sync_single_object(obj, &local_storage, &r2_storage, &d1_client).await;

            match result {
                Ok(_) => {
                    // Update local is_synced flag
                    let mut active: object_index::ActiveModel = obj.clone().into();
                    active.is_synced = Set(1);
                    if let Err(e) = active.update(db_conn).await {
                        cli_error!(
                            e,
                            "warning: failed to update local sync status for {}",
                            obj.o_id
                        );
                    }
                    synced_count += 1;
                }
                Err(e) => {
                    cli_error!(e, "error: failed to sync {}", obj.o_id);
                    failed_count += 1;
                }
            }
        }
        println!(
            "Progress: {}/{} synced, {} failed",
            synced_count,
            unsynced_objects.len(),
            failed_count
        );
    }

    println!(
        "Sync complete: {} synced, {} failed",
        synced_count, failed_count
    );

    if failed_count > 0 {
        Err(format!("{} objects failed to sync", failed_count))
    } else {
        sync_metadata(db_conn, &r2_storage)
            .await
            .map_err(|e| format!("Metadata sync failed: {}", e))?;
        Ok(())
    }
}

/// Sync a single object: R2 first (idempotent), then D1
async fn sync_single_object(
    obj: &object_index::Model,
    local_storage: &LocalStorage,
    r2_storage: &RemoteStorage,
    d1_client: &D1Client,
) -> Result<(), String> {
    let hash = ObjectHash::from_bytes(
        &hex::decode(&obj.o_id).map_err(|e| format!("Invalid hash: {}", e))?,
    )
    .map_err(|e| format!("Invalid object hash: {}", e))?;

    // Phase 1: Upload to R2 (idempotent - same hash will just overwrite)
    // Check if already exists in R2 to avoid unnecessary upload
    if !r2_storage.exist(&hash).await {
        // Read from local storage
        let (data, obj_type) = local_storage
            .get(&hash)
            .await
            .map_err(|e| format!("Failed to read local object: {}", e))?;

        // Upload to R2
        r2_storage
            .put(&hash, &data, obj_type)
            .await
            .map_err(|e| format!("R2 upload failed: {}", e))?;
    }

    // Phase 2: Upsert to D1 (idempotent - will update if exists)
    d1_client
        .upsert_object_index(
            &obj.o_id,
            &obj.o_type,
            obj.o_size,
            &obj.repo_id,
            obj.created_at,
        )
        .await
        .map_err(|e| format!("D1 write failed: {}", e.message))?;

    Ok(())
}

/// Execute restore command - resolves project name (if provided) and restores from D1/R2
async fn execute_restore(args: RestoreArgs) -> Result<(), String> {
    validate_cloud_backup_env(args.metadata_only)?;

    // Initialize D1 client
    let d1_client = D1Client::from_env().map_err(|e| format!("D1 client error: {}", e.message))?;

    let repo_id = if let Some(name) = &args.name {
        // Ensure repositories table exists before resolving name
        // This handles cases where the D1 database is old/uninitialized and missing the table
        d1_client
            .ensure_repositories_table()
            .await
            .map_err(|e| format!("Failed to ensure repositories table: {}", e.message))?;

        let id = d1_client
            .get_repo_id_by_name(name)
            .await
            .map_err(|e| format!("Failed to resolve repo name: {}", e.message))?;
        id.ok_or_else(|| format!("Repository with name '{}' not found", name))?
    } else {
        args.repo_id
            .clone()
            .ok_or_else(|| "repo_id is required".to_string())?
    };

    println!("Starting restore for repo: {}", repo_id);

    // Get object indexes from D1
    let indexes = d1_client
        .get_object_indexes(&repo_id)
        .await
        .map_err(|e| format!("Failed to query D1: {}", e.message))?;

    println!("Found {} objects in cloud for repo.", indexes.len());

    if indexes.is_empty() {
        println!("No objects found for this repo.");
        return Ok(());
    }

    // Get database connection and insert indexes
    let db_conn = db::get_db_conn_instance().await;

    for idx in &indexes {
        // Check if exists
        let existing = object_index::Entity::find()
            .filter(object_index::Column::OId.eq(&idx.o_id))
            .filter(object_index::Column::RepoId.eq(&idx.repo_id))
            .one(db_conn)
            .await
            .map_err(|e| format!("DB error: {}", e))?;

        if let Some(existing_model) = existing {
            let mut active: object_index::ActiveModel = existing_model.into();
            active.is_synced = Set(1);
            if let Err(e) = active.update(db_conn).await {
                cli_error!(e, "warning: failed to update index for {}", idx.o_id);
            }
        } else {
            let entry = object_index::ActiveModel {
                o_id: Set(idx.o_id.clone()),
                o_type: Set(idx.o_type.clone()),
                o_size: Set(idx.o_size),
                repo_id: Set(idx.repo_id.clone()),
                created_at: Set(idx.created_at),
                is_synced: Set(1), // Already synced since we're restoring from cloud
                ..Default::default()
            };

            if let Err(e) = entry.insert(db_conn).await {
                cli_error!(e, "warning: failed to insert index for {}", idx.o_id);
            }
        }
    }

    println!(
        "Restored {} object indexes to local database.",
        indexes.len()
    );

    // Update local config with restored repo_id
    if Config::get("libra", None, "repoid").await.is_some() {
        Config::update("libra", None, "repoid", &repo_id).await;
    } else {
        Config::insert("libra", None, "repoid", &repo_id).await;
    }

    if args.metadata_only {
        println!("Metadata-only restore complete.");
        return Ok(());
    }

    // Download objects from R2
    let r2_storage = create_r2_storage(&repo_id)?;
    let objects_path = path::objects();
    let local_storage = LocalStorage::new(objects_path);

    let mut downloaded = 0;
    let mut skipped = 0;
    let mut failed = 0;

    for idx in &indexes {
        let hash = match ObjectHash::from_bytes(
            &hex::decode(&idx.o_id).map_err(|e| format!("Invalid hash: {}", e))?,
        ) {
            Ok(h) => h,
            Err(e) => {
                cli_error!(e, "error: invalid object hash '{}'", idx.o_id);
                failed += 1;
                continue;
            }
        };

        // Check if already exists locally
        if local_storage.exist(&hash).await {
            skipped += 1;
            continue;
        }

        // Download from R2
        match r2_storage.get(&hash).await {
            Ok((data, obj_type)) => {
                // Verify hash
                let computed = ObjectHash::from_type_and_data(obj_type, &data);
                if computed != hash {
                    eprintln!(
                        "warning: hash mismatch for {}: expected {}, got {}",
                        idx.o_id, hash, computed
                    );
                    failed += 1;
                    continue;
                }

                // Save to local storage
                if let Err(e) = local_storage.put(&hash, &data, obj_type).await {
                    cli_error!(e, "error: failed to save object {}", idx.o_id);
                    failed += 1;
                    continue;
                }
                downloaded += 1;
            }
            Err(e) => {
                cli_error!(e, "error: failed to download {}", idx.o_id);
                failed += 1;
            }
        }
    }

    println!(
        "Restore complete: {} downloaded, {} skipped (already exist), {} failed",
        downloaded, skipped, failed
    );

    if failed > 0 {
        Err(format!("{} objects failed to restore", failed))
    } else {
        // Restore metadata
        if let Err(e) = restore_metadata(db_conn, &r2_storage).await {
            eprintln!("warning: failed to restore metadata: {}", e);
        }

        // Post-restore: update HEAD and restore worktree if we're in a fresh repo state
        // This handles the case where we restored a repo into an empty directory
        // We try to find the latest commit and checkout to it

        // Check if HEAD has a commit (either restored or existing)
        let head_commit = crate::internal::head::Head::current_commit().await;

        if let Some(commit) = head_commit {
            println!("Restoring working directory to HEAD ({})", commit);
            let _ = restore_worktree_to_head().await;
        } else {
            println!("Restoring working directory (fallback)...");

            // Try to find 'main' branch in references
            // We look for 'main' branch in the reference table as a fallback
            let main_branch = crate::internal::branch::Branch::find_branch("main", None).await;

            if let Some(branch) = main_branch {
                println!("Found main branch: {}", branch.commit);

                // Update HEAD to point to main
                crate::internal::head::Head::update(
                    crate::internal::head::Head::Branch("main".to_string()),
                    None,
                )
                .await;

                let _ = restore_worktree_to_head().await;
            } else {
                println!("No HEAD commit or main branch found. Skipping worktree restore.");
            }
        }

        Ok(())
    }
}

async fn restore_worktree_to_head() -> Result<(), String> {
    let restore_args = crate::command::restore::RestoreArgs {
        pathspec: vec![".".to_string()], // restore everything
        source: Some("HEAD".to_string()),
        worktree: true,
        staged: true,
    };

    if let Err(e) = crate::command::restore::execute_checked(restore_args).await {
        eprintln!("warning: failed to restore worktree files: {}", e);
        Err(e.to_string())
    } else {
        println!("Successfully restored working directory files.");
        Ok(())
    }
}

/// Execute status command - shows sync status
async fn execute_status(args: StatusArgs) -> Result<(), String> {
    // Get database connection
    let db_conn = db::get_db_conn_instance().await;

    // Count total and synced objects
    let repo_id = Config::get("libra", None, "repoid")
        .await
        .unwrap_or_else(|| "unknown-repo".to_string());

    let all_objects = object_index::Entity::find()
        .filter(object_index::Column::RepoId.eq(&repo_id))
        .all(db_conn)
        .await
        .map_err(|e| format!("Database query failed: {}", e))?;

    let synced_count = all_objects.iter().filter(|o| o.is_synced == 1).count();
    let unsynced_count = all_objects.len() - synced_count;

    println!("Cloud Sync Status:");
    println!("  Repo ID:       {}", repo_id);
    println!("  Total objects: {}", all_objects.len());
    println!(
        "  Synced:        {} ({}%)",
        synced_count,
        if all_objects.is_empty() {
            0
        } else {
            synced_count * 100 / all_objects.len()
        }
    );
    println!("  Pending:       {}", unsynced_count);

    // Group by type
    let mut by_type: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    for obj in &all_objects {
        let entry = by_type.entry(obj.o_type.clone()).or_insert((0, 0));
        entry.0 += 1;
        if obj.is_synced == 1 {
            entry.1 += 1;
        }
    }

    println!("\nBy object type:");
    for (obj_type, (total, synced)) in &by_type {
        println!("  {}: {}/{} synced", obj_type, synced, total);
    }

    if args.verbose && !all_objects.is_empty() {
        println!("\nUnsynced objects:");
        for obj in all_objects.iter().filter(|o| o.is_synced == 0).take(20) {
            println!("  {} ({}, {} bytes)", obj.o_id, obj.o_type, obj.o_size);
        }
        if unsynced_count > 20 {
            println!("  ... and {} more", unsynced_count - 20);
        }
    }

    Ok(())
}

/// Create R2 remote storage from environment variables
fn create_r2_storage(repo_id: &str) -> Result<RemoteStorage, String> {
    let endpoint =
        std::env::var("LIBRA_STORAGE_ENDPOINT").map_err(|_| "LIBRA_STORAGE_ENDPOINT not set")?;
    let bucket =
        std::env::var("LIBRA_STORAGE_BUCKET").map_err(|_| "LIBRA_STORAGE_BUCKET not set")?;
    let access_key = std::env::var("LIBRA_STORAGE_ACCESS_KEY")
        .map_err(|_| "LIBRA_STORAGE_ACCESS_KEY not set")?;
    let secret_key = std::env::var("LIBRA_STORAGE_SECRET_KEY")
        .map_err(|_| "LIBRA_STORAGE_SECRET_KEY not set")?;
    let region = std::env::var("LIBRA_STORAGE_REGION").unwrap_or_else(|_| "auto".to_string());

    let s3 = object_store::aws::AmazonS3Builder::new()
        .with_bucket_name(&bucket)
        .with_region(&region)
        .with_endpoint(&endpoint)
        .with_access_key_id(&access_key)
        .with_secret_access_key(&secret_key)
        .with_virtual_hosted_style_request(false)
        .build()
        .map_err(|e| format!("Failed to build R2 client: {}", e))?;

    Ok(RemoteStorage::new_with_prefix(
        Arc::new(s3),
        repo_id.to_string(),
    ))
}

fn validate_cloud_backup_env(skip_r2: bool) -> Result<(), String> {
    let mut required = vec![
        "LIBRA_D1_ACCOUNT_ID",
        "LIBRA_D1_API_TOKEN",
        "LIBRA_D1_DATABASE_ID",
    ];

    if !skip_r2 {
        required.extend_from_slice(&[
            "LIBRA_STORAGE_ENDPOINT",
            "LIBRA_STORAGE_BUCKET",
            "LIBRA_STORAGE_ACCESS_KEY",
            "LIBRA_STORAGE_SECRET_KEY",
        ]);
    }

    let missing: Vec<&str> = required
        .into_iter()
        .filter(|k| std::env::var(k).ok().map(|v| v.is_empty()).unwrap_or(true))
        .collect();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Cloud backup requires D1{} configuration. Missing: {}",
            if skip_r2 { "" } else { " + R2" },
            missing.join(", ")
        ))
    }
}

async fn ensure_repo_id() -> Result<String, String> {
    if let Some(existing) = Config::get("libra", None, "repoid").await
        && !existing.is_empty()
        && existing != "unknown-repo"
    {
        return Ok(existing);
    }

    let repo_id = Uuid::new_v4().to_string();
    Config::insert("libra", None, "repoid", &repo_id).await;

    let db_conn = db::get_db_conn_instance().await;
    let _ = object_index::Entity::update_many()
        .filter(object_index::Column::RepoId.eq("unknown-repo"))
        .col_expr(object_index::Column::RepoId, Expr::value(repo_id.clone()))
        .exec(db_conn)
        .await;

    Ok(repo_id)
}

fn calculate_metadata_hash(json: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    json.hash(&mut hasher);
    hasher.finish()
}

async fn sync_metadata(
    db_conn: &sea_orm::DatabaseConnection,
    r2_storage: &RemoteStorage,
) -> Result<(), String> {
    println!("Syncing metadata...");
    let references = reference::Entity::find()
        .all(db_conn)
        .await
        .map_err(|e| format!("Failed to fetch references: {}", e))?;

    // Sort to ensure deterministic output for hashing
    let mut sorted_refs = references;
    sorted_refs.sort_by(|a, b| {
        let a_kind = format!("{:?}", a.kind);
        let b_kind = format!("{:?}", b.kind);
        let a_key = (&a.name, &a.remote, a_kind);
        let b_key = (&b.name, &b.remote, b_kind);
        a_key.cmp(&b_key)
    });

    let json = serde_json::to_vec(&sorted_refs)
        .map_err(|e| format!("Failed to serialize metadata: {}", e))?;

    let current_hash = calculate_metadata_hash(&json);

    // Check if hash matches last sync
    if let Some(stored) = Config::get("cloud", None, "metadata_hash").await
        && let Ok(stored_hash) = stored.parse::<u64>()
        && stored_hash == current_hash
    {
        println!("Metadata unchanged, skipping upload.");
        return Ok(());
    }

    r2_storage
        .put_metadata(&json)
        .await
        .map_err(|e| format!("Failed to upload metadata: {}", e))?;

    // Update stored hash
    Config::insert("cloud", None, "metadata_hash", &current_hash.to_string()).await;

    println!("Metadata synced ({} references).", sorted_refs.len());
    Ok(())
}

async fn restore_metadata(
    db_conn: &sea_orm::DatabaseConnection,
    r2_storage: &RemoteStorage,
) -> Result<(), String> {
    println!("Restoring metadata...");

    let data = match r2_storage.get_metadata().await {
        Ok(data) => data,
        Err(e) => {
            println!("warning: failed to download metadata: {}", e);
            return Ok(());
        }
    };

    let references: Vec<reference::Model> = serde_json::from_slice(&data)
        .map_err(|e| format!("Failed to deserialize metadata: {}", e))?;

    for ref_model in references {
        // Build query to find matching reference
        let mut query = reference::Entity::find()
            .filter(reference::Column::Kind.eq(ref_model.kind.clone()))
            .filter(reference::Column::Remote.eq(ref_model.remote.clone()));

        // Head references are unique by kind and remote, name is the mutable current branch.
        // For other types, match by name as well.
        if ref_model.kind != reference::ConfigKind::Head {
            query = query.filter(reference::Column::Name.eq(ref_model.name.clone()));
        }

        let existing = query
            .one(db_conn)
            .await
            .map_err(|e| format!("DB error: {}", e))?;

        if let Some(existing_model) = existing {
            let mut active: reference::ActiveModel = existing_model.into();
            // Keep mutable HEAD name (attached branch) consistent during restore.
            active.name = Set(ref_model.name.clone());
            active.commit = Set(ref_model.commit.clone());
            active.remote = Set(ref_model.remote.clone());
            if let Err(e) = active.update(db_conn).await {
                eprintln!(
                    "warning: failed to update reference {:?}: {}",
                    ref_model.name, e
                );
            }
        } else {
            let active = reference::ActiveModel {
                name: Set(ref_model.name.clone()),
                kind: Set(ref_model.kind.clone()),
                commit: Set(ref_model.commit.clone()),
                remote: Set(ref_model.remote.clone()),
                ..Default::default()
            };
            if let Err(e) = active.insert(db_conn).await {
                eprintln!(
                    "warning: failed to insert reference {:?}: {}",
                    ref_model.name, e
                );
            }
        }
    }

    println!("Metadata restored.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_restore_args_repo_id() {
        let args = RestoreArgs::try_parse_from(["restore", "--repo-id", "123"]).unwrap();
        assert_eq!(args.repo_id, Some("123".to_string()));
        assert_eq!(args.name, None);
    }

    #[test]
    fn test_restore_args_name() {
        let args = RestoreArgs::try_parse_from(["restore", "--name", "test-repo"]).unwrap();
        assert_eq!(args.name, Some("test-repo".to_string()));
        assert_eq!(args.repo_id, None);
    }

    #[test]
    fn test_restore_args_missing() {
        let result = RestoreArgs::try_parse_from(["restore"]);
        assert!(result.is_err());
    }
}
