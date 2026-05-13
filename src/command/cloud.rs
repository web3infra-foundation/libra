//! Cloud backup command for synchronizing repository data to Cloudflare D1 and R2.
//!
//! This module provides subcommands for:
//! - `libra cloud sync` - Sync local DB to D1, objects to R2
//! - `libra cloud restore` - Restore from D1/R2
//! - `libra cloud status` - Show sync status

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::PathBuf,
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
    command::restore::{self as restore_cmd, RestoreArgs as RestoreWorktreeArgs},
    internal::{
        branch::Branch,
        config::ConfigKv,
        db,
        head::Head,
        model::{object_index, reference},
    },
    utils::{
        d1_client::{AgentCheckpointRow, AgentSessionRow, D1Client},
        error::{CliError, CliResult, emit_warning},
        output::OutputConfig,
        path,
        storage::{
            Storage, local::LocalStorage, publish_storage::PublishStorage, remote::RemoteStorage,
        },
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

// ───────────────────────────────────────────────────────────────────
// Phase 1 (publish.md) — structured `cloud sync` helper.
//
// `run_cloud_sync` is the headless entry that `libra publish` will
// reuse in Phase 4+. It performs the full object + metadata + agent
// capture sync but emits human-readable progress through a callback
// trait instead of `println!`/`eprintln!` directly. The legacy
// `execute_sync` wraps this helper with `ConsoleCloudSyncProgress` so
// `libra cloud sync` keeps its original output verbatim.

/// Inputs for [`run_cloud_sync`].
#[derive(Debug, Clone)]
pub struct CloudSyncContext {
    /// Number of objects per batch when streaming to R2 / D1.
    pub batch_size: usize,
    /// Re-sync every object regardless of `is_synced`.
    pub force: bool,
}

/// Metadata-sync outcome surfaced in [`CloudSyncReport`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum MetadataSyncOutcome {
    /// Skipped because object failures preceded it.
    NotRun,
    /// Refs payload uploaded; references emitted = refs count.
    Synced { references: usize },
    /// Metadata hash unchanged since the last sync; nothing uploaded.
    Skipped,
}

/// Agent-capture mirroring outcome surfaced in [`CloudSyncReport`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AgentCaptureSyncOutcome {
    /// Skipped because object failures preceded it.
    NotRun,
    /// Local schema predates the agent_session/agent_checkpoint
    /// migration; nothing to mirror.
    SkippedLegacySchema,
    /// Tables exist; ran the upsert pass. Per-row failures are
    /// reflected in the counts and surface as warnings, not hard
    /// errors (matches the legacy semantics).
    Completed {
        sessions_synced: usize,
        sessions_failed: usize,
        checkpoints_synced: usize,
        checkpoints_failed: usize,
    },
    /// Hard error (table-existence query, ensure-table call, ...).
    Failed { error: String },
}

/// Final outcome of a `run_cloud_sync` call. Hard errors short-
/// circuit and surface as `Err`; recoverable per-object failures live
/// in `failed_count` and the metadata/agent_capture variants.
#[derive(Debug, Clone)]
pub struct CloudSyncReport {
    pub repo_id: String,
    pub project_name: String,
    pub total_unsynced: usize,
    pub synced_count: usize,
    pub failed_count: usize,
    pub metadata: MetadataSyncOutcome,
    pub agent_capture: AgentCaptureSyncOutcome,
}

/// Progress callbacks fired during a `run_cloud_sync` call.
///
/// All methods have empty default impls — implementors only override
/// the events they care about. `ConsoleCloudSyncProgress` mirrors the
/// pre-Phase-1 `libra cloud sync` output verbatim. Phase 4+ publish
/// callers may pass a quieter or structured implementation.
pub trait CloudSyncProgress: Send + Sync {
    fn on_starting(&self) {}
    fn on_no_objects(&self) {}
    fn on_object_total(&self, total: usize) {
        let _ = total;
    }
    fn on_batch_progress(&self, synced: usize, total: usize, failed: usize) {
        let _ = (synced, total, failed);
    }
    fn on_object_error(&self, oid: &str, err: &str) {
        let _ = (oid, err);
    }
    fn on_local_status_warning(&self, oid: &str, err: &str) {
        let _ = (oid, err);
    }
    fn on_sync_complete(&self, synced: usize, failed: usize) {
        let _ = (synced, failed);
    }
    fn on_metadata_starting(&self) {}
    fn on_metadata_skipped(&self) {}
    fn on_metadata_synced(&self, references: usize) {
        let _ = references;
    }
    fn on_agent_capture_starting(&self) {}
    fn on_agent_capture_session_warning(&self, session_id: &str, err: &str) {
        let _ = (session_id, err);
    }
    fn on_agent_capture_checkpoint_warning(&self, checkpoint_id: &str, err: &str) {
        let _ = (checkpoint_id, err);
    }
    fn on_agent_capture_done(
        &self,
        sessions_synced: usize,
        sessions_failed: usize,
        checkpoints_synced: usize,
        checkpoints_failed: usize,
    ) {
        let _ = (
            sessions_synced,
            sessions_failed,
            checkpoints_synced,
            checkpoints_failed,
        );
    }
    fn on_agent_capture_warning(&self, err: &str) {
        let _ = err;
    }
}

/// Console implementation that reproduces the legacy `libra cloud
/// sync` output verbatim.
pub struct ConsoleCloudSyncProgress;

impl CloudSyncProgress for ConsoleCloudSyncProgress {
    fn on_starting(&self) {
        println!("Starting cloud sync...");
    }
    fn on_no_objects(&self) {
        println!("No objects to sync.");
    }
    fn on_object_total(&self, total: usize) {
        println!("Found {total} objects to sync.");
    }
    fn on_batch_progress(&self, synced: usize, total: usize, failed: usize) {
        println!("Progress: {synced}/{total} synced, {failed} failed");
    }
    fn on_object_error(&self, oid: &str, err: &str) {
        cli_error!(err => format!("error: failed to sync {oid}"));
    }
    fn on_local_status_warning(&self, oid: &str, err: &str) {
        cli_error!(err => format!("warning: failed to update local sync status for {oid}"));
    }
    fn on_sync_complete(&self, synced: usize, failed: usize) {
        println!("Sync complete: {synced} synced, {failed} failed");
    }
    fn on_metadata_starting(&self) {
        println!("Syncing metadata...");
    }
    fn on_metadata_skipped(&self) {
        println!("Metadata unchanged, skipping upload.");
    }
    fn on_metadata_synced(&self, references: usize) {
        println!("Metadata synced ({references} references).");
    }
    fn on_agent_capture_starting(&self) {
        println!("Syncing agent_session / agent_checkpoint to D1...");
    }
    fn on_agent_capture_session_warning(&self, session_id: &str, err: &str) {
        eprintln!("warning: agent_session {session_id} upsert failed: {err}");
    }
    fn on_agent_capture_checkpoint_warning(&self, checkpoint_id: &str, err: &str) {
        eprintln!("warning: agent_checkpoint {checkpoint_id} upsert failed: {err}");
    }
    fn on_agent_capture_done(
        &self,
        sessions_synced: usize,
        sessions_failed: usize,
        checkpoints_synced: usize,
        checkpoints_failed: usize,
    ) {
        println!(
            "Agent capture sync: {sessions_synced} sessions ({sessions_failed} failed), \
             {checkpoints_synced} checkpoints ({checkpoints_failed} failed)."
        );
    }
    fn on_agent_capture_warning(&self, err: &str) {
        eprintln!("warning: agent capture sync incomplete: {err}");
    }
}

/// Execute cloud command
pub async fn execute(args: CloudArgs) -> CliResult<()> {
    match args.command {
        CloudCommand::Sync(sync_args) => execute_sync(sync_args)
            .await
            .map_err(|e| cloud_cli_error("sync", e))?,
        CloudCommand::Restore(restore_args) => execute_restore(restore_args)
            .await
            .map_err(|e| cloud_cli_error("restore", e))?,
        CloudCommand::Status(status_args) => execute_status(status_args)
            .await
            .map_err(|e| cloud_cli_error("status", e))?,
    }

    Ok(())
}

pub async fn execute_safe(args: CloudArgs, _output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    execute(args).await
}

fn cloud_cli_error(operation: &str, error: String) -> CliError {
    CliError::fatal(format!("{operation} failed: {error}"))
        .with_detail("operation", operation)
        .with_detail("component", "cloud")
}

/// Execute sync command - uploads objects to R2, indexes to D1, and registers project name
async fn execute_sync(args: SyncArgs) -> Result<(), String> {
    let ctx = CloudSyncContext {
        batch_size: args.batch_size,
        force: args.force,
    };
    let report = run_cloud_sync(ctx, &ConsoleCloudSyncProgress).await?;

    // Preserve the pre-Phase-1 exit semantics: per-object failures
    // surface as a hard error after the human-readable summary has
    // already been emitted by `ConsoleCloudSyncProgress`.
    if report.failed_count > 0 {
        return Err(format!("{} objects failed to sync", report.failed_count));
    }
    Ok(())
}

/// Phase 1 helper extracted from `execute_sync`.
///
/// Runs the full `libra cloud sync` flow without printing directly to
/// stdout / stderr: env validation → D1 / R2 init → object stream →
/// metadata refresh → agent_capture mirror. Human-readable progress
/// flows through the [`CloudSyncProgress`] trait so callers can plug
/// in their own renderer (`ConsoleCloudSyncProgress` for the legacy
/// CLI, a quieter or structured one for `libra publish` later).
///
/// Returns a [`CloudSyncReport`] for the completed run. Hard errors
/// (env, D1, R2, repo-id, db-query, metadata-sync) short-circuit as
/// `Err`. Per-object failures are captured in `failed_count` and skip
/// the metadata + agent_capture phases (preserving the pre-Phase-1
/// "block follow-up work on object failure" gate).
pub async fn run_cloud_sync(
    ctx: CloudSyncContext,
    progress: &dyn CloudSyncProgress,
) -> Result<CloudSyncReport, String> {
    if ctx.batch_size < 1 {
        return Err("Batch size must be at least 1".to_string());
    }

    progress.on_starting();

    validate_cloud_backup_env(false).await?;

    // Initialize D1 client.
    let d1_client = D1Client::from_env()
        .await
        .map_err(|e| format!("D1 client error: {}", e.message))?;

    // Ensure D1 table exists before any operations.
    d1_client
        .ensure_object_index_table()
        .await
        .map_err(|e| format!("Failed to create D1 table: {}", e.message))?;

    // Get database connection.
    let db_conn = db::get_db_conn_instance().await;

    // Check if object_index table exists locally, create if not.
    let builder = db_conn.get_database_backend();
    let schema = Schema::new(builder);
    let stmt = schema
        .create_table_from_entity(object_index::Entity)
        .if_not_exists()
        .to_owned();

    let _ = db_conn.execute(builder.build(&stmt)).await;

    let repo_id = ensure_repo_id().await?;

    // Determine project name from config 'cloud.name' or current directory name.
    let project_name = ConfigKv::get("cloud.name")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
        .unwrap_or_else(|| {
            util::working_dir()
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown-project".to_string())
        });

    // Ensure repositories table exists.
    d1_client
        .ensure_repositories_table()
        .await
        .map_err(|e| format!("Failed to create repositories table: {}", e.message))?;

    // Upsert repository info.
    let repo_row = d1_client
        .upsert_repository(&repo_id, &project_name)
        .await
        .map_err(|e| format!("Failed to upsert repository: {}", e.message))?;

    // Verify repo_id matches (to detect name conflict).
    if repo_row.repo_id != repo_id {
        return Err(format!(
            "Project name '{}' is already taken by another repository (ID: {}). Please choose a different name in cloud.name config.",
            project_name, repo_row.repo_id
        ));
    }

    // Query unsynced objects.
    let query = if ctx.force {
        object_index::Entity::find().filter(object_index::Column::RepoId.eq(&repo_id))
    } else {
        object_index::Entity::find()
            .filter(object_index::Column::RepoId.eq(&repo_id))
            .filter(object_index::Column::IsSynced.eq(0))
    };

    let unsynced_objects = query
        .all(&db_conn)
        .await
        .map_err(|e| format!("Database query failed: {}", e))?;

    // Initialize R2 storage.
    let r2_storage = create_r2_storage(&repo_id).await?;

    let total_unsynced = unsynced_objects.len();

    if unsynced_objects.is_empty() {
        progress.on_no_objects();
        let metadata = sync_metadata(&db_conn, &r2_storage, progress)
            .await
            .map_err(|e| format!("Metadata sync failed: {e}"))?;
        // CEX-EntireIO §10.2: even when there are no new git objects to
        // ship, the agent_session/agent_checkpoint catalog may have new
        // rows from local hook ingestion. Mirror them on every sync.
        let agent_capture =
            match sync_agent_capture_tables(&db_conn, &d1_client, &repo_id, progress).await {
                Ok(outcome) => outcome,
                Err(err) => {
                    progress.on_agent_capture_warning(&err);
                    AgentCaptureSyncOutcome::Failed { error: err }
                }
            };
        return Ok(CloudSyncReport {
            repo_id,
            project_name,
            total_unsynced: 0,
            synced_count: 0,
            failed_count: 0,
            metadata,
            agent_capture,
        });
    }

    progress.on_object_total(total_unsynced);

    // Initialize local storage for reading objects.
    let objects_path = path::objects();
    let local_storage = LocalStorage::new(objects_path);

    let mut synced_count = 0usize;
    let mut failed_count = 0usize;

    // Process in batches.
    for batch in unsynced_objects.chunks(ctx.batch_size) {
        for obj in batch {
            let result = sync_single_object(obj, &local_storage, &r2_storage, &d1_client).await;

            match result {
                Ok(_) => {
                    // Update local is_synced flag.
                    let mut active: object_index::ActiveModel = obj.clone().into();
                    active.is_synced = Set(1);
                    if let Err(e) = active.update(&db_conn).await {
                        progress.on_local_status_warning(&obj.o_id, &e.to_string());
                    }
                    synced_count += 1;
                }
                Err(e) => {
                    progress.on_object_error(&obj.o_id, &e);
                    failed_count += 1;
                }
            }
        }
        progress.on_batch_progress(synced_count, total_unsynced, failed_count);
    }

    progress.on_sync_complete(synced_count, failed_count);

    if failed_count > 0 {
        return Ok(CloudSyncReport {
            repo_id,
            project_name,
            total_unsynced,
            synced_count,
            failed_count,
            metadata: MetadataSyncOutcome::NotRun,
            agent_capture: AgentCaptureSyncOutcome::NotRun,
        });
    }

    let metadata = sync_metadata(&db_conn, &r2_storage, progress)
        .await
        .map_err(|e| format!("Metadata sync failed: {e}"))?;
    // CEX-EntireIO §10.2: append agent capture catalog mirroring at the
    // tail of the sync flow per the plan. Errors here surface as a
    // warning rather than a hard failure so an entirely green object
    // sync is not undone by a transient D1 hiccup.
    let agent_capture =
        match sync_agent_capture_tables(&db_conn, &d1_client, &repo_id, progress).await {
            Ok(outcome) => outcome,
            Err(err) => {
                progress.on_agent_capture_warning(&err);
                AgentCaptureSyncOutcome::Failed { error: err }
            }
        };

    Ok(CloudSyncReport {
        repo_id,
        project_name,
        total_unsynced,
        synced_count,
        failed_count,
        metadata,
        agent_capture,
    })
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
    validate_cloud_backup_env(args.metadata_only).await?;

    // Initialize D1 client
    let d1_client = D1Client::from_env()
        .await
        .map_err(|e| format!("D1 client error: {}", e.message))?;

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
            .one(&db_conn)
            .await
            .map_err(|e| format!("DB error: {}", e))?;

        if let Some(existing_model) = existing {
            let mut active: object_index::ActiveModel = existing_model.into();
            active.is_synced = Set(1);
            if let Err(e) = active.update(&db_conn).await {
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

            if let Err(e) = entry.insert(&db_conn).await {
                cli_error!(e, "warning: failed to insert index for {}", idx.o_id);
            }
        }
    }

    println!(
        "Restored {} object indexes to local database.",
        indexes.len()
    );

    // Update local config with restored repo_id
    let _ = ConfigKv::set("libra.repoid", &repo_id, false).await;

    if args.metadata_only {
        println!("Metadata-only restore complete.");
        return Ok(());
    }

    // Download objects from R2
    let r2_storage = create_r2_storage(&repo_id).await?;
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
        if let Err(e) = restore_metadata(&db_conn, &r2_storage).await {
            emit_warning(format!("failed to restore metadata: {}", e));
        }

        // Post-restore: update HEAD and restore worktree if we're in a fresh repo state.
        // We do this BEFORE the agent-capture restore so that a strict
        // agent-capture failure (Codex Q2: hard-fail on partial restore)
        // doesn't leave the user with a populated objects/refs but no
        // worktree. The agent_session / agent_checkpoint catalogue is
        // metadata about external agent runs — it's not blocking for the
        // user to start working in the restored tree (Codex Q3).

        // Check if HEAD has a commit (either restored or existing)
        let head_commit = Head::current_commit_result()
            .await
            .map_err(|error| format!("failed to resolve HEAD commit: {error}"))?;

        if let Some(commit) = head_commit {
            println!("Restoring working directory to HEAD ({})", commit);
            let _ = restore_worktree_to_head().await;
        } else {
            println!("Restoring working directory (fallback)...");

            // Try to find 'main' branch in references
            // We look for 'main' branch in the reference table as a fallback
            let main_branch = Branch::find_branch_result("main", None)
                .await
                .map_err(|error| format!("failed to resolve main branch: {error}"))?;

            if let Some(branch) = main_branch {
                println!("Found main branch: {}", branch.commit);

                // Update HEAD to point to main
                Head::update(Head::Branch("main".to_string()), None).await;

                let _ = restore_worktree_to_head().await;
            } else {
                println!("No HEAD commit or main branch found. Skipping worktree restore.");
            }
        }

        // CEX-EntireIO §14.3 acceptance: pull `agent_session` /
        // `agent_checkpoint` rows back from D1 so the new machine sees the
        // captured-agent listing without having to re-ingest hooks. This
        // runs LAST (after worktree restore) per Codex Q3 — the inner
        // helper is strict (Q2), so propagating its error here surfaces
        // partial-restore problems to the caller without blocking the
        // worktree materialization that runs above.
        restore_agent_capture_from_d1(&db_conn, &d1_client, &repo_id)
            .await
            .map_err(|e| format!("agent capture restore failed: {}", e))?;

        Ok(())
    }
}

async fn restore_worktree_to_head() -> Result<(), String> {
    let restore_args = RestoreWorktreeArgs {
        pathspec: vec![".".to_string()], // restore everything
        source: Some("HEAD".to_string()),
        worktree: true,
        staged: true,
    };

    if let Err(e) = restore_cmd::execute_checked(restore_args).await {
        emit_warning(format!("failed to restore worktree files: {}", e));
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
    let repo_id = ConfigKv::get("libra.repoid")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
        .unwrap_or_else(|| "unknown-repo".to_string());

    let all_objects = object_index::Entity::find()
        .filter(object_index::Column::RepoId.eq(&repo_id))
        .all(&db_conn)
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

fn cloud_local_db_path() -> Result<PathBuf, String> {
    let storage = util::try_get_storage_path(None)
        .map_err(|e| format!("failed to resolve current repository storage: {e}"))?;
    Ok(storage.join(util::DATABASE))
}

async fn resolve_cloud_env(
    name: &str,
    local_db_path: Option<&std::path::Path>,
) -> Result<Option<String>, String> {
    let local_target = match local_db_path {
        Some(db_path) => crate::internal::config::LocalIdentityTarget::ExplicitDb(db_path),
        None => crate::internal::config::LocalIdentityTarget::CurrentRepo,
    };

    crate::internal::config::resolve_env_for_target(name, local_target)
        .await
        .map_err(|e| format!("failed to resolve '{name}' from env or config: {e}"))
}

async fn resolve_required_cloud_env(
    name: &str,
    local_db_path: Option<&std::path::Path>,
) -> Result<String, String> {
    match resolve_cloud_env(name, local_db_path).await? {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(format!("{name} not set")),
    }
}

/// Create R2 remote storage from environment variables and config.
async fn create_r2_storage(repo_id: &str) -> Result<RemoteStorage, String> {
    let local_db_path = cloud_local_db_path()?;
    create_r2_storage_for_db_path(repo_id, &local_db_path).await
}

async fn create_r2_storage_for_db_path(
    repo_id: &str,
    local_db_path: &std::path::Path,
) -> Result<RemoteStorage, String> {
    let store = create_r2_object_store_for_db_path(local_db_path).await?;
    Ok(RemoteStorage::new_with_prefix(store, repo_id.to_string()))
}

/// Create publish arbitrary-object storage from the same R2
/// environment/config surface used by `libra cloud sync`.
pub(crate) async fn create_publish_storage(
    repo_id: &str,
    site_id: &str,
) -> Result<PublishStorage, String> {
    let local_db_path = cloud_local_db_path()?;
    let store = create_r2_object_store_for_db_path(&local_db_path).await?;
    PublishStorage::new(store, repo_id, site_id)
        .map_err(|e| format!("failed to build publish storage prefix: {e}"))
}

async fn create_r2_object_store_for_db_path(
    local_db_path: &std::path::Path,
) -> Result<Arc<dyn object_store::ObjectStore>, String> {
    let endpoint =
        resolve_required_cloud_env("LIBRA_STORAGE_ENDPOINT", Some(local_db_path)).await?;
    let bucket = resolve_required_cloud_env("LIBRA_STORAGE_BUCKET", Some(local_db_path)).await?;
    let access_key =
        resolve_required_cloud_env("LIBRA_STORAGE_ACCESS_KEY", Some(local_db_path)).await?;
    let secret_key =
        resolve_required_cloud_env("LIBRA_STORAGE_SECRET_KEY", Some(local_db_path)).await?;
    let region = resolve_cloud_env("LIBRA_STORAGE_REGION", Some(local_db_path))
        .await?
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "auto".to_string());

    let s3 = object_store::aws::AmazonS3Builder::new()
        .with_bucket_name(&bucket)
        .with_region(&region)
        .with_endpoint(&endpoint)
        .with_access_key_id(&access_key)
        .with_secret_access_key(&secret_key)
        .with_virtual_hosted_style_request(false)
        .build()
        .map_err(|e| format!("Failed to build R2 client: {}", e))?;

    Ok(Arc::new(s3))
}

async fn validate_cloud_backup_env(skip_r2: bool) -> Result<(), String> {
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

    let local_db_path = cloud_local_db_path()?;
    let mut missing = Vec::new();
    for key in required {
        match resolve_cloud_env(key, Some(&local_db_path)).await? {
            Some(value) if !value.is_empty() => {}
            _ => missing.push(key),
        }
    }

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
    if let Some(entry) = ConfigKv::get("libra.repoid").await.ok().flatten()
        && !entry.value.is_empty()
        && entry.value != "unknown-repo"
    {
        return Ok(entry.value);
    }

    let repo_id = Uuid::new_v4().to_string();
    let _ = ConfigKv::set("libra.repoid", &repo_id, false).await;

    let db_conn = db::get_db_conn_instance().await;
    let _ = object_index::Entity::update_many()
        .filter(object_index::Column::RepoId.eq("unknown-repo"))
        .col_expr(object_index::Column::RepoId, Expr::value(repo_id.clone()))
        .exec(&db_conn)
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
    progress: &dyn CloudSyncProgress,
) -> Result<MetadataSyncOutcome, String> {
    progress.on_metadata_starting();
    let references = reference::Entity::find()
        .all(db_conn)
        .await
        .map_err(|e| format!("Failed to fetch references: {}", e))?;

    // Sort to ensure deterministic output for hashing.
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

    // Check if hash matches last sync.
    if let Some(stored) = ConfigKv::get("cloud.metadata_hash")
        .await
        .ok()
        .flatten()
        .map(|e| e.value)
        && let Ok(stored_hash) = stored.parse::<u64>()
        && stored_hash == current_hash
    {
        progress.on_metadata_skipped();
        return Ok(MetadataSyncOutcome::Skipped);
    }

    r2_storage
        .put_metadata(&json)
        .await
        .map_err(|e| format!("Failed to upload metadata: {}", e))?;

    // Update stored hash.
    let _ = ConfigKv::set("cloud.metadata_hash", &current_hash.to_string(), false).await;

    progress.on_metadata_synced(sorted_refs.len());
    Ok(MetadataSyncOutcome::Synced {
        references: sorted_refs.len(),
    })
}

/// Mirror local `agent_session` and `agent_checkpoint` rows up to the D1
/// side. CEX-EntireIO §10.2 — explicitly skips the per-event JSONL stream
/// (Phase 4 work) and only ships session / checkpoint summaries.
///
/// Both tables are best-effort: if the local schema is at a version that
/// predates `2026050303`, we skip the table without erroring; if the D1
/// upserts fail individually we report the count and keep going so the rest
/// of `libra cloud sync` does not roll back.
async fn sync_agent_capture_tables(
    db_conn: &sea_orm::DatabaseConnection,
    d1_client: &D1Client,
    repo_id: &str,
    progress: &dyn CloudSyncProgress,
) -> Result<AgentCaptureSyncOutcome, String> {
    use sea_orm::{ConnectionTrait, Statement};

    // Bail out cleanly when the migration that creates these tables
    // hasn't run on this clone yet. We do this rather than blanket-erroring
    // because `libra cloud sync` is callable on legacy databases.
    let backend = db_conn.get_database_backend();
    let session_present = db_conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'agent_session' LIMIT 1",
            [],
        ))
        .await
        .map_err(|e| format!("query sqlite_master: {e}"))?
        .is_some();
    if !session_present {
        // Older local schema — nothing to mirror.
        return Ok(AgentCaptureSyncOutcome::SkippedLegacySchema);
    }

    progress.on_agent_capture_starting();
    d1_client
        .ensure_agent_session_table()
        .await
        .map_err(|e| format!("ensure_agent_session_table: {}", e.message))?;
    d1_client
        .ensure_agent_checkpoint_table()
        .await
        .map_err(|e| format!("ensure_agent_checkpoint_table: {}", e.message))?;

    // Pull every session, push it. The on-disk catalog is small in v1
    // (capped at the number of agent sessions per repo) so a full
    // re-upload is cheap and lets us avoid a `dirty` watermark column.
    let session_rows = db_conn
        .query_all(Statement::from_sql_and_values(
            backend,
            "SELECT session_id, agent_kind, provider_session_id, state, working_dir,
                    worktree_id, parent_commit, parent_session_id, metadata_json,
                    redaction_report, started_at, last_event_at, stopped_at, schema_version
             FROM agent_session",
            [],
        ))
        .await
        .map_err(|e| format!("query agent_session: {e}"))?;

    let mut sessions_synced = 0usize;
    let mut sessions_failed = 0usize;
    for row in session_rows {
        let agent_row = AgentSessionRow {
            session_id: row.try_get_by("session_id").unwrap_or_default(),
            agent_kind: row.try_get_by("agent_kind").unwrap_or_default(),
            provider_session_id: row.try_get_by("provider_session_id").unwrap_or_default(),
            state: row.try_get_by("state").unwrap_or_default(),
            working_dir: row.try_get_by("working_dir").unwrap_or_default(),
            worktree_id: row.try_get_by("worktree_id").ok().flatten(),
            parent_commit: row.try_get_by("parent_commit").ok().flatten(),
            parent_session_id: row.try_get_by("parent_session_id").ok().flatten(),
            metadata_json: row.try_get_by("metadata_json").unwrap_or_default(),
            redaction_report: row.try_get_by("redaction_report").unwrap_or_default(),
            started_at: row.try_get_by("started_at").unwrap_or_default(),
            last_event_at: row.try_get_by("last_event_at").unwrap_or_default(),
            stopped_at: row.try_get_by("stopped_at").ok().flatten(),
            schema_version: row.try_get_by("schema_version").unwrap_or(1i64),
        };
        match d1_client.upsert_agent_session(repo_id, &agent_row).await {
            Ok(_) => sessions_synced += 1,
            Err(e) => {
                progress.on_agent_capture_session_warning(&agent_row.session_id, &e.message);
                sessions_failed += 1;
            }
        }
    }

    let checkpoint_rows = db_conn
        .query_all(Statement::from_sql_and_values(
            backend,
            "SELECT checkpoint_id, session_id, parent_checkpoint_id, scope, parent_commit,
                    tree_oid, metadata_blob_oid, traces_commit, tool_use_id,
                    subagent_session_id, description, created_at
             FROM agent_checkpoint",
            [],
        ))
        .await
        .map_err(|e| format!("query agent_checkpoint: {e}"))?;

    let mut checkpoints_synced = 0usize;
    let mut checkpoints_failed = 0usize;
    for row in checkpoint_rows {
        let cp_row = AgentCheckpointRow {
            checkpoint_id: row.try_get_by("checkpoint_id").unwrap_or_default(),
            session_id: row.try_get_by("session_id").unwrap_or_default(),
            parent_checkpoint_id: row.try_get_by("parent_checkpoint_id").ok().flatten(),
            scope: row.try_get_by("scope").unwrap_or_default(),
            parent_commit: row.try_get_by("parent_commit").ok().flatten(),
            tree_oid: row.try_get_by("tree_oid").unwrap_or_default(),
            metadata_blob_oid: row.try_get_by("metadata_blob_oid").unwrap_or_default(),
            traces_commit: row.try_get_by("traces_commit").unwrap_or_default(),
            tool_use_id: row.try_get_by("tool_use_id").ok().flatten(),
            subagent_session_id: row.try_get_by("subagent_session_id").ok().flatten(),
            description: row.try_get_by("description").ok().flatten(),
            created_at: row.try_get_by("created_at").unwrap_or_default(),
        };
        match d1_client.upsert_agent_checkpoint(repo_id, &cp_row).await {
            Ok(_) => checkpoints_synced += 1,
            Err(e) => {
                progress.on_agent_capture_checkpoint_warning(&cp_row.checkpoint_id, &e.message);
                checkpoints_failed += 1;
            }
        }
    }

    progress.on_agent_capture_done(
        sessions_synced,
        sessions_failed,
        checkpoints_synced,
        checkpoints_failed,
    );
    if sessions_failed > 0 || checkpoints_failed > 0 {
        Err(format!(
            "{} session + {} checkpoint upserts failed",
            sessions_failed, checkpoints_failed
        ))
    } else {
        Ok(AgentCaptureSyncOutcome::Completed {
            sessions_synced,
            sessions_failed,
            checkpoints_synced,
            checkpoints_failed,
        })
    }
}

/// CEX-EntireIO §10.2 / §14.3: restore the local `agent_session` +
/// `agent_checkpoint` catalog from D1.
///
/// Mirrors [`sync_agent_capture_tables`] in reverse: lists D1 rows for the
/// repo and inserts them into the local SQLite catalog.
///
/// Behaviour, refined per Codex Phase-3.5b review:
/// - Bails with an explicit warning when the local schema predates the
///   migration that creates these tables (was a silent `Ok(())` previously
///   — Codex Q4).
/// - Hard-fails the aggregate when any row can't be restored — restore is
///   stricter than the upload-side soft-fail because a missing session
///   would leave orphan checkpoints in the local catalog (Codex Q2).
/// - Checkpoint upserts use explicit `ON CONFLICT(checkpoint_id) DO UPDATE
///   SET …` rather than `INSERT OR REPLACE` so the row's CASCADE delete
///   semantics are preserved on conflict (Codex Q1).
async fn restore_agent_capture_from_d1(
    db_conn: &sea_orm::DatabaseConnection,
    d1_client: &D1Client,
    repo_id: &str,
) -> Result<(), String> {
    use sea_orm::{ConnectionTrait, Statement};

    // Codex round-2 follow-up: check BOTH tables locally — a partial
    // schema (e.g. `agent_session` exists but `agent_checkpoint` does not
    // because a half-applied legacy migration left things mid-flight)
    // would otherwise bypass the warning and either fail loudly later or
    // silently succeed with no checkpoint rows. Warn and bail in that
    // case so the user gets a single actionable hint.
    let backend = db_conn.get_database_backend();
    let session_present = db_conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'agent_session' LIMIT 1",
            [],
        ))
        .await
        .map_err(|e| format!("query sqlite_master: {e}"))?
        .is_some();
    let checkpoint_present = db_conn
        .query_one(Statement::from_sql_and_values(
            backend,
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'agent_checkpoint' LIMIT 1",
            [],
        ))
        .await
        .map_err(|e| format!("query sqlite_master: {e}"))?
        .is_some();
    if !session_present || !checkpoint_present {
        // Codex review Q4: emit an actionable hint instead of silently
        // succeeding so a user on an old binary knows why their session
        // list is empty after restore. Round-2 expanded this check to
        // include `agent_checkpoint` so a partial schema can't sneak past.
        emit_warning(
            "agent_session / agent_checkpoint table absent locally — restore skipped. \
             Run `libra init` (or upgrade libra) to create the schema, \
             then rerun `libra cloud restore`.",
        );
        return Ok(());
    }

    println!("Restoring agent_session / agent_checkpoint from D1...");

    // Codex round-2 follow-up: ensure the catalogue tables exist on the
    // remote D1 before listing. Old backups taken by a libra binary that
    // predates Phase 3.5a will not have these tables, and the bare
    // `SELECT … FROM agent_session` would surface as a hard error and
    // (now that Q3 propagates errors) abort `libra cloud restore`. This
    // matches the symmetric `sync_agent_capture_tables` upload path,
    // which already creates the tables before writing — running it on
    // restore makes a fresh pull from a legacy remote behave like an
    // empty catalogue rather than failing the whole restore.
    d1_client
        .ensure_agent_session_table()
        .await
        .map_err(|e| format!("ensure_agent_session_table on D1: {}", e.message))?;
    d1_client
        .ensure_agent_checkpoint_table()
        .await
        .map_err(|e| format!("ensure_agent_checkpoint_table on D1: {}", e.message))?;

    let session_rows = d1_client
        .list_agent_sessions(repo_id)
        .await
        .map_err(|e| format!("list_agent_sessions: {}", e.message))?;
    let checkpoint_rows = d1_client
        .list_agent_checkpoints(repo_id)
        .await
        .map_err(|e| format!("list_agent_checkpoints: {}", e.message))?;

    restore_agent_capture_from_rows(db_conn, &session_rows, &checkpoint_rows).await
}

/// Connection-bound core of [`restore_agent_capture_from_d1`]. Extracted
/// per Codex Phase-3.5b review Q5 so the per-row INSERT logic is
/// testable against an in-memory SQLite without a live D1 endpoint.
///
/// Returns aggregate counts via the printed report and a hard error if
/// any row failed to insert. Caller decides what to do with the error
/// (e.g. defer it past the worktree restore).
async fn restore_agent_capture_from_rows(
    db_conn: &sea_orm::DatabaseConnection,
    session_rows: &[AgentSessionRow],
    checkpoint_rows: &[AgentCheckpointRow],
) -> Result<(), String> {
    use sea_orm::{ConnectionTrait, Statement};

    let backend = db_conn.get_database_backend();

    let mut sessions_inserted = 0usize;
    let mut sessions_failed = 0usize;
    for row in session_rows {
        // Mirror the same upsert semantics as the local hook ingest path
        // (`ON CONFLICT(agent_kind, provider_session_id) DO UPDATE SET …`)
        // so re-running restore over an existing local row is idempotent.
        let stmt = Statement::from_sql_and_values(
            backend,
            "INSERT INTO agent_session (
                session_id, agent_kind, provider_session_id, state, working_dir,
                worktree_id, parent_commit, parent_session_id, metadata_json,
                redaction_report, started_at, last_event_at, stopped_at, schema_version
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(agent_kind, provider_session_id) DO UPDATE SET
                state = excluded.state,
                working_dir = excluded.working_dir,
                worktree_id = excluded.worktree_id,
                parent_commit = excluded.parent_commit,
                parent_session_id = excluded.parent_session_id,
                metadata_json = excluded.metadata_json,
                redaction_report = excluded.redaction_report,
                last_event_at = excluded.last_event_at,
                stopped_at = excluded.stopped_at,
                schema_version = excluded.schema_version",
            [
                row.session_id.clone().into(),
                row.agent_kind.clone().into(),
                row.provider_session_id.clone().into(),
                row.state.clone().into(),
                row.working_dir.clone().into(),
                row.worktree_id.clone().into(),
                row.parent_commit.clone().into(),
                row.parent_session_id.clone().into(),
                row.metadata_json.clone().into(),
                row.redaction_report.clone().into(),
                row.started_at.into(),
                row.last_event_at.into(),
                row.stopped_at.into(),
                row.schema_version.into(),
            ],
        );
        match db_conn.execute(stmt).await {
            Ok(_) => sessions_inserted += 1,
            Err(e) => {
                eprintln!(
                    "warning: agent_session {} restore failed: {e}",
                    row.session_id
                );
                sessions_failed += 1;
            }
        }
    }

    let mut checkpoints_inserted = 0usize;
    let mut checkpoints_failed = 0usize;
    for row in checkpoint_rows {
        // Codex Q1: explicit ON CONFLICT rather than INSERT OR REPLACE
        // — REPLACE deletes the conflicting row first, which would also
        // cascade-delete child rows in any FK-enforcing context. The
        // local schema doesn't currently have children of agent_checkpoint
        // but using DO UPDATE keeps semantics future-proof.
        let stmt = Statement::from_sql_and_values(
            backend,
            "INSERT INTO agent_checkpoint (
                checkpoint_id, session_id, parent_checkpoint_id, scope, parent_commit,
                tree_oid, metadata_blob_oid, traces_commit, tool_use_id,
                subagent_session_id, description, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(checkpoint_id) DO UPDATE SET
                session_id = excluded.session_id,
                parent_checkpoint_id = excluded.parent_checkpoint_id,
                scope = excluded.scope,
                parent_commit = excluded.parent_commit,
                tree_oid = excluded.tree_oid,
                metadata_blob_oid = excluded.metadata_blob_oid,
                traces_commit = excluded.traces_commit,
                tool_use_id = excluded.tool_use_id,
                subagent_session_id = excluded.subagent_session_id,
                description = excluded.description,
                created_at = excluded.created_at",
            [
                row.checkpoint_id.clone().into(),
                row.session_id.clone().into(),
                row.parent_checkpoint_id.clone().into(),
                row.scope.clone().into(),
                row.parent_commit.clone().into(),
                row.tree_oid.clone().into(),
                row.metadata_blob_oid.clone().into(),
                row.traces_commit.clone().into(),
                row.tool_use_id.clone().into(),
                row.subagent_session_id.clone().into(),
                row.description.clone().into(),
                row.created_at.into(),
            ],
        );
        match db_conn.execute(stmt).await {
            Ok(_) => checkpoints_inserted += 1,
            Err(e) => {
                eprintln!(
                    "warning: agent_checkpoint {} restore failed: {e}",
                    row.checkpoint_id
                );
                checkpoints_failed += 1;
            }
        }
    }

    println!(
        "Agent capture restore: {sessions_inserted}/{} sessions, \
         {checkpoints_inserted}/{} checkpoints ({sessions_failed} + \
         {checkpoints_failed} failed).",
        session_rows.len(),
        checkpoint_rows.len()
    );
    if sessions_failed > 0 || checkpoints_failed > 0 {
        Err(format!(
            "{} session + {} checkpoint inserts failed",
            sessions_failed, checkpoints_failed
        ))
    } else {
        Ok(())
    }
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
    restore_metadata_from_bytes(db_conn, &data).await?;
    println!("Metadata restored.");
    Ok(())
}

/// Restore refs metadata and fail hard when the metadata object is missing.
///
/// `libra cloud restore` keeps its historical warning-only behavior through
/// [`restore_metadata`]. Cloud clone restore needs a stricter contract: without
/// refs metadata it cannot set HEAD/branches safely, so the caller must fail and
/// clean up the just-created destination.
#[expect(
    dead_code,
    reason = "cloud clone restore will call this strict helper when the local restore path lands"
)]
pub(crate) async fn restore_metadata_strict(
    db_conn: &sea_orm::DatabaseConnection,
    r2_storage: &RemoteStorage,
) -> Result<(), String> {
    let data = r2_storage
        .get_metadata()
        .await
        .map_err(|e| format!("failed to download metadata: {}", e))?;
    restore_metadata_from_bytes(db_conn, &data).await
}

async fn restore_metadata_from_bytes(
    db_conn: &sea_orm::DatabaseConnection,
    data: &[u8],
) -> Result<(), String> {
    let references: Vec<reference::Model> = serde_json::from_slice(data)
        .map_err(|e| format!("Failed to deserialize metadata: {}", e))?;

    for ref_model in references {
        // Build query to find matching reference
        let remote_filter = match &ref_model.remote {
            Some(remote) => reference::Column::Remote.eq(remote),
            None => reference::Column::Remote.is_null(),
        };
        let mut query = reference::Entity::find()
            .filter(reference::Column::Kind.eq(ref_model.kind.clone()))
            .filter(remote_filter);

        // Head references are unique by kind and remote, name is the mutable current branch.
        // For other types, match by name as well.
        if ref_model.kind != reference::ConfigKind::Head {
            query = match &ref_model.name {
                Some(name) => query.filter(reference::Column::Name.eq(name)),
                None => query.filter(reference::Column::Name.is_null()),
            };
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{env, ffi::OsString, fs, sync::Arc};

    use object_store::memory::InMemory;
    use serial_test::serial;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        internal::config::ConfigKv,
        utils::test::{ChangeDirGuard, ScopedEnvVar, setup_with_new_libra_in},
    };

    struct ClearedEnvVarGuard {
        key: String,
        previous: Option<OsString>,
    }

    impl ClearedEnvVarGuard {
        fn new(key: &str) -> Self {
            let previous = env::var_os(key);
            // SAFETY: unit tests mutate process env in a controlled serial context.
            unsafe {
                env::remove_var(key);
            }
            Self {
                key: key.to_string(),
                previous,
            }
        }
    }

    impl Drop for ClearedEnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: this restores the exact previous value for the same env key.
            unsafe {
                if let Some(value) = &self.previous {
                    env::set_var(&self.key, value);
                } else {
                    env::remove_var(&self.key);
                }
            }
        }
    }

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

    /// Scenario: metadata restore into a freshly initialized repo where local refs
    /// have `remote = NULL`. This is the edge hit by live cloud restore: SQL
    /// `remote = NULL` does not match existing rows, so the restore must use
    /// `IS NULL` and update the existing HEAD/branch rows instead of inserting
    /// duplicates that leave HEAD pointing at the init-time repository state.
    #[test]
    #[serial]
    fn restore_metadata_updates_existing_null_remote_references() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        let home = tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", home.path());
        let _test_home = ScopedEnvVar::set("LIBRA_TEST_HOME", home.path());
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());

        rt.block_on(async {
            let db_conn = db::get_db_conn_instance().await;
            let restored_commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
            let restored_refs = vec![
                reference::Model {
                    id: 0,
                    name: Some("restored-main".to_string()),
                    kind: reference::ConfigKind::Head,
                    commit: None,
                    remote: None,
                },
                reference::Model {
                    id: 0,
                    name: Some("intent".to_string()),
                    kind: reference::ConfigKind::Branch,
                    commit: Some(restored_commit.clone()),
                    remote: None,
                },
            ];
            let remote = RemoteStorage::new(Arc::new(InMemory::new()));
            let metadata = serde_json::to_vec(&restored_refs).unwrap();
            remote.put_metadata(&metadata).await.unwrap();

            restore_metadata(&db_conn, &remote)
                .await
                .expect("metadata restore should update existing NULL-remote refs");

            let heads = reference::Entity::find()
                .filter(reference::Column::Kind.eq(reference::ConfigKind::Head))
                .filter(reference::Column::Remote.is_null())
                .all(&db_conn)
                .await
                .unwrap();
            assert_eq!(heads.len(), 1);
            assert_eq!(heads[0].name.as_deref(), Some("restored-main"));

            let intent_refs = reference::Entity::find()
                .filter(reference::Column::Kind.eq(reference::ConfigKind::Branch))
                .filter(reference::Column::Name.eq("intent"))
                .filter(reference::Column::Remote.is_null())
                .all(&db_conn)
                .await
                .unwrap();
            assert_eq!(intent_refs.len(), 1);
            assert_eq!(intent_refs[0].commit.as_ref(), Some(&restored_commit));
        });
    }

    #[test]
    #[serial]
    fn restore_metadata_strict_fails_when_metadata_object_is_missing() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        let home = tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", home.path());
        let _test_home = ScopedEnvVar::set("LIBRA_TEST_HOME", home.path());
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());

        rt.block_on(async {
            let db_conn = db::get_db_conn_instance().await;
            let remote = RemoteStorage::new(Arc::new(InMemory::new()));

            let error = restore_metadata_strict(&db_conn, &remote)
                .await
                .expect_err("strict metadata restore must fail on missing metadata.json");

            assert!(
                error.contains("failed to download metadata"),
                "error should explain metadata download failure: {error}",
            );
        });
    }

    #[test]
    #[serial]
    fn create_r2_storage_reads_values_from_local_config() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        let home = tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", home.path());
        let _test_home = ScopedEnvVar::set("LIBRA_TEST_HOME", home.path());
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());
        let _endpoint = ClearedEnvVarGuard::new("LIBRA_STORAGE_ENDPOINT");
        let _bucket = ClearedEnvVarGuard::new("LIBRA_STORAGE_BUCKET");
        let _access = ClearedEnvVarGuard::new("LIBRA_STORAGE_ACCESS_KEY");
        let _secret = ClearedEnvVarGuard::new("LIBRA_STORAGE_SECRET_KEY");
        let _region = ClearedEnvVarGuard::new("LIBRA_STORAGE_REGION");

        let repo_db_path = repo.path().join(".libra").join(util::DATABASE);

        rt.block_on(crate::internal::vault::lazy_init_vault_for_scope("local"))
            .unwrap();

        let encrypted_endpoint = rt
            .block_on(crate::internal::config::encrypt_value(
                "https://storage.example.com",
                "local",
            ))
            .unwrap();
        let encrypted_bucket = rt
            .block_on(crate::internal::config::encrypt_value(
                "test-bucket",
                "local",
            ))
            .unwrap();
        let encrypted_access = rt
            .block_on(crate::internal::config::encrypt_value(
                "test-access",
                "local",
            ))
            .unwrap();
        let encrypted_secret = rt
            .block_on(crate::internal::config::encrypt_value(
                "test-secret",
                "local",
            ))
            .unwrap();
        let encrypted_region = rt
            .block_on(crate::internal::config::encrypt_value("auto", "local"))
            .unwrap();

        rt.block_on(async {
            ConfigKv::set(
                "vault.env.LIBRA_STORAGE_ENDPOINT",
                &encrypted_endpoint,
                true,
            )
            .await
            .unwrap();
            ConfigKv::set("vault.env.LIBRA_STORAGE_BUCKET", &encrypted_bucket, true)
                .await
                .unwrap();
            ConfigKv::set(
                "vault.env.LIBRA_STORAGE_ACCESS_KEY",
                &encrypted_access,
                true,
            )
            .await
            .unwrap();
            ConfigKv::set(
                "vault.env.LIBRA_STORAGE_SECRET_KEY",
                &encrypted_secret,
                true,
            )
            .await
            .unwrap();
            ConfigKv::set("vault.env.LIBRA_STORAGE_REGION", &encrypted_region, true)
                .await
                .unwrap();
        });

        let _manifest_dir = ChangeDirGuard::new(env!("CARGO_MANIFEST_DIR"));

        rt.block_on(create_r2_storage_for_db_path(
            "repo-from-config",
            &repo_db_path,
        ))
        .expect("R2 storage should initialize from local config values even after cwd drift");
    }

    /// Build a minimum-viable `AgentSessionRow` for the restore-fixture tests.
    /// Defaults to a kind/state pair that satisfies the schema's CHECK
    /// constraints; tests override fields they care about.
    fn fixture_session_row(session_id: &str, provider_session_id: &str) -> AgentSessionRow {
        AgentSessionRow {
            session_id: session_id.to_string(),
            agent_kind: "claude_code".to_string(),
            provider_session_id: provider_session_id.to_string(),
            state: "active".to_string(),
            working_dir: "/tmp/fixture".to_string(),
            worktree_id: None,
            parent_commit: None,
            parent_session_id: None,
            metadata_json: "{}".to_string(),
            redaction_report: "{}".to_string(),
            started_at: 1_700_000_000,
            last_event_at: 1_700_000_001,
            stopped_at: None,
            schema_version: 1,
        }
    }

    fn fixture_checkpoint_row(
        checkpoint_id: &str,
        session_id: &str,
        description: Option<&str>,
    ) -> AgentCheckpointRow {
        AgentCheckpointRow {
            checkpoint_id: checkpoint_id.to_string(),
            session_id: session_id.to_string(),
            parent_checkpoint_id: None,
            scope: "committed".to_string(),
            parent_commit: None,
            tree_oid: "0000000000000000000000000000000000000000".to_string(),
            metadata_blob_oid: "1111111111111111111111111111111111111111".to_string(),
            traces_commit: "2222222222222222222222222222222222222222".to_string(),
            tool_use_id: None,
            subagent_session_id: None,
            description: description.map(String::from),
            created_at: 1_700_000_010,
        }
    }

    /// Codex Q5 fixture: a fresh restore inserts both sessions and
    /// checkpoints into the local catalog. Smoke-tests the happy path
    /// without spinning up a D1 client.
    #[test]
    #[serial]
    fn restore_agent_capture_inserts_fresh_rows() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        let home = tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", home.path());
        let _test_home = ScopedEnvVar::set("LIBRA_TEST_HOME", home.path());
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());

        rt.block_on(async {
            let db_conn = db::get_db_conn_instance().await;
            let sessions = vec![fixture_session_row("sess-A", "prov-A")];
            let checkpoints = vec![fixture_checkpoint_row("ckpt-A", "sess-A", Some("first"))];

            restore_agent_capture_from_rows(&db_conn, &sessions, &checkpoints)
                .await
                .expect("fresh restore should succeed");

            let session_count = scalar_count(&db_conn, "SELECT COUNT(*) AS n FROM agent_session")
                .await
                .unwrap();
            let checkpoint_count =
                scalar_count(&db_conn, "SELECT COUNT(*) AS n FROM agent_checkpoint")
                    .await
                    .unwrap();
            assert_eq!(session_count, 1);
            assert_eq!(checkpoint_count, 1);
        });
    }

    /// Codex Q5 fixture: re-running restore over an existing session row
    /// with the same `(agent_kind, provider_session_id)` MUST update the
    /// existing row in place rather than inserting a duplicate or erroring
    /// on the unique index (`idx_agent_session_provider`).
    #[test]
    #[serial]
    fn restore_agent_capture_upserts_existing_session_on_conflict() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        let home = tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", home.path());
        let _test_home = ScopedEnvVar::set("LIBRA_TEST_HOME", home.path());
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());

        rt.block_on(async {
            let db_conn = db::get_db_conn_instance().await;
            let initial = vec![fixture_session_row("sess-A", "prov-A")];
            restore_agent_capture_from_rows(&db_conn, &initial, &[])
                .await
                .expect("first restore");

            let mut updated = fixture_session_row("sess-A", "prov-A");
            updated.state = "stopped".to_string();
            updated.last_event_at = 1_800_000_000;
            updated.stopped_at = Some(1_800_000_000);

            restore_agent_capture_from_rows(&db_conn, &[updated], &[])
                .await
                .expect("conflict update");

            let session_count = scalar_count(&db_conn, "SELECT COUNT(*) AS n FROM agent_session")
                .await
                .unwrap();
            assert_eq!(session_count, 1, "no duplicate row");

            let stopped_count = scalar_count(
                &db_conn,
                "SELECT COUNT(*) AS n FROM agent_session WHERE state = 'stopped'",
            )
            .await
            .unwrap();
            assert_eq!(stopped_count, 1, "state column reflects updated row");
        });
    }

    /// Codex Q1 + Q5 fixture: checkpoint conflict goes through the
    /// explicit `ON CONFLICT(checkpoint_id) DO UPDATE SET …` path. We
    /// verify by mutating `description` and checking the column was
    /// rewritten on the second restore.
    #[test]
    #[serial]
    fn restore_agent_capture_upserts_existing_checkpoint_on_conflict() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        let home = tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", home.path());
        let _test_home = ScopedEnvVar::set("LIBRA_TEST_HOME", home.path());
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());

        rt.block_on(async {
            let db_conn = db::get_db_conn_instance().await;
            let session = vec![fixture_session_row("sess-A", "prov-A")];
            let initial = vec![fixture_checkpoint_row("ckpt-A", "sess-A", Some("v1"))];
            restore_agent_capture_from_rows(&db_conn, &session, &initial)
                .await
                .expect("first restore");

            let updated = vec![fixture_checkpoint_row("ckpt-A", "sess-A", Some("v2"))];
            restore_agent_capture_from_rows(&db_conn, &session, &updated)
                .await
                .expect("conflict update");

            use sea_orm::Statement;
            let backend = db_conn.get_database_backend();
            let row = db_conn
                .query_one(Statement::from_sql_and_values(
                    backend,
                    "SELECT description FROM agent_checkpoint WHERE checkpoint_id = ?",
                    ["ckpt-A".into()],
                ))
                .await
                .unwrap()
                .expect("row present");
            let description: Option<String> = row.try_get_by(0).unwrap();
            assert_eq!(
                description.as_deref(),
                Some("v2"),
                "ON CONFLICT DO UPDATE rewrote description"
            );

            let count = scalar_count(&db_conn, "SELECT COUNT(*) AS n FROM agent_checkpoint")
                .await
                .unwrap();
            assert_eq!(count, 1, "no duplicate checkpoint row");
        });
    }

    /// Codex Q2 + Q5 fixture: a partial failure (one row violates the
    /// CHECK constraint on `agent_kind`) MUST surface as `Err(...)` from
    /// the helper so the cloud-restore caller treats the restore as
    /// strict. The valid sibling row should still land in the catalog —
    /// we don't roll back, we report.
    #[test]
    #[serial]
    fn restore_agent_capture_partial_failure_returns_err() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        let home = tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", home.path());
        let _test_home = ScopedEnvVar::set("LIBRA_TEST_HOME", home.path());
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());

        rt.block_on(async {
            let db_conn = db::get_db_conn_instance().await;
            let mut bad = fixture_session_row("sess-bad", "prov-bad");
            bad.agent_kind = "not_a_real_kind".to_string(); // violates CHECK
            let good = fixture_session_row("sess-good", "prov-good");

            let err = restore_agent_capture_from_rows(&db_conn, &[bad, good], &[])
                .await
                .expect_err("strict restore should bubble the failure");
            assert!(
                err.contains("session") || err.contains("checkpoint"),
                "error message identifies the failing kind: {err}"
            );

            // Good row still landed — we report aggregate failure but do not
            // roll back; that matches the helper's documented contract.
            let good_count = scalar_count(
                &db_conn,
                "SELECT COUNT(*) AS n FROM agent_session WHERE session_id = 'sess-good'",
            )
            .await
            .unwrap();
            assert_eq!(good_count, 1);
        });
    }

    /// Codex round-2 follow-up Q4: when the local `agent_checkpoint`
    /// table is missing (partial schema), `restore_agent_capture_from_d1`
    /// must take the warning-and-bail path rather than proceed to insert
    /// rows into a half-built catalogue. This test simulates that
    /// scenario by dropping the checkpoint table after init.
    #[test]
    #[serial]
    fn restore_agent_capture_warns_when_checkpoint_table_missing() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        let home = tempdir().unwrap();
        let _home = ScopedEnvVar::set("HOME", home.path());
        let _test_home = ScopedEnvVar::set("LIBRA_TEST_HOME", home.path());
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());

        rt.block_on(async {
            let db_conn = db::get_db_conn_instance().await;
            use sea_orm::Statement;
            let backend = db_conn.get_database_backend();
            // Drop the checkpoint table to simulate a partial schema. We
            // exercise the local-presence guard, not the D1 list call —
            // the helper bails before either ensure_*_table fires.
            db_conn
                .execute(Statement::from_sql_and_values(
                    backend,
                    "DROP TABLE agent_checkpoint",
                    [],
                ))
                .await
                .expect("drop checkpoint table");

            // Build a stub D1Client that we never actually call. The
            // helper short-circuits on the local-schema check before
            // touching the network, so the stub credentials are never
            // dereferenced.
            let d1_client = D1Client::new(
                "stub-account".to_string(),
                "stub-token".to_string(),
                "stub-database".to_string(),
            );

            let result = restore_agent_capture_from_d1(&db_conn, &d1_client, "fixture-repo").await;
            assert!(
                result.is_ok(),
                "partial-schema path returns Ok with a warning, not Err: {:?}",
                result.err()
            );
        });
    }

    /// Tiny helper for the fixture tests above. Mirrors the shape of
    /// `agent::doctor::scalar_count` but lives in this module so the cloud
    /// tests don't depend on a binary-only helper.
    async fn scalar_count(
        conn: &sea_orm::DatabaseConnection,
        sql: &str,
    ) -> Result<i64, sea_orm::DbErr> {
        use sea_orm::Statement;
        let backend = conn.get_database_backend();
        let row = conn
            .query_one(Statement::from_sql_and_values(backend, sql, []))
            .await?
            .ok_or(sea_orm::DbErr::Custom("count returned no rows".to_string()))?;
        row.try_get_by::<i64, _>("n")
    }

    #[test]
    #[serial]
    fn validate_cloud_backup_env_surfaces_config_resolution_errors() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let repo = tempdir().unwrap();
        rt.block_on(setup_with_new_libra_in(repo.path()));
        let _cwd = ChangeDirGuard::new(repo.path());
        let _account = ClearedEnvVarGuard::new("LIBRA_D1_ACCOUNT_ID");
        let _token = ClearedEnvVarGuard::new("LIBRA_D1_API_TOKEN");
        let _database = ClearedEnvVarGuard::new("LIBRA_D1_DATABASE_ID");

        let bad_global_dir = tempdir().unwrap();
        let bad_global_db = bad_global_dir.path().join("bad-global.db");
        fs::write(&bad_global_db, "not sqlite").unwrap();
        let _global_db = ScopedEnvVar::set("LIBRA_CONFIG_GLOBAL_DB", &bad_global_db);

        let err = rt
            .block_on(validate_cloud_backup_env(true))
            .expect_err("global config resolution failure should surface");
        assert!(
            err.contains("failed to open config database")
                || err.contains("failed to connect to global config"),
            "unexpected error: {err}"
        );
    }
}
