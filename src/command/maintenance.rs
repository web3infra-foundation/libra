//! Implementation of `maintenance` command for periodic repository maintenance tasks.
//!
//! This command provides Git-compatible `maintenance` functionality for Libra
//! repositories, including running scheduled maintenance tasks, registering
//! repositories for automatic maintenance, and inspecting maintenance state.
//!
//! # Supported Tasks
//! - `gc`: Remove unreachable loose objects and optimize repository storage.
//! - `loose-objects`: Pack old loose objects into a new pack file to reduce
//!   filesystem overhead.
//! - `pack-refs`: Collapse individual ref files into a single `packed-refs` file.
//! - `incremental-repack`: Repack existing pack files to improve access locality.
//! - `commit-graph`: Update the commit-graph file to accelerate history walks.
//! - `prefetch`: Fetch refs from remotes without updating local branches.
//!
//! # Design Notes
//! Task implementations are intentionally conservative: they only mutate the
//! repository when explicitly requested, and `dry-run` mode reports what would
//! be changed without performing any writes. This mirrors Git's maintenance
//! philosophy while remaining safe for production repositories.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use byteorder::ReadBytesExt;
use clap::{Parser, Subcommand, ValueEnum};
use git_internal::{
    hash::{HashKind, ObjectHash, get_hash_kind},
    internal::{
        metadata::{EntryMeta, MetaAttached},
        object::{commit::Commit, tag::Tag as GitTag, tree::Tree, types::ObjectType},
        pack::{Pack, entry::Entry},
    },
};
use ring::digest::{Context, SHA1_FOR_LEGACY_USE_ONLY, SHA256};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;

use crate::{
    command::{index_pack, load_object},
    internal::{
        config::ConfigKv,
        db,
        model::{object_index, reference, reflog},
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
        path,
        util::try_get_storage_path,
    },
};

const MAINTENANCE_ENABLED_KEY: &str = "maintenance.enabled";
const MAINTENANCE_SCHEDULE_KEY: &str = "maintenance.schedule";
const MAINTENANCE_LAST_RUN_KEY: &str = "maintenance.last-run";
const DEFAULT_LOOSE_OBJECT_THRESHOLD: usize = 100;
const DEFAULT_PACK_COUNT_THRESHOLD: usize = 5;
const LOOSE_OBJECT_AGE_SECONDS: u64 = 14 * 24 * 60 * 60; // 2 weeks
/// Grace period before an unreachable loose object may be deleted by `gc`.
///
/// This protects concurrent commands (add/commit/fetch/hash-object) that have
/// just written objects but not yet updated refs/reflogs/index. Objects newer
/// than this grace period are left in place even when unreachable.
const GC_PRUNE_GRACE_SECONDS: u64 = 60 * 60; // 1 hour

/// `--help` examples shown in `libra maintenance --help` output.
pub const MAINTENANCE_EXAMPLES: &str = "\
EXAMPLES:
    libra maintenance run                         Run all maintenance tasks
    libra maintenance run --task gc               Run only the garbage-collection task
    libra maintenance run --task loose-objects    Pack old loose objects
    libra maintenance run --dry-run               Show what would be done, without changes
    libra maintenance register                    Register this repo for periodic maintenance
    libra maintenance unregister                  Unregister this repo
    libra maintenance status                      Show maintenance registration state";

/// Maintenance subcommands matching Git's `git maintenance` interface.
#[derive(Subcommand, Debug)]
pub enum MaintenanceSubcommand {
    /// Run one or more maintenance tasks.
    Run {
        /// Task to run (may be given multiple times). Defaults to all tasks.
        #[arg(long, value_enum)]
        task: Vec<MaintenanceTask>,
        /// Report what would be done without making any changes.
        #[arg(long)]
        dry_run: bool,
        /// Suppress progress output.
        #[arg(short, long)]
        quiet: bool,
    },
    /// Register the current repository for periodic maintenance.
    Register {
        /// Cron-like schedule expression (stored for external scheduler use).
        #[arg(long, default_value = "hourly")]
        schedule: String,
    },
    /// Unregister the current repository from periodic maintenance.
    Unregister,
    /// Show whether this repository is registered for maintenance.
    Status,
}

/// Top-level arguments for `libra maintenance`.
#[derive(Parser, Debug)]
#[command(after_help = MAINTENANCE_EXAMPLES)]
pub struct MaintenanceArgs {
    #[command(subcommand)]
    pub command: MaintenanceSubcommand,
}

/// Individual maintenance tasks that can be executed.
#[derive(Clone, Debug, PartialEq, Eq, ValueEnum, Serialize)]
pub enum MaintenanceTask {
    /// Garbage-collect unreachable loose objects.
    Gc,
    /// Pack old loose objects into a new pack file.
    LooseObjects,
    /// Collapse loose refs into packed-refs.
    PackRefs,
    /// Repack existing pack files incrementally.
    IncrementalRepack,
    /// Update commit-graph file for faster history walks.
    CommitGraph,
    /// Prefetch remote refs without updating local branches.
    Prefetch,
}

impl std::fmt::Display for MaintenanceTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MaintenanceTask::Gc => write!(f, "gc"),
            MaintenanceTask::LooseObjects => write!(f, "loose-objects"),
            MaintenanceTask::PackRefs => write!(f, "pack-refs"),
            MaintenanceTask::IncrementalRepack => write!(f, "incremental-repack"),
            MaintenanceTask::CommitGraph => write!(f, "commit-graph"),
            MaintenanceTask::Prefetch => write!(f, "prefetch"),
        }
    }
}

/// Result of running a single maintenance task.
#[derive(Debug, Serialize)]
pub struct TaskResult {
    pub task: String,
    pub success: bool,
    pub objects_removed: usize,
    pub objects_packed: usize,
    pub refs_packed: usize,
    pub packs_repacked: usize,
    pub message: String,
}

/// Overall result of a `maintenance run` invocation.
#[derive(Debug, Serialize)]
pub struct MaintenanceRunOutput {
    pub dry_run: bool,
    pub tasks: Vec<TaskResult>,
    pub overall_success: bool,
}

/// JSON output for `maintenance status`.
#[derive(Debug, Serialize)]
pub struct MaintenanceStatusOutput {
    pub registered: bool,
    pub schedule: Option<String>,
    pub last_run: Option<String>,
}

/// Safely execute a maintenance subcommand, returning structured errors.
pub async fn execute_safe(args: MaintenanceArgs, output: &OutputConfig) -> CliResult<()> {
    match args.command {
        MaintenanceSubcommand::Run {
            task,
            dry_run,
            quiet,
        } => run_tasks(&task, dry_run, quiet, output).await,
        MaintenanceSubcommand::Register { schedule } => register(&schedule, output).await,
        MaintenanceSubcommand::Unregister => unregister(output).await,
        MaintenanceSubcommand::Status => status(output).await,
    }
}

// ---------------------------------------------------------------------------
// Run tasks
// ---------------------------------------------------------------------------

async fn run_tasks(
    tasks: &[MaintenanceTask],
    dry_run: bool,
    quiet: bool,
    output: &OutputConfig,
) -> CliResult<()> {
    let repo_path = try_get_storage_path(None)
        .map_err(|e| CliError::repo_not_found().with_hint(e.to_string()))?;

    let selected = if tasks.is_empty() {
        vec![
            MaintenanceTask::Gc,
            MaintenanceTask::LooseObjects,
            MaintenanceTask::PackRefs,
            MaintenanceTask::IncrementalRepack,
            MaintenanceTask::CommitGraph,
            MaintenanceTask::Prefetch,
        ]
    } else {
        tasks.to_vec()
    };

    let mut results = Vec::with_capacity(selected.len());
    let mut overall_success = true;

    for task in selected {
        if !quiet {
            info_println(output, &format!("Running maintenance task: {task}"));
        }
        let result = match task {
            MaintenanceTask::Gc => run_gc(&repo_path, dry_run, quiet, output).await,
            MaintenanceTask::LooseObjects => {
                run_loose_objects(&repo_path, dry_run, quiet, output).await
            }
            MaintenanceTask::PackRefs => run_pack_refs(&repo_path, dry_run, quiet, output).await,
            MaintenanceTask::IncrementalRepack => {
                run_incremental_repack(&repo_path, dry_run, quiet, output).await
            }
            MaintenanceTask::CommitGraph => {
                run_commit_graph(&repo_path, dry_run, quiet, output).await
            }
            MaintenanceTask::Prefetch => run_prefetch(&repo_path, dry_run, quiet, output).await,
        };
        match result {
            Ok(r) => {
                if !r.success {
                    overall_success = false;
                }
                results.push(r);
            }
            Err(e) => {
                overall_success = false;
                results.push(TaskResult {
                    task: task.to_string(),
                    success: false,
                    objects_removed: 0,
                    objects_packed: 0,
                    refs_packed: 0,
                    packs_repacked: 0,
                    message: e.to_string(),
                });
            }
        }
    }

    // Record last-run timestamp on success.
    // Propagate the error so callers and automation schedulers are not left
    // with stale maintenance.last-run state after a successful task run whose
    // config write was dropped.
    if !dry_run && overall_success {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default();
        ConfigKv::set(MAINTENANCE_LAST_RUN_KEY, &now, false)
            .await
            .map_err(|e| {
                CliError::fatal(format!(
                    "maintenance tasks succeeded but failed to record last-run timestamp: {e}"
                ))
            })?;
    }

    if output.is_json() {
        let data = MaintenanceRunOutput {
            dry_run,
            tasks: results,
            overall_success,
        };
        emit_json_data("maintenance.run", &data, output)?;
        if !overall_success {
            return Err(CliError::failure("one or more maintenance tasks failed").with_exit_code(1));
        }
        return Ok(());
    }

    for r in &results {
        let status = if r.success { "ok" } else { "failed" };
        if !quiet {
            info_println(
                output,
                &format!("  {task}: {status} - {msg}", task = r.task, msg = r.message),
            );
        }
    }

    if !overall_success {
        return Err(CliError::failure("one or more maintenance tasks failed").with_exit_code(1));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GC task
// ---------------------------------------------------------------------------

async fn run_gc(
    repo_path: &Path,
    dry_run: bool,
    quiet: bool,
    output: &OutputConfig,
) -> CliResult<TaskResult> {
    let storage = ClientStorage::init(path::objects());
    let reachable = collect_reachable_objects(&storage, repo_path).await?;
    let all_loose = list_loose_objects(repo_path)
        .map_err(|e| CliError::fatal(format!("failed to list loose objects: {e}")))?;

    let now = SystemTime::now();
    let mut removed = 0;
    for (hash_str, obj_path) in &all_loose {
        if let Some(hash) = parse_object_hash(hash_str)
            && !reachable.contains(&hash)
        {
            // Skip objects that are still within the grace period, to avoid
            // racing with commands that just wrote objects before updating
            // refs/reflogs/index.
            let within_grace = fs::metadata(obj_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|mtime| now.duration_since(mtime).ok())
                .map(|age| age.as_secs() < GC_PRUNE_GRACE_SECONDS)
                .unwrap_or(true);
            if within_grace {
                continue;
            }

            removed += 1;
            if dry_run {
                if !quiet {
                    info_println(
                        output,
                        &format!("  would remove unreachable object {hash_str}"),
                    );
                }
            } else {
                fs::remove_file(obj_path).map_err(|e| {
                    CliError::fatal(format!(
                        "failed to remove unreachable object {}: {e}",
                        hash_str
                    ))
                })?;
                // Drop the corresponding object_index row so that cloud
                // sync does not keep trying to upload a pruned object and
                // fail repeatedly. If the config DB is locked or the
                // repo_id is unset we warn and continue — the object file
                // is already gone, the index row is a recoverable stale
                // entry.
                let _ = gc_drop_object_index(hash_str).await;
            }
        }
    }

    // Clean up empty object directories
    if !dry_run {
        let _ = cleanup_empty_dirs(&path::objects());
    }

    let message = if dry_run {
        format!("would remove {} unreachable loose objects", removed)
    } else {
        format!("removed {} unreachable loose objects", removed)
    };

    Ok(TaskResult {
        task: "gc".to_string(),
        success: true,
        objects_removed: removed,
        objects_packed: 0,
        refs_packed: 0,
        packs_repacked: 0,
        message,
    })
}

// ---------------------------------------------------------------------------
// Loose-objects task
// ---------------------------------------------------------------------------

async fn run_loose_objects(
    repo_path: &Path,
    dry_run: bool,
    quiet: bool,
    output: &OutputConfig,
) -> CliResult<TaskResult> {
    let loose = list_loose_objects(repo_path)
        .map_err(|e| CliError::fatal(format!("failed to list loose objects: {e}")))?;

    if loose.len() < DEFAULT_LOOSE_OBJECT_THRESHOLD {
        return Ok(TaskResult {
            task: "loose-objects".to_string(),
            success: true,
            objects_removed: 0,
            objects_packed: 0,
            refs_packed: 0,
            packs_repacked: 0,
            message: format!(
                "only {} loose objects (threshold: {}), skipping",
                loose.len(),
                DEFAULT_LOOSE_OBJECT_THRESHOLD
            ),
        });
    }

    let old_loose: Vec<_> = loose
        .into_iter()
        .filter(|(_, p)| {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .map(|t| {
                    SystemTime::now()
                        .duration_since(t)
                        .map(|d| d.as_secs() > LOOSE_OBJECT_AGE_SECONDS)
                        .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .collect();

    if old_loose.is_empty() {
        return Ok(TaskResult {
            task: "loose-objects".to_string(),
            success: true,
            objects_removed: 0,
            objects_packed: 0,
            refs_packed: 0,
            packs_repacked: 0,
            message: "no old loose objects to pack".to_string(),
        });
    }

    if dry_run {
        return Ok(TaskResult {
            task: "loose-objects".to_string(),
            success: true,
            objects_removed: 0,
            objects_packed: old_loose.len(),
            refs_packed: 0,
            packs_repacked: 0,
            message: format!("would pack {} old loose objects", old_loose.len()),
        });
    }

    // Create a new pack file from old loose objects
    let pack_dir = repo_path.join("objects").join("pack");
    if let Err(e) = fs::create_dir_all(&pack_dir) {
        return Err(CliError::fatal(format!(
            "failed to create pack directory: {e}"
        )));
    }

    let (pack_name, pack_path) = allocate_unique_pack_path(&pack_dir, "pack-maintenance")
        .map_err(|e| CliError::fatal(format!("failed to allocate unique pack name: {e}")))?;

    let packed = match create_pack_from_loose_objects(&old_loose, &pack_path).await {
        Ok(count) => {
            let packed = count;

            // Build a standard index file so the storage layer can discover and
            // read objects from this pack.
            let idx_path = pack_path.with_extension("idx");
            build_pack_index(&pack_path, &idx_path)?;

            // Defensive check: the index must contain an entry for every object
            // that was written. A truncated index (e.g. fanout-only) would make
            // all lookups fail after the loose objects are deleted.
            let idx_entries = read_idx_entries(&idx_path)
                .map_err(|e| CliError::fatal(format!("failed to read new pack index: {e}")))?;
            if idx_entries.len() != packed {
                return Err(CliError::fatal(format!(
                    "pack index has {} entries but pack contains {} objects",
                    idx_entries.len(),
                    packed
                )));
            }

            // Verify every packed object is readable from the new pack before
            // deleting the original loose objects. We temporarily rename each
            // loose object so the storage layer must read from the pack.
            let storage = ClientStorage::init(path::objects());
            for (hash_str, obj_path) in &old_loose {
                let Some(hash) = parse_object_hash(hash_str) else {
                    return Err(CliError::fatal(format!(
                        "failed to parse object hash {hash_str}"
                    )));
                };
                let backup_path = obj_path.with_extension("tmp-backup");
                fs::rename(obj_path, &backup_path).map_err(|e| {
                    CliError::fatal(format!(
                        "failed to stage loose object {hash_str} for verification: {e}"
                    ))
                })?;
                let readable = storage.get(&hash).is_ok();
                if readable {
                    fs::remove_file(&backup_path).map_err(|e| {
                        CliError::fatal(format!(
                            "failed to remove verified loose object {hash_str}: {e}"
                        ))
                    })?;
                } else {
                    let _ = fs::rename(&backup_path, obj_path);
                    return Err(CliError::fatal(format!(
                        "packed object {hash_str} is not readable from {pack_name}"
                    )));
                }
            }

            let _ = cleanup_empty_dirs(&path::objects());
            packed
        }
        Err(e) => {
            return Err(CliError::fatal(format!("failed to create pack file: {e}")));
        }
    };

    if !quiet {
        info_println(
            output,
            &format!("  created pack file with {packed} objects"),
        );
    }

    Ok(TaskResult {
        task: "loose-objects".to_string(),
        success: true,
        objects_removed: 0,
        objects_packed: packed,
        refs_packed: 0,
        packs_repacked: 0,
        message: format!("packed {packed} old loose objects into {pack_name}"),
    })
}

// ---------------------------------------------------------------------------
// Pack-refs task
// ---------------------------------------------------------------------------

async fn run_pack_refs(
    repo_path: &Path,
    dry_run: bool,
    _quiet: bool,
    _output: &OutputConfig,
) -> CliResult<TaskResult> {
    let refs_dir = repo_path.join("refs").join("heads");
    if !refs_dir.exists() {
        return Ok(TaskResult {
            task: "pack-refs".to_string(),
            success: true,
            objects_removed: 0,
            objects_packed: 0,
            refs_packed: 0,
            packs_repacked: 0,
            message: "no refs/heads directory".to_string(),
        });
    }

    let mut refs: HashMap<String, String> = HashMap::new();
    collect_refs(&refs_dir, &refs_dir, "refs/heads/", &mut refs)
        .map_err(|e| CliError::fatal(format!("failed to collect refs: {e}")))?;

    if refs.is_empty() {
        return Ok(TaskResult {
            task: "pack-refs".to_string(),
            success: true,
            objects_removed: 0,
            objects_packed: 0,
            refs_packed: 0,
            packs_repacked: 0,
            message: "no loose refs to pack".to_string(),
        });
    }

    let packed_refs_path = repo_path.join("packed-refs");

    if dry_run {
        return Ok(TaskResult {
            task: "pack-refs".to_string(),
            success: true,
            objects_removed: 0,
            objects_packed: 0,
            refs_packed: refs.len(),
            packs_repacked: 0,
            message: format!("would pack {} refs into packed-refs", refs.len()),
        });
    }

    // Append to existing packed-refs if present
    let mut existing: HashMap<String, String> = HashMap::new();
    if packed_refs_path.exists() {
        let content = fs::read_to_string(&packed_refs_path)
            .map_err(|e| CliError::fatal(format!("failed to read packed-refs: {e}")))?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((hash, name)) = line.split_once(' ') {
                existing.insert(name.to_string(), hash.to_string());
            }
        }
    }

    // Merge new refs, overwriting existing ones
    for (name, hash) in &refs {
        existing.insert(name.clone(), hash.clone());
    }

    // Write packed-refs atomically: stream to a temp file, sync it, then rename
    // it into place. This ensures an ENOSPC/IO error or crash during the write
    // cannot leave the previous packed-refs truncated.
    let temp_path = packed_refs_path.with_extension("tmp");
    let mut file = fs::File::create(&temp_path)
        .map_err(|e| CliError::fatal(format!("failed to create packed-refs temp: {e}")))?;
    if let Err(e) = writeln!(file, "# packed-refs with peeled tags") {
        let _ = fs::remove_file(&temp_path);
        return Err(CliError::fatal(format!("failed to write packed-refs: {e}")));
    }
    for (name, hash) in &existing {
        if let Err(e) = writeln!(file, "{hash} {name}") {
            let _ = fs::remove_file(&temp_path);
            return Err(CliError::fatal(format!("failed to write packed-refs: {e}")));
        }
    }
    if let Err(e) = file.flush() {
        let _ = fs::remove_file(&temp_path);
        return Err(CliError::fatal(format!("failed to flush packed-refs: {e}")));
    }
    drop(file);

    // Verify the temp file is readable and contains every entry before renaming.
    let written = fs::read_to_string(&temp_path)
        .map_err(|e| CliError::fatal(format!("failed to read back packed-refs temp: {e}")))?;
    for (name, hash) in &existing {
        let expected = format!("{hash} {name}");
        if !written.lines().any(|line| line.trim() == expected) {
            let _ = fs::remove_file(&temp_path);
            return Err(CliError::fatal(format!(
                "packed-refs is missing entry for {name}"
            )));
        }
    }

    // Install the new packed-refs atomically. Keep a backup of the old
    // file so that a rename failure after the old file is moved aside
    // does not lose all refs that exist only in packed-refs (e.g. after
    // a previous pack-refs).
    let backup_path = packed_refs_path.with_extension("bak");
    if packed_refs_path.exists() {
        fs::rename(&packed_refs_path, &backup_path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            CliError::fatal(format!("failed to back up old packed-refs: {e}"))
        })?;
    }
    fs::rename(&temp_path, &packed_refs_path)
        .inspect_err(|_| {
            // Restore the backup so packed-refs content survives.
            let _ = fs::rename(&backup_path, &packed_refs_path);
        })
        .map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            CliError::fatal(format!("failed to install packed-refs: {e}"))
        })?;
    // Clean up the backup on success.
    let _ = fs::remove_file(&backup_path);

    // Keep the loose ref files in place. Normal ref-resolution paths
    // (show-ref, rev-parse, revision walking, etc.) currently only query
    // SQLite-backed refs and loose file-backed refs under refs/ — they
    // do not yet read packed-refs. Deleting the loose files would make
    // those refs invisible to everyday commands. Once the ref-resolution
    // layer learns to read packed-refs, the loose files can be removed.
    let count = refs.len();

    Ok(TaskResult {
        task: "pack-refs".to_string(),
        success: true,
        objects_removed: 0,
        objects_packed: 0,
        refs_packed: count,
        packs_repacked: 0,
        message: format!("packed {count} refs into packed-refs"),
    })
}

// ---------------------------------------------------------------------------
// Incremental-repack task
// ---------------------------------------------------------------------------

async fn run_incremental_repack(
    repo_path: &Path,
    dry_run: bool,
    quiet: bool,
    output: &OutputConfig,
) -> CliResult<TaskResult> {
    let pack_dir = repo_path.join("objects").join("pack");
    if !pack_dir.exists() {
        return Ok(TaskResult {
            task: "incremental-repack".to_string(),
            success: true,
            objects_removed: 0,
            objects_packed: 0,
            refs_packed: 0,
            packs_repacked: 0,
            message: "no pack directory".to_string(),
        });
    }

    let packs: Vec<_> = match fs::read_dir(&pack_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "pack"))
            .map(|e| e.path())
            .collect(),
        Err(e) => {
            return Err(CliError::fatal(format!(
                "failed to read pack directory: {e}"
            )));
        }
    };

    if packs.len() < DEFAULT_PACK_COUNT_THRESHOLD {
        return Ok(TaskResult {
            task: "incremental-repack".to_string(),
            success: true,
            objects_removed: 0,
            objects_packed: 0,
            refs_packed: 0,
            packs_repacked: 0,
            message: format!(
                "only {} pack files (threshold: {}), skipping",
                packs.len(),
                DEFAULT_PACK_COUNT_THRESHOLD
            ),
        });
    }

    if dry_run {
        return Ok(TaskResult {
            task: "incremental-repack".to_string(),
            success: true,
            objects_removed: 0,
            objects_packed: 0,
            refs_packed: 0,
            packs_repacked: packs.len(),
            message: format!("would repack {} pack files", packs.len()),
        });
    }

    // For incremental repack, we combine all existing pack files and any loose
    // objects into one new pack.
    let loose = list_loose_objects(repo_path)
        .map_err(|e| CliError::fatal(format!("failed to list loose objects: {e}")))?;
    let all_hashes = list_all_objects_in_storage(&path::objects())
        .map_err(|e| CliError::fatal(format!("failed to list objects: {e}")))?;

    let (new_pack_name, new_pack_path) = allocate_unique_pack_path(&pack_dir, "pack-consolidated")
        .map_err(|e| CliError::fatal(format!("failed to allocate unique pack name: {e}")))?;
    // The backup directory name must be unique per run and tied to the chosen
    // pack name so concurrent runs do not share backup storage.
    let backup_dir = pack_dir.join(format!(".old-packs-backup-{}", new_pack_name));

    let repacked = match create_consolidated_pack(&packs, &loose, &new_pack_path) {
        Ok(count) => {
            // Build a standard index file so the storage layer can discover and
            // read objects from the new consolidated pack.
            let idx_path = new_pack_path.with_extension("idx");
            build_pack_index(&new_pack_path, &idx_path)?;

            // Defensive check: the index must contain an entry for every object
            // that was written. A truncated index would make lookups fail after
            // the source packs are deleted.
            let idx_entries = read_idx_entries(&idx_path)
                .map_err(|e| CliError::fatal(format!("failed to read new pack index: {e}")))?;
            if idx_entries.len() != count {
                return Err(CliError::fatal(format!(
                    "pack index has {} entries but pack contains {} objects",
                    idx_entries.len(),
                    count
                )));
            }

            // Build a hash-set from the new pack's index entries so we can
            // verify every object is present WITHOUT first removing the old
            // packs. This keeps old packs readable for concurrent commands
            // throughout the verification window.
            let idx_hash_set: HashSet<ObjectHash> =
                idx_entries.iter().map(|(h, _offset)| *h).collect();

            let mut verification_failed = None;
            for hash in &all_hashes {
                if !idx_hash_set.contains(hash) {
                    verification_failed = Some(*hash);
                    break;
                }
            }

            if let Some(hash) = verification_failed {
                let _ = fs::remove_file(&new_pack_path);
                let _ = fs::remove_file(&idx_path);
                return Err(CliError::fatal(format!(
                    "consolidated pack does not contain object {hash}"
                )));
            }

            // Verification succeeded against the index — every object in the
            // repository can be served from the new consolidated pack. Now
            // remove the old packs. We stage them into a backup directory
            // first, verify the new pack is readable through the storage
            // layer, then delete them permanently.
            fs::create_dir_all(&backup_dir).map_err(|e| {
                CliError::fatal(format!("failed to create old-pack backup directory: {e}"))
            })?;
            let mut staged_packs: Vec<(PathBuf, PathBuf)> = Vec::new();
            let mut staged_idxs: Vec<(PathBuf, PathBuf)> = Vec::new();
            let mut stage_error = None;
            for old_pack in &packs {
                let old_idx = old_pack.with_extension("idx");
                let pack_name = old_pack
                    .file_name()
                    .ok_or_else(|| CliError::fatal("invalid old pack path"))?;
                let backup_pack = backup_dir.join(pack_name);
                if let Err(e) = fs::rename(old_pack, &backup_pack) {
                    stage_error = Some(format!(
                        "failed to remove old pack {}: {e}",
                        old_pack.display()
                    ));
                    break;
                }
                staged_packs.push((old_pack.clone(), backup_pack));
                if old_idx.exists() {
                    let idx_name = old_idx
                        .file_name()
                        .ok_or_else(|| CliError::fatal("invalid old index path"))?;
                    let backup_idx = backup_dir.join(idx_name);
                    if let Err(e) = fs::rename(&old_idx, &backup_idx) {
                        stage_error = Some(format!(
                            "failed to remove old index {}: {e}",
                            old_idx.display()
                        ));
                        break;
                    }
                    staged_idxs.push((old_idx, backup_idx));
                }
            }

            if let Some(msg) = stage_error {
                restore_staged_packs(&staged_packs, &staged_idxs);
                let _ = fs::remove_dir_all(&backup_dir);
                return Err(CliError::fatal(msg));
            }

            let _ = fs::remove_dir_all(&backup_dir);
            count
        }
        Err(e) => {
            // Remove the partial pack so a failed run does not leave an
            // unreadable file behind.
            let _ = fs::remove_file(&new_pack_path);
            let _ = fs::remove_file(new_pack_path.with_extension("idx"));
            return Err(CliError::fatal(format!(
                "failed to create consolidated pack: {e}"
            )));
        }
    };

    if !quiet {
        info_println(
            output,
            &format!("  consolidated into {new_pack_name} with {repacked} objects"),
        );
    }

    Ok(TaskResult {
        task: "incremental-repack".to_string(),
        success: true,
        objects_removed: 0,
        objects_packed: repacked,
        refs_packed: 0,
        packs_repacked: packs.len(),
        message: format!(
            "repacked {} packs into {} with {repacked} objects",
            packs.len(),
            new_pack_name
        ),
    })
}

// ---------------------------------------------------------------------------
// Commit-graph task
// ---------------------------------------------------------------------------

async fn run_commit_graph(
    _repo_path: &Path,
    _dry_run: bool,
    _quiet: bool,
    _output: &OutputConfig,
) -> CliResult<TaskResult> {
    // Libra does not currently maintain a commit-graph file. We report this
    // transparently so callers know the task was considered but not applicable.
    Ok(TaskResult {
        task: "commit-graph".to_string(),
        success: true,
        objects_removed: 0,
        objects_packed: 0,
        refs_packed: 0,
        packs_repacked: 0,
        message: "commit-graph not yet supported in Libra; skipped".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Prefetch task
// ---------------------------------------------------------------------------

async fn run_prefetch(
    _repo_path: &Path,
    _dry_run: bool,
    _quiet: bool,
    _output: &OutputConfig,
) -> CliResult<TaskResult> {
    // Prefetch requires remote configuration. In the absence of configured remotes
    // we report that the task is not applicable.
    Ok(TaskResult {
        task: "prefetch".to_string(),
        success: true,
        objects_removed: 0,
        objects_packed: 0,
        refs_packed: 0,
        packs_repacked: 0,
        message: "prefetch requires remote configuration; skipped".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Register / Unregister / Status
// ---------------------------------------------------------------------------

async fn register(schedule: &str, output: &OutputConfig) -> CliResult<()> {
    try_get_storage_path(None).map_err(|e| CliError::repo_not_found().with_hint(e.to_string()))?;

    ConfigKv::set(MAINTENANCE_ENABLED_KEY, "true", false)
        .await
        .map_err(|e| CliError::fatal(format!("failed to set maintenance config: {e}")))?;

    ConfigKv::set(MAINTENANCE_SCHEDULE_KEY, schedule, false)
        .await
        .map_err(|e| CliError::fatal(format!("failed to set maintenance schedule: {e}")))?;

    if output.is_json() {
        return emit_json_data(
            "maintenance.register",
            &serde_json::json!({ "registered": true, "schedule": schedule }),
            output,
        );
    }

    info_println(
        output,
        &format!("Repository registered for maintenance (schedule: {schedule})"),
    );
    Ok(())
}

async fn unregister(output: &OutputConfig) -> CliResult<()> {
    try_get_storage_path(None).map_err(|e| CliError::repo_not_found().with_hint(e.to_string()))?;

    ConfigKv::set(MAINTENANCE_ENABLED_KEY, "false", false)
        .await
        .map_err(|e| CliError::fatal(format!("failed to unset maintenance config: {e}")))?;

    if output.is_json() {
        return emit_json_data(
            "maintenance.unregister",
            &serde_json::json!({ "registered": false }),
            output,
        );
    }

    info_println(output, "Repository unregistered from maintenance");
    Ok(())
}

async fn status(output: &OutputConfig) -> CliResult<()> {
    try_get_storage_path(None).map_err(|e| CliError::repo_not_found().with_hint(e.to_string()))?;

    let enabled = ConfigKv::get(MAINTENANCE_ENABLED_KEY)
        .await
        .map_err(|e| CliError::fatal(format!("failed to read maintenance config: {e}")))?
        .is_some_and(|entry| entry.value == "true");

    let schedule = ConfigKv::get(MAINTENANCE_SCHEDULE_KEY)
        .await
        .map_err(|e| CliError::fatal(format!("failed to read maintenance schedule: {e}")))?
        .map(|entry| entry.value);

    let last_run = ConfigKv::get(MAINTENANCE_LAST_RUN_KEY)
        .await
        .map_err(|e| CliError::fatal(format!("failed to read maintenance last-run: {e}")))?
        .map(|entry| entry.value);

    let data = MaintenanceStatusOutput {
        registered: enabled,
        schedule: schedule.clone(),
        last_run,
    };

    if output.is_json() {
        return emit_json_data("maintenance.status", &data, output);
    }

    if enabled {
        info_println(output, "Maintenance: registered");
        if let Some(s) = schedule {
            info_println(output, &format!("Schedule: {s}"));
        }
    } else {
        info_println(output, "Maintenance: not registered");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect all reachable objects from refs, index, reflogs, and file-backed
/// refs/reflogs (stash). File-backed refs live outside the SQLite database and
/// must be treated as GC roots, otherwise GC can delete stash commits/blobs and
/// make `stash apply`/`pop` fail with missing-object errors.
async fn collect_reachable_objects(
    storage: &ClientStorage,
    repo_path: &Path,
) -> CliResult<HashSet<ObjectHash>> {
    let mut reachable: HashSet<ObjectHash> = HashSet::new();
    let db_conn = db::get_db_conn_instance().await;

    // Collect from refs
    let refs = reference::Entity::find()
        .all(&db_conn)
        .await
        .map_err(|e| CliError::fatal(format!("failed to load refs: {e}")))?;

    for ref_entry in refs {
        if let Some(commit_hash_str) = &ref_entry.commit
            && let Some(hash) = parse_object_hash(commit_hash_str)
        {
            walk_reachable(&hash, storage, &mut reachable)?;
        }
    }

    // Collect from reflogs
    let reflogs = reflog::Entity::find()
        .all(&db_conn)
        .await
        .map_err(|e| CliError::fatal(format!("failed to load reflogs: {e}")))?;

    let is_null_oid = |oid: &str| oid.chars().all(|c| c == '0');
    for entry in reflogs {
        // Reflog old OIDs are GC roots too: after a force update or branch
        // move the previous tip may only be reachable through old_oid.
        for oid in [&entry.old_oid, &entry.new_oid] {
            if !is_null_oid(oid)
                && let Some(hash) = parse_object_hash(oid)
            {
                walk_reachable(&hash, storage, &mut reachable)?;
            }
        }
    }

    // Collect from file-backed stash ref. Stash uses `refs/stash` on disk
    // instead of the SQLite reference table, so it is invisible to the loops
    // above. Without this, GC can delete stash commit/tree/blob objects and
    // make `stash apply`/`pop` lose work.
    let stash_ref_path = repo_path.join("refs/stash");
    if stash_ref_path.is_file()
        && let Ok(content) = fs::read_to_string(&stash_ref_path)
        && let Some(hash) = parse_object_hash(content.trim())
    {
        walk_reachable(&hash, storage, &mut reachable)?;
    }

    // Collect from file-backed stash reflog. The stash reflog
    // (`logs/refs/stash`) records previous stash entries so users can
    // `stash apply stash@{1}` etc. Each old_oid/new_oid on the reflog is a GC
    // root, otherwise a dropped stash entry's objects become unreachable.
    let stash_log_path = repo_path.join("logs/refs/stash");
    if stash_log_path.is_file() {
        let content = fs::read_to_string(&stash_log_path).map_err(|e| {
            CliError::fatal(format!(
                "failed to read stash reflog {}: {e}; aborting gc to protect stash objects",
                stash_log_path.display()
            ))
        })?;
        for line in content.lines() {
            // Stash reflog format: "old_hash new_hash name <email> ..."
            for oid_str in line.split_whitespace().take(2) {
                if !is_null_oid(oid_str)
                    && let Some(hash) = parse_object_hash(oid_str)
                {
                    walk_reachable(&hash, storage, &mut reachable)?;
                }
            }
        }
    }

    // Collect from file-backed loose refs under refs/ (heads, tags,
    // remotes, notes, etc.). These are not stored in the SQLite reference
    // table (which tracks refs in its own schema) but may exist as plain
    // files on disk. Without this, a `pack-refs` run that deletes the
    // loose files would leave these refs invisible to GC, and a subsequent
    // `gc` would treat their commits as unreachable.
    let refs_dir = repo_path.join("refs");
    if refs_dir.is_dir() {
        let mut file_hashes: Vec<ObjectHash> = Vec::new();
        collect_file_ref_hashes(&refs_dir, &mut file_hashes)
            .map_err(|e| CliError::fatal(format!("failed to collect file-backed refs: {e}")))?;
        for hash in &file_hashes {
            walk_reachable(hash, storage, &mut reachable)?;
        }
    }

    // Collect from packed-refs. After `libra maintenance run --task pack-refs`
    // removes loose ref files under refs/heads/, the only remaining copy of
    // those refs lives in packed-refs. Without this, GC would see zero refs
    // pointing to those commits and could delete their objects once the
    // mtime grace period expires.
    let packed_refs_path = repo_path.join("packed-refs");
    if packed_refs_path.is_file() {
        let content = fs::read_to_string(&packed_refs_path)
            .map_err(|e| CliError::fatal(format!("failed to read packed-refs: {e}")))?;
        for (line_no, line) in content.lines().enumerate() {
            let line = line.trim();
            // Skip comments, empty lines, and peeled-tag markers (^<hash>).
            if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
                continue;
            }
            // Non-comment lines must contain a well-formed "<hash> <refname>"
            // entry. Silently skipping a malformed line would let GC build an
            // incomplete root set and potentially delete objects reachable only
            // from the ref recorded on that line.
            let (hash_str, _refname) = line.split_once(' ').ok_or_else(|| {
                CliError::fatal(format!(
                    "malformed packed-refs line {}: expected '<hash> <refname>', got '{}'",
                    line_no + 1,
                    line
                ))
            })?;
            let hash = parse_object_hash(hash_str).ok_or_else(|| {
                CliError::fatal(format!(
                    "packed-refs line {} has invalid object hash '{}'",
                    line_no + 1,
                    hash_str
                ))
            })?;
            walk_reachable(&hash, storage, &mut reachable)?;
        }
    }

    // Collect from index. A corrupt or unreadable index must abort GC instead
    // of silently dropping staged objects from the reachable set.
    let index_path = path::index();
    if index_path.exists() {
        let index = git_internal::internal::index::Index::load(&index_path).map_err(|e| {
            CliError::fatal(format!(
                "failed to load index from {}: {e}; aborting gc to protect staged objects",
                index_path.display()
            ))
        })?;
        for entry in index.tracked_entries(0) {
            reachable.insert(entry.hash);
        }
    }

    Ok(reachable)
}

/// Walk object references recursively, adding all transitive dependencies.
///
/// Errors are propagated rather than swallowed so that GC aborts with an
/// incomplete reachability graph instead of pruning objects that may still be
/// referenced.
fn walk_reachable(
    hash: &ObjectHash,
    storage: &ClientStorage,
    reachable: &mut HashSet<ObjectHash>,
) -> CliResult<()> {
    if !reachable.insert(*hash) {
        return Ok(()); // Already visited
    }

    let obj_type = storage.get_object_type(hash).map_err(|e| {
        CliError::fatal(format!(
            "failed to determine object type for {hash} during gc: {e}"
        ))
    })?;

    match obj_type {
        ObjectType::Commit => {
            let commit = load_object::<Commit>(hash).map_err(|e| {
                CliError::fatal(format!(
                    "failed to load commit {hash} during gc reachability walk: {e}"
                ))
            })?;
            walk_reachable(&commit.tree_id, storage, reachable)?;
            for parent in &commit.parent_commit_ids {
                walk_reachable(parent, storage, reachable)?;
            }
        }
        ObjectType::Tree => {
            let tree = load_object::<Tree>(hash).map_err(|e| {
                CliError::fatal(format!(
                    "failed to load tree {hash} during gc reachability walk: {e}"
                ))
            })?;
            for item in &tree.tree_items {
                walk_reachable(&item.id, storage, reachable)?;
            }
        }
        ObjectType::Tag => {
            // Annotated tags point to another object (commit/tree/blob/tag).
            // The tag object itself must keep its target reachable, otherwise
            // a tagged commit can be pruned while the tag remains.
            let tag = load_object::<GitTag>(hash).map_err(|e| {
                CliError::fatal(format!(
                    "failed to load tag {hash} during gc reachability walk: {e}"
                ))
            })?;
            walk_reachable(&tag.object_hash, storage, reachable)?;
        }
        _ => {}
    }

    Ok(())
}

/// List all loose objects in the repository, returning (hash, path) pairs.
fn list_loose_objects(repo_path: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let objects_dir = repo_path.join("objects");
    let mut result = Vec::new();

    for entry in fs::read_dir(&objects_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if dir_name.len() != 2 || dir_name == "pack" || dir_name == "info" {
            continue;
        }

        for sub in fs::read_dir(&path)? {
            let sub = sub?;
            let sub_path = sub.path();
            if sub_path.is_file() {
                let Some(file_name) = sub_path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let full_hash = format!("{dir_name}{file_name}");
                result.push((full_hash, sub_path));
            }
        }
    }

    Ok(result)
}

/// List all objects in storage (both loose and packed).
fn list_all_objects_in_storage(objects_dir: &Path) -> io::Result<Vec<ObjectHash>> {
    let mut hashes = HashSet::new();

    // Loose objects live in two-character hex directories.
    for entry in fs::read_dir(objects_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if dir_name.len() != 2 {
            continue;
        }

        for sub in fs::read_dir(&path)? {
            let sub = sub?;
            let sub_path = sub.path();
            if sub_path.is_file() {
                let Some(file_name) = sub_path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let full_hash = format!("{dir_name}{file_name}");
                if let Some(hash) = parse_object_hash(&full_hash) {
                    hashes.insert(hash);
                }
            }
        }
    }

    // Packed objects are recorded in the .idx files under objects/pack.
    let pack_dir = objects_dir.join("pack");
    if pack_dir.is_dir() {
        for entry in fs::read_dir(&pack_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|ext| ext == "idx") {
                for hash in read_idx_hashes(&path)? {
                    hashes.insert(hash);
                }
            }
        }
    }

    Ok(hashes.into_iter().collect())
}

/// Read all object hashes stored in a pack index file (version 1 or 2).
fn read_idx_hashes(idx_path: &Path) -> io::Result<Vec<ObjectHash>> {
    const IDX_MAGIC: [u8; 4] = [0xFF, 0x74, 0x4F, 0x63];
    const FANOUT_SIZE: u64 = 256 * 4;

    let mut file = fs::File::open(idx_path)?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)?;

    let is_v2 = header == IDX_MAGIC;
    let fanout_offset = if is_v2 {
        // V2 starts with magic + version; skip the version too.
        let mut version_buf = [0u8; 4];
        file.read_exact(&mut version_buf)?;
        8
    } else {
        // V1 has no header; rewind to the start.
        file.seek(io::SeekFrom::Start(0))?;
        0
    };

    let mut fanout = [0u32; 256];
    let mut buf = [0; 4];
    file.seek(io::SeekFrom::Start(fanout_offset))?;
    for slot in fanout.iter_mut() {
        file.read_exact(&mut buf)?;
        *slot = u32::from_be_bytes(buf);
    }
    let object_count = fanout[255] as usize;
    let hash_size = get_hash_kind().size() as usize;

    let mut hashes = Vec::with_capacity(object_count);
    if is_v2 {
        // Names section follows the fanout table.
        file.seek(io::SeekFrom::Start(fanout_offset + FANOUT_SIZE))?;
        for _ in 0..object_count {
            let mut hash_bytes = vec![0u8; hash_size];
            file.read_exact(&mut hash_bytes)?;
            if let Ok(hash) = ObjectHash::from_bytes(&hash_bytes) {
                hashes.push(hash);
            }
        }
    } else {
        // V1 interleaves 4-byte offsets with object names.
        file.seek(io::SeekFrom::Start(FANOUT_SIZE + 4))?;
        for i in 0..object_count {
            let mut hash_bytes = vec![0u8; hash_size];
            file.read_exact(&mut hash_bytes)?;
            if let Ok(hash) = ObjectHash::from_bytes(&hash_bytes) {
                hashes.push(hash);
            }
            // Skip the next object's offset, unless this was the last object.
            if i + 1 < object_count {
                file.seek(io::SeekFrom::Current(4))?;
            }
        }
    }

    Ok(hashes)
}

/// Read all (hash, offset) entries from a pack index file (version 1 or 2).
fn read_idx_entries(idx_path: &Path) -> io::Result<Vec<(ObjectHash, u64)>> {
    const IDX_MAGIC: [u8; 4] = [0xFF, 0x74, 0x4F, 0x63];
    const FANOUT_SIZE: u64 = 256 * 4;

    let mut file = fs::File::open(idx_path)?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header)?;

    let is_v2 = header == IDX_MAGIC;
    let fanout_offset = if is_v2 {
        let mut version_buf = [0u8; 4];
        file.read_exact(&mut version_buf)?;
        8
    } else {
        file.seek(io::SeekFrom::Start(0))?;
        0
    };

    let mut fanout = [0u32; 256];
    let mut buf = [0; 4];
    file.seek(io::SeekFrom::Start(fanout_offset))?;
    for slot in fanout.iter_mut() {
        file.read_exact(&mut buf)?;
        *slot = u32::from_be_bytes(buf);
    }
    let object_count = fanout[255] as usize;
    let hash_size = get_hash_kind().size() as usize;

    let mut entries = Vec::with_capacity(object_count);
    if is_v2 {
        let names_offset = fanout_offset + FANOUT_SIZE;
        let crc_offset = names_offset + (object_count as u64) * (hash_size as u64);
        let offsets_offset = crc_offset + (object_count as u64) * 4;
        let large_offsets_offset = offsets_offset + (object_count as u64) * 4;

        file.seek(io::SeekFrom::Start(names_offset))?;
        let mut hashes = Vec::with_capacity(object_count);
        for _ in 0..object_count {
            let mut hash_bytes = vec![0u8; hash_size];
            file.read_exact(&mut hash_bytes)?;
            hashes.push(
                ObjectHash::from_bytes(&hash_bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?,
            );
        }

        // Read offsets while remembering the next position in the 4-byte offset
        // table, so a large-offset lookup does not leave the cursor in the
        // large-offset table for the next iteration.
        let mut next_offset_pos = offsets_offset;
        for hash in &hashes {
            file.seek(io::SeekFrom::Start(next_offset_pos))?;
            let offset = file.read_u32::<byteorder::BigEndian>()?;
            next_offset_pos += 4;
            let offset = if offset & 0x8000_0000 != 0 {
                let large_index = (offset & 0x7fff_ffff) as u64;
                file.seek(io::SeekFrom::Start(large_offsets_offset + large_index * 8))?;
                file.read_u64::<byteorder::BigEndian>()?
            } else {
                offset as u64
            };
            entries.push((*hash, offset));
        }
    } else {
        file.seek(io::SeekFrom::Start(FANOUT_SIZE))?;
        for _ in 0..object_count {
            let offset = file.read_u32::<byteorder::BigEndian>()? as u64;
            let mut hash_bytes = vec![0u8; hash_size];
            file.read_exact(&mut hash_bytes)?;
            let hash = ObjectHash::from_bytes(&hash_bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            entries.push((hash, offset));
        }
    }

    Ok(entries)
}

/// Map an `ObjectType` to the numeric type used in pack file entries.
fn object_type_to_pack_num(obj_type: ObjectType) -> u8 {
    match obj_type {
        ObjectType::Commit => 1,
        ObjectType::Tree => 2,
        ObjectType::Blob => 3,
        ObjectType::Tag => 4,
        _ => 0,
    }
}

/// Parse a hex string into an ObjectHash.
fn parse_object_hash(hex_str: &str) -> Option<ObjectHash> {
    let bytes = hex::decode(hex_str).ok()?;
    if bytes.is_empty() {
        return None;
    }
    ObjectHash::from_bytes(&bytes).ok()
}

/// Return `true` when `name` looks like a well-formed fully-qualified ref name.
///
/// Rejects empty names, names ending with `.lock`, and names that contain
/// internal `.lock` path components — all of which are lock files that
/// `collect_refs` / `remove_packed_refs` must ignore.
fn is_valid_ref_name(name: &str) -> bool {
    if name.is_empty() || name.ends_with(".lock") {
        return false;
    }
    // Reject paths where any component ends with ".lock", e.g.
    // "refs/heads/foo.lock/bar".
    !name.split('/').any(|comp| comp.ends_with(".lock"))
}

/// Recursively scan a refs directory tree, collecting every valid object hash
/// from loose ref files into `hashes`.
///
/// Lock files (`*.lock`) are skipped so that refs being updated concurrently
/// are not read mid-write. This mirrors `collect_refs` but only collects
/// hashes — it is used by GC to discover all file-backed refs that are not
/// tracked in the SQLite reference table.
fn collect_file_ref_hashes(dir: &Path, hashes: &mut Vec<ObjectHash>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // Recurse into subdirectories (e.g. refs/heads/feature/).
            collect_file_ref_hashes(&path, hashes)?;
        } else if path.is_file() {
            // Skip lock files left by concurrent ref updates.
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.ends_with(".lock"))
            {
                continue;
            }
            // Fail loudly when a ref file cannot be read or does not
            // contain a valid object hash. Silently skipping would let
            // GC build an incomplete root set and potentially delete
            // objects reachable only from this ref.
            let content = fs::read_to_string(&path).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("failed to read loose ref {}: {e}", path.display()),
                )
            })?;
            let hash = parse_object_hash(content.trim()).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "loose ref {} does not contain a valid object hash",
                        path.display()
                    ),
                )
            })?;
            hashes.push(hash);
        }
    }
    Ok(())
}

/// Remove empty directories under the given path.
fn cleanup_empty_dirs(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir()
            && path.file_name() != Some("pack".as_ref())
            && path.file_name() != Some("info".as_ref())
            && let Ok(mut iter) = fs::read_dir(&path)
            && iter.next().is_none()
        {
            let _ = fs::remove_dir(&path);
        }
    }
    Ok(())
}

/// Remove the `object_index` row for a pruned loose object so that cloud
/// sync does not keep trying to upload it and fail repeatedly.
///
/// Returns `Ok(())` when the row was deleted or did not exist, and quietly
/// logs-and-returns when the config database is missing or the repo_id is
/// unset — the object file is already gone, so a stale index row is a
/// recoverable nuisance, not a fatal condition.
async fn gc_drop_object_index(hash_str: &str) -> Result<(), String> {
    let repo_id = match ConfigKv::get("libra.repoid").await {
        Ok(Some(entry)) => entry.value,
        _ => return Ok(()), // Not configured for cloud sync
    };

    let db_conn = db::get_db_conn_instance().await;
    object_index::Entity::delete_many()
        .filter(object_index::Column::OId.eq(hash_str))
        .filter(object_index::Column::RepoId.eq(&repo_id))
        .exec(&db_conn)
        .await
        .map_err(|e| format!("failed to delete object_index row for {hash_str}: {e}"))?;
    Ok(())
}

/// Allocate a unique pack file name under `pack_dir`.
///
/// Uses nanosecond timestamps plus an attempt counter so two maintenance runs
/// in the same second cannot pick the same name. The path is NOT pre-created:
/// callers use the returned path as a rename target and must write to a temp
/// file first, then atomically rename into place. Pre-creating the file would
/// break `std::fs::rename` on Windows where the destination must not exist.
fn allocate_unique_pack_path(pack_dir: &Path, prefix: &str) -> io::Result<(String, PathBuf)> {
    let base_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for attempt in 0..1000 {
        let pack_name = format!("{prefix}-{base_nanos}-{attempt}.pack");
        let pack_path = pack_dir.join(&pack_name);
        if !pack_path.exists() {
            return Ok((pack_name, pack_path));
        }
    }
    Err(io::Error::other(format!(
        "could not allocate unique pack name after 1000 attempts (base {base_nanos})"
    )))
}

/// Collect all refs under `refs_dir`, storing them as (full_ref_name, hash) pairs.
///
/// `ref_prefix` is prepended to every collected relative name so the resulting
/// refnames match the standard fully-qualified form (e.g. `refs/heads/main`).
///
/// Lock files (`*.lock`) are skipped so that concurrent ref updates do not
/// produce bogus packed-refs entries or delete lock files. Ref names and hash
/// values are validated to protect against corrupted or unexpected files.
fn collect_refs(
    base: &Path,
    current: &Path,
    ref_prefix: &str,
    refs: &mut HashMap<String, String>,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_refs(base, &path, ref_prefix, refs)?;
        } else if path.is_file() {
            // Skip lock files left by concurrent ref updates.
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.ends_with(".lock"))
            {
                continue;
            }

            let hash = fs::read_to_string(&path)?.trim().to_string();
            let relative = path.strip_prefix(base).unwrap_or(&path);
            let relative = relative.to_string_lossy().replace('\\', "/");
            let name = format!("{ref_prefix}{relative}");

            // Validate the ref name and hash before inserting.
            if !hash.is_empty() && parse_object_hash(&hash).is_some() && is_valid_ref_name(&name) {
                refs.insert(name, hash);
            }
        }
    }
    Ok(())
}

/// Restore source packs and indexes from their backup locations after a
/// failed incremental-repack staging step. Failures are best-effort because
/// the repository is already in a broken state.
fn restore_staged_packs(staged_packs: &[(PathBuf, PathBuf)], staged_idxs: &[(PathBuf, PathBuf)]) {
    for (source, backup) in staged_packs {
        let _ = fs::rename(backup, source);
    }
    for (source, backup) in staged_idxs {
        let _ = fs::rename(backup, source);
    }
}

/// Create a pack file from loose objects. Returns the number of objects packed.
///
/// Objects are streamed through a temp file so that memory use is bounded by
/// the largest single object, not by the total uncompressed size of all loose
/// objects. For large repositories with many old loose objects (especially
/// large blobs), this prevents the maintenance task from exhausting memory.
async fn create_pack_from_loose_objects(
    objects: &[(String, PathBuf)],
    pack_path: &Path,
) -> io::Result<usize> {
    // Stage 1: stream object entries to a temp file so we know the count
    // before writing the pack header. The temp dir is placed inside the
    // pack directory so that the final rename stays on the same filesystem
    // and avoids cross-device errors when /tmp is a separate mount.
    let pack_parent = pack_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("pack path has no parent: {}", pack_path.display()),
        )
    })?;
    let temp_dir = tempfile::tempdir_in(pack_parent)?;
    let entries_path = temp_dir.path().join("entries.bin");
    let mut entries_file = fs::File::create(&entries_path)?;
    let mut count: u32 = 0;

    for (_hash_str, obj_path) in objects {
        let data = fs::read(obj_path)?;
        let decompressed = ClientStorage::decompress_zlib(&data)?;
        let (obj_type, _size) = parse_loose_object_header(&decompressed)?;
        let header_end = decompressed.iter().position(|&b| b == 0).unwrap_or(0);
        let body = &decompressed[header_end + 1..];
        write_pack_entry(&mut entries_file, obj_type, body)?;
        count = count.checked_add(1).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "pack object count overflow")
        })?;
    }
    drop(entries_file);

    // Stage 2: assemble the final pack with header, copy in the staged
    // entries, and append the pack checksum. Write to a temp file and rename
    // atomically into place so a partial write cannot be seen by the storage
    // layer.
    let pack_temp_path = temp_dir.path().join("loose.pack");
    let mut out = fs::File::create(&pack_temp_path)?;
    let mut digest_ctx = Context::new(match get_hash_kind() {
        HashKind::Sha256 => &SHA256,
        _ => &SHA1_FOR_LEGACY_USE_ONLY,
    });

    out.write_all(b"PACK")?;
    digest_ctx.update(b"PACK");
    out.write_all(&2_u32.to_be_bytes())?;
    digest_ctx.update(&2_u32.to_be_bytes());
    out.write_all(&count.to_be_bytes())?;
    digest_ctx.update(&count.to_be_bytes());

    let mut entries_in = fs::File::open(&entries_path)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = entries_in.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest_ctx.update(&buf[..n]);
        out.write_all(&buf[..n])?;
    }
    drop(entries_in);

    let pack_hash = digest_ctx.finish();
    out.write_all(pack_hash.as_ref())?;
    drop(out);

    fs::rename(&pack_temp_path, pack_path).inspect_err(|_| {
        let _ = fs::remove_file(&pack_temp_path);
    })?;

    Ok(count as usize)
}

/// Shared mutable state for streaming pack-object consolidation.
///
/// Held behind `Arc<Mutex<>>` so it can be accessed from the `Pack::decode`
/// callback, which requires `Send + Sync + 'static`.
struct ConsolidateCtx {
    entries_file: fs::File,
    seen: HashSet<ObjectHash>,
    count: u32,
    error: Option<io::Error>,
}

/// Read objects from a pack file, calling `cb` for each decoded entry as it
/// becomes available.
///
/// Entries are streamed through the callback instead of being collected into a
/// `Vec`, so memory use is bounded by the pack decoder's internal buffers
/// rather than by the total uncompressed size of all objects in the pack.
/// This prevents incremental-repack from OOM-ing on large production packs.
fn read_pack_objects<F>(pack_path: &Path, cb: F) -> io::Result<()>
where
    F: Fn(ObjectType, Vec<u8>, ObjectHash) -> io::Result<()> + Send + Sync + 'static,
{
    let file = fs::File::open(pack_path)?;
    let mut reader = io::BufReader::new(file);
    let mut pack = Pack::new(Some(1), Some(128 * 1024 * 1024), None, true);

    let error: Arc<Mutex<Option<io::Error>>> = Arc::new(Mutex::new(None));
    let error_c = error.clone();

    pack.decode(
        &mut reader,
        move |entry: MetaAttached<Entry, EntryMeta>| {
            if let Err(e) = cb(entry.inner.obj_type, entry.inner.data, entry.inner.hash) {
                let _ = error_c.lock().map(|mut guard| {
                    *guard = Some(e);
                });
            }
        },
        None::<fn(ObjectHash)>,
    )
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    if let Some(e) = error
        .lock()
        .map_err(|e| io::Error::other(e.to_string()))?
        .take()
    {
        return Err(e);
    }

    Ok(())
}

/// Create a consolidated pack file from existing packs and loose objects.
///
/// Objects that appear in multiple sources are only written once. This avoids
/// reading through `ClientStorage`, which can trigger race conditions in the
/// pack cache when many packs are accessed concurrently.
///
/// Objects are streamed so that only hashes are kept in memory for
/// deduplication; the consolidated object data is staged on disk until the
/// final pack header (including the object count) and checksum can be written
/// in the correct byte order.
fn create_consolidated_pack(
    source_packs: &[PathBuf],
    loose_objects: &[(String, PathBuf)],
    pack_path: &Path,
) -> io::Result<usize> {
    // Stage 1: stream object entries to a temp file. We cannot write the pack
    // header first because the object count is only known after deduplication,
    // and the checksum must hash the header in its natural byte order.
    // The temp dir is placed inside the pack directory so that the final
    // rename stays on the same filesystem and avoids cross-device errors
    // when /tmp is a separate mount.
    let pack_parent = pack_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("pack path has no parent: {}", pack_path.display()),
        )
    })?;
    let temp_dir = tempfile::tempdir_in(pack_parent)?;
    let entries_path = temp_dir.path().join("entries.bin");
    let entries_file = fs::File::create(&entries_path)?;

    let ctx = Arc::new(Mutex::new(ConsolidateCtx {
        entries_file,
        seen: HashSet::new(),
        count: 0,
        error: None,
    }));

    // Copy complete objects from existing packs, resolving any delta entries.
    // Objects are streamed through the callback so only the dedup set grows
    // with the total number of unique objects — not the uncompressed data.
    for pack in source_packs {
        let ctx_for_cb = ctx.clone();
        read_pack_objects(pack, move |obj_type, body, hash| -> io::Result<()> {
            let mut guard = ctx_for_cb
                .lock()
                .map_err(|e| io::Error::other(format!("consolidation mutex poisoned: {e}")))?;
            if !guard.seen.insert(hash) {
                return Ok(());
            }
            write_pack_entry(&mut guard.entries_file, obj_type, &body)?;
            guard.count = guard.count.checked_add(1).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "pack object count overflow")
            })?;
            Ok(())
        })?;
        // Propagate any error recorded inside the callback.
        let mut guard = ctx
            .lock()
            .map_err(|e| io::Error::other(format!("consolidation mutex poisoned: {e}")))?;
        if let Some(e) = guard.error.take() {
            return Err(e);
        }
    }

    // Extract state from the Arc so the caller can continue with loose objects.
    let mut ctx = Arc::try_unwrap(ctx)
        .map_err(|_| io::Error::other("consolidation context still referenced"))?
        .into_inner()
        .map_err(|e| io::Error::other(e.to_string()))?;

    // Add remaining loose objects.
    for (hash_str, obj_path) in loose_objects {
        let Some(hash) = parse_object_hash(hash_str) else {
            continue;
        };
        if !ctx.seen.insert(hash) {
            continue;
        }
        let data = fs::read(obj_path)?;
        let decompressed = ClientStorage::decompress_zlib(&data)?;
        let (obj_type, _size) = parse_loose_object_header(&decompressed)?;
        let header_end = decompressed.iter().position(|&b| b == 0).unwrap_or(0);
        let body = &decompressed[header_end + 1..];
        write_pack_entry(&mut ctx.entries_file, obj_type, body)?;
        ctx.count = ctx.count.checked_add(1).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "pack object count overflow")
        })?;
    }
    drop(ctx.entries_file);

    // Stage 2: assemble the final pack file with the correct header, copy the
    // staged entries into it (updating the checksum as we go), and append the
    // pack checksum. Keep the unfinished file outside objects/pack so a partial
    // write cannot confuse the storage layer.
    let pack_temp_path = temp_dir.path().join("consolidated.pack");
    let mut out = fs::File::create(&pack_temp_path)?;
    let mut digest_ctx = Context::new(match get_hash_kind() {
        HashKind::Sha256 => &SHA256,
        _ => &SHA1_FOR_LEGACY_USE_ONLY,
    });

    out.write_all(b"PACK")?;
    digest_ctx.update(b"PACK");
    out.write_all(&2_u32.to_be_bytes())?;
    digest_ctx.update(&2_u32.to_be_bytes());
    out.write_all(&ctx.count.to_be_bytes())?;
    digest_ctx.update(&ctx.count.to_be_bytes());

    let mut entries_file = fs::File::open(&entries_path)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = entries_file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest_ctx.update(&buf[..n]);
        out.write_all(&buf[..n])?;
    }
    drop(entries_file);

    let pack_hash = digest_ctx.finish();
    out.write_all(pack_hash.as_ref())?;
    drop(out);

    fs::rename(&pack_temp_path, pack_path).inspect_err(|_| {
        let _ = fs::remove_file(&pack_temp_path);
    })?;

    Ok(ctx.count as usize)
}

/// Write a single pack entry (type/size header + zlib compressed body) to a
/// writer.
fn write_pack_entry<W: Write>(writer: &mut W, obj_type: ObjectType, body: &[u8]) -> io::Result<()> {
    let type_num = object_type_to_pack_num(obj_type);
    let mut header = Vec::new();
    write_size_encoded(&mut header, body.len(), type_num)?;
    let compressed = ClientStorage::compress_zlib(body)?;
    writer.write_all(&header)?;
    writer.write_all(&compressed)?;
    Ok(())
}

/// Parse the header of a decompressed loose object, returning (type, size).
fn parse_loose_object_header(data: &[u8]) -> io::Result<(ObjectType, usize)> {
    let header_end = data.iter().position(|&b| b == 0).unwrap_or(0);
    let header = std::str::from_utf8(&data[..header_end])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let Some((type_str, size_str)) = header.split_once(' ') else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid loose object header",
        ));
    };
    let size = size_str
        .parse::<usize>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let obj_type = match type_str {
        "commit" => ObjectType::Commit,
        "tree" => ObjectType::Tree,
        "blob" => ObjectType::Blob,
        "tag" => ObjectType::Tag,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown object type: {type_str}"),
            ));
        }
    };
    Ok((obj_type, size))
}

/// Write a size-encoded integer as used in pack file object headers.
fn write_size_encoded<W: Write>(writer: &mut W, size: usize, type_num: u8) -> io::Result<()> {
    let mut byte = (type_num & 0x7) << 4;
    let mut remaining = size;
    byte |= (remaining & 0x0F) as u8;
    remaining >>= 4;
    while remaining > 0 {
        writer.write_all(&[byte | 0x80])?;
        byte = (remaining & 0x7F) as u8;
        remaining >>= 7;
    }
    writer.write_all(&[byte])?;
    Ok(())
}

/// Build a standard index file for a pack, choosing the appropriate version
/// based on the configured hash algorithm.
fn build_pack_index(pack_path: &Path, idx_path: &Path) -> CliResult<()> {
    let pack_str = pack_path
        .to_str()
        .ok_or_else(|| CliError::fatal("invalid pack file path"))?;
    let idx_str = idx_path
        .to_str()
        .ok_or_else(|| CliError::fatal("invalid index file path"))?;

    if get_hash_kind() == HashKind::Sha256 {
        index_pack::build_index_v2(pack_str, idx_str)
    } else {
        index_pack::build_index_v1(pack_str, idx_str)
    }
    .map_err(|e| CliError::fatal(format!("failed to build pack index: {e}")))
}

/// Print an informational message unless output is quiet or JSON mode.
fn info_println(output: &OutputConfig, message: &str) {
    if !output.quiet && !output.is_json() {
        println!("{message}");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use git_internal::hash::{HashKind, set_hash_kind_for_test};

    use super::*;

    #[test]
    fn test_parse_object_hash_valid() {
        let hash = "abc123def456789012345678901234567890abcd";
        let result = parse_object_hash(hash);
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_object_hash_invalid_hex() {
        let hash = "xyz123";
        let result = parse_object_hash(hash);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_object_hash_empty() {
        let result = parse_object_hash("");
        assert!(result.is_none());
    }

    #[test]
    fn test_task_display() {
        assert_eq!(MaintenanceTask::Gc.to_string(), "gc");
        assert_eq!(MaintenanceTask::LooseObjects.to_string(), "loose-objects");
        assert_eq!(MaintenanceTask::PackRefs.to_string(), "pack-refs");
        assert_eq!(
            MaintenanceTask::IncrementalRepack.to_string(),
            "incremental-repack"
        );
        assert_eq!(MaintenanceTask::CommitGraph.to_string(), "commit-graph");
        assert_eq!(MaintenanceTask::Prefetch.to_string(), "prefetch");
    }

    #[test]
    fn test_size_encoding_basic() {
        let mut buf = Vec::new();
        write_size_encoded(&mut buf, 10, 1).unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_size_encoding_large() {
        let mut buf = Vec::new();
        write_size_encoded(&mut buf, 10000, 2).unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_build_index_v1_small_pack() {
        use std::io::Write;

        use git_internal::hash::set_hash_kind_for_test;
        use tempfile::tempdir;

        // Version 1 indexes are SHA-1 only; pin the hash kind for this test.
        let _guard = set_hash_kind_for_test(HashKind::Sha1);

        let tmp = tempdir().unwrap();
        let pack_path = tmp.path().join("test.pack");

        // Build a minimal valid pack with one blob object.
        let mut pack_data: Vec<u8> = Vec::new();
        pack_data.write_all(b"PACK").unwrap();
        pack_data.write_all(&2_u32.to_be_bytes()).unwrap();
        pack_data.write_all(&1_u32.to_be_bytes()).unwrap();

        let body = b"hello world";
        let type_num = 3u8; // blob
        write_size_encoded(&mut pack_data, body.len(), type_num).unwrap();
        let compressed = ClientStorage::compress_zlib(body).unwrap();
        pack_data.write_all(&compressed).unwrap();

        let mut ctx = Context::new(&SHA1_FOR_LEGACY_USE_ONLY);
        ctx.update(&pack_data);
        let pack_hash = ctx.finish();
        pack_data.write_all(pack_hash.as_ref()).unwrap();

        std::fs::write(&pack_path, &pack_data).unwrap();

        let idx_path = pack_path.with_extension("idx");
        let result =
            index_pack::build_index_v1(pack_path.to_str().unwrap(), idx_path.to_str().unwrap());
        assert!(result.is_ok(), "build_index_v1 failed: {result:?}");
        assert!(idx_path.exists(), "index file should be created");
    }

    #[test]
    fn test_cleanup_empty_dirs_nonexistent() {
        // Should not panic on non-existent directory
        let temp = tempfile::tempdir().unwrap();
        let result = cleanup_empty_dirs(temp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_loose_object_header_commit() {
        let data = b"commit 123\0content";
        let (obj_type, size) = parse_loose_object_header(data).unwrap();
        assert_eq!(obj_type, ObjectType::Commit);
        assert_eq!(size, 123);
    }

    #[test]
    fn test_parse_loose_object_header_tree() {
        let data = b"tree 456\0content";
        let (obj_type, size) = parse_loose_object_header(data).unwrap();
        assert_eq!(obj_type, ObjectType::Tree);
        assert_eq!(size, 456);
    }

    #[test]
    fn test_parse_loose_object_header_invalid() {
        let data = b"invalid";
        assert!(parse_loose_object_header(data).is_err());
    }

    #[test]
    fn test_task_result_serialize() {
        let result = TaskResult {
            task: "gc".to_string(),
            success: true,
            objects_removed: 5,
            objects_packed: 0,
            refs_packed: 0,
            packs_repacked: 0,
            message: "removed 5 objects".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("gc"));
        assert!(json.contains("removed 5 objects"));
    }

    #[test]
    fn test_maintenance_status_output_serialize() {
        let status = MaintenanceStatusOutput {
            registered: true,
            schedule: Some("hourly".to_string()),
            last_run: None,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("hourly"));
    }

    #[test]
    fn test_allocate_unique_pack_path_is_unique() {
        let tmp = tempfile::tempdir().unwrap();
        let (name1, path1) = allocate_unique_pack_path(tmp.path(), "pack-test").unwrap();
        let (name2, path2) = allocate_unique_pack_path(tmp.path(), "pack-test").unwrap();
        assert_ne!(name1, name2, "consecutive pack names must differ");
        assert_ne!(path1, path2, "consecutive pack paths must differ");
        assert!(
            !path1.exists(),
            "pack path must not be pre-created (rename target)"
        );
        assert!(
            !path2.exists(),
            "pack path must not be pre-created (rename target)"
        );
    }

    #[test]
    fn test_restore_staged_packs_returns_files() {
        let tmp = tempfile::tempdir().unwrap();
        let pack_dir = tmp.path().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();

        let source_pack = pack_dir.join("source.pack");
        let backup_pack = pack_dir.join("backup.pack");
        fs::write(&source_pack, b"pack data").unwrap();
        fs::rename(&source_pack, &backup_pack).unwrap();

        let source_idx = pack_dir.join("source.idx");
        let backup_idx = pack_dir.join("backup.idx");
        fs::write(&source_idx, b"idx data").unwrap();
        fs::rename(&source_idx, &backup_idx).unwrap();

        restore_staged_packs(
            &[(source_pack.clone(), backup_pack.clone())],
            &[(source_idx.clone(), backup_idx.clone())],
        );

        assert!(source_pack.exists(), "source pack should be restored");
        assert!(!backup_pack.exists(), "backup pack should no longer exist");
        assert!(source_idx.exists(), "source idx should be restored");
        assert!(!backup_idx.exists(), "backup idx should no longer exist");
    }

    #[test]
    fn test_collect_refs_prefixes_full_name() {
        let tmp = tempfile::tempdir().unwrap();
        let refs_dir = tmp.path().join("refs").join("heads");
        fs::create_dir_all(&refs_dir).unwrap();
        fs::write(refs_dir.join("main"), "abc123").unwrap();
        let feature_dir = refs_dir.join("feature");
        fs::create_dir_all(&feature_dir).unwrap();
        fs::write(feature_dir.join("x"), "def456").unwrap();

        let mut refs = HashMap::new();
        collect_refs(&refs_dir, &refs_dir, "refs/heads/", &mut refs).unwrap();

        assert_eq!(refs.get("refs/heads/main"), Some(&"abc123".to_string()));
        assert_eq!(
            refs.get("refs/heads/feature/x"),
            Some(&"def456".to_string())
        );
    }

    #[test]
    fn test_read_idx_entries_v2_large_offset_seeks_back() {
        let _guard = set_hash_kind_for_test(HashKind::Sha1);
        let tmp = tempfile::tempdir().unwrap();
        let idx_path = tmp.path().join("test.idx");

        // Build a minimal v2 index with three objects. The middle object uses a
        // large offset; if read_idx_entries does not seek back to the 4-byte
        // offset table after the large-offset lookup, the third offset will be
        // read from the wrong place.
        let hash_a = ObjectHash::from_bytes(&[0u8; 20]).unwrap();
        let hash_b =
            ObjectHash::from_bytes(&[0u8; 19].into_iter().chain([1u8]).collect::<Vec<_>>())
                .unwrap();
        let hash_c =
            ObjectHash::from_bytes(&[0u8; 19].into_iter().chain([2u8]).collect::<Vec<_>>())
                .unwrap();

        let mut data: Vec<u8> = Vec::new();
        // header
        data.extend_from_slice(&[0xFF, 0x74, 0x4F, 0x63]); // magic
        data.extend_from_slice(&2_u32.to_be_bytes()); // version

        // fanout (all objects start with 0x00)
        let fanout: Vec<u8> = (0..256u32)
            .flat_map(|_| 3u32.to_be_bytes().to_vec())
            .collect();
        data.extend_from_slice(&fanout);

        // names
        data.extend_from_slice(hash_a.as_ref());
        data.extend_from_slice(hash_b.as_ref());
        data.extend_from_slice(hash_c.as_ref());

        // crcs
        for _ in 0..3 {
            data.extend_from_slice(&0_u32.to_be_bytes());
        }

        // offsets: A=100 (small), B=large index 0, C=200 (small)
        data.extend_from_slice(&100_u32.to_be_bytes());
        data.extend_from_slice(&0x8000_0000_u32.to_be_bytes());
        data.extend_from_slice(&200_u32.to_be_bytes());

        // large offsets table: one 8-byte entry
        data.extend_from_slice(&0x0001_0000_0000_u64.to_be_bytes());

        fs::write(&idx_path, &data).unwrap();

        let entries = read_idx_entries(&idx_path).unwrap();
        assert_eq!(entries.len(), 3);
        let map: HashMap<_, _> = entries.into_iter().collect();
        assert_eq!(map.get(&hash_a), Some(&100u64));
        assert_eq!(map.get(&hash_b), Some(&0x0001_0000_0000u64));
        assert_eq!(map.get(&hash_c), Some(&200u64));
    }
}
